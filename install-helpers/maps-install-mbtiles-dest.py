#!/usr/bin/env python3
"""Copy dest-root MBTiles onto the buffalo-niagara install dest.

Operator lock (2026-08-22): dest-root already holds the OSM-derived raster
`buffalo-niagara.pbf-raster.mbtiles`. This helper never fetches, never talks
to a public OSM tile CDN, and never marks production_admitted.

Source is a relative leaf under a real dest-root. Destination must be
exactly `buffalo-niagara.mbtiles` under a real parent named
`buffalo-niagara/`. Publication is no-replace, mode 0400. The known 12 KiB
fixture digest/size is refused. Sidecar kind is `mcnf-maps-dest-install`,
not a production MBTiles receipt.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import os
import stat
import sys
from collections.abc import Iterator
from pathlib import Path, PurePosixPath

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
INSTALL_KIND = "mcnf-maps-dest-install"
PRODUCTION_RECEIPT_KIND = "mcnf-maps-mbtiles-receipt"
REGION_ID = verify.REGION_ID
MBTILES_NAME = verify.MBTILES_NAME
CANONICAL_INSTALL_PATH = verify.CANONICAL_INSTALL_PATH
PARENT_NAME = REGION_ID
OPERATOR_AUTHORIZATION = fetch.OPERATOR_AUTHORIZATION
APPROVED_PROVIDER = verify.APPROVED_PROVIDER
APPROVED_LICENSE = verify.APPROVED_LICENSE
MAX_SIDECAR_BYTES = fetch.MAX_SIDECAR_BYTES
MAX_SOURCE_FILE_BYTES = 4 * 1024 * 1024 * 1024
FIXTURE_BYTES = 12288
FIXTURE_SHA256 = "dd7cde7e116cb52f114fc1c886fec32618bdfcb8c82a16e3e45dae601c87046e"
SIDECAR_KEYS = {
    "schema_version",
    "kind",
    "region_id",
    "license",
    "provider",
    "operator_authorization",
    "source",
    "source_sha256",
    "source_bytes",
    "destination",
    "mbtiles_sha256",
    "mbtiles_bytes",
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


def hash_local_source(path: Path, label: str, maximum: int) -> tuple[str, int]:
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


def resolve_source(dest_root: Path, relative: str) -> tuple[PurePosixPath, Path]:
    refuse_tile_cdn_text(str(dest_root), "dest-root")
    refuse_tile_cdn_text(relative, "source")
    fetch.real_directory(dest_root, "dest-root")
    rel = fetch.relative_leaf(relative, "source")
    if rel.name == MBTILES_NAME:
        raise Refusal("fixture buffalo-niagara.mbtiles digest/size refused")
    path = fetch.resolve_beneath(dest_root, rel, "source")
    if not path.exists():
        raise Refusal("source is missing or inaccessible")
    return rel, path


def resolve_install_dest(destination: Path) -> Path:
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
    if destination.exists() or destination.is_symlink():
        if destination.is_symlink() or (destination.exists() and stat.S_ISLNK(destination.lstat().st_mode)):
            raise Refusal("path substitution refused: destination is a symlink")
        raise Refusal("destination already exists; publication is no-replace")
    return destination


def resolve_sidecar(dest_root: Path, destination: Path, sidecar: str | None) -> Path:
    if sidecar is None:
        path = destination.with_name(f"{destination.name}.sha256.json")
    else:
        refuse_tile_cdn_text(sidecar, "sidecar")
        candidate = Path(sidecar)
        if candidate.is_absolute():
            if "\\" in sidecar or any(part in ("", ".", "..") for part in candidate.parts[1:]):
                raise Refusal("path substitution refused: sidecar is not a safe absolute path")
            fetch.real_directory(candidate.parent, "sidecar parent")
            path = candidate
        else:
            rel = fetch.relative_leaf(sidecar, "sidecar")
            path = fetch.resolve_beneath(dest_root, rel, "sidecar")
    if path.exists() or path.is_symlink():
        if path.is_symlink() or (path.exists() and stat.S_ISLNK(path.lstat().st_mode)):
            raise Refusal("path substitution refused: sidecar is a symlink")
        raise Refusal("sidecar already exists; publication is no-replace")
    if path.name == MBTILES_NAME:
        raise Refusal("path substitution refused: sidecar filename is buffalo-niagara.mbtiles")
    return path


def stream_source(path: Path, size: int) -> Iterator[bytes]:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        remaining = size
        while remaining:
            chunk = os.read(descriptor, min(1024 * 1024, remaining))
            if not chunk:
                raise Refusal("source truncated while reading")
            remaining -= len(chunk)
            yield chunk
    finally:
        os.close(descriptor)


def bind_sidecar(
    *,
    source: str,
    source_sha256: str,
    source_bytes: int,
    destination: str,
    mbtiles_sha256: str,
    mbtiles_bytes: int,
) -> dict[str, object]:
    sidecar = {
        "schema_version": 1,
        "kind": INSTALL_KIND,
        "region_id": REGION_ID,
        "license": APPROVED_LICENSE,
        "provider": APPROVED_PROVIDER,
        "operator_authorization": OPERATOR_AUTHORIZATION,
        "source": source,
        "source_sha256": source_sha256,
        "source_bytes": source_bytes,
        "destination": destination,
        "mbtiles_sha256": mbtiles_sha256,
        "mbtiles_bytes": mbtiles_bytes,
        # Dest-path copy of dest-root raster is not a production Maps receipt
        # and never closes the production Maps gate.
        "production_admitted": False,
    }
    if sidecar["kind"] == PRODUCTION_RECEIPT_KIND:
        raise Refusal("dest-install sidecar must not be a production Maps receipt")
    if sidecar["kind"] != INSTALL_KIND:
        raise Refusal("dest-install sidecar kind is unsupported")
    if sidecar["production_admitted"] is not False:
        raise Refusal("dest-install sidecar must never mark production_admitted")
    exact_keys(sidecar, SIDECAR_KEYS, "dest-install sidecar")
    return sidecar


def install_mbtiles_dest(
    *,
    dest_root: Path,
    source: str,
    destination: Path,
    sidecar: str | None = None,
) -> dict[str, object]:
    source_rel, source_path = resolve_source(dest_root, source)
    dest_path = resolve_install_dest(destination)
    sidecar_path = resolve_sidecar(dest_root, dest_path, sidecar)
    if dest_path in {source_path, sidecar_path} or source_path == sidecar_path:
        raise Refusal("path substitution refused: destination collides with an input path")
    source_sha256, source_bytes = hash_local_source(source_path, "source", MAX_SOURCE_FILE_BYTES)
    published_sha256, published_bytes = fetch.atomic_write_stream(
        dest_path,
        stream_source(source_path, source_bytes),
        label="destination",
    )
    if published_sha256 != source_sha256 or published_bytes != source_bytes:
        raise Refusal("installed MBTiles bytes differ from the source digest")
    refuse_fixture_identity(published_sha256, published_bytes)
    record = bind_sidecar(
        source=str(source_rel),
        source_sha256=source_sha256,
        source_bytes=source_bytes,
        destination=str(dest_path),
        mbtiles_sha256=published_sha256,
        mbtiles_bytes=published_bytes,
    )
    body = canonical(record)
    if len(body) > MAX_SIDECAR_BYTES:
        raise Refusal("dest-install sidecar exceeds its bound")
    try:
        fetch.atomic_write_bytes(sidecar_path, body, label="sidecar")
    except Exception:
        if dest_path.exists() and dest_path != source_path:
            try:
                dest_path.unlink()
            except OSError:
                pass
        raise
    return record


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dest-root", type=Path, required=True)
    parser.add_argument(
        "--source",
        required=True,
        help="relative dest-root leaf; buffalo-niagara.mbtiles fixture is refused",
    )
    parser.add_argument(
        "--destination",
        type=Path,
        required=True,
        help="absolute .../buffalo-niagara/buffalo-niagara.mbtiles; no-replace",
    )
    parser.add_argument(
        "--sidecar",
        default=None,
        help="absolute path or dest-root relative leaf; default is dest + .sha256.json",
    )
    args = parser.parse_args()
    try:
        value = install_mbtiles_dest(
            dest_root=args.dest_root,
            source=args.source,
            destination=args.destination,
            sidecar=args.sidecar,
        )
    except (Refusal, OSError, UnicodeError, ValueError) as error:
        print(f"maps-install-mbtiles-dest: refusal: {error}", file=sys.stderr)
        return EXIT_REFUSED
    print(canonical(value).decode("ascii"), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
