#!/usr/bin/env python3
"""Bind a candidate dest MBTiles to a non-production Maps receipt.

Operator lock (2026-08-22): BigBoy dest already inspects. This helper
never fetches, never talks to a public OSM tile CDN, and never marks
production_admitted.

Destination must be an absolute real file named exactly
`buffalo-niagara.mbtiles` under a real parent named `buffalo-niagara/`.
The known 12 KiB fixture digest/size is refused. Default quota 65536
refuses the 167936 B dest; callers that admit dest must pass a quota
>= 167936 (DEST_ADMIT_QUOTA_BYTES). Receipt kind is
`mcnf-maps-mbtiles-receipt` from bind_receipt. Publication is
no-replace and must not overwrite dest-install or dest-inspect sidecars.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import os
import sqlite3
import stat
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_GIT_DIR = HERE.parent


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


fetch = _load("maps_fetch_authorized_sources", HERE / "maps-fetch-authorized-sources.py")
verify = _load("maps_verify_mbtiles", HERE / "maps-verify-mbtiles.py")

EXIT_REFUSED = fetch.EXIT_REFUSED
PRODUCTION_RECEIPT_KIND = verify.KIND
DEST_INSTALL_KIND = "mcnf-maps-dest-install"
DEST_INSPECT_KIND = "mcnf-maps-dest-inspect"
DEST_INSTALL_SIDECAR_NAME = "buffalo-niagara.mbtiles.sha256.json"
DEST_INSPECT_SIDECAR_NAME = "buffalo-niagara.mbtiles.inspect.json"
RECEIPT_SIDECAR_NAME = "buffalo-niagara.mbtiles.receipt.json"
APPROVAL_SIDECAR_NAME = "buffalo-niagara.mbtiles.approval.json"
REGION_ID = verify.REGION_ID
MBTILES_NAME = verify.MBTILES_NAME
CANONICAL_INSTALL_PATH = verify.CANONICAL_INSTALL_PATH
PARENT_NAME = REGION_ID
APPROVED_PROVIDER = verify.APPROVED_PROVIDER
APPROVED_LICENSE = verify.APPROVED_LICENSE
MAX_SIDECAR_BYTES = fetch.MAX_SIDECAR_BYTES
MAX_SOURCE_FILE_BYTES = 4 * 1024 * 1024 * 1024
DEFAULT_QUOTA_BYTES = 65_536
DEST_ADMIT_QUOTA_BYTES = 262_144
FIXTURE_BYTES = 12288
FIXTURE_SHA256 = "dd7cde7e116cb52f114fc1c886fec32618bdfcb8c82a16e3e45dae601c87046e"
FORBIDDEN_RECEIPT_NAMES = {
    MBTILES_NAME,
    DEST_INSTALL_SIDECAR_NAME,
    DEST_INSPECT_SIDECAR_NAME,
}
FORBIDDEN_APPROVAL_NAMES = FORBIDDEN_RECEIPT_NAMES | {RECEIPT_SIDECAR_NAME}

Refusal = fetch.Refusal


def canonical(value: object) -> bytes:
    return verify.canonical(value)


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


def resolve_dest(destination: Path) -> Path:
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


def resolve_output(
    dest_root: Path | None,
    destination: Path,
    requested: str | None,
    *,
    default_name: str,
    label: str,
    forbidden_names: set[str],
) -> Path:
    if requested is None:
        path = destination.with_name(default_name)
    else:
        refuse_tile_cdn_text(requested, label)
        candidate = Path(requested)
        if candidate.is_absolute():
            if "\\" in requested or any(part in ("", ".", "..") for part in candidate.parts[1:]):
                raise Refusal(f"path substitution refused: {label} is not a safe absolute path")
            fetch.real_directory(candidate.parent, f"{label} parent")
            path = candidate
        else:
            if dest_root is None:
                raise Refusal(f"path substitution refused: relative {label} requires dest-root")
            refuse_tile_cdn_text(str(dest_root), "dest-root")
            fetch.real_directory(dest_root, "dest-root")
            rel = fetch.relative_leaf(requested, label)
            path = fetch.resolve_beneath(dest_root, rel, label)
    if path.exists() or path.is_symlink():
        if path.is_symlink() or (path.exists() and stat.S_ISLNK(path.lstat().st_mode)):
            raise Refusal(f"path substitution refused: {label} is a symlink")
        raise Refusal(f"{label} already exists; publication is no-replace")
    if path.name in forbidden_names:
        if path.name == DEST_INSTALL_SIDECAR_NAME:
            raise Refusal("path substitution refused: dest-install sidecar is no-replace")
        if path.name == DEST_INSPECT_SIDECAR_NAME:
            raise Refusal("path substitution refused: dest-inspect sidecar is no-replace")
        if path.name == MBTILES_NAME:
            raise Refusal(f"path substitution refused: {label} filename is buffalo-niagara.mbtiles")
        raise Refusal(f"path substitution refused: {label} filename is reserved")
    return path


def resolve_existing_approval(dest_root: Path | None, destination: Path, approval: str) -> Path:
    refuse_tile_cdn_text(approval, "approval")
    candidate = Path(approval)
    if candidate.is_absolute():
        if "\\" in approval or any(part in ("", ".", "..") for part in candidate.parts[1:]):
            raise Refusal("path substitution refused: approval is not a safe absolute path")
        path = candidate
    else:
        if dest_root is None:
            raise Refusal("path substitution refused: relative approval requires dest-root")
        refuse_tile_cdn_text(str(dest_root), "dest-root")
        fetch.real_directory(dest_root, "dest-root")
        rel = fetch.relative_leaf(approval, "approval")
        path = fetch.resolve_beneath(dest_root, rel, "approval")
    if path.is_symlink() or (path.exists() and stat.S_ISLNK(path.lstat().st_mode)):
        raise Refusal("path substitution refused: approval is a symlink")
    if not path.exists():
        raise Refusal("approval is missing or inaccessible")
    if path.name == MBTILES_NAME:
        raise Refusal("path substitution refused: approval filename is buffalo-niagara.mbtiles")
    _ = destination
    return path


def git_identity(repo: Path) -> tuple[str, int]:
    refuse_tile_cdn_text(str(repo), "git-dir")
    fetch.real_directory(repo, "git-dir")
    try:
        revision = subprocess.check_output(
            ["git", "-C", str(repo), "rev-parse", "--verify", "HEAD^{commit}"],
            text=True,
        ).strip()
        epoch_raw = subprocess.check_output(
            ["git", "-C", str(repo), "show", "-s", "--format=%ct"],
            text=True,
        ).strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise Refusal(f"git identity is unavailable: {error}") from error
    try:
        verify.require_revision(revision)
    except verify.Refusal as error:
        raise Refusal(str(error)) from error
    try:
        epoch = int(epoch_raw)
    except ValueError as error:
        raise Refusal("git commit epoch is not an integer") from error
    try:
        verify.require_positive_int(epoch, "source epoch")
    except verify.Refusal as error:
        raise Refusal(str(error)) from error
    return revision, epoch


def compose_approval(
    *,
    inspected: dict[str, object],
    quota_bytes: int,
    source_revision: str,
    source_epoch: int,
) -> dict[str, object]:
    approval = {
        "schema": 1,
        "provider": APPROVED_PROVIDER,
        "attribution": inspected["attribution"],
        "license": APPROVED_LICENSE,
        "source_revision": source_revision,
        "source_epoch": source_epoch,
        "quota_bytes": quota_bytes,
        "region_id": REGION_ID,
        "install_path": CANONICAL_INSTALL_PATH,
    }
    exact_keys(approval, verify.APPROVAL_KEYS, "approval")
    return approval


def refuse_production_admitted(receipt: dict[str, object], label: str) -> None:
    if receipt.get("kind") != PRODUCTION_RECEIPT_KIND:
        raise Refusal(f"{label} kind is not {PRODUCTION_RECEIPT_KIND}")
    if receipt.get("production_admitted") is not False:
        raise Refusal(f"{label} must never mark production_admitted")


def resolve_write_identity(
    git_dir: Path,
    source_revision: str | None,
    source_epoch: int | None,
) -> tuple[str, int]:
    if source_revision is None and source_epoch is None:
        return git_identity(git_dir)
    if source_revision is None or source_epoch is None:
        raise Refusal("source revision and epoch must be supplied together")
    try:
        verify.require_revision(source_revision)
        verify.require_positive_int(source_epoch, "source epoch")
    except verify.Refusal as error:
        raise Refusal(str(error)) from error
    return source_revision, source_epoch


def bind_dest_receipt(
    *,
    destination: Path,
    dest_root: Path | None = None,
    approval: str | None = None,
    write_approval: str | None = None,
    receipt: str | None = None,
    quota_bytes: int = DEFAULT_QUOTA_BYTES,
    git_dir: Path = DEFAULT_GIT_DIR,
    source_revision: str | None = None,
    source_epoch: int | None = None,
) -> dict[str, object]:
    if approval is not None and write_approval is not None:
        raise Refusal("approval load and write-approval are mutually exclusive")
    if not isinstance(quota_bytes, int) or isinstance(quota_bytes, bool) or quota_bytes <= 0:
        raise Refusal("quota must be a positive integer")
    dest_path = resolve_dest(destination)
    receipt_path = resolve_output(
        dest_root,
        dest_path,
        receipt,
        default_name=RECEIPT_SIDECAR_NAME,
        label="receipt",
        forbidden_names=FORBIDDEN_RECEIPT_NAMES,
    )
    if dest_path == receipt_path:
        raise Refusal("path substitution refused: destination collides with receipt")
    sha256, size = hash_local_dest(dest_path, "destination", MAX_SOURCE_FILE_BYTES)
    refuse_fixture_identity(sha256, size)
    try:
        inspected = verify.inspect_mbtiles(dest_path, quota_bytes)
    except verify.Refusal as error:
        raise Refusal(str(error)) from error
    if inspected["mbtiles_sha256"] != sha256 or inspected["payload_bytes"] != size:
        raise Refusal("inspected MBTiles bytes differ from the dest digest")
    if approval is not None:
        approval_path = resolve_existing_approval(dest_root, dest_path, approval)
        try:
            loaded = verify.load_approval(approval_path)
        except verify.Refusal as error:
            raise Refusal(str(error)) from error
    else:
        source_revision, source_epoch = resolve_write_identity(
            git_dir, source_revision, source_epoch
        )
        composed = compose_approval(
            inspected=inspected,
            quota_bytes=quota_bytes,
            source_revision=source_revision,
            source_epoch=source_epoch,
        )
        approval_path = resolve_output(
            dest_root,
            dest_path,
            write_approval,
            default_name=APPROVAL_SIDECAR_NAME,
            label="approval",
            forbidden_names=FORBIDDEN_APPROVAL_NAMES,
        )
        if approval_path in {dest_path, receipt_path}:
            raise Refusal("path substitution refused: approval collides with dest or receipt")
        body = canonical(composed)
        if len(body) > MAX_SIDECAR_BYTES:
            raise Refusal("approval exceeds its bound")
        fetch.atomic_write_bytes(approval_path, body, label="approval")
        try:
            loaded = verify.load_approval(approval_path)
        except verify.Refusal as error:
            raise Refusal(str(error)) from error
    try:
        bound = verify.bind_receipt(loaded, inspected)
    except verify.Refusal as error:
        raise Refusal(str(error)) from error
    refuse_production_admitted(bound, "bind_receipt")
    body = canonical(bound)
    if len(body) > MAX_SIDECAR_BYTES:
        raise Refusal("receipt exceeds its bound")
    fetch.atomic_write_bytes(receipt_path, body, label="receipt")
    try:
        verified = verify.verify_receipt(
            receipt_path,
            dest_path,
            str(bound["source_revision"]),
            int(bound["source_epoch"]),
            int(bound["quota_bytes"]),
        )
    except verify.Refusal as error:
        raise Refusal(str(error)) from error
    refuse_production_admitted(verified, "verify_receipt")
    if verified["mbtiles_sha256"] != bound["mbtiles_sha256"]:
        raise Refusal("verified receipt digest differs from the bound receipt")
    return verified


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
        help="real dest-root; required only for a relative approval or receipt leaf",
    )
    parser.add_argument(
        "--approval",
        default=None,
        help="existing approval JSON with exact APPROVAL_KEYS; load only",
    )
    parser.add_argument(
        "--write-approval",
        default=None,
        help="no-replace approval path; default is dest + .approval.json",
    )
    parser.add_argument(
        "--receipt",
        default=None,
        help="absolute path or dest-root relative leaf; default is dest + .receipt.json",
    )
    parser.add_argument(
        "--quota-bytes",
        type=int,
        default=DEFAULT_QUOTA_BYTES,
        help=f"inspect quota; default {DEFAULT_QUOTA_BYTES} refuses dest 167936 B",
    )
    parser.add_argument(
        "--git-dir",
        type=Path,
        default=DEFAULT_GIT_DIR,
        help="git checkout whose HEAD and %%ct epoch bind a written approval",
    )
    parser.add_argument(
        "--source-revision",
        default=None,
        help="full 40-char HEAD when writing approval without a farm .git",
    )
    parser.add_argument(
        "--source-epoch",
        type=int,
        default=None,
        help="git show -s --format=%%ct epoch when writing approval without a farm .git",
    )
    args = parser.parse_args()
    try:
        value = bind_dest_receipt(
            destination=args.destination,
            dest_root=args.dest_root,
            approval=args.approval,
            write_approval=args.write_approval,
            receipt=args.receipt,
            quota_bytes=args.quota_bytes,
            git_dir=args.git_dir,
            source_revision=args.source_revision,
            source_epoch=args.source_epoch,
        )
    except (Refusal, OSError, UnicodeError, ValueError, sqlite3.Error) as error:
        print(f"maps-bind-dest-receipt: refusal: {error}", file=sys.stderr)
        return EXIT_REFUSED
    print(canonical(value).decode("ascii"), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
