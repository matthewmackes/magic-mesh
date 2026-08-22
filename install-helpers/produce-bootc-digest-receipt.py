#!/usr/bin/env python3
"""Resolve and attest one immutable bootc registry manifest/list digest."""

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
MAX_REGISTRY_DOCUMENT = 16 * 1024 * 1024
MAX_RECEIPT = 8192
DIGEST_RE = re.compile(r"sha256:[0-9a-f]{64}")
REVISION_RE = re.compile(r"[0-9a-f]{40}|[0-9a-f]{64}")
ARCH_RE = re.compile(r"[a-z0-9][a-z0-9_.-]{0,31}")
ROLE_RE = re.compile(r"[a-z0-9][a-z0-9-]{0,63}")
EPOCH_RE = re.compile(r"[1-9][0-9]{0,11}")
CANONICAL_ROLE = "all-roles"
LEGACY_ROLES = frozenset({"base", "unified-seat-server"})
LIST_MEDIA_TYPES = {
    "application/vnd.docker.distribution.manifest.list.v2+json",
    "application/vnd.oci.image.index.v1+json",
}
MANIFEST_MEDIA_TYPES = {
    "application/vnd.docker.distribution.manifest.v2+json",
    "application/vnd.oci.image.manifest.v1+json",
}


class Refusal(RuntimeError):
    pass


def run(command: list[str], label: str, limit: int = MAX_REGISTRY_DOCUMENT) -> bytes:
    try:
        result = subprocess.run(command, check=False, capture_output=True, timeout=30)
    except subprocess.TimeoutExpired as exc:
        raise Refusal(f"{label} exceeded the 30-second inspection bound") from exc
    except OSError as exc:
        raise Refusal(f"{label} is unavailable: {exc}") from exc
    if result.returncode != 0:
        detail = result.stderr.decode("utf-8", "replace").strip().splitlines()
        suffix = f": {detail[-1]}" if detail else ""
        raise Refusal(f"{label} failed{suffix}")
    if not result.stdout or len(result.stdout) > limit:
        raise Refusal(f"{label} output is empty or exceeds the bounded limit")
    return result.stdout


def parse_json(body: bytes, label: str) -> dict[str, object]:
    try:
        value = json.loads(body)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise Refusal(f"{label} is not valid JSON") from exc
    if not isinstance(value, dict):
        raise Refusal(f"{label} must be a JSON object")
    return value


def validate_release(repo: Path, revision: str, epoch_text: str) -> tuple[str, int]:
    if not REVISION_RE.fullmatch(revision) or set(revision) == {"0"}:
        raise Refusal("source revision must be one non-null lowercase Git object ID")
    if not EPOCH_RE.fullmatch(epoch_text):
        raise Refusal("commit epoch must be one non-null bounded integer")
    resolved = run(["git", "-C", str(repo), "rev-parse", "--verify", f"{revision}^{{commit}}"], "source revision verification", 256).decode().strip()
    if resolved != revision:
        raise Refusal("source revision is not the exact resolved Git commit")
    actual_epoch = run(["git", "-C", str(repo), "show", "-s", "--format=%ct", revision], "source commit epoch lookup", 256).decode().strip()
    if actual_epoch != epoch_text:
        raise Refusal("commit epoch does not match the exact source revision")
    return revision, int(epoch_text)


def validate_identity(reference: str, architecture: str, role: str) -> None:
    if not reference or len(reference) > 1024 or any(ord(ch) < 0x21 or ord(ch) > 0x7e for ch in reference):
        raise Refusal("image reference is empty, overlong, or contains unsafe characters")
    if reference.startswith("docker://"):
        raise Refusal("image reference must omit the transport prefix")
    if not ARCH_RE.fullmatch(architecture):
        raise Refusal("architecture is invalid")
    if not ROLE_RE.fullmatch(role):
        raise Refusal("release role is invalid")
    if role in LEGACY_ROLES:
        raise Refusal(f"bootc receipt refuses legacy {role} role identity")
    if role != CANONICAL_ROLE:
        raise Refusal("bootc receipt must use the canonical all-roles release role")


def resolve(skopeo: str, reference: str, architecture: str) -> tuple[str, str]:
    raw = run([skopeo, "inspect", "--raw", f"docker://{reference}"], "bounded bootc manifest inspection")
    manifest = parse_json(raw, "registry manifest")
    media_type = manifest.get("mediaType")
    if media_type not in LIST_MEDIA_TYPES | MANIFEST_MEDIA_TYPES:
        raise Refusal("registry returned an unsupported manifest media type")
    digest = "sha256:" + hashlib.sha256(raw).hexdigest()
    if digest == "sha256:" + "0" * 64:
        raise Refusal("registry resolved a null digest")
    if media_type in LIST_MEDIA_TYPES:
        entries = manifest.get("manifests")
        if not isinstance(entries, list):
            raise Refusal("manifest list has no platform entries")
        matches = []
        for entry in entries:
            if not isinstance(entry, dict) or not isinstance(entry.get("platform"), dict):
                continue
            platform = entry["platform"]
            if platform.get("os") == "linux" and platform.get("architecture") == architecture and not platform.get("variant"):
                matches.append(entry)
        if len(matches) != 1:
            raise Refusal(f"manifest list must contain exactly one linux/{architecture} entry without a variant")
        selected = matches[0].get("digest")
        if not isinstance(selected, str) or not DIGEST_RE.fullmatch(selected) or selected.endswith("0" * 64):
            raise Refusal("selected platform manifest has an invalid digest")
    else:
        config = parse_json(run([skopeo, "inspect", "--config", f"docker://{reference}"], "bounded bootc config inspection"), "image config")
        if config.get("os") != "linux" or config.get("architecture") != architecture:
            raise Refusal(f"single manifest is not linux/{architecture}")
    return digest, str(media_type)


def document(repo: Path, skopeo: str, reference: str, architecture: str, revision: str, epoch: str, role: str) -> dict[str, object]:
    validate_identity(reference, architecture, role)
    revision, commit_epoch = validate_release(repo, revision, epoch)
    digest, media_type = resolve(skopeo, reference, architecture)
    return {
        "schema_version": 1,
        "kind": "mcnf-bootc-image-digest",
        "image_reference": reference,
        "resolved_digest": digest,
        "manifest_media_type": media_type,
        "os": "linux",
        "architecture": architecture,
        "source_revision": revision,
        "commit_epoch": commit_epoch,
        "release_role": role,
    }


def read_regular(path: Path) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise Refusal("bootc digest receipt is not a regular non-symlink file")
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0))
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode) or before.st_size <= 0 or before.st_size > MAX_RECEIPT:
            raise Refusal("bootc digest receipt size is outside the allowed bound")
        body = os.read(fd, MAX_RECEIPT + 1)
        after = os.fstat(fd)
        if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
            raise Refusal("bootc digest receipt changed while reading")
    finally:
        os.close(fd)
    return body


EXPECTED_KEYS = {"schema_version", "kind", "image_reference", "resolved_digest", "manifest_media_type", "os", "architecture", "source_revision", "commit_epoch", "release_role"}


def inspect_receipt(repo: Path, path: Path, reference: str, architecture: str, revision: str, epoch: str, role: str) -> dict[str, object]:
    validate_identity(reference, architecture, role)
    revision, commit_epoch = validate_release(repo, revision, epoch)
    body = read_regular(path)
    value = parse_json(body, "bootc digest receipt")
    canonical = json.dumps(value, sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"
    if body != canonical or set(value) != EXPECTED_KEYS:
        raise Refusal("bootc digest receipt is non-canonical or has an unexpected schema")
    if value.get("schema_version") != 1 or value.get("kind") != "mcnf-bootc-image-digest":
        raise Refusal("bootc digest receipt kind or version is unsupported")
    expected = {"image_reference": reference, "os": "linux", "architecture": architecture, "source_revision": revision, "commit_epoch": commit_epoch, "release_role": role}
    if any(value.get(key) != item for key, item in expected.items()):
        raise Refusal("bootc digest receipt does not match the requested release identity")
    digest = value.get("resolved_digest")
    if not isinstance(digest, str) or not DIGEST_RE.fullmatch(digest) or digest.endswith("0" * 64):
        raise Refusal("bootc digest receipt contains an invalid immutable digest")
    if value.get("manifest_media_type") not in LIST_MEDIA_TYPES | MANIFEST_MEDIA_TYPES:
        raise Refusal("bootc digest receipt contains an unsupported media type")
    return value


def atomic_write(path: Path, body: bytes) -> None:
    if path.exists() or path.is_symlink():
        raise Refusal(f"receipt output already exists: {path}")
    parent = path.parent.resolve(strict=True)
    fd, name = tempfile.mkstemp(prefix=f".{path.name}.", dir=parent)
    temporary = Path(name)
    try:
        os.fchmod(fd, 0o400)
        offset = 0
        while offset < len(body):
            written = os.write(fd, body[offset:])
            if written <= 0:
                raise Refusal("receipt write made no progress")
            offset += written
        os.fsync(fd)
        os.close(fd); fd = -1
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
        try: os.fsync(parent_fd)
        finally: os.close(parent_fd)
    finally:
        if fd >= 0: os.close(fd)
        try: temporary.unlink()
        except FileNotFoundError: pass


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--skopeo", default="skopeo")
    sub = parser.add_subparsers(dest="command", required=True)
    produce = sub.add_parser("produce")
    produce.add_argument("--image-reference", required=True)
    produce.add_argument("--architecture", required=True)
    produce.add_argument("--source-revision", required=True)
    produce.add_argument("--commit-epoch", required=True)
    produce.add_argument("--release-role", required=True)
    produce.add_argument("--output", required=True, type=Path)
    inspect_parser = sub.add_parser("inspect")
    inspect_parser.add_argument("--receipt", required=True, type=Path)
    inspect_parser.add_argument("--expected-image-reference", required=True)
    inspect_parser.add_argument("--expected-architecture", required=True)
    inspect_parser.add_argument("--expected-source-revision", required=True)
    inspect_parser.add_argument("--expected-commit-epoch", required=True)
    inspect_parser.add_argument("--expected-release-role", required=True)
    args = parser.parse_args()
    try:
        if args.command == "produce":
            value = document(args.repo, args.skopeo, args.image_reference, args.architecture, args.source_revision, args.commit_epoch, args.release_role)
            body = json.dumps(value, sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n"
            atomic_write(args.output, body)
        else:
            value = inspect_receipt(args.repo, args.receipt, args.expected_image_reference, args.expected_architecture, args.expected_source_revision, args.expected_commit_epoch, args.expected_release_role)
        print(json.dumps(value, sort_keys=True, separators=(",", ":")))
        return 0
    except (Refusal, OSError, UnicodeError, ValueError) as exc:
        print(f"bootc-digest-receipt: REFUSED: {exc}", file=sys.stderr)
        return EXIT_REFUSED


if __name__ == "__main__":
    raise SystemExit(main())
