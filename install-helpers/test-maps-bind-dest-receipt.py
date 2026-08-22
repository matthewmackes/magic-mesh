#!/usr/bin/env python3
"""Hostile tests for Maps dest-path candidate-bound receipt.

Bind stays local. Tests never download Geofabrik and never hit a public
OSM tile CDN. Receipts use bind_receipt / verify_receipt and never mark
production_admitted. The known 12 KiB fixture digest/size is refused.
Default quota 65536 refuses a 167936 B dest. Destination must be
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
BINDER = HERE / "maps-bind-dest-receipt.py"
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
REVISION = "1" * 40
EPOCH = 1_800_000_000


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader
    spec.loader.exec_module(module)
    return module


binder = load("maps_bind_dest_receipt", BINDER)
verify = load("maps_verify_mbtiles", VERIFIER)


def network_get(url: str, *args, **kwargs):
    raise AssertionError(f"test must never download: {url}")


def expect_refusal(label: str, call, needle: str) -> None:
    try:
        call()
    except binder.Refusal as error:
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
            # file so happy-path bind is not the fixture identity.
            "description": "dest-receipt-temp-png " + ("x" * 3900),
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


def write_approval(path: Path, *, quota: int = DEST_BYTES) -> Path:
    document = {
        "schema": 1,
        "provider": "openstreetmap-derived",
        "attribution": "© OpenStreetMap contributors",
        "license": "ODbL-1.0",
        "source_revision": REVISION,
        "source_epoch": EPOCH,
        "quota_bytes": quota,
        "region_id": "buffalo-niagara",
        "install_path": "/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles",
    }
    path.write_bytes(verify.canonical(document))
    path.chmod(0o400)
    return path


def init_git(repo: Path) -> tuple[str, int]:
    repo.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "config", "user.email", "maps@example.test"], check=True)
    subprocess.run(["git", "-C", str(repo), "config", "user.name", "Maps Bind"], check=True)
    (repo / "marker").write_text("dest-receipt\n")
    subprocess.run(["git", "-C", str(repo), "add", "marker"], check=True)
    subprocess.run(["git", "-C", str(repo), "commit", "-q", "-m", "temp dest receipt"], check=True)
    revision = subprocess.check_output(
        ["git", "-C", str(repo), "rev-parse", "--verify", "HEAD^{commit}"],
        text=True,
    ).strip()
    epoch = int(
        subprocess.check_output(
            ["git", "-C", str(repo), "show", "-s", "--format=%ct"],
            text=True,
        ).strip()
    )
    return revision, epoch


def run_cli(args: list[str], ok: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        [sys.executable, str(BINDER), *args],
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
        assert binder.PRODUCTION_RECEIPT_KIND == "mcnf-maps-mbtiles-receipt"
        assert binder.PRODUCTION_RECEIPT_KIND == verify.KIND
        assert binder.RECEIPT_SIDECAR_NAME == "buffalo-niagara.mbtiles.receipt.json"
        assert binder.RECEIPT_SIDECAR_NAME != binder.DEST_INSTALL_SIDECAR_NAME
        assert binder.RECEIPT_SIDECAR_NAME != binder.DEST_INSPECT_SIDECAR_NAME
        assert binder.FIXTURE_BYTES == 12288
        assert binder.FIXTURE_SHA256 == "dd7cde7e116cb52f114fc1c886fec32618bdfcb8c82a16e3e45dae601c87046e"
        assert binder.DEFAULT_QUOTA_BYTES == 65_536
        assert binder.DEST_ADMIT_QUOTA_BYTES >= DEST_BYTES
        assert binder.MBTILES_NAME == "buffalo-niagara.mbtiles"
        assert binder.CANONICAL_INSTALL_PATH.endswith("/buffalo-niagara/buffalo-niagara.mbtiles")
        assert set(verify.APPROVAL_KEYS) == {
            "schema",
            "provider",
            "attribution",
            "license",
            "source_revision",
            "source_epoch",
            "quota_bytes",
            "region_id",
            "install_path",
        }

        expect_refusal(
            "fixture-digest",
            lambda: binder.refuse_fixture_identity(binder.FIXTURE_SHA256, DEST_BYTES),
            "fixture",
        )
        expect_refusal(
            "fixture-size",
            lambda: binder.refuse_fixture_identity("ab" * 32, binder.FIXTURE_BYTES),
            "fixture",
        )

        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            parent = dest_parent(root)
            dest = write_sized(parent / "buffalo-niagara.mbtiles", binder.FIXTURE_BYTES)
            expect_refusal(
                "fixture-size-bind",
                lambda: binder.bind_dest_receipt(destination=dest),
                "fixture",
            )
            assert dest.stat().st_size == binder.FIXTURE_BYTES
            assert not dest.with_name(binder.RECEIPT_SIDECAR_NAME).exists()
            assert not dest.with_name(binder.APPROVAL_SIDECAR_NAME).exists()

        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            parent = dest_parent(root)
            dest = write_sized(parent / "buffalo-niagara.mbtiles", DEST_BYTES)
            expect_refusal(
                "quota-default",
                lambda: binder.bind_dest_receipt(destination=dest),
                "quota",
            )
            expect_refusal(
                "quota-65536",
                lambda: binder.bind_dest_receipt(
                    destination=dest,
                    quota_bytes=binder.DEFAULT_QUOTA_BYTES,
                ),
                "quota",
            )
            assert dest.stat().st_size == DEST_BYTES
            assert not dest.with_name(binder.RECEIPT_SIDECAR_NAME).exists()

        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            parent = dest_parent(root)
            dest = write_png_mbtiles(parent / "buffalo-niagara.mbtiles")
            approved = write_approval(
                root / "approval.json",
                quota=binder.DEST_ADMIT_QUOTA_BYTES,
            )
            receipt_path = parent / binder.RECEIPT_SIDECAR_NAME
            record = binder.bind_dest_receipt(
                destination=dest,
                approval=str(approved),
                quota_bytes=binder.DEST_ADMIT_QUOTA_BYTES,
            )
            assert record["kind"] == "mcnf-maps-mbtiles-receipt"
            assert record["kind"] != binder.DEST_INSTALL_KIND
            assert record["kind"] != binder.DEST_INSPECT_KIND
            assert record["production_admitted"] is False
            assert record["quota_bytes"] == binder.DEST_ADMIT_QUOTA_BYTES
            assert record["bounds"] == OFFICIAL_PARSED
            assert record["payload_bytes"] == dest.stat().st_size
            assert record["payload_bytes"] != binder.FIXTURE_BYTES
            assert record["mbtiles_sha256"] != binder.FIXTURE_SHA256
            assert record["tile_count"] == 1
            assert record["provider"] == "openstreetmap-derived"
            assert record["license"] == "ODbL-1.0"
            assert record["region_id"] == "buffalo-niagara"
            assert record["install_path"] == binder.CANONICAL_INSTALL_PATH
            assert record["source_revision"] == REVISION
            assert record["source_epoch"] == EPOCH
            assert dest.read_bytes()
            assert stat.S_IMODE(dest.stat().st_mode) == 0o400
            assert not dest.is_symlink()
            assert receipt_path.is_file()
            assert not receipt_path.is_symlink()
            assert stat.S_IMODE(receipt_path.stat().st_mode) == 0o400
            loaded = json.loads(receipt_path.read_bytes())
            assert loaded["kind"] == "mcnf-maps-mbtiles-receipt"
            assert loaded["production_admitted"] is False
            assert loaded["mbtiles_sha256"] == record["mbtiles_sha256"]
            assert receipt_path.read_bytes() == binder.canonical(record)
            verified = verify.verify_receipt(
                receipt_path,
                dest,
                REVISION,
                EPOCH,
                binder.DEST_ADMIT_QUOTA_BYTES,
            )
            assert verified["production_admitted"] is False
            assert verified["mbtiles_sha256"] == record["mbtiles_sha256"]
            assert verified["kind"] == "mcnf-maps-mbtiles-receipt"
            assert not dest.with_name(binder.APPROVAL_SIDECAR_NAME).exists()

            expect_refusal(
                "receipt-no-replace",
                lambda: binder.bind_dest_receipt(
                    destination=dest,
                    approval=str(approved),
                    quota_bytes=binder.DEST_ADMIT_QUOTA_BYTES,
                ),
                "already exists",
            )
            assert receipt_path.read_bytes() == binder.canonical(record)

            claimed = dict(record)
            claimed["production_admitted"] = True
            expect_refusal(
                "claimed-production",
                lambda: binder.refuse_production_admitted(claimed, "bind_receipt"),
                "production_admitted",
            )

            original_bind = binder.verify.bind_receipt

            def claim_bind(approval_doc, inspected):
                bound = original_bind(approval_doc, inspected)
                bound["production_admitted"] = True
                return bound

            binder.verify.bind_receipt = claim_bind
            try:
                claim_parent = dest_parent(root / "claimed")
                claim_dest = write_png_mbtiles(claim_parent / "buffalo-niagara.mbtiles")
                claim_approval = write_approval(
                    root / "approval-claimed.json",
                    quota=binder.DEST_ADMIT_QUOTA_BYTES,
                )
                expect_refusal(
                    "bind-would-admit",
                    lambda: binder.bind_dest_receipt(
                        destination=claim_dest,
                        approval=str(claim_approval),
                        quota_bytes=binder.DEST_ADMIT_QUOTA_BYTES,
                    ),
                    "production_admitted",
                )
                assert not claim_dest.with_name(binder.RECEIPT_SIDECAR_NAME).exists()
            finally:
                binder.verify.bind_receipt = original_bind

            pinned_parent = dest_parent(root / "pinned")
            pinned_dest = write_png_mbtiles(pinned_parent / "buffalo-niagara.mbtiles")
            pinned_revision = "ab4a9d5546fe05da65338ff4d3355e70e7e2231a"
            pinned_epoch = 1787438581
            pinned_record = binder.bind_dest_receipt(
                destination=pinned_dest,
                quota_bytes=binder.DEST_ADMIT_QUOTA_BYTES,
                source_revision=pinned_revision,
                source_epoch=pinned_epoch,
            )
            assert pinned_record["production_admitted"] is False
            assert pinned_record["source_revision"] == pinned_revision
            assert pinned_record["source_epoch"] == pinned_epoch
            pinned_verified = verify.verify_receipt(
                pinned_parent / binder.RECEIPT_SIDECAR_NAME,
                pinned_dest,
                pinned_revision,
                pinned_epoch,
                binder.DEST_ADMIT_QUOTA_BYTES,
            )
            assert pinned_verified["production_admitted"] is False

            git_repo = root / "git"
            revision, epoch = init_git(git_repo)
            write_parent = dest_parent(root / "write")
            write_dest = write_png_mbtiles(write_parent / "buffalo-niagara.mbtiles")
            write_record = binder.bind_dest_receipt(
                destination=write_dest,
                quota_bytes=binder.DEST_ADMIT_QUOTA_BYTES,
                git_dir=git_repo,
            )
            assert write_record["production_admitted"] is False
            assert write_record["kind"] == "mcnf-maps-mbtiles-receipt"
            assert write_record["source_revision"] == revision
            assert write_record["source_epoch"] == epoch
            write_approval_path = write_parent / binder.APPROVAL_SIDECAR_NAME
            write_receipt_path = write_parent / binder.RECEIPT_SIDECAR_NAME
            assert write_approval_path.is_file()
            assert write_receipt_path.is_file()
            assert stat.S_IMODE(write_approval_path.stat().st_mode) == 0o400
            loaded_approval = json.loads(write_approval_path.read_bytes())
            assert set(loaded_approval) == verify.APPROVAL_KEYS
            assert loaded_approval["source_revision"] == revision
            assert loaded_approval["source_epoch"] == epoch
            assert loaded_approval["quota_bytes"] == binder.DEST_ADMIT_QUOTA_BYTES
            re_verified = verify.verify_receipt(
                write_receipt_path,
                write_dest,
                revision,
                epoch,
                binder.DEST_ADMIT_QUOTA_BYTES,
            )
            assert re_verified["production_admitted"] is False

            cli_parent = dest_parent(root / "cli")
            cli_dest = write_png_mbtiles(cli_parent / "buffalo-niagara.mbtiles")
            cli_approval = write_approval(
                root / "approval-cli.json",
                quota=binder.DEST_ADMIT_QUOTA_BYTES,
            )
            result = run_cli(
                [
                    "--destination",
                    str(cli_dest),
                    "--approval",
                    str(cli_approval),
                    "--quota-bytes",
                    str(binder.DEST_ADMIT_QUOTA_BYTES),
                ]
            )
            cli_record = json.loads(result.stdout)
            assert cli_record["production_admitted"] is False
            assert cli_record["kind"] == "mcnf-maps-mbtiles-receipt"
            assert cli_record["quota_bytes"] == binder.DEST_ADMIT_QUOTA_BYTES
            cli_sidecar = cli_parent / binder.RECEIPT_SIDECAR_NAME
            assert cli_sidecar.is_file()
            assert stat.S_IMODE(cli_sidecar.stat().st_mode) == 0o400

            dest_root = root / "dest-root"
            dest_root.mkdir()
            root_parent = dest_parent(root / "root-sidecar")
            root_dest = write_png_mbtiles(root_parent / "buffalo-niagara.mbtiles")
            root_approval = write_approval(
                dest_root / "existing-approval.json",
                quota=binder.DEST_ADMIT_QUOTA_BYTES,
            )
            root_record = binder.bind_dest_receipt(
                destination=root_dest,
                dest_root=dest_root,
                approval="existing-approval.json",
                receipt="buffalo-niagara.dest-receipt.json",
                quota_bytes=binder.DEST_ADMIT_QUOTA_BYTES,
            )
            assert root_record["production_admitted"] is False
            written = dest_root / "buffalo-niagara.dest-receipt.json"
            assert written.is_file()
            assert stat.S_IMODE(written.stat().st_mode) == 0o400
            _ = root_approval

            install_sidecar = parent / binder.DEST_INSTALL_SIDECAR_NAME
            expect_refusal(
                "dest-install-sidecar-name",
                lambda: binder.bind_dest_receipt(
                    destination=dest,
                    approval=str(approved),
                    receipt=str(install_sidecar),
                    quota_bytes=binder.DEST_ADMIT_QUOTA_BYTES,
                ),
                "dest-install",
            )
            inspect_sidecar = parent / binder.DEST_INSPECT_SIDECAR_NAME
            expect_refusal(
                "dest-inspect-sidecar-name",
                lambda: binder.bind_dest_receipt(
                    destination=dest,
                    approval=str(approved),
                    receipt=str(inspect_sidecar),
                    quota_bytes=binder.DEST_ADMIT_QUOTA_BYTES,
                ),
                "dest-inspect",
            )

            expect_refusal(
                "dest-filename",
                lambda: binder.bind_dest_receipt(
                    destination=parent / "other.mbtiles",
                    approval=str(approved),
                    quota_bytes=binder.DEST_ADMIT_QUOTA_BYTES,
                ),
                "dest filename",
            )
            expect_refusal(
                "path-escape",
                lambda: binder.bind_dest_receipt(
                    destination=parent / ".." / "buffalo-niagara" / "buffalo-niagara.mbtiles",
                    approval=str(approved),
                    quota_bytes=binder.DEST_ADMIT_QUOTA_BYTES,
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
                lambda: binder.bind_dest_receipt(
                    destination=cdn_dest,
                    approval=str(approved),
                    quota_bytes=binder.DEST_ADMIT_QUOTA_BYTES,
                ),
                "tile",
            )
            linked_parent = dest_parent(root / "linked")
            linked = linked_parent / "buffalo-niagara.mbtiles"
            linked.symlink_to(dest)
            expect_refusal(
                "symlink-dest",
                lambda: binder.bind_dest_receipt(
                    destination=linked,
                    approval=str(approved),
                    quota_bytes=binder.DEST_ADMIT_QUOTA_BYTES,
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
                lambda: binder.bind_dest_receipt(
                    destination=missing,
                    approval=str(approved),
                    quota_bytes=binder.DEST_ADMIT_QUOTA_BYTES,
                ),
                "provider",
            )
            assert not (missing_meta / binder.RECEIPT_SIDECAR_NAME).exists()
            _ = TILE_CDN
            _ = urlopen_orig
            _ = os
    finally:
        import urllib.request

        urllib.request.urlopen = urlopen_orig
    print("maps dest-receipt mbtiles hostile suite passed")


if __name__ == "__main__":
    main()
