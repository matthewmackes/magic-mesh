#!/usr/bin/env python3
"""Probe and validate a Browser VM deployment receipt.

The receipt is produced after an immutable qcow2 base is installed and a
separate writable qcow2 overlay is attached to the running Browser VM domain.
It binds live acceptance artifacts to a target node and domain instead of
accepting records from any guest carrying the same image digest.

Usage:
  verify-browser-vm-deployment.py validate receipt.json
  verify-browser-vm-deployment.py probe-live --remote-image PATH \
    --domain NAME --expected-digest sha256:<64-hex>
  verify-browser-vm-deployment.py --self-test
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import grp
import hashlib
import json
import os
from pathlib import Path
import pwd
import re
import shutil
import socket
import stat
import subprocess
import sys
from typing import Any, NoReturn
import xml.etree.ElementTree as ET


SCHEMA_VERSION = 2
MAX_FILE_BYTES = 1024 * 1024
MAX_COMMAND_OUTPUT_BYTES = 2 * 1024 * 1024
COMMAND_TIMEOUT_SECONDS = 30
SAFE_COMMAND_PATH = "/usr/sbin:/usr/bin:/sbin:/bin"
COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
IMAGE_DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
HOST_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:-]{0,252}$")
NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$")
UUID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
UTC_TIMESTAMP_RE = re.compile(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$")
PATH_RE = re.compile(r"^/(?:[A-Za-z0-9._@+-]+/)*[A-Za-z0-9._@+-]+$")
CREDENTIAL_FIELD_RE = re.compile(
    r"(?:pass(?:word|phrase)?|secret|token|ticket|credential|bearer|"
    r"api[_-]?key|access[_-]?key|private[_-]?key|cookie|authorization|"
    r"identity[_-]?file|pem)",
    re.IGNORECASE,
)
EXPECTED_FIELDS = frozenset(
    {
        "schema_version",
        "kind",
        "profile",
        "image",
        "status",
        "source",
        "target_host",
        "node_hostname",
        "domain_name",
        "domain_uuid",
        "domain_state",
        "remote_image",
        "remote_image_format",
        "attached_disk",
        "attached_disk_format",
        "backing_image",
        "backing_chain_depth",
        "source_commit",
        "image_digest",
        "remote_image_digest",
        "recorded_at",
    }
)

PROBE_FIELDS = frozenset(
    {
        "node_hostname",
        "domain_uuid",
        "domain_state",
        "remote_image",
        "remote_image_format",
        "attached_disk",
        "attached_disk_format",
        "backing_image",
        "backing_chain_depth",
        "remote_image_digest",
    }
)


class EvidenceError(ValueError):
    """The receipt cannot prove an attached Browser VM image."""


def fail(message: str) -> NoReturn:
    raise EvidenceError(message)


def reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            fail(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def read_json(path: Path) -> Any:
    try:
        stat_result = path.lstat()
    except OSError as exc:
        fail(f"receipt is not readable: {exc}")
    if os.path.islink(path) or not path.is_file():
        fail("receipt must be a regular non-symlink file")
    if stat_result.st_mode & 0o077 or stat_result.st_mode & 0o111:
        fail("receipt must be private and non-executable")
    if stat_result.st_size > MAX_FILE_BYTES:
        fail("receipt is too large")
    try:
        return json.loads(
            path.read_text(encoding="utf-8"), object_pairs_hook=reject_duplicate_keys
        )
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail(f"malformed receipt JSON: {exc}")


def reject_credential_fields(value: Any, location: str = "root") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            if not isinstance(key, str):
                fail(f"field name at {location} is not a string")
            if CREDENTIAL_FIELD_RE.search(key):
                fail(f"credential-shaped field is not allowed: {location}.{key}")
            reject_credential_fields(child, f"{location}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            reject_credential_fields(child, f"{location}[{index}]")


def require_string(data: dict[str, Any], field: str, pattern: re.Pattern[str]) -> str:
    value = data.get(field)
    if not isinstance(value, str) or pattern.fullmatch(value) is None:
        fail(f"{field} is malformed")
    return value


def require_exact_fields(data: dict[str, Any], expected: frozenset[str], label: str) -> None:
    fields = frozenset(data)
    if fields == expected:
        return
    missing = expected - fields
    extra = fields - expected
    if missing:
        fail(f"{label} is missing fields: {', '.join(sorted(missing))}")
    fail(f"{label} has unexpected fields: {', '.join(sorted(extra))}")


def validate_timestamp(value: Any) -> None:
    if not isinstance(value, str) or UTC_TIMESTAMP_RE.fullmatch(value) is None:
        fail("recorded_at must use second-precision UTC form YYYY-MM-DDTHH:MM:SSZ")
    try:
        recorded = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(
            tzinfo=timezone.utc
        )
    except ValueError as exc:
        fail(f"recorded_at is not a real UTC timestamp: {exc}")
    age = (datetime.now(timezone.utc) - recorded).total_seconds()
    if age < -300 or age > 24 * 60 * 60:
        fail(f"recorded_at is stale or from the future (age_seconds={age:.0f})")


def validate_path(data: dict[str, Any], field: str) -> str:
    value = require_string(data, field, PATH_RE)
    parts = value.split("/")[1:]
    if not parts or any(part in {"", ".", ".."} for part in parts):
        fail(f"{field} must be a normalized absolute path")
    return value


def validate_path_value(value: Any, field: str) -> str:
    return validate_path({field: value}, field)


def validate_backing_contract(
    *,
    remote_image: str,
    attached_disk: str,
    backing_image: str,
    remote_image_format: Any,
    attached_disk_format: Any,
    backing_chain_depth: Any,
) -> None:
    if attached_disk == remote_image:
        fail("attached_disk must be a writable overlay, not remote_image")
    if backing_image != remote_image:
        fail("backing_image does not match remote_image")
    if remote_image_format != "qcow2":
        fail("remote_image_format must be qcow2")
    if attached_disk_format != "qcow2":
        fail("attached_disk_format must be qcow2")
    if backing_chain_depth != 1 or isinstance(backing_chain_depth, bool):
        fail("backing_chain_depth must be integer 1")


def validate_probe_document(data: Any) -> dict[str, Any]:
    if not isinstance(data, dict):
        fail("live probe root must be one JSON object")
    require_exact_fields(data, PROBE_FIELDS, "live probe")
    node_hostname = require_string(data, "node_hostname", HOST_RE)
    domain_uuid = require_string(data, "domain_uuid", UUID_RE)
    if data["domain_state"] != "running":
        fail("deployment domain is not running")
    remote_image = validate_path(data, "remote_image")
    attached_disk = validate_path(data, "attached_disk")
    backing_image = validate_path(data, "backing_image")
    validate_backing_contract(
        remote_image=remote_image,
        attached_disk=attached_disk,
        backing_image=backing_image,
        remote_image_format=data["remote_image_format"],
        attached_disk_format=data["attached_disk_format"],
        backing_chain_depth=data["backing_chain_depth"],
    )
    remote_digest = require_string(data, "remote_image_digest", IMAGE_DIGEST_RE)
    if remote_digest == "sha256:" + "0" * 64:
        fail("remote_image_digest must not be null")
    return {
        "node_hostname": node_hostname,
        "domain_uuid": domain_uuid,
        "domain_state": "running",
        "remote_image": remote_image,
        "remote_image_format": "qcow2",
        "attached_disk": attached_disk,
        "attached_disk_format": "qcow2",
        "backing_image": backing_image,
        "backing_chain_depth": 1,
        "remote_image_digest": remote_digest,
    }


def find_command(name: str) -> str:
    executable = shutil.which(name, path=SAFE_COMMAND_PATH)
    if executable is None or not os.path.isabs(executable):
        fail(f"required command is unavailable: {name}")
    return executable


def run_command(executable: str, *arguments: str) -> str:
    try:
        completed = subprocess.run(
            [executable, *arguments],
            check=False,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="strict",
            timeout=COMMAND_TIMEOUT_SECONDS,
            env={"LANG": "C", "LC_ALL": "C", "PATH": SAFE_COMMAND_PATH},
        )
    except (OSError, subprocess.SubprocessError, UnicodeError) as exc:
        fail(f"command failed safely: {Path(executable).name}: {exc}")
    if completed.returncode != 0:
        fail(f"command failed safely: {Path(executable).name}")
    if len(completed.stdout.encode("utf-8")) > MAX_COMMAND_OUTPUT_BYTES:
        fail(f"command output is too large: {Path(executable).name}")
    return completed.stdout


def parse_json_output(output: str, label: str) -> Any:
    try:
        return json.loads(output, object_pairs_hook=reject_duplicate_keys)
    except (UnicodeError, json.JSONDecodeError) as exc:
        fail(f"{label} returned malformed JSON: {exc}")


def validate_vda_xml(xml_text: str) -> tuple[str, str]:
    if len(xml_text.encode("utf-8")) > MAX_COMMAND_OUTPUT_BYTES:
        fail("libvirt domain XML is too large")
    try:
        root = ET.fromstring(xml_text)
    except ET.ParseError as exc:
        fail(f"libvirt domain XML is malformed: {exc}")
    matches: list[ET.Element] = []
    for disk in root.findall("./devices/disk"):
        target = disk.find("target")
        if target is not None and target.get("dev") == "vda":
            matches.append(disk)
    if len(matches) != 1:
        fail("running domain must expose exactly one vda block device")
    disk = matches[0]
    if disk.get("device") != "disk" or disk.get("type") != "file":
        fail("vda must be one file-backed disk")
    if disk.find("readonly") is not None:
        fail("vda overlay must be writable")
    source = disk.find("source")
    if source is None or source.get("file") is None:
        fail("vda source must contain a file path")
    attached_disk = validate_path_value(source.get("file"), "attached_disk")
    driver = disk.find("driver")
    if driver is None or driver.get("type") != "qcow2":
        fail("vda driver must declare qcow2")
    backing_store = disk.find("backingStore")
    if backing_store is None or backing_store.get("type") != "file":
        fail("vda must declare one file-backed backingStore")
    backing_format = backing_store.find("format")
    if backing_format is None or backing_format.get("type") != "qcow2":
        fail("vda backingStore format must be qcow2")
    backing_source = backing_store.find("source")
    if backing_source is None or backing_source.get("file") is None:
        fail("vda backingStore source must contain a file path")
    backing_image = validate_path_value(
        backing_source.get("file"), "backing_image"
    )
    terminator = backing_store.find("backingStore")
    if terminator is None or terminator.find("source") is not None:
        fail("vda backingStore must terminate after one backing level")
    return attached_disk, backing_image


def require_qcow2_info(data: Any, expected_path: str, label: str) -> dict[str, Any]:
    if not isinstance(data, dict):
        fail(f"{label} qemu-img info is not one JSON object")
    if data.get("format") != "qcow2":
        fail(f"{label} format is not qcow2")
    filename = validate_path_value(data.get("filename"), f"{label}_filename")
    if filename != expected_path:
        fail(f"{label} qemu-img filename does not match its declared path")
    return data


def validate_qcow2_graph(
    remote_image: str,
    attached_disk: str,
    base_info: Any,
    overlay_info: Any,
    chain_info: Any,
) -> str:
    if attached_disk == remote_image:
        fail("running domain directly attaches the immutable base")
    base = require_qcow2_info(base_info, remote_image, "remote_image")
    if "backing-filename" in base or "full-backing-filename" in base:
        fail("remote_image must not have a backing image")
    overlay = require_qcow2_info(overlay_info, attached_disk, "attached_disk")
    backing_image = validate_path_value(
        overlay.get("backing-filename"), "backing_image"
    )
    if backing_image != remote_image:
        fail("attached_disk immediate backing image does not match remote_image")
    full_backing = overlay.get("full-backing-filename")
    if full_backing is not None:
        full_backing_path = validate_path_value(full_backing, "full_backing_image")
        if full_backing_path != remote_image:
            fail("attached_disk resolved backing image does not match remote_image")
    if not isinstance(chain_info, list) or len(chain_info) != 2:
        fail("attached_disk must have exactly one backing level")
    chain_overlay = require_qcow2_info(
        chain_info[0], attached_disk, "backing_chain_overlay"
    )
    chain_base = require_qcow2_info(chain_info[1], remote_image, "backing_chain_base")
    chain_backing = validate_path_value(
        chain_overlay.get("backing-filename"), "backing_chain_image"
    )
    if chain_backing != remote_image:
        fail("backing chain does not point directly to remote_image")
    if "backing-filename" in chain_base or "full-backing-filename" in chain_base:
        fail("backing chain base must terminate at remote_image")
    return backing_image


def lstat_regular(path: str, label: str) -> os.stat_result:
    try:
        metadata = os.lstat(path)
    except OSError as exc:
        fail(f"{label} is not readable: {exc}")
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        fail(f"{label} must be a regular non-symlink file")
    return metadata


def validate_base_metadata(metadata: os.stat_result, qemu_gid: int) -> None:
    if metadata.st_uid != 0:
        fail("remote_image must be owned by root")
    if metadata.st_gid != qemu_gid:
        fail("remote_image must be owned by the qemu group")
    if metadata.st_mode & 0o022:
        fail("remote_image must not be writable by group or other")
    if not metadata.st_mode & stat.S_IRGRP:
        fail("remote_image must be readable by the qemu group")


def validate_overlay_metadata(
    metadata: os.stat_result, qemu_uid: int, qemu_gid: int
) -> None:
    owner_access = metadata.st_uid == qemu_uid and (
        metadata.st_mode & (stat.S_IRUSR | stat.S_IWUSR)
    ) == (stat.S_IRUSR | stat.S_IWUSR)
    group_access = metadata.st_gid == qemu_gid and (
        metadata.st_mode & (stat.S_IRGRP | stat.S_IWGRP)
    ) == (stat.S_IRGRP | stat.S_IWGRP)
    if not owner_access and not group_access:
        fail("attached_disk must be readable and writable by qemu")
    if metadata.st_mode & stat.S_IWOTH:
        fail("attached_disk must not be writable by other")


def file_identity(metadata: os.stat_result) -> tuple[int, int, int, int, int]:
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_size,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    )


def hash_base_image(path: str, expected_metadata: os.stat_result) -> str:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as exc:
        fail(f"remote_image could not be opened safely: {exc}")
    digest = hashlib.sha256()
    try:
        try:
            before = os.fstat(descriptor)
            if (before.st_dev, before.st_ino) != (
                expected_metadata.st_dev,
                expected_metadata.st_ino,
            ):
                fail("remote_image changed before hashing")
            while True:
                chunk = os.read(descriptor, 1024 * 1024)
                if not chunk:
                    break
                digest.update(chunk)
            after = os.fstat(descriptor)
        except OSError as exc:
            fail(f"remote_image hashing failed safely: {exc}")
    finally:
        os.close(descriptor)
    if file_identity(before) != file_identity(after):
        fail("remote_image changed while it was hashed")
    return "sha256:" + digest.hexdigest()


def probe_live(remote_image: str, domain_name: str, expected_digest: str) -> dict[str, Any]:
    if os.geteuid() != 0:
        fail("probe-live must run as root")
    remote_image = validate_path_value(remote_image, "remote_image")
    if NAME_RE.fullmatch(domain_name) is None:
        fail("domain_name is malformed")
    if IMAGE_DIGEST_RE.fullmatch(expected_digest) is None or expected_digest == (
        "sha256:" + "0" * 64
    ):
        fail("expected digest is malformed")
    try:
        qemu_gid = grp.getgrnam("qemu").gr_gid
        qemu_uid = pwd.getpwnam("qemu").pw_uid
    except KeyError:
        fail("required qemu user or group is unavailable")
    virsh = find_command("virsh")
    qemu_img = find_command("qemu-img")
    state = run_command(
        virsh, "-c", "qemu:///system", "domstate", domain_name
    ).strip()
    if state != "running":
        fail(f"deployment domain is not running: {state or 'unknown'}")
    domain_uuid = run_command(
        virsh, "-c", "qemu:///system", "domuuid", domain_name
    ).strip()
    if UUID_RE.fullmatch(domain_uuid) is None:
        fail("running domain UUID is malformed")
    attached_disk, xml_backing_image = validate_vda_xml(
        run_command(virsh, "-c", "qemu:///system", "dumpxml", domain_name)
    )
    if xml_backing_image != remote_image:
        fail("libvirt backingStore does not match remote_image")
    base_metadata = lstat_regular(remote_image, "remote_image")
    validate_base_metadata(base_metadata, qemu_gid)
    overlay_metadata = lstat_regular(attached_disk, "attached_disk")
    validate_overlay_metadata(overlay_metadata, qemu_uid, qemu_gid)
    base_info = parse_json_output(
        run_command(
            qemu_img, "info", "--force-share", "--output=json", "--", remote_image
        ),
        "remote_image qemu-img info",
    )
    overlay_info = parse_json_output(
        run_command(
            qemu_img, "info", "--force-share", "--output=json", "--", attached_disk
        ),
        "attached_disk qemu-img info",
    )
    chain_info = parse_json_output(
        run_command(
            qemu_img,
            "info",
            "--force-share",
            "--output=json",
            "--backing-chain",
            "--",
            attached_disk,
        ),
        "attached_disk backing chain",
    )
    backing_image = validate_qcow2_graph(
        remote_image, attached_disk, base_info, overlay_info, chain_info
    )
    remote_digest = hash_base_image(remote_image, base_metadata)
    if remote_digest != expected_digest:
        fail("remote_image digest does not match expected digest")
    final_metadata = lstat_regular(remote_image, "remote_image")
    validate_base_metadata(final_metadata, qemu_gid)
    if file_identity(base_metadata) != file_identity(final_metadata):
        fail("remote_image changed during the live probe")
    final_state = run_command(
        virsh, "-c", "qemu:///system", "domstate", domain_name
    ).strip()
    if final_state != "running":
        fail("deployment domain stopped during the live probe")
    final_uuid = run_command(
        virsh, "-c", "qemu:///system", "domuuid", domain_name
    ).strip()
    if final_uuid != domain_uuid:
        fail("deployment domain identity changed during the live probe")
    final_attached_disk, final_xml_backing_image = validate_vda_xml(
        run_command(virsh, "-c", "qemu:///system", "dumpxml", domain_name)
    )
    if final_attached_disk != attached_disk:
        fail("deployment vda changed during the live probe")
    if final_xml_backing_image != remote_image:
        fail("deployment vda backingStore changed during the live probe")
    if run_command(
        virsh, "-c", "qemu:///system", "domstate", domain_name
    ).strip() != "running":
        fail("deployment domain stopped during the final identity check")
    hostname = socket.getfqdn() or socket.gethostname()
    if HOST_RE.fullmatch(hostname) is None:
        fail("node hostname is malformed")
    return validate_probe_document(
        {
            "node_hostname": hostname,
            "domain_uuid": domain_uuid,
            "domain_state": "running",
            "remote_image": remote_image,
            "remote_image_format": "qcow2",
            "attached_disk": attached_disk,
            "attached_disk_format": "qcow2",
            "backing_image": backing_image,
            "backing_chain_depth": 1,
            "remote_image_digest": remote_digest,
        }
    )


def validate_document(data: Any) -> dict[str, Any]:
    if not isinstance(data, dict):
        fail("receipt root must be one JSON object")
    reject_credential_fields(data)
    require_exact_fields(data, EXPECTED_FIELDS, "receipt")
    if data["schema_version"] != SCHEMA_VERSION or isinstance(data["schema_version"], bool):
        fail(f"schema_version must be integer {SCHEMA_VERSION}")
    if data["kind"] != "browser_vm_deployment_receipt":
        fail("kind is not browser_vm_deployment_receipt")
    if data["profile"] != "browser-vm-chromium" or data["image"] != "browser-vm-chromium":
        fail("receipt is not bound to browser-vm-chromium")
    if data["status"] != "observed":
        fail("deployment receipt is not observed")
    if data["source"] != "deploy-image.sh":
        fail("receipt source is not deploy-image.sh")
    target_host = require_string(data, "target_host", HOST_RE)
    require_string(data, "node_hostname", HOST_RE)
    require_string(data, "domain_name", NAME_RE)
    require_string(data, "domain_uuid", UUID_RE)
    if data["domain_state"] != "running":
        fail("deployment domain is not running")
    remote_image = validate_path(data, "remote_image")
    attached_disk = validate_path(data, "attached_disk")
    backing_image = validate_path(data, "backing_image")
    validate_backing_contract(
        remote_image=remote_image,
        attached_disk=attached_disk,
        backing_image=backing_image,
        remote_image_format=data["remote_image_format"],
        attached_disk_format=data["attached_disk_format"],
        backing_chain_depth=data["backing_chain_depth"],
    )
    source_commit = require_string(data, "source_commit", COMMIT_RE)
    image_digest = require_string(data, "image_digest", IMAGE_DIGEST_RE)
    remote_digest = require_string(data, "remote_image_digest", IMAGE_DIGEST_RE)
    if source_commit == "0" * 40:
        fail("source_commit must not be the null revision")
    if image_digest == "sha256:" + "0" * 64 or remote_digest == "sha256:" + "0" * 64:
        fail("image digests must not be null")
    if remote_digest != image_digest:
        fail("remote_image_digest does not match image_digest")
    validate_timestamp(data["recorded_at"])
    return {
        "status": "validated",
        "target_host": target_host,
        "node_hostname": data["node_hostname"],
        "domain_name": data["domain_name"],
        "domain_uuid": data["domain_uuid"],
        "remote_image": remote_image,
        "attached_disk": attached_disk,
        "backing_image": backing_image,
        "backing_chain_depth": 1,
        "source_commit": source_commit,
        "image_digest": image_digest,
    }


def self_test() -> None:
    valid = {
        "schema_version": 2,
        "kind": "browser_vm_deployment_receipt",
        "profile": "browser-vm-chromium",
        "image": "browser-vm-chromium",
        "status": "observed",
        "source": "deploy-image.sh",
        "target_host": "172.20.146.225",
        "node_hostname": "Dell",
        "domain_name": "browser-vm",
        "domain_uuid": "01234567-89ab-4cde-8fab-0123456789ab",
        "domain_state": "running",
        "remote_image": "/var/lib/libvirt/images/browser-vm-chromium.qcow2",
        "remote_image_format": "qcow2",
        "attached_disk": "/var/lib/libvirt/images/browser-vm-r1-overlay.qcow2",
        "attached_disk_format": "qcow2",
        "backing_image": "/var/lib/libvirt/images/browser-vm-chromium.qcow2",
        "backing_chain_depth": 1,
        "source_commit": "0123456789abcdef0123456789abcdef01234567",
        "image_digest": "sha256:" + "a" * 64,
        "remote_image_digest": "sha256:" + "a" * 64,
        "recorded_at": datetime.now(timezone.utc).replace(microsecond=0).strftime(
            "%Y-%m-%dT%H:%M:%SZ"
        ),
    }
    assert validate_document(valid)["status"] == "validated"
    for mutation, needle in (
        (lambda value: value.update({"schema_version": 1}), "schema_version"),
        (lambda value: value.update({"domain_state": "shut off"}), "domain"),
        (
            lambda value: value.update({"attached_disk": value["remote_image"]}),
            "writable overlay",
        ),
        (
            lambda value: value.update(
                {"backing_image": "/var/lib/libvirt/images/alternate.qcow2"}
            ),
            "backing_image",
        ),
        (
            lambda value: value.update({"backing_chain_depth": 2}),
            "backing_chain_depth",
        ),
        (lambda value: value.update({"attached_disk_format": "raw"}), "qcow2"),
        (lambda value: value.update({"attached_disk": "relative.qcow2"}), "malformed"),
        (lambda value: value.update({"remote_image_digest": "sha256:" + "b" * 64}), "digest"),
        (lambda value: value.update({"api_token": "never"}), "credential-shaped"),
    ):
        candidate = dict(valid)
        mutation(candidate)
        try:
            validate_document(candidate)
        except EvidenceError as exc:
            assert needle in str(exc), (needle, exc)
        else:
            raise AssertionError(f"accepted invalid deployment receipt: {needle}")

    def fake_stat(mode: int, uid: int, gid: int) -> os.stat_result:
        return os.stat_result((stat.S_IFREG | mode, 1, 1, 1, uid, gid, 0, 0, 0, 0))

    validate_base_metadata(fake_stat(0o440, 0, 107), 107)
    validate_overlay_metadata(fake_stat(0o640, 107, 107), 107, 107)
    for check, metadata, needle in (
        (validate_base_metadata, fake_stat(0o460, 0, 107), "writable"),
        (validate_overlay_metadata, fake_stat(0o640, 0, 107), "qemu"),
        (validate_overlay_metadata, fake_stat(0o642, 107, 107), "other"),
    ):
        try:
            if check is validate_base_metadata:
                check(metadata, 107)
            else:
                check(metadata, 107, 107)
        except EvidenceError as exc:
            assert needle in str(exc), (needle, exc)
        else:
            raise AssertionError(f"accepted unsafe image metadata: {needle}")

    base_path = "/var/lib/libvirt/images/browser-vm-chromium.qcow2"
    overlay_path = "/var/lib/libvirt/images/browser-vm-r1-overlay.qcow2"
    base_info = {"filename": base_path, "format": "qcow2"}
    overlay_info = {
        "filename": overlay_path,
        "format": "qcow2",
        "backing-filename": base_path,
        "full-backing-filename": base_path,
    }
    assert (
        validate_qcow2_graph(
            base_path,
            overlay_path,
            base_info,
            overlay_info,
            [overlay_info, base_info],
        )
        == base_path
    )
    graph_mutations = (
        (base_path, overlay_info, [overlay_info, base_info], "directly attaches"),
        (
            overlay_path,
            dict(overlay_info, **{"backing-filename": "base.qcow2"}),
            [overlay_info, base_info],
            "malformed",
        ),
        (
            overlay_path,
            dict(
                overlay_info,
                **{
                    "backing-filename": "/var/lib/libvirt/images/alternate.qcow2",
                    "full-backing-filename": "/var/lib/libvirt/images/alternate.qcow2",
                },
            ),
            [overlay_info, base_info],
            "does not match",
        ),
        (
            overlay_path,
            overlay_info,
            [overlay_info, base_info, base_info],
            "exactly one backing level",
        ),
        (
            overlay_path,
            overlay_info,
            [overlay_info, overlay_info],
            "filename",
        ),
        (
            overlay_path,
            overlay_info,
            [overlay_info, dict(base_info, **{"backing-filename": base_path})],
            "terminate",
        ),
        (
            overlay_path,
            overlay_info,
            [overlay_info, base_info],
            "remote_image must not have a backing image",
        ),
        (
            overlay_path,
            dict(overlay_info, format="raw"),
            [overlay_info, base_info],
            "format",
        ),
    )
    for index, (attached, info, chain, needle) in enumerate(graph_mutations):
        candidate_base = base_info
        if needle == "remote_image must not have a backing image":
            candidate_base = dict(base_info, **{"backing-filename": base_path})
        try:
            validate_qcow2_graph(base_path, attached, candidate_base, info, chain)
        except EvidenceError as exc:
            assert needle in str(exc), (needle, exc)
        else:
            raise AssertionError(f"accepted invalid qcow2 graph {index}: {needle}")

    valid_xml = (
        "<domain><devices><disk type='file' device='disk'>"
        "<driver name='qemu' type='qcow2'/>"
        f"<source file='{overlay_path}'/><backingStore type='file'>"
        f"<format type='qcow2'/><source file='{base_path}'/><backingStore/>"
        "</backingStore><target dev='vda' bus='virtio'/>"
        "</disk></devices></domain>"
    )
    assert validate_vda_xml(valid_xml) == (overlay_path, base_path)
    duplicate_disk = valid_xml.split("<devices>", 1)[1].split("</devices>", 1)[0]
    for xml_text, needle in (
        (valid_xml.replace("</devices>", duplicate_disk + "</devices>"), "exactly one vda"),
        (valid_xml.replace(overlay_path, "relative.qcow2"), "malformed"),
        (valid_xml.replace("device='disk'", "device='cdrom'"), "file-backed disk"),
        (valid_xml.replace("</disk>", "<readonly/></disk>"), "must be writable"),
        (valid_xml.replace("<backingStore type='file'>", ""), "malformed"),
        (valid_xml.replace("type='qcow2'", "type='raw'", 1), "driver"),
        (valid_xml.replace("<format type='qcow2'/>", "<format type='raw'/>"), "format"),
        (
            valid_xml.replace(
                "<backingStore/>",
                f"<backingStore type='file'><source file='{base_path}'/></backingStore>",
            ),
            "terminate",
        ),
    ):
        try:
            validate_vda_xml(xml_text)
        except EvidenceError as exc:
            assert needle in str(exc), (needle, exc)
        else:
            raise AssertionError(f"accepted invalid vda XML: {needle}")
    print("verify-browser-vm-deployment: self-test passed")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs="?", choices=("validate", "probe-live"))
    parser.add_argument("path", nargs="?", type=Path)
    parser.add_argument("--remote-image")
    parser.add_argument("--domain")
    parser.add_argument("--expected-digest")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.self_test:
            if (
                args.command is not None
                or args.path is not None
                or args.remote_image is not None
                or args.domain is not None
                or args.expected_digest is not None
            ):
                parser.error("--self-test does not accept a command or path")
            self_test()
            return 0
        if args.command == "validate":
            if (
                args.path is None
                or args.remote_image is not None
                or args.domain is not None
                or args.expected_digest is not None
            ):
                parser.error("validate requires exactly one receipt path")
            print(json.dumps(validate_document(read_json(args.path)), sort_keys=True))
            return 0
        if args.command == "probe-live":
            if args.path is not None:
                parser.error("probe-live does not accept a receipt path")
            if not args.remote_image or not args.domain or not args.expected_digest:
                parser.error(
                    "probe-live requires --remote-image, --domain, and --expected-digest"
                )
            print(
                json.dumps(
                    probe_live(args.remote_image, args.domain, args.expected_digest),
                    sort_keys=True,
                    separators=(",", ":"),
                )
            )
            return 0
        parser.error("use validate receipt.json, probe-live options, or --self-test")
        return 0
    except EvidenceError as exc:
        print(f"verify-browser-vm-deployment: rejected: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
