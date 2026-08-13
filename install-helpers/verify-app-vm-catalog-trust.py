#!/usr/bin/env python3
"""Admit one immutable Flatpak catalog trust handoff for an App-VM build."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import tempfile
from pathlib import Path
from typing import Callable

MAX_RECEIPT_BYTES = 4096
MAX_KEY_BYTES = 256


class Refusal(RuntimeError):
    pass


def strict_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise Refusal(f"catalog trust receipt repeats field {key!r}")
        result[key] = value
    return result


def opened_bytes(path: Path, limit: int, label: str) -> bytes:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags)
    except OSError as exc:
        raise Refusal(f"{label} cannot be opened without following links: {exc}") from exc
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
            raise Refusal(f"{label} must be one regular, singly-linked file")
        if before.st_mode & 0o022:
            raise Refusal(f"{label} must not be group/world writable")
        if before.st_size <= 0 or before.st_size > limit:
            raise Refusal(f"{label} exceeds its bounded size")
        body = b""
        while len(body) <= limit:
            chunk = os.read(fd, min(65536, limit + 1 - len(body)))
            if not chunk:
                break
            body += chunk
        after = os.fstat(fd)
        if len(body) > limit or len(body) != before.st_size:
            raise Refusal(f"{label} changed or exceeded its bound while read")
        identity_before = (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
        identity_after = (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
        if identity_before != identity_after:
            raise Refusal(f"{label} changed while read")
        return body
    finally:
        os.close(fd)


def admit(receipt_path: Path, key_path: Path, expected_revision: str) -> tuple[bytes, bytes, dict[str, object]]:
    if not (len(expected_revision) == 40 and all(c in "0123456789abcdef" for c in expected_revision)) or expected_revision == "0" * 40:
        raise Refusal("expected source revision must be one non-null lowercase Git object ID")
    receipt_bytes = opened_bytes(receipt_path, MAX_RECEIPT_BYTES, "catalog trust receipt")
    key_bytes = opened_bytes(key_path, MAX_KEY_BYTES, "catalog verification key")
    try:
        receipt = json.loads(receipt_bytes, object_pairs_hook=strict_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise Refusal(f"catalog trust receipt is not strict JSON: {exc}") from exc
    expected_keys = {"schema_version", "kind", "signer_id", "source_revision", "verification_key_sha256"}
    if not isinstance(receipt, dict) or set(receipt) != expected_keys:
        raise Refusal("catalog trust receipt has an unexpected field set")
    if receipt["schema_version"] != 1 or receipt["kind"] != "mcnf-flatpak-catalog-trust":
        raise Refusal("catalog trust receipt contract identity is invalid")
    signer = receipt["signer_id"]
    if not isinstance(signer, str) or not signer or len(signer) > 128 or any(ord(c) < 0x21 or ord(c) > 0x7e for c in signer):
        raise Refusal("catalog signer ID is invalid")
    if receipt["source_revision"] != expected_revision:
        raise Refusal("catalog trust source revision does not match the App VM release input")
    try:
        key_text = key_bytes.decode("ascii").strip()
    except UnicodeDecodeError as exc:
        raise Refusal("catalog verification key is not ASCII") from exc
    if len(key_text) != 64 or any(c not in "0123456789abcdef" for c in key_text):
        raise Refusal("catalog verification key must be 64 lowercase hex characters")
    key_canonical = (key_text + "\n").encode()
    digest = hashlib.sha256(key_canonical).hexdigest()
    if receipt["verification_key_sha256"] != digest:
        raise Refusal("catalog verification key digest does not match the release receipt")
    return receipt_bytes, key_canonical, receipt


def stage_identity(stage_path: Path, stage_fd: int) -> tuple[int, int]:
    opened = os.fstat(stage_fd)
    try:
        named = os.stat(stage_path, follow_symlinks=False)
    except OSError as exc:
        raise Refusal(f"trust stage directory identity is unavailable: {exc}") from exc
    if not stat.S_ISDIR(opened.st_mode) or not stat.S_ISDIR(named.st_mode):
        raise Refusal("trust stage must be a directory")
    if (opened.st_dev, opened.st_ino) != (named.st_dev, named.st_ino):
        raise Refusal("trust stage directory was substituted")
    if opened.st_uid != os.geteuid() or opened.st_mode & 0o077:
        raise Refusal("trust stage directory must be private and owned by the verifier account")
    return opened.st_dev, opened.st_ino


def publish_at(stage_path: Path, stage_fd: int, name: str, body: bytes) -> None:
    stage_identity(stage_path, stage_fd)
    temporary = f".{name}.{os.getpid()}.{os.urandom(8).hex()}"
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(temporary, flags, 0o400, dir_fd=stage_fd)
    try:
        os.fchmod(fd, 0o400)
        with os.fdopen(fd, "wb", closefd=True) as stream:
            stream.write(body)
            stream.flush()
            os.fsync(stream.fileno())
        stage_identity(stage_path, stage_fd)
        os.link(
            temporary,
            name,
            src_dir_fd=stage_fd,
            dst_dir_fd=stage_fd,
            follow_symlinks=False,
        )
        os.unlink(temporary, dir_fd=stage_fd)
        os.fsync(stage_fd)
        stage_identity(stage_path, stage_fd)
    except BaseException:
        try:
            os.close(fd)
        except OSError:
            pass
        try:
            os.unlink(temporary, dir_fd=stage_fd)
        except FileNotFoundError:
            pass
        raise


def publish_stage(stage_path: Path, receipt: bytes, key: bytes) -> None:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_DIRECTORY", 0)
    try:
        stage_fd = os.open(stage_path, flags)
    except OSError as exc:
        raise Refusal(f"trust stage cannot be opened without following links: {exc}") from exc
    try:
        stage_identity(stage_path, stage_fd)
        publish_at(stage_path, stage_fd, "catalog-trust-receipt.json", receipt)
        publish_at(stage_path, stage_fd, "catalog-verification.key", key)
    except OSError as exc:
        raise Refusal(f"trust stage publication failed closed: {exc}") from exc
    finally:
        os.close(stage_fd)


def self_test() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        revision = "a" * 40
        key = ("07" * 32 + "\n").encode()
        key_path = root / "catalog.key"
        receipt_path = root / "receipt.json"
        key_path.write_bytes(key)
        key_path.chmod(0o400)
        receipt = {
            "schema_version": 1,
            "kind": "mcnf-flatpak-catalog-trust",
            "signer_id": "flatpak-release-v1",
            "source_revision": revision,
            "verification_key_sha256": hashlib.sha256(key).hexdigest(),
        }
        receipt_path.write_text(json.dumps(receipt, sort_keys=True) + "\n", encoding="utf-8")
        receipt_path.chmod(0o400)
        admit(receipt_path, key_path, revision)

        duplicate_path = root / "duplicate.json"
        duplicate_path.write_text(
            '{"schema_version":1,"schema_version":1,"kind":"mcnf-flatpak-catalog-trust",'
            f'"signer_id":"flatpak-release-v1","source_revision":"{revision}",'
            f'"verification_key_sha256":"{hashlib.sha256(key).hexdigest()}"}}\n',
            encoding="utf-8",
        )
        duplicate_path.chmod(0o400)

        def refused(label: str, action: Callable[[], object]) -> None:
            try:
                action()
            except Refusal:
                return
            raise AssertionError(f"{label} was accepted")

        refused("missing trust key", lambda: admit(receipt_path, root / "missing.key", revision))
        refused("duplicate receipt field", lambda: admit(duplicate_path, key_path, revision))
        refused("mismatched source revision", lambda: admit(receipt_path, key_path, "b" * 40))
        receipt_path.chmod(0o622)
        refused("mutable trust receipt", lambda: admit(receipt_path, key_path, revision))
        receipt_path.chmod(0o400)
        key_path.chmod(0o622)
        refused("mutable trust key", lambda: admit(receipt_path, key_path, revision))
        key_path.chmod(0o600)
        key_path.write_text("08" * 32 + "\n", encoding="ascii")
        key_path.chmod(0o400)
        refused("replaced trust key", lambda: admit(receipt_path, key_path, revision))
        key_path.chmod(0o600)
        key_path.write_bytes(key)
        key_path.chmod(0o400)
        hostile = dict(receipt)
        hostile["verification_key_sha256"] = "0" * 64
        receipt_path.chmod(0o600)
        receipt_path.write_text(json.dumps(hostile) + "\n", encoding="utf-8")
        receipt_path.chmod(0o400)
        refused("replaced key digest", lambda: admit(receipt_path, key_path, revision))
        receipt_path.unlink()
        receipt_path.symlink_to(key_path)
        refused("symlink receipt", lambda: admit(receipt_path, key_path, revision))

        stage = root / "stage"
        stage.mkdir(mode=0o700)
        publish_stage(stage, b"receipt\n", key)
        if (stage / "catalog-verification.key").read_bytes() != key:
            raise AssertionError("pinned stage publication changed key bytes")
        symlink_stage = root / "stage-link"
        symlink_stage.symlink_to(stage, target_is_directory=True)
        refused("symlink stage", lambda: publish_stage(symlink_stage, b"receipt\n", key))
        pinned = root / "pinned-stage"
        pinned.mkdir(mode=0o700)
        pinned_fd = os.open(pinned, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            stage_identity(pinned, pinned_fd)
            replacement = root / "replacement-stage"
            replacement.mkdir(mode=0o700)
            pinned.rename(root / "original-stage")
            replacement.rename(pinned)
            refused("substituted stage", lambda: stage_identity(pinned, pinned_fd))
        finally:
            os.close(pinned_fd)
    print("verify-app-vm-catalog-trust: self-test passed")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--receipt", type=Path)
    parser.add_argument("--key", type=Path)
    parser.add_argument("--expected-source-revision")
    parser.add_argument("--stage-dir", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0
    if None in (args.receipt, args.key, args.expected_source_revision, args.stage_dir):
        parser.error("--receipt, --key, --expected-source-revision, and --stage-dir are required")
    receipt, key, metadata = admit(args.receipt, args.key, args.expected_source_revision)
    publish_stage(args.stage_dir, receipt, key)
    print(json.dumps(metadata, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Refusal as exc:
        print(f"REFUSED[WL-FUNC-018/catalog-trust]: {exc}", file=os.sys.stderr)
        raise SystemExit(2)
