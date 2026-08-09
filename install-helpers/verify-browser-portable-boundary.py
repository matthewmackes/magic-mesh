#!/usr/bin/env python3
"""Validate the source-level WL-ARCH-008 portable Browser migration boundary.

This helper exercises only disposable fixtures through ``migrate-browser-profile``.
It neither discovers a user profile nor imports a bundle into a guest.  A pass proves
the migration helper maintains its explicit portable-data policy; it is not live
legacy-profile migration evidence.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
from types import ModuleType


HELPER = "install-helpers/migrate-browser-profile.py"


class BoundaryError(RuntimeError):
    """The migration helper no longer satisfies the portable-data contract."""


def load_migration(repo_root: Path) -> ModuleType:
    path = repo_root / HELPER
    if not path.is_file() or path.is_symlink():
        raise BoundaryError(f"migration helper must be a regular file: {path}")
    spec = importlib.util.spec_from_file_location("browser_profile_migration", path)
    if spec is None or spec.loader is None:
        raise BoundaryError(f"cannot load migration helper: {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def require_policy(module: ModuleType) -> None:
    required_names = {"cookies", "login data", "local state", "local storage", "session storage"}
    denied_names = getattr(module, "DENY_NAMES", None)
    denied_parts = getattr(module, "DENY_PARTS", None)
    if not isinstance(denied_names, set) or not required_names <= denied_names:
        raise BoundaryError("denylist must explicitly reject cookie, login, and state stores")
    if not isinstance(denied_parts, tuple) or not {"credential", "password", "secret", "token"} <= set(denied_parts):
        raise BoundaryError("denylist must reject credential-bearing filename parts")
    for name in ("profile_candidates", "migrate", "self_test"):
        if not callable(getattr(module, name, None)):
            raise BoundaryError(f"migration helper is missing required callable: {name}")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fixture_roots(base: Path) -> list[tuple[Path, str]]:
    profile = base / "profile"
    downloads = base / "downloads"
    policies = base / "policies"
    (profile / "Sessions").mkdir(parents=True)
    (profile / "Extensions" / "safe-extension").mkdir(parents=True)
    downloads.mkdir()
    policies.mkdir()
    (profile / "Bookmarks").write_text('{"roots":{"bookmark_bar":{}}}\n', encoding="utf-8")
    (profile / "History").write_bytes(b"history-fixture")
    (profile / "Sessions" / "Current Tabs").write_bytes(b"session-fixture")
    (profile / "Extensions" / "safe-extension" / "manifest.json").write_text(
        '{"name":"safe fixture"}\n', encoding="utf-8"
    )
    (downloads / "manual.pdf").write_bytes(b"portable-download")
    (policies / "managed.json").write_text('{"BrowserSignin":0}\n', encoding="utf-8")
    (profile / "Cookies").write_text("COOKIE_SECRET", encoding="utf-8")
    (profile / "Login Data").write_text("PASSWORD_SECRET", encoding="utf-8")
    (profile / "Local Storage").mkdir()
    (profile / "Local Storage" / "token.txt").write_text("TOKEN_SECRET", encoding="utf-8")
    (profile / "unlisted.txt").write_text("not portable", encoding="utf-8")
    try:
        (profile / "Bookmarks-link").symlink_to(profile / "Bookmarks")
    except OSError as exc:
        raise BoundaryError(f"fixture filesystem must support symlinks: {exc}") from exc
    return [(profile, "profile"), (downloads, "downloads"), (policies, "policies")]


def validate_bundle(module: ModuleType, roots: list[tuple[Path, str]], output: Path) -> dict:
    first = module.migrate(roots, output)
    encoded_first = (output / "manifest.json").read_bytes()
    second = module.migrate(roots, output)
    encoded_second = (output / "manifest.json").read_bytes()
    if first != second or encoded_first != encoded_second:
        raise BoundaryError("existing identical bundle must be explicitly idempotent")
    try:
        manifest = json.loads(encoded_first)
    except json.JSONDecodeError as exc:
        raise BoundaryError("manifest is not valid JSON") from exc
    if manifest != first:
        raise BoundaryError("manifest JSON does not exactly describe migration result")
    policy = manifest.get("policy")
    if policy != {"credential_stores": "never-export", "symlinks": "reject", "deterministic": True}:
        raise BoundaryError("manifest must declare the complete portable-data policy")
    entries = manifest.get("entries")
    if not isinstance(entries, list) or entries != sorted(entries, key=lambda item: (item["category"], item["path"])):
        raise BoundaryError("manifest entries must be deterministic and sorted")
    identities = [(entry.get("category"), entry.get("path")) for entry in entries]
    if len(identities) != len(set(identities)):
        raise BoundaryError("manifest must not contain duplicate source identities")
    imported = [entry for entry in entries if entry.get("status") == "imported"]
    outputs = [entry.get("output") for entry in imported]
    if any(not isinstance(output_path, str) or not output_path for output_path in outputs):
        raise BoundaryError("every imported entry must have a destination identity")
    if len(outputs) != len(set(outputs)):
        raise BoundaryError("manifest must not contain duplicate destination identities")
    failed = [entry for entry in entries if entry.get("status") == "failed"]
    if failed:
        raise BoundaryError("migration failures must fail the portable boundary closed")
    counts = manifest.get("counts")
    expected_counts = {
        status: sum(entry.get("status") == status for entry in entries)
        for status in ("imported", "skipped", "failed")
    }
    if counts != expected_counts:
        raise BoundaryError("manifest counts must exactly match its entries")
    expected = {
        ("bookmarks", "Bookmarks"),
        ("history", "History"),
        ("sessions", "Sessions/Current Tabs"),
        ("extensions", "Extensions/safe-extension/manifest.json"),
        ("downloads", "manual.pdf"),
        ("policies", "managed.json"),
    }
    observed = {(entry.get("category"), entry.get("path")) for entry in imported}
    if observed != expected:
        raise BoundaryError(f"allowlist imported unexpected portable data: {sorted(observed)}")
    skipped = {(entry.get("path"), entry.get("reason")) for entry in entries if entry.get("status") == "skipped"}
    required_skips = {
        ("Cookies", "credential-bearing-store"),
        ("Login Data", "credential-bearing-store"),
        ("Local Storage/token.txt", "credential-bearing-store"),
        ("Bookmarks-link", "symlink-rejected"),
        ("unlisted.txt", "unsupported-profile-entry"),
    }
    if not required_skips <= skipped:
        raise BoundaryError(f"denylist or symlink policy was not enforced: {sorted(skipped)}")
    payload = output / "payload"
    for path in payload.rglob("*"):
        if path.is_symlink():
            raise BoundaryError(f"bundle payload contains a symlink: {path}")
        if path.is_file() and b"_SECRET" in path.read_bytes():
            raise BoundaryError(f"bundle payload leaked fixture secret: {path}")
    return manifest


def validate_duplicate_identity_rejection(
    module: ModuleType, roots: list[tuple[Path, str]], output: Path
) -> None:
    duplicate_roots = [*roots, roots[0]]
    try:
        validate_bundle(module, duplicate_roots, output)
    except BoundaryError as exc:
        if "duplicate source identities" not in str(exc):
            raise BoundaryError(f"duplicate identity failed for the wrong reason: {exc}") from exc
    else:
        raise BoundaryError("duplicate source identities must fail the portable boundary closed")


def validate_source(repo_root: Path) -> None:
    module = load_migration(repo_root)
    require_policy(module)
    # Keep the migration helper's own explicit idempotency test executable.
    module.self_test()
    with tempfile.TemporaryDirectory(prefix="browser-portable-boundary-") as raw:
        base = Path(raw)
        roots = fixture_roots(base)
        first_output = base / "first"
        second_output = base / "second"
        duplicate_output = base / "duplicate"
        first = validate_bundle(module, roots, first_output)
        second = validate_bundle(module, roots, second_output)
        if first != second or sha256(first_output / "manifest.json") != sha256(second_output / "manifest.json"):
            raise BoundaryError("identical input must yield byte-identical deterministic manifests")
        validate_duplicate_identity_rejection(module, roots, duplicate_output)


def self_test() -> None:
    root = Path(__file__).resolve().parent.parent
    validate_source(root)
    print("verify-browser-portable-boundary.py: self-test passed")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, help="repository root (defaults to this helper's parent)")
    parser.add_argument("--self-test", action="store_true", help="run disposable source-level fixtures")
    args = parser.parse_args(argv)
    root = (args.repo_root or Path(__file__).resolve().parent.parent).resolve()
    try:
        validate_source(root)
    except (BoundaryError, OSError, AssertionError) as exc:
        print(f"browser portable boundary: FAIL: {exc}", file=sys.stderr)
        return 1
    print("browser portable boundary: PASS")
    if args.self_test:
        print("verify-browser-portable-boundary.py: self-test passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
