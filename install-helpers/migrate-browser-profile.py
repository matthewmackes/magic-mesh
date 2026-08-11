#!/usr/bin/env python3
"""Safely stage portable Browser VM data without exporting credentials.

The Browser VM only receives an explicit portable bundle.  This helper walks
the requested legacy roots, copies an allowlist of bookmarks/history/session
data, downloads, policies, and extension payloads, and writes a deterministic
manifest.  Credential stores are deliberately classified and reported as
skipped; they are never read or copied.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
from pathlib import PurePosixPath
import shutil
import stat
import tempfile
from typing import Iterable


SCHEMA_VERSION = 1
MAX_FILE_BYTES = 128 * 1024 * 1024
MAX_TOTAL_BYTES = 512 * 1024 * 1024
MAX_FILES = 10_000

DENY_NAMES = {
    "cookies",
    "cookies-journal",
    "login data",
    "login data-journal",
    "web data",
    "web data-journal",
    "key4.db",
    "logins.json",
    "credentials.json",
    "passkeys",
    "passkeys.json",
    "private_keys",
    "private-keys",
    "local state",
    "local storage",
    "session storage",
    "managed storage",
}
DENY_PARTS = (
    "credential",
    "password",
    "passkey",
    "private_key",
    "private-key",
    "secret",
    "token",
)


class MigrationError(RuntimeError):
    pass


def source_snapshot(path: Path) -> tuple[int, str, tuple[int, int, int, int, int]]:
    """Hash one regular source without following a last-moment symlink swap."""

    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode):
            raise MigrationError(f"source is not a regular file: {path}")
        digest = hashlib.sha256()
        with os.fdopen(os.dup(descriptor), "rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
        after = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    identity = (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns)
    if identity != (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns):
        raise MigrationError(f"source changed while it was read: {path}")
    current = path.lstat()
    if identity != (current.st_dev, current.st_ino, current.st_size, current.st_mtime_ns, current.st_ctime_ns):
        raise MigrationError(f"source identity changed while it was read: {path}")
    return after.st_size, digest.hexdigest(), identity


def copy_verified(
    source: Path,
    destination: Path,
    expected_size: int,
    expected_sha256: str,
    expected_identity: tuple[int, int, int, int, int],
) -> None:
    """Copy the exact bytes inventoried above or refuse the whole bundle."""

    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(source, flags)
    try:
        before = os.fstat(descriptor)
        identity = (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns)
        if not stat.S_ISREG(before.st_mode) or identity != expected_identity:
            raise MigrationError(f"source changed before copy: {source}")
        digest = hashlib.sha256()
        copied = 0
        with os.fdopen(os.dup(descriptor), "rb") as source_handle, destination.open("xb") as output_handle:
            for chunk in iter(lambda: source_handle.read(1024 * 1024), b""):
                output_handle.write(chunk)
                digest.update(chunk)
                copied += len(chunk)
        after = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    final_identity = (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns)
    if final_identity != expected_identity or copied != expected_size or digest.hexdigest() != expected_sha256:
        raise MigrationError(f"source changed during copy: {source}")
    current = source.lstat()
    if expected_identity != (current.st_dev, current.st_ino, current.st_size, current.st_mtime_ns, current.st_ctime_ns):
        raise MigrationError(f"source identity changed during copy: {source}")


def relative_safe(path: Path, root: Path) -> str:
    try:
        relative = path.relative_to(root)
    except ValueError as error:
        raise MigrationError(f"path escaped its source root: {path}") from error
    if not relative.parts or any(part in ("", ".", "..") for part in relative.parts):
        raise MigrationError(f"unsafe relative path: {relative}")
    return "/".join(relative.parts)


def denied(relative: str) -> str | None:
    parts = relative.split("/")
    lowered = [part.casefold() for part in parts]
    for part in lowered:
        if part in DENY_NAMES:
            return "credential-bearing-store"
        if any(marker in part for marker in DENY_PARTS):
            return "credential-bearing-name"
    return None


def iter_files(root: Path) -> Iterable[tuple[Path, str]]:
    """Yield every non-directory entry beneath *root* without following links.

    ``os.walk`` omits FIFOs, sockets, and device nodes.  Omitting one from the
    inventory would make a migration appear successful while silently losing a
    source entry, so special nodes are yielded and rejected by ``migrate`` as
    non-regular files instead.
    """

    if not root.exists():
        return
    if root.is_symlink() or not root.is_dir():
        raise MigrationError(f"source root is not a real directory: {root}")

    def walk(directory: Path) -> Iterable[tuple[Path, str]]:
        try:
            entries = sorted(os.scandir(directory), key=lambda entry: entry.name)
        except OSError as error:
            raise MigrationError(f"source directory is unreadable: {directory}") from error
        for entry in entries:
            path = Path(entry.path)
            relative = relative_safe(path, root)
            try:
                if entry.is_symlink():
                    yield path, relative
                elif entry.is_dir(follow_symlinks=False):
                    yield from walk(path)
                else:
                    # Includes regular files and special nodes.  The caller
                    # records a failed entry for anything non-regular.
                    yield path, relative
            except OSError as error:
                raise MigrationError(f"source entry is unreadable: {path}") from error

    yield from walk(root)


def profile_candidates(root: Path) -> Iterable[tuple[Path, str, str | None]]:
    """Return (path, relative, category) for the portable profile allowlist."""

    for path, relative in iter_files(root):
        if "/" not in relative:
            category = {
                "Bookmarks": "bookmarks",
                "Bookmarks.bak": "bookmarks",
                "History": "history",
                "History-journal": "history",
                "Current Session": "sessions",
                "Current Tabs": "sessions",
                "Last Session": "sessions",
                "Last Tabs": "sessions",
            }.get(relative)
            yield path, relative, category
        elif relative.startswith("Sessions/"):
            yield path, relative, "sessions"
        elif relative.startswith("Extensions/"):
            yield path, relative, "extensions"
        else:
            yield path, relative, None


def all_candidates(root: Path, category: str) -> Iterable[tuple[Path, str, str]]:
    for path, relative in iter_files(root):
        yield path, relative, category


def output_name(category: str, relative: str) -> Path:
    return Path("payload") / category / Path(relative)


def validate_roots(roots: list[tuple[Path, str]], output: Path) -> None:
    output_absolute = output.absolute()
    for root, _category in roots:
        root_absolute = root.absolute()
        current = Path(root_absolute.anchor)
        relative_parts = root_absolute.relative_to(root_absolute.anchor).parts
        for part in relative_parts:
            current /= part
            try:
                metadata = current.lstat()
            except FileNotFoundError:
                # A missing suffix is handled by migrate as source-missing;
                # every existing ancestor has still been checked for
                # redirection before that decision is made.
                break
            except OSError as error:
                raise MigrationError(f"source root is unreadable: {current}") from error
            if stat.S_ISLNK(metadata.st_mode):
                raise MigrationError(f"source root path must not contain a symlink: {current}")
            if not stat.S_ISDIR(metadata.st_mode):
                raise MigrationError(f"source root path is not a directory: {current}")
        if root_absolute == output_absolute or root_absolute in output_absolute.parents:
            raise MigrationError("output must not be inside a source root")


def validate_output_parent(output: Path) -> None:
    """Refuse an existing symlink or non-directory in the output path.

    The bundle is published with ``os.replace`` only after staging, but a
    symlinked parent would still redirect the staging directory and final
    bundle outside the caller-selected destination.  Inspect without
    resolving so the portable boundary remains explicit and fail-closed.
    """

    parent = output.absolute().parent
    current = Path(parent.anchor)
    relative_parts = parent.relative_to(parent.anchor).parts
    for part in relative_parts:
        current /= part
        try:
            metadata = current.lstat()
        except FileNotFoundError:
            # Missing descendants are created below this point.  Existing
            # ancestors have already been checked and are the only paths that
            # can redirect this migration before mkdir.
            break
        except OSError as error:
            raise MigrationError(f"output parent is unreadable: {current}") from error
        if stat.S_ISLNK(metadata.st_mode):
            raise MigrationError(f"output parent must not contain a symlink: {current}")
        if not stat.S_ISDIR(metadata.st_mode):
            raise MigrationError(f"output parent is not a directory: {current}")


def manifest_for(output: Path) -> dict:
    manifest_path = output / "manifest.json"
    if not manifest_path.is_file() or manifest_path.is_symlink():
        raise MigrationError("existing output is not a migration bundle")
    try:
        data = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise MigrationError("existing output manifest is unreadable") from error
    if data.get("schema_version") != SCHEMA_VERSION or data.get("kind") != "browser_vm_portable_bundle":
        raise MigrationError("existing output has an unknown migration schema")
    return data


def bundle_payload_path(output: Path, relative: object) -> Path:
    """Resolve one manifest output without permitting bundle-root escape."""

    if not isinstance(relative, str) or not relative:
        raise MigrationError("existing bundle has an invalid payload identity")
    portable = PurePosixPath(relative)
    if portable.is_absolute() or portable.parts[0] != "payload" or any(
        part in ("", ".", "..") for part in portable.parts
    ):
        raise MigrationError("existing bundle has an unsafe payload identity")
    destination = output.joinpath(*portable.parts)
    cursor = output
    for part in portable.parts[:-1]:
        cursor /= part
        try:
            metadata = cursor.lstat()
        except OSError as error:
            raise MigrationError(
                f"existing bundle payload directory is unreadable: {relative}"
            ) from error
        if not stat.S_ISDIR(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
            raise MigrationError(f"existing bundle payload directory is unsafe: {relative}")
    return destination


def verify_existing_bundle(output: Path, manifest: dict) -> None:
    """Require manifest bytes and the private payload tree to agree exactly."""

    entries = manifest.get("entries")
    if not isinstance(entries, list):
        raise MigrationError("existing bundle manifest entries are invalid")
    expected_files = {"manifest.json"}
    for entry in entries:
        if not isinstance(entry, dict):
            raise MigrationError("existing bundle manifest entry is invalid")
        if entry.get("status") != "imported":
            if "output" in entry:
                raise MigrationError("existing bundle skipped entry names a payload")
            continue
        relative = entry.get("output")
        destination = bundle_payload_path(output, relative)
        expected_size = entry.get("bytes")
        expected_sha256 = entry.get("sha256")
        if (
            not isinstance(expected_size, int)
            or isinstance(expected_size, bool)
            or expected_size < 0
            or not isinstance(expected_sha256, str)
            or len(expected_sha256) != 64
        ):
            raise MigrationError("existing bundle manifest payload metadata is invalid")
        try:
            int(expected_sha256, 16)
        except ValueError as error:
            raise MigrationError("existing bundle manifest payload digest is invalid") from error
        try:
            actual_size, actual_sha256, _identity = source_snapshot(destination)
        except (OSError, MigrationError) as error:
            raise MigrationError(
                f"existing bundle payload is unreadable: {relative}"
            ) from error
        if actual_size != expected_size or actual_sha256 != expected_sha256:
            raise MigrationError(
                f"existing bundle payload differs from its manifest: {relative}"
            )
        if relative in expected_files:
            raise MigrationError("existing bundle contains duplicate payload identities")
        expected_files.add(relative)

    actual_files = set()
    for path, relative in iter_files(output):
        if path.is_symlink():
            raise MigrationError(f"existing bundle contains a symlink: {relative}")
        actual_files.add(relative)
    if actual_files != expected_files:
        raise MigrationError("existing bundle contains missing or unexpected files")


def migrate(roots: list[tuple[Path, str]], output: Path, replace: bool = False) -> dict:
    validate_roots(roots, output)
    validate_output_parent(output)
    if output.exists() and output.is_symlink():
        raise MigrationError("output must not be a symlink")
    if output.exists() and not output.is_dir():
        raise MigrationError("output is not a directory")
    if output.exists() and not replace:
        previous = manifest_for(output)
        verify_existing_bundle(output, previous)
    else:
        previous = None

    entries: list[dict | tuple[Path, dict, tuple[int, int, int, int, int]]] = []
    seen_outputs: set[str] = set()
    total_bytes = 0
    candidates = 0

    for root, category in roots:
        if not root.exists():
            entries.append(
                {"category": category, "path": ".", "status": "skipped", "reason": "source-missing"}
            )
            continue
        iterator = profile_candidates(root) if category == "profile" else all_candidates(root, category)
        for path, relative, actual_category in iterator:
            candidates += 1
            record = {"category": actual_category or "profile", "path": relative}
            if path.is_symlink():
                record.update(status="skipped", reason="symlink-rejected")
                entries.append(record)
                continue
            reason = denied(relative)
            if reason:
                record.update(status="skipped", reason=reason)
                entries.append(record)
                continue
            if actual_category is None:
                record.update(status="skipped", reason="unsupported-profile-entry")
                entries.append(record)
                continue
            try:
                metadata = path.lstat()
            except OSError as error:
                record.update(status="failed", reason=f"stat-failed:{error.__class__.__name__}")
                entries.append(record)
                continue
            if not stat.S_ISREG(metadata.st_mode):
                record.update(status="failed", reason="not-regular-file")
                entries.append(record)
                continue
            size = metadata.st_size
            initial_identity = (
                metadata.st_dev,
                metadata.st_ino,
                metadata.st_size,
                metadata.st_mtime_ns,
                metadata.st_ctime_ns,
            )
            if size > MAX_FILE_BYTES:
                record.update(status="failed", reason="file-too-large", bytes=size)
                entries.append(record)
                continue
            if total_bytes + size > MAX_TOTAL_BYTES:
                record.update(status="failed", reason="bundle-too-large", bytes=size)
                entries.append(record)
                continue
            destination_relative = output_name(actual_category, relative).as_posix()
            if destination_relative in seen_outputs:
                record.update(status="failed", reason="destination-collision")
                entries.append(record)
                continue
            seen_outputs.add(destination_relative)
            try:
                size, source_sha256, source_identity = source_snapshot(path)
            except (OSError, MigrationError) as error:
                record.update(status="failed", reason=f"source-read-failed:{error.__class__.__name__}")
                entries.append(record)
                continue
            if source_identity != initial_identity:
                record.update(status="failed", reason="source-changed-during-inventory")
                entries.append(record)
                continue
            record.update(
                status="imported",
                bytes=size,
                sha256=source_sha256,
                output=destination_relative,
            )
            total_bytes += size
            entries.append((path, record, source_identity))

    if candidates > MAX_FILES:
        raise MigrationError(f"source contains too many files: {candidates}")
    entries.sort(key=lambda item: (item[1]["category"], item[1]["path"]) if isinstance(item, tuple) else (item["category"], item["path"]))
    serial_entries = [item[1] if isinstance(item, tuple) else item for item in entries]
    counts = {status: sum(entry["status"] == status for entry in serial_entries) for status in ("imported", "skipped", "failed")}
    manifest = {
        "schema_version": SCHEMA_VERSION,
        "kind": "browser_vm_portable_bundle",
        "policy": {
            "credential_stores": "never-export",
            "symlinks": "reject",
            "deterministic": True,
        },
        "counts": counts,
        "bytes": total_bytes,
        "entries": serial_entries,
    }
    encoded = json.dumps(manifest, indent=2, sort_keys=True) + "\n"

    if counts["failed"]:
        raise MigrationError(f"migration refused {counts['failed']} failed source entries")

    if previous is not None and previous != manifest:
        raise MigrationError("existing bundle differs; rerun with --replace after review")
    if previous is not None:
        return manifest

    parent = output.parent
    parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=f".{output.name}.staging-", dir=parent) as staging_raw:
        staging = Path(staging_raw)
        (staging / "payload").mkdir(mode=0o700)
        for item in entries:
            if not isinstance(item, tuple):
                continue
            source, record, source_identity = item
            destination = staging / record["output"]
            destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            try:
                copy_verified(
                    source,
                    destination,
                    record["bytes"],
                    record["sha256"],
                    source_identity,
                )
            except (OSError, MigrationError) as error:
                raise MigrationError(f"source could not be copied safely: {source}") from error
            os.chmod(destination, stat.S_IRUSR | stat.S_IWUSR)
        (staging / "manifest.json").write_text(encoded, encoding="utf-8")
        os.chmod(staging / "manifest.json", stat.S_IRUSR | stat.S_IWUSR)
        verify_existing_bundle(staging, manifest)
        if output.exists():
            if not replace:
                raise MigrationError("output appeared during migration; refusing overwrite")
            shutil.rmtree(output)
        os.replace(staging, output)
    os.chmod(output, stat.S_IRWXU)
    return manifest


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="mcnf-browser-profile-test-") as raw:
        root = Path(raw) / "legacy"
        downloads = Path(raw) / "downloads"
        policies = Path(raw) / "policies"
        output = Path(raw) / "bundle"
        (root / "Sessions").mkdir(parents=True)
        (root / "Extensions" / "ext-redacted").mkdir(parents=True)
        downloads.mkdir()
        policies.mkdir()
        (root / "Bookmarks").write_text('{"roots":{}}\n', encoding="utf-8")
        (root / "History").write_bytes(b"sqlite-history-fixture")
        (root / "Sessions" / "Current Tabs").write_bytes(b"tabs")
        (root / "Extensions" / "ext-redacted" / "manifest.json").write_text('{"name":"fixture"}\n', encoding="utf-8")
        (root / "Cookies").write_text("SECRET-COOKIE", encoding="utf-8")
        (root / "Login Data").write_text("SECRET-PASSWORD", encoding="utf-8")
        (root / "Local Storage" / "x").parent.mkdir()
        (root / "Local Storage" / "x").write_text("SECRET-TOKEN", encoding="utf-8")
        (root / "ignored.txt").write_text("not in the portable allowlist", encoding="utf-8")
        (downloads / "report.pdf").write_bytes(b"download")
        (policies / "managed.json").write_text('{"BrowserSignin":0}\n', encoding="utf-8")
        try:
            (root / "symlink").symlink_to(root / "Bookmarks")
        except OSError:
            pass

        roots = [(root, "profile"), (downloads, "downloads"), (policies, "policies")]
        first = migrate(roots, output)
        second = migrate(roots, output)
        assert first == second
        assert first["counts"]["imported"] == 6
        assert first["counts"]["skipped"] >= 3
        assert first["counts"]["failed"] == 0
        assert not any("SECRET" in path.read_text(errors="ignore") for path in output.rglob("*") if path.is_file())
        assert (output / "payload" / "downloads" / "report.pdf").read_bytes() == b"download"
    print("migrate-browser-profile: self-test passed")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--source", type=Path, help="legacy Chromium profile root")
    parser.add_argument("--downloads", type=Path)
    parser.add_argument("--policies", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--replace", action="store_true", help="replace an existing reviewed bundle")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0
    if args.source is None or args.output is None:
        parser.error("--source and --output are required unless --self-test is used")
    roots = [(args.source, "profile")]
    if args.downloads is not None:
        roots.append((args.downloads, "downloads"))
    if args.policies is not None:
        roots.append((args.policies, "policies"))
    try:
        manifest = migrate(roots, args.output, args.replace)
    except MigrationError as error:
        parser.exit(1, f"migrate-browser-profile: {error}\n")
    print(json.dumps({"kind": manifest["kind"], "counts": manifest["counts"], "bytes": manifest["bytes"]}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
