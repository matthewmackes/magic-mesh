#!/usr/bin/env python3
"""Produce the exact governed RPM input consumed by the App VM image lane."""

from __future__ import annotations

import argparse
import ctypes
import errno
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path


EXIT_REFUSED = 2
MAX_RPM_BYTES = 1024 * 1024 * 1024
TARGET_IDENTITY = "mcnf-app-vm/wayland-standard-v1"
REVISION_RE = re.compile(r"[0-9a-f]{40}\Z")
FINGERPRINT_RE = re.compile(r"[0-9A-F]{40,64}\Z")
NEVRA_PART_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+~:-]*\Z")


class Refusal(RuntimeError):
    pass


def write_all(descriptor: int, body: bytes, purpose: str) -> None:
    written = 0
    while written < len(body):
        count = os.write(descriptor, body[written:])
        if count <= 0:
            raise Refusal(f"{purpose} could not be written completely")
        written += count


def run(command: list[str], purpose: str, *, env: dict[str, str] | None = None) -> str:
    try:
        result = subprocess.run(command, text=True, capture_output=True, env=env, check=False)
    except OSError as exc:
        raise Refusal(f"{purpose} could not run: {exc}") from exc
    if result.returncode != 0:
        detail = result.stderr.strip().splitlines()[-1:] or result.stdout.strip().splitlines()[-1:]
        suffix = f": {detail[0]}" if detail else ""
        raise Refusal(f"{purpose} failed{suffix}")
    return result.stdout


def file_identity(value: os.stat_result) -> tuple[int, ...]:
    return (
        value.st_dev, value.st_ino, value.st_mode, value.st_nlink, value.st_uid,
        value.st_gid, value.st_size, value.st_mtime_ns, value.st_ctime_ns,
    )


def snapshot_rpm(source: Path, root: Path) -> tuple[Path, str]:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(source, flags)
    except OSError as exc:
        raise Refusal(f"RPM cannot be opened safely: {exc}") from exc
    target = root / "candidate.rpm"
    digest = hashlib.sha256()
    try:
        before = os.fstat(descriptor)
        named = os.stat(source, follow_symlinks=False)
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
            raise Refusal("RPM must be a single-link regular file")
        if before.st_size <= 0 or before.st_size > MAX_RPM_BYTES:
            raise Refusal(f"RPM size must be between 1 and {MAX_RPM_BYTES} bytes")
        if before.st_mode & 0o022:
            raise Refusal("RPM must not be group/other writable")
        if file_identity(before) != file_identity(named):
            raise Refusal("RPM path identity changed before snapshot")
        output = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
        try:
            remaining = before.st_size
            while remaining:
                chunk = os.read(descriptor, min(1024 * 1024, remaining))
                if not chunk:
                    raise Refusal("RPM was truncated during snapshot")
                digest.update(chunk)
                write_all(output, chunk, "RPM snapshot")
                remaining -= len(chunk)
            if os.read(descriptor, 1):
                raise Refusal("RPM grew during snapshot")
            os.fsync(output)
        finally:
            os.close(output)
        after = os.fstat(descriptor)
        closed_name = os.stat(source, follow_symlinks=False)
        if file_identity(after) != file_identity(before) or file_identity(closed_name) != file_identity(before):
            raise Refusal("RPM changed during snapshot")
    finally:
        os.close(descriptor)
    return target, digest.hexdigest()


def governed_fingerprints(gpg: str, key: Path) -> set[str]:
    output = run(
        [gpg, "--batch", "--with-colons", "--show-keys", "--fingerprint", "--fingerprint", str(key)],
        "governed release public-key inspection",
    )
    values = {fields[9].upper() for line in output.splitlines() if (fields := line.split(":"))[0] == "fpr" and len(fields) > 9}
    if not values or any(FINGERPRINT_RE.fullmatch(value) is None for value in values):
        raise Refusal("governed release public key has no unambiguous full fingerprint authority")
    return values


def verify_signature(rpm_path: Path, key: Path, gpg: str, rpm_tool: str, rpmkeys: str) -> str:
    allowed = governed_fingerprints(gpg, key)
    with tempfile.TemporaryDirectory(prefix="app-vm-rpmdb-") as raw:
        database = Path(raw)
        run([rpm_tool, "--dbpath", str(database), "--initdb"], "temporary RPM database initialization")
        run([rpm_tool, "--dbpath", str(database), "--import", str(key)], "governed RPM key import")
        output = run(
            [rpmkeys, "--dbpath", str(database), "--checksig", "--verbose", "--", str(rpm_path)],
            "governed RPM signature verification",
        )
    matches = re.findall(r"key ID ([0-9a-fA-F]{8,64})\s*:\s*OK(?:\s|$)", output, re.IGNORECASE)
    if len(matches) != 1:
        raise Refusal("RPM signature verification did not yield exactly one signing key ID")
    key_id = matches[0].upper()
    resolved = {fingerprint for fingerprint in allowed if fingerprint.endswith(key_id)}
    if len(resolved) != 1:
        raise Refusal("RPM signing key ID does not resolve to exactly one governed full fingerprint")
    return next(iter(resolved))


def rpm_identity(rpm_path: Path, rpm_tool: str) -> tuple[str, str]:
    output = run(
        [rpm_tool, "-qp", "--qf", "%{NAME}\t%{EPOCHNUM}\t%{VERSION}\t%{RELEASE}\t%{ARCH}\n%{PAYLOADDIGESTALGO}\t%{PAYLOADDIGEST}\n", "--", str(rpm_path)],
        "RPM identity inspection",
    )
    lines = output.splitlines()
    if len(lines) != 2:
        raise Refusal("RPM identity inspection returned ambiguous metadata")
    metadata = lines[0].split("\t")
    payload = lines[1].split("\t")
    if len(metadata) != 5 or len(payload) != 2:
        raise Refusal("RPM identity metadata is incomplete")
    name, epoch, version, release, architecture = metadata
    if name != "magic-mesh" or any(NEVRA_PART_RE.fullmatch(value) is None for value in (version, release, architecture)):
        raise Refusal("RPM is not one exact magic-mesh Workstation package")
    if not epoch.isdigit() or payload[0].upper() not in {"8", "SHA256"}:
        raise Refusal("RPM epoch or payload digest algorithm is unsupported")
    payload_digest = payload[1].lower()
    if re.fullmatch(r"[0-9a-f]{64}", payload_digest) is None or payload_digest == "0" * 64:
        raise Refusal("RPM payload digest is malformed")
    epoch_prefix = "" if epoch == "0" else f"{epoch}:"
    return f"{name}-{epoch_prefix}{version}-{release}.{architecture}", payload_digest


def canonical_json(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def publish_noreplace(source: Path, destination: Path) -> None:
    libc = ctypes.CDLL(None, use_errno=True)
    renameat2 = getattr(libc, "renameat2", None)
    if renameat2 is None:
        raise Refusal("atomic no-replace publication is unavailable")
    renameat2.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    renameat2.restype = ctypes.c_int
    if renameat2(-100, os.fsencode(source), -100, os.fsencode(destination), 1) != 0:
        error = ctypes.get_errno()
        if error == errno.EEXIST:
            raise Refusal("output path already exists")
        raise Refusal(f"atomic output publication failed: {os.strerror(error)}")


def produce(args: argparse.Namespace) -> dict[str, object]:
    revision = args.source_revision.lower()
    if REVISION_RE.fullmatch(revision) is None or revision == "0" * 40:
        raise Refusal("source revision must be a non-null 40-character lowercase Git object ID")
    if args.release_key.is_symlink():
        raise Refusal("governed release public key must be a regular non-symlink file")
    key = args.release_key.resolve(strict=True)
    if not key.is_file():
        raise Refusal("governed release public key must be a regular non-symlink file")
    output = args.output
    if output.parent.is_symlink():
        raise Refusal("output parent has substituted authority")
    parent = output.parent.resolve(strict=True)
    if output.exists() or output.is_symlink():
        raise Refusal("output path already exists or has substituted authority")
    if parent.stat().st_mode & 0o022:
        raise Refusal("output parent must not be group/other writable")
    with tempfile.TemporaryDirectory(prefix="app-vm-rpm-candidate-") as snapshot_raw:
        snapshot, rpm_sha256 = snapshot_rpm(args.rpm, Path(snapshot_raw))
        signer = verify_signature(snapshot, key, args.gpg, args.rpm_tool, args.rpmkeys)
        nevra, payload_sha256 = rpm_identity(snapshot, args.rpm_tool)
        document = {
            "app_vm_target_identity": TARGET_IDENTITY,
            "artifact": {"nevra": nevra, "payload_sha256": payload_sha256, "rpm_sha256": rpm_sha256},
            "build_identity": {"source_revision": revision},
            "kind": "mcnf-app-vm-rpm-candidate-manifest",
            "schema_version": 2,
            "signing_fingerprint": signer,
        }
        body = canonical_json(document)
        stage = Path(tempfile.mkdtemp(prefix=f".{output.name}.", dir=parent))
        try:
            stage.chmod(0o700)
            manifest = stage / "candidate-manifest.json"
            descriptor = os.open(manifest, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
            try:
                write_all(descriptor, body, "candidate manifest")
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
            directory = os.open(stage, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
            run(
                [
                    str(args.verifier), "--key", str(key), "--source-commit", revision,
                    "--candidate-manifest", str(manifest), "--", str(snapshot),
                ],
                "App VM RPM candidate self-verification",
                env=dict(os.environ),
            )
            publish_noreplace(stage, output)
            stage = None
        finally:
            if stage is not None and stage.exists():
                shutil.rmtree(stage)
    return document


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rpm", required=True, type=Path)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument("--release-key", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--gpg", default="gpg", help=argparse.SUPPRESS)
    parser.add_argument("--rpm-tool", default="rpm", help=argparse.SUPPRESS)
    parser.add_argument("--rpmkeys", default="rpmkeys", help=argparse.SUPPRESS)
    parser.add_argument(
        "--verifier", type=Path,
        default=Path(__file__).resolve().with_name("verify-rpm-supply.sh"),
        help=argparse.SUPPRESS,
    )
    args = parser.parse_args()
    print(json.dumps(produce(args), sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, Refusal) as exc:
        print(f"REFUSED[WL-FUNC-018/rpm-candidate-producer]: {exc}", file=sys.stderr)
        raise SystemExit(EXIT_REFUSED)
