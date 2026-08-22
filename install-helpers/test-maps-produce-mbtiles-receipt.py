#!/usr/bin/env python3
"""Hostile tests for Maps MBTiles provider, path, and quota refusal.

Contract fixtures here exercise fail-closed admission. They are not a
production buffalo-niagara.mbtiles and must not be presented as one.
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

HERE = Path(__file__).resolve().parent
PRODUCER = HERE / "maps-produce-mbtiles-receipt.py"
VERIFIER = HERE / "maps-verify-mbtiles.py"
REVISION = "1" * 40
EPOCH = 1_800_000_000
# One-tile SQLite fixtures land well under 64 KiB; 256 B is a quota breach.
QUOTA = 65_536
PNG = bytes.fromhex(
    "89504e470d0a1a0a0000000d4948445200000001000000010802000000907753de"
    "0000000c49444154789c63f8cfc00000000300010005fed42b0000000049454e44ae426082"
)


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(module)
    return module


producer = load("maps_produce_mbtiles_receipt", PRODUCER)
verify = producer.verify


def approval(**overrides) -> dict:
    document = {
        "schema": 1,
        "provider": "openstreetmap-derived",
        "attribution": "© OpenStreetMap contributors",
        "license": "ODbL-1.0",
        "source_revision": REVISION,
        "source_epoch": EPOCH,
        "quota_bytes": QUOTA,
        "region_id": "buffalo-niagara",
        "install_path": "/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles",
    }
    document.update(overrides)
    return document


def write_json(path: Path, document: dict) -> Path:
    path.write_text(json.dumps(document))
    path.chmod(0o444)
    return path


def write_mbtiles(
    path: Path,
    *,
    provider: str = "openstreetmap-derived",
    bounds: str = "-79.12,42.48,-78.50,43.30",
    name: str = "buffalo-niagara",
    tile_data: bytes = PNG,
    zoom: int = 1,
    column: int = 0,
    row: int = 1,
    extra_metadata: dict[str, str] | None = None,
) -> Path:
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
            "minzoom": str(zoom),
            "maxzoom": str(zoom),
            "bounds": bounds,
            "center": "-78.85,42.89,1",
            "provider": provider,
            "attribution": "© OpenStreetMap contributors",
            "license": "ODbL-1.0",
            "name": name,
        }
        if extra_metadata:
            metadata.update(extra_metadata)
        for key, value in metadata.items():
            connection.execute("INSERT INTO metadata VALUES (?, ?)", (key, value))
        connection.execute(
            "INSERT INTO tiles VALUES (?, ?, ?, ?)",
            (zoom, column, row, tile_data),
        )
        connection.commit()
    finally:
        connection.close()
    path.chmod(0o444)
    return path


def expect_refusal(label: str, call) -> None:
    try:
        call()
    except verify.Refusal as error:
        text = str(error).lower()
        if label == "provider" and "provider" not in text:
            raise AssertionError(f"{label} refusal message drifted: {error}") from error
        if label == "path" and "path substitution" not in text:
            raise AssertionError(f"{label} refusal message drifted: {error}") from error
        if label == "quota" and "quota" not in text:
            raise AssertionError(f"{label} refusal message drifted: {error}") from error
        return
    raise AssertionError(f"hostile case was accepted: {label}")


def run_cli(script: Path, args: list[str], ok: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        [sys.executable, str(script), *args],
        text=True,
        capture_output=True,
    )
    if ok != (result.returncode == 0):
        raise AssertionError(result.stderr or result.stdout)
    return result


def main() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        source = root / "source"
        mbtiles = write_mbtiles(source / "buffalo-niagara.mbtiles")
        approved = write_json(root / "approval.json", approval())
        output = root / "receipt.json"
        receipt = producer.produce(approved, source, "buffalo-niagara.mbtiles", output)
        assert receipt["production_admitted"] is False
        assert receipt["provider"] == "openstreetmap-derived"
        assert receipt["region_id"] == "buffalo-niagara"
        assert receipt["install_path"] == "/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles"
        assert stat.S_IMODE(output.stat().st_mode) == 0o400
        verified = verify.verify_receipt(output, mbtiles, REVISION, EPOCH, QUOTA)
        assert verified["mbtiles_sha256"] == receipt["mbtiles_sha256"]

        run_cli(
            PRODUCER,
            [
                "verify",
                "--receipt",
                str(output),
                "--source-root",
                str(source),
                "--mbtiles",
                "buffalo-niagara.mbtiles",
                "--source-revision",
                REVISION,
                "--source-epoch",
                str(EPOCH),
                "--quota-bytes",
                str(QUOTA),
            ],
        )
        run_cli(
            VERIFIER,
            [
                "--receipt",
                str(output),
                "--source-root",
                str(source),
                "--mbtiles",
                "buffalo-niagara.mbtiles",
                "--source-revision",
                REVISION,
                "--source-epoch",
                str(EPOCH),
                "--quota-bytes",
                str(QUOTA),
            ],
        )

        expect_refusal(
            "provider",
            lambda: producer.produce(
                write_json(root / "approval-mapbox.json", approval(provider="mapbox")),
                source,
                "buffalo-niagara.mbtiles",
                root / "receipt-mapbox.json",
            ),
        )
        wrong_provider = write_mbtiles(source / "wrong" / "buffalo-niagara.mbtiles", provider="osm-public-tiles")
        expect_refusal(
            "provider",
            lambda: producer.produce(
                approved, source / "wrong", "buffalo-niagara.mbtiles", root / "receipt-osm-public.json"
            ),
        )
        assert not (root / "receipt-osm-public.json").exists()
        write_mbtiles(
            source / "cdn" / "buffalo-niagara.mbtiles",
            extra_metadata={"source": "https://tile.openstreetmap.org/{z}/{x}/{y}.png"},
        )
        expect_refusal(
            "provider",
            lambda: producer.produce(
                approved, source / "cdn", "buffalo-niagara.mbtiles", root / "receipt-cdn.json"
            ),
        )

        expect_refusal(
            "path",
            lambda: producer.produce(
                write_json(
                    root / "approval-path.json",
                    approval(install_path="/tmp/substituted/buffalo-niagara.mbtiles"),
                ),
                source,
                "buffalo-niagara.mbtiles",
                root / "receipt-path.json",
            ),
        )
        expect_refusal(
            "path",
            lambda: producer.produce(
                write_json(root / "approval-region.json", approval(region_id="east-texas")),
                source,
                "buffalo-niagara.mbtiles",
                root / "receipt-region.json",
            ),
        )
        expect_refusal(
            "path",
            lambda: producer.produce(approved, source, "../buffalo-niagara.mbtiles", root / "receipt-escape.json"),
        )
        linked = root / "linked-mbtiles"
        linked.symlink_to(mbtiles)
        linked_root = root / "linked-root"
        linked_root.mkdir()
        os.symlink(mbtiles, linked_root / "buffalo-niagara.mbtiles")
        expect_refusal(
            "path",
            lambda: producer.produce(
                approved, linked_root, "buffalo-niagara.mbtiles", root / "receipt-symlink.json"
            ),
        )
        symlink_dir = root / "linked-dir"
        symlink_dir.symlink_to(source, target_is_directory=True)
        expect_refusal(
            "path",
            lambda: producer.produce(
                approved, symlink_dir, "buffalo-niagara.mbtiles", root / "receipt-symlink-dir.json"
            ),
        )
        write_mbtiles(source / "east-texas.mbtiles", name="east-texas", bounds="-96.4,31.7,-95.3,32.6")
        expect_refusal(
            "path",
            lambda: producer.produce(approved, source, "east-texas.mbtiles", root / "receipt-name.json"),
        )

        expect_refusal(
            "quota",
            lambda: producer.produce(
                write_json(root / "approval-quota.json", approval(quota_bytes=256)),
                source,
                "buffalo-niagara.mbtiles",
                root / "receipt-quota.json",
            ),
        )
        assert not (root / "receipt-quota.json").exists()

        try:
            producer.produce(approved, source, "buffalo-niagara.mbtiles", output)
        except verify.Refusal:
            pass
        else:
            raise AssertionError("existing receipt was replaced")

        mutated = json.loads(output.read_text())
        mutated["mbtiles_sha256"] = "0" * 64
        changed = root / "changed-receipt.json"
        changed.write_text(json.dumps(mutated, sort_keys=True, separators=(",", ":")) + "\n")
        changed.chmod(0o444)
        try:
            verify.verify_receipt(changed, mbtiles, REVISION, EPOCH, QUOTA)
        except verify.Refusal as error:
            if "differ" not in str(error).lower() and "digest" not in str(error).lower():
                raise AssertionError(f"changed-byte refusal drifted: {error}") from error
        else:
            raise AssertionError("changed MBTiles digest was accepted")
        _ = wrong_provider
    print("maps mbtiles receipt hostile suite passed")


if __name__ == "__main__":
    main()
