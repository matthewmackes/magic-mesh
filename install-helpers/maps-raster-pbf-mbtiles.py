#!/usr/bin/env python3
"""Raster official-clip PBF ways into dest-root PNG MBTiles.

Operator lock (2026-08-22): dest-root already holds the clipped Erie+Niagara
PBF and official county GeoJSON. This helper never fetches, never talks to a
public OSM tile CDN, and never marks production_admitted.

The osmium export and Pillow raster seams are injectable so tests never need
the real binary or a 34 MiB PBF. Default invocation is a fixed argv list
only (never a shell string):

`osmium export --geometry-types=linestring --output-format=geojsonseq
 --overwrite -o DEST SRC`

Ways are rasterized with stdlib + Pillow into TMS PNG tiles over the
official clip bbox at a bounded zoom (z8–z10). Output leaf must be
`buffalo-niagara.pbf-raster.mbtiles`. The 12 KiB fixture
`buffalo-niagara.mbtiles` is no-replace. Sidecar kind is
`mcnf-maps-pbf-raster`, not a production MBTiles receipt. Official bbox
west may escape the verifier envelope; this helper does not shrink it.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import io
import json
import math
import os
import sqlite3
import stat
import subprocess
import sys
import tempfile
from collections.abc import Callable, Iterable
from pathlib import Path, PurePosixPath
from typing import Any

HERE = Path(__file__).resolve().parent


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


extract = _load("maps_extract_pbf_clip", HERE / "maps-extract-pbf-clip.py")
fetch = extract.fetch
verify = extract.verify

EXIT_REFUSED = fetch.EXIT_REFUSED
RASTER_KIND = "mcnf-maps-pbf-raster"
PRODUCTION_RECEIPT_KIND = "mcnf-maps-mbtiles-receipt"
PBF_CLIP_NAME = extract.PBF_CLIP_NAME
RASTER_MBTILES_NAME = "buffalo-niagara.pbf-raster.mbtiles"
FIXTURE_MBTILES_NAME = verify.MBTILES_NAME
REGION_ID = extract.REGION_ID
LOCKED_GEOIDS = extract.LOCKED_GEOIDS
LOCKED_NAMES = extract.LOCKED_NAMES
OPERATOR_AUTHORIZATION = fetch.OPERATOR_AUTHORIZATION
APPROVED_PROVIDER = extract.APPROVED_PROVIDER
APPROVED_LICENSE = extract.APPROVED_LICENSE
MAX_SIDECAR_BYTES = fetch.MAX_SIDECAR_BYTES
MAX_SOURCE_FILE_BYTES = extract.MAX_SOURCE_FILE_BYTES
MAX_GEOJSON_BYTES = extract.MAX_GEOJSON_BYTES
MAX_EXPORT_BYTES = 512 * 1024 * 1024
MAX_TILES = 64
TILE_SIZE = 256
DEFAULT_MIN_ZOOM = 8
DEFAULT_MAX_ZOOM = 10
DEFAULT_ATTRIBUTION = "© OpenStreetMap contributors"
OSMIUM_EXPORT = "export"
OSMIUM_GEOMETRY_TYPES = "--geometry-types=linestring"
OSMIUM_OUTPUT_FORMAT = "--output-format=geojsonseq"
LINE_RGB = (196, 208, 220)
BACKGROUND_RGB = (18, 22, 28)
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
    "mbtiles_sha256",
    "mbtiles_bytes",
    "tile_count",
    "bbox",
    "bounds_envelope_compatible",
    "min_zoom",
    "max_zoom",
    "format",
    "production_admitted",
}

Refusal = fetch.Refusal
OsmiumFn = Callable[[list[str]], None]
RasterFn = Callable[[dict[str, Any]], dict[str, Any]]


def canonical(value: object) -> bytes:
    return fetch.canonical(value)


def digest(data: bytes) -> str:
    return fetch.digest(data)


def exact_keys(value: object, expected: set[str], label: str) -> dict:
    return fetch.exact_keys(value, expected, label)


def bounds_string(bbox: list[float]) -> str:
    return ",".join(f"{value:.6f}" for value in bbox)


def bounds_dict(bbox: list[float]) -> dict[str, float]:
    west, south, east, north = bbox
    return {"west": west, "south": south, "east": east, "north": north}


def resolve_source_file(source_root: Path, relative: str, label: str) -> Path:
    return extract.resolve_source_file(source_root, relative, label)


def resolve_pbf(source_root: Path, relative: str) -> Path:
    rel = fetch.relative_leaf(relative, "pbf")
    if rel.name != PBF_CLIP_NAME:
        raise Refusal("path substitution refused: PBF filename is not erie-niagara.osm.pbf")
    return resolve_source_file(source_root, relative, "pbf")


def resolve_destination(dest_root: Path, relative: str) -> tuple[PurePosixPath, Path]:
    fetch.real_directory(dest_root, "dest-root")
    rel = fetch.relative_leaf(relative, "destination")
    if rel.name == FIXTURE_MBTILES_NAME:
        raise Refusal(
            "path substitution refused: fixture buffalo-niagara.mbtiles is no-replace"
        )
    if rel.name != RASTER_MBTILES_NAME:
        raise Refusal(
            "path substitution refused: MBTiles filename is not buffalo-niagara.pbf-raster.mbtiles"
        )
    return rel, fetch.resolve_beneath(dest_root, rel, "destination")


def osmium_export_argv(osmium: str, dest: Path, src: Path) -> list[str]:
    if not osmium or not isinstance(osmium, str):
        raise Refusal("osmium is missing")
    if dest is None or src is None:
        raise Refusal("osmium argv is malformed")
    return [
        osmium,
        OSMIUM_EXPORT,
        OSMIUM_GEOMETRY_TYPES,
        OSMIUM_OUTPUT_FORMAT,
        "--overwrite",
        "-o",
        str(dest),
        str(src),
    ]


def resolve_osmium(osmium: str) -> str:
    return extract.resolve_osmium(osmium)


def default_run_osmium(argv: list[str]) -> None:
    if not isinstance(argv, list) or any(not isinstance(item, str) or not item for item in argv):
        raise Refusal("osmium argv is malformed")
    if (
        len(argv) != 8
        or argv[1:6] != [OSMIUM_EXPORT, OSMIUM_GEOMETRY_TYPES, OSMIUM_OUTPUT_FORMAT, "--overwrite", "-o"]
    ):
        raise Refusal("osmium argv is malformed")
    try:
        completed = subprocess.run(argv, check=False, capture_output=True)
    except OSError as error:
        raise Refusal("osmium is missing") from error
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout or b"").decode("utf-8", "replace")[:240]
        raise Refusal(f"osmium export refused: {detail or 'non-zero exit'}")


def lon_to_tile_x(lon: float, zoom: int) -> float:
    return (lon + 180.0) / 360.0 * float(1 << zoom)


def lat_to_tile_y(lat: float, zoom: int) -> float:
    # Web-Mercator OSM/XYZ row. TMS row is flipped at pack time.
    clamped = min(max(lat, -85.05112878), 85.05112878)
    lat_rad = math.radians(clamped)
    return (1.0 - math.log(math.tan(lat_rad) + (1.0 / math.cos(lat_rad))) / math.pi) / 2.0 * float(
        1 << zoom
    )


def xyz_to_tms_row(zoom: int, y: int) -> int:
    return (1 << zoom) - 1 - y


def tiles_covering_bbox(bbox: list[float], zoom: int) -> list[tuple[int, int]]:
    west, south, east, north = bbox
    x0 = int(math.floor(lon_to_tile_x(west, zoom)))
    x1 = int(math.floor(lon_to_tile_x(east, zoom)))
    y0 = int(math.floor(lat_to_tile_y(north, zoom)))
    y1 = int(math.floor(lat_to_tile_y(south, zoom)))
    side = 1 << zoom
    x0 = max(0, min(side - 1, x0))
    x1 = max(0, min(side - 1, x1))
    y0 = max(0, min(side - 1, y0))
    y1 = max(0, min(side - 1, y1))
    if x1 < x0:
        x0, x1 = x1, x0
    if y1 < y0:
        y0, y1 = y1, y0
    return [(column, row) for column in range(x0, x1 + 1) for row in range(y0, y1 + 1)]


def iter_line_coords(geometry: object) -> Iterable[list[tuple[float, float]]]:
    if not isinstance(geometry, dict):
        return
    kind = geometry.get("type")
    coordinates = geometry.get("coordinates")
    if kind == "LineString":
        if not isinstance(coordinates, list):
            return
        line: list[tuple[float, float]] = []
        for point in coordinates:
            if not isinstance(point, list) or len(point) < 2:
                continue
            try:
                line.append((float(point[0]), float(point[1])))
            except (TypeError, ValueError):
                continue
        if len(line) >= 2:
            yield line
        return
    if kind == "MultiLineString":
        if not isinstance(coordinates, list):
            return
        for part in coordinates:
            if not isinstance(part, list):
                continue
            line = []
            for point in part:
                if not isinstance(point, list) or len(point) < 2:
                    continue
                try:
                    line.append((float(point[0]), float(point[1])))
                except (TypeError, ValueError):
                    continue
            if len(line) >= 2:
                yield line
        return
    if kind == "GeometryCollection":
        geometries = geometry.get("geometries")
        if isinstance(geometries, list):
            for child in geometries:
                yield from iter_line_coords(child)
        return
    if kind == "Feature":
        yield from iter_line_coords(geometry.get("geometry"))
        return
    if kind == "FeatureCollection":
        features = geometry.get("features")
        if isinstance(features, list):
            for feature in features:
                yield from iter_line_coords(feature)


def iter_export_lines(path: Path) -> Iterable[list[tuple[float, float]]]:
    extract.admit_regular_file(path, "osmium export", MAX_EXPORT_BYTES)
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        prefix = os.read(descriptor, 4096)
        os.lseek(descriptor, 0, os.SEEK_SET)
        lowered = prefix[:64].lstrip().lower()
        if lowered.startswith(b"{") and b'"type"' in prefix[:256] and b"featurecollection" in prefix[:512].lower():
            data = b""
            remaining = MAX_EXPORT_BYTES
            while remaining:
                chunk = os.read(descriptor, min(1024 * 1024, remaining))
                if not chunk:
                    break
                data += chunk
                remaining -= len(chunk)
            try:
                document = json.loads(data)
            except (UnicodeDecodeError, json.JSONDecodeError) as error:
                raise Refusal(f"osmium export is not valid GeoJSON: {error}") from error
            yield from iter_line_coords(document)
            return
        leftover = b""
        while True:
            chunk = os.read(descriptor, 1024 * 1024)
            if not chunk:
                break
            leftover += chunk
            while b"\n" in leftover:
                raw, leftover = leftover.split(b"\n", 1)
                text = raw.strip().lstrip(b"\x1e")
                if not text:
                    continue
                try:
                    document = json.loads(text)
                except (UnicodeDecodeError, json.JSONDecodeError):
                    continue
                yield from iter_line_coords(document)
        text = leftover.strip().lstrip(b"\x1e")
        if text:
            try:
                document = json.loads(text)
            except (UnicodeDecodeError, json.JSONDecodeError):
                return
            yield from iter_line_coords(document)
    finally:
        os.close(descriptor)


def project_line(line: list[tuple[float, float]], zoom: int, column: int, row: int) -> list[tuple[float, float]]:
    pixels: list[tuple[float, float]] = []
    for lon, lat in line:
        x = (lon_to_tile_x(lon, zoom) - column) * TILE_SIZE
        y = (lat_to_tile_y(lat, zoom) - row) * TILE_SIZE
        pixels.append((x, y))
    return pixels


def encode_png(image: Any) -> bytes:
    buffer = io.BytesIO()
    image.save(buffer, format="PNG", optimize=True)
    payload = buffer.getvalue()
    if not payload.startswith(verify.PNG_MAGIC):
        raise Refusal("MBTiles tile payload is not PNG")
    return payload


def default_pillow_raster(request: dict[str, Any]) -> dict[str, Any]:
    try:
        from PIL import Image, ImageDraw
    except ImportError as error:
        raise Refusal("python3-pillow is missing") from error
    geojson_path = request.get("geojson_path")
    bbox = request.get("bbox")
    min_zoom = request.get("min_zoom", DEFAULT_MIN_ZOOM)
    max_zoom = request.get("max_zoom", DEFAULT_MAX_ZOOM)
    if not isinstance(geojson_path, Path):
        raise Refusal("raster export path is missing")
    if not isinstance(bbox, list):
        raise Refusal("raster bbox is malformed")
    admitted = extract.admit_bbox(bbox, "raster")
    if not isinstance(min_zoom, int) or not isinstance(max_zoom, int) or isinstance(min_zoom, bool):
        raise Refusal("MBTiles zoom policy is invalid")
    if min_zoom < 0 or min_zoom > max_zoom or max_zoom > verify.MAX_ZOOM:
        raise Refusal("MBTiles zoom policy is invalid")
    covers: list[tuple[int, int, int]] = []
    for zoom in range(min_zoom, max_zoom + 1):
        for column, row in tiles_covering_bbox(admitted, zoom):
            covers.append((zoom, column, row))
            if len(covers) > MAX_TILES:
                raise Refusal("raster tile count exceeds its bound")
    if not covers:
        raise Refusal("MBTiles contains no tiles")
    canvases: dict[tuple[int, int, int], Any] = {}
    drawers: dict[tuple[int, int, int], Any] = {}
    for key in covers:
        image = Image.new("RGB", (TILE_SIZE, TILE_SIZE), BACKGROUND_RGB)
        canvases[key] = image
        drawers[key] = ImageDraw.Draw(image)
    drew = False
    for line in iter_export_lines(geojson_path):
        for zoom, column, row in covers:
            pixels = project_line(line, zoom, column, row)
            if not any(-TILE_SIZE <= x <= TILE_SIZE * 2 and -TILE_SIZE <= y <= TILE_SIZE * 2 for x, y in pixels):
                continue
            drawers[(zoom, column, row)].line(pixels, fill=LINE_RGB, width=1)
            drew = True
    if not drew:
        raise Refusal("osmium export produced no ways to raster")
    tiles = []
    for zoom, column, row in covers:
        tiles.append((zoom, column, xyz_to_tms_row(zoom, row), encode_png(canvases[(zoom, column, row)])))
    return {
        "tiles": tuple(tiles),
        "bounds": bounds_dict(admitted),
        "min_zoom": min_zoom,
        "max_zoom": max_zoom,
        "attribution": DEFAULT_ATTRIBUTION,
        "provider": APPROVED_PROVIDER,
        "license": APPROVED_LICENSE,
        "name": REGION_ID,
        "format": "png",
    }


def write_mbtiles_sqlite(path: Path, rendered: dict[str, Any], bbox: list[float]) -> None:
    tiles = rendered["tiles"]
    if not tiles:
        raise Refusal("MBTiles contains no tiles")
    if len(tiles) > MAX_TILES:
        raise Refusal("raster tile count exceeds its bound")
    if rendered.get("format") != "png":
        raise Refusal("MBTiles format must be png")
    verify.require_provider(rendered.get("provider"), "raster provider")
    verify.require_text(rendered.get("attribution"), "raster attribution")
    if rendered.get("license") != APPROVED_LICENSE:
        raise Refusal("MBTiles license must be ODbL-1.0")
    if rendered.get("name") not in {None, REGION_ID}:
        raise Refusal("path substitution refused: MBTiles name is not buffalo-niagara")
    # Official clip bbox is written as-is. Do not call verify.parse_bounds:
    # west -79.312136 escapes the verifier envelope -79.30, and shrinking
    # would cheat the leftover production gate.
    admitted = extract.admit_bbox(bbox, "MBTiles")
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
            "bounds": bounds_string(admitted),
            "center": f"{(admitted[0] + admitted[2]) / 2:.6f},{(admitted[1] + admitted[3]) / 2:.6f},{rendered['min_zoom']}",
            "provider": APPROVED_PROVIDER,
            "attribution": rendered["attribution"],
            "license": APPROVED_LICENSE,
            "name": REGION_ID,
        }
        for key, value in metadata.items():
            connection.execute("INSERT INTO metadata VALUES (?, ?)", (key, value))
        identities: set[tuple[int, int, int]] = set()
        for zoom, column, row, data in tiles:
            if any(isinstance(value, bool) or not isinstance(value, int) or value < 0 for value in (zoom, column, row)):
                raise Refusal("MBTiles tile coordinates must be integers")
            key = (zoom, column, row)
            if key in identities:
                raise Refusal("MBTiles tile identity is duplicated")
            identities.add(key)
            if not isinstance(data, (bytes, bytearray)) or not bytes(data).startswith(verify.PNG_MAGIC):
                raise Refusal("MBTiles tile payload is not PNG")
            connection.execute(
                "INSERT INTO tiles VALUES (?, ?, ?, ?)",
                (zoom, column, row, bytes(data)),
            )
        connection.commit()
    finally:
        connection.close()


def inspect_raster_mbtiles(path: Path, bbox: list[float]) -> dict[str, object]:
    before = path.lstat()
    if stat.S_ISLNK(before.st_mode):
        raise Refusal("path substitution refused: MBTiles is a symlink")
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        raise Refusal("MBTiles must be a singly-linked regular file")
    if before.st_mode & 0o222:
        raise Refusal("MBTiles input is mutable")
    if before.st_size <= 0:
        raise Refusal("MBTiles is empty")
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        hasher = hashlib.sha256()
        remaining = before.st_size
        while remaining:
            chunk = os.read(descriptor, min(1024 * 1024, remaining))
            if not chunk:
                raise Refusal("MBTiles truncated while reading")
            hasher.update(chunk)
            remaining -= len(chunk)
        file_digest = hasher.hexdigest()
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
            verify.require_text(value, f"MBTiles metadata {name}")
            metadata[name] = value
        if metadata.get("format") != "png":
            raise Refusal("MBTiles format must be png")
        verify.require_provider(metadata.get("provider"), "MBTiles metadata provider")
        verify.require_text(metadata.get("attribution"), "MBTiles attribution")
        if metadata.get("license", APPROVED_LICENSE) != APPROVED_LICENSE:
            raise Refusal("MBTiles license must be ODbL-1.0")
        if metadata.get("name") not in {None, REGION_ID}:
            raise Refusal("path substitution refused: MBTiles name is not buffalo-niagara")
        if metadata.get("bounds") != bounds_string(bbox):
            raise Refusal("MBTiles bounds were shrunk or substituted")
        try:
            min_zoom = int(metadata.get("minzoom", "0"))
            max_zoom = int(metadata.get("maxzoom", "0"))
        except ValueError as error:
            raise Refusal("MBTiles zoom metadata is not an integer") from error
        if min_zoom < 0 or min_zoom > max_zoom or max_zoom > verify.MAX_ZOOM:
            raise Refusal("MBTiles zoom policy is invalid")
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
            if not isinstance(data, (bytes, bytearray)) or not data.startswith(verify.PNG_MAGIC):
                raise Refusal("MBTiles tile payload is not PNG")
            tile_count += 1
            if tile_count > MAX_TILES:
                raise Refusal("raster tile count exceeds its bound")
        if tile_count < 1:
            raise Refusal("MBTiles contains no tiles")
    finally:
        connection.close()
    return {
        "mbtiles_sha256": file_digest,
        "mbtiles_bytes": before.st_size,
        "tile_count": tile_count,
        "min_zoom": min_zoom,
        "max_zoom": max_zoom,
        "bounds_envelope_compatible": extract.bounds_envelope_compatible(bbox),
    }


def atomic_publish_mbtiles(path: Path, rendered: dict[str, Any], bbox: list[float]) -> tuple[str, int]:
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
        write_mbtiles_sqlite(temporary, rendered, bbox)
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
            raise Refusal("destination raster produced no bytes")
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
    destination: str,
    clip_sha256: str,
    clip_size: int,
    mbtiles_sha256: str,
    mbtiles_size: int,
    tile_count: int,
    bbox: list[float],
    min_zoom: int,
    max_zoom: int,
) -> dict[str, object]:
    pbf_url, _pbf = fetch.locked_url(sources, "pbf")
    geometry_url, _geometry = fetch.locked_url(sources, "geometry")
    fetch.refuse_tile_cdn(pbf_url)
    fetch.refuse_tile_cdn(geometry_url)
    sidecar = {
        "schema_version": 1,
        "kind": RASTER_KIND,
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
        "mbtiles_sha256": mbtiles_sha256,
        "mbtiles_bytes": mbtiles_size,
        "tile_count": tile_count,
        "bbox": bbox,
        "bounds_envelope_compatible": extract.bounds_envelope_compatible(bbox),
        "min_zoom": min_zoom,
        "max_zoom": max_zoom,
        "format": "png",
        "production_admitted": False,
    }
    if sidecar["kind"] == PRODUCTION_RECEIPT_KIND:
        raise Refusal("raster sidecar must not be a production Maps receipt")
    if sidecar["kind"] != RASTER_KIND:
        raise Refusal("raster sidecar kind is unsupported")
    if sidecar["production_admitted"] is not False:
        raise Refusal("raster sidecar must never mark production_admitted")
    exact_keys(sidecar, SIDECAR_KEYS, "raster sidecar")
    return sidecar


def raster_pbf_mbtiles(
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
    raster: RasterFn | None = None,
    min_zoom: int = DEFAULT_MIN_ZOOM,
    max_zoom: int = DEFAULT_MAX_ZOOM,
) -> dict[str, object]:
    sources = extract.admit_authorized_sources(sources_path)
    extract.admit_url(sources, url)
    pbf_path = resolve_pbf(source_root, pbf)
    geometry_path = resolve_source_file(source_root, geometry, "geometry")
    dest_rel, dest_path = resolve_destination(dest_root, destination)
    sidecar_rel = fetch.relative_leaf(sidecar, "sidecar")
    sidecar_path = fetch.resolve_beneath(dest_root, sidecar_rel, "sidecar")
    if dest_path in {sidecar_path, pbf_path, geometry_path}:
        raise Refusal("path substitution refused: destination collides with an input path")
    if dest_path.exists() or dest_path.is_symlink():
        raise Refusal("destination already exists; publication is no-replace")
    if sidecar_path.exists() or sidecar_path.is_symlink():
        raise Refusal("sidecar already exists; publication is no-replace")
    pbf_sha256, pbf_size = extract.hash_local_source(pbf_path, "pbf", MAX_SOURCE_FILE_BYTES)
    geometry_bytes = extract.read_local_source(geometry_path, "geometry", MAX_GEOJSON_BYTES)
    geoids, bbox = extract.bbox_from_geojson(geometry_bytes)
    if geoids != list(LOCKED_GEOIDS):
        raise Refusal("clip must be Erie 36029 / Niagara 36063")
    if geometry_sidecar is not None:
        geometry_sidecar_path = resolve_source_file(source_root, geometry_sidecar, "geometry sidecar")
        sidecar_bbox = extract.bbox_from_geometry_sidecar(geometry_sidecar_path)
        if sidecar_bbox is not None:
            bbox = sidecar_bbox
    bbox = extract.admit_bbox(bbox, "official clip")
    parent = dest_path.parent
    fetch.real_directory(parent, "destination parent")
    export_fd, export_name = tempfile.mkstemp(prefix=".erie-niagara.ways.", suffix=".geojsonseq", dir=parent)
    export_path = Path(export_name)
    os.close(export_fd)
    try:
        export_path.unlink()
    except FileNotFoundError:
        pass
    binary = osmium if run_osmium is not None else resolve_osmium(osmium)
    argv = osmium_export_argv(binary, export_path, pbf_path)
    runner = run_osmium if run_osmium is not None else default_run_osmium
    try:
        runner(argv)
        if not export_path.exists():
            raise Refusal("osmium export produced no destination")
        renderer = raster if raster is not None else default_pillow_raster
        rendered = renderer(
            {
                "geojson_path": export_path,
                "bbox": list(bbox),
                "min_zoom": min_zoom,
                "max_zoom": max_zoom,
                "pbf_path": pbf_path,
            }
        )
        if not isinstance(rendered, dict):
            raise Refusal("injected raster returned no MBTiles description")
        mbtiles_sha256, mbtiles_size = atomic_publish_mbtiles(dest_path, rendered, bbox)
        inspected = inspect_raster_mbtiles(dest_path, bbox)
        if inspected["mbtiles_sha256"] != mbtiles_sha256:
            raise Refusal("MBTiles bytes differ from the raster digest")
        record = bind_sidecar(
            sources=sources,
            pbf_sha256=pbf_sha256,
            pbf_size=pbf_size,
            geometry_sha256=digest(geometry_bytes),
            geometry_size=len(geometry_bytes),
            destination=str(dest_rel),
            clip_sha256=pbf_sha256,
            clip_size=pbf_size,
            mbtiles_sha256=mbtiles_sha256,
            mbtiles_size=mbtiles_size,
            tile_count=int(inspected["tile_count"]),
            bbox=bbox,
            min_zoom=int(inspected["min_zoom"]),
            max_zoom=int(inspected["max_zoom"]),
        )
        sidecar_body = canonical(record)
        if len(sidecar_body) > MAX_SIDECAR_BYTES:
            raise Refusal("raster sidecar exceeds its bound")
        fetch.atomic_write_bytes(sidecar_path, sidecar_body, label="sidecar")
        return record
    except Exception:
        if dest_path.exists() and dest_path != pbf_path:
            try:
                dest_path.unlink()
            except OSError:
                pass
        if sidecar_path.exists():
            try:
                sidecar_path.unlink()
            except OSError:
                pass
        raise
    finally:
        try:
            export_path.unlink()
        except FileNotFoundError:
            pass


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--sources",
        type=Path,
        default=HERE / "maps-authorized-sources.json",
    )
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--pbf", required=True, help="relative clipped PBF; must be erie-niagara.osm.pbf")
    parser.add_argument("--geometry", required=True, help="relative local Erie/Niagara GeoJSON")
    parser.add_argument("--geometry-sidecar", default=None, help="optional GeoJSON sidecar with bbox")
    parser.add_argument("--dest-root", type=Path, required=True)
    parser.add_argument(
        "--destination",
        required=True,
        help="must be buffalo-niagara.pbf-raster.mbtiles; buffalo-niagara.mbtiles is no-replace",
    )
    parser.add_argument("--sidecar", required=True)
    parser.add_argument("--url", default=None, help="if set, must match a locked authorized source URL")
    parser.add_argument("--osmium", default="osmium", help="osmium binary; refused when missing")
    parser.add_argument("--min-zoom", type=int, default=DEFAULT_MIN_ZOOM)
    parser.add_argument("--max-zoom", type=int, default=DEFAULT_MAX_ZOOM)
    args = parser.parse_args()
    try:
        value = raster_pbf_mbtiles(
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
            min_zoom=args.min_zoom,
            max_zoom=args.max_zoom,
        )
    except (Refusal, OSError, UnicodeError, ValueError, sqlite3.Error) as error:
        print(f"maps-raster-pbf-mbtiles: refusal: {error}", file=sys.stderr)
        return EXIT_REFUSED
    print(canonical(value).decode("ascii"), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
