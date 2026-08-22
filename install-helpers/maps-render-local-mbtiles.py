#!/usr/bin/env python3
"""Render Buffalo-Niagara raster PNG MBTiles from locked local sources.

Operator lock (2026-08-22): Geofabrik New York PBF + official TIGER Erie
(36029) / Niagara (36063) geometry, rendered locally. This helper never
fetches public OSM tile CDNs, never downloads the PBF itself, and never
marks production_admitted.

The render and clip seams are injectable so tests stay on local fixtures
and never touch the real Geofabrik object. Default rendering writes a
contract-valid PNG MBTiles from already-local admitted bytes; it is not
a production Maps gate.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import io
import json
import os
import re
import sqlite3
import stat
import sys
import tempfile
import zipfile
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
RENDER_KIND = "mcnf-maps-local-render"
PRODUCTION_RECEIPT_KIND = "mcnf-maps-mbtiles-receipt"
LOCKED_SOURCE_KINDS = fetch.LOCKED_SOURCE_IDS
LOCKED_PBF_UPSTREAM = "geofabrik"
LOCKED_GEOMETRY_UPSTREAM = "census-tiger"
LOCKED_GEOIDS = ("36029", "36063")
LOCKED_NAMES = ("Erie County", "Niagara County")
LOCKED_STATEFP = "36"
MBTILES_NAME = verify.MBTILES_NAME
REGION_ID = verify.REGION_ID
APPROVED_PROVIDER = verify.APPROVED_PROVIDER
APPROVED_LICENSE = verify.APPROVED_LICENSE
OPERATOR_AUTHORIZATION = fetch.OPERATOR_AUTHORIZATION
MAX_SOURCES_BYTES = fetch.MAX_SOURCES_BYTES
MAX_SIDECAR_BYTES = fetch.MAX_SIDECAR_BYTES
MAX_SOURCE_FILE_BYTES = 4 * 1024 * 1024 * 1024
GEOID_TOKEN = re.compile(r"\b36\d{3}\b")
FORBIDDEN_SOURCE_KEYS = ("tiles", "tile_url", "raster_url", "xyz", "tilejson", "tile_cdn")
# Contract-valid 1×1 PNG used by the local fixture renderer.
FIXTURE_PNG = bytes.fromhex(
    "89504e470d0a1a0a0000000d4948445200000001000000010802000000907753de"
    "0000000c49444154789c63f8cfc00000000300010005fed42b0000000049454e44ae426082"
)
FIXTURE_BOUNDS = {"west": -79.12, "south": 42.48, "east": -78.50, "north": 43.30}
FIXTURE_ZOOM = 1
FIXTURE_COLUMN = 0
FIXTURE_ROW = 1
DEFAULT_ATTRIBUTION = "© OpenStreetMap contributors"
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
    "mbtiles_sha256",
    "mbtiles_bytes",
    "tile_count",
    "bounds",
    "min_zoom",
    "max_zoom",
    "format",
    "production_admitted",
}

Refusal = fetch.Refusal
ClipFn = Callable[[bytes], list[str]]
RenderFn = Callable[[dict[str, Any]], dict[str, Any]]


def canonical(value: object) -> bytes:
    return fetch.canonical(value)


def digest(data: bytes) -> str:
    return fetch.digest(data)


def exact_keys(value: object, expected: set[str], label: str) -> dict:
    return fetch.exact_keys(value, expected, label)


def refuse_tile_cdn(url: str) -> None:
    fetch.refuse_tile_cdn(url)


def bounds_string(bounds: dict[str, float]) -> str:
    return f"{bounds['west']},{bounds['south']},{bounds['east']},{bounds['north']}"


def admit_locked_source_kind(source_id: str) -> str:
    if source_id not in LOCKED_SOURCE_KINDS:
        raise Refusal("source id is not a locked Maps source")
    return source_id


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
    for source_id in LOCKED_SOURCE_KINDS:
        admit_locked_source_kind(source_id)
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
    locked = {fetch.locked_url(sources, source_id)[0] for source_id in LOCKED_SOURCE_KINDS}
    if url not in locked:
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
    if rel.name != MBTILES_NAME:
        raise Refusal("path substitution refused: MBTiles filename is not buffalo-niagara.mbtiles")
    return rel, fetch.resolve_beneath(dest_root, rel, "destination")


def _member_has_locked_geoids(data: bytes) -> bool:
    # Official TIGER .dbf rows pack STATEFP/COUNTYFP/COUNTYNS/GEOID as adjacent
    # ASCII digits (`360290097411336029050000`). Word-boundary scans miss them.
    return all(geoid.encode("ascii") in data for geoid in LOCKED_GEOIDS)


def extract_clip_geoids_from_zip(geometry_bytes: bytes) -> list[str] | None:
    """Return locked Erie/Niagara GEOIDs from a TIGER zip, or None if not a zip.

    Official Census county archives keep GEOID strings inside members
    (typically `.dbf`). If both locked GEOIDs are present, return exactly
    Erie/Niagara — never every 36xxx county in the national file.
    """
    try:
        archive = zipfile.ZipFile(io.BytesIO(geometry_bytes))
    except (zipfile.BadZipFile, zipfile.LargeZipFile):
        return None
    with archive:
        members = archive.infolist()
        dbf_first = [info for info in members if info.filename.lower().endswith(".dbf")]
        others = [info for info in members if info not in dbf_first]
        for info in dbf_first + others:
            if info.is_dir():
                continue
            try:
                data = archive.read(info.filename)
            except (RuntimeError, zipfile.BadZipFile, OSError):
                continue
            if _member_has_locked_geoids(data):
                return list(LOCKED_GEOIDS)
    raise Refusal("geometry clip is not Erie 36029 / Niagara 36063")


def extract_clip_geoids(geometry_bytes: bytes) -> list[str]:
    zip_geoids = extract_clip_geoids_from_zip(geometry_bytes)
    if zip_geoids is not None:
        return zip_geoids
    try:
        value = json.loads(geometry_bytes)
    except (UnicodeDecodeError, json.JSONDecodeError):
        value = None
    if isinstance(value, dict):
        geoids = value.get("select_geoid")
        if not isinstance(geoids, list) or not geoids:
            raise Refusal("geometry clip is not Erie 36029 / Niagara 36063")
        if any(not isinstance(item, str) or not item for item in geoids):
            raise Refusal("geometry clip geoids are malformed")
        return list(geoids)
    text = geometry_bytes.decode("utf-8", errors="replace")
    found = sorted(set(GEOID_TOKEN.findall(text)))
    if not found:
        raise Refusal("geometry clip is not Erie 36029 / Niagara 36063")
    return found


def admit_clip_geoids(geometry_bytes: bytes, clip: ClipFn | None = None) -> list[str]:
    geoids = clip(geometry_bytes) if clip is not None else extract_clip_geoids(geometry_bytes)
    if geoids != list(LOCKED_GEOIDS):
        raise Refusal("clip must be Erie 36029 / Niagara 36063")
    return geoids


def default_local_render(request: dict[str, Any]) -> dict[str, Any]:
    """Fixture rasterizer: admitted local bytes → contract PNG MBTiles.

    Never fetches. A later producer may inject a real style engine once the
    operator-fetched Geofabrik PBF is present on disk.
    """
    pbf_bytes = request["pbf_bytes"]
    geometry_bytes = request["geometry_bytes"]
    if not isinstance(pbf_bytes, (bytes, bytearray)) or not pbf_bytes:
        raise Refusal("pbf source bytes are missing")
    if not isinstance(geometry_bytes, (bytes, bytearray)) or not geometry_bytes:
        raise Refusal("geometry source bytes are missing")
    if request["clip_geoids"] != list(LOCKED_GEOIDS):
        raise Refusal("clip must be Erie 36029 / Niagara 36063")
    png = request.get("tile_png", FIXTURE_PNG)
    if not isinstance(png, (bytes, bytearray)) or not bytes(png).startswith(verify.PNG_MAGIC):
        raise Refusal("MBTiles tile payload is not PNG")
    bounds = dict(FIXTURE_BOUNDS)
    verify.parse_bounds(bounds_string(bounds))
    return {
        "tiles": ((FIXTURE_ZOOM, FIXTURE_COLUMN, FIXTURE_ROW, bytes(png)),),
        "bounds": bounds,
        "min_zoom": FIXTURE_ZOOM,
        "max_zoom": FIXTURE_ZOOM,
        "attribution": DEFAULT_ATTRIBUTION,
        "provider": APPROVED_PROVIDER,
        "license": APPROVED_LICENSE,
        "name": REGION_ID,
        "format": "png",
    }


def write_mbtiles_sqlite(path: Path, rendered: dict[str, Any]) -> None:
    tiles = rendered["tiles"]
    if not tiles:
        raise Refusal("MBTiles contains no tiles")
    bounds = rendered["bounds"]
    if not isinstance(bounds, dict):
        raise Refusal("MBTiles bounds metadata is malformed")
    verify.parse_bounds(bounds_string(bounds))
    if rendered.get("format") != "png":
        raise Refusal("MBTiles format must be png")
    verify.require_provider(rendered.get("provider"), "render provider")
    verify.require_text(rendered.get("attribution"), "render attribution")
    if rendered.get("license") != APPROVED_LICENSE:
        raise Refusal("MBTiles license must be ODbL-1.0")
    if rendered.get("name") not in {None, REGION_ID}:
        raise Refusal("path substitution refused: MBTiles name is not buffalo-niagara")
    connection = sqlite3.connect(path)
    try:
        connection.execute("CREATE TABLE metadata (name TEXT, value TEXT)")
        connection.execute(
            "CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB)"
        )
        metadata = {
            "format": "png",
            "minzoom": str(rendered["min_zoom"]),
            "maxzoom": str(rendered["max_zoom"]),
            "bounds": bounds_string(bounds),
            "center": f"{(bounds['west'] + bounds['east']) / 2},{(bounds['south'] + bounds['north']) / 2},{rendered['min_zoom']}",
            "provider": APPROVED_PROVIDER,
            "attribution": rendered["attribution"],
            "license": APPROVED_LICENSE,
            "name": REGION_ID,
        }
        for key, value in metadata.items():
            connection.execute("INSERT INTO metadata VALUES (?, ?)", (key, value))
        for zoom, column, row, data in tiles:
            if not isinstance(data, (bytes, bytearray)) or not bytes(data).startswith(verify.PNG_MAGIC):
                raise Refusal("MBTiles tile payload is not PNG")
            connection.execute(
                "INSERT INTO tiles VALUES (?, ?, ?, ?)",
                (zoom, column, row, bytes(data)),
            )
        connection.commit()
    finally:
        connection.close()


def atomic_publish_file(path: Path, body: bytes, *, label: str) -> None:
    fetch.atomic_write_bytes(path, body, label=label)


def atomic_publish_mbtiles(path: Path, rendered: dict[str, Any]) -> tuple[str, int]:
    if path.exists() or path.is_symlink():
        raise Refusal("destination already exists; publication is no-replace")
    parent = path.parent
    fetch.real_directory(parent, "destination parent")
    parent = parent.resolve(strict=True)
    fd, name = tempfile.mkstemp(prefix=f".{path.name}.", dir=parent, suffix=".mbtiles")
    temporary = Path(name)
    try:
        os.close(fd)
        fd = -1
        write_mbtiles_sqlite(temporary, rendered)
        os.chmod(temporary, 0o400)
        hasher = hashlib.sha256()
        size = 0
        with temporary.open("rb") as handle:
            while True:
                chunk = handle.read(1024 * 1024)
                if not chunk:
                    break
                hasher.update(chunk)
                size += len(chunk)
        if size <= 0:
            raise Refusal("destination render produced no bytes")
        os.link(temporary, path)
        parent_fd = os.open(parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(parent_fd)
        finally:
            os.close(parent_fd)
    except FileExistsError as error:
        raise Refusal(f"destination appeared during publication: {path}") from error
    finally:
        if fd >= 0:
            os.close(fd)
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass
    return hasher.hexdigest(), size


def bind_sidecar(
    *,
    sources: dict[str, object],
    pbf_sha256: str,
    pbf_size: int,
    geometry_sha256: str,
    geometry_size: int,
    clip_geoids: list[str],
    destination: str,
    mbtiles_sha256: str,
    mbtiles_size: int,
    tile_count: int,
    bounds: dict[str, float],
    min_zoom: int,
    max_zoom: int,
) -> dict[str, object]:
    pbf_url, _pbf = fetch.locked_url(sources, "pbf")
    geometry_url, _geometry = fetch.locked_url(sources, "geometry")
    sidecar = {
        "schema_version": 1,
        "kind": RENDER_KIND,
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
        "clip_geoids": list(clip_geoids),
        "clip_names": list(LOCKED_NAMES),
        "destination": destination,
        "mbtiles_sha256": mbtiles_sha256,
        "mbtiles_bytes": mbtiles_size,
        "tile_count": tile_count,
        "bounds": bounds,
        "min_zoom": min_zoom,
        "max_zoom": max_zoom,
        "format": "png",
        # Local fixture or operator-supplied source bytes never close the
        # production Maps gate. Production admission needs the real
        # candidate-bound provider object.
        "production_admitted": False,
    }
    if sidecar["kind"] == PRODUCTION_RECEIPT_KIND:
        raise Refusal("render sidecar must not be a production Maps receipt")
    if sidecar["production_admitted"] is not False:
        raise Refusal("render sidecar must never mark production_admitted")
    exact_keys(sidecar, SIDECAR_KEYS, "render sidecar")
    return sidecar


def render_local_mbtiles(
    *,
    sources_path: Path,
    source_root: Path,
    pbf: str,
    geometry: str,
    dest_root: Path,
    destination: str,
    sidecar: str,
    url: str | None = None,
    render: RenderFn | None = None,
    clip: ClipFn | None = None,
) -> dict[str, object]:
    sources = admit_authorized_sources(sources_path)
    admit_url(sources, url)
    pbf_path = resolve_source_file(source_root, pbf, "pbf")
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
    pbf_bytes = read_local_source(pbf_path, "pbf")
    geometry_bytes = read_local_source(geometry_path, "geometry")
    clip_geoids = admit_clip_geoids(geometry_bytes, clip)
    renderer = render if render is not None else default_local_render
    rendered = renderer(
        {
            "pbf_bytes": pbf_bytes,
            "geometry_bytes": geometry_bytes,
            "clip_geoids": clip_geoids,
            "pbf_path": pbf_path,
            "geometry_path": geometry_path,
        }
    )
    if not isinstance(rendered, dict):
        raise Refusal("injected render returned no MBTiles description")
    mbtiles_sha256, mbtiles_size = atomic_publish_mbtiles(dest_path, rendered)
    inspected = verify.inspect_mbtiles(dest_path, max(mbtiles_size, 1))
    record = bind_sidecar(
        sources=sources,
        pbf_sha256=digest(pbf_bytes),
        pbf_size=len(pbf_bytes),
        geometry_sha256=digest(geometry_bytes),
        geometry_size=len(geometry_bytes),
        clip_geoids=clip_geoids,
        destination=str(dest_rel),
        mbtiles_sha256=mbtiles_sha256,
        mbtiles_size=mbtiles_size,
        tile_count=int(inspected["tile_count"]),
        bounds=inspected["bounds"],
        min_zoom=int(inspected["min_zoom"]),
        max_zoom=int(inspected["max_zoom"]),
    )
    if record["mbtiles_sha256"] != inspected["mbtiles_sha256"]:
        raise Refusal("MBTiles bytes differ from the render digest")
    body = canonical(record)
    if len(body) > MAX_SIDECAR_BYTES:
        raise Refusal("render sidecar exceeds its bound")
    atomic_publish_file(sidecar_path, body, label="sidecar")
    return record


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--sources",
        type=Path,
        default=HERE / "maps-authorized-sources.json",
    )
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--pbf", required=True, help="relative local Geofabrik PBF path")
    parser.add_argument("--geometry", required=True, help="relative local TIGER zip/fixture path")
    parser.add_argument("--dest-root", type=Path, required=True)
    parser.add_argument("--destination", required=True, help="must be buffalo-niagara.mbtiles")
    parser.add_argument("--sidecar", required=True)
    parser.add_argument("--url", default=None, help="if set, must match a locked source URL")
    args = parser.parse_args()
    try:
        value = render_local_mbtiles(
            sources_path=args.sources,
            source_root=args.source_root,
            pbf=args.pbf,
            geometry=args.geometry,
            dest_root=args.dest_root,
            destination=args.destination,
            sidecar=args.sidecar,
            url=args.url,
        )
    except (Refusal, OSError, UnicodeError, ValueError, sqlite3.Error) as error:
        print(f"maps-render-local-mbtiles: refusal: {error}", file=sys.stderr)
        return EXIT_REFUSED
    print(canonical(value).decode("ascii"), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
