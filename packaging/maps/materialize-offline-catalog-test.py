#!/usr/bin/env python3
"""Hostile first-release assembly tests for the offline Maps materializer."""

import hashlib
import importlib.util
import json
import os
import shutil
import stat
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(module)
    return module


producer = load("maps_producer", HERE / "produce-offline-catalog.py")
materializer = load("maps_materializer", HERE / "materialize-offline-catalog.py")
REVISION = "1" * 40
EPOCH = 1_800_000_000
QUOTA = 1024


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def approval(tile: bytes) -> dict:
    return {
        "schema": 1,
        "provider": "openstreetmap-derived",
        "attribution": "© OpenStreetMap contributors",
        "license": "ODbL-1.0",
        "source_revision": REVISION,
        "source_epoch": EPOCH,
        "quota_bytes": QUOTA,
        "regions": [{
            "region_id": "east-texas", "revision": "2026.08",
            "bounds": {"west": -96.0, "south": 29.0, "east": -93.0, "north": 34.0},
            "min_zoom": 1, "max_zoom": 2, "expires_at_ms": 1_900_000_000_000,
            "tiles": [{"z": 1, "x": 0, "y": 0, "source": "tile.bin", "sha256": sha(tile)}],
        }],
    }


def make_bundle(root: Path) -> tuple[Path, bytes]:
    tile = b"governed first-release tile\n"
    source = root / "source"
    source.mkdir()
    (source / "tile.bin").write_bytes(tile)
    (source / "tile.bin").chmod(0o444)
    approval_path = root / "approval.json"
    approval_path.write_text(json.dumps(approval(tile)))
    approval_path.chmod(0o444)
    bundle = root / "bundle"
    producer.produce(approval_path, source, bundle)
    return bundle, tile


def verifier(root: Path) -> Path:
    production = os.environ.get("MAPS_MATERIALIZER_VERIFIER")
    if production:
        return Path(production)
    path = root / "verifier"
    path.write_text("#!/bin/sh\nexit 0\n")
    path.chmod(0o500)
    return path


def args(bundle: Path, output: Path, verify: Path):
    return type("Args", (), {
        "bundle": bundle, "cache_root": output, "verifier": verify,
        "source_revision": REVISION, "source_epoch": EPOCH, "quota_bytes": QUOTA,
    })()


def copy_bundle(source: Path, target: Path) -> None:
    shutil.copytree(source, target)
    for directory, _, files in os.walk(target, topdown=False):
        for filename in files:
            os.chmod(Path(directory) / filename, 0o444)
        os.chmod(directory, 0o555)


def expect_refusal(call, output: Path, label: str) -> None:
    try:
        call()
    except (materializer.Refusal, OSError):
        assert not output.exists(), f"{label} left a partial cache"
        return
    raise AssertionError(f"hostile case was accepted: {label}")


def main() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        bundle, tile = make_bundle(root)
        verify = verifier(root)
        output = root / "cache"
        materializer.materialize(args(bundle, output, verify))
        assert stat.S_IMODE(output.stat().st_mode) == 0o700
        assert (output / "catalog.json").read_bytes() == (bundle / "catalog.json").read_bytes()
        assert (output / "index.json").read_bytes() == (bundle / "payload/index.json").read_bytes()
        digest = sha(tile)
        assert (output / f"east-texas/1/0/0-{digest}.tile").read_bytes() == tile

        existing = root / "existing"
        existing.mkdir()
        marker = existing / "keep"
        marker.write_text("old authority")
        expect_refusal(lambda: materializer.materialize(args(bundle, existing, verify)), existing / "impossible", "replace")
        assert marker.read_text() == "old authority"

        for label, mutate in (
            ("revision", lambda manifest: manifest.__setitem__("source_revision", "2" * 40)),
            ("epoch", lambda manifest: manifest.__setitem__("source_epoch", EPOCH + 1)),
            ("quota", lambda manifest: manifest.__setitem__("quota_bytes", QUOTA + 1)),
            ("catalog-digest", lambda manifest: manifest.__setitem__("catalog_sha256", "0" * 64)),
        ):
            hostile = root / f"bundle-{label}"
            copy_bundle(bundle, hostile)
            manifest_path = hostile / "manifest.json"
            manifest = json.loads(manifest_path.read_text())
            mutate(manifest)
            manifest_path.chmod(0o644)
            manifest_path.write_text(json.dumps(manifest))
            manifest_path.chmod(0o444)
            destination = root / f"cache-{label}"
            expect_refusal(lambda h=hostile, d=destination: materializer.materialize(args(h, d, verify)), destination, label)

        writable = root / "bundle-writable"
        copy_bundle(bundle, writable)
        writable.chmod(0o755)
        destination = root / "cache-writable"
        expect_refusal(lambda: materializer.materialize(args(writable, destination, verify)), destination, "writable bundle")

        linked = root / "bundle-hardlink"
        copy_bundle(bundle, linked)
        linked_tile = next(linked.glob("payload/**/*.tile"))
        linked_tile.chmod(0o644)
        duplicate = root / "duplicate.tile"
        os.link(linked_tile, duplicate)
        linked_tile.chmod(0o444)
        destination = root / "cache-hardlink"
        expect_refusal(lambda: materializer.materialize(args(linked, destination, verify)), destination, "hardlink")

        symlinked = root / "bundle-symlink"
        copy_bundle(bundle, symlinked)
        tile_path = next(symlinked.glob("payload/**/*.tile"))
        original = root / "outside.tile"
        original.write_bytes(tile)
        original.chmod(0o444)
        tile_path.parent.chmod(0o755)
        tile_path.unlink()
        tile_path.symlink_to(original)
        tile_path.parent.chmod(0o555)
        destination = root / "cache-symlink"
        expect_refusal(lambda: materializer.materialize(args(symlinked, destination, verify)), destination, "symlink")

        rejected = root / "reject-verifier"
        rejected.write_text("#!/bin/sh\nexit 2\n")
        rejected.chmod(0o500)
        destination = root / "cache-verifier"
        expect_refusal(lambda: materializer.materialize(args(bundle, destination, rejected)), destination, "production verifier")
    print("offline Maps materializer hostile suite passed")


if __name__ == "__main__":
    main()
