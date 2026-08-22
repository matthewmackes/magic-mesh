#!/usr/bin/env python3
"""Hostile tests for the six-role open-source input inventory. No network."""

from __future__ import annotations

import json
import socket
import stat
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PRODUCER = ROOT / "install-helpers/produce-open-source-input-inventory.py"
FAMILIES = ("maps", "app-vm", "bootc", "browser-vm", "rpm", "ux-014")
MAPS_SHA256 = "6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895"
APP_RECEIPT = "aca7573bc"
BOOTC_RECEIPT = "479ec2b8c"
BROWSER_RECEIPT = "b30954e31"
BROWSER_DIGEST = "sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357"
RPM_FINGERPRINT = "06B1C27EA0E08A225155EB3314018AA1497DDC7C"


_original_socket = socket.socket


class NetworkGuard(Exception):
    pass


def _blocked_socket(*_args: object, **_kwargs: object) -> socket.socket:
    raise NetworkGuard("inventory tests must not use the network")


def invoke(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(PRODUCER), *args],
        text=True,
        capture_output=True,
    )


def write_inventory(path: Path, value: dict[str, object]) -> None:
    path.write_bytes((json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii"))
    path.chmod(0o400)


def main() -> None:
    socket.socket = _blocked_socket  # type: ignore[method-assign, assignment]
    with tempfile.TemporaryDirectory(prefix="open-source-inventory-test-") as raw:
        root = Path(raw)
        root.chmod(0o700)
        output = root / "inventory.json"
        result = invoke("produce", "--output", str(output))
        assert result.returncode == 0, result.stderr
        assert stat.S_IMODE(output.stat().st_mode) == 0o400
        document = json.loads(output.read_text(encoding="utf-8"))
        assert document["schema_version"] == 1
        assert document["kind"] == "mcnf-open-source-input-inventory"
        rows = document["families"]
        assert isinstance(rows, list)
        names = [row["family"] for row in rows]
        assert tuple(names) == FAMILIES
        assert set(names) == set(FAMILIES)
        families = {row["family"]: row for row in rows}
        assert families["maps"]["production_admitted"] is False
        assert families["maps"]["license"] == "ODbL-1.0"
        assert families["maps"]["dest_sha256"] == MAPS_SHA256
        assert families["maps"]["dest"] == "/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles"
        assert families["app-vm"]["receipt_revision"] == APP_RECEIPT
        assert families["app-vm"]["resolved_digest"].startswith("sha256:")
        assert families["bootc"]["receipt_revision"] == BOOTC_RECEIPT
        assert families["bootc"]["release_role"] == "all-roles"
        assert families["browser-vm"]["producer"] == "packaging/browser-vm/produce-base-image-receipt.py"
        assert families["browser-vm"]["receipt_revision"] == BROWSER_RECEIPT
        assert families["browser-vm"]["resolved_digest"] == BROWSER_DIGEST
        assert families["browser-vm"]["leftover"].startswith("private dest bound")
        assert families["rpm"]["signing_fingerprint"] == RPM_FINGERPRINT
        assert families["ux-014"]["package"] == "kiron"
        assert "cuttlefish" not in json.dumps(document).lower()
        assert "android" not in json.dumps(document).lower()
        assert "org.example.App" not in json.dumps(document)
        inspect = invoke("inspect", "--inventory", str(output))
        assert inspect.returncode == 0, inspect.stderr

        previous = output.read_bytes()
        again = invoke("produce", "--output", str(output))
        assert again.returncode == 2 and output.read_bytes() == previous

        for family in ("cuttlefish", "android", "Cuttlefish", "Android"):
            refused = invoke(
                "produce",
                "--family",
                family,
                "--output",
                str(root / f"{family}.json"),
            )
            assert refused.returncode == 2, family
            assert "Android/Cuttlefish" in refused.stderr, refused.stderr
            assert not (root / f"{family}.json").exists()

        fixture = invoke(
            "produce",
            "--catalog-ref",
            "org.example.App",
            "--output",
            str(root / "fixture-catalog.json"),
        )
        assert fixture.returncode == 2
        assert "fixture catalog ref" in fixture.stderr
        assert not (root / "fixture-catalog.json").exists()

        admitted = json.loads(previous)
        for row in admitted["families"]:
            if row["family"] == "maps":
                row["production_admitted"] = True
        admitted_path = root / "maps-admitted.json"
        write_inventory(admitted_path, admitted)
        admitted_result = invoke("inspect", "--inventory", str(admitted_path))
        assert admitted_result.returncode == 2
        assert "production_admitted must be false" in admitted_result.stderr

        extra = json.loads(previous)
        extra["families"].append({"family": "cuttlefish", "license": "Apache-2.0"})
        extra_path = root / "extra-cuttlefish.json"
        write_inventory(extra_path, extra)
        extra_result = invoke("inspect", "--inventory", str(extra_path))
        assert extra_result.returncode == 2
        assert "Android/Cuttlefish" in extra_result.stderr

        catalog = json.loads(previous)
        for row in catalog["families"]:
            if row["family"] == "app-vm":
                row["catalog_refs"] = ["org.example.App"]
        catalog_path = root / "catalog-ref.json"
        write_inventory(catalog_path, catalog)
        catalog_result = invoke("inspect", "--inventory", str(catalog_path))
        assert catalog_result.returncode == 2
        assert "fixture catalog ref" in catalog_result.stderr

        missing = json.loads(previous)
        missing["families"] = [row for row in missing["families"] if row["family"] != "maps"]
        missing_path = root / "missing-maps.json"
        write_inventory(missing_path, missing)
        missing_result = invoke("inspect", "--inventory", str(missing_path))
        assert missing_result.returncode == 2
        assert "six-role" in missing_result.stderr

        symlink = root / "inventory-link"
        symlink.symlink_to(output)
        linked = invoke("inspect", "--inventory", str(symlink))
        assert linked.returncode == 2

    socket.socket = _original_socket  # type: ignore[method-assign, assignment]
    print("open-source input inventory hostile self-test: PASS")


if __name__ == "__main__":
    main()
