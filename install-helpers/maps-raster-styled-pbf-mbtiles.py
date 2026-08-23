#!/usr/bin/env python3
"""Raster a styled dark MBTiles dest from the official Erie+Niagara PBF clip.

Construct opens Maps at view zoom 13. The dest-root z8–z10 line sketch is
one pale-on-dark overview tile when stretched to that zoom. This helper
never fetches, never talks to a public OSM tile CDN, and never marks
production_admitted.

It filters the already-local clipped PBF, exports polygon+linestring
GeoJSON, and Pillow-rasterizes water / park / highway classes onto TMS
PNG tiles at z8–z13 so the default viewport shows streets instead of one
upscaled sketch. Output leaf must be
`buffalo-niagara.styled-raster.mbtiles`. The 12 KiB fixture
`buffalo-niagara.mbtiles` and the dest-root z8–z10
`buffalo-niagara.pbf-raster.mbtiles` are no-replace.

Osmium argv lists are fixed (never a shell string). Seams are injectable
so tests never need the real binary or the 34 MiB PBF.
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
RASTER_KIND = "mcnf-maps-styled-raster"
PRODUCTION_RECEIPT_KIND = "mcnf-maps-mbtiles-receipt"
PBF_CLIP_NAME = extract.PBF_CLIP_NAME
STYLED_MBTILES_NAME = "buffalo-niagara.styled-raster.mbtiles"
LINE_RASTER_MBTILES_NAME = "buffalo-niagara.pbf-raster.mbtiles"
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
MAX_EXPORT_BYTES = 2 * 1024 * 1024 * 1024
MAX_TILES = 2048
MAX_CANVAS_PIXELS = 80 * 1024 * 1024
TILE_SIZE = 256
DEFAULT_MIN_ZOOM = 8
DEFAULT_MAX_ZOOM = 13
DEFAULT_ATTRIBUTION = "© OpenStreetMap contributors"
OSMIUM_TAGS_FILTER = "tags-filter"
OSMIUM_EXPORT = "export"
OSMIUM_GEOMETRY_TYPES = "--geometry-types=polygon,linestring,point"
OSMIUM_OUTPUT_FORMAT = "--output-format=geojsonseq"
OSMIUM_FILTERS = (
    "w/highway",
    "w/waterway",
    "w/railway",
    "nwr/natural=water",
    "nwr/water",
    "wr/leisure=park,pitch,golf_course,playground",
    "wr/landuse=forest,reservoir,basin,residential,commercial,industrial,retail,meadow,grass,recreation_ground,cemetery",
    "wr/natural=wood",
    "wr/building",
    "n/place=city,town,village,hamlet,suburb,neighbourhood",
)
BACKGROUND_RGB = (18, 22, 28)
WATER_FILL = (38, 64, 96)
PARK_FILL = (28, 48, 32)
RESIDENTIAL_FILL = (32, 34, 40)
COMMERCIAL_FILL = (44, 38, 32)
INDUSTRIAL_FILL = (38, 38, 34)
BUILDING_FILL = (54, 60, 72)
WATERWAY_RGB = (56, 92, 128)
RAILWAY_RGB = (110, 96, 88)
LABEL_RGB = (236, 238, 244)
HALO_RGB = (12, 14, 18)
CARTO_MBTILES_NAME = "buffalo-niagara.carto-raster.mbtiles"
FONT_CANDIDATES = (
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf",
    "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
)
PLACE_RANKS = {
    "city": 8,
    "town": 9,
    "village": 10,
    "suburb": 11,
    "hamlet": 12,
    "neighbourhood": 12,
}
DRAW_ORDER = (
    "landuse",
    "park",
    "water",
    "building",
    "railway",
    "waterway",
    "road",
    "place",
)
ROAD_COLORS = {
    "motorway": (232, 196, 96),
    "motorway_link": (232, 196, 96),
    "trunk": (220, 180, 80),
    "trunk_link": (220, 180, 80),
    "primary": (210, 200, 160),
    "primary_link": (210, 200, 160),
    "secondary": (180, 188, 196),
    "secondary_link": (180, 188, 196),
    "tertiary": (150, 160, 172),
    "tertiary_link": (150, 160, 172),
    "residential": (120, 132, 148),
    "unclassified": (120, 132, 148),
    "service": (96, 108, 120),
}
MAJOR_HIGHWAYS = {
    "motorway",
    "motorway_link",
    "trunk",
    "trunk_link",
    "primary",
    "primary_link",
}
ARTERIAL_HIGHWAYS = {
    "secondary",
    "secondary_link",
    "tertiary",
    "tertiary_link",
}
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
    if rel.name == LINE_RASTER_MBTILES_NAME:
        raise Refusal(
            "path substitution refused: dest-root buffalo-niagara.pbf-raster.mbtiles is no-replace"
        )
    if rel.name not in {STYLED_MBTILES_NAME, CARTO_MBTILES_NAME}:
        raise Refusal(
            "path substitution refused: MBTiles filename is not a styled buffalo-niagara raster"
        )
    return rel, fetch.resolve_beneath(dest_root, rel, "destination")


def osmium_filter_argv(osmium: str, dest: Path, src: Path) -> list[str]:
    if not osmium or not isinstance(osmium, str):
        raise Refusal("osmium is missing")
    if dest is None or src is None:
        raise Refusal("osmium argv is malformed")
    return [osmium, OSMIUM_TAGS_FILTER, "--overwrite", "-o", str(dest), str(src), *OSMIUM_FILTERS]


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


def _require_osmium_argv(argv: list[str]) -> None:
    if not isinstance(argv, list) or any(not isinstance(item, str) or not item for item in argv):
        raise Refusal("osmium argv is malformed")
    if len(argv) >= 6 and argv[1] == OSMIUM_TAGS_FILTER:
        expected = [OSMIUM_TAGS_FILTER, "--overwrite", "-o"]
        if argv[1:4] != expected or tuple(argv[6:]) != OSMIUM_FILTERS:
            raise Refusal("osmium argv is malformed")
        return
    if (
        len(argv) == 8
        and argv[1:6]
        == [OSMIUM_EXPORT, OSMIUM_GEOMETRY_TYPES, OSMIUM_OUTPUT_FORMAT, "--overwrite", "-o"]
    ):
        return
    raise Refusal("osmium argv is malformed")


def default_run_osmium(argv: list[str]) -> None:
    _require_osmium_argv(argv)
    try:
        completed = subprocess.run(argv, check=False, capture_output=True)
    except OSError as error:
        raise Refusal("osmium is missing") from error
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout or b"").decode("utf-8", "replace")[:240]
        raise Refusal(f"osmium {argv[1]} refused: {detail or 'non-zero exit'}")


def lon_to_tile_x(lon: float, zoom: int) -> float:
    return (lon + 180.0) / 360.0 * float(1 << zoom)


def lat_to_tile_y(lat: float, zoom: int) -> float:
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


def _as_point(point: object) -> tuple[float, float] | None:
    if not isinstance(point, list) or len(point) < 2:
        return None
    try:
        return (float(point[0]), float(point[1]))
    except (TypeError, ValueError):
        return None


def iter_rings(geometry: object) -> Iterable[list[tuple[float, float]]]:
    if not isinstance(geometry, dict):
        return
    kind = geometry.get("type")
    coordinates = geometry.get("coordinates")
    if kind == "Polygon":
        if not isinstance(coordinates, list) or not coordinates:
            return
        ring: list[tuple[float, float]] = []
        exterior = coordinates[0]
        if not isinstance(exterior, list):
            return
        for point in exterior:
            parsed = _as_point(point)
            if parsed is not None:
                ring.append(parsed)
        if len(ring) >= 3:
            yield ring
        return
    if kind == "MultiPolygon":
        if not isinstance(coordinates, list):
            return
        for polygon in coordinates:
            if not isinstance(polygon, list) or not polygon:
                continue
            ring = []
            exterior = polygon[0]
            if not isinstance(exterior, list):
                continue
            for point in exterior:
                parsed = _as_point(point)
                if parsed is not None:
                    ring.append(parsed)
            if len(ring) >= 3:
                yield ring
        return
    if kind == "GeometryCollection":
        geometries = geometry.get("geometries")
        if isinstance(geometries, list):
            for child in geometries:
                yield from iter_rings(child)


def iter_lines(geometry: object) -> Iterable[list[tuple[float, float]]]:
    if not isinstance(geometry, dict):
        return
    kind = geometry.get("type")
    coordinates = geometry.get("coordinates")
    if kind == "LineString":
        if not isinstance(coordinates, list):
            return
        line: list[tuple[float, float]] = []
        for point in coordinates:
            parsed = _as_point(point)
            if parsed is not None:
                line.append(parsed)
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
                parsed = _as_point(point)
                if parsed is not None:
                    line.append(parsed)
            if len(line) >= 2:
                yield line
        return
    if kind == "GeometryCollection":
        geometries = geometry.get("geometries")
        if isinstance(geometries, list):
            for child in geometries:
                yield from iter_lines(child)


def iter_points(geometry: object) -> Iterable[tuple[float, float]]:
    if not isinstance(geometry, dict):
        return
    kind = geometry.get("type")
    coordinates = geometry.get("coordinates")
    if kind == "Point":
        parsed = _as_point(coordinates)
        if parsed is not None:
            yield parsed
        return
    if kind == "MultiPoint":
        if not isinstance(coordinates, list):
            return
        for point in coordinates:
            parsed = _as_point(point)
            if parsed is not None:
                yield parsed
        return
    if kind == "GeometryCollection":
        geometries = geometry.get("geometries")
        if isinstance(geometries, list):
            for child in geometries:
                yield from iter_points(child)


def classify_feature(properties: object, geometry_type: object) -> str | None:
    if not isinstance(properties, dict) or not isinstance(geometry_type, str):
        return None
    natural = properties.get("natural")
    landuse = properties.get("landuse")
    leisure = properties.get("leisure")
    waterway = properties.get("waterway")
    highway = properties.get("highway")
    railway = properties.get("railway")
    building = properties.get("building")
    water = properties.get("water")
    place = properties.get("place")
    if geometry_type == "Point":
        if isinstance(place, str) and place in PLACE_RANKS:
            name = properties.get("name")
            if isinstance(name, str) and name.strip():
                return f"place:{place}"
        return None
    if geometry_type in {"Polygon", "MultiPolygon"}:
        if (
            natural == "water"
            or waterway == "riverbank"
            or landuse in {"reservoir", "basin"}
            or isinstance(water, str)
        ):
            return "water"
        if leisure in {"park", "pitch", "golf_course", "playground"} or landuse in {
            "forest",
            "meadow",
            "grass",
            "recreation_ground",
            "cemetery",
        } or natural == "wood":
            return "park"
        if landuse == "residential":
            return "landuse:residential"
        if landuse in {"commercial", "retail"}:
            return "landuse:commercial"
        if landuse == "industrial":
            return "landuse:industrial"
        if building not in {None, "no", "false"}:
            return "building"
        return None
    if geometry_type in {"LineString", "MultiLineString"}:
        if isinstance(highway, str) and highway in ROAD_COLORS:
            return f"road:{highway}"
        if waterway in {"river", "canal", "stream"}:
            return f"waterway:{waterway}"
        if isinstance(railway, str) and railway not in {"no", "false"}:
            return "railway"
    return None


def feature_visible(kind: str, zoom: int) -> bool:
    if kind == "water":
        return True
    if kind.startswith("waterway:"):
        waterway = kind.split(":", 1)[1]
        return zoom >= 10 if waterway == "stream" else True
    if kind == "park" or kind.startswith("landuse:"):
        return zoom >= 10
    if kind == "building":
        return zoom >= 13
    if kind == "railway":
        return zoom >= 11
    if kind.startswith("place:"):
        place = kind.split(":", 1)[1]
        return zoom >= PLACE_RANKS.get(place, 13)
    if kind.startswith("road:"):
        highway = kind.split(":", 1)[1]
        if highway in MAJOR_HIGHWAYS:
            return True
        if highway in ARTERIAL_HIGHWAYS:
            return zoom >= 10
        return zoom >= 12
    return False


def layer_key(kind: str) -> str:
    if kind.startswith("landuse:"):
        return "landuse"
    if kind.startswith("waterway:"):
        return "waterway"
    if kind.startswith("road:"):
        return "road"
    if kind.startswith("place:"):
        return "place"
    return kind


def landuse_fill(kind: str) -> tuple[int, int, int]:
    if kind == "landuse:commercial":
        return COMMERCIAL_FILL
    if kind == "landuse:industrial":
        return INDUSTRIAL_FILL
    return RESIDENTIAL_FILL


def load_label_font(size: int) -> Any | None:
    try:
        from PIL import ImageFont
    except ImportError:
        return None
    for candidate in FONT_CANDIDATES:
        path = Path(candidate)
        if path.is_file():
            try:
                return ImageFont.truetype(str(path), size=size)
            except OSError:
                continue
    return None


def label_size(place: str, zoom: int) -> int:
    if place == "city":
        return 14 if zoom >= 12 else 12
    if place == "town":
        return 12 if zoom >= 12 else 11
    return 10


def road_width(highway: str, zoom: int) -> int:
    if highway in {"motorway", "motorway_link", "trunk", "trunk_link"}:
        return 3 if zoom >= 12 else 2
    if highway in {"primary", "primary_link", "secondary", "secondary_link"}:
        return 2 if zoom >= 12 else 1
    return 1


def iter_export_features(path: Path) -> Iterable[dict[str, Any]]:
    extract.admit_regular_file(path, "osmium export", MAX_EXPORT_BYTES)
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
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
                if isinstance(document, dict):
                    yield document
        text = leftover.strip().lstrip(b"\x1e")
        if text:
            try:
                document = json.loads(text)
            except (UnicodeDecodeError, json.JSONDecodeError):
                return
            if isinstance(document, dict):
                yield document
    finally:
        os.close(descriptor)


def load_styled_features(path: Path) -> list[dict[str, Any]]:
    features: list[dict[str, Any]] = []
    for document in iter_export_features(path):
        geometry = document.get("geometry") if document.get("type") == "Feature" else document
        if not isinstance(geometry, dict):
            continue
        properties = document.get("properties") if document.get("type") == "Feature" else {}
        geom_type = geometry.get("type")
        kind = classify_feature(properties, geom_type)
        if kind is None:
            continue
        if kind.startswith("place:"):
            name = properties.get("name") if isinstance(properties, dict) else None
            if not isinstance(name, str):
                continue
            for point in iter_points(geometry):
                features.append({"kind": kind, "point": point, "name": name.strip()[:48]})
            continue
        if kind in {"water", "park", "building"} or kind.startswith("landuse:"):
            rings = [list(ring) for ring in iter_rings(geometry)]
            if rings:
                features.append({"kind": kind, "rings": rings})
            continue
        lines = [list(line) for line in iter_lines(geometry)]
        if lines:
            features.append({"kind": kind, "lines": lines})
    if not features:
        raise Refusal("osmium export produced no styled geometry to raster")
    return features


def project_points(
    points: list[tuple[float, float]], zoom: int, origin_x: int, origin_y: int
) -> list[tuple[float, float]]:
    pixels: list[tuple[float, float]] = []
    for lon, lat in points:
        x = (lon_to_tile_x(lon, zoom) - origin_x) * TILE_SIZE
        y = (lat_to_tile_y(lat, zoom) - origin_y) * TILE_SIZE
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
    features = load_styled_features(geojson_path)
    tiles: list[tuple[int, int, int, bytes]] = []
    for zoom in range(min_zoom, max_zoom + 1):
        covers = tiles_covering_bbox(admitted, zoom)
        if not covers:
            continue
        if len(tiles) + len(covers) > MAX_TILES:
            raise Refusal("raster tile count exceeds its bound")
        xs = [column for column, _row in covers]
        ys = [row for _column, row in covers]
        origin_x = min(xs)
        origin_y = min(ys)
        width = (max(xs) - origin_x + 1) * TILE_SIZE
        height = (max(ys) - origin_y + 1) * TILE_SIZE
        if width * height > MAX_CANVAS_PIXELS:
            raise Refusal("raster canvas exceeds its bound")
        canvas = Image.new("RGB", (width, height), BACKGROUND_RGB)
        draw = ImageDraw.Draw(canvas)
        fonts: dict[int, Any] = {}
        for layer in DRAW_ORDER:
            for feature in features:
                kind = feature["kind"]
                if layer_key(kind) != layer or not feature_visible(kind, zoom):
                    continue
                if kind.startswith("landuse:") or kind in {"water", "park", "building"}:
                    if kind == "water":
                        fill = WATER_FILL
                    elif kind == "park":
                        fill = PARK_FILL
                    elif kind == "building":
                        fill = BUILDING_FILL
                    else:
                        fill = landuse_fill(kind)
                    for ring in feature["rings"]:
                        pixels = project_points(ring, zoom, origin_x, origin_y)
                        if len(pixels) >= 3:
                            draw.polygon(pixels, fill=fill)
                    continue
                if kind.startswith("place:"):
                    place = kind.split(":", 1)[1]
                    size = label_size(place, zoom)
                    font = fonts.get(size)
                    if font is None:
                        font = load_label_font(size)
                        fonts[size] = font
                    if font is None:
                        continue
                    lon, lat = feature["point"]
                    x, y = project_points([(lon, lat)], zoom, origin_x, origin_y)[0]
                    name = feature["name"]
                    for dx, dy in ((-1, 0), (1, 0), (0, -1), (0, 1)):
                        draw.text((x + dx, y + dy), name, font=font, fill=HALO_RGB, anchor="mm")
                    draw.text((x, y), name, font=font, fill=LABEL_RGB, anchor="mm")
                    continue
                if kind == "railway":
                    color = RAILWAY_RGB
                    width_px = 2 if zoom >= 13 else 1
                elif kind.startswith("waterway:"):
                    color = WATERWAY_RGB
                    width_px = 2 if zoom >= 12 else 1
                else:
                    highway = kind.split(":", 1)[1]
                    color = ROAD_COLORS[highway]
                    width_px = road_width(highway, zoom)
                for line in feature["lines"]:
                    pixels = project_points(line, zoom, origin_x, origin_y)
                    if len(pixels) >= 2:
                        draw.line(pixels, fill=color, width=width_px)
        for column, row in covers:
            box = (
                (column - origin_x) * TILE_SIZE,
                (row - origin_y) * TILE_SIZE,
                (column - origin_x + 1) * TILE_SIZE,
                (row - origin_y + 1) * TILE_SIZE,
            )
            tiles.append((zoom, column, xyz_to_tms_row(zoom, row), encode_png(canvas.crop(box))))
    if not tiles:
        raise Refusal("MBTiles contains no tiles")
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
            "center": (
                f"{(admitted[0] + admitted[2]) / 2:.6f},"
                f"{(admitted[1] + admitted[3]) / 2:.6f},"
                f"{rendered['max_zoom']}"
            ),
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


def raster_styled_pbf_mbtiles(
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
    filter_fd, filter_name = tempfile.mkstemp(prefix=".erie-niagara.styled.", suffix=".osm.pbf", dir=parent)
    filter_path = Path(filter_name)
    os.close(filter_fd)
    export_fd, export_name = tempfile.mkstemp(prefix=".erie-niagara.styled.", suffix=".geojsonseq", dir=parent)
    export_path = Path(export_name)
    os.close(export_fd)
    try:
        filter_path.unlink()
    except FileNotFoundError:
        pass
    try:
        export_path.unlink()
    except FileNotFoundError:
        pass
    binary = osmium if run_osmium is not None else resolve_osmium(osmium)
    runner = run_osmium if run_osmium is not None else default_run_osmium
    try:
        runner(osmium_filter_argv(binary, filter_path, pbf_path))
        if not filter_path.exists():
            raise Refusal("osmium tags-filter produced no destination")
        runner(osmium_export_argv(binary, export_path, filter_path))
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
        for temporary in (filter_path, export_path):
            try:
                temporary.unlink()
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
        help="buffalo-niagara.styled-raster.mbtiles or buffalo-niagara.carto-raster.mbtiles; fixture and pbf-raster are no-replace",
    )
    parser.add_argument("--sidecar", required=True)
    parser.add_argument("--url", default=None, help="if set, must match a locked authorized source URL")
    parser.add_argument("--osmium", default="osmium", help="osmium binary; refused when missing")
    parser.add_argument("--min-zoom", type=int, default=DEFAULT_MIN_ZOOM)
    parser.add_argument("--max-zoom", type=int, default=DEFAULT_MAX_ZOOM)
    args = parser.parse_args()
    try:
        value = raster_styled_pbf_mbtiles(
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
        print(f"maps-raster-styled-pbf-mbtiles: refusal: {error}", file=sys.stderr)
        return EXIT_REFUSED
    print(canonical(value).decode("ascii"), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
