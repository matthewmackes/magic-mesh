#!/usr/bin/env python3
"""Produce or inspect the non-secret identity receipt for RPM release signing."""

from __future__ import annotations

import argparse
import ctypes
import errno
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

EXIT_REFUSED = 2
MAX_RECEIPT_BYTES = 4096
FINGERPRINT_RE = re.compile(r"[0-9A-F]{40}|[0-9A-F]{64}")
REVISION_RE = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")
EPOCH_RE = re.compile(r"[1-9][0-9]{0,11}")


class Refusal(RuntimeError):
    pass


def run(command: list[str], label: str) -> str:
    try:
        result = subprocess.run(command, check=False, capture_output=True, text=True)
    except OSError as exc:
        raise Refusal(f"{label} is unavailable: {exc}") from exc
    if result.returncode != 0:
        raise Refusal(f"{label} failed")
    return result.stdout


def colon_records(output: str) -> list[list[str]]:
    return [line.split(":") for line in output.splitlines() if line]


def primary_fingerprint(output: str, record_kind: str, label: str) -> str:
    records = colon_records(output)
    starts = [index for index, row in enumerate(records) if row[0] == record_kind]
    if len(starts) != 1:
        raise Refusal(f"{label} must resolve to exactly one primary {record_kind} record")
    start = starts[0]
    end = next(
        (index for index in range(start + 1, len(records)) if records[index][0] in {"pub", "sec", "sub", "ssb"}),
        len(records),
    )
    fingerprints = [row[9].upper() for row in records[start:end] if row[0] == "fpr" and len(row) > 9]
    if len(fingerprints) != 1 or not FINGERPRINT_RE.fullmatch(fingerprints[0]):
        raise Refusal(f"{label} has an ambiguous, null, or invalid primary fingerprint")
    return fingerprints[0]


def regular_bounded(path: Path, label: str, limit: int) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise Refusal(f"{label} is not a regular non-symlink file: {path}")
    before = path.stat()
    if before.st_size <= 0 or before.st_size > limit:
        raise Refusal(f"{label} size is outside the allowed bound")
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0))
    try:
        opened = os.fstat(fd)
        if (opened.st_dev, opened.st_ino) != (before.st_dev, before.st_ino) or not stat.S_ISREG(opened.st_mode):
            raise Refusal(f"{label} changed while opening")
        body = b""
        while len(body) <= limit:
            chunk = os.read(fd, min(65536, limit + 1 - len(body)))
            if not chunk:
                break
            body += chunk
        after = path.stat()
        final = os.fstat(fd)
    finally:
        os.close(fd)
    identity = (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns)
    if identity != (final.st_dev, final.st_ino, final.st_size, final.st_mtime_ns):
        raise Refusal(f"{label} changed while reading")
    if (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns) != identity:
        raise Refusal(f"{label} pathname changed while reading")
    if not body or len(body) > limit:
        raise Refusal(f"{label} size is outside the allowed bound")
    return body


def governed_identity(repo: Path, gpg: str, key_id: str) -> tuple[str, str]:
    key_path = repo / "packaging/repo/RPM-GPG-KEY-magic-mesh"
    key_body = regular_bounded(key_path, "governed RPM public key", 1024 * 1024)
    public = run(
        [gpg, "--batch", "--no-options", "--with-colons", "--fingerprint", "--show-keys", str(key_path)],
        "governed RPM public-key inspection",
    )
    approved = primary_fingerprint(public, "pub", "governed RPM public key")
    secret = run(
        [gpg, "--batch", "--no-options", "--with-colons", "--fingerprint", "--list-secret-keys", key_id],
        "configured RPM signing identity lookup",
    )
    configured = primary_fingerprint(secret, "sec", "configured RPM signing identity")
    if configured != approved:
        raise Refusal("configured RPM signing identity is not the governed project authority")
    return approved, hashlib.sha256(key_body).hexdigest()


def validate_revision_epoch(repo: Path, requested_revision: str, requested_epoch: str) -> tuple[str, int]:
    if not REVISION_RE.fullmatch(requested_revision) or set(requested_revision) == {"0"}:
        raise Refusal("source revision must be one non-null lowercase Git object ID")
    resolved = run(
        ["git", "-C", str(repo), "rev-parse", "--verify", f"{requested_revision}^{{commit}}"],
        "source revision verification",
    ).strip()
    if resolved != requested_revision:
        raise Refusal("source revision is not the exact resolved Git commit")
    if not EPOCH_RE.fullmatch(requested_epoch):
        raise Refusal("release epoch must be one non-null bounded integer")
    epoch = int(requested_epoch)
    commit_epoch = run(
        ["git", "-C", str(repo), "show", "-s", "--format=%ct", requested_revision],
        "source revision epoch lookup",
    ).strip()
    if commit_epoch != requested_epoch:
        raise Refusal("release epoch does not match the exact source commit")
    return requested_revision, epoch


def receipt_document(repo: Path, gpg: str, key_id: str, revision: str, epoch: str) -> dict[str, object]:
    revision, release_epoch = validate_revision_epoch(repo, revision, epoch)
    fingerprint, key_digest = governed_identity(repo, gpg, key_id)
    return {
        "schema_version": 1,
        "kind": "mcnf-rpm-signing-identity",
        "configured_identity": key_id,
        "primary_fingerprint": fingerprint,
        "public_key_sha256": key_digest,
        "source_revision": revision,
        "release_epoch": release_epoch,
    }


def parse_receipt(path: Path) -> dict[str, object]:
    body = regular_bounded(path, "RPM signing identity receipt", MAX_RECEIPT_BYTES)
    try:
        value = json.loads(body)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise Refusal("RPM signing identity receipt is not canonical JSON") from exc
    if not isinstance(value, dict) or set(value) != {
        "schema_version", "kind", "configured_identity", "primary_fingerprint",
        "public_key_sha256", "source_revision", "release_epoch",
    }:
        raise Refusal("RPM signing identity receipt has an unexpected schema")
    canonical = json.dumps(value, sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"
    if body != canonical:
        raise Refusal("RPM signing identity receipt is not canonical JSON")
    return value


def inspect(
    repo: Path,
    receipt: Path,
    gpg: str,
    key_id: str,
    expected_revision: str,
    expected_epoch: str,
) -> dict[str, object]:
    value = parse_receipt(receipt)
    if value["schema_version"] != 1 or value["kind"] != "mcnf-rpm-signing-identity":
        raise Refusal("RPM signing identity receipt kind or version is unsupported")
    receipt_key_id = value["configured_identity"]
    if not isinstance(receipt_key_id, str) or not receipt_key_id or len(receipt_key_id) > 128 or any(ord(ch) < 0x20 or ord(ch) > 0x7E for ch in receipt_key_id):
        raise Refusal("RPM signing configured identity is invalid")
    if receipt_key_id != key_id:
        raise Refusal("RPM signing identity receipt does not name the currently configured signing authority")
    expected = receipt_document(repo, gpg, key_id, expected_revision, expected_epoch)
    if value != expected:
        raise Refusal("RPM signing identity receipt does not match current governed authority and release input")
    return value


def atomic_write(path: Path, body: bytes) -> None:
    if path.exists() or path.is_symlink():
        raise Refusal(f"receipt output already exists: {path}")
    parent = path.parent.resolve(strict=True)
    temporary_fd, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=parent)
    temporary = Path(temporary_name)
    try:
        os.fchmod(temporary_fd, 0o400)
        offset = 0
        while offset < len(body):
            written = os.write(temporary_fd, body[offset:])
            if written <= 0:
                raise Refusal("receipt write made no progress")
            offset += written
        os.fsync(temporary_fd)
        os.close(temporary_fd)
        temporary_fd = -1
        libc = ctypes.CDLL(None, use_errno=True)
        renameat2 = getattr(libc, "renameat2", None)
        if renameat2 is None:
            raise Refusal("atomic no-replace receipt publication is unavailable")
        renameat2.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
        renameat2.restype = ctypes.c_int
        if renameat2(-100, os.fsencode(temporary), -100, os.fsencode(path), 1) != 0:
            error = ctypes.get_errno()
            if error in {errno.EEXIST, errno.ENOTEMPTY}:
                raise Refusal(f"receipt output appeared during publication: {path}")
            raise Refusal(f"atomic receipt publication failed: {os.strerror(error)}")
        parent_fd = os.open(parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(parent_fd)
        finally:
            os.close(parent_fd)
    finally:
        if temporary_fd >= 0:
            os.close(temporary_fd)
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--gpg", default="gpg")
    subparsers = parser.add_subparsers(dest="command", required=True)
    produce = subparsers.add_parser("produce")
    produce.add_argument("--source-revision", required=True)
    produce.add_argument("--release-epoch", required=True)
    produce.add_argument("--output", required=True, type=Path)
    produce.add_argument("--signing-identity", default=os.environ.get("MAGIC_MESH_SIGN_KEY", "Magic Mesh Release Signing"))
    verify = subparsers.add_parser("inspect")
    verify.add_argument("--receipt", required=True, type=Path)
    verify.add_argument("--expected-source-revision", required=True)
    verify.add_argument("--expected-release-epoch", required=True)
    verify.add_argument("--signing-identity", default=os.environ.get("MAGIC_MESH_SIGN_KEY", "Magic Mesh Release Signing"))
    args = parser.parse_args()
    try:
        repo = args.repo.resolve(strict=True)
        if args.command == "produce":
            value = receipt_document(repo, args.gpg, args.signing_identity, args.source_revision, args.release_epoch)
            body = json.dumps(value, sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"
            if len(body) > MAX_RECEIPT_BYTES:
                raise Refusal("RPM signing identity receipt exceeds its size bound")
            atomic_write(args.output, body)
            print(args.output)
        else:
            value = inspect(repo, args.receipt, args.gpg, args.signing_identity, args.expected_source_revision, args.expected_release_epoch)
            print(value["primary_fingerprint"])
        return 0
    except Refusal as exc:
        print(f"rpm-signing-identity-receipt: REFUSED: {exc}", file=sys.stderr)
        return EXIT_REFUSED


if __name__ == "__main__":
    raise SystemExit(main())
