#!/usr/bin/env python3
"""Validate the bounded Browser VM runtime-evidence.json record.

This helper validates the guest-owned record emitted by
``mcnf-browser-vm-runtime.sh``.  It proves only that the guest session saw
bounded playback/capture endpoints when it wrote the record.  It does not
prove audible Chromium playback, captured samples, mute/volume behavior, or
recovery after reconnect; those claims have no accepted fields in this
schema.

The parser is deliberately fail-closed: input is size-limited, must be a
regular non-symlink file, must be one JSON object with exactly the admitted
fields, rejects duplicate keys and credential-shaped keys, and rejects JSON
constants such as NaN or Infinity.

Usage:
  verify-browser-vm-runtime-evidence.py validate runtime-evidence.json
  verify-browser-vm-runtime-evidence.py --self-test
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
MAX_FILE_BYTES = 64 * 1024
MAX_ENDPOINTS = 32
MAX_FUTURE_SKEW_SECONDS = 300

EXPECTED_FIELDS = frozenset(
    {
        "schema_version",
        "kind",
        "profile",
        "image",
        "transport",
        "transport_health",
        "gpu_status",
        "audio_status",
        "audio_playback_endpoints",
        "audio_capture_endpoints",
        "recorded_at",
    }
)

ALLOWED_TRANSPORTS = frozenset({"rdp", "spice"})
ALLOWED_TRANSPORT_HEALTH = frozenset(
    {"connected", "reconnecting", "failed", "unavailable"}
)
ALLOWED_GPU_STATUS = frozenset({"passed", "unavailable"})
ALLOWED_AUDIO_STATUS = frozenset({"wired", "unavailable"})

# These names are rejected independently of the exact-schema check so a
# credential-shaped extra field receives an explicit, stable failure reason.
CREDENTIAL_FIELD_RE = re.compile(
    r"(?:pass(?:word|phrase)?|secret|token|ticket|credential|bearer|"
    r"api[_-]?key|access[_-]?key|private[_-]?key|cookie|authorization|"
    r"identity[_-]?file|pem)",
    re.IGNORECASE,
)

UTC_TIMESTAMP_RE = re.compile(
    r"^(?P<date>[0-9]{4}-[0-9]{2}-[0-9]{2}T"
    r"[0-9]{2}:[0-9]{2}:[0-9]{2})Z$"
)


class EvidenceError(Exception):
    """A fail-closed runtime evidence validation error."""


def fail(message: str) -> NoReturn:
    raise EvidenceError(message)


def reject_json_constant(value: str) -> NoReturn:
    fail(f"non-finite JSON number is not allowed: {value}")


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
    except OSError:
        fail(f"evidence file is not readable: {path}")
    if os.path.islink(path):
        fail("evidence file must not be a symlink")
    if not path.is_file():
        fail("evidence path must be a regular file")
    if stat_result.st_mode & 0o077:
        fail("evidence file must not be accessible to group or other")
    if stat_result.st_mode & 0o111:
        fail("evidence file must not be executable")
    if stat_result.st_size > MAX_FILE_BYTES:
        fail(f"evidence file exceeds {MAX_FILE_BYTES} bytes")
    try:
        raw = path.read_bytes()
    except OSError as exc:
        fail(f"evidence file cannot be read: {exc}")
    if len(raw) > MAX_FILE_BYTES:
        fail(f"evidence file exceeds {MAX_FILE_BYTES} bytes")
    try:
        return json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=reject_duplicate_keys,
            parse_constant=reject_json_constant,
        )
    except RecursionError as exc:
        fail(f"evidence JSON nesting exceeds the bounded parser: {exc}")
    except UnicodeDecodeError as exc:
        fail(f"evidence is not UTF-8: {exc}")
    except json.JSONDecodeError as exc:
        fail(f"malformed JSON: {exc.msg}")


def reject_credential_fields(value: Any, location: str = "root") -> None:
    """Reject credential-shaped keys even before schema-specific checks."""
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


def require_string(data: dict[str, Any], field: str, allowed: frozenset[str]) -> str:
    value = data.get(field)
    if not isinstance(value, str) or value not in allowed:
        fail(f"{field} must be one of: {', '.join(sorted(allowed))}")
    return value


def require_bounded_uint(data: dict[str, Any], field: str) -> int:
    value = data.get(field)
    if isinstance(value, bool) or not isinstance(value, int):
        fail(f"{field} must be a bounded integer")
    if not 0 <= value <= MAX_ENDPOINTS:
        fail(f"{field} must be between 0 and {MAX_ENDPOINTS}")
    return value


def validate_timestamp(value: Any) -> None:
    if not isinstance(value, str):
        fail("recorded_at must be a UTC timestamp")
    match = UTC_TIMESTAMP_RE.fullmatch(value)
    if not match:
        fail("recorded_at must use second-precision UTC form YYYY-MM-DDTHH:MM:SSZ")
    try:
        recorded_at = datetime.strptime(
            match.group("date"), "%Y-%m-%dT%H:%M:%S"
        ).replace(tzinfo=timezone.utc)
    except ValueError as exc:
        fail(f"recorded_at is not a real UTC timestamp: {exc}")
    if recorded_at.timestamp() > (
        datetime.now(timezone.utc).timestamp() + MAX_FUTURE_SKEW_SECONDS
    ):
        fail("recorded_at is too far in the future")


def validate_document(data: Any) -> dict[str, Any]:
    if not isinstance(data, dict):
        fail("evidence root must be one JSON object")
    reject_credential_fields(data)
    fields = frozenset(data)
    missing = EXPECTED_FIELDS - fields
    extra = fields - EXPECTED_FIELDS
    if missing:
        fail(f"missing evidence fields: {', '.join(sorted(missing))}")
    if extra:
        fail(f"unexpected evidence fields: {', '.join(sorted(extra))}")

    if data["schema_version"] != SCHEMA_VERSION or isinstance(data["schema_version"], bool):
        fail(f"schema_version must be integer {SCHEMA_VERSION}")
    if data["kind"] != "browser_vm_runtime_evidence":
        fail("kind is not the admitted Browser VM runtime evidence kind")
    if data["profile"] != "browser-vm-chromium":
        fail("profile is not browser-vm-chromium")
    if data["image"] != "browser-vm-chromium":
        fail("image is not browser-vm-chromium")

    transport = require_string(data, "transport", ALLOWED_TRANSPORTS)
    transport_health = require_string(data, "transport_health", ALLOWED_TRANSPORT_HEALTH)
    gpu_status = require_string(data, "gpu_status", ALLOWED_GPU_STATUS)
    audio_status = require_string(data, "audio_status", ALLOWED_AUDIO_STATUS)
    playback = require_bounded_uint(data, "audio_playback_endpoints")
    capture = require_bounded_uint(data, "audio_capture_endpoints")
    validate_timestamp(data["recorded_at"])

    if audio_status == "wired" and (playback == 0 or capture == 0):
        fail("audio_status=wired requires at least one playback and capture endpoint")
    if audio_status == "unavailable" and playback > 0 and capture > 0:
        fail("audio_status=unavailable contradicts non-empty playback and capture endpoints")

    # The accepted record has no sample, Chromium, mute, capture, or recovery
    # fields. Keep this classification explicit so callers cannot mistake
    # endpoint wiring for live media proof.
    endpoint_wiring = "observed" if audio_status == "wired" else "unavailable"
    return {
        "status": "validated" if endpoint_wiring == "observed" else "unavailable",
        "evidence_class": "endpoint_wiring",
        "endpoint_wiring": endpoint_wiring,
        "live_proof": "unavailable",
        "transport": transport,
        "transport_health": transport_health,
        "gpu_status": gpu_status,
        "audio_playback_endpoints": playback,
        "audio_capture_endpoints": capture,
        "reason": (
            "endpoint counts prove guest-visible wiring only; live Chromium playback, "
            "capture samples, and reconnect recovery are not represented"
        ),
    }


def validate_path(path: Path) -> int:
    result = validate_document(read_json(path))
    print(json.dumps(result, sort_keys=True))
    return 0 if result["status"] == "validated" else 1


def make_valid() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "kind": "browser_vm_runtime_evidence",
        "profile": "browser-vm-chromium",
        "image": "browser-vm-chromium",
        "transport": "rdp",
        "transport_health": "connected",
        "gpu_status": "unavailable",
        "audio_status": "wired",
        "audio_playback_endpoints": 1,
        "audio_capture_endpoints": 1,
        "recorded_at": "2020-01-02T03:04:05Z",
    }


def assert_rejected(data: Any, needle: str) -> None:
    try:
        validate_document(data)
    except EvidenceError as exc:
        assert needle in str(exc), (needle, str(exc))
    else:
        raise AssertionError(f"accepted invalid evidence containing {needle!r}")


def self_test() -> None:
    valid = make_valid()
    result = validate_document(valid)
    assert result["status"] == "validated"
    assert result["endpoint_wiring"] == "observed"
    assert result["live_proof"] == "unavailable"

    unavailable = dict(valid, audio_status="unavailable", audio_capture_endpoints=0)
    unavailable_result = validate_document(unavailable)
    assert unavailable_result["status"] == "unavailable"
    assert unavailable_result["endpoint_wiring"] == "unavailable"
    assert_rejected(dict(valid, extra="no"), "unexpected evidence fields")
    assert_rejected(dict(valid, live_proof="passed"), "unexpected evidence fields")
    assert_rejected(dict(valid, password="not-a-secret"), "credential-shaped field")
    assert_rejected(dict(valid, audio_status="wired", audio_capture_endpoints=0), "requires")
    assert_rejected(dict(valid, audio_playback_endpoints=True), "bounded integer")
    assert_rejected(dict(valid, recorded_at="2026-99-99T12:00:00Z"), "real UTC")
    assert_rejected(dict(valid, recorded_at="2099-01-01T00:00:00Z"), "future")
    try:
        json.loads(
            '{"schema_version":1,"schema_version":1}',
            object_pairs_hook=reject_duplicate_keys,
        )
    except EvidenceError as exc:
        assert "duplicate JSON field" in str(exc)
    else:
        raise AssertionError("accepted duplicate JSON field")


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
            print("verify-browser-vm-runtime-evidence: self-test passed")
            return 0
        if args.command != "validate" or args.path is None:
            parser.error("use validate runtime-evidence.json or --self-test")
        return validate_path(args.path)
    except EvidenceError as exc:
        print(f"verify-browser-vm-runtime-evidence: rejected: {exc}", file=sys.stderr)
        return 2
    except RecursionError as exc:
        print(
            f"verify-browser-vm-runtime-evidence: rejected: bounded parser recursion exceeded: {exc}",
            file=sys.stderr,
        )
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
