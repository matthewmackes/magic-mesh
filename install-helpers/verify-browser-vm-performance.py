#!/usr/bin/env python3
"""Validate the bounded live Browser VM performance acceptance record.

This verifier accepts only evidence collected from a booted Browser VM and its
VDI session.  It does not run a local benchmark or synthesize live readiness.
A pass requires the version-4 challenged live stream provenance emitted by the
collector, distinct host/guest/domain identities, an observed 15-minute wall
window, immutable browser/workload/desktop-source and tab identities, every
tab's 1080p geometry and positive playback progress, visible shell-delivered
cadence, coordinate-derived
stationary-pointer and counter-derived hidden-surface windows, bounded
navigation/session latency, direct full/partial upload counters, broadly sampled
host-process CPU and DRM GPU load, and reconnect recovery. RDP is the default
transport; Sunshine/Moonlight must be selected explicitly.

Usage:
  verify-browser-vm-performance.py validate performance-evidence.json
  verify-browser-vm-performance.py --self-test
"""

from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import re
import sys
from typing import Any, NoReturn


SCHEMA_VERSION = 5
MAX_FILE_BYTES = 64 * 1024
MAX_DURATION_SECONDS = 24 * 60 * 60
MAX_TABS = 16
MAX_DIMENSION = 16_384
MAX_FPS = 240
MAX_HOST_LOAD_PERMILLE = 100_000
MAX_STALL_MS = 60_000
MAX_LATENCY_MS = 600_000
MAX_LATENCY_SAMPLES = 10_000
MAX_SAMPLE_INTERVALS = 4096
MAX_COUNTER = 10_000_000
LATENCY_P95_LIMIT_MS = 100
LATENCY_MAX_LIMIT_MS = 250
MIN_NAVIGATION_LATENCY_SAMPLES = 15
MIN_SESSION_LATENCY_SAMPLES = 15
HIDDEN_QUIESCENT_WINDOW_MS = 5 * 60 * 1000
MAX_HIDDEN_REPAINT_BURST_MS = 20_000
MAX_HIDDEN_REPAINT_ACTIVE_FRACTION = 0.05
MIN_CADENCE_RATIO_PERMILLE = 900
MIN_SUPPORTED_TARGET_FPS = 30
MIN_VISIBLE_DELIVERY_FPS = (
    MIN_SUPPORTED_TARGET_FPS * MIN_CADENCE_RATIO_PERMILLE + 999
) // 1_000
STATIONARY_POINTER_WINDOW_MS = 5 * 60 * 1000
MIN_LIVE_SAMPLE_COUNT = 91
MIN_MEDIA_TAB_COUNT = 5
MIN_HOST_LOAD_COVERAGE_PERMILLE = 900
MAX_EVIDENCE_AGE_SECONDS = 24 * 60 * 60
DEFAULT_TRANSPORT = "rdp"
ADMITTED_TRANSPORTS = (DEFAULT_TRANSPORT, "sunshine")
LIVE_COLLECTION_MODE = "live-endpoint-v4"
FIXTURE_COLLECTION_MODE = "fixture-dry-run"
SHELL_METRICS_SOURCE = "mde-shell-egui-vdi"
GUEST_METRICS_SOURCE = "chromium-devtools"
EXPECTED_FIELDS = frozenset(
    {
        "schema_version",
        "kind",
        "profile",
        "image",
        "workload",
        "status",
        "source",
        "source_commit",
        "image_digest",
        "transport",
        "collection_mode",
        "collection_nonce",
        "stream_sha256",
        "stream_session_id",
        "shell_metrics_source",
        "guest_metrics_source",
        "host_boot_id",
        "guest_boot_id",
        "domain_uuid",
        "browser_instance_id",
        "workload_instance_id",
        "source_instance_id",
        "observation_started_at",
        "wall_observation_ms",
        "sample_count",
        "duration_seconds",
        "tab_count",
        "tab_source_count",
        "tab_set_sha256",
        "tab_source_width",
        "tab_source_height",
        "viewport_width",
        "viewport_height",
        "min_fps",
        "source_fps",
        "cadence_ratio_permille",
        "supported_target_fps",
        "max_stall_ms",
        "pointer_updates",
        "stationary_pointer_continuous_ms",
        "navigation_latency_sample_count",
        "navigation_p95_ms",
        "navigation_max_ms",
        "session_latency_sample_count",
        "session_latency_p95_ms",
        "session_latency_max_ms",
        "full_uploads",
        "partial_uploads",
        "partial_rects",
        "host_process_cpu_sample_count",
        "host_process_cpu_p95_permille",
        "host_process_cpu_max_permille",
        "host_gpu_busy_sample_count",
        "host_gpu_busy_p95_permille",
        "host_gpu_busy_max_permille",
        "hidden_repaints",
        "hidden_repaint_active_intervals",
        "hidden_repaint_interval_count",
        "hidden_observation_ms",
        "hidden_repaint_longest_burst_ms",
        "hidden_repaint_quiescent_ms",
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
SHA256_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
COLLECTION_NONCE_RE = re.compile(r"^[0-9a-f]{64}$")
SESSION_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$")
SYNTHETIC_SESSION_RE = re.compile(r"(?:fixture|synthetic|dry[-_.]?run|self[-_.]?test)", re.I)
UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
)


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


def validate_timestamp(value: Any, field: str = "recorded_at") -> datetime:
    if not isinstance(value, str) or UTC_TIMESTAMP_RE.fullmatch(value) is None:
        fail(f"{field} must use second-precision UTC form YYYY-MM-DDTHH:MM:SSZ")
    try:
        recorded = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(
            tzinfo=timezone.utc
        )
    except ValueError as exc:
        fail(f"{field} is not a real UTC timestamp: {exc}")
    if recorded.timestamp() > datetime.now(timezone.utc).timestamp() + 300:
        fail(f"{field} is too far in the future")
    return recorded


def optional_string(
    data: dict[str, Any], field: str, pattern: re.Pattern[str]
) -> str | None:
    value = data.get(field)
    if value is None:
        return None
    if not isinstance(value, str) or pattern.fullmatch(value) is None:
        fail(f"{field} is malformed")
    return value


def optional_exact_string(data: dict[str, Any], field: str) -> str | None:
    value = data.get(field)
    if value is not None and not isinstance(value, str):
        fail(f"{field} must be a string or null")
    return value


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
    if data["workload"] != "browser-vm":
        fail("workload must identify browser-vm")
    status = data["status"]
    if status not in {"passed", "failed", "unavailable"}:
        fail("status must be passed, failed, or unavailable")
    if data["source"] != "live-browser-vm-acceptance":
        fail("source must identify the live Browser VM acceptance harness")
    source_commit = require_string(data, "source_commit", COMMIT_RE)
    image_digest = require_string(data, "image_digest", SHA256_RE)
    if source_commit == "0" * 40:
        fail("source_commit must be non-null")
    if image_digest == "sha256:" + "0" * 64:
        fail("image_digest must be non-null")
    if data["transport"] not in ADMITTED_TRANSPORTS:
        fail("transport must be rdp or explicit sunshine")
    collection_mode = data["collection_mode"]
    if collection_mode not in {LIVE_COLLECTION_MODE, FIXTURE_COLLECTION_MODE}:
        fail("collection_mode is not admitted")
    collection_nonce = optional_string(data, "collection_nonce", COLLECTION_NONCE_RE)
    stream_sha256 = optional_string(data, "stream_sha256", SHA256_RE)
    stream_session_id = optional_string(data, "stream_session_id", SESSION_ID_RE)
    shell_metrics_source = optional_exact_string(data, "shell_metrics_source")
    guest_metrics_source = optional_exact_string(data, "guest_metrics_source")
    host_boot_id = optional_string(data, "host_boot_id", UUID_RE)
    guest_boot_id = optional_string(data, "guest_boot_id", UUID_RE)
    domain_uuid = optional_string(data, "domain_uuid", UUID_RE)
    browser_instance_id = optional_string(data, "browser_instance_id", UUID_RE)
    workload_instance_id = optional_string(data, "workload_instance_id", UUID_RE)
    source_instance_id = optional_string(data, "source_instance_id", UUID_RE)
    tab_set_sha256 = optional_string(data, "tab_set_sha256", SHA256_RE)
    for field, value in (
        ("collection_nonce", collection_nonce),
        ("stream_sha256", stream_sha256),
        ("host_boot_id", host_boot_id),
        ("guest_boot_id", guest_boot_id),
        ("domain_uuid", domain_uuid),
        ("browser_instance_id", browser_instance_id),
        ("workload_instance_id", workload_instance_id),
        ("source_instance_id", source_instance_id),
        ("tab_set_sha256", tab_set_sha256),
    ):
        if value is not None and set(value.removeprefix("sha256:").replace("-", "")) == {"0"}:
            fail(f"{field} must be non-null")

    observation_started = validate_timestamp(
        data["observation_started_at"], "observation_started_at"
    )
    recorded_at = validate_timestamp(data["recorded_at"])
    if observation_started > recorded_at:
        fail("observation_started_at cannot follow recorded_at")
    wall_observation_ms = bounded_uint(
        data, "wall_observation_ms", MAX_DURATION_SECONDS * 1000
    )
    sample_count = bounded_uint(data, "sample_count", MAX_SAMPLE_INTERVALS)
    duration = bounded_uint(data, "duration_seconds", MAX_DURATION_SECONDS)
    tabs = bounded_uint(data, "tab_count", MAX_TABS)
    tab_source_count = bounded_uint(data, "tab_source_count", MAX_TABS)
    tab_source_width = bounded_uint(data, "tab_source_width", MAX_DIMENSION)
    tab_source_height = bounded_uint(data, "tab_source_height", MAX_DIMENSION)
    width = bounded_uint(data, "viewport_width", MAX_DIMENSION)
    height = bounded_uint(data, "viewport_height", MAX_DIMENSION)
    min_fps = bounded_uint(data, "min_fps", MAX_FPS)
    source_fps = bounded_uint(data, "source_fps", MAX_FPS)
    cadence_ratio = bounded_uint(data, "cadence_ratio_permille", 1_000)
    supported_target_fps = bounded_uint(data, "supported_target_fps", MAX_FPS)
    max_stall = bounded_uint(data, "max_stall_ms", MAX_STALL_MS)
    pointers = bounded_uint(data, "pointer_updates", MAX_COUNTER)
    stationary_pointer_ms = bounded_uint(
        data, "stationary_pointer_continuous_ms", MAX_DURATION_SECONDS * 1000
    )
    nav_samples = bounded_uint(
        data, "navigation_latency_sample_count", MAX_LATENCY_SAMPLES
    )
    nav_p95 = bounded_uint(data, "navigation_p95_ms", MAX_LATENCY_MS)
    nav_max = bounded_uint(data, "navigation_max_ms", MAX_LATENCY_MS)
    session_samples = bounded_uint(
        data, "session_latency_sample_count", MAX_LATENCY_SAMPLES
    )
    session_p95 = bounded_uint(data, "session_latency_p95_ms", MAX_LATENCY_MS)
    session_max = bounded_uint(data, "session_latency_max_ms", MAX_LATENCY_MS)
    full_uploads = bounded_uint(data, "full_uploads", MAX_COUNTER)
    partial_uploads = bounded_uint(data, "partial_uploads", MAX_COUNTER)
    partial_rects = bounded_uint(data, "partial_rects", MAX_COUNTER)
    host_cpu_samples = bounded_uint(
        data, "host_process_cpu_sample_count", MAX_SAMPLE_INTERVALS
    )
    host_cpu_p95 = bounded_uint(
        data, "host_process_cpu_p95_permille", MAX_HOST_LOAD_PERMILLE
    )
    host_cpu_max = bounded_uint(
        data, "host_process_cpu_max_permille", MAX_HOST_LOAD_PERMILLE
    )
    host_gpu_samples = bounded_uint(
        data, "host_gpu_busy_sample_count", MAX_SAMPLE_INTERVALS
    )
    host_gpu_p95 = bounded_uint(
        data, "host_gpu_busy_p95_permille", MAX_HOST_LOAD_PERMILLE
    )
    host_gpu_max = bounded_uint(
        data, "host_gpu_busy_max_permille", MAX_HOST_LOAD_PERMILLE
    )
    hidden = bounded_uint(data, "hidden_repaints", MAX_COUNTER)
    hidden_active = bounded_uint(
        data, "hidden_repaint_active_intervals", MAX_SAMPLE_INTERVALS
    )
    hidden_intervals = bounded_uint(
        data, "hidden_repaint_interval_count", MAX_SAMPLE_INTERVALS
    )
    hidden_observation_ms = bounded_uint(
        data, "hidden_observation_ms", MAX_DURATION_SECONDS * 1000
    )
    hidden_burst = bounded_uint(
        data,
        "hidden_repaint_longest_burst_ms",
        MAX_DURATION_SECONDS * 1000,
    )
    hidden_quiescent = bounded_uint(
        data,
        "hidden_repaint_quiescent_ms",
        MAX_DURATION_SECONDS * 1000,
    )
    reconnects = bounded_uint(data, "reconnects", MAX_COUNTER)
    recovery = data["recovery_observed"]
    if not isinstance(recovery, bool):
        fail("recovery_observed must be boolean")

    if collection_mode == FIXTURE_COLLECTION_MODE:
        if status == "passed":
            fail("fixture-dry-run evidence can never carry passed status")
        if any(
            value is not None
            for value in (
                collection_nonce,
                shell_metrics_source,
                guest_metrics_source,
                host_boot_id,
                guest_boot_id,
                domain_uuid,
                browser_instance_id,
                workload_instance_id,
                source_instance_id,
            )
        ):
            fail("fixture-dry-run evidence cannot carry live provenance")
        if any(
            value != 0
            for value in (
                host_cpu_samples,
                host_cpu_p95,
                host_cpu_max,
                host_gpu_samples,
                host_gpu_p95,
                host_gpu_max,
            )
        ):
            fail("fixture-dry-run evidence cannot carry host load measurements")
    if collection_mode == LIVE_COLLECTION_MODE and collection_nonce is None:
        fail("live-endpoint-v4 evidence requires collection_nonce")
    identity_values = (
        host_boot_id,
        guest_boot_id,
        domain_uuid,
        browser_instance_id,
        workload_instance_id,
        source_instance_id,
    )
    if any(value is not None for value in identity_values) and not all(
        value is not None for value in identity_values
    ):
        fail("all runtime identities must be present together")
    if all(value is not None for value in identity_values) and len(
        set(identity_values)
    ) != len(identity_values):
        fail("runtime identities must be distinct")
    if tab_source_count != tabs:
        fail("tab_source_count must match tab_count")
    if duration * 1000 > wall_observation_ms + 999:
        fail("duration_seconds exceeds the wall observation")
    if stationary_pointer_ms > wall_observation_ms:
        fail("stationary pointer window exceeds the wall observation")
    if hidden_observation_ms > wall_observation_ms:
        fail("hidden observation exceeds the wall observation")

    if nav_samples == 0:
        if nav_p95 != 0 or nav_max != 0:
            fail("navigation latency metrics require samples")
    elif nav_p95 == 0 or nav_max == 0:
        fail("navigation latency samples require nonzero metrics")
    if nav_p95 > nav_max:
        fail("navigation_p95_ms cannot exceed navigation_max_ms")
    if session_samples == 0:
        if session_p95 != 0 or session_max != 0:
            fail("session latency metrics require samples")
    elif session_p95 == 0 or session_max == 0:
        fail("session latency samples require nonzero metrics")
    if session_p95 > session_max:
        fail("session_latency_p95_ms cannot exceed session_latency_max_ms")
    if hidden_active > hidden_intervals:
        fail("hidden repaint active intervals cannot exceed all intervals")
    if hidden_active > hidden:
        fail("hidden repaint active intervals cannot exceed repaint events")
    if (hidden == 0) != (hidden_active == 0):
        fail("hidden repaint events and active intervals are inconsistent")
    if hidden_active == 0 and hidden_burst != 0:
        fail("hidden repaint burst requires an active interval")
    if hidden_active > 0 and hidden_burst == 0:
        fail("hidden repaint active intervals require a nonzero burst")
    if hidden_intervals == 0 and hidden_quiescent != 0:
        fail("hidden repaint quiescence requires observed intervals")
    if hidden_quiescent > hidden_observation_ms:
        fail("hidden_repaint_quiescent_ms exceeds hidden_observation_ms")
    if partial_rects < partial_uploads:
        fail("partial_rects cannot be less than partial_uploads")
    if host_cpu_samples > sample_count:
        fail("host_process_cpu_sample_count cannot exceed sample_count")
    if host_gpu_samples > sample_count:
        fail("host_gpu_busy_sample_count cannot exceed sample_count")
    if host_cpu_p95 > host_cpu_max:
        fail("host_process_cpu_p95_permille cannot exceed its maximum")
    if host_gpu_p95 > host_gpu_max:
        fail("host_gpu_busy_p95_permille cannot exceed its maximum")
    if host_cpu_samples == 0 and (host_cpu_p95 != 0 or host_cpu_max != 0):
        fail("host process CPU summaries require samples")
    if host_gpu_samples == 0 and (host_gpu_p95 != 0 or host_gpu_max != 0):
        fail("host GPU summaries require samples")

    if status == "passed":
        age_seconds = (datetime.now(timezone.utc) - recorded_at).total_seconds()
        if age_seconds > MAX_EVIDENCE_AGE_SECONDS:
            fail(
                "recorded_at is stale for performance acceptance "
                f"(age_seconds={age_seconds:.0f})"
            )
        timestamp_span_ms = int(
            (recorded_at - observation_started).total_seconds() * 1000
        )
        requirements = {
            "collection_mode": collection_mode == LIVE_COLLECTION_MODE,
            "collection_nonce": collection_nonce is not None,
            "stream_sha256": stream_sha256 is not None,
            "stream_session_id": stream_session_id is not None
            and SYNTHETIC_SESSION_RE.search(stream_session_id) is None,
            "shell_metrics_source": shell_metrics_source == SHELL_METRICS_SOURCE,
            "guest_metrics_source": guest_metrics_source == GUEST_METRICS_SOURCE,
            "runtime_identities": all(value is not None for value in identity_values),
            "immutable_tab_set": tab_set_sha256 is not None
            and tab_source_count >= MIN_MEDIA_TAB_COUNT,
            "wall_observation_ms": wall_observation_ms >= 900_000,
            "wall_timestamp_consistency": timestamp_span_ms >= 900_000
            and abs(timestamp_span_ms - wall_observation_ms) <= 5_000,
            "sample_count": sample_count >= MIN_LIVE_SAMPLE_COUNT,
            "duration_seconds": duration >= 900,
            "tab_count": tabs >= MIN_MEDIA_TAB_COUNT,
            "viewport": width >= 1920 and height >= 1080,
            "five_tab_source_resolution": tab_source_width >= 1920
            and tab_source_height >= 1080,
            "visible_delivery_fps": min_fps >= MIN_VISIBLE_DELIVERY_FPS,
            "five_tab_source_progress": source_fps > 0,
            "supported_target_fps": supported_target_fps
            >= MIN_SUPPORTED_TARGET_FPS,
            "cadence_ratio_permille": cadence_ratio
            >= MIN_CADENCE_RATIO_PERMILLE,
            "max_stall_ms": max_stall <= 500,
            "pointer_updates": pointers > 0,
            "stationary_pointer_continuous_ms": stationary_pointer_ms
            >= STATIONARY_POINTER_WINDOW_MS,
            "navigation_latency_sample_count": nav_samples
            >= MIN_NAVIGATION_LATENCY_SAMPLES,
            "navigation_p95_ms": 0 < nav_p95 <= LATENCY_P95_LIMIT_MS,
            "navigation_max_ms": 0 < nav_max <= LATENCY_MAX_LIMIT_MS,
            "session_latency_sample_count": session_samples
            >= MIN_SESSION_LATENCY_SAMPLES,
            "session_latency_p95_ms": 0
            < session_p95
            <= LATENCY_P95_LIMIT_MS,
            "session_latency_max_ms": 0 < session_max <= LATENCY_MAX_LIMIT_MS,
            "partial_uploads": partial_uploads > 0,
            "partial_upload_rects": partial_rects >= partial_uploads,
            "host_process_cpu_coverage": host_cpu_samples * 1_000
            >= sample_count * MIN_HOST_LOAD_COVERAGE_PERMILLE,
            "host_gpu_busy_coverage": host_gpu_samples * 1_000
            >= sample_count * MIN_HOST_LOAD_COVERAGE_PERMILLE,
            "hidden_observation_ms": hidden_observation_ms
            >= HIDDEN_QUIESCENT_WINDOW_MS,
            "hidden_repaint_longest_burst_ms": hidden_burst
            <= MAX_HIDDEN_REPAINT_BURST_MS,
            "hidden_repaint_frequency": hidden_intervals > 0
            and hidden_active / hidden_intervals
            <= MAX_HIDDEN_REPAINT_ACTIVE_FRACTION,
            "hidden_repaint_quiescent_ms": hidden_quiescent
            >= HIDDEN_QUIESCENT_WINDOW_MS,
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
        "collection_mode": collection_mode,
        "source_commit": source_commit,
        "image_digest": image_digest,
        "stream_sha256": stream_sha256,
        "stream_session_id": stream_session_id,
        "domain_uuid": domain_uuid,
        "browser_instance_id": browser_instance_id,
        "workload_instance_id": workload_instance_id,
        "source_instance_id": source_instance_id,
        "wall_observation_ms": wall_observation_ms,
        "sample_count": sample_count,
        "duration_seconds": duration,
        "tab_count": tabs,
        "tab_source_count": tab_source_count,
        "tab_set_sha256": tab_set_sha256,
        "tab_source_width": tab_source_width,
        "tab_source_height": tab_source_height,
        "min_fps": min_fps,
        "source_fps": source_fps,
        "cadence_ratio_permille": cadence_ratio,
        "supported_target_fps": supported_target_fps,
        "max_stall_ms": max_stall,
        "stationary_pointer_continuous_ms": stationary_pointer_ms,
        "navigation_latency_sample_count": nav_samples,
        "navigation_p95_ms": nav_p95,
        "navigation_max_ms": nav_max,
        "session_latency_sample_count": session_samples,
        "session_latency_p95_ms": session_p95,
        "session_latency_max_ms": session_max,
        "full_uploads": full_uploads,
        "partial_uploads": partial_uploads,
        "partial_rects": partial_rects,
        "host_process_cpu_sample_count": host_cpu_samples,
        "host_process_cpu_p95_permille": host_cpu_p95,
        "host_process_cpu_max_permille": host_cpu_max,
        "host_gpu_busy_sample_count": host_gpu_samples,
        "host_gpu_busy_p95_permille": host_gpu_p95,
        "host_gpu_busy_max_permille": host_gpu_max,
        "hidden_repaint_active_intervals": hidden_active,
        "hidden_repaint_interval_count": hidden_intervals,
        "hidden_observation_ms": hidden_observation_ms,
        "hidden_repaint_longest_burst_ms": hidden_burst,
        "hidden_repaint_quiescent_ms": hidden_quiescent,
        "reconnects": reconnects,
        "reason": (
            "all Browser VM performance acceptance criteria were observed"
            if status == "passed"
            else "live performance acceptance is not available"
        ),
    }


def valid_record(transport: str = DEFAULT_TRANSPORT) -> dict[str, Any]:
    recorded = datetime.now(timezone.utc).replace(microsecond=0)
    started = recorded - timedelta(seconds=900)
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": "browser_vm_performance",
        "profile": "browser-vm-chromium",
        "image": "browser-vm-chromium",
        "workload": "browser-vm",
        "status": "passed",
        "source": "live-browser-vm-acceptance",
        "source_commit": "0123456789abcdef0123456789abcdef01234567",
        "image_digest": "sha256:" + "a" * 64,
        "transport": transport,
        "collection_mode": LIVE_COLLECTION_MODE,
        "collection_nonce": "b" * 64,
        "stream_sha256": "sha256:" + "c" * 64,
        "stream_session_id": "browser-vm-a1100",
        "shell_metrics_source": SHELL_METRICS_SOURCE,
        "guest_metrics_source": GUEST_METRICS_SOURCE,
        "host_boot_id": "11111111-1111-4111-8111-111111111111",
        "guest_boot_id": "22222222-2222-4222-8222-222222222222",
        "domain_uuid": "33333333-3333-4333-8333-333333333333",
        "browser_instance_id": "44444444-4444-4444-8444-444444444444",
        "workload_instance_id": "55555555-5555-4555-8555-555555555555",
        "source_instance_id": "66666666-6666-4666-8666-666666666666",
        "observation_started_at": started.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "wall_observation_ms": 900_000,
        "sample_count": 181,
        "duration_seconds": 900,
        "tab_count": 5,
        "tab_source_count": 5,
        "tab_set_sha256": "sha256:" + "d" * 64,
        "tab_source_width": 1920,
        "tab_source_height": 1080,
        "viewport_width": 1920,
        "viewport_height": 1080,
        "min_fps": MIN_VISIBLE_DELIVERY_FPS,
        "source_fps": 1,
        "cadence_ratio_permille": MIN_CADENCE_RATIO_PERMILLE,
        "supported_target_fps": MIN_SUPPORTED_TARGET_FPS,
        "max_stall_ms": 500,
        "pointer_updates": 1,
        "stationary_pointer_continuous_ms": 300_000,
        "navigation_latency_sample_count": 15,
        "navigation_p95_ms": 80,
        "navigation_max_ms": 200,
        "session_latency_sample_count": 15,
        "session_latency_p95_ms": 90,
        "session_latency_max_ms": 225,
        "full_uploads": 1,
        "partial_uploads": 120,
        "partial_rects": 240,
        "host_process_cpu_sample_count": 180,
        "host_process_cpu_p95_permille": 18_000,
        "host_process_cpu_max_permille": 24_000,
        "host_gpu_busy_sample_count": 180,
        "host_gpu_busy_p95_permille": 42_000,
        "host_gpu_busy_max_permille": 50_000,
        "hidden_repaints": 1,
        "hidden_repaint_active_intervals": 1,
        "hidden_repaint_interval_count": 180,
        "hidden_observation_ms": 900_000,
        "hidden_repaint_longest_burst_ms": 5_000,
        "hidden_repaint_quiescent_ms": 895_000,
        "reconnects": 1,
        "recovery_observed": True,
        "recorded_at": recorded.strftime("%Y-%m-%dT%H:%M:%SZ"),
    }


def assert_rejected(data: Any, needle: str) -> None:
    try:
        validate_document(data)
    except EvidenceError as exc:
        assert needle in str(exc), (needle, str(exc))
    else:
        raise AssertionError(f"accepted invalid evidence containing {needle!r}")


def self_test() -> None:
    rdp_result = validate_document(valid_record())
    assert rdp_result["status"] == "validated"
    assert rdp_result["live_proof"] == "observed"
    assert rdp_result["transport"] == DEFAULT_TRANSPORT
    assert rdp_result["min_fps"] == MIN_VISIBLE_DELIVERY_FPS
    assert rdp_result["source_fps"] == 1
    assert rdp_result["cadence_ratio_permille"] == MIN_CADENCE_RATIO_PERMILLE
    sunshine_result = validate_document(valid_record("sunshine"))
    assert sunshine_result["status"] == "validated"
    assert sunshine_result["transport"] == "sunshine"
    assert_rejected(valid_record("spice"), "transport")
    assert_rejected(
        dict(valid_record(), schema_version=SCHEMA_VERSION - 1), "schema_version"
    )
    assert_rejected(dict(valid_record(), extra="no"), "unexpected evidence fields")
    assert_rejected(dict(valid_record(), password="no"), "credential-shaped field")
    assert_rejected(
        dict(valid_record(), collection_mode=FIXTURE_COLLECTION_MODE),
        "fixture-dry-run",
    )
    assert_rejected(dict(valid_record(), collection_nonce=None), "collection_nonce")
    assert_rejected(dict(valid_record(), stream_sha256=None), "stream_sha256")
    assert_rejected(
        dict(valid_record(), stream_session_id="fixture-browser-vm"),
        "stream_session_id",
    )
    assert_rejected(dict(valid_record(), sample_count=90), "sample_count")
    assert_rejected(dict(valid_record(), workload="other-vm"), "workload")
    assert_rejected(
        dict(valid_record(), browser_instance_id=None), "runtime identities"
    )
    assert_rejected(
        dict(
            valid_record(),
            browser_instance_id="33333333-3333-4333-8333-333333333333",
        ),
        "runtime identities must be distinct",
    )
    assert_rejected(dict(valid_record(), tab_source_count=4), "tab_source_count")
    assert_rejected(dict(valid_record(), tab_set_sha256=None), "immutable_tab_set")
    assert_rejected(
        dict(valid_record(), tab_source_height=1_079),
        "five_tab_source_resolution",
    )
    assert_rejected(dict(valid_record(), duration_seconds=899), "duration_seconds")
    assert_rejected(
        dict(valid_record(), min_fps=MIN_VISIBLE_DELIVERY_FPS - 1),
        "visible_delivery_fps",
    )
    assert_rejected(
        dict(valid_record(), source_fps=0),
        "five_tab_source_progress",
    )
    assert_rejected(
        dict(valid_record(), supported_target_fps=MIN_SUPPORTED_TARGET_FPS - 1),
        "supported_target_fps",
    )
    assert_rejected(
        dict(valid_record(), cadence_ratio_permille=899),
        "cadence_ratio_permille",
    )
    assert_rejected(
        dict(valid_record(), stationary_pointer_continuous_ms=299_999),
        "stationary_pointer_continuous_ms",
    )
    assert_rejected(dict(valid_record(), max_stall_ms=501), "max_stall_ms")
    assert_rejected(dict(valid_record(), partial_uploads=0), "partial_uploads")
    assert_rejected(dict(valid_record(), partial_rects=119), "partial_rects")
    assert_rejected(
        dict(valid_record(), host_process_cpu_sample_count=162),
        "host_process_cpu_coverage",
    )
    assert_rejected(
        dict(valid_record(), host_gpu_busy_sample_count=162),
        "host_gpu_busy_coverage",
    )
    assert_rejected(
        dict(valid_record(), host_process_cpu_p95_permille=24_001),
        "host_process_cpu_p95_permille",
    )
    assert_rejected(
        dict(valid_record(), host_gpu_busy_p95_permille=50_001),
        "host_gpu_busy_p95_permille",
    )
    assert_rejected(dict(valid_record(), navigation_p95_ms=101), "navigation_p95_ms")
    assert_rejected(dict(valid_record(), navigation_max_ms=251), "navigation_max_ms")
    assert_rejected(
        dict(valid_record(), navigation_latency_sample_count=14),
        "navigation_latency_sample_count",
    )
    assert_rejected(
        dict(valid_record(), session_latency_p95_ms=101),
        "session_latency_p95_ms",
    )
    assert_rejected(
        dict(valid_record(), session_latency_max_ms=251),
        "session_latency_max_ms",
    )
    assert_rejected(
        dict(valid_record(), session_latency_sample_count=14),
        "session_latency_sample_count",
    )
    assert_rejected(
        dict(
            valid_record(),
            hidden_repaints=180,
            hidden_repaint_active_intervals=180,
            hidden_repaint_interval_count=180,
            hidden_repaint_longest_burst_ms=900_000,
            hidden_repaint_quiescent_ms=0,
        ),
        "hidden_repaint",
    )
    assert_rejected(
        dict(valid_record(), hidden_observation_ms=299_999),
        "hidden_observation_ms",
    )
    no_hidden_repaints = dict(
        valid_record(),
        hidden_repaints=0,
        hidden_repaint_active_intervals=0,
        hidden_repaint_longest_burst_ms=0,
        hidden_repaint_quiescent_ms=900_000,
    )
    assert validate_document(no_hidden_repaints)["status"] == "validated"
    assert_rejected(dict(valid_record(), recovery_observed=False), "recovery_observed")
    assert_rejected(dict(valid_record(), source_commit="short"), "source_commit")
    assert_rejected(dict(valid_record(), source_commit="0" * 40), "non-null")
    assert_rejected(
        dict(valid_record(), image_digest="sha256:" + "0" * 64), "non-null"
    )
    assert_rejected(dict(valid_record(), recorded_at="2026-99-99T00:00:00Z"), "real UTC")
    stale_record = valid_record()
    stale_recorded = datetime.now(timezone.utc).replace(microsecond=0) - timedelta(
        seconds=MAX_EVIDENCE_AGE_SECONDS + 1
    )
    stale_record["recorded_at"] = stale_recorded.strftime("%Y-%m-%dT%H:%M:%SZ")
    stale_record["observation_started_at"] = (
        stale_recorded - timedelta(seconds=900)
    ).strftime("%Y-%m-%dT%H:%M:%SZ")
    assert_rejected(stale_record, "stale")
    unavailable = dict(
        valid_record(),
        status="unavailable",
        duration_seconds=0,
        tab_count=0,
        tab_source_count=0,
    )
    assert validate_document(unavailable)["status"] == "unavailable"
    fixture_unavailable = dict(
        unavailable,
        collection_mode=FIXTURE_COLLECTION_MODE,
        collection_nonce=None,
        shell_metrics_source=None,
        guest_metrics_source=None,
        host_boot_id=None,
        guest_boot_id=None,
        domain_uuid=None,
        browser_instance_id=None,
        workload_instance_id=None,
        source_instance_id=None,
        stream_session_id="fixture-browser-vm",
        host_process_cpu_sample_count=0,
        host_process_cpu_p95_permille=0,
        host_process_cpu_max_permille=0,
        host_gpu_busy_sample_count=0,
        host_gpu_busy_p95_permille=0,
        host_gpu_busy_max_permille=0,
    )
    assert validate_document(fixture_unavailable)["status"] == "unavailable"
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
