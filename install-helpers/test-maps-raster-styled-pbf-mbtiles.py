#!/usr/bin/env python3
"""Hostile tests for dest-root styled PBF raster MBTiles.

Injected osmium records the tags-filter + export argv lists and writes
dummy GeoJSON. Injected raster returns fixture PNG tiles. Tests never
need the real binary, never download Geofabrik, and never hit a public
OSM tile CDN. Sidecars are not production Maps receipts and never mark
production_admitted. Destination must be
buffalo-niagara.styled-raster.mbtiles; the 12 KiB fixture and the
dest-root z8–z10 pbf-raster are no-replace.
"""

from __future__ import annotations

import importlib.util
import json
import os
import sqlite3
import stat
import sys
import tempfile
from pathlib import Path
from urllib.request import urlopen

HERE = Path(__file__).resolve().parent
RASTER = HERE / "maps-raster-styled-pbf-mbtiles.py"
LOCK = HERE / "maps-authorized-sources.json"
PBF_URL = "https://download.geofabrik.de/north-america/us/new-york-latest.osm.pbf"
TIGER_URL = "https://www2.census.gov/geo/tiger/TIGER2024/COUNTY/tl_2024_us_county.zip"
FIXTURE_PBF = b"OSM-PBF-CLIP-FIXTURE\n"
FIXTURE_EXPORT = (
    b'{"type":"Feature","properties":{"highway":"primary"},'
    b'"geometry":{"type":"LineString","coordinates":[[-79.12,42.70],[-78.50,43.30]]}}\n'
    b'{"type":"Feature","properties":{"natural":"water"},'
    b'"geometry":{"type":"Polygon","coordinates":'
    b"[[[-79.10,42.80],[-78.90,42.80],[-78.90,42.95],[-79.10,42.95],[-79.10,42.80]]]}}\n"
)
OFFICIAL_BBOX = [-79.312136, 42.437997, -78.460416, 43.634799]
FIXTURE_BBOX = [-79.120000, 42.700000, -78.500000, 43.300000]
FIXTURE_PNG = bytes.fromhex(
    "89504e470d0a1a0a0000000d4948445200000001000000010802000000907753de"
    "0000000c49444154789c63f8cfc00000000300010005fed42b0000000049454e44ae426082"
)


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(module)
    return module


raster = load("maps_raster_styled_pbf_mbtiles", RASTER)


def network_get(url: str, *args, **kwargs):
    raise AssertionError(f"test must never download: {url}")


def expect_refusal(label: str, call, needle: str) -> None:
    try:
        call()
    except raster.Refusal as error:
        text = str(error).lower()
        if needle not in text:
            raise AssertionError(f"{label} refusal message drifted: {error}") from error
        return
    raise AssertionError(f"hostile case was accepted: {label}")


def fixture_geojson(bbox: list[float] | None = None) -> bytes:
    west, south, east, north = bbox if bbox is not None else FIXTURE_BBOX
    mid_lat = (south + north) / 2.0
    mid_lon = (west + east) / 2.0
    collection = {
        "type": "FeatureCollection",
        "bbox": [west, south, east, north],
        "features": [
            {
                "type": "Feature",
                "bbox": [west, south, east, mid_lat],
                "properties": {"GEOID": "36029", "NAME": "Erie County"},
                "geometry": {
                    "type": "Polygon",
                    "coordinates": [[[west, south], [east, south], [mid_lon, mid_lat], [west, south]]],
                },
            },
            {
                "type": "Feature",
                "bbox": [west, mid_lat, east, north],
                "properties": {"GEOID": "36063", "NAME": "Niagara County"},
                "geometry": {
                    "type": "Polygon",
                    "coordinates": [[[west, mid_lat], [east, mid_lat], [mid_lon, north], [west, mid_lat]]],
                },
            },
        ],
    }
    return (json.dumps(collection, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii")


def write_sources(root: Path, *, pbf: bytes = FIXTURE_PBF, geometry: bytes | None = None) -> Path:
    source_root = root / "source"
    source_root.mkdir(parents=True)
    (source_root / "erie-niagara.osm.pbf").write_bytes(pbf)
    (source_root / "erie-niagara.geojson").write_bytes(geometry if geometry is not None else fixture_geojson())
    return source_root


def fake_osmium(captured: dict) -> raster.OsmiumFn:
    def runner(argv: list[str]) -> None:
        captured.setdefault("argv", []).append(list(argv))
        dest = Path(argv[argv.index("-o") + 1])
        if argv[1] == "tags-filter":
            dest.write_bytes(b"OSM-FILTERED\n")
            return
        dest.write_bytes(FIXTURE_EXPORT)

    return runner


def fake_raster(captured: dict, bbox: list[float] | None = None) -> raster.RasterFn:
    def renderer(request: dict) -> dict:
        captured["request"] = dict(request)
        admitted = list(bbox) if bbox is not None else list(request["bbox"])
        return {
            "tiles": ((13, 2320, 5470, FIXTURE_PNG),),
            "bounds": raster.bounds_dict(admitted),
            "min_zoom": 8,
            "max_zoom": 13,
            "attribution": raster.DEFAULT_ATTRIBUTION,
            "provider": raster.APPROVED_PROVIDER,
            "license": raster.APPROVED_LICENSE,
            "name": raster.REGION_ID,
            "format": "png",
        }

    return renderer


def raster_ok(*, source_root: Path, dest_root: Path, captured: dict | None = None, **overrides):
    record_seams = captured if captured is not None else {}
    args = {
        "sources_path": LOCK,
        "source_root": source_root,
        "pbf": "erie-niagara.osm.pbf",
        "geometry": "erie-niagara.geojson",
        "dest_root": dest_root,
        "destination": "buffalo-niagara.styled-raster.mbtiles",
        "sidecar": "buffalo-niagara.styled-raster.mbtiles.sha256.json",
        "run_osmium": fake_osmium(record_seams),
        "raster": fake_raster(record_seams),
    }
    args.update(overrides)
    return raster.raster_styled_pbf_mbtiles(**args), record_seams


def inspect_mbtiles(path: Path) -> dict[str, str]:
    connection = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        return {name: value for name, value in connection.execute("SELECT name, value FROM metadata")}
    finally:
        connection.close()


def main() -> None:
    original_urlopen = urlopen
    try:
        import urllib.request

        urllib.request.urlopen = network_get  # type: ignore[assignment]
        raster.fetch.default_https_get = network_get  # type: ignore[method-assign]
        raster.extract.fetch.default_https_get = network_get  # type: ignore[method-assign]

        lock = json.loads(LOCK.read_text())
        assert lock["kind"] == "mcnf-maps-authorized-sources"
        assert lock["pbf"]["url"] == PBF_URL
        assert lock["geometry"]["url"] == TIGER_URL
        assert "tile.openstreetmap.org" in "".join(lock["never_fetch"])
        assert raster.RASTER_KIND == "mcnf-maps-styled-raster"
        assert raster.RASTER_KIND != "mcnf-maps-mbtiles-receipt"
        assert raster.STYLED_MBTILES_NAME == "buffalo-niagara.styled-raster.mbtiles"
        assert raster.CARTO_MBTILES_NAME == "buffalo-niagara.carto-raster.mbtiles"
        assert raster.FIXTURE_MBTILES_NAME == "buffalo-niagara.mbtiles"
        assert raster.LINE_RASTER_MBTILES_NAME == "buffalo-niagara.pbf-raster.mbtiles"
        assert raster.DEFAULT_MAX_ZOOM == 13
        assert raster.classify_feature({"highway": "primary"}, "LineString") == "road:primary"
        assert raster.classify_feature({"natural": "water"}, "Polygon") == "water"
        assert raster.classify_feature({"building": "yes"}, "Polygon") == "building"
        assert raster.classify_feature({"landuse": "residential"}, "Polygon") == "landuse:residential"
        assert raster.classify_feature({"place": "city", "name": "Buffalo"}, "Point") == "place:city"
        assert raster.feature_visible("road:residential", 11) is False
        assert raster.feature_visible("road:residential", 12) is True
        assert raster.feature_visible("building", 12) is False
        assert raster.feature_visible("building", 13) is True

        geojson = fixture_geojson()
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            source_root = write_sources(root, geometry=geojson)
            dest_root = root / "dest"
            dest_root.mkdir()
            (dest_root / "buffalo-niagara.mbtiles").write_bytes(b"FIXTURE-MBTILES\n")
            dest_root.joinpath("buffalo-niagara.mbtiles").chmod(0o400)
            (dest_root / "buffalo-niagara.pbf-raster.mbtiles").write_bytes(b"LINE-RASTER\n")
            dest_root.joinpath("buffalo-niagara.pbf-raster.mbtiles").chmod(0o400)

            record, captured = raster_ok(source_root=source_root, dest_root=dest_root)
            dest = dest_root / "buffalo-niagara.styled-raster.mbtiles"
            sidecar = dest_root / "buffalo-niagara.styled-raster.mbtiles.sha256.json"
            assert dest.is_file()
            assert dest_root.joinpath("buffalo-niagara.mbtiles").read_bytes() == b"FIXTURE-MBTILES\n"
            assert dest_root.joinpath("buffalo-niagara.pbf-raster.mbtiles").read_bytes() == b"LINE-RASTER\n"
            assert stat.S_IMODE(dest.stat().st_mode) == 0o400
            assert stat.S_IMODE(sidecar.stat().st_mode) == 0o400
            assert len(captured["argv"]) == 2
            assert captured["argv"][0][1:4] == ["tags-filter", "--overwrite", "-o"]
            assert captured["argv"][0][6:] == list(raster.OSMIUM_FILTERS)
            assert captured["argv"][1][1:6] == [
                "export",
                "--geometry-types=polygon,linestring,point",
                "--output-format=geojsonseq",
                "--overwrite",
                "-o",
            ]
            assert record["kind"] == "mcnf-maps-styled-raster"
            assert record["kind"] != "mcnf-maps-mbtiles-receipt"
            assert record["production_admitted"] is False
            assert record["region_id"] == "buffalo-niagara"
            assert record["provider"] == "openstreetmap-derived"
            assert record["license"] == "ODbL-1.0"
            assert record["clip_geoids"] == ["36029", "36063"]
            assert record["destination"] == "buffalo-niagara.styled-raster.mbtiles"
            assert record["bbox"] == FIXTURE_BBOX
            assert record["min_zoom"] == 8
            assert record["max_zoom"] == 13
            assert record["format"] == "png"
            stored = json.loads(sidecar.read_bytes())
            assert stored == record
            metadata = inspect_mbtiles(dest)
            assert metadata["maxzoom"] == "13"
            assert metadata["center"].endswith(",13")
            assert metadata["bounds"] == raster.bounds_string(FIXTURE_BBOX)

            expect_refusal(
                "fixture-no-replace",
                lambda: raster.raster_styled_pbf_mbtiles(
                    sources_path=LOCK,
                    source_root=source_root,
                    pbf="erie-niagara.osm.pbf",
                    geometry="erie-niagara.geojson",
                    dest_root=dest_root,
                    destination="buffalo-niagara.mbtiles",
                    sidecar="hostile-fixture.json",
                    run_osmium=fake_osmium({}),
                    raster=fake_raster({}),
                ),
                "no-replace",
            )
            expect_refusal(
                "line-raster-no-replace",
                lambda: raster.raster_styled_pbf_mbtiles(
                    sources_path=LOCK,
                    source_root=source_root,
                    pbf="erie-niagara.osm.pbf",
                    geometry="erie-niagara.geojson",
                    dest_root=dest_root,
                    destination="buffalo-niagara.pbf-raster.mbtiles",
                    sidecar="hostile-line.json",
                    run_osmium=fake_osmium({}),
                    raster=fake_raster({}),
                ),
                "no-replace",
            )
            expect_refusal(
                "dest-exists",
                lambda: raster.raster_styled_pbf_mbtiles(
                    sources_path=LOCK,
                    source_root=source_root,
                    pbf="erie-niagara.osm.pbf",
                    geometry="erie-niagara.geojson",
                    dest_root=dest_root,
                    destination="buffalo-niagara.styled-raster.mbtiles",
                    sidecar="second.json",
                    run_osmium=fake_osmium({}),
                    raster=fake_raster({}),
                ),
                "no-replace",
            )
            expect_refusal(
                "missing-osmium",
                lambda: raster.raster_styled_pbf_mbtiles(
                    sources_path=LOCK,
                    source_root=source_root,
                    pbf="erie-niagara.osm.pbf",
                    geometry="erie-niagara.geojson",
                    dest_root=(root / "missing-dest").mkdir() or (root / "missing-dest"),
                    destination="buffalo-niagara.styled-raster.mbtiles",
                    sidecar="missing.json",
                    osmium=str(root / "no-such-osmium"),
                ),
                "osmium is missing",
            )

            export_path = dest_root / "styled.geojsonseq"
            export_path.write_bytes(FIXTURE_EXPORT)
            pillow_rendered = raster.default_pillow_raster(
                {
                    "geojson_path": export_path,
                    "bbox": FIXTURE_BBOX,
                    "min_zoom": 8,
                    "max_zoom": 8,
                }
            )
            assert pillow_rendered["format"] == "png"
            assert pillow_rendered["tiles"]
            assert all(tile[3].startswith(raster.verify.PNG_MAGIC) for tile in pillow_rendered["tiles"])
            assert pillow_rendered["max_zoom"] == 8

        print("maps raster styled pbf mbtiles hostile suite passed")
    finally:
        import urllib.request

        urllib.request.urlopen = original_urlopen  # type: ignore[assignment]


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"maps raster styled pbf mbtiles hostile suite failed: {error}", file=sys.stderr)
        raise
