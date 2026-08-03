#!/usr/bin/env python3
"""Validate a Browser VM deployment receipt.

The receipt is produced after the immutable image is installed and attached to
the running Browser VM domain. It binds live acceptance artifacts to a target
node and domain instead of accepting records from any guest carrying the same
image digest.

Usage:
  verify-browser-vm-deployment.py validate receipt.json
  verify-browser-vm-deployment.py --self-test
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import sys
from typing import Any, NoReturn


SCHEMA_VERSION = 1
MAX_FILE_BYTES = 1024 * 1024
COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
IMAGE_DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
HOST_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:-]{0,252}$")
NAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$")
UUID_RE = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
UTC_TIMESTAMP_RE = re.compile(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$")
PATH_RE = re.compile(r"^/[A-Za-z0-9._/@+-]+$")
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
        "attached_disk",
        "source_commit",
        "image_digest",
        "remote_image_digest",
        "recorded_at",
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
    if ".." in Path(value).parts:
        fail(f"{field} must not contain path traversal")
    return value


def validate_document(data: Any) -> dict[str, Any]:
    if not isinstance(data, dict):
        fail("receipt root must be one JSON object")
    reject_credential_fields(data)
    if frozenset(data) != EXPECTED_FIELDS:
        missing = EXPECTED_FIELDS - frozenset(data)
        extra = frozenset(data) - EXPECTED_FIELDS
        if missing:
            fail(f"receipt is missing fields: {', '.join(sorted(missing))}")
        fail(f"receipt has unexpected fields: {', '.join(sorted(extra))}")
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
    if attached_disk != remote_image:
        fail("attached_disk does not match remote_image")
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
        "source_commit": source_commit,
        "image_digest": image_digest,
    }


def self_test() -> None:
    valid = {
        "schema_version": 1,
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
        "attached_disk": "/var/lib/libvirt/images/browser-vm-chromium.qcow2",
        "source_commit": "0123456789abcdef0123456789abcdef01234567",
        "image_digest": "sha256:" + "a" * 64,
        "remote_image_digest": "sha256:" + "a" * 64,
        "recorded_at": datetime.now(timezone.utc).replace(microsecond=0).strftime(
            "%Y-%m-%dT%H:%M:%SZ"
        ),
    }
    assert validate_document(valid)["status"] == "validated"
    for mutation, needle in (
        (lambda value: value.update({"domain_state": "shut off"}), "domain"),
        (lambda value: value.update({"attached_disk": "/tmp/other.qcow2"}), "attached_disk"),
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
    print("verify-browser-vm-deployment: self-test passed")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs="?", choices=("validate",))
    parser.add_argument("path", nargs="?", type=Path)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.self_test:
            if args.command is not None or args.path is not None:
                parser.error("--self-test does not accept a command or path")
            self_test()
            return 0
        if args.command != "validate" or args.path is None:
            parser.error("use validate receipt.json or --self-test")
        print(json.dumps(validate_document(read_json(args.path)), sort_keys=True))
        return 0
    except EvidenceError as exc:
        print(f"verify-browser-vm-deployment: rejected: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
