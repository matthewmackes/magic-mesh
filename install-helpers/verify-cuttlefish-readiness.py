#!/usr/bin/env python3
"""Verify the bounded Cuttlefish placement and guest-tool readiness contract.

This is a read-only placement gate for the two-layer Android design:

* the placement host must expose KVM, libvirt, the managed pool/network, and a
  valid qcow2 Cuttlefish base image bound to the governed Android manifest;
* the nested Debian host may publish a guest-tool receipt only after its fixed
  ``cvd`` and ``adb`` commands and ``/dev/kvm`` are present.

The receipt is tooling readiness only.  This helper never reports a booted
Android guest, a package inventory, a display, or a launchable app.  Missing
prerequisites are a structured ``unavailable`` result (exit 3), while malformed
configuration is a hard input error (exit 2).

Usage:
  verify-cuttlefish-readiness.py --config placement.json
  verify-cuttlefish-readiness.py --self-test
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import subprocess
import sys
from typing import Any, Callable, NoReturn, Optional


SCHEMA_VERSION = 2
CONFIG_KIND = "cuttlefish_placement_config"
REPORT_KIND = "cuttlefish_placement_readiness"
RECEIPT_KIND = "cuttlefish_guest_tool_readiness"
MAX_CONFIG_BYTES = 64 * 1024
MAX_RECEIPT_BYTES = 16 * 1024
MAX_PATH_BYTES = 512
MAX_ID_BYTES = 128
MIN_IMAGE_BYTES = 80 * 1024**3
MAX_IMAGE_BYTES = 4 * 1024**4
MAX_RECEIPT_AGE_SECONDS = 24 * 60 * 60
MAX_FUTURE_SKEW_SECONDS = 300
MAX_COMMAND_OUTPUT_BYTES = 64 * 1024
HASH_CHUNK_BYTES = 1024 * 1024
MAX_RELEASE_ARTIFACT_BYTES = 8 * 1024**3

IDENTITY_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$")
VERSION_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+:/ -]{0,127}$")
DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
URI_RE = re.compile(
    r"^(?:qemu:///system|qemu\+ssh://(?:[A-Za-z0-9_.-]{1,32}@)?"
    r"[A-Za-z0-9][A-Za-z0-9_.:-]{0,252}/system)$"
)
TIMESTAMP_RE = re.compile(
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"
)

CONFIG_FIELDS = frozenset(
    {
        "schema_version",
        "kind",
        "workload_id",
        "libvirt_uri",
        "pool",
        "network",
        "base_image",
        "release_artifact",
        "guest_readiness_receipt",
    }
)
BASE_IMAGE_FIELDS = frozenset({"path", "manifest_path", "digest"})
RELEASE_ARTIFACT_FIELDS = frozenset(
    {
        "path",
        "digest",
        "package_manifest_digest",
        "installed_guest_payload_digest",
        "architecture",
        "compatibility_version",
    }
)
RECEIPT_FIELDS = frozenset(
    {
        "schema_version",
        "kind",
        "workload_id",
        "image_digest",
        "release_artifact_digest",
        "package_manifest_digest",
        "installed_guest_payload_digest",
        "architecture",
        "compatibility_version",
        "cvd_version",
        "adb_version",
        "kvm_access",
        "recorded_at",
    }
)


class ReadinessError(Exception):
    """A malformed config or proof artifact."""


class Unavailable(Exception):
    """A prerequisite is absent or cannot currently be probed."""


def fail(message: str) -> NoReturn:
    print(f"cuttlefish-readiness: {message}", file=sys.stderr)
    raise SystemExit(2)


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ReadinessError(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def read_bounded_json(path: Path, maximum: int, label: str) -> dict[str, Any]:
    if not path.is_file() or path.is_symlink():
        raise Unavailable(f"{label}_missing")
    try:
        size = path.stat().st_size
        if size <= 0 or size > maximum:
            raise ReadinessError(f"{label} size is outside the bounded range")
        raw = path.read_bytes()
        value = json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=unique_object,
            parse_constant=lambda constant: (_ for _ in ()).throw(
                ReadinessError(f"non-finite JSON number: {constant}")
            ),
        )
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise Unavailable(f"{label}_unreadable") from error
    if not isinstance(value, dict):
        raise ReadinessError(f"{label} must be a JSON object")
    return value


def exact_fields(value: dict[str, Any], expected: frozenset[str], label: str) -> None:
    actual = frozenset(value)
    if actual != expected:
        omitted = sorted(expected - actual)
        unknown = sorted(actual - expected)
        raise ReadinessError(
            f"{label} fields are not exact (omitted={omitted}, unknown={unknown})"
        )


def bounded_string(value: Any, label: str, pattern: re.Pattern[str]) -> str:
    if not isinstance(value, str) or not value or len(value.encode("utf-8")) > MAX_ID_BYTES:
        raise ReadinessError(f"{label} is empty or oversized")
    if pattern.fullmatch(value) is None:
        raise ReadinessError(f"{label} contains unsafe characters")
    return value


def bounded_path(value: Any, label: str) -> Path:
    if not isinstance(value, str) or not value or "\x00" in value:
        raise ReadinessError(f"{label} is not a valid absolute path")
    if len(value.encode("utf-8")) > MAX_PATH_BYTES or not value.startswith("/"):
        raise ReadinessError(f"{label} is outside the bounded absolute-path form")
    parts = PurePosixPath(value).parts
    if ".." in parts or any(ord(character) < 0x20 for character in value):
        raise ReadinessError(f"{label} contains an unsafe path component")
    return Path(value)


def bounded_digest(value: Any, label: str) -> str:
    if not isinstance(value, str) or DIGEST_RE.fullmatch(value) is None:
        raise ReadinessError(f"{label} is not a lowercase sha256 digest")
    if value == "sha256:" + "0" * 64:
        raise ReadinessError(f"{label} is the null digest")
    return value


@dataclass(frozen=True)
class PlacementConfig:
    workload_id: str
    libvirt_uri: str
    pool: str
    network: str
    image_path: Path
    manifest_path: Path
    image_digest: str
    release_artifact_path: Path
    release_artifact_digest: str
    package_manifest_digest: str
    installed_guest_payload_digest: str
    architecture: str
    compatibility_version: str
    guest_readiness_receipt: Path


@dataclass(frozen=True)
class GuestToolReceipt:
    workload_id: str
    image_digest: str
    release_artifact_digest: str
    package_manifest_digest: str
    installed_guest_payload_digest: str
    architecture: str
    compatibility_version: str
    cvd_version: str
    adb_version: str
    recorded_at: datetime


def parse_config(value: dict[str, Any]) -> PlacementConfig:
    exact_fields(value, CONFIG_FIELDS, "placement config")
    if type(value["schema_version"]) is not int or value["schema_version"] != SCHEMA_VERSION:
        raise ReadinessError("unsupported placement config schema_version")
    if value["kind"] != CONFIG_KIND:
        raise ReadinessError("unsupported placement config kind")

    workload_id = bounded_string(value["workload_id"], "workload_id", IDENTITY_RE)
    libvirt_uri = value["libvirt_uri"]
    if not isinstance(libvirt_uri, str) or URI_RE.fullmatch(libvirt_uri) is None:
        raise ReadinessError("libvirt_uri is outside the closed local/SSH form")
    pool = bounded_string(value["pool"], "pool", IDENTITY_RE)
    network = bounded_string(value["network"], "network", IDENTITY_RE)

    base_image = value["base_image"]
    if not isinstance(base_image, dict):
        raise ReadinessError("base_image must be an object")
    exact_fields(base_image, BASE_IMAGE_FIELDS, "base_image")
    image_path = bounded_path(base_image["path"], "base_image.path")
    manifest_path = bounded_path(base_image["manifest_path"], "base_image.manifest_path")
    image_digest = bounded_digest(base_image["digest"], "base_image.digest")

    release_artifact = value["release_artifact"]
    if not isinstance(release_artifact, dict):
        raise ReadinessError("release_artifact must be an object")
    exact_fields(release_artifact, RELEASE_ARTIFACT_FIELDS, "release_artifact")

    return PlacementConfig(
        workload_id=workload_id,
        libvirt_uri=libvirt_uri,
        pool=pool,
        network=network,
        image_path=image_path,
        manifest_path=manifest_path,
        image_digest=image_digest,
        release_artifact_path=bounded_path(
            release_artifact["path"], "release_artifact.path"
        ),
        release_artifact_digest=bounded_digest(
            release_artifact["digest"], "release_artifact.digest"
        ),
        package_manifest_digest=bounded_digest(
            release_artifact["package_manifest_digest"],
            "release_artifact.package_manifest_digest",
        ),
        installed_guest_payload_digest=bounded_digest(
            release_artifact["installed_guest_payload_digest"],
            "release_artifact.installed_guest_payload_digest",
        ),
        architecture=bounded_string(
            release_artifact["architecture"], "release_artifact.architecture", IDENTITY_RE
        ),
        compatibility_version=bounded_string(
            release_artifact["compatibility_version"],
            "release_artifact.compatibility_version",
            VERSION_RE,
        ),
        guest_readiness_receipt=bounded_path(
            value["guest_readiness_receipt"], "guest_readiness_receipt"
        ),
    )


def parse_timestamp(value: Any, label: str) -> datetime:
    if not isinstance(value, str) or TIMESTAMP_RE.fullmatch(value) is None:
        raise ReadinessError(f"{label} is not a UTC timestamp")
    try:
        return datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    except ValueError as error:
        raise ReadinessError(f"{label} is not a calendar timestamp") from error


def parse_guest_receipt(
    value: dict[str, Any], config: PlacementConfig, now: datetime
) -> GuestToolReceipt:
    exact_fields(value, RECEIPT_FIELDS, "guest readiness receipt")
    if type(value["schema_version"]) is not int or value["schema_version"] != SCHEMA_VERSION:
        raise ReadinessError("unsupported guest readiness receipt schema_version")
    if value["kind"] != RECEIPT_KIND:
        raise ReadinessError("unsupported guest readiness receipt kind")
    workload_id = bounded_string(value["workload_id"], "receipt.workload_id", IDENTITY_RE)
    if workload_id != config.workload_id:
        raise ReadinessError("guest readiness receipt workload identity mismatch")
    image_digest = bounded_digest(value["image_digest"], "receipt.image_digest")
    if image_digest != config.image_digest:
        raise ReadinessError("guest readiness receipt image digest mismatch")
    release_artifact_digest = bounded_digest(
        value["release_artifact_digest"], "receipt.release_artifact_digest"
    )
    if release_artifact_digest != config.release_artifact_digest:
        raise ReadinessError("guest readiness receipt release artifact digest mismatch")
    package_manifest_digest = bounded_digest(
        value["package_manifest_digest"], "receipt.package_manifest_digest"
    )
    if package_manifest_digest != config.package_manifest_digest:
        raise ReadinessError("guest readiness receipt package manifest digest mismatch")
    installed_guest_payload_digest = bounded_digest(
        value["installed_guest_payload_digest"],
        "receipt.installed_guest_payload_digest",
    )
    if installed_guest_payload_digest != config.installed_guest_payload_digest:
        raise ReadinessError("guest readiness receipt installed payload digest mismatch")
    architecture = bounded_string(value["architecture"], "receipt.architecture", IDENTITY_RE)
    if architecture != config.architecture:
        raise ReadinessError("guest readiness receipt architecture mismatch")
    compatibility_version = bounded_string(
        value["compatibility_version"], "receipt.compatibility_version", VERSION_RE
    )
    if compatibility_version != config.compatibility_version:
        raise ReadinessError("guest readiness receipt compatibility mismatch")
    cvd_version = bounded_string(value["cvd_version"], "receipt.cvd_version", VERSION_RE)
    adb_version = bounded_string(value["adb_version"], "receipt.adb_version", VERSION_RE)
    if value["kvm_access"] is not True:
        raise ReadinessError("guest readiness receipt does not prove KVM access")
    recorded_at = parse_timestamp(value["recorded_at"], "receipt.recorded_at")
    age = (now - recorded_at).total_seconds()
    if age < -MAX_FUTURE_SKEW_SECONDS or age > MAX_RECEIPT_AGE_SECONDS:
        raise ReadinessError("guest readiness receipt is outside its freshness window")
    return GuestToolReceipt(
        workload_id=workload_id,
        image_digest=image_digest,
        release_artifact_digest=release_artifact_digest,
        package_manifest_digest=package_manifest_digest,
        installed_guest_payload_digest=installed_guest_payload_digest,
        architecture=architecture,
        compatibility_version=compatibility_version,
        cvd_version=cvd_version,
        adb_version=adb_version,
        recorded_at=recorded_at,
    )


@dataclass(frozen=True)
class CommandResult:
    returncode: int
    stdout: str


CommandRunner = Callable[[list[str]], Optional[CommandResult]]


def default_command_runner(argv: list[str]) -> CommandResult | None:
    try:
        completed = subprocess.run(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=8,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return CommandResult(completed.returncode, completed.stdout)


def passed(name: str) -> dict[str, str]:
    return {"name": name, "status": "passed", "reason": ""}


def unavailable(name: str, reason: str) -> dict[str, str]:
    return {"name": name, "status": "unavailable", "reason": reason}


def digest_regular_path(path: Path, maximum: int) -> str:
    before = os.stat(path, follow_symlinks=False)
    if not stat.S_ISREG(before.st_mode) or before.st_size <= 0 or before.st_size > maximum:
        raise Unavailable("artifact_not_regular")
    digest = hashlib.sha256()
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    with os.fdopen(descriptor, "rb") as stream:
        opened = os.fstat(stream.fileno())
        if (
            not stat.S_ISREG(opened.st_mode)
            or (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns)
            != (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
        ):
            raise Unavailable("artifact_changed_during_verification")
        for chunk in iter(lambda: stream.read(HASH_CHUNK_BYTES), b""):
            digest.update(chunk)
    after = os.stat(path, follow_symlinks=False)
    if (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns) != (
        before.st_dev,
        before.st_ino,
        before.st_size,
        before.st_mtime_ns,
    ):
        raise Unavailable("artifact_changed_during_verification")
    return "sha256:" + digest.hexdigest()


def check_release_artifact(config: PlacementConfig) -> dict[str, str]:
    try:
        artifact_digest = digest_regular_path(
            config.release_artifact_path, MAX_RELEASE_ARTIFACT_BYTES
        )
    except (OSError, Unavailable):
        return unavailable("release_artifact", "release_artifact_unavailable")
    if artifact_digest != config.release_artifact_digest:
        return unavailable("release_artifact", "release_artifact_digest_mismatch")
    try:
        manifest_digest = digest_regular_path(config.manifest_path, MAX_CONFIG_BYTES)
    except (OSError, Unavailable):
        return unavailable("release_artifact", "package_manifest_unavailable")
    if manifest_digest != config.package_manifest_digest:
        return unavailable("release_artifact", "package_manifest_digest_mismatch")
    return passed("release_artifact")


def check_image(config: PlacementConfig, runner: CommandRunner) -> dict[str, str]:
    try:
        before = os.stat(config.image_path, follow_symlinks=False)
    except OSError:
        return unavailable("base_image", "image_missing")
    if not stat.S_ISREG(before.st_mode):
        return unavailable("base_image", "image_not_regular_file")
    if before.st_size <= 0 or before.st_size > MAX_IMAGE_BYTES:
        return unavailable("base_image", "image_size_invalid")
    result = runner(
        [
            "qemu-img",
            "info",
            "--output=json",
            "--force-share",
            str(config.image_path),
        ]
    )
    if result is None:
        return unavailable("base_image", "qemu_img_unavailable")
    if result.returncode != 0:
        return unavailable("base_image", "qemu_img_failed")
    if len(result.stdout.encode("utf-8")) > MAX_COMMAND_OUTPUT_BYTES:
        return unavailable("base_image", "qemu_img_output_oversized")
    try:
        info = json.loads(result.stdout, object_pairs_hook=unique_object)
    except (json.JSONDecodeError, ReadinessError):
        return unavailable("base_image", "qemu_img_invalid")
    if not isinstance(info, dict) or info.get("format") != "qcow2":
        return unavailable("base_image", "image_not_qcow2")
    virtual_size = info.get("virtual-size")
    if type(virtual_size) is not int or not MIN_IMAGE_BYTES <= virtual_size <= MAX_IMAGE_BYTES:
        return unavailable("base_image", "image_virtual_size_invalid")
    digest = hashlib.sha256()
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(config.image_path, flags)
        with os.fdopen(descriptor, "rb") as stream:
            opened = os.fstat(stream.fileno())
            if (
                not stat.S_ISREG(opened.st_mode)
                or (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns)
                != (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
            ):
                return unavailable("base_image", "image_changed_during_verification")
            for chunk in iter(lambda: stream.read(HASH_CHUNK_BYTES), b""):
                digest.update(chunk)
        after = os.stat(config.image_path, follow_symlinks=False)
    except OSError:
        return unavailable("base_image", "image_unreadable")
    if (
        after.st_dev,
        after.st_ino,
        after.st_size,
        after.st_mtime_ns,
    ) != (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns):
        return unavailable("base_image", "image_changed_during_verification")
    if "sha256:" + digest.hexdigest() != config.image_digest:
        return unavailable("base_image", "image_digest_mismatch")
    return passed("base_image")


def manifest_verifier_path() -> Path:
    return Path(__file__).resolve().parents[1] / "packaging/android/verify-manifest.sh"


def check_manifest(
    config: PlacementConfig,
    runner: CommandRunner,
    verifier: Path,
) -> dict[str, str]:
    if not verifier.is_file() or verifier.is_symlink() or not os.access(verifier, os.X_OK):
        return unavailable("image_manifest", "manifest_verifier_unavailable")
    result = runner([str(verifier), str(config.manifest_path)])
    if result is None:
        return unavailable("image_manifest", "manifest_verifier_unavailable")
    if result.returncode != 0:
        return unavailable("image_manifest", "manifest_rejected")
    try:
        manifest = read_bounded_json(config.manifest_path, MAX_CONFIG_BYTES, "image_manifest")
        provenance = manifest.get("image_provenance")
        if not isinstance(provenance, dict) or provenance.get("image_digest") != config.image_digest:
            return unavailable("image_manifest", "image_digest_mismatch")
    except (ReadinessError, Unavailable):
        return unavailable("image_manifest", "manifest_unreadable")
    return passed("image_manifest")


KvmOpener = Callable[[Path], int]


def default_kvm_opener(path: Path) -> int:
    flags = os.O_RDWR | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    return os.open(path, flags)


def check_kvm(
    kvm_path: Path, opener: KvmOpener = default_kvm_opener
) -> dict[str, str]:
    try:
        mode = os.stat(kvm_path, follow_symlinks=False).st_mode
    except OSError:
        return unavailable("kvm", "kvm_device_missing")
    if not stat.S_ISCHR(mode):
        return unavailable("kvm", "kvm_device_not_character_device")
    try:
        descriptor = opener(kvm_path)
    except OSError:
        return unavailable("kvm", "kvm_access_denied")
    try:
        if not stat.S_ISCHR(os.fstat(descriptor).st_mode):
            return unavailable("kvm", "kvm_device_changed_during_verification")
    finally:
        os.close(descriptor)
    return passed("kvm")


def check_active_libvirt_resource(
    *,
    name: str,
    check_name: str,
    argv: list[str],
    runner: CommandRunner,
) -> dict[str, str]:
    result = runner(argv)
    if result is None:
        return unavailable(check_name, "virsh_unavailable")
    if result.returncode != 0:
        return unavailable(check_name, f"{check_name}_unavailable")
    if len(result.stdout.encode("utf-8")) > MAX_COMMAND_OUTPUT_BYTES:
        return unavailable(check_name, f"{check_name}_output_oversized")
    active_names = {line.strip() for line in result.stdout.splitlines() if line.strip()}
    if name not in active_names:
        return unavailable(check_name, f"{check_name}_inactive")
    return passed(check_name)


def check_libvirt(config: PlacementConfig, runner: CommandRunner) -> list[dict[str, str]]:
    return [
        check_active_libvirt_resource(
            name=config.pool,
            check_name="libvirt_pool",
            argv=["virsh", "--connect", config.libvirt_uri, "pool-list", "--name"],
            runner=runner,
        ),
        check_active_libvirt_resource(
            name=config.network,
            check_name="libvirt_network",
            argv=["virsh", "--connect", config.libvirt_uri, "net-list", "--name"],
            runner=runner,
        ),
    ]


def check_guest_tools(
    config: PlacementConfig, now: datetime
) -> dict[str, str]:
    try:
        receipt = read_bounded_json(
            config.guest_readiness_receipt,
            MAX_RECEIPT_BYTES,
            "guest_readiness_receipt",
        )
        parse_guest_receipt(receipt, config, now)
    except Unavailable as error:
        return unavailable("guest_tools", str(error))
    except ReadinessError:
        return unavailable("guest_tools", "guest_receipt_invalid")
    return passed("guest_tools")


def readiness_report(
    config: PlacementConfig,
    *,
    runner: CommandRunner = default_command_runner,
    verifier: Path | None = None,
    kvm_path: Path = Path("/dev/kvm"),
    kvm_opener: KvmOpener = default_kvm_opener,
    now: datetime | None = None,
) -> dict[str, Any]:
    now = now or datetime.now(timezone.utc)
    checks = [
        check_image(config, runner),
        check_manifest(config, runner, verifier or manifest_verifier_path()),
        check_release_artifact(config),
        check_kvm(kvm_path, kvm_opener),
        *check_libvirt(config, runner),
        check_guest_tools(config, now),
    ]
    ready = all(check["status"] == "passed" for check in checks)
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": REPORT_KIND,
        "workload_id": config.workload_id,
        "status": "ready_for_provisioning" if ready else "unavailable",
        "provisioning_eligible": ready,
        "live_android_guest_proof": "unavailable",
        "checks": checks,
    }


def self_test() -> None:
    import tempfile

    image_bytes = b"fixture"
    digest = "sha256:" + hashlib.sha256(image_bytes).hexdigest()
    release_bytes = b"signed Cuttlefish release artifact fixture"
    release_digest = "sha256:" + hashlib.sha256(release_bytes).hexdigest()
    payload_digest = "sha256:" + hashlib.sha256(b"installed cvd and adb payload").hexdigest()
    now = datetime(2026, 8, 5, 12, 0, 0, tzinfo=timezone.utc)
    with tempfile.TemporaryDirectory(prefix="cuttlefish-readiness-") as temporary:
        root = Path(temporary)
        image = root / "base.qcow2"
        manifest = root / "manifest.json"
        release_artifact = root / "cuttlefish-guest-tools.deb"
        receipt = root / "guest-receipt.json"
        image.write_bytes(image_bytes)
        release_artifact.write_bytes(release_bytes)

        starter = [
            ("browser", "com.android.browser"),
            ("calendar", "com.android.calendar"),
            ("camera", "com.android.camera2"),
            ("clock", "com.android.deskclock"),
            ("contacts", "com.android.contacts"),
            ("files", "com.android.documentsui"),
            ("gallery", "com.android.gallery3d"),
            ("calculator", "com.android.calculator2"),
            ("settings", "com.android.settings"),
        ]
        manifest.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "image_provenance": {
                        "image_id": "aosp-cuttlefish-test",
                        "image_digest": digest,
                        "source_revision": "aosp-source-test",
                        "catalog_revision": "starter-catalog-v1",
                    },
                    "packages": [
                        {
                            "app": app,
                            "package_id": package_id,
                            "version": {"version_name": "2026.08.1", "version_code": 1},
                        }
                        for app, package_id in starter
                    ],
                }
            ),
            encoding="utf-8",
        )
        manifest_digest = "sha256:" + hashlib.sha256(manifest.read_bytes()).hexdigest()
        receipt.write_text(
            json.dumps(
                {
                    "schema_version": SCHEMA_VERSION,
                    "kind": RECEIPT_KIND,
                    "workload_id": "android-seat-15",
                    "image_digest": digest,
                    "release_artifact_digest": release_digest,
                    "package_manifest_digest": manifest_digest,
                    "installed_guest_payload_digest": payload_digest,
                    "architecture": "x86_64",
                    "compatibility_version": "debian:13",
                    "cvd_version": "2026.08.1",
                    "adb_version": "1.0.41",
                    "kvm_access": True,
                    "recorded_at": "2026-08-05T12:00:00Z",
                }
            ),
            encoding="utf-8",
        )
        verifier = manifest_verifier_path()
        config_value = {
            "schema_version": SCHEMA_VERSION,
            "kind": CONFIG_KIND,
            "workload_id": "android-seat-15",
            "libvirt_uri": "qemu:///system",
            "pool": "mde-vms",
            "network": "mde-cloud",
            "base_image": {
                "path": str(image),
                "manifest_path": str(manifest),
                "digest": digest,
            },
            "release_artifact": {
                "path": str(release_artifact),
                "digest": release_digest,
                "package_manifest_digest": manifest_digest,
                "installed_guest_payload_digest": payload_digest,
                "architecture": "x86_64",
                "compatibility_version": "debian:13",
            },
            "guest_readiness_receipt": str(receipt),
        }
        config = parse_config(config_value)
        calls: list[list[str]] = []

        def fake_runner(argv: list[str]) -> CommandResult | None:
            calls.append(argv)
            if argv[0] == "qemu-img":
                return CommandResult(
                    0,
                    json.dumps({"format": "qcow2", "virtual-size": MIN_IMAGE_BYTES}),
                )
            if argv[0] == "virsh":
                if argv[-2:] == ["pool-list", "--name"]:
                    return CommandResult(0, config.pool + "\n")
                if argv[-2:] == ["net-list", "--name"]:
                    return CommandResult(0, config.network + "\n")
                raise AssertionError(f"unexpected virsh command: {argv}")
            if argv[0] == str(verifier):
                return default_command_runner(argv)
            raise AssertionError(f"unexpected command: {argv}")

        report = readiness_report(
            config,
            runner=fake_runner,
            verifier=verifier,
            kvm_path=Path("/dev/null"),
            now=now,
        )
        assert report["status"] == "ready_for_provisioning"
        assert report["provisioning_eligible"] is True
        assert report["live_android_guest_proof"] == "unavailable"
        assert all(check["status"] == "passed" for check in report["checks"])
        assert calls[0][:4] == ["qemu-img", "info", "--output=json", "--force-share"]
        assert ["virsh", "--connect", "qemu:///system", "pool-list", "--name"] in calls
        assert ["virsh", "--connect", "qemu:///system", "net-list", "--name"] in calls
        assert all("shell" not in argument for call in calls for argument in call)

        legacy_config = json.loads(json.dumps(config_value))
        legacy_config["schema_version"] = 1
        try:
            parse_config(legacy_config)
        except ReadinessError:
            pass
        else:
            raise AssertionError("legacy placement schema was accepted")

        release_artifact.unlink()
        report = readiness_report(
            config,
            runner=fake_runner,
            verifier=verifier,
            kvm_path=Path("/dev/null"),
            now=now,
        )
        assert {
            check["reason"]
            for check in report["checks"]
            if check["name"] == "release_artifact"
        } == {"release_artifact_unavailable"}
        assert report["status"] == "unavailable"
        assert report["provisioning_eligible"] is False
        release_artifact.write_bytes(release_bytes)

        image.write_bytes(b"tampered")
        report = readiness_report(
            config,
            runner=fake_runner,
            verifier=verifier,
            kvm_path=Path("/dev/null"),
            now=now,
        )
        assert {check["reason"] for check in report["checks"] if check["name"] == "base_image"} == {
            "image_digest_mismatch"
        }
        image.write_bytes(image_bytes)

        release_artifact.write_bytes(b"substituted release artifact")
        report = readiness_report(
            config,
            runner=fake_runner,
            verifier=verifier,
            kvm_path=Path("/dev/null"),
            now=now,
        )
        assert {
            check["reason"]
            for check in report["checks"]
            if check["name"] == "release_artifact"
        } == {"release_artifact_digest_mismatch"}
        assert report["provisioning_eligible"] is False
        release_artifact.write_bytes(release_bytes)

        def denied_kvm(_path: Path) -> int:
            raise PermissionError("fixture denies KVM")

        report = readiness_report(
            config,
            runner=fake_runner,
            verifier=verifier,
            kvm_path=Path("/dev/null"),
            kvm_opener=denied_kvm,
            now=now,
        )
        assert {check["reason"] for check in report["checks"] if check["name"] == "kvm"} == {
            "kvm_access_denied"
        }

        def inactive_pool_runner(argv: list[str]) -> CommandResult | None:
            if argv[0] == "virsh" and argv[-2:] == ["pool-list", "--name"]:
                return CommandResult(0, "")
            return fake_runner(argv)

        report = readiness_report(
            config,
            runner=inactive_pool_runner,
            verifier=verifier,
            kvm_path=Path("/dev/null"),
            now=now,
        )
        assert {
            check["reason"] for check in report["checks"] if check["name"] == "libvirt_pool"
        } == {"libvirt_pool_inactive"}

        invalid_manifest = json.loads(manifest.read_text(encoding="utf-8"))
        invalid_manifest["packages"].pop()
        manifest.write_text(json.dumps(invalid_manifest), encoding="utf-8")
        report = readiness_report(
            config,
            runner=fake_runner,
            verifier=verifier,
            kvm_path=Path("/dev/null"),
            now=now,
        )
        assert {
            check["reason"] for check in report["checks"] if check["name"] == "image_manifest"
        } == {"manifest_rejected"}
        assert {
            check["reason"]
            for check in report["checks"]
            if check["name"] == "release_artifact"
        } == {"package_manifest_digest_mismatch"}
        assert report["provisioning_eligible"] is False

        mismatched_manifest = json.loads(
            json.dumps(
                {
                    "schema_version": 1,
                    "image_provenance": {
                        "image_id": "aosp-cuttlefish-test",
                        "image_digest": "sha256:" + "b" * 64,
                        "source_revision": "aosp-source-test",
                        "catalog_revision": "starter-catalog-v1",
                    },
                    "packages": [
                        {
                            "app": app,
                            "package_id": package_id,
                            "version": {"version_name": "2026.08.1", "version_code": 1},
                        }
                        for app, package_id in starter
                    ],
                }
            )
        )
        manifest.write_text(json.dumps(mismatched_manifest), encoding="utf-8")
        report = readiness_report(
            config,
            runner=fake_runner,
            verifier=verifier,
            kvm_path=Path("/dev/null"),
            now=now,
        )
        assert {
            check["reason"] for check in report["checks"] if check["name"] == "image_manifest"
        } == {"image_digest_mismatch"}

        manifest.write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "image_provenance": {
                        "image_id": "aosp-cuttlefish-test",
                        "image_digest": digest,
                        "source_revision": "aosp-source-test",
                        "catalog_revision": "starter-catalog-v1",
                    },
                    "packages": [
                        {
                            "app": app,
                            "package_id": package_id,
                            "version": {"version_name": "2026.08.1", "version_code": 1},
                        }
                        for app, package_id in starter
                    ],
                }
            ),
            encoding="utf-8",
        )

        hostile_receipt = json.loads(receipt.read_text(encoding="utf-8"))
        hostile_receipt["installed_guest_payload_digest"] = "sha256:" + "c" * 64
        receipt.write_text(json.dumps(hostile_receipt), encoding="utf-8")
        report = readiness_report(
            config,
            runner=fake_runner,
            verifier=verifier,
            kvm_path=Path("/dev/null"),
            now=now,
        )
        assert {check["reason"] for check in report["checks"] if check["name"] == "guest_tools"} == {
            "guest_receipt_invalid"
        }
        assert report["provisioning_eligible"] is False

        hostile_receipt["installed_guest_payload_digest"] = payload_digest
        hostile_receipt["compatibility_version"] = "debian:12"
        receipt.write_text(json.dumps(hostile_receipt), encoding="utf-8")
        report = readiness_report(
            config,
            runner=fake_runner,
            verifier=verifier,
            kvm_path=Path("/dev/null"),
            now=now,
        )
        assert {check["reason"] for check in report["checks"] if check["name"] == "guest_tools"} == {
            "guest_receipt_invalid"
        }
        receipt.unlink()

        report = readiness_report(
            config,
            runner=fake_runner,
            verifier=verifier,
            kvm_path=Path("/dev/null"),
            now=now,
        )
        assert report["status"] == "unavailable"
        assert report["provisioning_eligible"] is False
        assert {check["reason"] for check in report["checks"] if check["name"] == "guest_tools"} == {
            "guest_readiness_receipt_missing"
        }

        image.unlink()
        report = readiness_report(
            config,
            runner=fake_runner,
            verifier=verifier,
            kvm_path=Path("/dev/null"),
            now=now,
        )
        assert {check["reason"] for check in report["checks"] if check["name"] == "base_image"} == {
            "image_missing"
        }

        invalid = dict(config_value)
        invalid["unexpected"] = True
        try:
            parse_config(invalid)
        except ReadinessError:
            pass
        else:
            raise AssertionError("unknown config field was accepted")
    print("verify-cuttlefish-readiness: self-test passed")


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        self_test()
        return 0
    if len(argv) != 2 or argv[0] != "--config":
        print("usage: verify-cuttlefish-readiness.py --config CONFIG | --self-test", file=sys.stderr)
        return 2
    try:
        config = parse_config(
            read_bounded_json(Path(argv[1]), MAX_CONFIG_BYTES, "placement_config")
        )
    except (ReadinessError, Unavailable) as error:
        fail(str(error))
    report = readiness_report(config)
    print(json.dumps(report, separators=(",", ":"), sort_keys=True))
    return 0 if report["status"] == "ready_for_provisioning" else 3


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
