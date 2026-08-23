#!/usr/bin/env python3
"""Sign a LifecycleConfirmationV1 from a dest-backed Ed25519 seed.

Writes confirmation JSON only to ``--output``. Never prints the seed.
Stdout names dest path and verifying-key sha256 only. production_admitted
stays false. Does not run leave, join, or enroll.
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
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

DOMAIN = "magic-mesh:lifecycle-confirmation:v1"
KEY_ID = "lifecycle-confirmation-v1"
PHRASE = "FORCE OFFBOARD 1 SYSTEMS"
EXIT_REFUSED = 2
HEX32 = 32
HEX64 = 64


class Refusal(ValueError):
    pass


def refuse(message: str) -> None:
    raise Refusal(message)


def dest_resolved(path: Path) -> Path:
    return path.parent.resolve() / path.name


def load_seed(path: Path) -> bytes:
    dest = dest_resolved(path)
    try:
        meta = dest.lstat()
    except OSError as error:
        refuse("confirmation seed dest is missing")
        raise AssertionError from error
    if stat.S_ISLNK(meta.st_mode):
        refuse("confirmation seed dest is a symlink")
    if not stat.S_ISREG(meta.st_mode):
        refuse("confirmation seed dest must be a regular file")
    if stat.S_IMODE(meta.st_mode) & 0o077:
        refuse("confirmation seed dest must be mode 0600")
    seed = dest.read_bytes()
    if len(seed) != HEX32:
        refuse("confirmation seed dest must be 32 raw bytes")
    return seed


def signing_bytes(
    session_id: str,
    generation: int,
    scope_digest_hex: str,
) -> bytes:
    return (
        f"{DOMAIN}|1|{session_id}|\"offboard\"|1|{scope_digest_hex}|{PHRASE}|{generation}|{KEY_ID}"
    ).encode("utf-8")


def sign_confirmation(
    seed_dest: Path,
    session_id: str,
    generation: int,
    scope_digest_hex: str,
    output: Path,
) -> dict[str, object]:
    if not session_id.startswith("offboard-peer:") or len(session_id) > 256:
        refuse("session_id must be an offboard-peer request id")
    if generation < 1:
        refuse("generation must be positive")
    if len(scope_digest_hex) != HEX64 or any(c not in "0123456789abcdef" for c in scope_digest_hex):
        refuse("scope_digest_hex must be 64 lowercase hex characters")
    seed = load_seed(seed_dest)
    key = Ed25519PrivateKey.from_private_bytes(seed)
    signature = key.sign(signing_bytes(session_id, generation, scope_digest_hex))
    verifying = key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
    confirmation = {
        "action": "offboard",
        "generation": generation,
        "key_id": KEY_ID,
        "phrase": PHRASE,
        "schema_version": 1,
        "scope_digest_hex": scope_digest_hex,
        "session_id": session_id,
        "signature_hex": signature.hex(),
        "target_count": 1,
    }
    dest = dest_resolved(output)
    if dest.exists() or dest.is_symlink():
        refuse("confirmation dest already exists; refusing replace")
    parent = dest.parent.resolve(strict=True)
    if not parent.is_dir() or parent.stat().st_mode & 0o022:
        refuse("confirmation dest parent must be a private real directory")
    body = (json.dumps(confirmation, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii")
    directory = Path(tempfile.mkdtemp(prefix=f".{dest.name}.", dir=parent))
    try:
        directory.chmod(0o700)
        staged = directory / "body"
        descriptor = os.open(staged, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(body)
            stream.flush()
            os.fsync(stream.fileno())
        os.link(staged, dest, follow_symlinks=False)
        staged.unlink()
    finally:
        try:
            directory.rmdir()
        except OSError:
            pass
    return {
        "confirmation_dest": str(dest),
        "kind": "mcnf-lifecycle-confirmation-v1",
        "production_admitted": False,
        "verifying_key_hex": verifying.hex(),
        "verifying_key_sha256": hashlib.sha256(verifying).hexdigest(),
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seed", type=Path, required=True)
    parser.add_argument("--session-id", required=True)
    parser.add_argument("--generation", type=int, required=True)
    parser.add_argument("--scope-digest-hex", required=True)
    parser.add_argument("--output", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        record = sign_confirmation(
            args.seed,
            args.session_id,
            args.generation,
            args.scope_digest_hex,
            args.output,
        )
        print(
            "sign-lifecycle-confirmation: wrote dest; "
            f"verifying_key_sha256={record['verifying_key_sha256']}; "
            "production_admitted=false"
        )
        return 0
    except (OSError, Refusal, ValueError) as error:
        print(f"sign-lifecycle-confirmation: REFUSED: {error}", file=sys.stderr)
        return EXIT_REFUSED


if __name__ == "__main__":
    raise SystemExit(main())
