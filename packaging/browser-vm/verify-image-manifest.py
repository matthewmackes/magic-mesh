#!/usr/bin/env python3
"""Create and verify the sole Browser VM disk-artifact identity manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile

SCHEMA_VERSION = 1
KIND = "mcnf-browser-vm-image-manifest"
IMAGE_VERSION = "browser-vm-chromium-v1"
MAX_MANIFEST_BYTES = 64 * 1024
MAX_PROFILE_BYTES = 16 * 1024
MAX_ASSET_BYTES = 32 * 1024 * 1024
MAX_ARTIFACT_BYTES = 128 * 1024**3
HASH_CHUNK = 1024 * 1024

PROFILE_KEYS = {
    "BROWSER_VM_PROFILE_SCHEMA",
    "BROWSER_VM_PROFILE_ID",
    "BROWSER_VM_IMAGE_ID",
    "BROWSER_VM_SOURCE_REPOSITORY",
    "BROWSER_VM_SOURCE_PATH",
    "BROWSER_VM_SOURCE_COMMIT",
    "BROWSER_VM_GUEST_OS",
    "BROWSER_VM_COMPOSITOR",
    "BROWSER_VM_BROWSER",
    "BROWSER_VM_VCPU",
    "BROWSER_VM_MEMORY_MB",
    "BROWSER_VM_DISK_GB",
    "BROWSER_VM_TRANSPORTS",
    "BROWSER_VM_DEFAULT_TRANSPORT",
    "BROWSER_VM_HOST_BROWSER",
    "BROWSER_VM_NETWORK",
    "BROWSER_VM_RUNTIME_FAILURE_POLICY",
    "BROWSER_VM_GUEST_TERMINAL_STATES",
}

# Exact source inputs copied into, compiled into, or used to configure the
# immutable guest. The final disk digest remains the complete runtime binding;
# this list makes its source/profile derivation independently reviewable.
RUNTIME_ASSETS = (
    "packaging/browser-vm/Containerfile",
    "packaging/browser-vm/mcnf-browser-vm-managed-policy.json",
    "packaging/browser-vm/mcnf-browser-vm-media-fixture.html",
    "packaging/browser-vm/mcnf-browser-vm-media-probe.sh",
    "packaging/browser-vm/mcnf-browser-vm-runtime.service",
    "packaging/browser-vm/mcnf-browser-vm-runtime.sh",
    "packaging/browser-vm/mcnf-browser-vm-session.sh",
    "packaging/browser-vm/mcnf-browser-vm-xrdp-startwm.sh",
    "packaging/browser-vm/mcnf-x11-present-copy.c",
    "packaging/browser-vm/validate-runtime-inputs.sh",
    "packaging/browser-vm/verify-session-input-contract.sh",
    "crates/desktop/mde-media-core/tests/fixtures/tiny_clip.mkv",
    "install-helpers/browser-vm-production-control/Cargo.toml",
    "install-helpers/browser-vm-production-control/Cargo.lock",
    "install-helpers/browser-vm-production-control/guest-controller/Cargo.toml",
    "install-helpers/browser-vm-production-control/guest-controller/Cargo.lock",
    "install-helpers/browser-vm-production-control/src/bin/browser-vm-guest-audio-probe-controller.rs",
    "install-helpers/browser-vm-production-control/deploy/browser-vm-guest-audio-probe-controller.service",
    "install-helpers/browser-vm-production-control/deploy/controller-config.example.json",
)


class ManifestError(ValueError):
    pass


def fail(message: str) -> None:
    raise ManifestError(message)


def regular_file(path: Path, label: str, maximum: int) -> os.stat_result:
    try:
        metadata = path.lstat()
    except OSError as exc:
        fail(f"{label} metadata unavailable: {exc}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail(f"{label} must be a regular non-symlink file: {path}")
    if metadata.st_size <= 0 or metadata.st_size > maximum:
        fail(f"{label} size is outside the bounded contract: {metadata.st_size}")
    if metadata.st_mode & 0o022:
        fail(f"{label} must not be writable by group or other: {path}")
    return metadata


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(HASH_CHUNK):
            digest.update(chunk)
    return f"sha256:{digest.hexdigest()}"


def parse_profile(path: Path) -> tuple[dict[str, str], os.stat_result]:
    metadata = regular_file(path, "profile", MAX_PROFILE_BYTES)
    values: dict[str, str] = {}
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError) as exc:
        fail(f"profile is unreadable UTF-8: {exc}")
    for line in lines:
        line = line.removesuffix("\r")
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            fail("profile contains a malformed line")
        key, value = line.split("=", 1)
        if key not in PROFILE_KEYS or not value or any(ch.isspace() for ch in value):
            fail(f"profile contains an unknown or malformed field: {key}")
        if key in values:
            fail(f"profile contains a duplicate field: {key}")
        values[key] = value
    if set(values) != PROFILE_KEYS:
        fail("profile fields do not match the manifest schema")
    return values, metadata


def integer(value: object, label: str, minimum: int, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        fail(f"{label} must be an integer")
    if value < minimum or value > maximum:
        fail(f"{label} is outside the bounded contract")
    return value


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


def virtual_size(path: Path, image_format: str) -> int | None:
    if image_format == "anaconda-iso":
        return None
    try:
        result = subprocess.run(
            ["qemu-img", "info", "--output=json", "--", str(path)],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=30,
        )
        value = json.loads(result.stdout)
    except (OSError, subprocess.SubprocessError, json.JSONDecodeError) as exc:
        fail(f"qemu image metadata unavailable: {exc}")
    if not isinstance(value, dict) or value.get("format") != image_format:
        fail("qemu image format does not match the manifest")
    return integer(value.get("virtual-size"), "virtual image size", 1, MAX_ARTIFACT_BYTES)


def profile_record(profile: Path, values: dict[str, str], metadata: os.stat_result) -> dict[str, object]:
    if values["BROWSER_VM_HOST_BROWSER"] != "false":
        fail("artifact profile enables host Browser ownership")
    try:
        transports = values["BROWSER_VM_TRANSPORTS"].split(",")
        vcpu = int(values["BROWSER_VM_VCPU"])
        memory = int(values["BROWSER_VM_MEMORY_MB"])
        disk = int(values["BROWSER_VM_DISK_GB"])
    except ValueError as exc:
        fail(f"profile resource value is malformed: {exc}")
    return {
        "bytes": metadata.st_size,
        "default_transport": values["BROWSER_VM_DEFAULT_TRANSPORT"],
        "disk_gib": disk,
        "host_browser": values["BROWSER_VM_HOST_BROWSER"] == "true",
        "id": values["BROWSER_VM_PROFILE_ID"],
        "image_id": values["BROWSER_VM_IMAGE_ID"],
        "image_version": IMAGE_VERSION,
        "memory_mib": memory,
        "sha256": sha256(profile),
        "source_commit": values["BROWSER_VM_SOURCE_COMMIT"],
        "transports": transports,
        "vcpu": vcpu,
    }


def asset_records(repo_root: Path) -> list[dict[str, object]]:
    records = []
    for relative in RUNTIME_ASSETS:
        path = repo_root / relative
        metadata = regular_file(path, f"runtime asset {relative}", MAX_ASSET_BYTES)
        records.append({"bytes": metadata.st_size, "path": relative, "sha256": sha256(path)})
    return records


def build_manifest(repo_root: Path, profile: Path, image: Path, image_format: str) -> dict[str, object]:
    if image_format not in {"qcow2", "raw", "anaconda-iso"}:
        fail("unsupported Browser VM artifact format")
    values, profile_metadata = parse_profile(profile)
    image_metadata = regular_file(image, "image artifact", MAX_ARTIFACT_BYTES)
    expected_virtual = int(values["BROWSER_VM_DISK_GB"]) * 1024**3
    observed_virtual = virtual_size(image, image_format)
    if observed_virtual is not None and observed_virtual != expected_virtual:
        fail("image virtual size does not match the profile")
    return {
        "artifact": {
            "bytes": image_metadata.st_size,
            "filename": image.name,
            "format": image_format,
            "sha256": sha256(image),
            "virtual_size_bytes": observed_virtual,
        },
        "kind": KIND,
        "profile": profile_record(profile, values, profile_metadata),
        "runtime_assets": asset_records(repo_root),
        "schema_version": SCHEMA_VERSION,
    }


def load_manifest(path: Path) -> dict[str, object]:
    regular_file(path, "image manifest", MAX_MANIFEST_BYTES)
    try:
        value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=no_duplicate_json)
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        fail(f"image manifest is malformed: {exc}")
    return exact_keys(
        value,
        {"artifact", "kind", "profile", "runtime_assets", "schema_version"},
        "manifest",
    )


def verify_manifest(repo_root: Path, profile: Path, image: Path, manifest: Path) -> None:
    if manifest.name != f"{image.name}.mcnf-manifest.json":
        fail("manifest does not use the canonical image sidecar name")
    value = load_manifest(manifest)
    if value["schema_version"] != SCHEMA_VERSION or value["kind"] != KIND:
        fail("manifest identity/version is unsupported")
    artifact = exact_keys(
        value["artifact"],
        {"bytes", "filename", "format", "sha256", "virtual_size_bytes"},
        "artifact",
    )
    image_format = artifact["format"]
    if not isinstance(image_format, str):
        fail("artifact format must be a string")
    expected = build_manifest(repo_root, profile, image, image_format)
    if value != expected:
        fail("manifest is stale or does not match the profile, image, or runtime assets")


def write_manifest(value: dict[str, object], output: Path) -> None:
    if output.is_symlink() or (output.exists() and not output.is_file()):
        fail("manifest output must not be a symlink or non-regular file")
    body = (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()
    if len(body) > MAX_MANIFEST_BYTES:
        fail("generated manifest exceeds the bounded contract")
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(f".{output.name}.tmp")
    if temporary.exists() or temporary.is_symlink():
        fail("manifest temporary output already exists")
    old_umask = os.umask(0o022)
    try:
        with temporary.open("xb") as handle:
            handle.write(body)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, output)
    finally:
        os.umask(old_umask)
        if temporary.exists() or temporary.is_symlink():
            temporary.unlink()


def self_test(repo_root: Path, profile: Path) -> None:
    def expect_reject(action: object, label: str) -> None:
        try:
            action()
        except ManifestError:
            return
        fail(f"accepted {label}")

    with tempfile.TemporaryDirectory(prefix="browser-vm-manifest-") as raw:
        root = Path(raw)
        image = root / "browser-vm.iso"
        manifest = root / "browser-vm.iso.mcnf-manifest.json"
        image.write_bytes(b"bounded-browser-vm-image-fixture\n")
        image.chmod(0o644)
        write_manifest(build_manifest(repo_root, profile, image, "anaconda-iso"), manifest)
        verify_manifest(repo_root, profile, image, manifest)

        original = manifest.read_bytes()
        image.write_bytes(b"truncated\n")
        expect_reject(
            lambda: verify_manifest(repo_root, profile, image, manifest),
            "a truncated image",
        )
        image.write_bytes(b"bounded-browser-vm-image-fixture\n")

        value = json.loads(original)
        value["unknown"] = True
        manifest.write_text(json.dumps(value), encoding="utf-8")
        expect_reject(
            lambda: verify_manifest(repo_root, profile, image, manifest),
            "an unknown manifest field",
        )

        value.pop("unknown")
        value["profile"]["source_commit"] = "f" * 40
        manifest.write_text(json.dumps(value), encoding="utf-8")
        expect_reject(
            lambda: verify_manifest(repo_root, profile, image, manifest),
            "stale profile identity",
        )

        manifest.write_bytes(original[: max(1, len(original) // 2)])
        expect_reject(
            lambda: verify_manifest(repo_root, profile, image, manifest),
            "a truncated manifest",
        )

        manifest.unlink()
        manifest.symlink_to(root / "missing-manifest")
        expect_reject(
            lambda: verify_manifest(repo_root, profile, image, manifest),
            "a symlinked manifest",
        )

        host_browser_profile = root / "host-browser.env"
        host_browser_profile.write_text(
            profile.read_text(encoding="utf-8").replace(
                "BROWSER_VM_HOST_BROWSER=false",
                "BROWSER_VM_HOST_BROWSER=true",
            ),
            encoding="utf-8",
        )
        host_browser_profile.chmod(0o644)
        expect_reject(
            lambda: build_manifest(repo_root, host_browser_profile, image, "anaconda-iso"),
            "a profile that enables host Browser ownership",
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    for command in ("create", "verify"):
        child = subparsers.add_parser(command)
        child.add_argument("--repo-root", required=True, type=Path)
        child.add_argument("--profile", required=True, type=Path)
        child.add_argument("--image", required=True, type=Path)
        child.add_argument("--manifest", required=True, type=Path)
        if command == "create":
            child.add_argument("--format", required=True, choices=("qcow2", "raw", "anaconda-iso"))
    test = subparsers.add_parser("self-test")
    test.add_argument("--repo-root", required=True, type=Path)
    test.add_argument("--profile", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.command == "create":
            value = build_manifest(args.repo_root, args.profile, args.image, args.format)
            write_manifest(value, args.manifest)
            verify_manifest(args.repo_root, args.profile, args.image, args.manifest)
            print(f"Browser VM image manifest written: {args.manifest}")
        elif args.command == "verify":
            verify_manifest(args.repo_root, args.profile, args.image, args.manifest)
            print("Browser VM image manifest passed")
        else:
            self_test(args.repo_root, args.profile)
            print("Browser VM image manifest self-tests passed")
    except ManifestError as exc:
        print(f"verify-browser-vm-image-manifest: {exc}", file=os.sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
