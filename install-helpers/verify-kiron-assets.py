#!/usr/bin/env python3
"""Verify the bounded, provenance-bound UX-014 Kiron asset package.

The validator is intentionally independent of the desktop renderer.  It is a
package admission gate: every authored scene must be licensed, size bounded,
content-addressed, and covered by a complete live/pre-rendered/static fallback
ladder before a package can be consumed by a renderer.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import stat
import tempfile

SCHEMA_VERSION = 1
KIND = "mcnf-kiron-asset-manifest"
MAX_MANIFEST_BYTES = 128 * 1024
MAX_ASSET_BYTES = 32 * 1024 * 1024
HASH_CHUNK = 1024 * 1024
GRADES = tuple("ABCDEF")
MODES = ("live-3d", "pre-rendered", "static")
LICENSES = {"Apache-2.0", "CC0-1.0", "CC-BY-4.0", "MIT"}


class ManifestError(ValueError):
    pass


def fail(message: str) -> None:
    raise ManifestError(message)


def exact_keys(value: object, keys: set[str], label: str) -> dict[str, object]:
    if not isinstance(value, dict) or set(value) != keys:
        fail(f"{label} fields do not match the schema")
    return value


def no_duplicate_json(pairs: list[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        if key in value:
            fail(f"manifest contains duplicate JSON field: {key}")
        value[key] = item
    return value


def regular_asset(root: Path, relative: object, label: str) -> tuple[Path, int]:
    if not isinstance(relative, str) or not relative or "\\" in relative:
        fail(f"{label} path is malformed")
    path = Path(relative)
    if path.is_absolute() or ".." in path.parts or any(part == "" for part in path.parts):
        fail(f"{label} path escapes the package root")
    target = root / path
    try:
        metadata = target.lstat()
    except OSError as exc:
        fail(f"{label} is unavailable: {exc}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail(f"{label} must be a regular non-symlink file")
    if metadata.st_size <= 0 or metadata.st_size > MAX_ASSET_BYTES:
        fail(f"{label} size is outside the bounded contract")
    if metadata.st_mode & 0o022:
        fail(f"{label} must not be writable by group or other")
    return target, metadata.st_size


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(HASH_CHUNK):
            digest.update(chunk)
    return digest.hexdigest()


def digest(value: object, label: str) -> str:
    if not isinstance(value, str) or len(value) != 64 or value != value.lower():
        fail(f"{label} is not a lowercase SHA-256 digest")
    try:
        int(value, 16)
    except ValueError:
        fail(f"{label} is not a hexadecimal SHA-256 digest")
    if set(value) == {"0"}:
        fail(f"{label} must not be all zeroes")
    return value


def verify_manifest(root: Path, manifest: Path) -> None:
    try:
        metadata = manifest.lstat()
    except OSError as exc:
        fail(f"manifest is unavailable: {exc}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail("manifest must be a regular non-symlink file")
    if metadata.st_size <= 0 or metadata.st_size > MAX_MANIFEST_BYTES:
        fail("manifest size is outside the bounded contract")
    try:
        value = json.loads(manifest.read_text(encoding="utf-8"), object_pairs_hook=no_duplicate_json)
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        fail(f"manifest is malformed: {exc}")

    document = exact_keys(value, {"kind", "schema_version", "assets"}, "manifest")
    if document["kind"] != KIND or document["schema_version"] != SCHEMA_VERSION:
        fail("manifest identity/version is unsupported")
    assets = document["assets"]
    if not isinstance(assets, list) or len(assets) != len(GRADES) * len(MODES):
        fail("manifest must contain exactly one asset for each grade and fallback mode")

    seen: set[tuple[str, str]] = set()
    for index, raw in enumerate(assets):
        row = exact_keys(raw, {"grade", "mode", "path", "bytes", "sha256", "license"}, f"assets[{index}]")
        grade, mode = row["grade"], row["mode"]
        if grade not in GRADES or mode not in MODES:
            fail(f"assets[{index}] has an unsupported grade or fallback mode")
        identity = (grade, mode)
        if identity in seen:
            fail(f"duplicate asset identity: {grade}/{mode}")
        seen.add(identity)
        if row["license"] not in LICENSES:
            fail(f"assets[{index}] has no approved SPDX license")
        if not isinstance(row["bytes"], int) or isinstance(row["bytes"], bool) or row["bytes"] <= 0 or row["bytes"] > MAX_ASSET_BYTES:
            fail(f"assets[{index}].bytes is outside the bounded contract")
        expected = digest(row["sha256"], f"assets[{index}].sha256")
        path, size = regular_asset(root, row["path"], f"assets[{index}]")
        if size != row["bytes"]:
            fail(f"assets[{index}] byte count does not match the file")
        if sha256(path) != expected:
            fail(f"assets[{index}] digest does not match the file")

    expected_identities = {(grade, mode) for grade in GRADES for mode in MODES}
    if seen != expected_identities:
        fail("fallback ladder is incomplete: every A-F grade needs live, pre-rendered, and static assets")


def write_fixture(root: Path, manifest: Path) -> None:
    assets = []
    for grade in GRADES:
        for mode in MODES:
            relative = f"assets/{grade.lower()}-{mode}.bin"
            target = root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(f"{grade}:{mode}\n".encode())
            os.chmod(target, 0o644)
            assets.append({
                "grade": grade,
                "mode": mode,
                "path": relative,
                "bytes": target.stat().st_size,
                "sha256": sha256(target),
                "license": "CC0-1.0",
            })
    manifest.write_text(json.dumps({"kind": KIND, "schema_version": SCHEMA_VERSION, "assets": assets}, separators=(",", ":")), encoding="utf-8")


def self_test() -> None:
    with tempfile.TemporaryDirectory(prefix="mcnf-kiron-assets-") as directory:
        root = Path(directory)
        manifest = root / "manifest.json"
        write_fixture(root, manifest)
        verify_manifest(root, manifest)

        tampered = json.loads(manifest.read_text(encoding="utf-8"))
        tampered["assets"][0]["sha256"] = "1" * 64
        manifest.write_text(json.dumps(tampered, separators=(",", ":")), encoding="utf-8")
        try:
            verify_manifest(root, manifest)
        except ManifestError:
            pass
        else:
            fail("self-test accepted a digest-mismatched asset")

        write_fixture(root, manifest)
        incomplete = json.loads(manifest.read_text(encoding="utf-8"))
        incomplete["assets"] = [row for row in incomplete["assets"] if row["mode"] != "static" or row["grade"] != "F"]
        manifest.write_text(json.dumps(incomplete, separators=(",", ":")), encoding="utf-8")
        try:
            verify_manifest(root, manifest)
        except ManifestError:
            pass
        else:
            fail("self-test accepted an incomplete static fallback")
    print("Kiron asset manifest verification self-tests passed")


def main() -> int:
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, help="package root containing the admitted assets")
    parser.add_argument("manifest", nargs="?", type=Path, help="manifest JSON path")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    try:
        if args.self_test:
            if args.root or args.manifest:
                parser.error("--self-test does not accept a manifest")
            self_test()
        elif args.root and args.manifest:
            verify_manifest(args.root, args.manifest)
            print(f"OK: {args.manifest} — six-grade fallback ladder and hashes verified")
        else:
            parser.error("provide --root ROOT MANIFEST or --self-test")
    except (ManifestError, OSError, UnicodeError, TypeError) as exc:
        parser.error(str(exc))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
