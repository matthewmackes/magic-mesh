#!/usr/bin/env python3
"""Validate the shared mde-egui Carbon icon registry.

The registry is deliberately source-driven: every SVG in the embedded asset
directory must have exactly one matching include_bytes! entry, and every entry
must resolve to a symbolic, local SVG.  Keeping this check outside Rust makes
it useful before cargo has compiled the shared crate and catches accidental
asset/license drift in review.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path
from xml.etree import ElementTree


ROOT = Path(__file__).resolve().parents[1]
REGISTRY = ROOT / "crates/shared/mde-egui/src/carbon.rs"
ASSET_DIR = ROOT / "crates/shared/mde-egui/assets/carbon"
SOURCE_LICENSE = ROOT / "assets/icons/Mackes-Carbon/LICENSE"
INCLUDE_RE = re.compile(
    r'\(\s*"([a-z0-9][a-z0-9-]*)",\s*include_bytes!\("\.\./assets/carbon/([^"]+\.svg)"\)\s*,?\s*\)'
)
NAME_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")


def validate(registry: str, assets: list[tuple[str, str]], license_text: str) -> list[str]:
    errors: list[str] = []
    entries = INCLUDE_RE.findall(registry)
    names = [name for name, _ in entries]
    if len(names) != len(set(names)):
        duplicates = sorted({name for name in names if names.count(name) > 1})
        errors.append(f"duplicate registry name(s): {', '.join(duplicates)}")

    asset_names = [name for name, _ in assets]
    if len(asset_names) != len(set(asset_names)):
        errors.append("duplicate asset name(s)")
    for name, filename in entries:
        if not NAME_RE.fullmatch(name):
            errors.append(f"unsafe registry name: {name!r}")
        if filename != f"{name}.svg":
            errors.append(f"registry key {name!r} does not match {filename!r}")
    registered_files = [filename for _, filename in entries]
    on_disk = [filename for _, filename in assets]
    if sorted(registered_files) != sorted(on_disk):
        missing = sorted(set(on_disk) - set(registered_files))
        orphaned = sorted(set(registered_files) - set(on_disk))
        if missing:
            errors.append(f"unregistered asset(s): {', '.join(missing)}")
        if orphaned:
            errors.append(f"missing asset file(s): {', '.join(orphaned)}")
    if "Apache License" not in license_text or "Version 2.0" not in license_text:
        errors.append("Mackes-Carbon Apache-2.0 provenance license is missing or unexpected")
    return errors


def validate_repository() -> list[str]:
    assets: list[tuple[str, str]] = []
    for path in sorted(ASSET_DIR.glob("*.svg")):
        filename = path.name
        assets.append((path.stem, filename))
        try:
            root = ElementTree.fromstring(path.read_bytes())
        except (OSError, ElementTree.ParseError) as exc:
            return [f"{filename}: invalid SVG: {exc}"]
        if root.tag.rsplit("}", 1)[-1] != "svg":
            return [f"{filename}: root element is not <svg>"]
        source = path.read_text(encoding="utf-8")
        if "currentColor" not in source:
            return [f"{filename}: symbolic SVG must use currentColor"]
        if re.search(r"<\s*(?:text|image|use)\b", source, re.IGNORECASE):
            return [f"{filename}: unsupported text/image/use element"]
    return validate(REGISTRY.read_text(encoding="utf-8"), assets, SOURCE_LICENSE.read_text(encoding="utf-8"))


def self_test() -> None:
    valid = '("go-next", include_bytes!("../assets/carbon/go-next.svg"))'
    assert not validate(valid, [("go-next", "go-next.svg")], "Apache License Version 2.0")
    assert validate(valid + "\n" + valid, [("go-next", "go-next.svg")], "Apache License Version 2.0")
    assert validate(valid, [("go-next", "go-next.svg"), ("orphan", "orphan.svg")], "Apache License Version 2.0")
    assert validate(
        '("go-next", include_bytes!("../assets/carbon/go-previous.svg"))',
        [("go-next", "go-previous.svg")],
        "Apache License Version 2.0",
    )
    assert validate(valid, [("go-next", "go-next.svg")], "GPL-3.0-only")
    print("Carbon icon registry self-tests passed")


def main() -> int:
    if len(sys.argv) == 2 and sys.argv[1] == "--self-test":
        self_test()
        return 0
    errors = validate_repository()
    if errors:
        for error in errors:
            print(f"[FAIL] {error}", file=sys.stderr)
        return 1
    print(f"[OK] Carbon icon registry: {len(list(ASSET_DIR.glob('*.svg')))} assets, exact parity, symbolic SVGs, Apache-2.0 provenance")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
