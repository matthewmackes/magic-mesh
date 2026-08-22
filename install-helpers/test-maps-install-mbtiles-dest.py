#!/usr/bin/env python3
"""Hostile tests for Maps dest-path MBTiles install.

Copies stay local. Tests never download Geofabrik and never hit a public
OSM tile CDN. Dest-install sidecars are not production Maps receipts and
never mark production_admitted. The known 12 KiB fixture digest/size is
refused. Destination must be buffalo-niagara.mbtiles under a real
buffalo-niagara parent.
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
INSTALLER = HERE / "maps-install-mbtiles-dest.py"
TILE_CDN = "https://tile.openstreetmap.org/0/0/0.png"
SOURCE_NAME = "buffalo-niagara.pbf-raster.mbtiles"
RASTER_BYTES = b"OSM-DERIVED-RASTER-FIXTURE\n"


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(module)
    return module


install = load("maps_install_mbtiles_dest", INSTALLER)


def network_get(url: str, *args, **kwargs):
    raise AssertionError(f"test must never download: {url}")


def expect_refusal(label: str, call, needle: str) -> None:
    try:
        call()
    except install.Refusal as error:
        text = str(error).lower()
        if needle not in text:
            raise AssertionError(f"{label} refusal message drifted: {error}") from error
        return
    raise AssertionError(f"hostile case was accepted: {label}")


def write_source(root: Path, name: str = SOURCE_NAME, body: bytes = RASTER_BYTES) -> Path:
    dest_root = root / "dest-root"
    dest_root.mkdir(parents=True, exist_ok=True)
    path = dest_root / name
    path.write_bytes(body)
    path.chmod(0o400)
    return dest_root


def install_parent(root: Path) -> Path:
    parent = root / "var" / "lib" / "mde" / "maps" / "buffalo-niagara"
    parent.mkdir(parents=True, exist_ok=True)
    return parent


def run_cli(args: list[str], ok: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        [sys.executable, str(INSTALLER), *args],
        text=True,
        capture_output=True,
    )
    if ok != (result.returncode == 0):
        raise AssertionError(result.stderr or result.stdout)
    return result


def main() -> None:
    urlopen_orig = urlopen
    try:
        import urllib.request

        urllib.request.urlopen = network_get  # type: ignore[assignment]
        assert install.INSTALL_KIND == "mcnf-maps-dest-install"
        assert install.INSTALL_KIND != install.PRODUCTION_RECEIPT_KIND
        assert install.FIXTURE_BYTES == 12288
        assert install.FIXTURE_SHA256 == "dd7cde7e116cb52f114fc1c886fec32618bdfcb8c82a16e3e45dae601c87046e"
        assert install.MBTILES_NAME == "buffalo-niagara.mbtiles"
        assert install.CANONICAL_INSTALL_PATH.endswith("/buffalo-niagara/buffalo-niagara.mbtiles")

        expect_refusal(
            "fixture-digest",
            lambda: install.refuse_fixture_identity(install.FIXTURE_SHA256, 167936),
            "fixture",
        )
        expect_refusal(
            "fixture-size",
            lambda: install.refuse_fixture_identity("ab" * 32, install.FIXTURE_BYTES),
            "fixture",
        )

        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            dest_root = write_source(root, body=b"\x00" * install.FIXTURE_BYTES)
            parent = install_parent(root)
            dest = parent / "buffalo-niagara.mbtiles"
            expect_refusal(
                "fixture-digest-install",
                lambda: install.install_mbtiles_dest(
                    dest_root=dest_root,
                    source=SOURCE_NAME,
                    destination=dest,
                ),
                "fixture",
            )
            assert not dest.exists()
            assert not dest.with_name("buffalo-niagara.mbtiles.sha256.json").exists()
            assert (dest_root / SOURCE_NAME).stat().st_size == install.FIXTURE_BYTES

        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            dest_root = write_source(root)
            parent = install_parent(root)
            dest = parent / "buffalo-niagara.mbtiles"
            dest.write_bytes(b"ALREADY-PRESENT\n")
            dest.chmod(0o400)
            sidecar = parent / "buffalo-niagara.mbtiles.sha256.json"
            expect_refusal(
                "dest-exists",
                lambda: install.install_mbtiles_dest(
                    dest_root=dest_root,
                    source=SOURCE_NAME,
                    destination=dest,
                ),
                "already exists",
            )
            assert dest.read_bytes() == b"ALREADY-PRESENT\n"
            assert not sidecar.exists()

        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            dest_root = write_source(root)
            parent = install_parent(root)
            dest = parent / "buffalo-niagara.mbtiles"
            sidecar = parent / "buffalo-niagara.mbtiles.sha256.json"
            record = install.install_mbtiles_dest(
                dest_root=dest_root,
                source=SOURCE_NAME,
                destination=dest,
            )
            assert record["kind"] == "mcnf-maps-dest-install"
            assert record["kind"] != "mcnf-maps-mbtiles-receipt"
            assert record["production_admitted"] is False
            assert record["source"] == SOURCE_NAME
            assert record["destination"] == str(dest)
            assert record["mbtiles_sha256"] == install.digest(RASTER_BYTES)
            assert record["mbtiles_bytes"] == len(RASTER_BYTES)
            assert record["source_sha256"] == record["mbtiles_sha256"]
            assert dest.read_bytes() == RASTER_BYTES
            assert stat.S_IMODE(dest.stat().st_mode) == 0o400
            assert not dest.is_symlink()
            assert sidecar.is_file()
            assert not sidecar.is_symlink()
            assert stat.S_IMODE(sidecar.stat().st_mode) == 0o400
            loaded = json.loads(sidecar.read_bytes())
            assert loaded["kind"] == "mcnf-maps-dest-install"
            assert loaded["production_admitted"] is False
            assert loaded["mbtiles_sha256"] == record["mbtiles_sha256"]
            assert sidecar.read_bytes() == install.canonical(record)

            cli_root = root / "cli"
            cli_dest_root = write_source(cli_root)
            cli_parent = install_parent(cli_root)
            cli_dest = cli_parent / "buffalo-niagara.mbtiles"
            result = run_cli(
                [
                    "--dest-root",
                    str(cli_dest_root),
                    "--source",
                    SOURCE_NAME,
                    "--destination",
                    str(cli_dest),
                ]
            )
            cli_record = json.loads(result.stdout)
            assert cli_record["production_admitted"] is False
            assert cli_record["kind"] == "mcnf-maps-dest-install"
            assert cli_dest.read_bytes() == RASTER_BYTES
            assert stat.S_IMODE(cli_dest.stat().st_mode) == 0o400

            expect_refusal(
                "dest-filename",
                lambda: install.install_mbtiles_dest(
                    dest_root=dest_root,
                    source=SOURCE_NAME,
                    destination=parent / "other.mbtiles",
                ),
                "dest filename",
            )
            expect_refusal(
                "path-escape",
                lambda: install.install_mbtiles_dest(
                    dest_root=dest_root,
                    source="../buffalo-niagara.pbf-raster.mbtiles",
                    destination=parent / "second" / "buffalo-niagara" / "buffalo-niagara.mbtiles",
                ),
                "path substitution",
            )
            cdn_root = root / "cdn"
            cdn_root.mkdir()
            cdn_source = write_source(cdn_root, body=b"tile.openstreetmap.org/0/0/0.png\n")
            cdn_parent = install_parent(cdn_root)
            expect_refusal(
                "tile-cdn",
                lambda: install.install_mbtiles_dest(
                    dest_root=cdn_source,
                    source=SOURCE_NAME,
                    destination=cdn_parent / "buffalo-niagara.mbtiles",
                ),
                "tile",
            )
            linked = root / "linked-dest"
            linked_parent = install_parent(root / "linked")
            (linked_parent / "buffalo-niagara.mbtiles").symlink_to(dest)
            expect_refusal(
                "symlink-dest",
                lambda: install.install_mbtiles_dest(
                    dest_root=dest_root,
                    source=SOURCE_NAME,
                    destination=linked_parent / "buffalo-niagara.mbtiles",
                ),
                "symlink",
            )
            _ = TILE_CDN
            _ = urlopen_orig
            _ = os
    finally:
        import urllib.request

        urllib.request.urlopen = urlopen_orig
    print("maps dest-install mbtiles hostile suite passed")


if __name__ == "__main__":
    main()
