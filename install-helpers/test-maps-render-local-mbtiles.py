#!/usr/bin/env python3
"""Hostile tests for the local Maps MBTiles renderer contract.

Fixtures never download the Geofabrik PBF or any public OSM tile CDN.
Rendered sidecars are not production Maps receipts and never mark
production_admitted. Clip is Erie 36029 / Niagara 36063 only.
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
RENDERER = HERE / "maps-render-local-mbtiles.py"
LOCK = HERE / "maps-authorized-sources.json"
PBF_URL = "https://download.geofabrik.de/north-america/us/new-york-latest.osm.pbf"
TIGER_URL = "https://www2.census.gov/geo/tiger/TIGER2024/COUNTY/tl_2024_us_county.zip"
WRONG_URL = "https://download.geofabrik.de/europe/germany-latest.osm.pbf"
TILE_CDN = "https://tile.openstreetmap.org/0/0/0.png"
TILE_CDN_ALT = "https://tiles.openstreetmap.org/1/0/0.png"
TILE_OSM = "https://tile.osm.org/0/0/0.png"
FIXTURE_PBF = b"OSM-PBF-FIXTURE\n"
FIXTURE_GEOMETRY = b"PK\x03\x04TIGER-FIXTURE\n36029 Erie County\n36063 Niagara County\n"
FIXTURE_GEOMETRY_JSON = json.dumps(
    {"select_geoid": ["36029", "36063"], "select_name": ["Erie County", "Niagara County"]}
).encode("ascii")


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(module)
    return module


render = load("maps_render_local_mbtiles", RENDERER)


def network_get(url: str, *args, **kwargs):
    raise AssertionError(f"test must never download: {url}")


def expect_refusal(label: str, call, needle: str) -> None:
    try:
        call()
    except render.Refusal as error:
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


def write_sources(root: Path, pbf: bytes = FIXTURE_PBF, geometry: bytes = FIXTURE_GEOMETRY) -> Path:
    source_root = root / "source"
    source_root.mkdir(parents=True)
    (source_root / "new-york-latest.osm.pbf").write_bytes(pbf)
    (source_root / "tl_2024_us_county.zip").write_bytes(geometry)
    return source_root


def render_ok(*, source_root: Path, dest_root: Path, **overrides):
    args = {
        "sources_path": LOCK,
        "source_root": source_root,
        "pbf": "new-york-latest.osm.pbf",
        "geometry": "tl_2024_us_county.zip",
        "dest_root": dest_root,
        "destination": "buffalo-niagara.mbtiles",
        "sidecar": "buffalo-niagara.mbtiles.render.json",
    }
    args.update(overrides)
    return render.render_local_mbtiles(**args)


def main() -> None:
    original_urlopen = urlopen
    try:
        import urllib.request

        urllib.request.urlopen = network_get  # type: ignore[assignment]
        render.fetch.default_https_get = network_get  # type: ignore[method-assign]

        lock = base_lock()
        assert lock["kind"] == "mcnf-maps-authorized-sources"
        assert lock["pbf"]["url"] == PBF_URL
        assert lock["pbf"]["upstream"] == "geofabrik"
        assert lock["geometry"]["url"] == TIGER_URL
        assert lock["geometry"]["select_geoid"] == ["36029", "36063"]
        assert "tile.openstreetmap.org" in "".join(lock["never_fetch"])

        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            source_root = write_sources(root)
            dest_root = root / "dest"
            dest_root.mkdir()

            record = render_ok(source_root=source_root, dest_root=dest_root)
            mbtiles = dest_root / "buffalo-niagara.mbtiles"
            sidecar = dest_root / "buffalo-niagara.mbtiles.render.json"
            assert mbtiles.name == "buffalo-niagara.mbtiles"
            assert stat.S_IMODE(mbtiles.stat().st_mode) == 0o400
            assert stat.S_IMODE(sidecar.stat().st_mode) == 0o400
            assert record["kind"] == "mcnf-maps-local-render"
            assert record["kind"] != "mcnf-maps-mbtiles-receipt"
            assert record["production_admitted"] is False
            assert record["region_id"] == "buffalo-niagara"
            assert record["provider"] == "openstreetmap-derived"
            assert record["clip_geoids"] == ["36029", "36063"]
            assert record["clip_names"] == ["Erie County", "Niagara County"]
            assert record["pbf_url"] == PBF_URL
            assert record["geometry_url"] == TIGER_URL
            assert record["pbf_sha256"] == render.digest(FIXTURE_PBF)
            assert record["geometry_sha256"] == render.digest(FIXTURE_GEOMETRY)
            assert record["format"] == "png"
            assert record["destination"] == "buffalo-niagara.mbtiles"
            stored = json.loads(sidecar.read_bytes())
            assert stored == record
            assert stored["production_admitted"] is False

            inspected = render.verify.inspect_mbtiles(mbtiles, record["mbtiles_bytes"])
            assert inspected["mbtiles_sha256"] == record["mbtiles_sha256"]
            assert inspected["tile_count"] >= 1
            connection = sqlite3.connect(f"file:{mbtiles}?mode=ro", uri=True)
            try:
                tiles = list(connection.execute("SELECT tile_data FROM tiles"))
                assert tiles
                assert all(row[0].startswith(render.verify.PNG_MAGIC) for row in tiles)
                meta = dict(connection.execute("SELECT name, value FROM metadata"))
                assert meta["format"] == "png"
                assert meta["name"] == "buffalo-niagara"
                assert meta["provider"] == "openstreetmap-derived"
            finally:
                connection.close()

            json_root = root / "json-src"
            json_dest = root / "json-dest"
            json_src = write_sources(json_root, geometry=FIXTURE_GEOMETRY_JSON)
            json_dest.mkdir()
            json_record = render_ok(
                source_root=json_src,
                dest_root=json_dest,
                sidecar="json-render.json",
            )
            assert json_record["clip_geoids"] == ["36029", "36063"]
            assert json_record["production_admitted"] is False

            injected = {"called": False}

            def injected_render(request):
                injected["called"] = True
                if request["clip_geoids"] != ["36029", "36063"]:
                    raise AssertionError("injected render saw unlocked clip")
                return render.default_local_render(request)

            injected_dest = root / "injected-dest"
            injected_dest.mkdir()
            (injected_dest / "nested").mkdir()
            injected_record = render_ok(
                source_root=source_root,
                dest_root=injected_dest,
                destination="nested/buffalo-niagara.mbtiles",
                sidecar="nested/render.json",
                render=injected_render,
            )
            assert injected["called"] is True
            assert injected_record["destination"] == "nested/buffalo-niagara.mbtiles"
            assert injected_record["production_admitted"] is False
            assert injected_record["kind"] == "mcnf-maps-local-render"
            assert (injected_dest / "nested" / "buffalo-niagara.mbtiles").is_file()

            def claiming_production(request):
                payload = render.default_local_render(request)
                # Hostile: a fixture renderer must not be able to close the
                # production Maps gate by stamping the sidecar itself.
                payload["production_admitted"] = True
                payload["kind"] = render.PRODUCTION_RECEIPT_KIND
                return payload

            claim_dest = root / "claim-dest"
            claim_dest.mkdir()
            claim_record = render_ok(
                source_root=source_root,
                dest_root=claim_dest,
                sidecar="claimed-production.json",
                render=claiming_production,
            )
            if claim_record["production_admitted"] is not False:
                raise AssertionError("fixture local-render claimed production_admitted")
            if claim_record["kind"] != render.RENDER_KIND:
                raise AssertionError("fixture local-render claimed a production receipt kind")
            if claim_record["kind"] == render.PRODUCTION_RECEIPT_KIND:
                raise AssertionError("fixture local-render published a production Maps receipt")
            claimed_sidecar = json.loads((claim_dest / "claimed-production.json").read_bytes())
            if claimed_sidecar["production_admitted"] is not False:
                raise AssertionError("published fixture sidecar claimed production_admitted")
            if claimed_sidecar["kind"] != "mcnf-maps-local-render":
                raise AssertionError("published fixture sidecar is not a local-render record")
            try:
                render.verify.verify_receipt(
                    claim_dest / "claimed-production.json",
                    claim_dest / "buffalo-niagara.mbtiles",
                    "a" * 40,
                    1,
                    claim_record["mbtiles_bytes"],
                )
            except render.verify.Refusal:
                pass
            else:
                raise AssertionError("local-render sidecar was admitted as a production receipt")

            bound = render.bind_sidecar(
                sources=render.admit_authorized_sources(LOCK),
                pbf_sha256=render.digest(FIXTURE_PBF),
                pbf_size=len(FIXTURE_PBF),
                geometry_sha256=render.digest(FIXTURE_GEOMETRY),
                geometry_size=len(FIXTURE_GEOMETRY),
                clip_geoids=["36029", "36063"],
                destination="buffalo-niagara.mbtiles",
                mbtiles_sha256="0" * 64,
                mbtiles_size=1,
                tile_count=1,
                bounds=dict(render.FIXTURE_BOUNDS),
                min_zoom=1,
                max_zoom=1,
            )
            if bound["production_admitted"] is not False:
                raise AssertionError("bind_sidecar claimed production_admitted on fixture bytes")
            if bound["kind"] != render.RENDER_KIND:
                raise AssertionError("bind_sidecar kind drifted off local-render")

            expect_refusal(
                "wrong-url",
                lambda: render_ok(
                    source_root=source_root,
                    dest_root=dest_root,
                    destination="wrong/buffalo-niagara.mbtiles",
                    sidecar="wrong/render.json",
                    url=WRONG_URL,
                ),
                "wrong url",
            )
            assert not (dest_root / "wrong").exists()

            for label, banned in (
                ("tile-cdn", TILE_CDN),
                ("tile-cdn-plural", TILE_CDN_ALT),
                ("tile-osm", TILE_OSM),
            ):
                expect_refusal(
                    label,
                    lambda banned=banned: render_ok(
                        source_root=source_root,
                        dest_root=dest_root,
                        destination="cdn/buffalo-niagara.mbtiles",
                        sidecar="cdn/render.json",
                        url=banned,
                    ),
                    "tile",
                )
            assert not (dest_root / "cdn").exists()

            expect_refusal(
                "path-escape-dest",
                lambda: render_ok(
                    source_root=source_root,
                    dest_root=dest_root,
                    destination="../buffalo-niagara.mbtiles",
                    sidecar="escape.json",
                ),
                "path substitution",
            )
            expect_refusal(
                "path-escape-absolute",
                lambda: render_ok(
                    source_root=source_root,
                    dest_root=dest_root,
                    destination="/tmp/buffalo-niagara.mbtiles",
                    sidecar="escape-abs.json",
                ),
                "path substitution",
            )
            expect_refusal(
                "path-escape-pbf",
                lambda: render_ok(
                    source_root=source_root,
                    dest_root=dest_root,
                    pbf="../new-york-latest.osm.pbf",
                    destination="esc-pbf/buffalo-niagara.mbtiles",
                    sidecar="esc-pbf/render.json",
                ),
                "path substitution",
            )
            expect_refusal(
                "path-escape-geometry",
                lambda: render_ok(
                    source_root=source_root,
                    dest_root=dest_root,
                    geometry="../tl_2024_us_county.zip",
                    destination="esc-geo/buffalo-niagara.mbtiles",
                    sidecar="esc-geo/render.json",
                ),
                "path substitution",
            )
            (dest_root / "esc-side").mkdir()
            expect_refusal(
                "sidecar-escape",
                lambda: render_ok(
                    source_root=source_root,
                    dest_root=dest_root,
                    destination="esc-side/buffalo-niagara.mbtiles",
                    sidecar="../escape.render.json",
                ),
                "path substitution",
            )
            assert not (root / "buffalo-niagara.mbtiles").exists()
            assert not (root / "escape.render.json").exists()

            expect_refusal(
                "wrong-name",
                lambda: render_ok(
                    source_root=source_root,
                    dest_root=dest_root,
                    destination="east-texas.mbtiles",
                    sidecar="east-texas.render.json",
                ),
                "path substitution",
            )
            assert not (dest_root / "east-texas.mbtiles").exists()

            expect_refusal(
                "overwrite-dest",
                lambda: render_ok(source_root=source_root, dest_root=dest_root),
                "no-replace",
            )
            assert mbtiles.read_bytes()
            (dest_root / "second").mkdir()
            expect_refusal(
                "overwrite-sidecar",
                lambda: render_ok(
                    source_root=source_root,
                    dest_root=dest_root,
                    destination="second/buffalo-niagara.mbtiles",
                    sidecar="buffalo-niagara.mbtiles.render.json",
                ),
                "no-replace",
            )
            assert not (dest_root / "second" / "buffalo-niagara.mbtiles").exists()

            expect_refusal(
                "clip-missing-niagara",
                lambda: render_ok(
                    source_root=write_sources(
                        root / "clip-erie",
                        geometry=b"TIGER-FIXTURE\n36029 Erie County\n",
                    ),
                    dest_root=(root / "clip-erie-dest").mkdir() or (root / "clip-erie-dest"),
                    sidecar="clip-erie.json",
                ),
                "clip",
            )
            expect_refusal(
                "clip-extra-county",
                lambda: render_ok(
                    source_root=write_sources(
                        root / "clip-extra",
                        geometry=b"TIGER-FIXTURE\n36029 Erie County\n36061 New York County\n36063 Niagara County\n",
                    ),
                    dest_root=(root / "clip-extra-dest").mkdir() or (root / "clip-extra-dest"),
                    sidecar="clip-extra.json",
                ),
                "clip",
            )
            expect_refusal(
                "clip-wrong-json",
                lambda: render_ok(
                    source_root=write_sources(
                        root / "clip-json",
                        geometry=json.dumps({"select_geoid": ["36061"]}).encode("ascii"),
                    ),
                    dest_root=(root / "clip-json-dest").mkdir() or (root / "clip-json-dest"),
                    sidecar="clip-json.json",
                ),
                "clip",
            )
            locked_clip = base_lock()
            locked_clip["geometry"]["select_geoid"] = ["36061"]
            expect_refusal(
                "lock-clip-drift",
                lambda: render_ok(
                    source_root=source_root,
                    dest_root=(root / "lock-clip-dest").mkdir() or (root / "lock-clip-dest"),
                    sources_path=write_lock(root / "lock-clip.json", locked_clip),
                    sidecar="lock-clip.json",
                ),
                "clip",
            )

            tiles_lock = base_lock()
            tiles_lock["tiles"] = {"url": TILE_CDN, "upstream": "osm-public-tiles"}
            expect_refusal(
                "tiles-source-kind",
                lambda: render_ok(
                    source_root=source_root,
                    dest_root=(root / "tiles-dest").mkdir() or (root / "tiles-dest"),
                    sources_path=write_lock(root / "tiles-lock.json", tiles_lock),
                    sidecar="tiles.json",
                ),
                "source",
            )
            osm_lock = base_lock()
            osm_lock["pbf"]["upstream"] = "osm-public-tiles"
            expect_refusal(
                "wrong-pbf-kind",
                lambda: render_ok(
                    source_root=source_root,
                    dest_root=(root / "osm-dest").mkdir() or (root / "osm-dest"),
                    sources_path=write_lock(root / "osm-lock.json", osm_lock),
                    sidecar="osm.json",
                ),
                "source",
            )
            expect_refusal(
                "admit-tiles-kind",
                lambda: render.admit_locked_source_kind("tiles"),
                "source",
            )

            linked_root = root / "linked-root"
            linked_root.symlink_to(dest_root, target_is_directory=True)
            expect_refusal(
                "symlink-dest-root",
                lambda: render_ok(
                    source_root=source_root,
                    dest_root=linked_root,
                    destination="via-symlink/buffalo-niagara.mbtiles",
                    sidecar="via-symlink/render.json",
                ),
                "path substitution",
            )
            linked_source = root / "linked-source"
            linked_source.symlink_to(source_root, target_is_directory=True)
            expect_refusal(
                "symlink-source-root",
                lambda: render_ok(
                    source_root=linked_source,
                    dest_root=(root / "sym-src-dest").mkdir() or (root / "sym-src-dest"),
                    sidecar="sym-src.json",
                ),
                "path substitution",
            )

            env = os.environ.copy()
            env["PYTHONPATH"] = ""
            cli = subprocess.run(
                [
                    sys.executable,
                    str(RENDERER),
                    "--sources",
                    str(LOCK),
                    "--source-root",
                    str(source_root),
                    "--pbf",
                    "new-york-latest.osm.pbf",
                    "--geometry",
                    "tl_2024_us_county.zip",
                    "--dest-root",
                    str(dest_root),
                    "--destination",
                    "cli-wrong/buffalo-niagara.mbtiles",
                    "--sidecar",
                    "cli-wrong/render.json",
                    "--url",
                    WRONG_URL,
                ],
                text=True,
                capture_output=True,
                env=env,
            )
            if cli.returncode != render.EXIT_REFUSED:
                raise AssertionError(f"CLI wrong-URL exit drifted: {cli.returncode} {cli.stderr}")
            if "wrong url" not in cli.stderr.lower():
                raise AssertionError(f"CLI wrong-URL refusal drifted: {cli.stderr}")
            assert not (dest_root / "cli-wrong").exists()

            cdn_cli = subprocess.run(
                [
                    sys.executable,
                    str(RENDERER),
                    "--sources",
                    str(LOCK),
                    "--source-root",
                    str(source_root),
                    "--pbf",
                    "new-york-latest.osm.pbf",
                    "--geometry",
                    "tl_2024_us_county.zip",
                    "--dest-root",
                    str(dest_root),
                    "--destination",
                    "cli-cdn/buffalo-niagara.mbtiles",
                    "--sidecar",
                    "cli-cdn/render.json",
                    "--url",
                    TILE_CDN,
                ],
                text=True,
                capture_output=True,
                env=env,
            )
            if cdn_cli.returncode != render.EXIT_REFUSED or "tile" not in cdn_cli.stderr.lower():
                raise AssertionError(f"CLI tile-CDN refusal drifted: {cdn_cli.stderr}")

            escape_cli = subprocess.run(
                [
                    sys.executable,
                    str(RENDERER),
                    "--sources",
                    str(LOCK),
                    "--source-root",
                    str(source_root),
                    "--pbf",
                    "new-york-latest.osm.pbf",
                    "--geometry",
                    "tl_2024_us_county.zip",
                    "--dest-root",
                    str(dest_root),
                    "--destination",
                    "../cli-escape.mbtiles",
                    "--sidecar",
                    "cli-escape.json",
                ],
                text=True,
                capture_output=True,
                env=env,
            )
            if escape_cli.returncode != render.EXIT_REFUSED or "path substitution" not in escape_cli.stderr.lower():
                raise AssertionError(f"CLI path-escape refusal drifted: {escape_cli.stderr}")

            overwrite_cli = subprocess.run(
                [
                    sys.executable,
                    str(RENDERER),
                    "--sources",
                    str(LOCK),
                    "--source-root",
                    str(source_root),
                    "--pbf",
                    "new-york-latest.osm.pbf",
                    "--geometry",
                    "tl_2024_us_county.zip",
                    "--dest-root",
                    str(dest_root),
                    "--destination",
                    "buffalo-niagara.mbtiles",
                    "--sidecar",
                    "buffalo-niagara.mbtiles.render.json",
                ],
                text=True,
                capture_output=True,
                env=env,
            )
            if overwrite_cli.returncode != render.EXIT_REFUSED or "no-replace" not in overwrite_cli.stderr.lower():
                raise AssertionError(f"CLI overwrite refusal drifted: {overwrite_cli.stderr}")
            assert mbtiles.exists()

            happy_cli_dest = root / "cli-ok"
            happy_cli_dest.mkdir()
            happy_cli = subprocess.run(
                [
                    sys.executable,
                    str(RENDERER),
                    "--sources",
                    str(LOCK),
                    "--source-root",
                    str(source_root),
                    "--pbf",
                    "new-york-latest.osm.pbf",
                    "--geometry",
                    "tl_2024_us_county.zip",
                    "--dest-root",
                    str(happy_cli_dest),
                    "--destination",
                    "buffalo-niagara.mbtiles",
                    "--sidecar",
                    "buffalo-niagara.mbtiles.render.json",
                ],
                text=True,
                capture_output=True,
                env=env,
            )
            if happy_cli.returncode != 0:
                raise AssertionError(f"CLI happy-path drifted: {happy_cli.stderr}")
            cli_record = json.loads(happy_cli.stdout)
            assert cli_record["production_admitted"] is False
            assert cli_record["clip_geoids"] == ["36029", "36063"]
            assert (happy_cli_dest / "buffalo-niagara.mbtiles").is_file()
    finally:
        import urllib.request

        urllib.request.urlopen = original_urlopen  # type: ignore[assignment]

    print("maps local-render mbtiles hostile suite passed")


if __name__ == "__main__":
    main()
