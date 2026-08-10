#!/usr/bin/python3
"""Collect bounded, read-only Surface Pro 5/6 acceptance evidence.

This tool inventories a seat.  It never changes power state, service state,
firmware, networking, input devices, cameras, audio, or the installed image.
It deliberately cannot claim the physical touch/pen/audio/camera experience;
the manifest identifies the manual checks that still need an operator.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import selectors
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


SCHEMA_VERSION = 1
MAX_COMMAND_BYTES = 256 * 1024
MAX_ARTIFACT_BYTES = 512 * 1024
MAX_BUNDLE_BYTES = 4 * 1024 * 1024
COMMAND_TIMEOUT_SECONDS = 12.0
MAX_SYSFS_ENTRIES = 128
ALLOWED_SEAT = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$")
EXPECTED_FILES = (
    "identity.json",
    "release-packages.json",
    "kernel-modules.json",
    "iptsd.json",
    "input.json",
    "buttons-storage.json",
    "sam-iio.json",
    "drm.json",
    "cameras.json",
    "radios.json",
    "firmware.json",
    "audio.json",
    "power.json",
    "services.json",
)
REQUIRED_PACKAGES = (
    "magic-mesh",
    "kernel-surface",
    "iptsd",
    "libwacom-surface",
    "surface-control",
    "surface-secureboot",
    "fwupd",
)
SURFACE_MODULES = (
    "surface_aggregator",
    "surface_aggregator_registry",
    "surface_hid_core",
    "surface_hid",
    "surface_kbd",
    "surface_gpe",
    "surface_platform_profile",
    "surfacepro3_button",
    "intel_ish_ipc",
    "hid_multitouch",
    "ipts",
)
SERVICE_UNITS = (
    "mackesd.service",
    "mde-shell-egui.service",
    "nebula.service",
    "syncthing.service",
    "NetworkManager.service",
    "bluetooth.service",
    "upower.service",
    "fwupd.service",
)
MANUAL_CHECKS = (
    "ten-finger touch accuracy and edge gestures",
    "pen hover, pressure, eraser, and palm rejection",
    "Type Cover attach, detach, keyboard, touchpad, and backlight",
    "power/volume buttons and microSD insertion, read, eject, and reinsertion",
    "portrait and landscape rotation with correct touch transform",
    "camera preview and privacy indication (collector captures no frames)",
    "speaker, microphone, headphone, and Bluetooth audio judgement",
    "suspend/resume, S0ix residency delta, Wi-Fi, Bluetooth, and mesh recovery",
    "cold boot, reboot, upgrade, rollback, and secure-boot recovery",
)


class CollectError(RuntimeError):
    pass


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def clean_text(value: str, limit: int = 4096) -> str:
    """Remove identifiers/secrets and bound a human-readable field."""
    value = value.replace("\x00", "")
    value = re.sub(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]", "?", value)
    value = re.sub(r"(?i)\b(?:[0-9a-f]{2}:){5}[0-9a-f]{2}\b", "[REDACTED-MAC]", value)
    value = re.sub(r"(?<![0-9])(?:[0-9]{1,3}\.){3}[0-9]{1,3}(?![0-9])", "[REDACTED-IP]", value)
    value = re.sub(r"(?i)\b(?:[0-9a-f]{0,4}:){2,7}[0-9a-f]{0,4}\b", "[REDACTED-IP]", value)
    value = re.sub(r"(?i)\b(bearer|token|password|secret|private[-_ ]?key)\s*[:=]\s*\S+", r"\1=[REDACTED]", value)
    value = re.sub(r"(?i)\b(ssid|connection)\s*[:=]\s*[^\s,;]+", r"\1=[REDACTED]", value)
    value = re.sub(r"(?i)\b(https?|ssh)://[^/\s:@]+:[^@\s/]+@", r"\1://[REDACTED]@", value)
    value = re.sub(r"\bgh[pousr]_[A-Za-z0-9_]{20,}\b", "[REDACTED-TOKEN]", value)
    value = re.sub(r"\b[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b", "[REDACTED-TOKEN]", value)
    return value[:limit]


def scrub_json(value: Any, depth: int = 0) -> Any:
    """Recursively bound and redact selected command JSON."""
    if depth > 8:
        raise CollectError("JSON nesting exceeds eight levels")
    if value is None or isinstance(value, (bool, int, float)):
        return value
    if isinstance(value, str):
        return clean_text(value, 4096)
    if isinstance(value, list):
        if len(value) > 256:
            raise CollectError("JSON array exceeds 256 entries")
        return [scrub_json(item, depth + 1) for item in value]
    if isinstance(value, dict):
        if len(value) > 128:
            raise CollectError("JSON object exceeds 128 keys")
        result_value = {}
        for key, item in value.items():
            if not isinstance(key, str) or len(key) > 128:
                raise CollectError("JSON object key is invalid")
            result_value[clean_text(key, 128)] = scrub_json(item, depth + 1)
        return result_value
    raise CollectError("JSON contains an unsupported value")


def read_limited(path: Path, limit: int = 65536) -> str:
    try:
        if path.is_symlink() and not str(path).startswith("/sys/"):
            raise CollectError(f"refusing non-sysfs symlink: {path}")
        with path.open("rb") as stream:
            data = stream.read(limit + 1)
    except (FileNotFoundError, PermissionError, OSError) as exc:
        raise CollectError(f"cannot read {path}: {exc}") from exc
    if len(data) > limit:
        raise CollectError(f"input exceeds {limit} bytes: {path}")
    return data.decode("utf-8", errors="strict").strip()


def run_fixed(argv: tuple[str, ...], timeout: float = COMMAND_TIMEOUT_SECONDS) -> dict[str, Any]:
    """Run an absolute, constant command with bounded pipes and a deadline."""
    if not argv or not argv[0].startswith("/"):
        raise CollectError("command executable must be an absolute path")
    if not Path(argv[0]).is_file():
        return {"status": "unavailable", "reason": f"command absent: {argv[0]}"}
    env = {"PATH": "/usr/sbin:/usr/bin", "LANG": "C", "LC_ALL": "C"}
    try:
        proc = subprocess.Popen(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            start_new_session=True,
        )
    except OSError as exc:
        return {"status": "error", "reason": clean_text(str(exc))}
    assert proc.stdout is not None and proc.stderr is not None
    selector = selectors.DefaultSelector()
    selector.register(proc.stdout, selectors.EVENT_READ, "stdout")
    selector.register(proc.stderr, selectors.EVENT_READ, "stderr")
    chunks: dict[str, bytearray] = {"stdout": bytearray(), "stderr": bytearray()}
    deadline = time.monotonic() + timeout
    failure = ""
    while selector.get_map():
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            failure = "command timed out"
            break
        events = selector.select(min(remaining, 0.25))
        for key, _ in events:
            piece = os.read(key.fileobj.fileno(), 16384)
            if not piece:
                selector.unregister(key.fileobj)
                continue
            chunks[key.data].extend(piece)
            if sum(len(v) for v in chunks.values()) > MAX_COMMAND_BYTES:
                failure = f"command output exceeded {MAX_COMMAND_BYTES} bytes"
                break
        if failure:
            break
        if proc.poll() is not None and not events:
            for key in list(selector.get_map().values()):
                piece = os.read(key.fileobj.fileno(), 16384)
                if piece:
                    chunks[key.data].extend(piece)
                else:
                    selector.unregister(key.fileobj)
    if failure:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    try:
        returncode = proc.wait(timeout=2)
    except subprocess.TimeoutExpired:
        proc.kill()
        returncode = proc.wait()
    stdout = bytes(chunks["stdout"][:MAX_COMMAND_BYTES]).decode("utf-8", errors="replace")
    stderr = bytes(chunks["stderr"][:MAX_COMMAND_BYTES]).decode("utf-8", errors="replace")
    if failure:
        return {"status": "error", "reason": failure, "returncode": returncode}
    if returncode != 0:
        return {
            "status": "error",
            "reason": f"command exited {returncode}",
            "stderr": clean_text(stderr, 2048),
            "returncode": returncode,
        }
    return {"status": "ok", "stdout": stdout, "stderr": clean_text(stderr, 2048), "returncode": 0}


def result(status: str, data: Any = None, reason: str | None = None) -> dict[str, Any]:
    value: dict[str, Any] = {"schema_version": SCHEMA_VERSION, "status": status}
    if data is not None:
        value["data"] = data
    if reason:
        value["reason"] = clean_text(reason)
    return value


def read_optional(path: Path, limit: int = 4096) -> str | None:
    try:
        return clean_text(read_limited(path, limit), limit)
    except CollectError:
        return None


def detect_generation(vendor: str | None, model: str, sku: str | None, expected: int) -> int | None:
    if vendor != "Microsoft Corporation":
        return None
    if expected == 5:
        # Surface Pro 5 uses the generic product name. The SKU is the exact
        # generation/variant discriminator: 1796 is Wi-Fi and 1807 is LTE.
        return 5 if model == "Surface Pro" and sku in {"Surface_Pro_1796", "Surface_Pro_1807"} else None
    return 6 if model == "Surface Pro 6" else None


def probe_identity(expected: int, seat: str) -> dict[str, Any]:
    model = read_optional(Path("/sys/class/dmi/id/product_name"))
    if model is None:
        return result("error", reason="DMI product_name is unavailable")
    vendor = read_optional(Path("/sys/class/dmi/id/sys_vendor"))
    sku = read_optional(Path("/sys/class/dmi/id/product_sku"))
    generation = detect_generation(vendor, model, sku, expected)
    data = {
        "seat_label": seat,
        "manufacturer": vendor,
        "product_name": model,
        "product_version": read_optional(Path("/sys/class/dmi/id/product_version")),
        "product_sku": sku,
        "expected_generation": expected,
        "detected_generation": generation,
    }
    if generation is None:
        return result("error", data, f"DMI vendor/model/SKU is not an allowlisted Surface Pro {expected}")
    return result("ok", data)


def parse_os_release() -> dict[str, str]:
    allowed = {"ID", "VERSION_ID", "VARIANT_ID", "IMAGE_ID", "IMAGE_VERSION", "OSTREE_VERSION"}
    values: dict[str, str] = {}
    text = read_limited(Path("/usr/lib/os-release"), 32768)
    for line in text.splitlines():
        if "=" not in line or line.startswith("#"):
            continue
        key, raw = line.split("=", 1)
        if key not in allowed:
            continue
        values[key] = clean_text(raw.strip().strip('"').strip("'"), 256)
    return values


def probe_release_packages() -> dict[str, Any]:
    try:
        release = parse_os_release()
    except CollectError as exc:
        return result("error", reason=str(exc))
    packages: list[dict[str, Any]] = []
    failed: list[str] = []
    for package in REQUIRED_PACKAGES:
        command = run_fixed(("/usr/bin/rpm", "-q", "--qf", "%{NAME}|%{EPOCHNUM}|%{VERSION}|%{RELEASE}|%{ARCH}\\n", package))
        if command["status"] != "ok":
            packages.append({"name": package, "status": "unavailable"})
            failed.append(package)
            continue
        rows = []
        for line in command["stdout"].splitlines()[:16]:
            parts = line.split("|")
            if len(parts) == 5 and parts[0] == package:
                rows.append({"name": clean_text(parts[0], 256), "epoch": clean_text(parts[1], 64), "version": clean_text(parts[2], 256), "release": clean_text(parts[3], 256), "arch": clean_text(parts[4], 64)})
        if not rows:
            failed.append(package)
        packages.append({"name": package, "status": "installed" if rows else "error", "nevra": rows})
    deployment = {"status": "unavailable", "reason": "no allowlisted immutable-deployment client installed"}
    for argv in (("/usr/bin/bootc", "status", "--json"), ("/usr/bin/rpm-ostree", "status", "--json")):
        if not Path(argv[0]).is_file():
            continue
        command = run_fixed(argv)
        if command["status"] != "ok":
            deployment = {"status": command["status"], "command": argv[0], "reason": command.get("reason", "unavailable")}
        else:
            try:
                deployment = {"status": "ok", "command": argv[0], "inventory": scrub_json(json.loads(command["stdout"]))}
            except (json.JSONDecodeError, CollectError) as exc:
                deployment = {"status": "error", "command": argv[0], "reason": clean_text(str(exc))}
        break
    data = {"os_release": release, "immutable_deployment": deployment, "packages": packages}
    if release.get("ID") != "fedora" or release.get("VERSION_ID") != "44":
        return result("error", data, "collector requires Fedora 44")
    if failed:
        return result("error", data, "required packages unavailable: " + ", ".join(failed))
    if deployment["status"] != "ok":
        return result("error", data, "immutable deployment identity is unavailable")
    return result("ok", data)


def probe_kernel_modules() -> dict[str, Any]:
    uname = run_fixed(("/usr/bin/uname", "-r"))
    modules: list[dict[str, Any]] = []
    loaded_count = 0
    unsigned_loaded: list[str] = []
    for name in SURFACE_MODULES:
        loaded = Path("/sys/module", name).is_dir()
        item: dict[str, Any] = {"name": name, "loaded": loaded}
        if loaded:
            loaded_count += 1
        for field in ("filename", "signer", "version"):
            command = run_fixed(("/usr/sbin/modinfo", "-F", field, name))
            item[field] = clean_text(command.get("stdout", "").strip(), 1024) if command["status"] == "ok" else None
        if loaded and not item["signer"]:
            unsigned_loaded.append(name)
        modules.append(item)
    data = {
        "kernel_release": clean_text(uname.get("stdout", "").strip(), 256) if uname["status"] == "ok" else None,
        "modules": modules,
        "loaded_module_count": loaded_count,
        "unsigned_loaded_modules": unsigned_loaded,
        "secure_boot_state": command_lines(("/usr/bin/mokutil", "--sb-state"), 16),
        "surface_certificate": file_descriptor(Path("/usr/share/surface-secureboot/surface.cer")),
    }
    required_loaded = {"surface_aggregator", "ipts"}
    loaded_names = {item["name"] for item in modules if item["loaded"]}
    secure_boot_ok = data["secure_boot_state"].get("status") == "ok"
    certificate_ok = data["surface_certificate"].get("status") == "ok"
    if uname["status"] != "ok" or not required_loaded.issubset(loaded_names) or unsigned_loaded or not secure_boot_ok or not certificate_ok:
        return result("error", data, "kernel/module provenance is incomplete")
    return result("ok", data)


def command_lines(argv: tuple[str, ...], max_lines: int) -> dict[str, Any]:
    command = run_fixed(argv)
    if command["status"] != "ok":
        return {"status": command["status"], "reason": command.get("reason", "unavailable")}
    return {"status": "ok", "lines": [clean_text(line, 2048) for line in command["stdout"].splitlines()[:max_lines]]}


def file_descriptor(path: Path) -> dict[str, Any]:
    try:
        size = path.stat().st_size
        if size > MAX_ARTIFACT_BYTES or not path.is_file():
            raise CollectError("not a bounded regular file")
        return {"status": "ok", "path": str(path), "size": size, "sha256": sha256_file(path)}
    except (CollectError, OSError) as exc:
        return {"status": "unavailable", "path": str(path), "reason": clean_text(str(exc))}


def probe_iptsd() -> dict[str, Any]:
    template = command_lines(("/usr/bin/systemctl", "show", "iptsd@.service", "--property=LoadState,FragmentPath", "--no-pager"), 16)
    units = run_fixed(("/usr/bin/systemctl", "list-units", "--type=service", "--state=active", "--no-legend", "--no-pager", "iptsd@*.service"))
    active = []
    if units["status"] == "ok":
        for line in units["stdout"].splitlines()[:64]:
            fields = line.split()
            if fields and re.fullmatch(r"iptsd@[A-Za-z0-9_.\\x-]+\.service", fields[0]):
                active.append(fields[0])
    data = {"template": template, "active_instances": sorted(set(active))}
    return result("ok" if active else "error", data, None if active else "no active iptsd instance")


def probe_input() -> dict[str, Any]:
    try:
        text = read_limited(Path("/proc/bus/input/devices"), MAX_ARTIFACT_BYTES)
    except CollectError as exc:
        return result("error", reason=str(exc))
    devices: list[dict[str, Any]] = []
    for block in text.split("\n\n")[:MAX_SYSFS_ENTRIES]:
        item: dict[str, Any] = {}
        for line in block.splitlines():
            if line.startswith("N: Name="):
                item["name"] = clean_text(line.removeprefix("N: Name=").strip('"'), 512)
            elif line.startswith("H: Handlers="):
                item["handlers"] = clean_text(line.removeprefix("H: Handlers="), 512).split()[:32]
            elif line.startswith("B: EV="):
                item["event_bits"] = clean_text(line.removeprefix("B: EV="), 128)
        if item:
            devices.append(item)
    names = " ".join(str(d.get("name", "")) for d in devices).lower()
    classes = {
        "touch_candidate": any(token in names for token in ("touch", "ipts", "digitizer")),
        "pen_candidate": any(token in names for token in ("pen", "stylus", "ipts", "digitizer")),
        "type_cover_candidate": any(token in names for token in ("type cover", "surface keyboard", "surface type")),
    }
    missing_classes = sorted(name for name, present in classes.items() if not present)
    status = "ok" if devices and not missing_classes else "error"
    reason = None if status == "ok" else "missing input candidates: " + ", ".join(missing_classes)
    return result(status, {"devices": devices, "classes": classes}, reason)


def probe_buttons_storage() -> dict[str, Any]:
    """Inventory button candidates and MMC topology without opening media."""
    button_names = []
    try:
        text = read_limited(Path("/proc/bus/input/devices"), MAX_ARTIFACT_BYTES)
        for block in text.split("\n\n")[:MAX_SYSFS_ENTRIES]:
            for line in block.splitlines():
                if line.startswith("N: Name="):
                    name = clean_text(line.removeprefix("N: Name=").strip('"'), 512)
                    if any(token in name.lower() for token in ("button", "power", "volume")):
                        button_names.append(name)
    except CollectError as exc:
        return result("error", reason=str(exc))
    mmc_hosts = []
    mmc_blocks = []
    try:
        mmc_hosts = [path.name for path in bounded_glob("/sys/class/mmc_host/mmc*")]
        for path in bounded_glob("/sys/class/block/mmcblk*"):
            # Partitions are represented by names containing 'p'; the parent
            # block device is enough to prove reader/media enumeration.
            if "p" in path.name:
                continue
            mmc_blocks.append({
                "block_device": path.name,
                "removable": read_optional(path / "removable", 16),
                "read_only": read_optional(path / "ro", 16),
                "size_512b_sectors": read_optional(path / "size", 64),
            })
    except CollectError as exc:
        return result("error", {"button_candidates": sorted(set(button_names)), "mmc_hosts": mmc_hosts, "mmc_block_devices": mmc_blocks}, str(exc))
    data = {
        "button_candidates": sorted(set(button_names)),
        "mmc_hosts": mmc_hosts,
        "mmc_block_devices": mmc_blocks,
        "note": "inventory only; button presses and media reads are not performed",
    }
    # An empty removable-media slot can legitimately have a host and no block
    # device. Require both a button candidate and the reader host.
    status = "ok" if button_names and mmc_hosts else "error"
    return result(status, data, None if status == "ok" else "button or MMC-reader inventory is incomplete")


def bounded_glob(pattern: str) -> list[Path]:
    paths = sorted(Path("/").glob(pattern.lstrip("/")))
    if len(paths) > MAX_SYSFS_ENTRIES:
        raise CollectError(f"sysfs match count exceeds {MAX_SYSFS_ENTRIES}: {pattern}")
    return paths


def probe_sam_iio() -> dict[str, Any]:
    iio = []
    platform = []
    try:
        for path in bounded_glob("/sys/bus/iio/devices/iio:device*"):
            iio.append({"device": path.name, "name": read_optional(path / "name", 512)})
        for path in bounded_glob("/sys/bus/platform/devices/*"):
            name = path.name
            lowered = name.lower()
            if any(token in lowered for token in ("surface", "ssam", "sam", "aggregator")):
                platform.append(clean_text(name, 512))
                if len(platform) >= 64:
                    break
    except CollectError as exc:
        return result("error", {"iio_devices": iio, "surface_platform_devices": platform}, str(exc))
    loaded = [name for name in SURFACE_MODULES if Path("/sys/module", name).is_dir() and ("surface" in name or name == "ipts")]
    profile = {
        "current": read_optional(Path("/sys/firmware/acpi/platform_profile"), 128),
        "choices": (read_optional(Path("/sys/firmware/acpi/platform_profile_choices"), 1024) or "").split()[:32],
    }
    required_loaded = {"surface_aggregator", "ipts"}.issubset(set(loaded))
    status = "ok" if iio and required_loaded and profile["current"] and profile["choices"] else "error"
    return result(status, {"iio_devices": iio, "surface_platform_devices": platform, "loaded_surface_modules": loaded, "platform_profile": profile}, None if status == "ok" else "SAM/IIO/platform-profile inventory is incomplete")


def probe_drm() -> dict[str, Any]:
    connectors = []
    try:
        paths = bounded_glob("/sys/class/drm/card*-*")
    except CollectError as exc:
        return result("error", reason=str(exc))
    for path in paths:
        status_value = read_optional(path / "status", 64)
        if status_value is None:
            continue
        modes_text = read_optional(path / "modes", 16384) or ""
        modes = [clean_text(line, 128) for line in modes_text.splitlines()[:128]]
        connectors.append({
            "connector": path.name,
            "status": status_value,
            "enabled": read_optional(path / "enabled", 64),
            "dpms": read_optional(path / "dpms", 64),
            "modes": modes,
        })
    connected = [item["connector"] for item in connectors if item["status"] == "connected"]
    framebuffers = []
    try:
        for path in bounded_glob("/sys/class/graphics/fb*"):
            framebuffers.append({
                "framebuffer": path.name,
                "name": read_optional(path / "name", 256),
                "current_mode": read_optional(path / "mode", 256),
                "virtual_size": read_optional(path / "virtual_size", 256),
                "bits_per_pixel": read_optional(path / "bits_per_pixel", 64),
            })
    except CollectError as exc:
        return result("error", {"connectors": connectors, "connected": connected}, str(exc))
    atomic_state = []
    try:
        for path in bounded_glob("/sys/kernel/debug/dri/*/state"):
            card = path.parent.name
            if not re.fullmatch(r"[0-9]{1,3}", card):
                continue
            state_text = read_optional(path, 131072)
            if state_text is not None:
                atomic_state.append({"dri_card_index": card, "lines": [clean_text(line, 2048) for line in state_text.splitlines()[:2048]]})
    except CollectError as exc:
        return result("error", {"connectors": connectors, "connected": connected, "framebuffers": framebuffers}, str(exc))
    drm_info: dict[str, Any] = {"status": "unavailable", "reason": "drm_info not installed"}
    if Path("/usr/bin/drm_info").is_file():
        command = run_fixed(("/usr/bin/drm_info", "-j"))
        if command["status"] == "ok":
            try:
                drm_info = {"status": "ok", "inventory": scrub_json(json.loads(command["stdout"]))}
            except (json.JSONDecodeError, CollectError) as exc:
                drm_info = {"status": "error", "reason": clean_text(str(exc))}
        else:
            drm_info = {"status": command["status"], "reason": command.get("reason", "unavailable")}
    data = {"connectors": connectors, "connected": connected, "framebuffers": framebuffers, "atomic_state": atomic_state, "drm_info": drm_info}
    current_state_present = bool(atomic_state) or any(item.get("current_mode") for item in framebuffers) or drm_info["status"] == "ok"
    status = "ok" if connected and current_state_present else "error"
    return result(status, data, None if status == "ok" else "connected connector or bounded current DRM state is unavailable")


def probe_cameras() -> dict[str, Any]:
    choices = (("/usr/bin/cam", "-l"), ("/usr/bin/libcamera-hello", "--list-cameras"))
    for argv in choices:
        if Path(argv[0]).is_file():
            inventory = command_lines(argv, 256)
            lines = inventory.get("lines", [])
            camera_entry = any(re.search(r"(?:^|\s)(?:[0-9]+\s*:|[0-9]+\s*\[)", line) for line in lines)
            status = "ok" if inventory["status"] == "ok" and camera_entry else "error"
            reason = inventory.get("reason") if inventory["status"] != "ok" else (None if camera_entry else "libcamera reported no camera entries")
            return result(status, {"operation": "enumeration-only", "command": argv[0], "inventory": inventory}, reason)
    return result("error", {"operation": "enumeration-only"}, "no allowlisted libcamera enumerator installed")


def probe_radios() -> dict[str, Any]:
    nm = command_lines(("/usr/bin/nmcli", "--terse", "--fields", "DEVICE,TYPE,STATE", "device", "status"), 128)
    bt_command = run_fixed(("/usr/bin/bluetoothctl", "show"))
    bluetooth: dict[str, Any]
    if bt_command["status"] == "ok":
        allowed = {"Powered", "Discoverable", "DiscoverableTimeout", "Pairable", "PairableTimeout"}
        bluetooth = {"status": "ok", "properties": {}}
        for line in bt_command["stdout"].splitlines()[:128]:
            stripped = line.strip()
            if ":" not in stripped:
                continue
            key, value = stripped.split(":", 1)
            if key in allowed:
                bluetooth["properties"][key] = clean_text(value.strip(), 128)
    else:
        bluetooth = {"status": bt_command["status"], "reason": bt_command.get("reason", "unavailable")}
    network_hardware = []
    bluetooth_hardware = []
    try:
        for path in bounded_glob("/sys/class/net/*"):
            if not (path / "wireless").is_dir():
                continue
            driver_link = path / "device/driver"
            network_hardware.append({
                "interface": clean_text(path.name, 128),
                "kind": "wifi",
                "driver": clean_text(driver_link.resolve().name, 256) if driver_link.exists() else None,
                "operstate": read_optional(path / "operstate", 64),
            })
        for path in bounded_glob("/sys/class/bluetooth/hci*"):
            driver_link = path / "device/driver"
            bluetooth_hardware.append({
                "controller": clean_text(path.name, 128),
                "driver": clean_text(driver_link.resolve().name, 256) if driver_link.exists() else None,
            })
    except (CollectError, OSError) as exc:
        return result("error", {"network_devices_without_connections": nm, "bluetooth_without_identity": bluetooth}, str(exc))
    wifi_present = nm["status"] == "ok" and any(":wifi:" in line for line in nm.get("lines", [])) and bool(network_hardware)
    status = "ok" if wifi_present and bluetooth["status"] == "ok" and bluetooth_hardware else "error"
    return result(status, {"network_devices_without_connections": nm, "wifi_hardware_without_addresses": network_hardware, "bluetooth_without_identity": bluetooth, "bluetooth_hardware_without_addresses": bluetooth_hardware}, None if status == "ok" else "radio inventory incomplete")


def project_fwupd(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise CollectError("fwupd root is not an object")
    raw_devices = value.get("Devices")
    if not isinstance(raw_devices, list) or len(raw_devices) > 64:
        raise CollectError("fwupd Devices must be an array of at most 64 entries")
    allowed = ("Name", "Vendor", "VendorId", "Version", "VersionLowest", "VersionFormat", "UpdateState", "Flags", "Guid", "Guids", "Plugin", "Protocol")
    devices = []
    for raw in raw_devices:
        if not isinstance(raw, dict):
            raise CollectError("fwupd device is not an object")
        item = {key: scrub_json(raw[key]) for key in allowed if key in raw}
        encoded = json.dumps(item, sort_keys=True)
        if len(encoded.encode()) > 32768:
            raise CollectError("projected fwupd device exceeds 32768 bytes")
        devices.append(item)
    return {"Devices": devices}


def probe_firmware() -> dict[str, Any]:
    command = run_fixed(("/usr/bin/fwupdmgr", "get-devices", "--json", "--no-unreported-check"))
    if command["status"] != "ok":
        return result("error", reason=command.get("reason", "fwupd unavailable"))
    try:
        inventory = project_fwupd(json.loads(command["stdout"]))
    except (json.JSONDecodeError, CollectError) as exc:
        return result("error", reason=f"invalid bounded fwupd inventory: {exc}")
    if not inventory["Devices"]:
        return result("error", {"operation": "get-devices only", "inventory": inventory}, "fwupd inventory is empty")
    return result("ok", {"operation": "get-devices only", "inventory": inventory})


def probe_audio() -> dict[str, Any]:
    inventory = command_lines(("/usr/bin/wpctl", "status", "--name"), 512)
    return result("ok" if inventory["status"] == "ok" else "error", {"operation": "inventory-only; no playback or recording", "inventory": inventory}, inventory.get("reason"))


def probe_power() -> dict[str, Any]:
    supplies = []
    fields = ("type", "status", "capacity", "capacity_level", "health", "cycle_count", "technology", "energy_full", "energy_full_design")
    try:
        for path in bounded_glob("/sys/class/power_supply/*"):
            supplies.append({"supply": path.name, **{field: read_optional(path / field, 256) for field in fields}})
    except CollectError as exc:
        return result("error", {"power_supplies": supplies}, str(exc))
    counters = {}
    for path in (
        Path("/sys/kernel/debug/pmc_core/slp_s0_residency_usec"),
        Path("/sys/devices/system/cpu/cpuidle/low_power_idle_system_residency_us"),
    ):
        counters[str(path)] = read_optional(path, 256)
    data = {
        "power_supplies": supplies,
        "available_suspend_states": read_optional(Path("/sys/power/state"), 256),
        "available_mem_sleep_modes": read_optional(Path("/sys/power/mem_sleep"), 256),
        "available_hibernation_modes": read_optional(Path("/sys/power/disk"), 256),
        "s0ix_counters_snapshot": counters,
        "boot_id_sha256": None,
        "uptime_seconds_snapshot": None,
        "note": "single read-only snapshot; suspend/resume acceptance requires before/after operator evidence",
    }
    boot_id = read_optional(Path("/proc/sys/kernel/random/boot_id"), 128)
    if boot_id:
        data["boot_id_sha256"] = sha256_bytes(boot_id.encode())
    uptime = read_optional(Path("/proc/uptime"), 256)
    if uptime:
        data["uptime_seconds_snapshot"] = clean_text(uptime.split()[0], 64)
    return result("ok" if supplies else "error", data, None if supplies else "no power-supply inventory")


def parse_systemctl_show(text: str) -> dict[str, str]:
    allowed = {"Id", "LoadState", "ActiveState", "SubState", "UnitFileState", "NRestarts", "FragmentPath", "ExecMainPID", "ExecMainStatus"}
    parsed = {}
    for line in text.splitlines():
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        if key in allowed:
            parsed[key] = clean_text(value, 1024)
    return parsed


def probe_services() -> dict[str, Any]:
    services = []
    for unit in SERVICE_UNITS:
        command = run_fixed(("/usr/bin/systemctl", "show", unit, "--no-pager", "--property=Id,LoadState,ActiveState,SubState,UnitFileState,NRestarts,FragmentPath,ExecMainPID,ExecMainStatus"))
        services.append({"unit": unit, "status": command["status"], "properties": parse_systemctl_show(command.get("stdout", ""))})
    binaries = [file_descriptor(Path(path)) for path in ("/usr/bin/mackesd", "/usr/bin/mde-shell-egui")]
    inactive_core = [item["unit"] for item in services[:2] if item["properties"].get("ActiveState") != "active"]
    data = {"services": services, "binary_revisions": binaries}
    return result("ok" if not inactive_core else "error", data, None if not inactive_core else "inactive core services: " + ", ".join(inactive_core))


PROBES = (
    ("identity.json", probe_identity),
    ("release-packages.json", probe_release_packages),
    ("kernel-modules.json", probe_kernel_modules),
    ("iptsd.json", probe_iptsd),
    ("input.json", probe_input),
    ("buttons-storage.json", probe_buttons_storage),
    ("sam-iio.json", probe_sam_iio),
    ("drm.json", probe_drm),
    ("cameras.json", probe_cameras),
    ("radios.json", probe_radios),
    ("firmware.json", probe_firmware),
    ("audio.json", probe_audio),
    ("power.json", probe_power),
    ("services.json", probe_services),
)


def write_json(path: Path, value: Any) -> None:
    data = (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n").encode()
    if len(data) > MAX_ARTIFACT_BYTES:
        raise CollectError(f"artifact exceeds {MAX_ARTIFACT_BYTES} bytes: {path.name}")
    path.write_bytes(data)
    path.chmod(0o600)


def collect(out: Path, seat: str, expected: int) -> int:
    if not ALLOWED_SEAT.fullmatch(seat):
        raise CollectError("seat label must match [A-Za-z0-9][A-Za-z0-9_.-]{0,63}")
    if out.exists() or out.is_symlink():
        raise CollectError(f"output must not already exist: {out}")
    parent = out.parent.resolve(strict=True)
    if not parent.is_dir():
        raise CollectError("output parent is not a directory")
    old_umask = os.umask(0o077)
    temp: Path | None = Path(tempfile.mkdtemp(prefix=f".{out.name}.tmp-", dir=parent))
    try:
        statuses: dict[str, str] = {}
        for filename, probe in PROBES:
            try:
                value = probe(expected, seat) if filename == "identity.json" else probe()
            except Exception as exc:  # each failed probe remains explicit evidence
                value = result("error", reason=f"collector probe failed: {type(exc).__name__}: {exc}")
            statuses[filename] = value["status"]
            write_json(temp / filename, value)
        artifacts = []
        for filename in EXPECTED_FILES:
            path = temp / filename
            artifacts.append({"file": filename, "bytes": path.stat().st_size, "sha256": sha256_file(path), "status": statuses[filename]})
        incomplete = sorted(name for name, status in statuses.items() if status != "ok")
        manifest = {
            "schema_version": SCHEMA_VERSION,
            "collector": {
                "path": "install-helpers/collect-surface-acceptance.py",
                "sha256": sha256_file(Path(__file__).resolve()),
                "command_timeout_seconds": COMMAND_TIMEOUT_SECONDS,
                "max_command_bytes": MAX_COMMAND_BYTES,
                "max_artifact_bytes": MAX_ARTIFACT_BYTES,
            },
            "seat_label": seat,
            "captured_at_utc": datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z"),
            "expected_surface_pro_generation": expected,
            "collection_scope": "read-only inventory; no pixels or audio captured",
            "collection_verdict": "complete" if not incomplete else "incomplete",
            "incomplete_probes": incomplete,
            "physical_acceptance_claimed": False,
            "manual_acceptance_required": list(MANUAL_CHECKS),
            "artifacts": artifacts,
        }
        write_json(temp / "manifest.json", manifest)
        total = sum(path.stat().st_size for path in temp.iterdir())
        if total > MAX_BUNDLE_BYTES:
            raise CollectError(f"bundle exceeds {MAX_BUNDLE_BYTES} bytes")
        os.replace(temp, out)
        temp = None
        print(f"Surface acceptance evidence: {out}")
        print(f"collection_verdict={manifest['collection_verdict']}")
        if incomplete:
            print("incomplete_probes=" + ",".join(incomplete), file=sys.stderr)
            return 3
        return 0
    finally:
        os.umask(old_umask)
        if temp is not None and temp.exists():
            shutil.rmtree(temp)


def validate(bundle: Path) -> int:
    if not bundle.is_dir() or bundle.is_symlink():
        raise CollectError("bundle must be a real directory")
    names = sorted(path.name for path in bundle.iterdir())
    expected = sorted((*EXPECTED_FILES, "manifest.json"))
    if names != expected:
        raise CollectError("bundle contains missing or unknown files")
    for path in bundle.iterdir():
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or path.is_symlink() or info.st_size > MAX_ARTIFACT_BYTES:
            raise CollectError(f"invalid artifact: {path.name}")
    if sum(path.stat().st_size for path in bundle.iterdir()) > MAX_BUNDLE_BYTES:
        raise CollectError(f"bundle exceeds {MAX_BUNDLE_BYTES} bytes")
    manifest = json.loads(read_limited(bundle / "manifest.json", MAX_ARTIFACT_BYTES))
    manifest_keys = {
        "schema_version", "collector", "seat_label", "captured_at_utc", "expected_surface_pro_generation",
        "collection_scope", "collection_verdict", "incomplete_probes",
        "physical_acceptance_claimed", "manual_acceptance_required", "artifacts",
    }
    if not isinstance(manifest, dict) or set(manifest) != manifest_keys or manifest.get("schema_version") != SCHEMA_VERSION:
        raise CollectError("manifest schema is invalid")
    if not isinstance(manifest.get("seat_label"), str) or not ALLOWED_SEAT.fullmatch(manifest["seat_label"]):
        raise CollectError("manifest seat label is invalid")
    if not isinstance(manifest.get("captured_at_utc"), str) or not re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z", manifest["captured_at_utc"]):
        raise CollectError("manifest capture timestamp is invalid")
    if manifest.get("expected_surface_pro_generation") not in (5, 6):
        raise CollectError("manifest generation is invalid")
    if manifest.get("collection_scope") != "read-only inventory; no pixels or audio captured":
        raise CollectError("manifest collection scope is invalid")
    collector = manifest.get("collector")
    if not isinstance(collector, dict) or set(collector) != {"path", "sha256", "command_timeout_seconds", "max_command_bytes", "max_artifact_bytes"}:
        raise CollectError("collector provenance is invalid")
    if collector.get("path") != "install-helpers/collect-surface-acceptance.py" or not re.fullmatch(r"[0-9a-f]{64}", str(collector.get("sha256", ""))):
        raise CollectError("collector identity is invalid")
    if collector.get("command_timeout_seconds") != COMMAND_TIMEOUT_SECONDS or collector.get("max_command_bytes") != MAX_COMMAND_BYTES or collector.get("max_artifact_bytes") != MAX_ARTIFACT_BYTES:
        raise CollectError("collector bounds are invalid")
    artifacts = manifest.get("artifacts")
    if not isinstance(artifacts, list) or len(artifacts) != len(EXPECTED_FILES):
        raise CollectError("manifest artifact list is invalid")
    seen = set()
    observed_incomplete = []
    for item in artifacts:
        if not isinstance(item, dict) or set(item) != {"file", "bytes", "sha256", "status"}:
            raise CollectError("artifact descriptor is invalid")
        name = item["file"]
        if name not in EXPECTED_FILES or name in seen:
            raise CollectError("artifact filename is invalid or duplicated")
        seen.add(name)
        path = bundle / name
        if item["bytes"] != path.stat().st_size or item["sha256"] != sha256_file(path):
            raise CollectError(f"artifact integrity mismatch: {name}")
        document = json.loads(read_limited(path, MAX_ARTIFACT_BYTES))
        if not isinstance(document, dict) or not set(document).issubset({"schema_version", "status", "data", "reason"}):
            raise CollectError(f"artifact envelope is invalid: {name}")
        if document.get("schema_version") != SCHEMA_VERSION or document.get("status") not in ("ok", "error", "unavailable") or document.get("status") != item["status"]:
            raise CollectError(f"artifact envelope mismatch: {name}")
        if item["status"] != "ok":
            observed_incomplete.append(name)
    if seen != set(EXPECTED_FILES):
        raise CollectError("manifest does not cover every artifact")
    if manifest.get("physical_acceptance_claimed") is not False:
        raise CollectError("collector may not claim physical acceptance")
    if manifest.get("manual_acceptance_required") != list(MANUAL_CHECKS):
        raise CollectError("manual acceptance checklist is invalid")
    if manifest.get("incomplete_probes") != sorted(observed_incomplete):
        raise CollectError("manifest incomplete-probe list is inconsistent")
    expected_verdict = "incomplete" if observed_incomplete else "complete"
    if manifest.get("collection_verdict") != expected_verdict:
        raise CollectError("manifest collection verdict is inconsistent")
    print(f"Surface acceptance evidence valid: {bundle}")
    print(f"collection_verdict={manifest.get('collection_verdict')}")
    return 0 if expected_verdict == "complete" else 3


def self_test() -> int:
    hostile = (
        "AA:BB:CC:DD:EE:FF",
        "10.42.0.7",
        "2001:db8::1",
        "token=abcdef",
        "Bearer: supersecret",
        "password=hunter2",
        "private_key=deadbeef",
        "ssid=MyHouse",
        "connection=CorpWifi",
        "safe\x00hidden",
        "bad\x01control",
        "https://alice:swordfish@example.invalid/image",
        "ghp_abcdefghijklmnopqrstuvwxyz123456",
        "abcdefghijk.abcdefghijk.abcdefghijk",
    )
    for value in hostile:
        cleaned = clean_text(value)
        if value == cleaned or "supersecret" in cleaned or "hunter2" in cleaned or "MyHouse" in cleaned or "CorpWifi" in cleaned:
            raise CollectError(f"redaction self-test failed: {value!r}")
    if detect_generation("Microsoft Corporation", "Surface Pro 6", "Surface_Pro_6", 6) != 6 or detect_generation("Microsoft Corporation", "Surface Pro 6", "Surface_Pro_6", 5) is not None:
        raise CollectError("generation binding self-test failed")
    if detect_generation("Microsoft Corporation", "Surface Pro", "Surface_Pro_1796", 5) != 5 or detect_generation("Microsoft Corporation", "Surface Pro", "Surface_Pro_1807", 5) != 5 or detect_generation("Microsoft Corporation", "Surface Pro", "Surface_Pro_6", 5) is not None or detect_generation("Microsoft Corporation", "Surface Pro", None, 5) is not None or detect_generation("Other", "Surface Pro", "Surface_Pro_1796", 5) is not None or detect_generation("Microsoft Corporation", "Surface Pro 7", None, 6) is not None:
        raise CollectError("generation allowlist self-test failed")
    projected = project_fwupd({"Devices": [{"Name": "UEFI", "Version": "1", "SerialNumber": "secret", "DeviceId": "secret", "Guid": "public-guid"}]})
    encoded = json.dumps(projected)
    if "SerialNumber" in encoded or "DeviceId" in encoded or "secret" in encoded:
        raise CollectError("fwupd projection leaked an identifier")
    try:
        project_fwupd({"Devices": [{}] * 65})
        raise CollectError("oversized fwupd inventory accepted")
    except CollectError as exc:
        if str(exc) == "oversized fwupd inventory accepted":
            raise
    print(f"Surface acceptance collector self-test passed ({len(hostile)} hostile strings and bounded fwupd fixtures)")
    return 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    sub = parser.add_subparsers(dest="command")
    collect_parser = sub.add_parser("collect", help="collect a new evidence directory")
    collect_parser.add_argument("--out", required=True, type=Path)
    collect_parser.add_argument("--seat", default="Surface")
    collect_parser.add_argument("--expected-generation", required=True, type=int, choices=(5, 6))
    validate_parser = sub.add_parser("validate", help="validate hashes and envelope shape")
    validate_parser.add_argument("bundle", type=Path)
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if args.self_test:
        return self_test()
    if args.command == "collect":
        return collect(args.out, args.seat, args.expected_generation)
    if args.command == "validate":
        return validate(args.bundle)
    raise CollectError("choose collect, validate, or --self-test")


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv[1:]))
    except (CollectError, json.JSONDecodeError, OSError) as exc:
        print(f"collect-surface-acceptance: {clean_text(str(exc), 2048)}", file=sys.stderr)
        raise SystemExit(2)
