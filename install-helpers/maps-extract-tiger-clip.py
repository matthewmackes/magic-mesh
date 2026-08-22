#!/usr/bin/env python3
"""Extract official Erie+Niagara county polygons from a local TIGER zip.

Operator lock (2026-08-22): Census TIGER 2024 county zip, GEOIDs 36029 and
36063 only. This helper never fetches, never installs GDAL/Mapnik/osmium,
and never marks production_admitted. Output is a bounded GeoJSON
FeatureCollection under an operator dest-root (no-replace, mode 0400).

The sidecar kind is mcnf-maps-tiger-clip, not a production MBTiles receipt.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import io
import json
import os
import stat
import struct
import sys
import zipfile
from pathlib import Path, PurePosixPath
from typing import Any

HERE = Path(__file__).resolve().parent


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


fetch = _load("maps_fetch_authorized_sources", HERE / "maps-fetch-authorized-sources.py")

EXIT_REFUSED = fetch.EXIT_REFUSED
EXTRACT_KIND = "mcnf-maps-tiger-clip"
PRODUCTION_RECEIPT_KIND = "mcnf-maps-mbtiles-receipt"
LOCKED_GEOIDS = ("36029", "36063")
LOCKED_NAMES = ("Erie County", "Niagara County")
LOCKED_STATEFP = "36"
LOCKED_GEOMETRY_UPSTREAM = "census-tiger"
REGION_ID = "buffalo-niagara"
OPERATOR_AUTHORIZATION = fetch.OPERATOR_AUTHORIZATION
APPROVED_PROVIDER = "openstreetmap-derived"
APPROVED_LICENSE = "ODbL-1.0"
GEOJSON_NAME = "erie-niagara.geojson"
MAX_SOURCES_BYTES = fetch.MAX_SOURCES_BYTES
MAX_SIDECAR_BYTES = fetch.MAX_SIDECAR_BYTES
MAX_SOURCE_FILE_BYTES = 4 * 1024 * 1024 * 1024
MAX_MEMBER_BYTES = 512 * 1024 * 1024
MAX_GEOJSON_BYTES = 16 * 1024 * 1024
SHAPE_NULL = 0
SHAPE_POLYGON = 5
SHAPE_POLYGON_Z = 15
SHAPE_POLYGON_M = 25
SHP_FILE_CODE = 9994
SHP_VERSION = 1000
FORBIDDEN_SOURCE_KEYS = ("tiles", "tile_url", "raster_url", "xyz", "tilejson", "tile_cdn")
SIDECAR_KEYS = {
    "schema_version",
    "kind",
    "region_id",
    "license",
    "provider",
    "operator_authorization",
    "geometry_url",
    "geometry_sha256",
    "geometry_bytes",
    "clip_geoids",
    "clip_names",
    "destination",
    "geojson_sha256",
    "geojson_bytes",
    "feature_count",
    "bbox",
    "production_admitted",
}

Refusal = fetch.Refusal


def canonical(value: object) -> bytes:
    return fetch.canonical(value)


def digest(data: bytes) -> str:
    return fetch.digest(data)


def exact_keys(value: object, expected: set[str], label: str) -> dict:
    return fetch.exact_keys(value, expected, label)


def refuse_tile_cdn(url: str) -> None:
    fetch.refuse_tile_cdn(url)


def admit_authorized_sources(sources_path: Path) -> dict[str, object]:
    sources = fetch.load_authorized_sources(sources_path)
    for banned in FORBIDDEN_SOURCE_KEYS:
        if banned in sources:
            raise Refusal("source id is not a locked Maps source")
    if sources.get("provider") != APPROVED_PROVIDER:
        raise Refusal(f"wrong provider refused: authorized sources must be {APPROVED_PROVIDER}")
    geometry = sources.get("geometry")
    if not isinstance(geometry, dict):
        raise Refusal("authorized sources geometry entry is malformed")
    if geometry.get("upstream") != LOCKED_GEOMETRY_UPSTREAM:
        raise Refusal("source id is not a locked Maps source")
    admit_clip_policy(geometry)
    url, _entry = fetch.locked_url(sources, "geometry")
    refuse_tile_cdn(url)
    fetch.refuse_never_fetch(url, sources.get("never_fetch"))
    return sources


def admit_clip_policy(geometry: dict[str, object]) -> None:
    geoids = geometry.get("select_geoid")
    names = geometry.get("select_name")
    if geoids != list(LOCKED_GEOIDS):
        raise Refusal("clip must be Erie 36029 / Niagara 36063")
    if names != list(LOCKED_NAMES):
        raise Refusal("clip must be Erie County / Niagara County")
    if geometry.get("statefp") != LOCKED_STATEFP:
        raise Refusal("clip must stay inside New York statefp 36")


def admit_url(sources: dict[str, object], requested: str | None) -> None:
    if requested is None:
        return
    url = fetch.canonical_https_url(requested, "requested URL")
    refuse_tile_cdn(url)
    fetch.refuse_never_fetch(url, sources.get("never_fetch"))
    locked, _entry = fetch.locked_url(sources, "geometry")
    if url != locked:
        raise Refusal("wrong URL refused: requested URL is not the locked authorized source")


def read_local_source(path: Path, label: str) -> bytes:
    try:
        before = path.lstat()
    except OSError as error:
        raise Refusal(f"{label} is missing or inaccessible") from error
    if stat.S_ISLNK(before.st_mode):
        raise Refusal(f"path substitution refused: {label} is a symlink")
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        raise Refusal(f"{label} must be a singly-linked regular file")
    if before.st_size <= 0 or before.st_size > MAX_SOURCE_FILE_BYTES:
        raise Refusal(f"{label} size is outside its bound")
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        data = b""
        remaining = before.st_size
        while remaining:
            chunk = os.read(descriptor, min(1024 * 1024, remaining))
            if not chunk:
                raise Refusal(f"{label} truncated while reading")
            data += chunk
            remaining -= len(chunk)
    finally:
        os.close(descriptor)
    if not data:
        raise Refusal(f"{label} is empty")
    lowered = data[:4096].lower()
    if any(marker.encode("ascii") in lowered for marker in fetch.TILE_CDN_MARKERS):
        raise Refusal("public OSM tile CDN refused")
    return data


def resolve_source_file(source_root: Path, relative: str, label: str) -> Path:
    rel = fetch.relative_leaf(relative, label)
    path = fetch.resolve_beneath(source_root, rel, label)
    if not path.exists():
        raise Refusal(f"{label} is missing or inaccessible")
    return path


def resolve_destination(dest_root: Path, relative: str) -> tuple[PurePosixPath, Path]:
    rel = fetch.relative_leaf(relative, "destination")
    if rel.name != GEOJSON_NAME:
        raise Refusal("path substitution refused: GeoJSON filename is not erie-niagara.geojson")
    return rel, fetch.resolve_beneath(dest_root, rel, "destination")


def admit_zip_member(name: str) -> PurePosixPath:
    path = PurePosixPath(name)
    if (
        not name
        or path.is_absolute()
        or "\\" in name
        or any(part in ("", ".", "..") for part in path.parts)
    ):
        raise Refusal(f"path substitution refused: zip member is unsafe: {name}")
    return path


def _member_suffix(name: str) -> str:
    return PurePosixPath(name).suffix.lower()


def _read_zip_member(archive: zipfile.ZipFile, name: str) -> bytes:
    admit_zip_member(name)
    info = archive.getinfo(name)
    if info.is_dir():
        raise Refusal(f"shapefile parse refused: zip member {name} is a directory")
    if info.file_size <= 0 or info.file_size > MAX_MEMBER_BYTES:
        raise Refusal(f"shapefile parse refused: zip member {name} size is outside its bound")
    try:
        data = archive.read(name)
    except (RuntimeError, zipfile.BadZipFile, OSError) as error:
        raise Refusal(f"shapefile parse refused: zip member {name} is unreadable") from error
    if len(data) != info.file_size:
        raise Refusal(f"shapefile parse refused: zip member {name} truncated")
    return data


def _pair_shapefile_members(names: list[str]) -> dict[str, str]:
    by_stem: dict[str, dict[str, str]] = {}
    for name in names:
        path = admit_zip_member(name)
        suffix = path.suffix.lower()
        if suffix not in {".shp", ".dbf", ".shx"}:
            continue
        stem = str(path.with_suffix("")).lower()
        by_stem.setdefault(stem, {})[suffix] = name
    complete = {
        stem: members
        for stem, members in by_stem.items()
        if ".shp" in members and ".dbf" in members
    }
    if not complete:
        raise Refusal("shapefile parse refused: zip is missing .dbf/.shp members")
    if len(complete) == 1:
        return next(iter(complete.values()))
    county = {
        stem: members
        for stem, members in complete.items()
        if "county" in PurePosixPath(stem).name
    }
    if len(county) == 1:
        return next(iter(county.values()))
    raise Refusal("shapefile parse refused: zip has more than one shapefile")


def _need(data: bytes, offset: int, size: int, label: str) -> None:
    if offset < 0 or size < 0 or offset + size > len(data):
        raise Refusal(f"shapefile parse refused: {label} is truncated")


def parse_dbf_records(data: bytes) -> list[dict[str, str]]:
    _need(data, 0, 32, ".dbf header")
    nrecords, header_len, rec_len = struct.unpack_from("<IHH", data, 4)
    if header_len < 33 or rec_len < 1 or nrecords < 0:
        raise Refusal("shapefile parse refused: .dbf header is malformed")
    _need(data, 0, header_len, ".dbf header body")
    if data[header_len - 1] not in (0x0D, 0x00):
        raise Refusal("shapefile parse refused: .dbf header terminator is missing")
    fields: list[tuple[str, int]] = []
    cursor = 32
    while cursor + 32 <= header_len - 1:
        if data[cursor] == 0x0D:
            break
        raw_name = data[cursor : cursor + 11].split(b"\x00", 1)[0]
        try:
            name = raw_name.decode("ascii").strip().upper()
        except UnicodeDecodeError as error:
            raise Refusal("shapefile parse refused: .dbf field name is not ASCII") from error
        length = data[cursor + 16]
        if not name or length <= 0:
            raise Refusal("shapefile parse refused: .dbf field descriptor is malformed")
        fields.append((name, length))
        cursor += 32
    if not fields:
        raise Refusal("shapefile parse refused: .dbf has no fields")
    expected = 1 + sum(length for _name, length in fields)
    if expected != rec_len:
        raise Refusal("shapefile parse refused: .dbf record length does not match fields")
    records: list[dict[str, str]] = []
    body = header_len
    for index in range(nrecords):
        _need(data, body, rec_len, f".dbf record {index}")
        row = data[body : body + rec_len]
        body += rec_len
        if row[0:1] == b"*":
            records.append({})
            continue
        if row[0:1] not in (b" ", b"\x00"):
            raise Refusal(f"shapefile parse refused: .dbf record {index} flag is invalid")
        values: dict[str, str] = {}
        offset = 1
        for name, length in fields:
            raw = row[offset : offset + length]
            offset += length
            values[name] = raw.decode("latin-1").strip()
        records.append(values)
    return records


def record_geoid(row: dict[str, str]) -> str | None:
    if not row:
        return None
    geoid = row.get("GEOID")
    if geoid:
        return geoid
    statefp = row.get("STATEFP", "")
    countyfp = row.get("COUNTYFP", "")
    if statefp and countyfp:
        return f"{statefp}{countyfp}"
    return None


def record_name(row: dict[str, str], geoid: str) -> str:
    for key in ("NAMELSAD", "NAME"):
        value = row.get(key, "")
        if value:
            if value.endswith(" County"):
                return value
            return f"{value} County"
    locked = dict(zip(LOCKED_GEOIDS, LOCKED_NAMES))
    return locked[geoid]


def select_locked_rows(rows: list[dict[str, str]]) -> list[tuple[int, str, str]]:
    selected: dict[str, tuple[int, str, str]] = {}
    extras: set[str] = set()
    for index, row in enumerate(rows):
        geoid = record_geoid(row)
        if geoid is None:
            continue
        if geoid in LOCKED_GEOIDS:
            if geoid in selected:
                raise Refusal(f"shapefile parse refused: GEOID {geoid} is duplicated")
            selected[geoid] = (index, geoid, record_name(row, geoid))
            continue
        if geoid.startswith(LOCKED_STATEFP) and len(geoid) == 5 and geoid.isdigit():
            extras.add(geoid)
    missing = [geoid for geoid in LOCKED_GEOIDS if geoid not in selected]
    if missing:
        raise Refusal(
            "clip must be Erie 36029 / Niagara 36063; missing "
            + ",".join(missing)
        )
    ordered = [selected[geoid] for geoid in LOCKED_GEOIDS]
    names = [item[2] for item in ordered]
    if names != list(LOCKED_NAMES):
        raise Refusal("clip must be Erie County / Niagara County")
    return ordered


def _ring_area(ring: list[list[float]]) -> float:
    area = 0.0
    for index in range(len(ring) - 1):
        x1, y1 = ring[index]
        x2, y2 = ring[index + 1]
        area += x1 * y2 - x2 * y1
    return area / 2.0


def _close_ring(points: list[list[float]]) -> list[list[float]]:
    if len(points) < 3:
        raise Refusal("shapefile parse refused: polygon ring is too short")
    if points[0] != points[-1]:
        points = points + [points[0]]
    if len(points) < 4:
        raise Refusal("shapefile parse refused: polygon ring is not closed")
    return points


def _rings_to_geometry(rings: list[list[list[float]]]) -> dict[str, Any]:
    polygons: list[list[list[list[float]]]] = []
    current: list[list[list[float]]] | None = None
    for ring in rings:
        closed = _close_ring(ring)
        area = _ring_area(closed)
        # Shapefile outer rings are clockwise (negative area in lon/lat).
        is_outer = area <= 0.0
        geojson_ring = list(reversed(closed))
        if is_outer or current is None:
            current = [geojson_ring]
            polygons.append(current)
        else:
            current.append(geojson_ring)
    if not polygons:
        raise Refusal("shapefile parse refused: polygon has no rings")
    if len(polygons) == 1:
        return {"type": "Polygon", "coordinates": polygons[0]}
    return {"type": "MultiPolygon", "coordinates": polygons}


def _bbox_of_rings(rings: list[list[list[float]]]) -> list[float]:
    xs = [point[0] for ring in rings for point in ring]
    ys = [point[1] for ring in rings for point in ring]
    if not xs or not ys:
        raise Refusal("shapefile parse refused: polygon has no coordinates")
    return [min(xs), min(ys), max(xs), max(ys)]


def _parse_polygon_content(content: bytes, label: str) -> tuple[dict[str, Any], list[float]]:
    _need(content, 0, 44, label)
    shape_type, = struct.unpack_from("<i", content, 0)
    if shape_type == SHAPE_NULL:
        raise Refusal(f"shapefile parse refused: {label} is a null shape")
    if shape_type not in {SHAPE_POLYGON, SHAPE_POLYGON_Z, SHAPE_POLYGON_M}:
        raise Refusal(f"shapefile parse refused: {label} is not a polygon")
    num_parts, num_points = struct.unpack_from("<ii", content, 36)
    if num_parts <= 0 or num_points < 4:
        raise Refusal(f"shapefile parse refused: {label} part/point counts are invalid")
    parts_off = 44
    points_off = parts_off + num_parts * 4
    _need(content, parts_off, num_parts * 4 + num_points * 16, label)
    starts = list(struct.unpack_from(f"<{num_parts}i", content, parts_off))
    if starts[0] != 0:
        raise Refusal(f"shapefile parse refused: {label} first part index is not 0")
    points = [
        list(struct.unpack_from("<2d", content, points_off + index * 16))
        for index in range(num_points)
    ]
    rings: list[list[list[float]]] = []
    for part_index, start in enumerate(starts):
        end = starts[part_index + 1] if part_index + 1 < len(starts) else num_points
        if start < 0 or end > num_points or end - start < 3:
            raise Refusal(f"shapefile parse refused: {label} ring {part_index} is invalid")
        rings.append(points[start:end])
    geometry = _rings_to_geometry(rings)
    return geometry, _bbox_of_rings(rings)


def parse_shp_header(data: bytes) -> int:
    _need(data, 0, 100, ".shp header")
    file_code, file_length = struct.unpack_from(">i", data, 0)[0], struct.unpack_from(">i", data, 24)[0]
    version, shape_type = struct.unpack_from("<ii", data, 28)
    if file_code != SHP_FILE_CODE or version != SHP_VERSION:
        raise Refusal("shapefile parse refused: .shp header is not a shapefile")
    if shape_type not in {SHAPE_NULL, SHAPE_POLYGON, SHAPE_POLYGON_Z, SHAPE_POLYGON_M}:
        raise Refusal("shapefile parse refused: .shp is not a polygon shapefile")
    if file_length * 2 != len(data):
        raise Refusal("shapefile parse refused: .shp length does not match member bytes")
    return shape_type


def parse_shx_offsets(data: bytes, nrecords: int) -> list[int] | None:
    if not data:
        return None
    _need(data, 0, 100, ".shx header")
    file_code, file_length = struct.unpack_from(">i", data, 0)[0], struct.unpack_from(">i", data, 24)[0]
    version = struct.unpack_from("<i", data, 28)[0]
    if file_code != SHP_FILE_CODE or version != SHP_VERSION:
        raise Refusal("shapefile parse refused: .shx header is not a shapefile index")
    if file_length * 2 != len(data):
        raise Refusal("shapefile parse refused: .shx length does not match member bytes")
    count = (len(data) - 100) // 8
    if count != nrecords:
        raise Refusal("shapefile parse refused: .shx record count does not match .dbf")
    offsets = []
    for index in range(count):
        offset_words, = struct.unpack_from(">i", data, 100 + index * 8)
        offsets.append(offset_words * 2)
    return offsets


def read_shp_record(shp: bytes, offset: int, expected_number: int, label: str) -> bytes:
    _need(shp, offset, 8, f"{label} header")
    rec_num, content_words = struct.unpack_from(">ii", shp, offset)
    if rec_num != expected_number:
        raise Refusal(f"shapefile parse refused: {label} record number drifted")
    content_len = content_words * 2
    _need(shp, offset + 8, content_len, label)
    return shp[offset + 8 : offset + 8 + content_len]


def scan_shp_offsets(shp: bytes, nrecords: int) -> list[int]:
    parse_shp_header(shp)
    offsets: list[int] = []
    cursor = 100
    for index in range(nrecords):
        _need(shp, cursor, 8, f".shp record {index + 1} header")
        _rec_num, content_words = struct.unpack_from(">ii", shp, cursor)
        offsets.append(cursor)
        cursor += 8 + content_words * 2
    if cursor != len(shp):
        raise Refusal("shapefile parse refused: .shp has trailing unread bytes")
    return offsets


def extract_locked_features(shp: bytes, dbf: bytes, shx: bytes | None) -> list[dict[str, Any]]:
    parse_shp_header(shp)
    rows = parse_dbf_records(dbf)
    selected = select_locked_rows(rows)
    offsets = parse_shx_offsets(shx, len(rows)) if shx is not None else None
    if offsets is None:
        offsets = scan_shp_offsets(shp, len(rows))
    features: list[dict[str, Any]] = []
    seen: list[str] = []
    for index, geoid, name in selected:
        content = read_shp_record(shp, offsets[index], index + 1, f"GEOID {geoid}")
        geometry, bbox = _parse_polygon_content(content, f"GEOID {geoid}")
        if geoid not in LOCKED_GEOIDS:
            raise Refusal("clip refused: extra invented county in output")
        seen.append(geoid)
        features.append(
            {
                "type": "Feature",
                "bbox": bbox,
                "properties": {"GEOID": geoid, "NAME": name},
                "geometry": geometry,
            }
        )
    if seen != list(LOCKED_GEOIDS):
        raise Refusal("clip refused: extra invented county in output")
    if len(features) != 2:
        raise Refusal("clip refused: extra invented county in output")
    return features


def extract_clip_collection(geometry_bytes: bytes) -> dict[str, Any]:
    try:
        archive = zipfile.ZipFile(io.BytesIO(geometry_bytes))
    except (zipfile.BadZipFile, zipfile.LargeZipFile) as error:
        raise Refusal("shapefile parse refused: geometry is not a zip") from error
    with archive:
        names = [info.filename for info in archive.infolist() if not info.is_dir()]
        members = _pair_shapefile_members(names)
        dbf = _read_zip_member(archive, members[".dbf"])
        shp = _read_zip_member(archive, members[".shp"])
        shx = _read_zip_member(archive, members[".shx"]) if ".shx" in members else None
    features = extract_locked_features(shp, dbf, shx)
    geoids = [feature["properties"]["GEOID"] for feature in features]
    if geoids != list(LOCKED_GEOIDS):
        raise Refusal("clip refused: extra invented county in output")
    xs = [feature["bbox"][0] for feature in features] + [feature["bbox"][2] for feature in features]
    ys = [feature["bbox"][1] for feature in features] + [feature["bbox"][3] for feature in features]
    collection = {
        "type": "FeatureCollection",
        "bbox": [min(xs), min(ys), max(xs), max(ys)],
        "features": features,
    }
    return collection


def encode_geojson(collection: dict[str, Any]) -> bytes:
    body = (json.dumps(collection, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii")
    if len(body) <= 0 or len(body) > MAX_GEOJSON_BYTES:
        raise Refusal("GeoJSON size is outside its bound")
    parsed = json.loads(body)
    if parsed.get("type") != "FeatureCollection":
        raise Refusal("GeoJSON root is not a FeatureCollection")
    features = parsed.get("features")
    if not isinstance(features, list) or len(features) != 2:
        raise Refusal("clip refused: extra invented county in output")
    geoids = []
    for feature in features:
        if not isinstance(feature, dict):
            raise Refusal("clip refused: extra invented county in output")
        props = feature.get("properties")
        if not isinstance(props, dict):
            raise Refusal("clip refused: extra invented county in output")
        geoid = props.get("GEOID")
        if geoid not in LOCKED_GEOIDS:
            raise Refusal("clip refused: extra invented county in output")
        geoids.append(geoid)
    if geoids != list(LOCKED_GEOIDS):
        raise Refusal("clip refused: extra invented county in output")
    return body


def bind_sidecar(
    *,
    sources: dict[str, object],
    geometry_sha256: str,
    geometry_size: int,
    destination: str,
    geojson_sha256: str,
    geojson_size: int,
    bbox: list[float],
) -> dict[str, object]:
    geometry_url, _geometry = fetch.locked_url(sources, "geometry")
    refuse_tile_cdn(geometry_url)
    sidecar = {
        "schema_version": 1,
        "kind": EXTRACT_KIND,
        "region_id": REGION_ID,
        "license": APPROVED_LICENSE,
        "provider": APPROVED_PROVIDER,
        "operator_authorization": OPERATOR_AUTHORIZATION,
        "geometry_url": geometry_url,
        "geometry_sha256": geometry_sha256,
        "geometry_bytes": geometry_size,
        "clip_geoids": list(LOCKED_GEOIDS),
        "clip_names": list(LOCKED_NAMES),
        "destination": destination,
        "geojson_sha256": geojson_sha256,
        "geojson_bytes": geojson_size,
        "feature_count": 2,
        "bbox": bbox,
        # Official county polygons are a clip artifact, not production MBTiles.
        "production_admitted": False,
    }
    if sidecar["kind"] == PRODUCTION_RECEIPT_KIND:
        raise Refusal("extract sidecar must not be a production Maps receipt")
    if sidecar["kind"] != EXTRACT_KIND:
        raise Refusal("extract sidecar kind is unsupported")
    if sidecar["production_admitted"] is not False:
        raise Refusal("extract sidecar must never mark production_admitted")
    exact_keys(sidecar, SIDECAR_KEYS, "extract sidecar")
    return sidecar


def extract_tiger_clip(
    *,
    sources_path: Path,
    source_root: Path,
    geometry: str,
    dest_root: Path,
    destination: str,
    sidecar: str,
    url: str | None = None,
) -> dict[str, object]:
    sources = admit_authorized_sources(sources_path)
    admit_url(sources, url)
    geometry_path = resolve_source_file(source_root, geometry, "geometry")
    dest_rel, dest_path = resolve_destination(dest_root, destination)
    sidecar_rel = fetch.relative_leaf(sidecar, "sidecar")
    sidecar_path = fetch.resolve_beneath(dest_root, sidecar_rel, "sidecar")
    if dest_path == sidecar_path:
        raise Refusal("path substitution refused: destination and sidecar are the same path")
    if dest_path.exists() or dest_path.is_symlink():
        raise Refusal("destination already exists; publication is no-replace")
    if sidecar_path.exists() or sidecar_path.is_symlink():
        raise Refusal("sidecar already exists; publication is no-replace")
    geometry_bytes = read_local_source(geometry_path, "geometry")
    collection = extract_clip_collection(geometry_bytes)
    body = encode_geojson(collection)
    fetch.atomic_write_bytes(dest_path, body, label="destination")
    record = bind_sidecar(
        sources=sources,
        geometry_sha256=digest(geometry_bytes),
        geometry_size=len(geometry_bytes),
        destination=str(dest_rel),
        geojson_sha256=digest(body),
        geojson_size=len(body),
        bbox=list(collection["bbox"]),
    )
    sidecar_body = canonical(record)
    if len(sidecar_body) > MAX_SIDECAR_BYTES:
        raise Refusal("extract sidecar exceeds its bound")
    fetch.atomic_write_bytes(sidecar_path, sidecar_body, label="sidecar")
    return record


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--sources",
        type=Path,
        default=HERE / "maps-authorized-sources.json",
    )
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--geometry", required=True, help="relative local TIGER zip path")
    parser.add_argument("--dest-root", type=Path, required=True)
    parser.add_argument("--destination", required=True, help="must be erie-niagara.geojson")
    parser.add_argument("--sidecar", required=True)
    parser.add_argument("--url", default=None, help="if set, must match the locked geometry URL")
    args = parser.parse_args()
    try:
        value = extract_tiger_clip(
            sources_path=args.sources,
            source_root=args.source_root,
            geometry=args.geometry,
            dest_root=args.dest_root,
            destination=args.destination,
            sidecar=args.sidecar,
            url=args.url,
        )
    except (Refusal, OSError, UnicodeError, ValueError, struct.error, zipfile.BadZipFile) as error:
        print(f"maps-extract-tiger-clip: refusal: {error}", file=sys.stderr)
        return EXIT_REFUSED
    print(canonical(value).decode("ascii"), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
