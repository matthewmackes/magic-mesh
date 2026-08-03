#!/usr/bin/env python3
"""Validate the bounded live Browser VM performance acceptance record.

This verifier accepts only evidence collected from a booted Browser VM and its
VDI session.  It does not run a local benchmark or synthesize live readiness.
The passing record must cover five concurrent 1080p tabs for at least fifteen
minutes, the frame/stall threshold, focused pointer activity, navigation and
session latency, partial uploads, hidden repaint, and reconnect recovery.

Usage:
  verify-browser-vm-performance.py validate performance-evidence.json
  verify-browser-vm-performance.py --self-test
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
MAX_DURATION_SECONDS = 24 * 60 * 60
MAX_TABS = 16
MAX_DIMENSION = 16_384
MAX_FPS = 240
MAX_STALL_MS = 60_000
MAX_LATENCY_MS = 600_000
MAX_COUNTER = 10_000_000
EXPECTED_FIELDS = frozenset(
    {
        "schema_version",
        "kind",
        "profile",
        "image",
        "status",
        "source",
        "source_commit",
        "image_digest",
        "transport",
        "duration_seconds",
        "tab_count",
        "viewport_width",
        "viewport_height",
        "min_fps",
        "max_stall_ms",
        "pointer_updates",
        "navigation_p95_ms",
        "session_latency_p95_ms",
        "partial_uploads",
        "hidden_repaints",
        "reconnects",
        "recovery_observed",
        "recorded_at",
    }
)
UTC_TIMESTAMP_RE = re.compile(
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"
)
CREDENTIAL_FIELD_RE = re.compile(
    r"(?:pass(?:word|phrase)?|secret|token|ticket|credential|bearer|"
    r"api[_-]?key|access[_-]?key|private[_-]?key|cookie|authorization|"
    r"identity[_-]?file|pem)",
    re.IGNORECASE,
)
SHA256_RE = re.compile(r"^sha256:[0-9a-fA-F]{64}$")
COMMIT_RE = re.compile(r"^[0-9a-fA-F]{40}$")


class EvidenceError(Exception):
    """The record cannot support live performance acceptance."""


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
    except OSError as exc:
        fail(f"evidence file is not readable: {exc}")
    if os.path.islink(path) or not path.is_file():
        fail("evidence path must be a regular non-symlink file")
    if stat_result.st_mode & 0o077 or stat_result.st_mode & 0o111:
        fail("evidence file must be private and non-executable")
    if stat_result.st_size > MAX_FILE_BYTES:
        fail(f"evidence file exceeds {MAX_FILE_BYTES} bytes")
    try:
        return json.loads(
            path.read_bytes().decode("utf-8"),
            object_pairs_hook=reject_duplicate_keys,
            parse_constant=reject_json_constant,
        )
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail(f"malformed evidence JSON: {exc}")


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


def bounded_uint(data: dict[str, Any], field: str, maximum: int) -> int:
    value = data.get(field)
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= maximum:
        fail(f"{field} must be an integer between 0 and {maximum}")
    return value


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
    if recorded.timestamp() > datetime.now(timezone.utc).timestamp() + 300:
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
    if data["kind"] != "browser_vm_performance":
        fail("kind is not the admitted Browser VM performance evidence kind")
    if data["profile"] != "browser-vm-chromium" or data["image"] != "browser-vm-chromium":
        fail("profile and image must identify browser-vm-chromium")
    status = data["status"]
    if status not in {"passed", "failed", "unavailable"}:
        fail("status must be passed, failed, or unavailable")
    if data["source"] != "live-browser-vm-acceptance":
        fail("source must identify the live Browser VM acceptance harness")
    require_string(data, "source_commit", COMMIT_RE)
    require_string(data, "image_digest", SHA256_RE)
    if data["transport"] not in {"rdp", "spice"}:
        fail("transport must be rdp or spice")
    duration = bounded_uint(data, "duration_seconds", MAX_DURATION_SECONDS)
    tabs = bounded_uint(data, "tab_count", MAX_TABS)
    width = bounded_uint(data, "viewport_width", MAX_DIMENSION)
    height = bounded_uint(data, "viewport_height", MAX_DIMENSION)
    min_fps = bounded_uint(data, "min_fps", MAX_FPS)
    max_stall = bounded_uint(data, "max_stall_ms", MAX_STALL_MS)
    pointers = bounded_uint(data, "pointer_updates", MAX_COUNTER)
    nav_p95 = bounded_uint(data, "navigation_p95_ms", MAX_LATENCY_MS)
    session_p95 = bounded_uint(data, "session_latency_p95_ms", MAX_LATENCY_MS)
    partial = bounded_uint(data, "partial_uploads", MAX_COUNTER)
    hidden = bounded_uint(data, "hidden_repaints", MAX_COUNTER)
    reconnects = bounded_uint(data, "reconnects", MAX_COUNTER)
    recovery = data["recovery_observed"]
    if not isinstance(recovery, bool):
        fail("recovery_observed must be boolean")
    validate_timestamp(data["recorded_at"])

    if status == "passed":
        requirements = {
            "duration_seconds": duration >= 900,
            "tab_count": tabs >= 5,
            "viewport": width >= 1920 and height >= 1080,
            "min_fps": min_fps >= 30,
            "max_stall_ms": max_stall <= 500,
            "pointer_updates": pointers > 0,
            "navigation_p95_ms": nav_p95 > 0,
            "session_latency_p95_ms": session_p95 > 0,
            "partial_uploads": partial > 0,
            "hidden_repaints": hidden > 0,
            "reconnects": reconnects > 0,
            "recovery_observed": recovery,
        }
        missing_requirements = [name for name, met in requirements.items() if not met]
        if missing_requirements:
            fail(
                "passed performance evidence misses acceptance criteria: "
                + ", ".join(missing_requirements)
            )

    return {
        "status": "validated" if status == "passed" else status,
        "evidence_class": "live_browser_vm_performance",
        "live_proof": "observed" if status == "passed" else "unavailable",
        "transport": data["transport"],
        "duration_seconds": duration,
        "tab_count": tabs,
        "min_fps": min_fps,
        "max_stall_ms": max_stall,
        "reconnects": reconnects,
        "reason": (
            "all Browser VM performance acceptance criteria were observed"
            if status == "passed"
            else "live performance acceptance is not available"
        ),
    }


def valid_record() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "kind": "browser_vm_performance",
        "profile": "browser-vm-chromium",
        "image": "browser-vm-chromium",
        "status": "passed",
        "source": "live-browser-vm-acceptance",
        "source_commit": "0123456789abcdef0123456789abcdef01234567",
        "image_digest": "sha256:" + "a" * 64,
        "transport": "rdp",
        "duration_seconds": 900,
        "tab_count": 5,
        "viewport_width": 1920,
        "viewport_height": 1080,
        "min_fps": 30,
        "max_stall_ms": 500,
        "pointer_updates": 1,
        "navigation_p95_ms": 1,
        "session_latency_p95_ms": 1,
        "partial_uploads": 1,
        "hidden_repaints": 1,
        "reconnects": 1,
        "recovery_observed": True,
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
    result = validate_document(valid_record())
    assert result["status"] == "validated"
    assert result["live_proof"] == "observed"
    assert_rejected(dict(valid_record(), extra="no"), "unexpected evidence fields")
    assert_rejected(dict(valid_record(), password="no"), "credential-shaped field")
    assert_rejected(dict(valid_record(), duration_seconds=899), "duration_seconds")
    assert_rejected(dict(valid_record(), min_fps=29), "min_fps")
    assert_rejected(dict(valid_record(), max_stall_ms=501), "max_stall_ms")
    assert_rejected(dict(valid_record(), recovery_observed=False), "recovery_observed")
    assert_rejected(dict(valid_record(), source_commit="short"), "source_commit")
    assert_rejected(dict(valid_record(), recorded_at="2026-99-99T00:00:00Z"), "real UTC")
    unavailable = dict(valid_record(), status="unavailable", duration_seconds=0, tab_count=0)
    assert validate_document(unavailable)["status"] == "unavailable"
    print("verify-browser-vm-performance: self-test passed")


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
            parser.error("use validate performance-evidence.json or --self-test")
        result = validate_document(read_json(args.path))
        print(json.dumps(result, sort_keys=True))
        return 0 if result["status"] == "validated" else 1
    except EvidenceError as exc:
        print(f"verify-browser-vm-performance: rejected: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
