#!/usr/bin/env python3
"""Inspect a canonical buffalo-niagara dest MBTiles with inspect_mbtiles.

Operator lock (2026-08-22): BigBoy dest already holds dest-root OSM-derived
raster bytes. This helper never fetches, never talks to a public OSM tile
CDN, and never marks production_admitted.

Destination must be an absolute real file named exactly
`buffalo-niagara.mbtiles` under a real parent named `buffalo-niagara/`.
The known 12 KiB fixture digest/size is refused. Default quota 65536
refuses the 167936 B dest; callers that admit dest must pass a quota
>= 167936 (DEST_ADMIT_QUOTA_BYTES). Sidecar kind is
`mcnf-maps-dest-inspect`, not a production MBTiles receipt. Publication
is no-replace and must not overwrite the dest-install sidecar.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import os
import sqlite3
import stat
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


fetch = _load("maps_fetch_authorized_sources", HERE / "maps-fetch-authorized-sources.py")
verify = _load("maps_verify_mbtiles", HERE / "maps-verify-mbtiles.py")

EXIT_REFUSED = fetch.EXIT_REFUSED
INSPECT_KIND = "mcnf-maps-dest-inspect"
PRODUCTION_RECEIPT_KIND = "mcnf-maps-mbtiles-receipt"
DEST_INSTALL_KIND = "mcnf-maps-dest-install"
DEST_INSTALL_SIDECAR_NAME = "buffalo-niagara.mbtiles.sha256.json"
REGION_ID = verify.REGION_ID
MBTILES_NAME = verify.MBTILES_NAME
CANONICAL_INSTALL_PATH = verify.CANONICAL_INSTALL_PATH
PARENT_NAME = REGION_ID
OPERATOR_AUTHORIZATION = fetch.OPERATOR_AUTHORIZATION
APPROVED_PROVIDER = verify.APPROVED_PROVIDER
APPROVED_LICENSE = verify.APPROVED_LICENSE
MAX_SIDECAR_BYTES = fetch.MAX_SIDECAR_BYTES
MAX_SOURCE_FILE_BYTES = 4 * 1024 * 1024 * 1024
DEFAULT_QUOTA_BYTES = 65_536
DEST_ADMIT_QUOTA_BYTES = 262_144
FIXTURE_BYTES = 12288
FIXTURE_SHA256 = "dd7cde7e116cb52f114fc1c886fec32618bdfcb8c82a16e3e45dae601c87046e"
SIDECAR_KEYS = {
    "schema_version",
    "kind",
    "region_id",
    "license",
    "provider",
    "attribution",
    "operator_authorization",
    "destination",
    "mbtiles_sha256",
    "mbtiles_bytes",
    "tile_bytes",
    "tile_count",
    "bounds",
    "min_zoom",
    "max_zoom",
    "quota_bytes",
    "production_admitted",
}

Refusal = fetch.Refusal


def canonical(value: object) -> bytes:
    return fetch.canonical(value)


def digest(data: bytes) -> str:
    return fetch.digest(data)


def exact_keys(value: object, expected: set[str], label: str) -> dict:
    return fetch.exact_keys(value, expected, label)


def refuse_tile_cdn_text(value: str, label: str) -> None:
    lowered = value.lower()
    markers = fetch.TILE_CDN_MARKERS + verify.FORBIDDEN_SOURCE_MARKERS
    if any(marker in lowered for marker in markers):
        raise Refusal(f"public OSM tile CDN refused: {label}")


def refuse_cdn_prefix(path: Path, label: str) -> None:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        prefix = os.read(descriptor, 4096)
    finally:
        os.close(descriptor)
    lowered = prefix.lower()
    if any(marker.encode("ascii") in lowered for marker in fetch.TILE_CDN_MARKERS):
        raise Refusal("public OSM tile CDN refused")


def refuse_fixture_identity(sha256: str, size: int) -> None:
    if size == FIXTURE_BYTES or sha256 == FIXTURE_SHA256:
        raise Refusal("fixture buffalo-niagara.mbtiles digest/size refused")


def admit_regular_file(path: Path, label: str, maximum: int) -> os.stat_result:
    try:
        before = path.lstat()
    except OSError as error:
        raise Refusal(f"{label} is missing or inaccessible") from error
    if stat.S_ISLNK(before.st_mode):
        raise Refusal(f"path substitution refused: {label} is a symlink")
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        raise Refusal(f"{label} must be a singly-linked regular file")
    if before.st_size <= 0 or before.st_size > maximum:
        raise Refusal(f"{label} size is outside its bound")
    return before


def hash_local_dest(path: Path, label: str, maximum: int) -> tuple[str, int]:
    before = admit_regular_file(path, label, maximum)
    refuse_cdn_prefix(path, label)
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    hasher = hashlib.sha256()
    size = 0
    try:
        remaining = before.st_size
        while remaining:
            chunk = os.read(descriptor, min(1024 * 1024, remaining))
            if not chunk:
                raise Refusal(f"{label} truncated while reading")
            hasher.update(chunk)
            size += len(chunk)
            remaining -= len(chunk)
    finally:
        os.close(descriptor)
    if size <= 0:
        raise Refusal(f"{label} is empty")
    refuse_fixture_identity(hasher.hexdigest(), size)
    return hasher.hexdigest(), size


def resolve_inspect_dest(destination: Path) -> Path:
    value = os.fspath(destination)
    refuse_tile_cdn_text(value, "destination")
    if not destination.is_absolute() or "\\" in value:
        raise Refusal("path substitution refused: destination is not a safe absolute path")
    parts = destination.parts
    if any(part in ("", ".", "..") for part in parts[1:]):
        raise Refusal("path substitution refused: destination escapes its parent")
    if destination.name != MBTILES_NAME:
        raise Refusal("path substitution refused: dest filename is not buffalo-niagara.mbtiles")
    parent = destination.parent
    if parent.name != PARENT_NAME:
        raise Refusal("path substitution refused: dest parent is not buffalo-niagara")
    fetch.real_directory(parent, "destination parent")
    if destination.is_symlink() or (
        destination.exists() and stat.S_ISLNK(destination.lstat().st_mode)
    ):
        raise Refusal("path substitution refused: destination is a symlink")
    if not destination.exists():
        raise Refusal("destination is missing or inaccessible")
    return destination


def resolve_sidecar(dest_root: Path | None, destination: Path, sidecar: str | None) -> Path:
    if sidecar is None:
        path = destination.with_name(f"{destination.name}.inspect.json")
    else:
        refuse_tile_cdn_text(sidecar, "sidecar")
        candidate = Path(sidecar)
        if candidate.is_absolute():
            if "\\" in sidecar or any(part in ("", ".", "..") for part in candidate.parts[1:]):
                raise Refusal("path substitution refused: sidecar is not a safe absolute path")
            fetch.real_directory(candidate.parent, "sidecar parent")
            path = candidate
        else:
            if dest_root is None:
                raise Refusal("path substitution refused: relative sidecar requires dest-root")
            refuse_tile_cdn_text(str(dest_root), "dest-root")
            fetch.real_directory(dest_root, "dest-root")
            rel = fetch.relative_leaf(sidecar, "sidecar")
            path = fetch.resolve_beneath(dest_root, rel, "sidecar")
    if path.exists() or path.is_symlink():
        if path.is_symlink() or (path.exists() and stat.S_ISLNK(path.lstat().st_mode)):
            raise Refusal("path substitution refused: sidecar is a symlink")
        raise Refusal("sidecar already exists; publication is no-replace")
    if path.name == MBTILES_NAME:
        raise Refusal("path substitution refused: sidecar filename is buffalo-niagara.mbtiles")
    if path.name == DEST_INSTALL_SIDECAR_NAME:
        raise Refusal("path substitution refused: dest-install sidecar is no-replace")
    return path


def bind_sidecar(
    *,
    destination: str,
    inspected: dict[str, object],
    quota_bytes: int,
) -> dict[str, object]:
    sidecar = {
        "schema_version": 1,
        "kind": INSPECT_KIND,
        "region_id": REGION_ID,
        "license": APPROVED_LICENSE,
        "provider": APPROVED_PROVIDER,
        "attribution": inspected["attribution"],
        "operator_authorization": OPERATOR_AUTHORIZATION,
        "destination": destination,
        "mbtiles_sha256": inspected["mbtiles_sha256"],
        "mbtiles_bytes": inspected["payload_bytes"],
        "tile_bytes": inspected["tile_bytes"],
        "tile_count": inspected["tile_count"],
        "bounds": inspected["bounds"],
        "min_zoom": inspected["min_zoom"],
        "max_zoom": inspected["max_zoom"],
        "quota_bytes": quota_bytes,
        # Dest inspect of dest-root raster is not a production Maps receipt
        # and never closes the production Maps gate.
        "production_admitted": False,
    }
    if sidecar["kind"] == PRODUCTION_RECEIPT_KIND:
        raise Refusal("dest-inspect sidecar must not be a production Maps receipt")
    if sidecar["kind"] == DEST_INSTALL_KIND:
        raise Refusal("dest-inspect sidecar must not be a dest-install sidecar")
    if sidecar["kind"] != INSPECT_KIND:
        raise Refusal("dest-inspect sidecar kind is unsupported")
    if sidecar["production_admitted"] is not False:
        raise Refusal("dest-inspect sidecar must never mark production_admitted")
    exact_keys(sidecar, SIDECAR_KEYS, "dest-inspect sidecar")
    return sidecar


def inspect_dest_mbtiles(
    *,
    destination: Path,
    dest_root: Path | None = None,
    sidecar: str | None = None,
    quota_bytes: int = DEFAULT_QUOTA_BYTES,
) -> dict[str, object]:
    if not isinstance(quota_bytes, int) or isinstance(quota_bytes, bool) or quota_bytes <= 0:
        raise Refusal("quota must be a positive integer")
    dest_path = resolve_inspect_dest(destination)
    sidecar_path = resolve_sidecar(dest_root, dest_path, sidecar)
    if dest_path == sidecar_path:
        raise Refusal("path substitution refused: destination collides with sidecar")
    sha256, size = hash_local_dest(dest_path, "destination", MAX_SOURCE_FILE_BYTES)
    refuse_fixture_identity(sha256, size)
    try:
        inspected = verify.inspect_mbtiles(dest_path, quota_bytes)
    except verify.Refusal as error:
        raise Refusal(str(error)) from error
    if inspected["mbtiles_sha256"] != sha256 or inspected["payload_bytes"] != size:
        raise Refusal("inspected MBTiles bytes differ from the dest digest")
    record = bind_sidecar(
        destination=str(dest_path),
        inspected=inspected,
        quota_bytes=quota_bytes,
    )
    body = canonical(record)
    if len(body) > MAX_SIDECAR_BYTES:
        raise Refusal("dest-inspect sidecar exceeds its bound")
    fetch.atomic_write_bytes(sidecar_path, body, label="sidecar")
    return record


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--destination",
        type=Path,
        required=True,
        help="absolute .../buffalo-niagara/buffalo-niagara.mbtiles; real file",
    )
    parser.add_argument(
        "--dest-root",
        type=Path,
        default=None,
        help="real dest-root; required only for a relative sidecar leaf",
    )
    parser.add_argument(
        "--sidecar",
        default=None,
        help="absolute path or dest-root relative leaf; default is dest + .inspect.json",
    )
    parser.add_argument(
        "--quota-bytes",
        type=int,
        default=DEFAULT_QUOTA_BYTES,
        help=f"inspect quota; default {DEFAULT_QUOTA_BYTES} refuses dest 167936 B",
    )
    args = parser.parse_args()
    try:
        value = inspect_dest_mbtiles(
            destination=args.destination,
            dest_root=args.dest_root,
            sidecar=args.sidecar,
            quota_bytes=args.quota_bytes,
        )
    except (Refusal, OSError, UnicodeError, ValueError, sqlite3.Error) as error:
        print(f"maps-inspect-dest-mbtiles: refusal: {error}", file=sys.stderr)
        return EXIT_REFUSED
    print(canonical(value).decode("ascii"), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
