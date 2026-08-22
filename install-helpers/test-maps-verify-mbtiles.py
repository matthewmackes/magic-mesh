#!/usr/bin/env python3
"""Hostile tests for Maps MBTiles envelope admission and refusal.

Official Erie/Niagara TIGER bbox must be admitted. Reversed, non-numeric,
tile-CDN, and still-escaping bounds must refuse. No network. Contract
fixtures never mark production_admitted.
"""

from __future__ import annotations

import importlib.util
import json
import sqlite3
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
PRODUCER = HERE / "maps-produce-mbtiles-receipt.py"
VERIFIER = HERE / "maps-verify-mbtiles.py"
REVISION = "1" * 40
EPOCH = 1_800_000_000
QUOTA = 65_536
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


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(module)
    return module


verify = load("maps_verify_mbtiles", VERIFIER)
producer = load("maps_produce_mbtiles_receipt", PRODUCER)


def expect_refusal(label: str, call, *needles: str) -> None:
    try:
        call()
    except verify.Refusal as error:
        text = str(error).lower()
        if needles and not any(needle in text for needle in needles):
            raise AssertionError(f"{label} refusal message drifted: {error}") from error
        return
    raise AssertionError(f"hostile case was accepted: {label}")


def write_json(path: Path, document: dict) -> Path:
    path.write_text(json.dumps(document))
    path.chmod(0o444)
    return path


def approval() -> dict:
    return {
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


def write_mbtiles(
    path: Path,
    *,
    bounds: str = OFFICIAL_BOUNDS,
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
            "minzoom": "1",
            "maxzoom": "1",
            "bounds": bounds,
            "provider": "openstreetmap-derived",
            "attribution": "© OpenStreetMap contributors",
            "license": "ODbL-1.0",
            "name": "buffalo-niagara",
        }
        if extra_metadata:
            metadata.update(extra_metadata)
        for key, value in metadata.items():
            connection.execute("INSERT INTO metadata VALUES (?, ?)", (key, value))
        connection.execute("INSERT INTO tiles VALUES (?, ?, ?, ?)", (1, 0, 1, PNG))
        connection.commit()
    finally:
        connection.close()
    path.chmod(0o444)
    return path


def main() -> None:
    admitted = verify.parse_bounds(OFFICIAL_BOUNDS)
    assert admitted == OFFICIAL_PARSED

    expect_refusal(
        "reversed-bounds",
        lambda: verify.parse_bounds("-78.460416,42.437997,-79.312136,43.634799"),
        "invalid",
    )
    expect_refusal(
        "reversed-lat",
        lambda: verify.parse_bounds("-79.312136,43.634799,-78.460416,42.437997"),
        "invalid",
    )
    expect_refusal(
        "non-numeric",
        lambda: verify.parse_bounds("-79.312136,south,-78.460416,43.634799"),
        "not numeric",
    )
    expect_refusal(
        "escape-west",
        lambda: verify.parse_bounds("-80,42.437997,-78.460416,43.634799"),
        "escape",
    )
    expect_refusal(
        "escape-north",
        lambda: verify.parse_bounds("-79.312136,42.437997,-78.460416,45"),
        "escape",
    )

    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        source = root / "source"
        mbtiles = write_mbtiles(source / "buffalo-niagara.mbtiles")
        inspected = verify.inspect_mbtiles(mbtiles, QUOTA)
        assert inspected["bounds"] == OFFICIAL_PARSED
        assert "production_admitted" not in inspected

        receipt = producer.produce(
            write_json(root / "approval.json", approval()),
            source,
            "buffalo-niagara.mbtiles",
            root / "receipt.json",
        )
        assert receipt["production_admitted"] is False
        assert receipt["bounds"] == OFFICIAL_PARSED
        verified = verify.verify_receipt(root / "receipt.json", mbtiles, REVISION, EPOCH, QUOTA)
        assert verified["production_admitted"] is False
        assert verified["bounds"] == OFFICIAL_PARSED

        cdn = write_mbtiles(
            source / "cdn" / "buffalo-niagara.mbtiles",
            extra_metadata={"source": "https://tile.openstreetmap.org/{z}/{x}/{y}.png"},
        )
        expect_refusal(
            "tile-cdn",
            lambda: verify.inspect_mbtiles(cdn, QUOTA),
            "provider",
        )
        expect_refusal(
            "tile-cdn-alt",
            lambda: verify.require_text(
                "https://tiles.openstreetmap.org/1/0/0.png",
                "MBTiles metadata source",
            ),
            "provider",
        )

    print("maps verify mbtiles envelope suite passed")


if __name__ == "__main__":
    main()
