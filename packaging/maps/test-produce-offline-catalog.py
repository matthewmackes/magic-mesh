#!/usr/bin/env python3
"""Hostile tests for the offline Maps catalog producer."""

import hashlib
import importlib.util
import json
import os
import subprocess
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("producer", HERE / "produce-offline-catalog.py")
producer = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(producer)


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def approval(tile_digest: str) -> dict:
    return {
        "schema": 1,
        "provider": "openstreetmap-derived",
        "attribution": "© OpenStreetMap contributors",
        "license": "ODbL-1.0",
        "source_revision": "1" * 40,
        "source_epoch": 1_800_000_000,
        "quota_bytes": 1024,
        "regions": [{
            "region_id": "east-texas", "revision": "2026.08", "bounds": {"west": -96.0, "south": 29.0, "east": -93.0, "north": 34.0},
            "min_zoom": 1, "max_zoom": 2, "expires_at_ms": 1_900_000_000_000,
            "tiles": [{"z": 1, "x": 0, "y": 0, "source": "approved/0.tile", "sha256": tile_digest}],
        }],
    }


def expect_refusal(root: Path, document: dict, label: str) -> None:
    path = root / f"{label}.json"
    path.write_text(json.dumps(document))
    path.chmod(0o444)
    try:
        producer.produce(path, root / "source", root / f"out-{label}")
    except producer.Refusal:
        return
    raise AssertionError(f"hostile case was accepted: {label}")


def main() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        source = root / "source" / "approved"
        source.mkdir(parents=True)
        tile = b"licensed exact tile bytes\n"
        (source / "0.tile").write_bytes(tile)
        (source / "0.tile").chmod(0o444)
        good = approval(sha(tile))
        approved = root / "approval.json"
        approved.write_text(json.dumps(good))
        approved.chmod(0o444)
        output = root / "bundle"
        producer.produce(approved, root / "source", output)
        manifest = json.loads((output / "manifest.json").read_text())
        catalog = (output / "catalog.json").read_bytes()
        index = json.loads((output / "payload/index.json").read_text())
        assert manifest["catalog_sha256"] == sha(catalog)
        assert manifest["provider"] == "openstreetmap-derived" and manifest["license"] == "ODbL-1.0"
        assert manifest["source_revision"] == "1" * 40 and manifest["quota_bytes"] == 1024
        entry = index["entries"][0]
        expected_path = output / "payload/east-texas/1/0" / f"0-{sha(tile)}.tile"
        assert expected_path.read_bytes() == tile
        assert entry["catalog_sha256"] == sha(catalog) and entry["sha256"] == sha(tile)
        assert stat_mode(output) == 0o555 and stat_mode(expected_path) == 0o444
        verifier = os.environ.get("MAPS_CATALOG_VERIFIER")
        if verifier:
            subprocess.run([verifier, str(output)], check=True)

        duplicate = approval(sha(tile)); duplicate["regions"][0]["tiles"].append(dict(duplicate["regions"][0]["tiles"][0]))
        expect_refusal(root, duplicate, "duplicate")
        overlap = approval(sha(tile)); overlap["regions"][0]["tiles"].append({"z": 1, "x": 1, "y": 0, "source": "approved/0.tile", "sha256": sha(tile)})
        expect_refusal(root, overlap, "overlap")
        traversal = approval(sha(tile)); traversal["regions"][0]["tiles"][0]["source"] = "../approved/0.tile"
        expect_refusal(root, traversal, "traversal")
        wrong = approval("0" * 64)
        expect_refusal(root, wrong, "digest")
        overquota = approval(sha(tile)); overquota["quota_bytes"] = 1
        expect_refusal(root, overquota, "quota")
        linked = root / "source/approved/linked.tile"; os.link(source / "0.tile", linked)
        hardlink = approval(sha(tile)); hardlink["regions"][0]["tiles"][0]["source"] = "approved/linked.tile"
        expect_refusal(root, hardlink, "hardlink")
        mutable = root / "source/approved/mutable.tile"; mutable.write_bytes(tile)
        mutable_doc = approval(sha(tile)); mutable_doc["regions"][0]["tiles"][0]["source"] = "approved/mutable.tile"
        expect_refusal(root, mutable_doc, "mutable")
        linked.unlink()
        symlink_dir = root / "source/linked-dir"; symlink_dir.symlink_to(source, target_is_directory=True)
        symlink_doc = approval(sha(tile)); symlink_doc["regions"][0]["tiles"][0]["source"] = "linked-dir/0.tile"
        expect_refusal(root, symlink_doc, "symlink-parent")
        try:
            producer.produce(approved, root / "source", output)
        except producer.Refusal:
            pass
        else:
            raise AssertionError("existing output was replaced")
    print("offline catalog producer hostile suite passed")


def stat_mode(path: Path) -> int:
    return path.stat().st_mode & 0o777


if __name__ == "__main__":
    main()
