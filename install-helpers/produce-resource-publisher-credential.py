#!/usr/bin/env python3
"""Produce a signed, non-secret resource-publisher credential receipt."""

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
FINGERPRINT_RE = re.compile(r"[0-9A-F]{40}|[0-9A-F]{64}")
REVISION_RE = re.compile(r"[0-9a-f]{40}")
NODE_RE = re.compile(r"peer:[A-Za-z0-9][A-Za-z0-9._-]{0,127}")
ROLES = {"lighthouse", "workstation"}
MAX_CREDENTIAL_BYTES = 4096


class Refusal(RuntimeError):
    pass


def run(command: list[str], label: str, env: dict[str, str]) -> str:
    try:
        result = subprocess.run(command, check=False, capture_output=True, text=True, env=env)
    except OSError as exc:
        raise Refusal(f"{label} is unavailable: {exc}") from exc
    if result.returncode != 0:
        detail = result.stderr.strip().splitlines()
        raise Refusal(f"{label} failed{': ' + detail[-1] if detail else ''}")
    return result.stdout


def stable_regular(path: Path, label: str, *, secret: bool = False) -> tuple[os.stat_result, bytes]:
    try:
        before = path.lstat()
    except OSError as exc:
        raise Refusal(f"{label} is unavailable: {path}") from exc
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        raise Refusal(f"{label} must be one regular non-symlink file")
    if before.st_mode & 0o022:
        raise Refusal(f"{label} is group/world writable")
    if secret and before.st_mode & 0o077:
        raise Refusal(f"{label} permissions must not grant group/world access")
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags)
    try:
        opened = os.fstat(fd)
        if (opened.st_dev, opened.st_ino) != (before.st_dev, before.st_ino):
            raise Refusal(f"{label} was replaced while being opened")
        body = os.read(fd, MAX_CREDENTIAL_BYTES + 1 if secret else 1024 * 1024 + 1)
        after = os.fstat(fd)
    finally:
        os.close(fd)
    if (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns) != (
        before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns
    ):
        raise Refusal(f"{label} changed while being read")
    if secret and (not body or len(body) > MAX_CREDENTIAL_BYTES or any(byte < 32 or byte == 127 for byte in body)):
        raise Refusal(f"{label} is empty, oversized, or contains control bytes")
    return before, body


def primary_fingerprint(output: str, kind: str) -> str:
    rows = [line.split(":") for line in output.splitlines() if line]
    starts = [index for index, row in enumerate(rows) if row[0] == kind]
    if len(starts) != 1:
        raise Refusal(f"release authority must resolve to exactly one primary {kind} record")
    start = starts[0]
    end = next((i for i in range(start + 1, len(rows)) if rows[i][0] in {"pub", "sec", "sub", "ssb"}), len(rows))
    fingerprints = [row[9].upper() for row in rows[start:end] if row[0] == "fpr" and len(row) > 9]
    if len(fingerprints) != 1 or not FINGERPRINT_RE.fullmatch(fingerprints[0]):
        raise Refusal(f"release authority has an ambiguous or invalid primary {kind} fingerprint")
    primary = rows[start]
    if len(primary) <= 3 or primary[3] != "22":
        raise Refusal("governed release primary key is not Ed25519")
    return fingerprints[0]


def release_authority(gpg: str, public_key: Path, key_id: str, env: dict[str, str]) -> tuple[str, os.stat_result, bytes]:
    identity, public_body = stable_regular(public_key, "governed release public key")
    public_fingerprint = primary_fingerprint(
        run([gpg, "--batch", "--with-colons", "--fingerprint", "--show-keys", str(public_key)], "governed release public-key inspection", env),
        "pub",
    )
    secret_fingerprint = primary_fingerprint(
        run([gpg, "--batch", "--with-colons", "--fingerprint", "--list-secret-keys", key_id], "governed release secret signing authority lookup", env),
        "sec",
    )
    if secret_fingerprint != public_fingerprint:
        raise Refusal("secret signing authority does not match the governed release public key")
    return public_fingerprint, identity, public_body


def exact_revision(repo: Path, requested: str | None, env: dict[str, str]) -> str:
    value = requested or run(["git", "-C", str(repo), "rev-parse", "--verify", "HEAD^{commit}"], "source revision lookup", env).strip()
    if not REVISION_RE.fullmatch(value) or value == "0" * 40:
        raise Refusal("source revision must be one non-null lowercase 40-character Git commit")
    resolved = run(["git", "-C", str(repo), "rev-parse", "--verify", f"{value}^{{commit}}"], "source revision verification", env).strip()
    if resolved != value:
        raise Refusal("source revision is not the exact resolved Git commit")
    return value


def write_exclusive(path: Path, body: bytes) -> None:
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o400)
    try:
        os.fchmod(fd, 0o400)
        offset = 0
        while offset < len(body):
            written = os.write(fd, body[offset:])
            if written <= 0:
                raise Refusal(f"receipt write made no progress: {path.name}")
            offset += written
        os.fsync(fd)
    finally:
        os.close(fd)


def publish_noreplace(source: Path, destination: Path) -> None:
    libc = ctypes.CDLL(None, use_errno=True)
    renameat2 = getattr(libc, "renameat2", None)
    if renameat2 is None:
        raise Refusal("atomic no-replace directory publication is unavailable")
    renameat2.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    renameat2.restype = ctypes.c_int
    result = renameat2(-100, os.fsencode(source), -100, os.fsencode(destination), 1)
    if result == 0:
        return
    error = ctypes.get_errno()
    if error in {errno.EEXIST, errno.ENOTEMPTY}:
        raise Refusal("output path appeared during publication; existing receipt was preserved")
    raise Refusal(f"atomic no-replace receipt publication failed: {os.strerror(error)}")


def produce(args: argparse.Namespace, env: dict[str, str]) -> dict[str, object]:
    repo = args.repo.resolve(strict=True)
    if not NODE_RE.fullmatch(args.target_node):
        raise Refusal("target node must be one canonical peer:<node> identity")
    if args.target_role not in ROLES:
        raise Refusal("target role must be lighthouse or workstation")
    if args.out_dir.exists() or args.out_dir.is_symlink():
        raise Refusal("output path already exists; choose a new receipt directory")
    parent = args.out_dir.parent.resolve(strict=True)
    if parent.stat().st_mode & 0o022:
        raise Refusal("output parent must not be group/world writable")
    revision = exact_revision(repo, args.source_revision, env)
    fingerprint, key_identity, key_body = release_authority(args.gpg, args.release_public_key, args.release_key_id, env)
    credential_identity, credential = stable_regular(args.credential, "resource-publisher SecretStore export", secret=True)
    receipt = {
        "schema_version": 1,
        "kind": "mcnf-resource-publisher-credential",
        "publisher_identity": f"openpgp-primary:{fingerprint}",
        "credential_sha256": hashlib.sha256(credential).hexdigest(),
        "source_revision": revision,
        "target_node": args.target_node,
        "target_role": args.target_role,
    }
    receipt_body = json.dumps(receipt, sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"
    temporary: Path | None = Path(tempfile.mkdtemp(prefix=f".{args.out_dir.name}.", dir=parent))
    try:
        temporary.chmod(0o700)
        receipt_path = temporary / "resource-publisher-receipt.json"
        signature_path = temporary / "resource-publisher-receipt.json.asc"
        write_exclusive(receipt_path, receipt_body)
        run([args.gpg, "--batch", "--armor", "--detach-sign", "--local-user", fingerprint, "--output", str(signature_path), str(receipt_path)], "resource-publisher receipt signing", env)
        signature_path.chmod(0o400)
        # Refuse authority or credential replacement across the signing boundary.
        key_after, key_after_body = stable_regular(args.release_public_key, "governed release public key")
        credential_after, credential_after_body = stable_regular(args.credential, "resource-publisher SecretStore export", secret=True)
        if (key_identity.st_dev, key_identity.st_ino, key_body) != (key_after.st_dev, key_after.st_ino, key_after_body):
            raise Refusal("governed release public key was replaced during production")
        if (credential_identity.st_dev, credential_identity.st_ino, credential) != (credential_after.st_dev, credential_after.st_ino, credential_after_body):
            raise Refusal("resource-publisher SecretStore export was replaced during production")
        publish_noreplace(temporary, args.out_dir)
        temporary = None
    finally:
        if temporary is not None and temporary.exists():
            shutil.rmtree(temporary)
    return receipt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--credential", required=True, type=Path, help="root-only transient export of resource/publisher-hmac")
    parser.add_argument("--target-node", required=True)
    parser.add_argument("--target-role", required=True)
    parser.add_argument("--out-dir", required=True, type=Path)
    parser.add_argument("--source-revision")
    parser.add_argument("--release-key-id", default=os.environ.get("MAGIC_MESH_SIGN_KEY", "B546CC2EF9489F1899657AC9E6C820DAFBD1B07A"))
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1], help=argparse.SUPPRESS)
    parser.add_argument("--release-public-key", type=Path, default=Path(__file__).resolve().parents[1] / "packaging/repo/RPM-GPG-KEY-magic-mesh", help=argparse.SUPPRESS)
    parser.add_argument("--gpg", default="gpg", help=argparse.SUPPRESS)
    args = parser.parse_args()
    print(json.dumps(produce(args, dict(os.environ)), sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Refusal as exc:
        print(f"REFUSED[WL-FUNC-019/publisher-credential-producer]: {exc}", file=sys.stderr)
        raise SystemExit(EXIT_REFUSED)
