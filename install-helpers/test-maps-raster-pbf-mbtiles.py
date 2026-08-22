#!/usr/bin/env python3
"""Hostile tests for dest-root PBF-way raster MBTiles.

Injected osmium records the exact export argv and writes dummy GeoJSON.
Injected raster returns fixture PNG tiles. Tests never need the real
binary, never download Geofabrik, and never hit a public OSM tile CDN.
Raster sidecars are not production Maps receipts and never mark
production_admitted. Destination must be buffalo-niagara.pbf-raster.mbtiles;
the 12 KiB fixture buffalo-niagara.mbtiles is no-replace.
"""

from __future__ import annotations

import importlib.util
import json
import os
import sqlite3
import stat
import subprocess
import sys
import tempfile
from pathlib import Path
from urllib.request import urlopen

HERE = Path(__file__).resolve().parent
RASTER = HERE / "maps-raster-pbf-mbtiles.py"
LOCK = HERE / "maps-authorized-sources.json"
PBF_URL = "https://download.geofabrik.de/north-america/us/new-york-latest.osm.pbf"
TIGER_URL = "https://www2.census.gov/geo/tiger/TIGER2024/COUNTY/tl_2024_us_county.zip"
TILE_CDN = "https://tile.openstreetmap.org/0/0/0.png"
TILE_CDN_ALT = "https://tiles.openstreetmap.org/1/0/0.png"
TILE_OSM = "https://tile.osm.org/0/0/0.png"
FIXTURE_PBF = b"OSM-PBF-CLIP-FIXTURE\n"
FIXTURE_EXPORT = (
    b'{"type":"Feature","geometry":{"type":"LineString","coordinates":'
    b"[[-79.12,42.70],[-78.50,43.30]]}}\n"
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


raster = load("maps_raster_pbf_mbtiles", RASTER)


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


def write_lock(path: Path, document: dict) -> Path:
    path.write_text(json.dumps(document, indent=2) + "\n")
    path.chmod(0o444)
    return path


def base_lock() -> dict:
    return json.loads(LOCK.read_text())


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
                    "coordinates": [
                        [
                            [west, south],
                            [east, south],
                            [mid_lon, mid_lat],
                            [west, south],
                        ]
                    ],
                },
            },
            {
                "type": "Feature",
                "bbox": [west, mid_lat, east, north],
                "properties": {"GEOID": "36063", "NAME": "Niagara County"},
                "geometry": {
                    "type": "Polygon",
                    "coordinates": [
                        [
                            [west, mid_lat],
                            [east, mid_lat],
                            [mid_lon, north],
                            [west, mid_lat],
                        ]
                    ],
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
        captured["argv"] = list(argv)
        dest = Path(argv[argv.index("-o") + 1])
        dest.write_bytes(FIXTURE_EXPORT)

    return runner


def fake_raster(captured: dict, bbox: list[float] | None = None) -> raster.RasterFn:
    def renderer(request: dict) -> dict:
        captured["request"] = dict(request)
        admitted = list(bbox) if bbox is not None else list(request["bbox"])
        return {
            "tiles": ((8, 71, 161, FIXTURE_PNG),),
            "bounds": raster.bounds_dict(admitted),
            "min_zoom": 8,
            "max_zoom": 8,
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
        "destination": "buffalo-niagara.pbf-raster.mbtiles",
        "sidecar": "buffalo-niagara.pbf-raster.mbtiles.sha256.json",
        "run_osmium": fake_osmium(record_seams),
        "raster": fake_raster(record_seams),
    }
    args.update(overrides)
    return raster.raster_pbf_mbtiles(**args), record_seams


def write_fake_osmium_script(path: Path, argv_log: Path) -> Path:
    path.write_text(
        "#!/usr/bin/env python3\n"
        "import json, sys\n"
        "from pathlib import Path\n"
        f"Path({str(argv_log)!r}).write_text(json.dumps(sys.argv))\n"
        "dest = Path(sys.argv[sys.argv.index('-o') + 1])\n"
        f"dest.write_bytes({FIXTURE_EXPORT!r})\n"
    )
    path.chmod(0o755)
    return path


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

        lock = base_lock()
        assert lock["kind"] == "mcnf-maps-authorized-sources"
        assert lock["pbf"]["url"] == PBF_URL
        assert lock["geometry"]["url"] == TIGER_URL
        assert lock["geometry"]["select_geoid"] == ["36029", "36063"]
        assert "tile.openstreetmap.org" in "".join(lock["never_fetch"])
        assert raster.RASTER_KIND == "mcnf-maps-pbf-raster"
        assert raster.RASTER_KIND != "mcnf-maps-mbtiles-receipt"
        assert raster.RASTER_MBTILES_NAME == "buffalo-niagara.pbf-raster.mbtiles"
        assert raster.FIXTURE_MBTILES_NAME == "buffalo-niagara.mbtiles"
        assert raster.extract.bounds_envelope_compatible(FIXTURE_BBOX) is True
        assert raster.extract.bounds_envelope_compatible(OFFICIAL_BBOX) is True

        geojson = fixture_geojson()
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            source_root = write_sources(root, geometry=geojson)
            dest_root = root / "dest"
            dest_root.mkdir()
            (dest_root / "buffalo-niagara.mbtiles").write_bytes(b"FIXTURE-MBTILES\n")
            dest_root.joinpath("buffalo-niagara.mbtiles").chmod(0o400)

            record, captured = raster_ok(source_root=source_root, dest_root=dest_root)
            dest = dest_root / "buffalo-niagara.pbf-raster.mbtiles"
            sidecar = dest_root / "buffalo-niagara.pbf-raster.mbtiles.sha256.json"
            fixture = dest_root / "buffalo-niagara.mbtiles"
            assert dest.is_file()
            assert fixture.read_bytes() == b"FIXTURE-MBTILES\n"
            assert stat.S_IMODE(dest.stat().st_mode) == 0o400
            assert stat.S_IMODE(sidecar.stat().st_mode) == 0o400
            expected_argv = [
                "osmium",
                "export",
                "--geometry-types=linestring",
                "--output-format=geojsonseq",
                "--overwrite",
                "-o",
                captured["argv"][6],
                str(source_root / "erie-niagara.osm.pbf"),
            ]
            assert captured["argv"] == expected_argv
            assert captured["argv"][1:6] == [
                "export",
                "--geometry-types=linestring",
                "--output-format=geojsonseq",
                "--overwrite",
                "-o",
            ]
            assert captured["request"]["bbox"] == FIXTURE_BBOX
            assert record["kind"] == "mcnf-maps-pbf-raster"
            assert record["kind"] != "mcnf-maps-mbtiles-receipt"
            assert record["production_admitted"] is False
            assert record["region_id"] == "buffalo-niagara"
            assert record["provider"] == "openstreetmap-derived"
            assert record["license"] == "ODbL-1.0"
            assert record["clip_geoids"] == ["36029", "36063"]
            assert record["clip_names"] == ["Erie County", "Niagara County"]
            assert record["pbf_url"] == PBF_URL
            assert record["pbf_sha256"] == raster.digest(FIXTURE_PBF)
            assert record["pbf_bytes"] == len(FIXTURE_PBF)
            assert record["pbf_clip_sha256"] == raster.digest(FIXTURE_PBF)
            assert record["destination"] == "buffalo-niagara.pbf-raster.mbtiles"
            assert record["bbox"] == FIXTURE_BBOX
            assert record["bounds_envelope_compatible"] is True
            assert record["format"] == "png"
            assert record["tile_count"] == 1
            assert record["min_zoom"] == 8
            assert record["max_zoom"] == 8
            stored = json.loads(sidecar.read_bytes())
            assert stored == record
            metadata = inspect_mbtiles(dest)
            assert metadata["format"] == "png"
            assert metadata["provider"] == "openstreetmap-derived"
            assert metadata["license"] == "ODbL-1.0"
            assert metadata["name"] == "buffalo-niagara"
            assert metadata["attribution"]
            assert metadata["bounds"] == raster.bounds_string(FIXTURE_BBOX)

            official_src = write_sources(root / "official-src", geometry=fixture_geojson(OFFICIAL_BBOX))
            official_dest = root / "official-dest"
            official_dest.mkdir()
            official_sidecar = {
                "schema_version": 1,
                "kind": "mcnf-maps-tiger-clip",
                "bbox": OFFICIAL_BBOX,
            }
            (official_src / "erie-niagara.geojson.sha256.json").write_text(
                json.dumps(official_sidecar, sort_keys=True) + "\n"
            )
            official_seams: dict = {}
            official_record, official_captured = raster_ok(
                source_root=official_src,
                dest_root=official_dest,
                geometry_sidecar="erie-niagara.geojson.sha256.json",
                captured=official_seams,
                raster=fake_raster(official_seams, OFFICIAL_BBOX),
            )
            assert official_record["bbox"] == OFFICIAL_BBOX
            assert official_record["bounds_envelope_compatible"] is True
            assert official_record["production_admitted"] is False
            assert official_record["kind"] == "mcnf-maps-pbf-raster"
            official_meta = inspect_mbtiles(official_dest / "buffalo-niagara.pbf-raster.mbtiles")
            assert official_meta["bounds"] == raster.bounds_string(OFFICIAL_BBOX)
            assert official_meta["bounds"].startswith("-79.312136")

            export_path = dest_root / "ways.geojsonseq"
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
            assert pillow_rendered["provider"] == "openstreetmap-derived"
            assert pillow_rendered["name"] == "buffalo-niagara"
            assert pillow_rendered["tiles"]
            assert all(tile[3].startswith(raster.verify.PNG_MAGIC) for tile in pillow_rendered["tiles"])
            assert len(pillow_rendered["tiles"]) <= raster.MAX_TILES

            missing_calls: list[list[str]] = []

            def must_not_run(argv: list[str]) -> None:
                missing_calls.append(list(argv))
                raise AssertionError("missing osmium must refuse before invoke")

            expect_refusal(
                "missing-osmium",
                lambda: raster.raster_pbf_mbtiles(
                    sources_path=LOCK,
                    source_root=source_root,
                    pbf="erie-niagara.osm.pbf",
                    geometry="erie-niagara.geojson",
                    dest_root=(root / "missing-dest").mkdir() or (root / "missing-dest"),
                    destination="buffalo-niagara.pbf-raster.mbtiles",
                    sidecar="missing.json",
                    osmium=str(root / "no-such-osmium"),
                ),
                "osmium",
            )
            assert missing_calls == []
            assert not (root / "missing-dest" / "buffalo-niagara.pbf-raster.mbtiles").exists()

            overwrite_calls: list[list[str]] = []

            def must_not_overwrite(argv: list[str]) -> None:
                overwrite_calls.append(list(argv))

            expect_refusal(
                "overwrite-dest",
                lambda: raster.raster_pbf_mbtiles(
                    sources_path=LOCK,
                    source_root=source_root,
                    pbf="erie-niagara.osm.pbf",
                    geometry="erie-niagara.geojson",
                    dest_root=dest_root,
                    destination="buffalo-niagara.pbf-raster.mbtiles",
                    sidecar="second.json",
                    run_osmium=must_not_overwrite,
                    raster=fake_raster({}),
                ),
                "no-replace",
            )
            assert overwrite_calls == []
            assert dest.exists()

            fixture_calls: list[list[str]] = []

            def must_not_touch_fixture(argv: list[str]) -> None:
                fixture_calls.append(list(argv))

            expect_refusal(
                "overwrite-fixture-mbtiles",
                lambda: raster.raster_pbf_mbtiles(
                    sources_path=LOCK,
                    source_root=source_root,
                    pbf="erie-niagara.osm.pbf",
                    geometry="erie-niagara.geojson",
                    dest_root=dest_root,
                    destination="buffalo-niagara.mbtiles",
                    sidecar="fixture-overwrite.json",
                    run_osmium=must_not_touch_fixture,
                    raster=fake_raster({}),
                ),
                "buffalo-niagara.mbtiles",
            )
            assert fixture_calls == []
            assert fixture.read_bytes() == b"FIXTURE-MBTILES\n"

            expect_refusal(
                "wrong-filename",
                lambda: raster_ok(
                    source_root=source_root,
                    dest_root=(root / "name-dest").mkdir() or (root / "name-dest"),
                    destination="albany.pbf-raster.mbtiles",
                    sidecar="albany.json",
                )[0],
                "path substitution",
            )

            expect_refusal(
                "path-escape",
                lambda: raster_ok(
                    source_root=source_root,
                    dest_root=(root / "escape-dest").mkdir() or (root / "escape-dest"),
                    destination="../buffalo-niagara.pbf-raster.mbtiles",
                    sidecar="escape.json",
                )[0],
                "path substitution",
            )

            linked_root = root / "linked-root"
            linked_root.symlink_to(dest_root, target_is_directory=True)
            expect_refusal(
                "symlink-dest-root",
                lambda: raster_ok(
                    source_root=source_root,
                    dest_root=linked_root,
                    destination="via-symlink/buffalo-niagara.pbf-raster.mbtiles",
                    sidecar="via-symlink/raster.json",
                )[0],
                "path substitution",
            )

            expect_refusal(
                "tile-cdn-url",
                lambda: raster_ok(
                    source_root=source_root,
                    dest_root=(root / "cdn-dest").mkdir() or (root / "cdn-dest"),
                    sidecar="cdn.json",
                    url=TILE_CDN,
                )[0],
                "tile",
            )
            expect_refusal(
                "tile-cdn-alt",
                lambda: raster_ok(
                    source_root=source_root,
                    dest_root=(root / "cdn-alt-dest").mkdir() or (root / "cdn-alt-dest"),
                    sidecar="cdn-alt.json",
                    url=TILE_CDN_ALT,
                )[0],
                "tile",
            )
            expect_refusal(
                "tile-osm",
                lambda: raster_ok(
                    source_root=source_root,
                    dest_root=(root / "cdn-osm-dest").mkdir() or (root / "cdn-osm-dest"),
                    sidecar="cdn-osm.json",
                    url=TILE_OSM,
                )[0],
                "tile",
            )

            tiles_lock = base_lock()
            tiles_lock["tiles"] = {"url": TILE_CDN, "upstream": "osm-public-tiles"}
            expect_refusal(
                "tiles-source-kind",
                lambda: raster_ok(
                    source_root=source_root,
                    dest_root=(root / "tiles-dest").mkdir() or (root / "tiles-dest"),
                    sources_path=write_lock(root / "tiles-lock.json", tiles_lock),
                    sidecar="tiles.json",
                )[0],
                "source",
            )

            env = os.environ.copy()
            env["PYTHONPATH"] = ""
            argv_log = root / "osmium-argv.json"
            fake_bin = write_fake_osmium_script(root / "fake-osmium", argv_log)
            cli_dest = root / "cli-dest"
            cli_dest.mkdir()
            cli_source = write_sources(root / "cli-src")
            # CLI has no raster inject; fake osmium writes GeoJSON and default
            # Pillow raster must succeed on that one line.
            cli = subprocess.run(
                [
                    sys.executable,
                    str(RASTER),
                    "--sources",
                    str(LOCK),
                    "--source-root",
                    str(cli_source),
                    "--pbf",
                    "erie-niagara.osm.pbf",
                    "--geometry",
                    "erie-niagara.geojson",
                    "--dest-root",
                    str(cli_dest),
                    "--destination",
                    "buffalo-niagara.pbf-raster.mbtiles",
                    "--sidecar",
                    "cli.json",
                    "--osmium",
                    str(fake_bin),
                    "--min-zoom",
                    "8",
                    "--max-zoom",
                    "8",
                ],
                check=False,
                capture_output=True,
                text=True,
                env=env,
            )
            if cli.returncode != 0:
                raise AssertionError(f"cli raster failed: {cli.stderr}")
            cli_record = json.loads(cli.stdout)
            assert cli_record["kind"] == "mcnf-maps-pbf-raster"
            assert cli_record["kind"] != "mcnf-maps-mbtiles-receipt"
            assert cli_record["production_admitted"] is False
            assert cli_record["destination"] == "buffalo-niagara.pbf-raster.mbtiles"
            cli_argv = json.loads(argv_log.read_text())
            assert cli_argv[1:6] == [
                "export",
                "--geometry-types=linestring",
                "--output-format=geojsonseq",
                "--overwrite",
                "-o",
            ]
            assert (cli_dest / "buffalo-niagara.pbf-raster.mbtiles").is_file()
            assert stat.S_IMODE((cli_dest / "buffalo-niagara.pbf-raster.mbtiles").stat().st_mode) == 0o400

            refused = subprocess.run(
                [
                    sys.executable,
                    str(RASTER),
                    "--sources",
                    str(LOCK),
                    "--source-root",
                    str(source_root),
                    "--pbf",
                    "erie-niagara.osm.pbf",
                    "--geometry",
                    "erie-niagara.geojson",
                    "--dest-root",
                    str((root / "cli-missing").mkdir() or (root / "cli-missing")),
                    "--destination",
                    "buffalo-niagara.pbf-raster.mbtiles",
                    "--sidecar",
                    "cli-missing.json",
                    "--osmium",
                    str(root / "still-missing-osmium"),
                ],
                check=False,
                capture_output=True,
                text=True,
                env=env,
            )
            if refused.returncode != raster.EXIT_REFUSED:
                raise AssertionError(f"cli missing osmium was not refused: {refused.stdout} {refused.stderr}")
            if "osmium" not in refused.stderr.lower():
                raise AssertionError(f"cli missing osmium refusal drifted: {refused.stderr}")
            assert not (root / "cli-missing" / "buffalo-niagara.pbf-raster.mbtiles").exists()

            overwrite_cli = subprocess.run(
                [
                    sys.executable,
                    str(RASTER),
                    "--sources",
                    str(LOCK),
                    "--source-root",
                    str(source_root),
                    "--pbf",
                    "erie-niagara.osm.pbf",
                    "--geometry",
                    "erie-niagara.geojson",
                    "--dest-root",
                    str(dest_root),
                    "--destination",
                    "buffalo-niagara.pbf-raster.mbtiles",
                    "--sidecar",
                    "cli-overwrite.json",
                    "--osmium",
                    str(fake_bin),
                ],
                check=False,
                capture_output=True,
                text=True,
                env=env,
            )
            if overwrite_cli.returncode != raster.EXIT_REFUSED or "no-replace" not in overwrite_cli.stderr.lower():
                raise AssertionError(f"cli dest-exists was not refused: {overwrite_cli.stderr}")

            fixture_cli = subprocess.run(
                [
                    sys.executable,
                    str(RASTER),
                    "--sources",
                    str(LOCK),
                    "--source-root",
                    str(source_root),
                    "--pbf",
                    "erie-niagara.osm.pbf",
                    "--geometry",
                    "erie-niagara.geojson",
                    "--dest-root",
                    str(dest_root),
                    "--destination",
                    "buffalo-niagara.mbtiles",
                    "--sidecar",
                    "cli-fixture.json",
                    "--osmium",
                    str(fake_bin),
                ],
                check=False,
                capture_output=True,
                text=True,
                env=env,
            )
            if (
                fixture_cli.returncode != raster.EXIT_REFUSED
                or "buffalo-niagara.mbtiles" not in fixture_cli.stderr
            ):
                raise AssertionError(f"cli fixture overwrite was not refused: {fixture_cli.stderr}")
            assert fixture.read_bytes() == b"FIXTURE-MBTILES\n"

        print("maps raster pbf mbtiles hostile suite passed")
    finally:
        import urllib.request

        urllib.request.urlopen = original_urlopen  # type: ignore[assignment]


if __name__ == "__main__":
    main()
