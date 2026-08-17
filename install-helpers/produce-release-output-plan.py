#!/usr/bin/env python3
"""Produce the canonical bounded six-role release-output collection plan."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import stat
import sys
import tempfile


REPO = Path(__file__).resolve().parent.parent
REVISION = re.compile(r"[0-9a-f]{40}\Z")
FINGERPRINT = re.compile(r"[0-9A-F]{40,64}\Z")
EPOCH = re.compile(r"[1-9][0-9]{0,11}\Z")
TOKEN = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+:-]{0,254}\Z")
REFERENCE = re.compile(r"[!-~]{1,1024}\Z")
MAX_INPUT = 1024 * 1024
MAX_FILE = 256 * 1024**3
MAX_PLAN = 1024 * 1024
ROLES = (
    "workstation-rpm", "server-rpm", "lighthouse-rpm", "browser-vm",
    "app-vm", "bootc-image",
)
MEDIA = {
    "workstation-rpm": "application/x-rpm",
    "server-rpm": "application/x-rpm",
    "lighthouse-rpm": "application/x-rpm",
    "browser-vm": "application/x-qemu-disk",
    "app-vm": "application/x-qemu-disk",
    "bootc-image": "application/vnd.mcnf.bootc-image-receipt+json",
}
ROLE_FIELDS = {
    "workstation-rpm": {"artifact", "candidate_manifest"},
    "server-rpm": {"artifact", "candidate_manifest"},
    "lighthouse-rpm": {"artifact", "candidate_manifest"},
    "browser-vm": {"artifact", "manifest", "frozen_profile"},
    "app-vm": {"artifact", "manifest"},
    "bootc-image": {"receipt", "image_reference", "architecture", "release_role"},
}


class Refusal(ValueError):
    pass


def refuse(message: str) -> None:
    raise Refusal(message)


def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        if key in value:
            refuse(f"duplicate JSON key: {key}")
        value[key] = item
    return value


def immutable(path: Path, label: str, maximum: int = MAX_FILE) -> Path:
    if not path.is_absolute():
        refuse(f"{label} path must be absolute")
    try:
        metadata = path.lstat()
    except OSError as exc:
        refuse(f"{label} metadata is unavailable: {exc}")
    if not stat.S_ISREG(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode) or metadata.st_nlink != 1:
        refuse(f"{label} must be a single-link regular non-symlink file")
    if not 0 < metadata.st_size <= maximum or metadata.st_mode & 0o022:
        refuse(f"{label} must be non-empty, bounded, and not group/other writable")
    return path.resolve(strict=True)


def load(path: Path) -> dict[str, object]:
    immutable(path, "release-output input", MAX_INPUT)
    try:
        value = json.loads(
            path.read_text(encoding="utf-8"), object_pairs_hook=unique_object,
            parse_constant=lambda item: refuse(f"non-finite JSON number: {item}"),
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        refuse(f"release-output input is malformed: {exc}")
    expected = {"schema_version", "kind", "source_revision", "commit_epoch", "signing_identity", "release_key", "outputs"}
    if not isinstance(value, dict) or set(value) != expected:
        refuse("release-output input fields are not exact")
    if value["schema_version"] != 1 or value["kind"] != "mcnf-release-output-plan-input":
        refuse("release-output input schema is unsupported")
    revision = value["source_revision"]
    signer = value["signing_identity"]
    epoch = value["commit_epoch"]
    if not isinstance(revision, str) or not REVISION.fullmatch(revision) or set(revision) == {"0"}:
        refuse("source revision must be one non-null lowercase Git object ID")
    if not isinstance(signer, str) or not FINGERPRINT.fullmatch(signer) or set(signer) == {"0"}:
        refuse("signing identity must be one non-null uppercase full fingerprint")
    if not isinstance(epoch, str) or not EPOCH.fullmatch(epoch):
        refuse("commit epoch must be one non-null bounded decimal string")
    outputs = value["outputs"]
    if not isinstance(outputs, dict) or set(outputs) != set(ROLES):
        refuse("outputs must contain exactly the six canonical release roles")
    return value


def string(row: dict[str, object], field: str, pattern: re.Pattern[str] = TOKEN) -> str:
    value = row[field]
    if not isinstance(value, str) or not pattern.fullmatch(value):
        refuse(f"{field} is malformed")
    return value


def role_row(value: dict[str, object], role: str) -> dict[str, object]:
    raw = value["outputs"]
    assert isinstance(raw, dict)
    row = raw[role]
    if not isinstance(row, dict) or set(row) != ROLE_FIELDS[role]:
        refuse(f"{role} input fields are not exact")
    return row


def path_field(row: dict[str, object], field: str, role: str) -> Path:
    raw = row[field]
    if not isinstance(raw, str):
        refuse(f"{role} {field} path is malformed")
    return immutable(Path(raw), f"{role} {field}")


def output_row(role: str, artifact: Path, revision: str, signer: str | None,
               companions: dict[str, Path], verifier: list[str]) -> dict[str, object]:
    row: dict[str, object] = {
        "role": role,
        "path": str(artifact),
        "media_type": MEDIA[role],
        "source_revision": revision,
        "companions": {name: str(path) for name, path in companions.items()},
        "verifier": verifier,
    }
    if signer is not None:
        row["signing_identity"] = signer
    return row


def produce(value: dict[str, object]) -> dict[str, object]:
    revision = str(value["source_revision"])
    signer = str(value["signing_identity"])
    epoch = str(value["commit_epoch"])
    release_key = immutable(Path(str(value["release_key"])), "governed release key", MAX_INPUT)
    rows: list[dict[str, object]] = []
    claimed: set[tuple[int, int]] = set()
    identities: dict[Path, tuple[int, ...]] = {}

    def identity(metadata: os.stat_result) -> tuple[int, ...]:
        return (metadata.st_dev, metadata.st_ino, metadata.st_mode, metadata.st_nlink,
                metadata.st_uid, metadata.st_gid, metadata.st_size,
                metadata.st_mtime_ns, metadata.st_ctime_ns)

    def claim(path: Path, label: str) -> Path:
        metadata = path.lstat()
        file_key = (metadata.st_dev, metadata.st_ino)
        if file_key in claimed:
            refuse(f"{label} duplicates another supplied file")
        claimed.add(file_key)
        identities[path] = (metadata.st_dev, metadata.st_ino, metadata.st_mode,
                            metadata.st_nlink, metadata.st_uid, metadata.st_gid,
                            metadata.st_size, metadata.st_mtime_ns, metadata.st_ctime_ns)
        return path

    claim(release_key, "governed release key")

    rpm_specs = {
        "workstation-rpm": REPO / "packaging/app-vm/verify-rpm-supply.sh",
        "server-rpm": REPO / "packaging/server-rpm/produce-server-rpm-candidate.py",
        "lighthouse-rpm": REPO / "packaging/browser-vm/produce-lighthouse-rpm-candidate.py",
    }
    for role, verifier in rpm_specs.items():
        row = role_row(value, role)
        artifact = claim(path_field(row, "artifact", role), f"{role} artifact")
        manifest = claim(path_field(row, "candidate_manifest", role), f"{role} candidate manifest")
        companions = {"candidate_manifest": manifest, "release_key": release_key}
        if role == "workstation-rpm":
            argv = [str(verifier), "--key", "{companion:release_key}", "--source-commit", "{source_revision}",
                    "--candidate-manifest", "{companion:candidate_manifest}",
                    "--expected-signing-fingerprint", "{signing_identity}", "--", "{artifact}"]
        else:
            mode = "reverify" if role == "server-rpm" else "verify"
            argv = [str(verifier), mode, "--rpm", "{artifact}", "--source-revision", "{source_revision}",
                    "--expected-signing-fingerprint", "{signing_identity}",
                    "--release-key", "{companion:release_key}", "--manifest", "{companion:candidate_manifest}"]
        rows.append(output_row(role, artifact, revision, signer, companions, argv))

    browser = role_row(value, "browser-vm")
    browser_artifact = claim(path_field(browser, "artifact", "browser-vm"), "browser-vm artifact")
    browser_manifest = claim(path_field(browser, "manifest", "browser-vm"), "browser-vm manifest")
    browser_profile = claim(path_field(browser, "frozen_profile", "browser-vm"), "browser-vm frozen profile")
    browser_companions = {"manifest": browser_manifest, "frozen_profile": browser_profile}
    rows.append(output_row("browser-vm", browser_artifact, revision, None, browser_companions, [
        str(REPO / "packaging/browser-vm/verify-image-manifest.py"), "verify",
        "--repo-root", str(REPO), "--profile", "{companion:frozen_profile}", "--image", "{artifact}",
        "--manifest", "{companion:manifest}", "--source-revision", "{source_revision}",
    ]))

    app = role_row(value, "app-vm")
    app_artifact = claim(path_field(app, "artifact", "app-vm"), "app-vm artifact")
    app_manifest = claim(path_field(app, "manifest", "app-vm"), "app-vm manifest")
    rows.append(output_row("app-vm", app_artifact, revision, None, {"manifest": app_manifest}, [
        str(REPO / "packaging/app-vm/verify-qcow2-manifest.py"), "--image", "{artifact}",
        "--manifest", "{companion:manifest}", "--source-revision", "{source_revision}",
    ]))

    bootc = role_row(value, "bootc-image")
    # A registry-native bootc image has no local image file. Its immutable digest
    # receipt is therefore the exact collected artifact; the owning inspector
    # rebinds it to the registry reference and release identity below.
    bootc_receipt = claim(path_field(bootc, "receipt", "bootc-image"), "bootc-image receipt")
    bootc_arch = string(bootc, "architecture")
    if not re.fullmatch(r"[a-z0-9][a-z0-9_.-]{0,31}", bootc_arch):
        refuse("bootc-image architecture is malformed")
    image_reference = string(bootc, "image_reference", REFERENCE)
    if image_reference.startswith("docker://"):
        refuse("bootc image reference must omit its transport prefix")
    bootc_role = string(bootc, "release_role", re.compile(r"[a-z0-9][a-z0-9-]{0,63}\Z"))
    rows.append(output_row("bootc-image", bootc_receipt, revision, None, {}, [
        str(REPO / "install-helpers/produce-bootc-digest-receipt.py"), "inspect",
        "--receipt", "{artifact}", "--expected-image-reference", image_reference,
        "--expected-architecture", bootc_arch, "--expected-source-revision", "{source_revision}",
        "--expected-commit-epoch", epoch, "--expected-release-role", bootc_role,
    ]))
    for path, before in identities.items():
        try:
            after = path.lstat()
        except OSError as exc:
            refuse(f"supplied file disappeared before plan publication: {path}: {exc}")
        if identity(after) != before:
            refuse(f"supplied file changed before plan publication: {path}")
    return {"schema_version": 1, "kind": "mcnf-release-output-collection-plan",
            "source_revision": revision, "outputs": rows}


def publish(output: Path, document: dict[str, object]) -> None:
    if output.exists() or output.is_symlink():
        refuse("plan output already exists or is substituted")
    parent = output.parent.resolve(strict=True)
    if not parent.is_dir() or parent.stat().st_mode & 0o022:
        refuse("plan output parent must be a private real directory")
    body = (json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii")
    if len(body) > MAX_PLAN:
        refuse("collection plan exceeds its bounded contract")
    directory = Path(tempfile.mkdtemp(prefix=f".{output.name}.", dir=parent))
    try:
        directory.chmod(0o700)
        staged = directory / "plan.json"
        descriptor = os.open(staged, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(body); stream.flush(); os.fsync(stream.fileno())
        os.link(staged, output, follow_symlinks=False)
        staged.unlink()
        parent_fd = os.open(parent, os.O_RDONLY | os.O_DIRECTORY)
        try: os.fsync(parent_fd)
        finally: os.close(parent_fd)
    finally:
        try: directory.rmdir()
        except OSError: pass


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inputs", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        publish(args.output, produce(load(args.inputs)))
    except (OSError, Refusal, UnicodeError, ValueError) as exc:
        print(f"release-output-plan: REFUSED: {exc}", file=sys.stderr)
        return 2
    print(f"release-output-plan: PASS: wrote six canonical roles to {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
