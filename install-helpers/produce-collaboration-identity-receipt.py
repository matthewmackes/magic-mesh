#!/usr/bin/env python3
"""Produce a signed, non-secret Collaboration identity release receipt."""

from __future__ import annotations

import argparse, hashlib, json, os, re, stat, subprocess, sys, tempfile
from pathlib import Path

REVISION = re.compile(r"[0-9a-f]{40}")
HEX32 = re.compile(r"[0-9a-f]{64}")
NODE = re.compile(r"peer:[A-Za-z0-9][A-Za-z0-9._-]{0,127}")

class Refusal(RuntimeError): pass

def run(argv: list[str], env: dict[str, str]) -> str:
    result = subprocess.run(argv, text=True, capture_output=True, env=env, check=False)
    if result.returncode:
        raise Refusal(result.stderr.strip().splitlines()[-1] if result.stderr.strip() else f"command failed: {argv[0]}")
    return result.stdout

def stable(path: Path, secret: bool = False) -> tuple[os.stat_result, bytes]:
    before = path.lstat()
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1 or before.st_mode & 0o022:
        raise Refusal(f"{path} must be one non-writable regular file")
    if secret and before.st_mode & 0o077:
        raise Refusal("SecretStore export must be owner-only")
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_CLOEXEC", 0))
    try:
        opened = os.fstat(fd); body = os.read(fd, 4097); after = os.fstat(fd)
    finally: os.close(fd)
    if (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns) != (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) or (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns) != (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns):
        raise Refusal(f"{path} changed while being read")
    if secret and len(body) != 32:
        raise Refusal("Collaboration SecretStore seed must be exactly 32 bytes")
    return before, body

def fingerprint(gpg: str, key: str, public_key: Path, env: dict[str, str]) -> str:
    shown = run([gpg, "--batch", "--with-colons", "--fingerprint", "--show-keys", str(public_key)], env)
    secret = run([gpg, "--batch", "--with-colons", "--fingerprint", "--list-secret-keys", key], env)
    def one(text: str, kind: str) -> str:
        rows = [row.split(":") for row in text.splitlines() if row]
        starts = [index for index, row in enumerate(rows) if row[0] == kind]
        if len(starts) != 1:
            raise Refusal("release authority has an ambiguous primary key record")
        start = starts[0]
        end = next((index for index in range(start + 1, len(rows)) if rows[index][0] in {"pub", "sec", "sub", "ssb"}), len(rows))
        values = [row[9].upper() for row in rows[start:end] if row[0] == "fpr" and len(row) > 9]
        if len(values) != 1 or not re.fullmatch(r"[0-9A-F]{40}|[0-9A-F]{64}", values[0]):
            raise Refusal("release authority has an ambiguous primary fingerprint")
        return values[0]
    public, private = one(shown, "pub"), one(secret, "sec")
    if public != private: raise Refusal("secret authority does not match governed public key")
    return public

def main() -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--secret-store-export", required=True, type=Path)
    p.add_argument("--public-key-hex", required=True)
    p.add_argument("--source-revision", required=True)
    p.add_argument("--target-node", required=True); p.add_argument("--target-user", default="system:mackesd")
    p.add_argument("--output-receipt", required=True, type=Path)
    p.add_argument("--release-public-key", required=True, type=Path)
    p.add_argument("--release-key-id", required=True); p.add_argument("--gpg", default="gpg")
    a = p.parse_args(); env = dict(os.environ)
    if not HEX32.fullmatch(a.public_key_hex) or not REVISION.fullmatch(a.source_revision) or not NODE.fullmatch(a.target_node) or a.target_user != "system:mackesd":
        raise Refusal("public identity, revision, node, or user scope is invalid")
    _, seed = stable(a.secret_store_export, True); stable(a.release_public_key)
    signer = fingerprint(a.gpg, a.release_key_id, a.release_public_key, env)
    if a.output_receipt.exists() or a.output_receipt.is_symlink() or a.output_receipt.with_suffix(a.output_receipt.suffix + ".asc").exists():
        raise Refusal("receipt output already exists")
    receipt = {"schema_version":1,"kind":"mcnf-collaboration-identity-admission","public_key_hex":a.public_key_hex,"seed_sha256":hashlib.sha256(seed).hexdigest(),"source_revision":a.source_revision,"target_node":a.target_node,"target_user":a.target_user,"release_signer":signer}
    body = json.dumps(receipt, sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"
    a.output_receipt.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".collaboration-receipt.", dir=a.output_receipt.parent)
    try:
        os.fchmod(fd, 0o400); os.write(fd, body); os.fsync(fd); os.close(fd); fd = -1
        signature = temporary + ".asc"
        run([a.gpg,"--batch","--armor","--detach-sign","--local-user",signer,"--output",signature,temporary], env)
        os.chmod(signature, 0o400); os.link(temporary, a.output_receipt); os.link(signature, str(a.output_receipt)+".asc")
    finally:
        if fd >= 0: os.close(fd)
        for path in (temporary, temporary + ".asc"):
            try: os.unlink(path)
            except FileNotFoundError: pass
    print(json.dumps(receipt, sort_keys=True, separators=(",", ":")))
    return 0

if __name__ == "__main__":
    try: raise SystemExit(main())
    except (OSError, Refusal) as exc:
        print(f"REFUSED[WL-FUNC-011/collaboration-identity-producer]: {exc}", file=sys.stderr); raise SystemExit(2)
