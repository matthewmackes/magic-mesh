#!/usr/bin/env python3
"""Validate guest-local Chromium media-decode evidence.

The Browser VM probe runs an image-owned fixed VP8/Opus fixture through
Chromium and records bounded media element/frame counters. This proves only
that the guest-local Chromium probe observed decode state; it never proves
VDI rendering, audible audio, GPU hardware acceleration, or reconnect recovery.
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


EXPECTED_FIELDS = frozenset(
    {
        "schema_version",
        "kind",
        "profile",
        "image",
        "source_commit",
        "image_digest",
        "status",
        "source",
        "video_ready_state",
        "video_total_frames",
        "video_dropped_frames",
        "video_width",
        "video_height",
        "audio_ready_state",
        "recorded_at",
    }
)
MAX_FILE_BYTES = 64 * 1024
MAX_COUNTER = 1_000_000
MAX_DIMENSION = 16_384
COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
IMAGE_DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
UTC_TIMESTAMP_RE = re.compile(
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"
)
CREDENTIAL_FIELD_RE = re.compile(
    r"(?:pass(?:word|phrase)?|secret|token|ticket|credential|bearer|"
    r"api[_-]?key|access[_-]?key|private[_-]?key|cookie|authorization)",
    re.IGNORECASE,
)


class EvidenceError(Exception):
    """The record cannot support the guest-local media claim."""


def fail(message: str) -> NoReturn:
    raise EvidenceError(message)


def reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            fail(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def reject_json_constant(value: str) -> NoReturn:
    fail(f"non-finite JSON number is not allowed: {value}")


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
        raw = path.read_bytes()
        return json.loads(
            raw.decode("utf-8"),
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
    if data["schema_version"] != 1 or isinstance(data["schema_version"], bool):
        fail("schema_version must be integer 1")
    if data["kind"] != "browser_vm_media_probe":
        fail("kind is not the admitted Browser VM media evidence kind")
    if data["profile"] != "browser-vm-chromium" or data["image"] != "browser-vm-chromium":
        fail("profile and image must identify browser-vm-chromium")
    source_commit = data.get("source_commit")
    if not isinstance(source_commit, str) or COMMIT_RE.fullmatch(source_commit) is None:
        fail("source_commit must be a 40-character lowercase Git revision")
    if source_commit == "0" * 40:
        fail("source_commit must not be the null revision")
    image_digest = data.get("image_digest")
    if not isinstance(image_digest, str) or IMAGE_DIGEST_RE.fullmatch(image_digest) is None:
        fail("image_digest must be an immutable sha256 digest")
    if image_digest == "sha256:" + "0" * 64:
        fail("image_digest must not be the null digest")
    if data["status"] not in {"passed", "unavailable"}:
        fail("status must be passed or unavailable")
    if data["source"] != "guest-local-fixed-mkv":
        fail("source must identify the image-owned fixed fixture")
    ready = bounded_uint(data, "video_ready_state", 4)
    total = bounded_uint(data, "video_total_frames", MAX_COUNTER)
    dropped = bounded_uint(data, "video_dropped_frames", MAX_COUNTER)
    width = bounded_uint(data, "video_width", MAX_DIMENSION)
    height = bounded_uint(data, "video_height", MAX_DIMENSION)
    audio_ready = bounded_uint(data, "audio_ready_state", 4)
    validate_timestamp(data["recorded_at"])
    if data["status"] == "passed" and (ready < 2 or total == 0 or width == 0 or height == 0):
        fail("passed media evidence requires decoded video frames and dimensions")
    return {
        "status": "validated" if data["status"] == "passed" else "unavailable",
        "evidence_class": "guest_media_decode",
        "live_proof": "unavailable",
        "source_commit": source_commit,
        "image_digest": image_digest,
        "video_ready_state": ready,
        "video_total_frames": total,
        "video_dropped_frames": dropped,
        "video_width": width,
        "video_height": height,
        "audio_ready_state": audio_ready,
        "reason": (
            "guest-local Chromium decode counters only; this record does not prove "
            "GPU hardware acceleration, audible audio, VDI presentation, or recovery"
        ),
    }


def valid_record() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "kind": "browser_vm_media_probe",
        "profile": "browser-vm-chromium",
        "image": "browser-vm-chromium",
        "source_commit": "0123456789abcdef0123456789abcdef01234567",
        "image_digest": "sha256:" + "a" * 64,
        "status": "passed",
        "source": "guest-local-fixed-mkv",
        "video_ready_state": 4,
        "video_total_frames": 8,
        "video_dropped_frames": 0,
        "video_width": 64,
        "video_height": 64,
        "audio_ready_state": 4,
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
    assert result["live_proof"] == "unavailable"
    assert_rejected(dict(valid_record(), extra="no"), "unexpected evidence fields")
    assert_rejected(dict(valid_record(), password="no"), "credential-shaped field")
    assert_rejected(dict(valid_record(), status="passed", video_total_frames=0), "decoded")
    assert_rejected(dict(valid_record(), video_width=True), "integer")
    assert_rejected(dict(valid_record(), recorded_at="2026-99-99T00:00:00Z"), "real UTC")
    unavailable = dict(valid_record(), status="unavailable", video_total_frames=0)
    assert validate_document(unavailable)["status"] == "unavailable"


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
            print("verify-browser-vm-media-evidence: self-test passed")
            return 0
        if args.command != "validate" or args.path is None:
            parser.error("use validate media-evidence.json or --self-test")
        result = validate_document(read_json(args.path))
        print(json.dumps(result, sort_keys=True))
        return 0 if result["status"] == "validated" else 1
    except EvidenceError as exc:
        print(f"verify-browser-vm-media-evidence: rejected: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
