#!/usr/bin/env python3
"""Clip the locked New York PBF to official Erie+Niagara county bounds.

Operator lock (2026-08-22): Geofabrik NY PBF + official TIGER Erie (36029) /
Niagara (36063) GeoJSON already on the dest-root. This helper never fetches,
never talks to a public OSM tile CDN, and never marks production_admitted.

The osmium argv seam is injectable so tests never need the real binary.
Default invocation is a fixed argv list only (never a shell string):
`osmium extract --strategy=smart --bbox=W,S,E,N --overwrite -o DEST SRC`.

Bbox is read from the GeoJSON (or its sidecar). Official county bounds are
used even when they escape the MBTiles verifier envelope; the helper does
not shrink geometry to cheat admission. Output is a clipped PBF under an
operator dest-root (no-replace, mode 0400). Sidecar kind is
mcnf-maps-pbf-clip, not a production MBTiles receipt.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import shutil
import stat
import subprocess
import sys
from collections.abc import Callable
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
verify = _load("maps_verify_mbtiles", HERE / "maps-verify-mbtiles.py")

EXIT_REFUSED = fetch.EXIT_REFUSED
EXTRACT_KIND = "mcnf-maps-pbf-clip"
PRODUCTION_RECEIPT_KIND = "mcnf-maps-mbtiles-receipt"
LOCKED_PBF_UPSTREAM = "geofabrik"
LOCKED_GEOMETRY_UPSTREAM = "census-tiger"
LOCKED_GEOIDS = ("36029", "36063")
LOCKED_NAMES = ("Erie County", "Niagara County")
LOCKED_STATEFP = "36"
REGION_ID = "buffalo-niagara"
OPERATOR_AUTHORIZATION = fetch.OPERATOR_AUTHORIZATION
APPROVED_PROVIDER = "openstreetmap-derived"
APPROVED_LICENSE = "ODbL-1.0"
PBF_CLIP_NAME = "erie-niagara.osm.pbf"
MAX_SOURCES_BYTES = fetch.MAX_SOURCES_BYTES
MAX_SIDECAR_BYTES = fetch.MAX_SIDECAR_BYTES
MAX_SOURCE_FILE_BYTES = 4 * 1024 * 1024 * 1024
MAX_GEOJSON_BYTES = 16 * 1024 * 1024
FORBIDDEN_SOURCE_KEYS = ("tiles", "tile_url", "raster_url", "xyz", "tilejson", "tile_cdn")
OSMIUM_EXTRACT = "extract"
OSMIUM_STRATEGY = "--strategy=smart"
SIDECAR_KEYS = {
    "schema_version",
    "kind",
    "region_id",
    "license",
    "provider",
    "operator_authorization",
    "pbf_url",
    "pbf_sha256",
    "pbf_bytes",
    "geometry_url",
    "geometry_sha256",
    "geometry_bytes",
    "clip_geoids",
    "clip_names",
    "destination",
    "pbf_clip_sha256",
    "pbf_clip_bytes",
    "bbox",
    "bounds_envelope_compatible",
    "production_admitted",
}

Refusal = fetch.Refusal
OsmiumFn = Callable[[list[str]], None]


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
    pbf = sources.get("pbf")
    geometry = sources.get("geometry")
    if not isinstance(pbf, dict) or not isinstance(geometry, dict):
        raise Refusal("authorized sources pbf and geometry entries are malformed")
    if pbf.get("upstream") != LOCKED_PBF_UPSTREAM:
        raise Refusal("source id is not a locked Maps source")
    if geometry.get("upstream") != LOCKED_GEOMETRY_UPSTREAM:
        raise Refusal("source id is not a locked Maps source")
    admit_clip_policy(geometry)
    for source_id in fetch.LOCKED_SOURCE_IDS:
        url, _entry = fetch.locked_url(sources, source_id)
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
    locked = {fetch.locked_url(sources, source_id)[0] for source_id in fetch.LOCKED_SOURCE_IDS}
    if url not in locked:
        raise Refusal("wrong URL refused: requested URL is not the locked authorized source")


def refuse_cdn_prefix(path: Path, label: str) -> None:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        prefix = os.read(descriptor, 4096)
    finally:
        os.close(descriptor)
    lowered = prefix.lower()
    if any(marker.encode("ascii") in lowered for marker in fetch.TILE_CDN_MARKERS):
        raise Refusal("public OSM tile CDN refused")


def admit_regular_file(path: Path, label: str, maximum: int) -> os.stat_result:
    try:
        before = path.lstat()
    except OSError as error:
        raise Refusal(f"{label} is missing or inaccessible") from error
    if stat.S_ISLNK(before.st_mode):
        raise Refusal(f"path substitution refused: {label} is a symlink")
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        raise Refusal(f"{label} must be a singly-linked regular file")
    if before.st_size <= 0 or before.st_size > maximum:
        raise Refusal(f"{label} size is outside its bound")
    return before


def hash_local_source(path: Path, label: str, maximum: int) -> tuple[str, int]:
    before = admit_regular_file(path, label, maximum)
    refuse_cdn_prefix(path, label)
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    hasher = hashlib.sha256()
    size = 0
    try:
        remaining = before.st_size
        while remaining:
            chunk = os.read(descriptor, min(1024 * 1024, remaining))
            if not chunk:
                raise Refusal(f"{label} truncated while reading")
            hasher.update(chunk)
            size += len(chunk)
            remaining -= len(chunk)
    finally:
        os.close(descriptor)
    if size <= 0:
        raise Refusal(f"{label} is empty")
    return hasher.hexdigest(), size


def read_local_source(path: Path, label: str, maximum: int) -> bytes:
    admit_regular_file(path, label, maximum)
    refuse_cdn_prefix(path, label)
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        data = b""
        remaining = path.lstat().st_size
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
    return data


def resolve_source_file(source_root: Path, relative: str, label: str) -> Path:
    fetch.real_directory(source_root, "source-root")
    rel = fetch.relative_leaf(relative, label)
    path = fetch.resolve_beneath(source_root, rel, label)
    if not path.exists():
        raise Refusal(f"{label} is missing or inaccessible")
    return path


def resolve_destination(dest_root: Path, relative: str) -> tuple[PurePosixPath, Path]:
    fetch.real_directory(dest_root, "dest-root")
    rel = fetch.relative_leaf(relative, "destination")
    if rel.name != PBF_CLIP_NAME:
        raise Refusal("path substitution refused: PBF clip filename is not erie-niagara.osm.pbf")
    return rel, fetch.resolve_beneath(dest_root, rel, "destination")


def _need_number(value: object, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise Refusal(f"{label} is not numeric")
    number = float(value)
    if number != number or number in (float("inf"), float("-inf")):
        raise Refusal(f"{label} is not a finite number")
    return number


def admit_bbox(value: object, label: str) -> list[float]:
    if not isinstance(value, list) or len(value) != 4:
        raise Refusal(f"{label} bbox is malformed")
    west = _need_number(value[0], f"{label} west")
    south = _need_number(value[1], f"{label} south")
    east = _need_number(value[2], f"{label} east")
    north = _need_number(value[3], f"{label} north")
    if not (west < east and south < north):
        raise Refusal(f"{label} bbox is invalid")
    return [west, south, east, north]


def format_bbox_arg(bbox: list[float]) -> str:
    return ",".join(f"{value:.6f}" for value in bbox)


def bounds_envelope_compatible(bbox: list[float]) -> bool:
    west, south, east, north = bbox
    envelope = verify.BOUNDS_ENVELOPE
    return bool(
        envelope["west"] <= west < east <= envelope["east"]
        and envelope["south"] <= south < north <= envelope["north"]
    )


def _ring_points(ring: object) -> list[tuple[float, float]]:
    if not isinstance(ring, list) or len(ring) < 4:
        raise Refusal("GeoJSON ring is malformed")
    points: list[tuple[float, float]] = []
    for point in ring:
        if not isinstance(point, list) or len(point) < 2:
            raise Refusal("GeoJSON coordinate is malformed")
        points.append((_need_number(point[0], "GeoJSON lon"), _need_number(point[1], "GeoJSON lat")))
    return points


def geometry_bbox(geometry: object) -> list[float]:
    if not isinstance(geometry, dict):
        raise Refusal("GeoJSON geometry is malformed")
    kind = geometry.get("type")
    coordinates = geometry.get("coordinates")
    rings: list[list[tuple[float, float]]] = []
    if kind == "Polygon":
        if not isinstance(coordinates, list):
            raise Refusal("GeoJSON polygon is malformed")
        rings.extend(_ring_points(ring) for ring in coordinates)
    elif kind == "MultiPolygon":
        if not isinstance(coordinates, list):
            raise Refusal("GeoJSON multipolygon is malformed")
        for polygon in coordinates:
            if not isinstance(polygon, list):
                raise Refusal("GeoJSON multipolygon is malformed")
            rings.extend(_ring_points(ring) for ring in polygon)
    else:
        raise Refusal("GeoJSON geometry is not a polygon")
    xs = [point[0] for ring in rings for point in ring]
    ys = [point[1] for ring in rings for point in ring]
    if not xs or not ys:
        raise Refusal("GeoJSON geometry has no coordinates")
    return [min(xs), min(ys), max(xs), max(ys)]


def admit_geojson_features(document: dict[str, Any]) -> tuple[list[str], list[float]]:
    if document.get("type") != "FeatureCollection":
        raise Refusal("GeoJSON root is not a FeatureCollection")
    features = document.get("features")
    if not isinstance(features, list) or len(features) != 2:
        raise Refusal("clip refused: extra invented county in output")
    geoids: list[str] = []
    names: list[str] = []
    feature_boxes: list[list[float]] = []
    for feature in features:
        if not isinstance(feature, dict):
            raise Refusal("clip refused: extra invented county in output")
        props = feature.get("properties")
        if not isinstance(props, dict):
            raise Refusal("clip refused: extra invented county in output")
        geoid = props.get("GEOID")
        name = props.get("NAME")
        if geoid not in LOCKED_GEOIDS:
            raise Refusal("clip refused: extra invented county in output")
        if name not in LOCKED_NAMES:
            raise Refusal("clip must be Erie County / Niagara County")
        geoids.append(str(geoid))
        names.append(str(name))
        if "bbox" in feature:
            feature_boxes.append(admit_bbox(feature.get("bbox"), f"feature {geoid}"))
        else:
            feature_boxes.append(geometry_bbox(feature.get("geometry")))
    if geoids != list(LOCKED_GEOIDS) or names != list(LOCKED_NAMES):
        raise Refusal("clip must be Erie 36029 / Niagara 36063")
    computed = [
        min(box[0] for box in feature_boxes),
        min(box[1] for box in feature_boxes),
        max(box[2] for box in feature_boxes),
        max(box[3] for box in feature_boxes),
    ]
    return geoids, admit_bbox(computed, "computed GeoJSON")


def bbox_from_geojson(data: bytes) -> tuple[list[str], list[float]]:
    try:
        document = json.loads(data)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise Refusal(f"GeoJSON is not valid JSON: {error}") from error
    if not isinstance(document, dict):
        raise Refusal("GeoJSON root must be an object")
    geoids, computed = admit_geojson_features(document)
    if "bbox" in document:
        return geoids, admit_bbox(document.get("bbox"), "GeoJSON")
    return geoids, computed


def bbox_from_geometry_sidecar(path: Path) -> list[float] | None:
    record = fetch.immutable_json(path, MAX_SIDECAR_BYTES, "geometry sidecar")
    if "bbox" not in record:
        return None
    return admit_bbox(record.get("bbox"), "geometry sidecar")


def osmium_extract_argv(osmium: str, bbox: str, dest: Path, src: Path) -> list[str]:
    if not osmium or not isinstance(osmium, str):
        raise Refusal("osmium is missing")
    if any(item is None for item in (bbox, dest, src)):
        raise Refusal("osmium argv is malformed")
    return [
        osmium,
        OSMIUM_EXTRACT,
        OSMIUM_STRATEGY,
        f"--bbox={bbox}",
        "--overwrite",
        "-o",
        str(dest),
        str(src),
    ]


def resolve_osmium(osmium: str) -> str:
    if not osmium:
        raise Refusal("osmium is missing")
    if os.sep in osmium or osmium.startswith("."):
        path = Path(osmium)
        if not path.is_file() or not os.access(path, os.X_OK):
            raise Refusal("osmium is missing")
        return str(path)
    found = shutil.which(osmium)
    if found is None:
        raise Refusal("osmium is missing")
    return found


def default_run_osmium(argv: list[str]) -> None:
    if not isinstance(argv, list) or any(not isinstance(item, str) or not item for item in argv):
        raise Refusal("osmium argv is malformed")
    if len(argv) != 8 or argv[1:3] != [OSMIUM_EXTRACT, OSMIUM_STRATEGY] or argv[4:6] != ["--overwrite", "-o"]:
        raise Refusal("osmium argv is malformed")
    try:
        completed = subprocess.run(argv, check=False, capture_output=True)
    except OSError as error:
        raise Refusal("osmium is missing") from error
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout or b"").decode("utf-8", "replace")[:240]
        raise Refusal(f"osmium extract refused: {detail or 'non-zero exit'}")


def finalize_destination(path: Path) -> tuple[str, int]:
    try:
        before = path.lstat()
    except OSError as error:
        raise Refusal("osmium extract produced no destination") from error
    if stat.S_ISLNK(before.st_mode):
        raise Refusal("path substitution refused: destination is a symlink")
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        raise Refusal("destination must be a singly-linked regular file")
    if before.st_size <= 0 or before.st_size > MAX_SOURCE_FILE_BYTES:
        raise Refusal("destination size is outside its bound")
    os.chmod(path, 0o400)
    digest_hex, size = hash_local_source(path, "destination", MAX_SOURCE_FILE_BYTES)
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    parent_fd = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(parent_fd)
    finally:
        os.close(parent_fd)
    return digest_hex, size


def bind_sidecar(
    *,
    sources: dict[str, object],
    pbf_sha256: str,
    pbf_size: int,
    geometry_sha256: str,
    geometry_size: int,
    destination: str,
    clip_sha256: str,
    clip_size: int,
    bbox: list[float],
) -> dict[str, object]:
    pbf_url, _pbf = fetch.locked_url(sources, "pbf")
    geometry_url, _geometry = fetch.locked_url(sources, "geometry")
    refuse_tile_cdn(pbf_url)
    refuse_tile_cdn(geometry_url)
    sidecar = {
        "schema_version": 1,
        "kind": EXTRACT_KIND,
        "region_id": REGION_ID,
        "license": APPROVED_LICENSE,
        "provider": APPROVED_PROVIDER,
        "operator_authorization": OPERATOR_AUTHORIZATION,
        "pbf_url": pbf_url,
        "pbf_sha256": pbf_sha256,
        "pbf_bytes": pbf_size,
        "geometry_url": geometry_url,
        "geometry_sha256": geometry_sha256,
        "geometry_bytes": geometry_size,
        "clip_geoids": list(LOCKED_GEOIDS),
        "clip_names": list(LOCKED_NAMES),
        "destination": destination,
        "pbf_clip_sha256": clip_sha256,
        "pbf_clip_bytes": clip_size,
        "bbox": bbox,
        # Official county bbox may escape the MBTiles verifier envelope.
        # Record that honestly; do not shrink the clip to cheat admission.
        "bounds_envelope_compatible": bounds_envelope_compatible(bbox),
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


def extract_pbf_clip(
    *,
    sources_path: Path,
    source_root: Path,
    pbf: str,
    geometry: str,
    dest_root: Path,
    destination: str,
    sidecar: str,
    url: str | None = None,
    geometry_sidecar: str | None = None,
    osmium: str = "osmium",
    run_osmium: OsmiumFn | None = None,
) -> dict[str, object]:
    sources = admit_authorized_sources(sources_path)
    admit_url(sources, url)
    pbf_path = resolve_source_file(source_root, pbf, "pbf")
    geometry_path = resolve_source_file(source_root, geometry, "geometry")
    dest_rel, dest_path = resolve_destination(dest_root, destination)
    sidecar_rel = fetch.relative_leaf(sidecar, "sidecar")
    sidecar_path = fetch.resolve_beneath(dest_root, sidecar_rel, "sidecar")
    if dest_path == sidecar_path or dest_path == pbf_path or dest_path == geometry_path:
        raise Refusal("path substitution refused: destination collides with an input path")
    if dest_path.exists() or dest_path.is_symlink():
        raise Refusal("destination already exists; publication is no-replace")
    if sidecar_path.exists() or sidecar_path.is_symlink():
        raise Refusal("sidecar already exists; publication is no-replace")
    pbf_sha256, pbf_size = hash_local_source(pbf_path, "pbf", MAX_SOURCE_FILE_BYTES)
    geometry_bytes = read_local_source(geometry_path, "geometry", MAX_GEOJSON_BYTES)
    geoids, bbox = bbox_from_geojson(geometry_bytes)
    if geoids != list(LOCKED_GEOIDS):
        raise Refusal("clip must be Erie 36029 / Niagara 36063")
    if geometry_sidecar is not None:
        geometry_sidecar_path = resolve_source_file(source_root, geometry_sidecar, "geometry sidecar")
        sidecar_bbox = bbox_from_geometry_sidecar(geometry_sidecar_path)
        if sidecar_bbox is not None:
            bbox = sidecar_bbox
    binary = osmium if run_osmium is not None else resolve_osmium(osmium)
    argv = osmium_extract_argv(binary, format_bbox_arg(bbox), dest_path, pbf_path)
    runner = run_osmium if run_osmium is not None else default_run_osmium
    try:
        runner(argv)
        clip_sha256, clip_size = finalize_destination(dest_path)
    except Exception:
        if dest_path.exists() and dest_path != pbf_path:
            try:
                dest_path.unlink()
            except OSError:
                pass
        raise
    record = bind_sidecar(
        sources=sources,
        pbf_sha256=pbf_sha256,
        pbf_size=pbf_size,
        geometry_sha256=digest(geometry_bytes),
        geometry_size=len(geometry_bytes),
        destination=str(dest_rel),
        clip_sha256=clip_sha256,
        clip_size=clip_size,
        bbox=bbox,
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
    parser.add_argument("--pbf", required=True, help="relative local NY PBF path")
    parser.add_argument("--geometry", required=True, help="relative local Erie/Niagara GeoJSON")
    parser.add_argument("--geometry-sidecar", default=None, help="optional GeoJSON sidecar with bbox")
    parser.add_argument("--dest-root", type=Path, required=True)
    parser.add_argument("--destination", required=True, help="must be erie-niagara.osm.pbf")
    parser.add_argument("--sidecar", required=True)
    parser.add_argument("--url", default=None, help="if set, must match a locked authorized source URL")
    parser.add_argument("--osmium", default="osmium", help="osmium binary; refused when missing")
    args = parser.parse_args()
    try:
        value = extract_pbf_clip(
            sources_path=args.sources,
            source_root=args.source_root,
            pbf=args.pbf,
            geometry=args.geometry,
            dest_root=args.dest_root,
            destination=args.destination,
            sidecar=args.sidecar,
            url=args.url,
            geometry_sidecar=args.geometry_sidecar,
            osmium=args.osmium,
        )
    except (Refusal, OSError, UnicodeError, ValueError) as error:
        print(f"maps-extract-pbf-clip: refusal: {error}", file=sys.stderr)
        return EXIT_REFUSED
    print(canonical(value).decode("ascii"), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
