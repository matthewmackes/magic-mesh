#!/usr/bin/env python3
"""Hostile tests for Maps dest-path MBTiles production inspect.

Inspect stays local. Tests never download Geofabrik and never hit a public
OSM tile CDN. Dest-inspect sidecars are not production Maps receipts and
never mark production_admitted. The known 12 KiB fixture digest/size is
refused. Default quota 65536 refuses a 167936 B dest. Destination must be
buffalo-niagara.mbtiles under a real buffalo-niagara parent.
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
INSPECTOR = HERE / "maps-inspect-dest-mbtiles.py"
VERIFIER = HERE / "maps-verify-mbtiles.py"
TILE_CDN = "https://tile.openstreetmap.org/0/0/0.png"
PNG = bytes.fromhex(
    "89504e470d0a1a0a0000000d4948445200000001000000010802000000907753de"
    "0000000c49444154789c63f8cfc00000000300010005fed42b0000000049454e44ae426082"
)
OFFICIAL_BOUNDS = "-79.312136,42.437997,-78.460416,43.634799"
OFFICIAL_PARSED = {
    "west": -79.312136,
    "south": 42.437997,
    "east": -78.460416,
    "north": 43.634799,
}
DEST_BYTES = 167936


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(module)
    return module


inspect = load("maps_inspect_dest_mbtiles", INSPECTOR)
verify = load("maps_verify_mbtiles", VERIFIER)


def network_get(url: str, *args, **kwargs):
    raise AssertionError(f"test must never download: {url}")


def expect_refusal(label: str, call, needle: str) -> None:
    try:
        call()
    except inspect.Refusal as error:
        text = str(error).lower()
        if needle not in text:
            raise AssertionError(f"{label} refusal message drifted: {error}") from error
        return
    raise AssertionError(f"hostile case was accepted: {label}")


def dest_parent(root: Path) -> Path:
    parent = root / "var" / "lib" / "mde" / "maps" / "buffalo-niagara"
    parent.mkdir(parents=True, exist_ok=True)
    return parent


def write_png_mbtiles(path: Path, *, extra_metadata: dict[str, str] | None = None) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        path.unlink()
    connection = sqlite3.connect(path)
    try:
        connection.execute("CREATE TABLE metadata (name TEXT, value TEXT)")
        connection.execute(
            "CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB)"
        )
        metadata = {
            "format": "png",
            "minzoom": "1",
            "maxzoom": "1",
            "bounds": OFFICIAL_BOUNDS,
            "provider": "openstreetmap-derived",
            "attribution": "© OpenStreetMap contributors",
            "license": "ODbL-1.0",
            "name": "buffalo-niagara",
            # One-tile SQLite lands on 12288 B, the fixture size. Grow the
            # file so happy-path inspect is not the fixture identity.
            "description": "dest-inspect-temp-png " + ("x" * 3900),
        }
        if extra_metadata:
            metadata.update(extra_metadata)
        for key, value in metadata.items():
            connection.execute("INSERT INTO metadata VALUES (?, ?)", (key, value))
        connection.execute("INSERT INTO tiles VALUES (?, ?, ?, ?)", (1, 0, 1, PNG))
        connection.commit()
    finally:
        connection.close()
    path.chmod(0o400)
    return path


def write_sized(path: Path, size: int, fill: bytes = b"\x00") -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(fill * size if len(fill) == 1 else (fill * ((size // len(fill)) + 1))[:size])
    path.chmod(0o400)
    return path


def run_cli(args: list[str], ok: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        [sys.executable, str(INSPECTOR), *args],
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
        assert inspect.INSPECT_KIND == "mcnf-maps-dest-inspect"
        assert inspect.INSPECT_KIND != inspect.PRODUCTION_RECEIPT_KIND
        assert inspect.INSPECT_KIND != inspect.DEST_INSTALL_KIND
        assert inspect.FIXTURE_BYTES == 12288
        assert inspect.FIXTURE_SHA256 == "dd7cde7e116cb52f114fc1c886fec32618bdfcb8c82a16e3e45dae601c87046e"
        assert inspect.DEFAULT_QUOTA_BYTES == 65_536
        assert inspect.DEST_ADMIT_QUOTA_BYTES >= DEST_BYTES
        assert inspect.MBTILES_NAME == "buffalo-niagara.mbtiles"
        assert inspect.CANONICAL_INSTALL_PATH.endswith("/buffalo-niagara/buffalo-niagara.mbtiles")

        expect_refusal(
            "fixture-digest",
            lambda: inspect.refuse_fixture_identity(inspect.FIXTURE_SHA256, DEST_BYTES),
            "fixture",
        )
        expect_refusal(
            "fixture-size",
            lambda: inspect.refuse_fixture_identity("ab" * 32, inspect.FIXTURE_BYTES),
            "fixture",
        )

        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            parent = dest_parent(root)
            dest = write_sized(parent / "buffalo-niagara.mbtiles", inspect.FIXTURE_BYTES)
            expect_refusal(
                "fixture-size-inspect",
                lambda: inspect.inspect_dest_mbtiles(destination=dest),
                "fixture",
            )
            assert dest.stat().st_size == inspect.FIXTURE_BYTES
            assert not dest.with_name("buffalo-niagara.mbtiles.inspect.json").exists()

        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            parent = dest_parent(root)
            dest = write_sized(parent / "buffalo-niagara.mbtiles", DEST_BYTES)
            expect_refusal(
                "quota-default",
                lambda: inspect.inspect_dest_mbtiles(destination=dest),
                "quota",
            )
            expect_refusal(
                "quota-65536",
                lambda: inspect.inspect_dest_mbtiles(
                    destination=dest,
                    quota_bytes=inspect.DEFAULT_QUOTA_BYTES,
                ),
                "quota",
            )
            assert dest.stat().st_size == DEST_BYTES
            assert not dest.with_name("buffalo-niagara.mbtiles.inspect.json").exists()

        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            parent = dest_parent(root)
            dest = write_png_mbtiles(parent / "buffalo-niagara.mbtiles")
            sidecar = parent / "buffalo-niagara.mbtiles.inspect.json"
            record = inspect.inspect_dest_mbtiles(
                destination=dest,
                quota_bytes=inspect.DEST_ADMIT_QUOTA_BYTES,
            )
            assert record["kind"] == "mcnf-maps-dest-inspect"
            assert record["kind"] != "mcnf-maps-mbtiles-receipt"
            assert record["kind"] != "mcnf-maps-dest-install"
            assert record["production_admitted"] is False
            assert record["destination"] == str(dest)
            assert record["quota_bytes"] == inspect.DEST_ADMIT_QUOTA_BYTES
            assert record["bounds"] == OFFICIAL_PARSED
            assert record["mbtiles_bytes"] == dest.stat().st_size
            assert record["mbtiles_bytes"] != inspect.FIXTURE_BYTES
            assert record["mbtiles_sha256"] != inspect.FIXTURE_SHA256
            assert record["tile_count"] == 1
            assert record["provider"] == "openstreetmap-derived"
            assert record["license"] == "ODbL-1.0"
            assert dest.read_bytes()
            assert stat.S_IMODE(dest.stat().st_mode) == 0o400
            assert not dest.is_symlink()
            assert sidecar.is_file()
            assert not sidecar.is_symlink()
            assert stat.S_IMODE(sidecar.stat().st_mode) == 0o400
            loaded = json.loads(sidecar.read_bytes())
            assert loaded["kind"] == "mcnf-maps-dest-inspect"
            assert loaded["production_admitted"] is False
            assert loaded["mbtiles_sha256"] == record["mbtiles_sha256"]
            assert sidecar.read_bytes() == inspect.canonical(record)
            inspected = verify.inspect_mbtiles(dest, inspect.DEST_ADMIT_QUOTA_BYTES)
            assert inspected["mbtiles_sha256"] == record["mbtiles_sha256"]
            assert "production_admitted" not in inspected

            expect_refusal(
                "sidecar-no-replace",
                lambda: inspect.inspect_dest_mbtiles(
                    destination=dest,
                    quota_bytes=inspect.DEST_ADMIT_QUOTA_BYTES,
                ),
                "already exists",
            )
            assert sidecar.read_bytes() == inspect.canonical(record)

            cli_parent = dest_parent(root / "cli")
            cli_dest = write_png_mbtiles(cli_parent / "buffalo-niagara.mbtiles")
            result = run_cli(
                [
                    "--destination",
                    str(cli_dest),
                    "--quota-bytes",
                    str(inspect.DEST_ADMIT_QUOTA_BYTES),
                ]
            )
            cli_record = json.loads(result.stdout)
            assert cli_record["production_admitted"] is False
            assert cli_record["kind"] == "mcnf-maps-dest-inspect"
            assert cli_record["quota_bytes"] == inspect.DEST_ADMIT_QUOTA_BYTES
            cli_sidecar = cli_parent / "buffalo-niagara.mbtiles.inspect.json"
            assert cli_sidecar.is_file()
            assert stat.S_IMODE(cli_sidecar.stat().st_mode) == 0o400

            dest_root = root / "dest-root"
            dest_root.mkdir()
            root_sidecar_name = "buffalo-niagara.dest-inspect.json"
            dest_root_parent = dest_parent(root / "root-sidecar")
            dest_root_dest = write_png_mbtiles(dest_root_parent / "buffalo-niagara.mbtiles")
            root_record = inspect.inspect_dest_mbtiles(
                destination=dest_root_dest,
                dest_root=dest_root,
                sidecar=root_sidecar_name,
                quota_bytes=inspect.DEST_ADMIT_QUOTA_BYTES,
            )
            assert root_record["production_admitted"] is False
            written = dest_root / root_sidecar_name
            assert written.is_file()
            assert stat.S_IMODE(written.stat().st_mode) == 0o400

            install_sidecar = parent / inspect.DEST_INSTALL_SIDECAR_NAME
            expect_refusal(
                "dest-install-sidecar-name",
                lambda: inspect.inspect_dest_mbtiles(
                    destination=dest,
                    sidecar=str(install_sidecar),
                    quota_bytes=inspect.DEST_ADMIT_QUOTA_BYTES,
                ),
                "dest-install",
            )

            expect_refusal(
                "dest-filename",
                lambda: inspect.inspect_dest_mbtiles(
                    destination=parent / "other.mbtiles",
                    quota_bytes=inspect.DEST_ADMIT_QUOTA_BYTES,
                ),
                "dest filename",
            )
            expect_refusal(
                "path-escape",
                lambda: inspect.inspect_dest_mbtiles(
                    destination=parent / ".." / "buffalo-niagara" / "buffalo-niagara.mbtiles",
                    quota_bytes=inspect.DEST_ADMIT_QUOTA_BYTES,
                ),
                "path substitution",
            )
            cdn_parent = dest_parent(root / "cdn")
            cdn_dest = write_sized(
                cdn_parent / "buffalo-niagara.mbtiles",
                2048,
                fill=b"tile.openstreetmap.org/0/0/0.png\n",
            )
            expect_refusal(
                "tile-cdn",
                lambda: inspect.inspect_dest_mbtiles(
                    destination=cdn_dest,
                    quota_bytes=inspect.DEST_ADMIT_QUOTA_BYTES,
                ),
                "tile",
            )
            linked_parent = dest_parent(root / "linked")
            linked = linked_parent / "buffalo-niagara.mbtiles"
            linked.symlink_to(dest)
            expect_refusal(
                "symlink-dest",
                lambda: inspect.inspect_dest_mbtiles(
                    destination=linked,
                    quota_bytes=inspect.DEST_ADMIT_QUOTA_BYTES,
                ),
                "symlink",
            )
            missing_meta = dest_parent(root / "nometa")
            missing = write_png_mbtiles(
                missing_meta / "buffalo-niagara.mbtiles",
                extra_metadata={"provider": "not-the-approved-provider"},
            )
            expect_refusal(
                "inspect-provider",
                lambda: inspect.inspect_dest_mbtiles(
                    destination=missing,
                    quota_bytes=inspect.DEST_ADMIT_QUOTA_BYTES,
                ),
                "provider",
            )
            assert not (missing_meta / "buffalo-niagara.mbtiles.inspect.json").exists()
            _ = TILE_CDN
            _ = urlopen_orig
            _ = os
    finally:
        import urllib.request

        urllib.request.urlopen = urlopen_orig
    print("maps dest-inspect mbtiles hostile suite passed")


if __name__ == "__main__":
    main()
