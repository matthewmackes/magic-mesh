#!/usr/bin/env python3
"""Mint a dest-backed Ed25519 lifecycle confirmation signing key.

Operator 2026-08-23: create what leftover (3) requires; no other source
exists. Writes the 32-byte seed only to ``--output`` (mode 0600). Sidecar
carries verifying-key sha256 only. Never prints seed, signature, or
OPENSSH material. Never marks production_admitted. Does not leave, join,
or enroll.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import sys
import tempfile
from pathlib import Path

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import (
    Encoding,
    NoEncryption,
    PrivateFormat,
    PublicFormat,
)

KIND = "mcnf-lifecycle-confirmation-dest"
KEY_ID = "lifecycle-confirmation-v1"
DEST_MODE = 0o600
SIDECAR_MODE = 0o400
SEED_LEN = 32
EXIT_REFUSED = 2


class Refusal(ValueError):
    pass


def refuse(message: str) -> None:
    raise Refusal(message)


def dest_resolved(path: Path) -> Path:
    return path.parent.resolve() / path.name


def publish(path: Path, body: bytes, mode: int) -> None:
    if path.exists() or path.is_symlink():
        refuse("dest already exists; refusing replace")
    parent = path.parent.resolve(strict=True)
    if not parent.is_dir() or parent.stat().st_mode & 0o022:
        refuse("dest parent must be a private real directory")
    directory = Path(tempfile.mkdtemp(prefix=f".{path.name}.", dir=parent))
    try:
        directory.chmod(0o700)
        staged = directory / "body"
        descriptor = os.open(staged, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(body)
            stream.flush()
            os.fsync(stream.fileno())
        os.link(staged, path, follow_symlinks=False)
        staged.unlink()
        parent_fd = os.open(parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(parent_fd)
        finally:
            os.close(parent_fd)
    finally:
        try:
            directory.rmdir()
        except OSError:
            pass


def mint(output: Path, sidecar: Path) -> dict[str, object]:
    dest = dest_resolved(output)
    side = dest_resolved(sidecar)
    key = Ed25519PrivateKey.generate()
    seed = key.private_bytes(Encoding.Raw, PrivateFormat.Raw, NoEncryption())
    if len(seed) != SEED_LEN:
        refuse("ed25519 seed must be 32 bytes")
    verifying = key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
    if len(verifying) != SEED_LEN:
        refuse("ed25519 verifying key must be 32 bytes")
    publish(dest, seed, DEST_MODE)
    record = {
        "enroll_succeeded": False,
        "key_id": KEY_ID,
        "kind": KIND,
        "production_admitted": False,
        "schema_version": 1,
        "sidecar_path": str(side),
        "verifying_key_sha256": hashlib.sha256(verifying).hexdigest(),
    }
    if record["production_admitted"] is not False:
        refuse("helper must never mark production_admitted")
    publish(side, (json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii"), SIDECAR_MODE)
    return record


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--sidecar", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        record = mint(args.output, args.sidecar)
        print(
            "mint-lifecycle-confirmation-dest: wrote dest; "
            f"verifying_key_sha256={record['verifying_key_sha256']}; "
            "production_admitted=false"
        )
        return 0
    except (OSError, Refusal, ValueError) as error:
        print(f"mint-lifecycle-confirmation-dest: REFUSED: {error}", file=sys.stderr)
        return EXIT_REFUSED


if __name__ == "__main__":
    raise SystemExit(main())
