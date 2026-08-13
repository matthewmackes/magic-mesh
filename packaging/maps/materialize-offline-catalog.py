#!/usr/bin/env python3
"""Atomically materialize a governed offline Maps bundle for first release."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath

MAX_MANIFEST_BYTES = 8 * 1024 * 1024
MAX_CATALOG_BYTES = 256 * 1024
MAX_INDEX_BYTES = 8 * 1024 * 1024
MAX_TILE_BYTES = 4 * 1024 * 1024
MAX_TILES = 100_000
SHA256_LENGTH = 64


class Refusal(ValueError):
    pass


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def exact_object(data: bytes, expected: set[str], label: str) -> dict:
    try:
        value = json.loads(data)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise Refusal(f"{label} is not valid JSON: {error}") from error
    if not isinstance(value, dict) or set(value) != expected:
        raise Refusal(f"{label} fields are not exact")
    return value


def safe_sha(value: object, label: str) -> str:
    if not isinstance(value, str) or len(value) != SHA256_LENGTH or any(
        byte not in "0123456789abcdef" for byte in value
    ):
        raise Refusal(f"{label} is not lower-case SHA-256")
    return value


def immutable_file(path: Path, maximum: int, label: str) -> bytes:
    before = path.lstat()
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        raise Refusal(f"{label} must be a singly-linked regular file")
    if before.st_mode & 0o222:
        raise Refusal(f"{label} is writable")
    if before.st_size == 0 or before.st_size > maximum:
        raise Refusal(f"{label} size is outside its bound")
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        opened = os.fstat(descriptor)
        data = b""
        while len(data) <= maximum:
            chunk = os.read(descriptor, min(1024 * 1024, maximum + 1 - len(data)))
            if not chunk:
                break
            data += chunk
    finally:
        os.close(descriptor)
    after = path.lstat()
    identity = lambda item: (item.st_dev, item.st_ino, item.st_size, item.st_mtime_ns)
    if identity(before) != identity(opened) or identity(opened) != identity(after):
        raise Refusal(f"{label} changed while being read")
    if len(data) != before.st_size:
        raise Refusal(f"{label} changed length while being read")
    return data


def real_directory(path: Path, label: str, immutable: bool = False) -> None:
    metadata = path.lstat()
    if not stat.S_ISDIR(metadata.st_mode):
        raise Refusal(f"{label} must be a real directory")
    if immutable and metadata.st_mode & 0o222:
        raise Refusal(f"{label} is writable")


def safe_relative(value: object, label: str) -> PurePosixPath:
    if not isinstance(value, str):
        raise Refusal(f"{label} is not a path")
    path = PurePosixPath(value)
    if (
        not value
        or path.is_absolute()
        or "\\" in value
        or any(part in ("", ".", "..") for part in path.parts)
    ):
        raise Refusal(f"{label} is unsafe")
    return path


def beneath_immutable(root: Path, relative: PurePosixPath, label: str) -> Path:
    current = root
    for component in relative.parts[:-1]:
        current /= component
        real_directory(current, f"{label} parent", immutable=True)
    return current / relative.parts[-1]


def run_verifier(verifier: Path, bundle: Path) -> None:
    metadata = verifier.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
        raise Refusal("Maps verifier must be a singly-linked regular file")
    if metadata.st_mode & 0o022 or not metadata.st_mode & 0o111:
        raise Refusal("Maps verifier must be executable and not group/world writable")
    completed = subprocess.run(
        [str(verifier.resolve(strict=True)), str(bundle)],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=120,
        check=False,
    )
    if completed.returncode != 0:
        detail = completed.stderr.decode(errors="replace").strip()[-2048:]
        raise Refusal(f"production Maps verifier rejected bundle: {detail}")


def validate_bundle(
    bundle: Path, expected_revision: str, expected_epoch: int, expected_quota: int
) -> tuple[bytes, bytes, list[tuple[PurePosixPath, bytes]]]:
    real_directory(bundle, "bundle root", immutable=True)
    manifest_bytes = immutable_file(bundle / "manifest.json", MAX_MANIFEST_BYTES, "manifest")
    manifest = exact_object(
        manifest_bytes,
        {
            "schema", "kind", "provider", "attribution", "license",
            "source_revision", "source_epoch", "quota_bytes", "payload_bytes",
            "catalog_sha256", "cache_index_sha256", "regions",
        },
        "manifest",
    )
    if manifest["schema"] != 1 or manifest["kind"] != "mcnf-offline-map-catalog":
        raise Refusal("manifest schema/kind is unsupported")
    if manifest["source_revision"] != expected_revision:
        raise Refusal("manifest source revision differs from release revision")
    if manifest["source_epoch"] != expected_epoch:
        raise Refusal("manifest source epoch differs from release epoch")
    if manifest["quota_bytes"] != expected_quota:
        raise Refusal("manifest quota differs from release quota")
    if not isinstance(manifest["payload_bytes"], int) or isinstance(manifest["payload_bytes"], bool):
        raise Refusal("manifest payload size is invalid")
    catalog = immutable_file(bundle / "catalog.json", MAX_CATALOG_BYTES, "catalog")
    catalog_digest = safe_sha(manifest["catalog_sha256"], "catalog digest")
    if sha256(catalog) != catalog_digest:
        raise Refusal("catalog digest differs from manifest")
    digest_file = immutable_file(bundle / "catalog.sha256", 65, "catalog digest file")
    if digest_file != (catalog_digest + "\n").encode():
        raise Refusal("catalog digest file differs from manifest")
    index_path = bundle / "payload/index.json"
    index_bytes = immutable_file(index_path, MAX_INDEX_BYTES, "cache index")
    if sha256(index_bytes) != safe_sha(manifest["cache_index_sha256"], "cache index digest"):
        raise Refusal("cache index digest differs from manifest")
    index = exact_object(index_bytes, {"schema", "entries"}, "cache index")
    if index["schema"] != 2 or not isinstance(index["entries"], list):
        raise Refusal("cache index schema is unsupported")
    if not 1 <= len(index["entries"]) <= MAX_TILES:
        raise Refusal("cache index entry count is outside its bound")

    manifest_tiles: dict[tuple[str, int, int, int], tuple[str, int, str]] = {}
    if not isinstance(manifest["regions"], list) or not manifest["regions"]:
        raise Refusal("manifest regions are missing")
    for region in manifest["regions"]:
        if not isinstance(region, dict) or set(region) != {
            "region_id", "revision", "bounds", "min_zoom", "max_zoom", "expires_at_ms", "tiles"
        }:
            raise Refusal("manifest region fields are not exact")
        region_id = region["region_id"]
        if not isinstance(region_id, str) or not isinstance(region["tiles"], list) or not region["tiles"]:
            raise Refusal("manifest region identity/tiles are invalid")
        for tile in region["tiles"]:
            if not isinstance(tile, dict) or set(tile) != {"z", "x", "y", "sha256", "size_bytes", "path"}:
                raise Refusal("manifest tile fields are not exact")
            if any(
                not isinstance(value, int) or isinstance(value, bool) or value < 0
                for value in (tile["z"], tile["x"], tile["y"])
            ):
                raise Refusal("manifest tile coordinate is invalid")
            if (
                not isinstance(tile["size_bytes"], int)
                or isinstance(tile["size_bytes"], bool)
                or not 1 <= tile["size_bytes"] <= MAX_TILE_BYTES
            ):
                raise Refusal("manifest tile size is outside its bound")
            identity = (region_id, tile["z"], tile["x"], tile["y"])
            if identity in manifest_tiles:
                raise Refusal("manifest tile identity is duplicated")
            relative = safe_relative(tile["path"], "manifest tile path")
            manifest_tiles[identity] = (
                safe_sha(tile["sha256"], "manifest tile digest"),
                tile["size_bytes"],
                relative.as_posix(),
            )

    payloads: list[tuple[PurePosixPath, bytes]] = []
    identities: set[tuple[str, int, int, int]] = set()
    paths: set[str] = set()
    total = 0
    for entry in index["entries"]:
        if not isinstance(entry, dict) or set(entry) != {
            "tile", "catalog_sha256", "sha256", "byte_len", "verified_at_ms", "last_access_ms"
        }:
            raise Refusal("cache index entry fields are not exact")
        tile = entry["tile"]
        if not isinstance(tile, dict) or set(tile) != {"region", "z", "x", "y"}:
            raise Refusal("cache tile identity fields are not exact")
        region, z, x, y = tile["region"], tile["z"], tile["x"], tile["y"]
        if not isinstance(region, str) or not region:
            raise Refusal("cache tile region is invalid")
        if any(not isinstance(value, int) or isinstance(value, bool) or value < 0 for value in (z, x, y)):
            raise Refusal("cache tile coordinate is invalid")
        identity = (region, z, x, y)
        if identity in identities:
            raise Refusal("cache tile identity is duplicated")
        identities.add(identity)
        if entry["catalog_sha256"] != catalog_digest:
            raise Refusal("cache entry is bound to another catalog")
        tile_digest = safe_sha(entry["sha256"], "tile digest")
        size = entry["byte_len"]
        if not isinstance(size, int) or isinstance(size, bool) or not 1 <= size <= MAX_TILE_BYTES:
            raise Refusal("cache tile size is outside its bound")
        if entry["verified_at_ms"] != expected_epoch * 1000 or entry["last_access_ms"] != expected_epoch * 1000:
            raise Refusal("cache entry epoch differs from release epoch")
        relative = safe_relative(f"{region}/{z}/{x}/{y}-{tile_digest}.tile", "cache tile path")
        if relative.as_posix() in paths:
            raise Refusal("cache tile path is duplicated")
        paths.add(relative.as_posix())
        source_relative = PurePosixPath("payload") / relative
        if manifest_tiles.get(identity) != (
            tile_digest,
            size,
            source_relative.as_posix(),
        ):
            raise Refusal("cache entry differs from release manifest tile")
        source = beneath_immutable(bundle, source_relative, "tile")
        data = immutable_file(source, MAX_TILE_BYTES, "tile")
        if len(data) != size or sha256(data) != tile_digest:
            raise Refusal("tile bytes differ from cache index")
        total += size
        if total > expected_quota:
            raise Refusal("cache payload exceeds release quota")
        payloads.append((relative, data))
    if identities != set(manifest_tiles):
        raise Refusal("cache index and release manifest tile sets differ")
    if total != manifest["payload_bytes"]:
        raise Refusal("cache payload usage differs from manifest")
    return catalog, index_bytes, payloads


def durable_write(path: Path, data: bytes) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        view = memoryview(data)
        while view:
            written = os.write(descriptor, view)
            view = view[written:]
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def fsync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def materialize(args: argparse.Namespace) -> None:
    bundle = args.bundle.resolve(strict=True)
    parent = args.cache_root.parent.resolve(strict=True)
    output = parent / args.cache_root.name
    real_directory(parent, "cache parent")
    if output.exists() or output.is_symlink():
        raise Refusal("cache root already exists; publication is no-replace")
    catalog, index, payloads = validate_bundle(
        bundle, args.source_revision, args.source_epoch, args.quota_bytes
    )
    run_verifier(args.verifier, bundle)

    stage = Path(tempfile.mkdtemp(prefix=f".{output.name}.", dir=parent))
    os.chmod(stage, 0o700)
    try:
        durable_write(stage / "catalog.json", catalog)
        durable_write(stage / "catalog.sha256", (sha256(catalog) + "\n").encode())
        durable_write(stage / "index.json", index)
        for relative, data in payloads:
            destination = stage.joinpath(*relative.parts)
            destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            durable_write(destination, data)
        for directory, _, _ in os.walk(stage, topdown=False):
            fsync_directory(Path(directory))
        os.rename(stage, output)
        fsync_directory(parent)
    except BaseException:
        shutil.rmtree(stage, ignore_errors=True)
        raise


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--cache-root", type=Path, required=True)
    parser.add_argument("--verifier", type=Path, required=True)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument("--source-epoch", type=int, required=True)
    parser.add_argument("--quota-bytes", type=int, required=True)
    args = parser.parse_args()
    if len(args.source_revision) != 40 or any(c not in "0123456789abcdef" for c in args.source_revision):
        print("materialize-offline-catalog: refusal: source revision is not a full lower-case Git revision", file=sys.stderr)
        return 2
    if args.source_epoch <= 0 or args.quota_bytes <= 0:
        print("materialize-offline-catalog: refusal: epoch and quota must be positive", file=sys.stderr)
        return 2
    try:
        materialize(args)
    except (OSError, Refusal, subprocess.SubprocessError) as error:
        print(f"materialize-offline-catalog: refusal: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
