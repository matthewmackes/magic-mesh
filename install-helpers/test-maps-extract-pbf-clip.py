#!/usr/bin/env python3
"""Hostile tests for Erie+Niagara PBF clip extract.

Injected osmium records the exact argv and writes dummy bytes. Tests never
need the real binary, never download Geofabrik, and never hit a public OSM
tile CDN. Extract sidecars are not production Maps receipts and never mark
production_admitted.
"""

from __future__ import annotations

import importlib.util
import json
import os
import stat
import subprocess
import sys
import tempfile
from pathlib import Path
from urllib.request import urlopen

HERE = Path(__file__).resolve().parent
EXTRACTOR = HERE / "maps-extract-pbf-clip.py"
LOCK = HERE / "maps-authorized-sources.json"
PBF_URL = "https://download.geofabrik.de/north-america/us/new-york-latest.osm.pbf"
TIGER_URL = "https://www2.census.gov/geo/tiger/TIGER2024/COUNTY/tl_2024_us_county.zip"
TILE_CDN = "https://tile.openstreetmap.org/0/0/0.png"
TILE_CDN_ALT = "https://tiles.openstreetmap.org/1/0/0.png"
TILE_OSM = "https://tile.osm.org/0/0/0.png"
FIXTURE_PBF = b"OSM-PBF-FIXTURE\n"
FIXTURE_CLIP = b"OSM-PBF-CLIP-FIXTURE\n"
# Official TIGER extract bbox (west is slightly west of verify envelope -79.30).
OFFICIAL_BBOX = [-79.312136, 42.437997, -78.460416, 43.634799]
FIXTURE_BBOX = [-79.120000, 42.700000, -78.500000, 43.300000]


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(module)
    return module


extract = load("maps_extract_pbf_clip", EXTRACTOR)


def network_get(url: str, *args, **kwargs):
    raise AssertionError(f"test must never download: {url}")


def expect_refusal(label: str, call, needle: str) -> None:
    try:
        call()
    except extract.Refusal as error:
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
    west, south, east, north = bbox if bbox is not None else [-79.12, 42.70, -78.50, 43.30]
    mid_lat = (south + north) / 2.0
    mid_lon = (west + east) / 2.0
    collection = {
        "type": "FeatureCollection",
        "bbox": bbox if bbox is not None else [west, south, east, north],
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
    (source_root / "new-york-latest.osm.pbf").write_bytes(pbf)
    (source_root / "erie-niagara.geojson").write_bytes(geometry if geometry is not None else fixture_geojson())
    return source_root


def fake_osmium(captured: dict) -> extract.OsmiumFn:
    def runner(argv: list[str]) -> None:
        captured["argv"] = list(argv)
        dest = Path(argv[argv.index("-o") + 1])
        dest.write_bytes(FIXTURE_CLIP)

    return runner


def extract_ok(*, source_root: Path, dest_root: Path, captured: dict | None = None, **overrides):
    record_argv = captured if captured is not None else {}
    args = {
        "sources_path": LOCK,
        "source_root": source_root,
        "pbf": "new-york-latest.osm.pbf",
        "geometry": "erie-niagara.geojson",
        "dest_root": dest_root,
        "destination": "erie-niagara.osm.pbf",
        "sidecar": "erie-niagara.osm.pbf.sha256.json",
        "run_osmium": fake_osmium(record_argv),
    }
    args.update(overrides)
    return extract.extract_pbf_clip(**args), record_argv


def write_fake_osmium_script(path: Path, argv_log: Path) -> Path:
    path.write_text(
        "#!/usr/bin/env python3\n"
        "import json, sys\n"
        "from pathlib import Path\n"
        f"Path({str(argv_log)!r}).write_text(json.dumps(sys.argv))\n"
        "dest = Path(sys.argv[sys.argv.index('-o') + 1])\n"
        f"dest.write_bytes({FIXTURE_CLIP!r})\n"
    )
    path.chmod(0o755)
    return path


def main() -> None:
    original_urlopen = urlopen
    try:
        import urllib.request

        urllib.request.urlopen = network_get  # type: ignore[assignment]
        extract.fetch.default_https_get = network_get  # type: ignore[method-assign]

        lock = base_lock()
        assert lock["kind"] == "mcnf-maps-authorized-sources"
        assert lock["pbf"]["url"] == PBF_URL
        assert lock["geometry"]["url"] == TIGER_URL
        assert lock["geometry"]["select_geoid"] == ["36029", "36063"]
        assert "tile.openstreetmap.org" in "".join(lock["never_fetch"])

        geojson = fixture_geojson()
        geoids, bbox = extract.bbox_from_geojson(geojson)
        assert geoids == ["36029", "36063"]
        assert bbox == [-79.12, 42.70, -78.50, 43.30]
        assert extract.format_bbox_arg(bbox) == "-79.120000,42.700000,-78.500000,43.300000"
        assert extract.bounds_envelope_compatible(bbox) is True
        assert extract.bounds_envelope_compatible(OFFICIAL_BBOX) is False

        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            source_root = write_sources(root, geometry=geojson)
            dest_root = root / "dest"
            dest_root.mkdir()

            record, captured = extract_ok(source_root=source_root, dest_root=dest_root)
            dest = dest_root / "erie-niagara.osm.pbf"
            sidecar = dest_root / "erie-niagara.osm.pbf.sha256.json"
            assert dest.read_bytes() == FIXTURE_CLIP
            assert stat.S_IMODE(dest.stat().st_mode) == 0o400
            assert stat.S_IMODE(sidecar.stat().st_mode) == 0o400
            expected_argv = [
                "osmium",
                "extract",
                "--strategy=smart",
                "--bbox=-79.120000,42.700000,-78.500000,43.300000",
                "--overwrite",
                "-o",
                str(dest),
                str(source_root / "new-york-latest.osm.pbf"),
            ]
            assert captured["argv"] == expected_argv
            assert record["kind"] == "mcnf-maps-pbf-clip"
            assert record["kind"] != "mcnf-maps-mbtiles-receipt"
            assert record["production_admitted"] is False
            assert record["region_id"] == "buffalo-niagara"
            assert record["provider"] == "openstreetmap-derived"
            assert record["clip_geoids"] == ["36029", "36063"]
            assert record["clip_names"] == ["Erie County", "Niagara County"]
            assert record["pbf_url"] == PBF_URL
            assert record["pbf_sha256"] == extract.digest(FIXTURE_PBF)
            assert record["pbf_bytes"] == len(FIXTURE_PBF)
            assert record["pbf_clip_sha256"] == extract.digest(FIXTURE_CLIP)
            assert record["pbf_clip_bytes"] == len(FIXTURE_CLIP)
            assert record["destination"] == "erie-niagara.osm.pbf"
            assert record["bbox"] == [-79.12, 42.70, -78.50, 43.30]
            assert record["bounds_envelope_compatible"] is True
            stored = json.loads(sidecar.read_bytes())
            assert stored == record
            assert stored["production_admitted"] is False
            assert stored["kind"] != "mcnf-maps-mbtiles-receipt"

            official_src = write_sources(
                root / "official-src",
                geometry=fixture_geojson(OFFICIAL_BBOX),
            )
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
            official_record, official_captured = extract_ok(
                source_root=official_src,
                dest_root=official_dest,
                geometry_sidecar="erie-niagara.geojson.sha256.json",
            )
            assert official_record["bbox"] == OFFICIAL_BBOX
            assert official_record["bounds_envelope_compatible"] is False
            assert official_record["production_admitted"] is False
            assert official_captured["argv"][3] == "--bbox=-79.312136,42.437997,-78.460416,43.634799"

            missing_calls: list[list[str]] = []

            def must_not_run(argv: list[str]) -> None:
                missing_calls.append(list(argv))
                raise AssertionError("missing osmium must refuse before invoke")

            expect_refusal(
                "missing-osmium",
                lambda: extract.extract_pbf_clip(
                    sources_path=LOCK,
                    source_root=source_root,
                    pbf="new-york-latest.osm.pbf",
                    geometry="erie-niagara.geojson",
                    dest_root=(root / "missing-dest").mkdir() or (root / "missing-dest"),
                    destination="erie-niagara.osm.pbf",
                    sidecar="missing.json",
                    osmium=str(root / "no-such-osmium"),
                ),
                "osmium",
            )
            assert missing_calls == []
            assert not (root / "missing-dest" / "erie-niagara.osm.pbf").exists()

            overwrite_calls: list[list[str]] = []

            def must_not_overwrite(argv: list[str]) -> None:
                overwrite_calls.append(list(argv))

            expect_refusal(
                "overwrite-dest",
                lambda: extract.extract_pbf_clip(
                    sources_path=LOCK,
                    source_root=source_root,
                    pbf="new-york-latest.osm.pbf",
                    geometry="erie-niagara.geojson",
                    dest_root=dest_root,
                    destination="erie-niagara.osm.pbf",
                    sidecar="second.json",
                    run_osmium=must_not_overwrite,
                ),
                "no-replace",
            )
            assert overwrite_calls == []
            assert dest.read_bytes() == FIXTURE_CLIP

            (dest_root / "nested").mkdir()
            expect_refusal(
                "overwrite-sidecar",
                lambda: extract_ok(
                    source_root=source_root,
                    dest_root=dest_root,
                    destination="nested/erie-niagara.osm.pbf",
                    sidecar="erie-niagara.osm.pbf.sha256.json",
                )[0],
                "no-replace",
            )
            assert not (dest_root / "nested" / "erie-niagara.osm.pbf").exists()

            expect_refusal(
                "wrong-filename",
                lambda: extract_ok(
                    source_root=source_root,
                    dest_root=(root / "name-dest").mkdir() or (root / "name-dest"),
                    destination="albany.osm.pbf",
                    sidecar="albany.json",
                )[0],
                "path substitution",
            )

            expect_refusal(
                "path-escape",
                lambda: extract_ok(
                    source_root=source_root,
                    dest_root=(root / "escape-dest").mkdir() or (root / "escape-dest"),
                    destination="../erie-niagara.osm.pbf",
                    sidecar="escape.json",
                )[0],
                "path substitution",
            )

            linked_root = root / "linked-root"
            linked_root.symlink_to(dest_root, target_is_directory=True)
            expect_refusal(
                "symlink-dest-root",
                lambda: extract_ok(
                    source_root=source_root,
                    dest_root=linked_root,
                    destination="via-symlink/erie-niagara.osm.pbf",
                    sidecar="via-symlink/extract.json",
                )[0],
                "path substitution",
            )

            expect_refusal(
                "tile-cdn-url",
                lambda: extract_ok(
                    source_root=source_root,
                    dest_root=(root / "cdn-dest").mkdir() or (root / "cdn-dest"),
                    sidecar="cdn.json",
                    url=TILE_CDN,
                )[0],
                "tile",
            )
            expect_refusal(
                "tile-cdn-alt",
                lambda: extract_ok(
                    source_root=source_root,
                    dest_root=(root / "cdn-alt-dest").mkdir() or (root / "cdn-alt-dest"),
                    sidecar="cdn-alt.json",
                    url=TILE_CDN_ALT,
                )[0],
                "tile",
            )
            expect_refusal(
                "tile-osm",
                lambda: extract_ok(
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
                lambda: extract_ok(
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
            cli = subprocess.run(
                [
                    sys.executable,
                    str(EXTRACTOR),
                    "--sources",
                    str(LOCK),
                    "--source-root",
                    str(source_root),
                    "--pbf",
                    "new-york-latest.osm.pbf",
                    "--geometry",
                    "erie-niagara.geojson",
                    "--dest-root",
                    str(cli_dest),
                    "--destination",
                    "erie-niagara.osm.pbf",
                    "--sidecar",
                    "cli.json",
                    "--osmium",
                    str(fake_bin),
                ],
                check=False,
                capture_output=True,
                text=True,
                env=env,
            )
            if cli.returncode != 0:
                raise AssertionError(f"cli extract failed: {cli.stderr}")
            cli_record = json.loads(cli.stdout)
            assert cli_record["kind"] == "mcnf-maps-pbf-clip"
            assert cli_record["kind"] != "mcnf-maps-mbtiles-receipt"
            assert cli_record["production_admitted"] is False
            assert cli_record["bbox"] == [-79.12, 42.70, -78.50, 43.30]
            cli_argv = json.loads(argv_log.read_text())
            assert cli_argv[1:6] == [
                "extract",
                "--strategy=smart",
                "--bbox=-79.120000,42.700000,-78.500000,43.300000",
                "--overwrite",
                "-o",
            ]
            assert cli_argv[6] == str(cli_dest / "erie-niagara.osm.pbf")
            assert (cli_dest / "erie-niagara.osm.pbf").read_bytes() == FIXTURE_CLIP

            refused = subprocess.run(
                [
                    sys.executable,
                    str(EXTRACTOR),
                    "--sources",
                    str(LOCK),
                    "--source-root",
                    str(source_root),
                    "--pbf",
                    "new-york-latest.osm.pbf",
                    "--geometry",
                    "erie-niagara.geojson",
                    "--dest-root",
                    str((root / "cli-missing").mkdir() or (root / "cli-missing")),
                    "--destination",
                    "erie-niagara.osm.pbf",
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
            if refused.returncode != extract.EXIT_REFUSED:
                raise AssertionError(f"cli missing osmium was not refused: {refused.stdout} {refused.stderr}")
            if "osmium" not in refused.stderr.lower():
                raise AssertionError(f"cli missing osmium refusal drifted: {refused.stderr}")
            assert not (root / "cli-missing" / "erie-niagara.osm.pbf").exists()

            overwrite_cli = subprocess.run(
                [
                    sys.executable,
                    str(EXTRACTOR),
                    "--sources",
                    str(LOCK),
                    "--source-root",
                    str(source_root),
                    "--pbf",
                    "new-york-latest.osm.pbf",
                    "--geometry",
                    "erie-niagara.geojson",
                    "--dest-root",
                    str(dest_root),
                    "--destination",
                    "erie-niagara.osm.pbf",
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
            if overwrite_cli.returncode != extract.EXIT_REFUSED or "no-replace" not in overwrite_cli.stderr.lower():
                raise AssertionError(f"cli dest-exists was not refused: {overwrite_cli.stderr}")

        print("maps extract pbf clip hostile suite passed")
    finally:
        import urllib.request

        urllib.request.urlopen = original_urlopen  # type: ignore[assignment]


if __name__ == "__main__":
    main()
