#!/usr/bin/env python3
"""Hostile tests for the authorized Maps source fetcher.

Fixtures never hit the network. A mocked GET seam supplies tiny bytes.
Fetched sidecars are not production Maps receipts and never mark
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

HERE = Path(__file__).resolve().parent
FETCHER = HERE / "maps-fetch-authorized-sources.py"
LOCK = HERE / "maps-authorized-sources.json"
PBF_URL = "https://download.geofabrik.de/north-america/us/new-york-latest.osm.pbf"
TIGER_URL = "https://www2.census.gov/geo/tiger/TIGER2024/COUNTY/tl_2024_us_county.zip"
WRONG_URL = "https://download.geofabrik.de/europe/germany-latest.osm.pbf"
TILE_CDN = "https://tile.openstreetmap.org/0/0/0.png"
TILE_CDN_ALT = "https://tiles.openstreetmap.org/1/0/0.png"
TILE_OSM = "https://tile.osm.org/0/0/0.png"
FIXTURE_PBF = b"OSM-PBF-FIXTURE\n"
FIXTURE_ZIP = b"PK\x03\x04TIGER-FIXTURE\n"


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(module)
    return module


fetch = load("maps_fetch_authorized_sources", FETCHER)


def mock_get(expected: str, body: bytes):
    def getter(url: str) -> bytes:
        if url != expected:
            raise AssertionError(f"GET seam was invoked with unexpected URL: {url}")
        return body

    return getter


def network_get(url: str) -> bytes:
    raise AssertionError(f"test GET must never hit the network: {url}")


def expect_refusal(label: str, call, needle: str) -> None:
    try:
        call()
    except fetch.Refusal as error:
        text = str(error).lower()
        if needle not in text:
            raise AssertionError(f"{label} refusal message drifted: {error}") from error
        return
    raise AssertionError(f"hostile case was accepted: {label}")


def run_cli(args: list[str], ok: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        [sys.executable, str(FETCHER), *args],
        text=True,
        capture_output=True,
    )
    if ok != (result.returncode == 0):
        raise AssertionError(result.stderr or result.stdout)
    return result


def main() -> None:
    lock = json.loads(LOCK.read_text())
    assert lock["kind"] == "mcnf-maps-authorized-sources"
    assert lock["pbf"]["url"] == PBF_URL
    assert lock["geometry"]["url"] == TIGER_URL
    assert "tile.openstreetmap.org" in "".join(lock["never_fetch"])

    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        dest_root = root / "dest"
        dest_root.mkdir()

        record = fetch.fetch_authorized_source(
            sources_path=LOCK,
            source_id="pbf",
            dest_root=dest_root,
            destination="new-york-latest.osm.pbf",
            sidecar="new-york-latest.osm.pbf.sha256.json",
            get=mock_get(PBF_URL, FIXTURE_PBF),
        )
        dest = dest_root / "new-york-latest.osm.pbf"
        sidecar = dest_root / "new-york-latest.osm.pbf.sha256.json"
        assert dest.read_bytes() == FIXTURE_PBF
        assert stat.S_IMODE(dest.stat().st_mode) == 0o400
        assert stat.S_IMODE(sidecar.stat().st_mode) == 0o400
        assert record["kind"] == "mcnf-maps-authorized-source-fetch"
        assert record["kind"] != "mcnf-maps-mbtiles-receipt"
        assert record["production_admitted"] is False
        assert record["url"] == PBF_URL
        assert record["source_id"] == "pbf"
        assert record["sha256"] == fetch.digest(FIXTURE_PBF)
        assert record["bytes"] == len(FIXTURE_PBF)
        stored = json.loads(sidecar.read_bytes())
        assert stored == record
        assert stored["production_admitted"] is False
        assert "mbtiles_sha256" not in stored
        assert stored["kind"] != "mcnf-maps-mbtiles-receipt"

        geometry = fetch.fetch_authorized_source(
            sources_path=LOCK,
            source_id="geometry",
            dest_root=dest_root,
            destination="tl_2024_us_county.zip",
            sidecar="tl_2024_us_county.zip.sha256.json",
            url=TIGER_URL,
            get=mock_get(TIGER_URL, FIXTURE_ZIP),
        )
        assert geometry["url"] == TIGER_URL
        assert geometry["source_id"] == "geometry"
        assert geometry["production_admitted"] is False
        assert geometry["sha256"] == fetch.digest(FIXTURE_ZIP)
        assert (dest_root / "tl_2024_us_county.zip").read_bytes() == FIXTURE_ZIP

        expect_refusal(
            "wrong-url",
            lambda: fetch.fetch_authorized_source(
                sources_path=LOCK,
                source_id="pbf",
                dest_root=dest_root,
                destination="germany-latest.osm.pbf",
                sidecar="germany-latest.osm.pbf.sha256.json",
                url=WRONG_URL,
                get=network_get,
            ),
            "wrong url",
        )
        assert not (dest_root / "germany-latest.osm.pbf").exists()
        assert not (dest_root / "germany-latest.osm.pbf.sha256.json").exists()

        for label, banned in (
            ("tile-cdn", TILE_CDN),
            ("tile-cdn-plural", TILE_CDN_ALT),
            ("tile-osm", TILE_OSM),
        ):
            expect_refusal(
                label,
                lambda banned=banned: fetch.fetch_authorized_source(
                    sources_path=LOCK,
                    source_id="pbf",
                    dest_root=dest_root,
                    destination="cdn-tiles.bin",
                    sidecar="cdn-tiles.bin.sha256.json",
                    url=banned,
                    get=network_get,
                ),
                "tile",
            )
        assert not (dest_root / "cdn-tiles.bin").exists()

        expect_refusal(
            "path-escape",
            lambda: fetch.fetch_authorized_source(
                sources_path=LOCK,
                source_id="pbf",
                dest_root=dest_root,
                destination="../escape.osm.pbf",
                sidecar="escape.osm.pbf.sha256.json",
                get=network_get,
            ),
            "path substitution",
        )
        expect_refusal(
            "path-escape-absolute",
            lambda: fetch.fetch_authorized_source(
                sources_path=LOCK,
                source_id="pbf",
                dest_root=dest_root,
                destination="/tmp/escape.osm.pbf",
                sidecar="escape-abs.sha256.json",
                get=network_get,
            ),
            "path substitution",
        )
        expect_refusal(
            "sidecar-escape",
            lambda: fetch.fetch_authorized_source(
                sources_path=LOCK,
                source_id="pbf",
                dest_root=dest_root,
                destination="ok.osm.pbf",
                sidecar="../escape.sha256.json",
                get=network_get,
            ),
            "path substitution",
        )
        assert not (root / "escape.osm.pbf").exists()
        assert not (root / "escape.sha256.json").exists()

        expect_refusal(
            "overwrite-dest",
            lambda: fetch.fetch_authorized_source(
                sources_path=LOCK,
                source_id="pbf",
                dest_root=dest_root,
                destination="new-york-latest.osm.pbf",
                sidecar="new-york-latest.osm.pbf.sha256.json",
                get=network_get,
            ),
            "no-replace",
        )
        assert dest.read_bytes() == FIXTURE_PBF

        expect_refusal(
            "overwrite-sidecar",
            lambda: fetch.fetch_authorized_source(
                sources_path=LOCK,
                source_id="pbf",
                dest_root=dest_root,
                destination="second.osm.pbf",
                sidecar="new-york-latest.osm.pbf.sha256.json",
                get=network_get,
            ),
            "no-replace",
        )
        assert not (dest_root / "second.osm.pbf").exists()

        linked_root = root / "linked-root"
        linked_root.symlink_to(dest_root, target_is_directory=True)
        expect_refusal(
            "symlink-root",
            lambda: fetch.fetch_authorized_source(
                sources_path=LOCK,
                source_id="pbf",
                dest_root=linked_root,
                destination="via-symlink.osm.pbf",
                sidecar="via-symlink.sha256.json",
                get=network_get,
            ),
            "path substitution",
        )

        expect_refusal(
            "query-string",
            lambda: fetch.fetch_authorized_source(
                sources_path=LOCK,
                source_id="pbf",
                dest_root=dest_root,
                destination="query.osm.pbf",
                sidecar="query.sha256.json",
                url=PBF_URL + "?redirect=1",
                get=network_get,
            ),
            "wrong url",
        )
        expect_refusal(
            "http-scheme",
            lambda: fetch.fetch_authorized_source(
                sources_path=LOCK,
                source_id="pbf",
                dest_root=dest_root,
                destination="http.osm.pbf",
                sidecar="http.sha256.json",
                url="http://download.geofabrik.de/north-america/us/new-york-latest.osm.pbf",
                get=network_get,
            ),
            "https",
        )

        nested = dest_root / "nested"
        nested.mkdir()
        nested_record = fetch.fetch_authorized_source(
            sources_path=LOCK,
            source_id="pbf",
            dest_root=dest_root,
            destination="nested/new-york-latest.osm.pbf",
            sidecar="nested/new-york-latest.osm.pbf.sha256.json",
            get=mock_get(PBF_URL, FIXTURE_PBF),
        )
        assert nested_record["destination"] == "nested/new-york-latest.osm.pbf"
        assert stat.S_IMODE((nested / "new-york-latest.osm.pbf").stat().st_mode) == 0o400

        missing_parent = dest_root / "missing-parent"
        expect_refusal(
            "missing-parent",
            lambda: fetch.fetch_authorized_source(
                sources_path=LOCK,
                source_id="pbf",
                dest_root=dest_root,
                destination="missing-parent/new-york-latest.osm.pbf",
                sidecar="missing-parent/sidecar.json",
                get=network_get,
            ),
            "missing",
        )
        _ = missing_parent

        env = os.environ.copy()
        env["PYTHONPATH"] = ""
        cli = subprocess.run(
            [
                sys.executable,
                str(FETCHER),
                "--sources",
                str(LOCK),
                "--source",
                "pbf",
                "--url",
                WRONG_URL,
                "--dest-root",
                str(dest_root),
                "--destination",
                "cli-wrong.osm.pbf",
                "--sidecar",
                "cli-wrong.sha256.json",
            ],
            text=True,
            capture_output=True,
            env=env,
        )
        if cli.returncode != fetch.EXIT_REFUSED:
            raise AssertionError(f"CLI wrong-URL exit drifted: {cli.returncode} {cli.stderr}")
        if "wrong url" not in cli.stderr.lower():
            raise AssertionError(f"CLI wrong-URL refusal drifted: {cli.stderr}")
        assert not (dest_root / "cli-wrong.osm.pbf").exists()

        cdn_cli = subprocess.run(
            [
                sys.executable,
                str(FETCHER),
                "--sources",
                str(LOCK),
                "--source",
                "geometry",
                "--url",
                TILE_CDN,
                "--dest-root",
                str(dest_root),
                "--destination",
                "cli-cdn.zip",
                "--sidecar",
                "cli-cdn.sha256.json",
            ],
            text=True,
            capture_output=True,
            env=env,
        )
        if cdn_cli.returncode != fetch.EXIT_REFUSED or "tile" not in cdn_cli.stderr.lower():
            raise AssertionError(f"CLI tile-CDN refusal drifted: {cdn_cli.stderr}")

        escape_cli = subprocess.run(
            [
                sys.executable,
                str(FETCHER),
                "--sources",
                str(LOCK),
                "--source",
                "pbf",
                "--dest-root",
                str(dest_root),
                "--destination",
                "../cli-escape.osm.pbf",
                "--sidecar",
                "cli-escape.sha256.json",
            ],
            text=True,
            capture_output=True,
            env=env,
        )
        if escape_cli.returncode != fetch.EXIT_REFUSED or "path substitution" not in escape_cli.stderr.lower():
            raise AssertionError(f"CLI path-escape refusal drifted: {escape_cli.stderr}")

        overwrite_cli = subprocess.run(
            [
                sys.executable,
                str(FETCHER),
                "--sources",
                str(LOCK),
                "--source",
                "pbf",
                "--dest-root",
                str(dest_root),
                "--destination",
                "new-york-latest.osm.pbf",
                "--sidecar",
                "new-york-latest.osm.pbf.sha256.json",
            ],
            text=True,
            capture_output=True,
            env=env,
        )
        if overwrite_cli.returncode != fetch.EXIT_REFUSED or "no-replace" not in overwrite_cli.stderr.lower():
            raise AssertionError(f"CLI overwrite refusal drifted: {overwrite_cli.stderr}")
        assert dest.read_bytes() == FIXTURE_PBF

    print("maps authorized-source fetch hostile suite passed")


if __name__ == "__main__":
    main()
