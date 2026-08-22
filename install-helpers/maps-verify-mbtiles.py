#!/usr/bin/env python3
"""Admit operator-supplied Maps MBTiles against the S2 receipt contract.

This verifier never fetches tiles. Fixture or operator bytes may bind a
non-production receipt; they cannot satisfy the production Maps gate.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sqlite3
import stat
import sys
from pathlib import Path, PurePosixPath

EXIT_REFUSED = 2
APPROVED_PROVIDER = "openstreetmap-derived"
APPROVED_LICENSE = "ODbL-1.0"
REGION_ID = "buffalo-niagara"
CANONICAL_INSTALL_PATH = "/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles"
MBTILES_NAME = "buffalo-niagara.mbtiles"
KIND = "mcnf-maps-mbtiles-receipt"
GIT_REVISION = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
PNG_MAGIC = b"\x89PNG\r\n\x1a\n"
MAX_APPROVAL_BYTES = 64 * 1024
MAX_RECEIPT_BYTES = 64 * 1024
MAX_METADATA_VALUE = 4096
MAX_ZOOM = 22
# Erie + Niagara county envelope with a small official-geometry margin.
BOUNDS_ENVELOPE = {"west": -79.30, "south": 42.35, "east": -78.35, "north": 43.45}
FORBIDDEN_SOURCE_MARKERS = (
    "tile.openstreetmap.org",
    "tiles.openstreetmap.org",
    "tile.osm.org",
    "mapbox.com",
    "googleapis.com",
    "google.com/maps",
    "hereapi.com",
    "arcgisonline.com",
)
APPROVAL_KEYS = {
    "schema",
    "provider",
    "attribution",
    "license",
    "source_revision",
    "source_epoch",
    "quota_bytes",
    "region_id",
    "install_path",
}
RECEIPT_KEYS = APPROVAL_KEYS | {
    "kind",
    "payload_bytes",
    "tile_bytes",
    "tile_count",
    "mbtiles_sha256",
    "bounds",
    "min_zoom",
    "max_zoom",
    "production_admitted",
}


class Refusal(ValueError):
    pass


def canonical(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii")


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def exact_keys(value: object, expected: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != expected:
        raise Refusal(f"{label} fields are not exact")
    return value


def identity(item: os.stat_result) -> tuple[int, int, int, int]:
    return (item.st_dev, item.st_ino, item.st_size, item.st_mtime_ns)


def immutable_file(path: Path, maximum: int, label: str) -> bytes:
    before = path.lstat()
    if stat.S_ISLNK(before.st_mode):
        raise Refusal(f"path substitution refused: {label} is a symlink")
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        raise Refusal(f"{label} must be a singly-linked regular file")
    if before.st_mode & 0o222:
        raise Refusal(f"{label} is mutable")
    if before.st_size <= 0 or before.st_size > maximum:
        raise Refusal(f"{label} size is outside its bound")
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        opened = os.fstat(descriptor)
        if identity(before)[:2] != (opened.st_dev, opened.st_ino) or not stat.S_ISREG(opened.st_mode):
            raise Refusal(f"{label} changed while opening")
        data = b""
        while len(data) <= maximum:
            chunk = os.read(descriptor, min(65536, maximum + 1 - len(data)))
            if not chunk:
                break
            data += chunk
        after = os.fstat(descriptor)
        if identity(opened) != identity(after):
            raise Refusal(f"{label} changed while reading")
    finally:
        os.close(descriptor)
    if path.lstat().st_mtime_ns != before.st_mtime_ns:
        raise Refusal(f"{label} pathname changed while reading")
    if not data or len(data) > maximum:
        raise Refusal(f"{label} size is outside its bound")
    return data


def real_directory(path: Path, label: str) -> None:
    metadata = path.lstat()
    if stat.S_ISLNK(metadata.st_mode):
        raise Refusal(f"path substitution refused: {label} is a symlink")
    if not stat.S_ISDIR(metadata.st_mode):
        raise Refusal(f"{label} must be a real directory")


def relative_source(value: str, label: str) -> PurePosixPath:
    path = PurePosixPath(value)
    if (
        not value
        or path.is_absolute()
        or "\\" in value
        or any(part in ("", ".", "..") for part in path.parts)
    ):
        raise Refusal(f"path substitution refused: {label} is not a safe relative path")
    return path


def resolve_beneath(root: Path, relative: PurePosixPath, label: str) -> Path:
    real_directory(root, f"{label} source root")
    current = root
    for component in relative.parts[:-1]:
        current /= component
        real_directory(current, f"{label} parent")
    candidate = current / relative.parts[-1]
    try:
        candidate.relative_to(root)
    except ValueError as error:
        raise Refusal(f"path substitution refused: {label} escapes source root") from error
    return candidate


def bounded_json(path: Path, maximum: int, label: str) -> dict:
    data = immutable_file(path, maximum, label)
    try:
        value = json.loads(data)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise Refusal(f"{label} is not valid JSON: {error}") from error
    if not isinstance(value, dict):
        raise Refusal(f"{label} root must be an object")
    return value


def require_provider(value: object, label: str) -> str:
    if not isinstance(value, str) or value != APPROVED_PROVIDER:
        raise Refusal(f"wrong provider refused: {label} must be {APPROVED_PROVIDER}")
    return value


def require_text(value: object, label: str) -> str:
    if not isinstance(value, str) or not value.strip() or len(value.encode()) > MAX_METADATA_VALUE:
        raise Refusal(f"{label} is missing or oversized")
    lowered = value.lower()
    if any(marker in lowered for marker in FORBIDDEN_SOURCE_MARKERS):
        raise Refusal(f"wrong provider refused: {label} names a public tile service")
    return value


def require_revision(value: object) -> str:
    if not isinstance(value, str) or not GIT_REVISION.fullmatch(value) or set(value) == {"0"}:
        raise Refusal("source revision must be a non-null full lower-case Git revision")
    return value


def require_positive_int(value: object, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise Refusal(f"{label} must be a positive integer")
    return value


def require_install_path(value: object) -> str:
    if not isinstance(value, str) or value != CANONICAL_INSTALL_PATH:
        raise Refusal("path substitution refused: install path is not the production Maps path")
    return value


def require_region(value: object) -> str:
    if not isinstance(value, str) or value != REGION_ID:
        raise Refusal("path substitution refused: region is not buffalo-niagara")
    return value


def parse_bounds(value: str) -> dict[str, float]:
    parts = [part.strip() for part in value.split(",")]
    if len(parts) != 4:
        raise Refusal("MBTiles bounds metadata is malformed")
    try:
        west, south, east, north = (float(part) for part in parts)
    except ValueError as error:
        raise Refusal("MBTiles bounds metadata is not numeric") from error
    if not (west < east and south < north):
        raise Refusal("MBTiles bounds metadata is invalid")
    envelope = BOUNDS_ENVELOPE
    if not (
        envelope["west"] <= west < east <= envelope["east"]
        and envelope["south"] <= south < north <= envelope["north"]
    ):
        raise Refusal("MBTiles bounds escape the Buffalo-Niagara envelope")
    return {"west": west, "south": south, "east": east, "north": north}


def inspect_mbtiles(path: Path, quota_bytes: int) -> dict[str, object]:
    before = path.lstat()
    if stat.S_ISLNK(before.st_mode):
        raise Refusal("path substitution refused: MBTiles is a symlink")
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        raise Refusal("MBTiles must be a singly-linked regular file")
    if before.st_mode & 0o222:
        raise Refusal("MBTiles input is mutable")
    if before.st_size <= 0:
        raise Refusal("MBTiles is empty")
    if before.st_size > quota_bytes:
        raise Refusal("quota breach refused: MBTiles file exceeds approved quota")
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        opened = os.fstat(descriptor)
        if identity(before)[:2] != (opened.st_dev, opened.st_ino):
            raise Refusal("MBTiles changed while opening")
        hasher = hashlib.sha256()
        remaining = opened.st_size
        while remaining:
            chunk = os.read(descriptor, min(1024 * 1024, remaining))
            if not chunk:
                raise Refusal("MBTiles truncated while reading")
            hasher.update(chunk)
            remaining -= len(chunk)
        after = os.fstat(descriptor)
        if identity(opened) != identity(after):
            raise Refusal("MBTiles changed while reading")
        file_digest = hasher.hexdigest()
        file_size = opened.st_size
    finally:
        os.close(descriptor)

    try:
        connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    except sqlite3.Error as error:
        raise Refusal(f"MBTiles is not readable SQLite: {error}") from error
    try:
        tables = {
            row[0]
            for row in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")
        }
        if not {"metadata", "tiles"}.issubset(tables):
            raise Refusal("MBTiles lacks metadata and tiles tables")
        metadata: dict[str, str] = {}
        for name, value in connection.execute("SELECT name, value FROM metadata"):
            if not isinstance(name, str) or not isinstance(value, str):
                raise Refusal("MBTiles metadata row is not text")
            if name in metadata:
                raise Refusal(f"MBTiles metadata field is duplicated: {name}")
            require_text(value, f"MBTiles metadata {name}")
            metadata[name] = value
        if metadata.get("format") != "png":
            raise Refusal("MBTiles format must be png")
        require_provider(metadata.get("provider"), "MBTiles metadata provider")
        require_text(metadata.get("attribution"), "MBTiles attribution")
        license_value = metadata.get("license", APPROVED_LICENSE)
        if license_value != APPROVED_LICENSE:
            raise Refusal("MBTiles license must be ODbL-1.0")
        if metadata.get("name") not in {None, REGION_ID}:
            raise Refusal("path substitution refused: MBTiles name is not buffalo-niagara")
        if "bounds" not in metadata:
            raise Refusal("MBTiles bounds metadata is missing")
        bounds = parse_bounds(metadata["bounds"])
        try:
            min_zoom = int(metadata.get("minzoom", "0"))
            max_zoom = int(metadata.get("maxzoom", "0"))
        except ValueError as error:
            raise Refusal("MBTiles zoom metadata is not an integer") from error
        if min_zoom < 0 or min_zoom > max_zoom or max_zoom > MAX_ZOOM:
            raise Refusal("MBTiles zoom policy is invalid")

        tile_bytes = 0
        tile_count = 0
        identities: set[tuple[int, int, int]] = set()
        for zoom, column, row, data in connection.execute(
            "SELECT zoom_level, tile_column, tile_row, tile_data FROM tiles"
        ):
            if any(isinstance(value, bool) or not isinstance(value, int) or value < 0 for value in (zoom, column, row)):
                raise Refusal("MBTiles tile coordinates must be integers")
            if zoom < min_zoom or zoom > max_zoom:
                raise Refusal("MBTiles tile zoom is outside metadata policy")
            side = 1 << zoom
            if column >= side or row >= side:
                raise Refusal("MBTiles TMS coordinate is outside the zoom square")
            key = (zoom, column, row)
            if key in identities:
                raise Refusal("MBTiles tile identity is duplicated")
            identities.add(key)
            if not isinstance(data, (bytes, bytearray)) or not data.startswith(PNG_MAGIC):
                raise Refusal("MBTiles tile payload is not PNG")
            tile_bytes += len(data)
            tile_count += 1
            if tile_bytes > quota_bytes:
                raise Refusal("quota breach refused: tile payload exceeds approved quota")
        if tile_count < 1:
            raise Refusal("MBTiles contains no tiles")
    finally:
        connection.close()

    if path.lstat().st_mtime_ns != before.st_mtime_ns:
        raise Refusal("MBTiles changed after inspection")
    return {
        "payload_bytes": file_size,
        "tile_bytes": tile_bytes,
        "tile_count": tile_count,
        "mbtiles_sha256": file_digest,
        "bounds": bounds,
        "min_zoom": min_zoom,
        "max_zoom": max_zoom,
        "provider": APPROVED_PROVIDER,
        "attribution": metadata["attribution"],
        "license": APPROVED_LICENSE,
    }


def load_approval(path: Path) -> dict[str, object]:
    approval = exact_keys(bounded_json(path, MAX_APPROVAL_BYTES, "Maps approval"), APPROVAL_KEYS, "approval")
    if approval["schema"] != 1:
        raise Refusal("approval schema is unsupported")
    require_provider(approval["provider"], "approval provider")
    require_text(approval["attribution"], "approval attribution")
    if approval["license"] != APPROVED_LICENSE:
        raise Refusal("approval license must be ODbL-1.0")
    require_revision(approval["source_revision"])
    require_positive_int(approval["source_epoch"], "source epoch")
    require_positive_int(approval["quota_bytes"], "quota")
    require_region(approval["region_id"])
    require_install_path(approval["install_path"])
    return approval


def bind_receipt(approval: dict[str, object], inspected: dict[str, object]) -> dict[str, object]:
    if inspected["payload_bytes"] > approval["quota_bytes"] or inspected["tile_bytes"] > approval["quota_bytes"]:
        raise Refusal("quota breach refused: approved tile bytes exceed quota")
    return {
        "schema": 1,
        "kind": KIND,
        "provider": APPROVED_PROVIDER,
        "attribution": approval["attribution"],
        "license": APPROVED_LICENSE,
        "source_revision": approval["source_revision"],
        "source_epoch": approval["source_epoch"],
        "quota_bytes": approval["quota_bytes"],
        "region_id": REGION_ID,
        "install_path": CANONICAL_INSTALL_PATH,
        "payload_bytes": inspected["payload_bytes"],
        "tile_bytes": inspected["tile_bytes"],
        "tile_count": inspected["tile_count"],
        "mbtiles_sha256": inspected["mbtiles_sha256"],
        "bounds": inspected["bounds"],
        "min_zoom": inspected["min_zoom"],
        "max_zoom": inspected["max_zoom"],
        # Contract fixtures and operator-supplied bytes never close the
        # production Maps gate. Production admission needs the real
        # candidate-bound provider object.
        "production_admitted": False,
    }


def resolve_mbtiles(source_root: Path, relative: str) -> Path:
    path = relative_source(relative, "MBTiles")
    if path.name != MBTILES_NAME:
        raise Refusal("path substitution refused: MBTiles filename is not buffalo-niagara.mbtiles")
    return resolve_beneath(source_root, path, "MBTiles")


def verify_receipt(
    receipt_path: Path,
    mbtiles: Path,
    source_revision: str,
    source_epoch: int,
    quota_bytes: int,
) -> dict[str, object]:
    body = immutable_file(receipt_path, MAX_RECEIPT_BYTES, "Maps receipt")
    receipt = exact_keys(json.loads(body), RECEIPT_KEYS, "receipt")
    if body != canonical(receipt):
        raise Refusal("Maps receipt is non-canonical")
    if receipt["schema"] != 1 or receipt["kind"] != KIND:
        raise Refusal("Maps receipt kind or schema is unsupported")
    require_provider(receipt["provider"], "receipt provider")
    if receipt["license"] != APPROVED_LICENSE:
        raise Refusal("receipt license must be ODbL-1.0")
    if receipt["source_revision"] != require_revision(source_revision):
        raise Refusal("receipt source revision differs from the requested revision")
    if receipt["source_epoch"] != require_positive_int(source_epoch, "source epoch"):
        raise Refusal("receipt source epoch differs from the requested epoch")
    if receipt["quota_bytes"] != require_positive_int(quota_bytes, "quota"):
        raise Refusal("receipt quota differs from the requested quota")
    require_region(receipt["region_id"])
    require_install_path(receipt["install_path"])
    if receipt["production_admitted"] is not False:
        raise Refusal("fixture or unverified MBTiles cannot be claimed as production")
    inspected = inspect_mbtiles(mbtiles, quota_bytes)
    if inspected["mbtiles_sha256"] != receipt["mbtiles_sha256"]:
        raise Refusal("MBTiles bytes differ from the receipt digest")
    if inspected["payload_bytes"] != receipt["payload_bytes"] or inspected["tile_bytes"] != receipt["tile_bytes"]:
        raise Refusal("MBTiles size differs from the receipt")
    if inspected["tile_count"] != receipt["tile_count"]:
        raise Refusal("MBTiles tile count differs from the receipt")
    if inspected["bounds"] != receipt["bounds"]:
        raise Refusal("MBTiles bounds differ from the receipt")
    return receipt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--mbtiles", required=True)
    parser.add_argument("--source-revision", required=True)
    parser.add_argument("--source-epoch", type=int, required=True)
    parser.add_argument("--quota-bytes", type=int, required=True)
    args = parser.parse_args()
    try:
        mbtiles = resolve_mbtiles(args.source_root, args.mbtiles)
        value = verify_receipt(
            args.receipt, mbtiles, args.source_revision, args.source_epoch, args.quota_bytes
        )
    except (Refusal, OSError, UnicodeError, ValueError, sqlite3.Error) as error:
        print(f"maps-verify-mbtiles: refusal: {error}", file=sys.stderr)
        return EXIT_REFUSED
    print(json.dumps(value, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
