#!/usr/bin/env python3
"""Verify one immutable App VM qcow2 derivative and its release manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys


MAX_IMAGE = 128 * 1024**3
MAX_MANIFEST = 1024 * 1024
REVISION = re.compile(r"[0-9a-f]{40}\Z")
DIGEST = re.compile(r"sha256:[0-9a-f]{64}\Z")
PROFILE = "mcnf-app-vm/wayland-standard-v1"
KIND = "mcnf-app-vm-image-manifest"


class Refusal(ValueError):
    pass


def identity(value: os.stat_result) -> tuple[int, ...]:
    return (value.st_dev, value.st_ino, value.st_mode, value.st_nlink,
            value.st_uid, value.st_gid, value.st_size, value.st_mtime_ns,
            value.st_ctime_ns)


def regular(path: Path, label: str, maximum: int) -> os.stat_result:
    try:
        value = path.lstat()
    except OSError as exc:
        raise Refusal(f"{label} metadata is unavailable: {exc}") from exc
    if not stat.S_ISREG(value.st_mode) or stat.S_ISLNK(value.st_mode) or value.st_nlink != 1:
        raise Refusal(f"{label} must be a single-link regular non-symlink file")
    if not 0 < value.st_size <= maximum or value.st_mode & 0o022:
        raise Refusal(f"{label} is empty, oversized, or group/other writable")
    return value


def exact_json(path: Path) -> dict[str, object]:
    regular(path, "App VM manifest", MAX_MANIFEST)

    def pairs(items: list[tuple[str, object]]) -> dict[str, object]:
        value: dict[str, object] = {}
        for key, item in items:
            if key in value:
                raise Refusal(f"manifest contains duplicate field {key}")
            value[key] = item
        return value

    try:
        value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=pairs)
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise Refusal(f"manifest is not exact JSON: {exc}") from exc
    if not isinstance(value, dict):
        raise Refusal("manifest must be one JSON object")
    return value


def digest_image(path: Path, before: os.stat_result) -> str:
    descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
    digest = hashlib.sha256()
    try:
        opened = os.fstat(descriptor)
        if identity(opened) != identity(before):
            raise Refusal("App VM image changed before verification")
        if os.read(descriptor, 4) != b"QFI\xfb":
            raise Refusal("App VM image is not a qcow2 artifact")
        os.lseek(descriptor, 0, os.SEEK_SET)
        remaining = before.st_size
        while remaining:
            chunk = os.read(descriptor, min(1024 * 1024, remaining))
            if not chunk:
                raise Refusal("App VM image shrank during verification")
            digest.update(chunk)
            remaining -= len(chunk)
        if os.read(descriptor, 1):
            raise Refusal("App VM image grew during verification")
        after = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    if identity(after) != identity(before) or identity(path.lstat()) != identity(before):
        raise Refusal("App VM image changed during verification")
    return "sha256:" + digest.hexdigest()


def verify(image: Path, manifest: Path, revision: str) -> dict[str, object]:
    if REVISION.fullmatch(revision) is None or revision == "0" * 40:
        raise Refusal("source revision must be one non-null lowercase Git object ID")
    before = regular(image, "App VM image", MAX_IMAGE)
    value = exact_json(manifest)
    if set(value) != {"artifact", "image_profile", "kind", "schema_version", "source_revision"}:
        raise Refusal("manifest fields are not exact")
    if value.get("schema_version") != 1 or value.get("kind") != KIND:
        raise Refusal("manifest identity is unsupported")
    if value.get("image_profile") != PROFILE or value.get("source_revision") != revision:
        raise Refusal("manifest profile or source revision does not match")
    artifact = value.get("artifact")
    if not isinstance(artifact, dict) or set(artifact) != {"filename", "sha256", "size"}:
        raise Refusal("manifest artifact fields are not exact")
    if artifact.get("filename") != image.name or artifact.get("size") != before.st_size:
        raise Refusal("manifest filename or size does not match the App VM image")
    expected = artifact.get("sha256")
    if not isinstance(expected, str) or DIGEST.fullmatch(expected) is None or expected == "sha256:" + "0" * 64:
        raise Refusal("manifest image digest is malformed")
    if digest_image(image, before) != expected:
        raise Refusal("App VM image digest does not match the manifest")
    return value


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--source-revision", required=True)
    args = parser.parse_args()
    try:
        value = verify(args.image, args.manifest, args.source_revision)
    except (OSError, Refusal) as exc:
        print(f"verify-app-vm-qcow2: REFUSED: {exc}", file=sys.stderr)
        return 2
    print(json.dumps(value, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
