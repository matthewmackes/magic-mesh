#!/usr/bin/env python3
"""Produce an immutable, provenance-bound Maps offline tile catalog.

This tool only admits operator-supplied bytes.  It never fetches map data.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import sys
import tempfile
from pathlib import Path, PurePosixPath

MAX_INPUT_BYTES = 8 * 1024 * 1024
MAX_CATALOG_BYTES = 256 * 1024
MAX_TILE_BYTES = 4 * 1024 * 1024
MAX_TILES = 100_000
MAX_REGIONS = 256
MAX_ZOOM = 22
SAFE_ID = re.compile(r"^[a-z0-9](?:[a-z0-9_-]{0,62}[a-z0-9])?$")
SAFE_REVISION = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9_.+-]{0,94}[A-Za-z0-9])?$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
GIT_REVISION = re.compile(r"^[0-9a-f]{40}$")


class Refusal(ValueError):
    pass


def canonical(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def bounded_json(path: Path) -> dict:
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
        raise Refusal("approval must be a singly-linked regular file")
    if metadata.st_mode & 0o222:
        raise Refusal("approval input is mutable")
    if metadata.st_size == 0 or metadata.st_size > MAX_INPUT_BYTES:
        raise Refusal("approval size is outside its bound")
    with path.open("rb") as source:
        data = source.read(MAX_INPUT_BYTES + 1)
        opened = os.fstat(source.fileno())
    after = path.lstat()
    if (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns) != (
        after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns
    ):
        raise Refusal("approval changed while being read")
    try:
        value = json.loads(data)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise Refusal(f"approval is not valid JSON: {error}") from error
    if not isinstance(value, dict):
        raise Refusal("approval root must be an object")
    return value


def exact_keys(value: dict, expected: set[str], label: str) -> None:
    if set(value) != expected:
        raise Refusal(f"{label} fields are not exact")


def read_tile(path: Path, expected: str) -> bytes:
    before = path.lstat()
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        raise Refusal(f"tile is not a singly-linked regular file: {path}")
    if before.st_mode & 0o222:
        raise Refusal(f"tile input is mutable: {path}")
    if before.st_size == 0 or before.st_size > MAX_TILE_BYTES:
        raise Refusal(f"tile size is outside its bound: {path}")
    with path.open("rb") as source:
        data = source.read(MAX_TILE_BYTES + 1)
        opened = os.fstat(source.fileno())
    after = path.lstat()
    identity = lambda item: (item.st_dev, item.st_ino, item.st_size, item.st_mtime_ns)
    if identity(before) != identity(opened) or identity(opened) != identity(after):
        raise Refusal(f"tile changed while being read: {path}")
    if digest(data) != expected:
        raise Refusal(f"tile SHA-256 does not match approval: {path}")
    return data


def validate_relative(value: str) -> PurePosixPath:
    path = PurePosixPath(value)
    if not value or path.is_absolute() or "\\" in value or any(part in ("", ".", "..") for part in path.parts):
        raise Refusal(f"unsafe tile source path: {value!r}")
    return path


def resolve_beneath_real_directories(root: Path, relative: PurePosixPath) -> Path:
    current = root
    for component in relative.parts[:-1]:
        current = current / component
        metadata = current.lstat()
        if not stat.S_ISDIR(metadata.st_mode):
            raise Refusal(f"tile source parent is not a real directory: {current}")
    return current / relative.parts[-1]


def produce(approval_path: Path, source_root: Path, output: Path) -> None:
    approval = bounded_json(approval_path)
    exact_keys(
        approval,
        {"schema", "provider", "attribution", "license", "source_revision", "source_epoch", "quota_bytes", "regions"},
        "approval",
    )
    if approval["schema"] != 1 or approval["provider"] != "openstreetmap-derived":
        raise Refusal("approval schema/provider is unsupported")
    for name in ("attribution", "license"):
        if not isinstance(approval[name], str) or not approval[name].strip() or len(approval[name].encode()) > 4096:
            raise Refusal(f"provider {name} is missing or oversized")
    if not isinstance(approval["source_revision"], str) or not GIT_REVISION.fullmatch(approval["source_revision"]):
        raise Refusal("source revision must be a full lower-case Git revision")
    epoch = approval["source_epoch"]
    quota = approval["quota_bytes"]
    if not isinstance(epoch, int) or isinstance(epoch, bool) or epoch <= 0:
        raise Refusal("source epoch must be a positive integer")
    if not isinstance(quota, int) or isinstance(quota, bool) or quota <= 0:
        raise Refusal("quota must be a positive integer")
    regions = approval["regions"]
    if not isinstance(regions, list) or not 1 <= len(regions) <= MAX_REGIONS:
        raise Refusal("region count is outside its bound")
    source_meta = source_root.lstat()
    if not stat.S_ISDIR(source_meta.st_mode):
        raise Refusal("source root must be a real directory")
    source_root = source_root.resolve(strict=True)

    runtime_regions = []
    manifest_regions = []
    payloads: list[tuple[str, bytes]] = []
    index_entries = []
    region_ids: set[str] = set()
    tile_ids: set[tuple[str, int, int, int]] = set()
    source_paths: set[str] = set()
    total = 0

    for region in regions:
        if not isinstance(region, dict):
            raise Refusal("region must be an object")
        exact_keys(region, {"region_id", "revision", "bounds", "min_zoom", "max_zoom", "expires_at_ms", "tiles"}, "region")
        region_id, revision = region["region_id"], region["revision"]
        if not isinstance(region_id, str) or not SAFE_ID.fullmatch(region_id) or region_id in region_ids:
            raise Refusal("region id is unsafe or duplicated")
        region_ids.add(region_id)
        if not isinstance(revision, str) or not SAFE_REVISION.fullmatch(revision):
            raise Refusal("region revision is unsafe")
        min_zoom, max_zoom, expiry = region["min_zoom"], region["max_zoom"], region["expires_at_ms"]
        if any(isinstance(v, bool) or not isinstance(v, int) for v in (min_zoom, max_zoom, expiry)):
            raise Refusal("zoom and expiry values must be integers")
        if min_zoom < 0 or min_zoom > max_zoom or max_zoom > MAX_ZOOM or expiry <= epoch * 1000:
            raise Refusal("region zoom/expiry policy is invalid")
        bounds = region["bounds"]
        if not isinstance(bounds, dict):
            raise Refusal("region bounds must be an object")
        exact_keys(bounds, {"west", "south", "east", "north"}, "bounds")
        if any(isinstance(bounds[k], bool) or not isinstance(bounds[k], (int, float)) for k in bounds):
            raise Refusal("region bounds must be numeric")
        west, south, east, north = (float(bounds[k]) for k in ("west", "south", "east", "north"))
        if not (-180 <= west < east <= 180 and -90 <= south < north <= 90):
            raise Refusal("region bounds are invalid")
        tiles = region["tiles"]
        if not isinstance(tiles, list) or not tiles:
            raise Refusal("region must contain tiles")
        tile_records = []
        for tile in tiles:
            if not isinstance(tile, dict):
                raise Refusal("tile must be an object")
            exact_keys(tile, {"z", "x", "y", "source", "sha256"}, "tile")
            z, x, y = tile["z"], tile["x"], tile["y"]
            if any(isinstance(v, bool) or not isinstance(v, int) for v in (z, x, y)):
                raise Refusal("tile coordinates must be integers")
            if z < min_zoom or z > max_zoom or x < 0 or y < 0 or x >= 1 << z or y >= 1 << z:
                raise Refusal("tile coordinate is outside region zoom policy")
            identity = (region_id, z, x, y)
            if identity in tile_ids:
                raise Refusal("tile identity is duplicated")
            tile_ids.add(identity)
            relative = validate_relative(tile["source"])
            relative_text = relative.as_posix()
            if relative_text in source_paths:
                raise Refusal("tile source path overlaps another tile")
            source_paths.add(relative_text)
            expected = tile["sha256"]
            if not isinstance(expected, str) or not SHA256.fullmatch(expected):
                raise Refusal("tile digest is not lower-case SHA-256")
            candidate = resolve_beneath_real_directories(source_root, relative)
            try:
                candidate.relative_to(source_root)
            except ValueError as error:
                raise Refusal("tile source escapes source root") from error
            data = read_tile(candidate, expected)
            total += len(data)
            if total > quota:
                raise Refusal("approved tile bytes exceed quota")
            if len(tile_ids) > MAX_TILES:
                raise Refusal("tile count exceeds cache bound")
            output_path = f"payload/{region_id}/{z}/{x}/{y}-{expected}.tile"
            payloads.append((output_path, data))
            tile_records.append({"z": z, "x": x, "y": y, "sha256": expected, "size_bytes": len(data), "path": output_path})
        runtime_regions.append({"region_id": region_id, "revision": revision, "min_zoom": min_zoom, "max_zoom": max_zoom, "expires_at_ms": expiry})
        manifest_regions.append({"region_id": region_id, "revision": revision, "bounds": {"west": west, "south": south, "east": east, "north": north}, "min_zoom": min_zoom, "max_zoom": max_zoom, "expires_at_ms": expiry, "tiles": sorted(tile_records, key=lambda row: (row["z"], row["x"], row["y"]))})

    runtime = {"schema": 1, "provider": approval["provider"], "regions": sorted(runtime_regions, key=lambda row: row["region_id"])}
    runtime_bytes = canonical(runtime)
    if len(runtime_bytes) > MAX_CATALOG_BYTES:
        raise Refusal("runtime catalog exceeds Maps bound")
    catalog_sha = digest(runtime_bytes)
    for region in manifest_regions:
        for tile in region["tiles"]:
            index_entries.append({"tile": {"region": region["region_id"], "z": tile["z"], "x": tile["x"], "y": tile["y"]}, "catalog_sha256": catalog_sha, "sha256": tile["sha256"], "byte_len": tile["size_bytes"], "verified_at_ms": epoch * 1000, "last_access_ms": epoch * 1000})
    cache_index = {"schema": 2, "entries": sorted(index_entries, key=lambda row: (row["tile"]["region"], row["tile"]["z"], row["tile"]["x"], row["tile"]["y"]))}
    manifest = {"schema": 1, "kind": "mcnf-offline-map-catalog", "provider": approval["provider"], "attribution": approval["attribution"], "license": approval["license"], "source_revision": approval["source_revision"], "source_epoch": epoch, "quota_bytes": quota, "payload_bytes": total, "catalog_sha256": catalog_sha, "cache_index_sha256": digest(canonical(cache_index)), "regions": sorted(manifest_regions, key=lambda row: row["region_id"])}

    output_parent = output.parent.resolve(strict=True)
    if output.exists() or output.is_symlink():
        raise Refusal("output already exists; publication is no-replace")
    stage = Path(tempfile.mkdtemp(prefix=f".{output.name}.", dir=output_parent))
    try:
        for relative, data in payloads:
            destination = stage / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
            destination.chmod(0o444)
        for name, data in (("catalog.json", runtime_bytes), ("catalog.sha256", (catalog_sha + "\n").encode()), ("payload/index.json", canonical(cache_index)), ("manifest.json", canonical(manifest))):
            target = stage / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            target.chmod(0o444)
        for directory, _, _ in os.walk(stage, topdown=False):
            os.chmod(directory, 0o555)
        os.rename(stage, output)
    except BaseException:
        shutil.rmtree(stage, ignore_errors=True)
        raise


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--approval", type=Path, required=True)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        produce(args.approval, args.source_root, args.output)
    except (OSError, Refusal) as error:
        print(f"produce-offline-catalog: refusal: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
