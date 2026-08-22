#!/usr/bin/env python3
"""Emit the redacted six-role open-source input inventory (schema 1).

Records the already-selected Maps, App VM, bootc, Browser VM, RPM, and
UX-014 families. Does not reopen source selection, invent Flatpak catalog
refs, admit Maps production, or treat Android/Cuttlefish as a production
family. No network.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
from pathlib import Path


KIND = "mcnf-open-source-input-inventory"
SCHEMA = 1
MAX_INVENTORY = 64 * 1024
FAMILIES = ("maps", "app-vm", "bootc", "browser-vm", "rpm", "ux-014")
REFUSED_FAMILY_MARKERS = ("android", "cuttlefish")
FIXTURE_CATALOG_REF = "org.example.App"
MAPS_DEST = "/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles"
MAPS_SHA256 = "6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895"
RPM_FINGERPRINT = "06B1C27EA0E08A225155EB3314018AA1497DDC7C"


class Refusal(ValueError):
    pass


def refuse(message: str) -> None:
    raise Refusal(message)


def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        if key in value:
            refuse(f"duplicate JSON key: {key}")
        value[key] = item
    return value


def family_is_refused(name: str) -> bool:
    lowered = name.lower()
    return any(marker in lowered for marker in REFUSED_FAMILY_MARKERS)


def catalog_ref_is_fixture(ref: str) -> bool:
    app_id = ref.split("@", 1)[0].split(":", 1)[0]
    return app_id == FIXTURE_CATALOG_REF or app_id.startswith("org.example.")


def check_family_names(names: list[str]) -> None:
    for name in names:
        if family_is_refused(name):
            refuse(f"{name} is not a production family; Android/Cuttlefish is deferred")
    if tuple(names) != FAMILIES:
        refuse("families must be exactly the six-role set")


def check_catalog_refs(refs: list[str]) -> None:
    for ref in refs:
        if catalog_ref_is_fixture(ref):
            refuse(f"{FIXTURE_CATALOG_REF} is a fixture catalog ref, not a production catalog ref")
        refuse("production App catalog refs are leftover; do not invent catalog refs")


def selected_families() -> list[dict[str, object]]:
    return [
        {
            "attribution": "OpenStreetMap contributors",
            "dest": MAPS_DEST,
            "dest_sha256": MAPS_SHA256,
            "family": "maps",
            "host": "172.20.0.130",
            "license": "ODbL-1.0",
            "production_admitted": False,
            "region": "buffalo-niagara",
            "sources": ["geofabrik-ny-pbf", "tiger-2024-zip"],
        },
        {
            "architecture": "amd64",
            "family": "app-vm",
            "image_reference": "quay.io/fedora/fedora:42",
            "leftover": "real curated catalog refs remain leftover",
            "license": "Fedora Project terms",
            "profile": "wayland-standard",
            "receipt_revision": "aca7573bc",
            "receipt_sha256": "f939be3864024f0e7bbfe53a26272eb796e3f85d9a35231f2a9b7ca6f4fb7891",
            "resolved_digest": "sha256:e78cd1a688cd079c23864f289a89a49a3f4ad66d817864e325e1d058310ee95c",
        },
        {
            "architecture": "amd64",
            "family": "bootc",
            "image_reference": "quay.io/fedora/fedora-bootc:44",
            "license": "Fedora Project terms",
            "receipt_revision": "479ec2b8c",
            "receipt_sha256": "2e1a183fc48de8124624881d7ec5f99770d954d81a61dcc4cf4d07919f2326ae",
            "release_role": "all-roles",
            "resolved_digest": "sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357",
        },
        {
            "architecture": "amd64",
            "containerfile_pin": (
                "quay.io/fedora/fedora-bootc@sha256:"
                "3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357"
            ),
            "family": "browser-vm",
            "image_reference": "quay.io/fedora/fedora-bootc:44",
            "leftover": "private dest bound to b30954e31 / :44; dest not rebound",
            "license": "Fedora Project terms",
            "producer": "packaging/browser-vm/produce-base-image-receipt.py",
            "profile": "browser-vm-chromium",
            "receipt_revision": "b30954e31",
            "receipt_sha256": "ac9755db790445048eb621542b69ec24220b58ecec3e056a9e570309b7c100a9",
            "resolved_digest": (
                "sha256:3a5e74e668761be9e16c6779950ae154d9dcbb0861d1e92140c0751fed1f5357"
            ),
            "target": "mcnf-browser-vm/browser-vm-chromium-v1",
            "verifier": "packaging/browser-vm/verify-image-manifest.py",
        },
        {
            "family": "rpm",
            "leftover": "current-revision receipt waits on freeze (WL-REL-001)",
            "license": "operator-controlled key policy",
            "roles": ["workstation-rpm", "server-rpm", "lighthouse-rpm"],
            "signing_fingerprint": RPM_FINGERPRINT,
        },
        {
            "admission": "--source",
            "family": "ux-014",
            "license": "CC0-1.0",
            "package": "kiron",
            "verifier": "packaging/kiron/verify-package.sh",
        },
    ]


def inventory_document() -> dict[str, object]:
    return {
        "families": selected_families(),
        "kind": KIND,
        "schema_version": SCHEMA,
    }


def family_rows(families: object) -> list[dict[str, object]]:
    if not isinstance(families, list) or not families:
        refuse("families must be the exact six-role row list")
    rows: list[dict[str, object]] = []
    names: list[str] = []
    for row in families:
        if not isinstance(row, dict) or "family" not in row:
            refuse("each family row must name its family")
        name = row["family"]
        if not isinstance(name, str):
            refuse("family key is malformed")
        names.append(name)
        rows.append(row)
    check_family_names(names)
    return rows


def walk_strings(value: object) -> list[str]:
    if isinstance(value, str):
        return [value]
    if isinstance(value, list):
        found: list[str] = []
        for item in value:
            found.extend(walk_strings(item))
        return found
    if isinstance(value, dict):
        found = []
        for key, item in value.items():
            found.append(str(key))
            found.extend(walk_strings(item))
        return found
    return []


def validate_inventory(value: dict[str, object]) -> dict[str, object]:
    expected = {"schema_version", "kind", "families"}
    if set(value) != expected:
        refuse("inventory fields are not exact")
    if value["schema_version"] != SCHEMA or value["kind"] != KIND:
        refuse("inventory schema is unsupported")
    rows = family_rows(value["families"])
    for text in walk_strings(rows):
        if catalog_ref_is_fixture(text):
            refuse(f"{FIXTURE_CATALOG_REF} is a fixture catalog ref, not a production catalog ref")
    by_name = {str(row["family"]): row for row in rows}
    maps = by_name["maps"]
    if maps.get("production_admitted") is not False:
        refuse("maps production_admitted must be false")
    if rows != selected_families():
        refuse("inventory identities are not the already-selected six-role set")
    return value


def load_inventory(path: Path) -> dict[str, object]:
    if path.is_symlink() or not path.is_file():
        refuse("inventory must be a regular non-symlink file")
    body = path.read_bytes()
    if not body or len(body) > MAX_INVENTORY:
        refuse("inventory size is outside the allowed bound")
    try:
        value = json.loads(
            body.decode("utf-8"),
            object_pairs_hook=unique_object,
            parse_constant=lambda item: refuse(f"non-finite JSON number: {item}"),
        )
    except (UnicodeError, json.JSONDecodeError) as exc:
        refuse(f"inventory is malformed: {exc}")
    if not isinstance(value, dict):
        refuse("inventory must be one JSON object")
    canonical = (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii")
    if body != canonical:
        refuse("inventory is not canonical JSON")
    return validate_inventory(value)


def publish(output: Path, document: dict[str, object]) -> None:
    if output.exists() or output.is_symlink():
        refuse("inventory output already exists or is substituted")
    parent = output.parent.resolve(strict=True)
    if not parent.is_dir() or parent.stat().st_mode & 0o022:
        refuse("inventory output parent must be a private real directory")
    body = (json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n").encode("ascii")
    if len(body) > MAX_INVENTORY:
        refuse("inventory exceeds its bounded contract")
    directory = Path(tempfile.mkdtemp(prefix=f".{output.name}.", dir=parent))
    try:
        directory.chmod(0o700)
        staged = directory / "inventory.json"
        descriptor = os.open(staged, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(body)
            stream.flush()
            os.fsync(stream.fileno())
        os.link(staged, output, follow_symlinks=False)
        staged.unlink()
        parent_fd = os.open(parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(parent_fd)
        finally:
            os.close(parent_fd)
    finally:
        try:
            directory.rmdir()
        except OSError:
            pass


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    produce = sub.add_parser("produce")
    produce.add_argument("--output", required=True, type=Path)
    produce.add_argument("--family", action="append", default=[])
    produce.add_argument("--catalog-ref", action="append", default=[])
    inspect = sub.add_parser("inspect")
    inspect.add_argument("--inventory", required=True, type=Path)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.command == "produce":
            names = list(args.family) if args.family else list(FAMILIES)
            check_family_names(names)
            check_catalog_refs(list(args.catalog_ref))
            document = inventory_document()
            validate_inventory(document)
            publish(args.output, document)
            print(f"open-source-input-inventory: PASS: wrote six-role inventory to {args.output}")
        else:
            validate_inventory(load_inventory(args.inventory))
            print("open-source-input-inventory: PASS: six-role inventory")
        return 0
    except (OSError, Refusal, UnicodeError, ValueError) as exc:
        print(f"open-source-input-inventory: REFUSED: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
