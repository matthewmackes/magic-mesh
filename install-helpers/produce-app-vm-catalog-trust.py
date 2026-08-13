#!/usr/bin/env python3
"""Produce the governed App VM Flatpak catalog trust input."""

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


class Refusal(RuntimeError):
    pass


def run(command: list[str], label: str, *, env: dict[str, str] | None = None) -> str:
    try:
        result = subprocess.run(command, check=False, capture_output=True, text=True, env=env)
    except OSError as exc:
        raise Refusal(f"{label} is unavailable: {exc}") from exc
    if result.returncode != 0:
        detail = result.stderr.strip().splitlines()
        suffix = f": {detail[-1]}" if detail else ""
        raise Refusal(f"{label} failed{suffix}")
    return result.stdout


def records(output: str) -> list[list[str]]:
    return [line.split(":") for line in output.splitlines() if line]


def primary(records_: list[list[str]], kind: str) -> tuple[list[str], str, dict[int, tuple[int, str]]]:
    starts = [index for index, row in enumerate(records_) if row[0] == kind]
    if len(starts) != 1:
        raise Refusal(f"release authority must resolve to exactly one primary {kind} record")
    start = starts[0]
    end = next((i for i in range(start + 1, len(records_)) if records_[i][0] in {"pub", "sec", "sub", "ssb"}), len(records_))
    block = records_[start:end]
    fingerprints = [row[9].upper() for row in block if row[0] == "fpr" and len(row) > 9]
    if len(fingerprints) != 1 or not FINGERPRINT_RE.fullmatch(fingerprints[0]):
        raise Refusal(f"release authority has an ambiguous or invalid primary {kind} fingerprint")
    parameters: dict[int, tuple[int, str]] = {}
    for row in block:
        if row[0] != "pkd" or len(row) < 4:
            continue
        try:
            parameters[int(row[1])] = (int(row[2]), row[3].upper())
        except ValueError as exc:
            raise Refusal("release authority contains malformed public-key parameters") from exc
    return records_[start], fingerprints[0], parameters


def governed_key(gpg: str, public_key: Path, key_id: str, env: dict[str, str]) -> tuple[str, bytes]:
    if public_key.is_symlink() or not public_key.is_file():
        raise Refusal(f"governed release public key is not a regular non-symlink file: {public_key}")
    mode = public_key.stat().st_mode
    if mode & 0o022:
        raise Refusal("governed release public key is group/world writable")
    public_output = run(
        [gpg, "--batch", "--with-colons", "--with-key-data", "--show-keys", str(public_key)],
        "governed release public-key inspection",
        env=env,
    )
    pub, fingerprint, parameters = primary(records(public_output), "pub")
    if len(pub) <= 3 or pub[3] != "22" or parameters.get(0) != (80, "092B06010401DA470F01"):
        raise Refusal("governed release primary key is not Ed25519")
    secret_output = run(
        [gpg, "--batch", "--with-colons", "--with-key-data", "--fingerprint", "--list-secret-keys", key_id],
        "governed release secret signing authority lookup",
        env=env,
    )
    _sec, secret_fingerprint, secret_parameters = primary(records(secret_output), "sec")
    if secret_fingerprint != fingerprint or secret_parameters != parameters:
        raise Refusal("secret signing authority does not match the governed release public key")
    point = parameters.get(1)
    if point is None or point[0] != 263 or len(point[1]) != 66 or not point[1].startswith("40"):
        raise Refusal("governed Ed25519 primary key has a non-canonical OpenPGP point")
    try:
        raw = bytes.fromhex(point[1][2:])
    except ValueError as exc:
        raise Refusal("governed Ed25519 primary key point is malformed") from exc
    if len(raw) != 32:
        raise Refusal("governed Ed25519 verification key is not 32 bytes")
    return fingerprint, raw


def revision(repo: Path, requested: str | None, env: dict[str, str]) -> str:
    value = requested or run(["git", "-C", str(repo), "rev-parse", "--verify", "HEAD^{commit}"], "source revision lookup", env=env).strip()
    if not REVISION_RE.fullmatch(value) or value == "0" * 40:
        raise Refusal("source revision must be one non-null lowercase 40-character Git commit")
    resolved = run(["git", "-C", str(repo), "rev-parse", "--verify", f"{value}^{{commit}}"], "source revision verification", env=env).strip()
    if resolved != value:
        raise Refusal("source revision is not the exact resolved Git commit")
    return value


def write_file(directory: Path, name: str, body: bytes) -> None:
    path = directory / name
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags, 0o400)
    try:
        os.fchmod(fd, 0o400)
        offset = 0
        while offset < len(body):
            written = os.write(fd, body[offset:])
            if written <= 0:
                raise Refusal(f"atomic trust-input write made no progress: {name}")
            offset += written
        os.fsync(fd)
    finally:
        os.close(fd)


def publish_directory_noreplace(source: Path, destination: Path) -> None:
    libc = ctypes.CDLL(None, use_errno=True)
    renameat2 = getattr(libc, "renameat2", None)
    if renameat2 is None:
        raise Refusal("atomic no-replace directory publication is unavailable on this release host")
    renameat2.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    renameat2.restype = ctypes.c_int
    result = renameat2(-100, os.fsencode(source), -100, os.fsencode(destination), 1)
    if result == 0:
        return
    error = ctypes.get_errno()
    if error in {errno.EEXIST, errno.ENOTEMPTY}:
        raise Refusal(f"output path appeared during publication; existing input was preserved: {destination}")
    raise Refusal(f"atomic no-replace trust-input publication failed: {os.strerror(error)}")


def produce(repo: Path, out_dir: Path, requested_revision: str | None, key_id: str, gpg: str, env: dict[str, str]) -> dict[str, object]:
    repo = repo.resolve(strict=True)
    if out_dir.exists() or out_dir.is_symlink():
        raise Refusal(f"output path already exists; choose a new trust-input directory: {out_dir}")
    source_revision = revision(repo, requested_revision, env)
    fingerprint, raw_key = governed_key(gpg, repo / "packaging/repo/RPM-GPG-KEY-magic-mesh", key_id, env)
    key_body = raw_key.hex().encode("ascii") + b"\n"
    receipt = {
        "schema_version": 1,
        "kind": "mcnf-flatpak-catalog-trust",
        "signer_id": f"openpgp-primary:{fingerprint}",
        "source_revision": source_revision,
        "verification_key_sha256": hashlib.sha256(key_body).hexdigest(),
    }
    receipt_body = json.dumps(receipt, sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"
    parent = out_dir.parent.resolve(strict=True)
    if parent.stat().st_mode & 0o022:
        raise Refusal("output parent must not be group/world writable")
    temporary: Path | None = Path(tempfile.mkdtemp(prefix=f".{out_dir.name}.", dir=parent))
    verify_stage = temporary / ".verified"
    try:
        temporary.chmod(0o700)
        write_file(temporary, "catalog-trust-receipt.json", receipt_body)
        write_file(temporary, "catalog-verification.key", key_body)
        verify_stage.mkdir(mode=0o700)
        verifier = repo / "install-helpers/verify-app-vm-catalog-trust.py"
        run(
            [sys.executable, str(verifier), "--receipt", str(temporary / "catalog-trust-receipt.json"), "--key", str(temporary / "catalog-verification.key"), "--expected-source-revision", source_revision, "--stage-dir", str(verify_stage)],
            "App VM catalog trust self-verification",
            env=env,
        )
        if (verify_stage / "catalog-trust-receipt.json").read_bytes() != receipt_body or (verify_stage / "catalog-verification.key").read_bytes() != key_body:
            raise Refusal("App VM catalog trust verifier changed the produced input")
        shutil.rmtree(verify_stage)
        parent_fd = os.open(parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(parent_fd)
        finally:
            os.close(parent_fd)
        publish_directory_noreplace(temporary, out_dir)
        temporary = None
        parent_fd = os.open(parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(parent_fd)
        finally:
            os.close(parent_fd)
    finally:
        if temporary is not None and temporary.exists():
            shutil.rmtree(temporary)
    return receipt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out-dir", required=True, type=Path)
    parser.add_argument("--source-revision")
    parser.add_argument("--release-key-id", default=os.environ.get("MAGIC_MESH_SIGN_KEY", "B546CC2EF9489F1899657AC9E6C820DAFBD1B07A"))
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1], help=argparse.SUPPRESS)
    parser.add_argument("--gpg", default="gpg", help=argparse.SUPPRESS)
    args = parser.parse_args()
    metadata = produce(args.repo, args.out_dir, args.source_revision, args.release_key_id, args.gpg, dict(os.environ))
    print(json.dumps(metadata, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Refusal as exc:
        print(f"REFUSED[WL-FUNC-018/catalog-trust-producer]: {exc}", file=sys.stderr)
        raise SystemExit(EXIT_REFUSED)
