#!/usr/bin/env python3
"""Collect bounded Browser VM performance evidence from one live session.

The collector consumes a versioned NDJSON telemetry stream from an explicitly
supplied, credential-free HTTP(S) endpoint.  The endpoint must describe the
running ``browser-vm`` session and stream cumulative VDI counters plus latency
samples.  A real pass requires the collector to remain attached to that live
stream for at least 900 seconds; endpoint timestamps alone cannot shorten the
observation.

Fixtures are admitted only by ``dry-run`` and ``--self-test``.  They exercise
the same parser and thresholds without sleeping, but their evidence status is
always ``unavailable``.  No command-line option can supply a performance
counter or turn fixture data, reachability, or a partial stream into a pass.

Live endpoint protocol (one compact JSON object per line):

* one ``browser_vm_performance_stream`` header bound to the source/image;
* ordered ``sample`` records, normally at five-second cadence; and
* one terminal ``complete`` record with ``completed``, ``failed``, or
  ``unavailable`` status.

Usage:
  collect-browser-vm-performance.py collect \
    --endpoint http://127.0.0.1:9080/v1/browser-vm/performance \
    --source-commit <40-hex> --image-digest sha256:<64-hex> \
    --out performance-evidence.json
  collect-browser-vm-performance.py dry-run \
    --fixture install-helpers/fixtures/browser-vm-performance/passing.ndjson \
    --source-commit <40-hex> --image-digest sha256:<64-hex>
  collect-browser-vm-performance.py --self-test
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import importlib.util
import json
import math
import os
from pathlib import Path
import re
import socket
import stat
import sys
import tempfile
import time
from typing import Any, Iterable, NoReturn
import urllib.error
import urllib.parse
import urllib.request


STREAM_SCHEMA_VERSION = 1
EVIDENCE_SCHEMA_VERSION = 2
PASS_DURATION_SECONDS = 15 * 60
MAX_COLLECTION_SECONDS = 20 * 60
MAX_STREAM_BYTES = 2 * 1024 * 1024
MAX_LINE_BYTES = 64 * 1024
MAX_STREAM_RECORDS = 4096
MAX_LATENCIES_PER_SAMPLE = 128
MAX_LATENCY_SAMPLES = 10_000
MAX_SAMPLE_GAP_MS = 10_000
MAX_RECEIPT_GAP_SECONDS = 15.0
MAX_DURATION_MS = 24 * 60 * 60 * 1000
MAX_TABS = 16
MAX_DIMENSION = 16_384
MAX_FRAME_COUNTER = 100_000_000
MAX_COUNTER = 10_000_000
MAX_STALL_MS = 60_000
MAX_LATENCY_MS = 600_000
MAX_FPS = 240
MAX_EVIDENCE_BYTES = 64 * 1024
DRY_RUN_RECORDED_AT = "2020-01-02T03:04:05Z"
LATENCY_P95_LIMIT_MS = 100
LATENCY_MAX_LIMIT_MS = 250
MIN_NAVIGATION_LATENCY_SAMPLES = 15
MIN_SESSION_LATENCY_SAMPLES = 15
HIDDEN_QUIESCENT_WINDOW_MS = 5 * 60 * 1000
MAX_HIDDEN_REPAINT_BURST_MS = 2 * MAX_SAMPLE_GAP_MS
MAX_HIDDEN_REPAINT_ACTIVE_FRACTION = 0.05
DEFAULT_TRANSPORT = "sunshine"
ADMITTED_TRANSPORTS = (DEFAULT_TRANSPORT, "rdp")

HEADER_FIELDS = frozenset(
    {
        "schema_version",
        "kind",
        "source",
        "profile",
        "image",
        "workload",
        "session_id",
        "source_commit",
        "image_digest",
        "transport",
    }
)
SAMPLE_FIELDS = frozenset(
    {
        "type",
        "elapsed_ms",
        "tab_count",
        "viewport_width",
        "viewport_height",
        "frames_received",
        "max_frame_gap_ms",
        "pointer_updates",
        "navigation_latencies_ms",
        "session_latencies_ms",
        "partial_uploads",
        "hidden_repaints",
        "reconnects",
        "connection_state",
    }
)
COMPLETE_FIELDS = frozenset({"type", "status"})
SOURCE_COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
IMAGE_DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
SESSION_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$")


class CollectorError(Exception):
    """Collector input, protocol, or output failed closed."""


class EndpointUnavailable(CollectorError):
    """The explicit live endpoint could not provide an observation."""


class EndpointFailed(CollectorError):
    """The endpoint responded, but its stream was not admissible evidence."""


def fail(message: str) -> NoReturn:
    raise CollectorError(message)


def reject_json_constant(value: str) -> NoReturn:
    fail(f"non-finite JSON number is not allowed: {value}")


def reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            fail(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def decode_record(raw: bytes, record_number: int) -> dict[str, Any]:
    try:
        value = json.loads(
            raw.decode("utf-8"),
            object_pairs_hook=reject_duplicate_keys,
            parse_constant=reject_json_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail(f"stream record {record_number} is malformed JSON: {exc}")
    if not isinstance(value, dict):
        fail(f"stream record {record_number} must be one JSON object")
    return value


def exact_fields(data: dict[str, Any], expected: frozenset[str], label: str) -> None:
    fields = frozenset(data)
    missing = expected - fields
    extra = fields - expected
    if missing:
        fail(f"{label} misses fields: {', '.join(sorted(missing))}")
    if extra:
        fail(f"{label} has unexpected fields: {', '.join(sorted(extra))}")


def bounded_uint(value: Any, maximum: int, label: str, minimum: int = 0) -> int:
    if (
        isinstance(value, bool)
        or not isinstance(value, int)
        or not minimum <= value <= maximum
    ):
        fail(f"{label} must be an integer between {minimum} and {maximum}")
    return value


def validate_source_binding(source_commit: str, image_digest: str) -> None:
    if SOURCE_COMMIT_RE.fullmatch(source_commit) is None or source_commit == "0" * 40:
        fail("source_commit must be a full, non-null lowercase Git revision")
    if (
        IMAGE_DIGEST_RE.fullmatch(image_digest) is None
        or image_digest == "sha256:" + "0" * 64
    ):
        fail("image_digest must be a full, non-null lowercase SHA-256 digest")


def validate_endpoint(raw: str) -> str:
    if not raw or len(raw) > 2048 or any(ch.isspace() or ord(ch) < 32 for ch in raw):
        fail("endpoint must be a bounded URL without whitespace or controls")
    parsed = urllib.parse.urlsplit(raw)
    if parsed.scheme not in {"http", "https"}:
        fail("endpoint must use http or https")
    if not parsed.hostname:
        fail("endpoint must include an explicit host")
    if parsed.username is not None or parsed.password is not None:
        fail("endpoint credentials are not allowed in the URL")
    if parsed.query or parsed.fragment:
        fail("endpoint query strings and fragments are not allowed")
    try:
        port = parsed.port
    except ValueError as exc:
        fail(f"endpoint port is malformed: {exc}")
    if port is not None and not 1 <= port <= 65535:
        fail("endpoint port must be between 1 and 65535")
    return raw


def validate_header(
    data: dict[str, Any], source_commit: str, image_digest: str, transport: str
) -> None:
    exact_fields(data, HEADER_FIELDS, "stream header")
    if data["schema_version"] != STREAM_SCHEMA_VERSION or isinstance(
        data["schema_version"], bool
    ):
        fail(f"stream schema_version must be integer {STREAM_SCHEMA_VERSION}")
    if data["kind"] != "browser_vm_performance_stream":
        fail("stream kind is not browser_vm_performance_stream")
    if data["source"] != "live-browser-vm-session-endpoint":
        fail("stream source is not the admitted live session endpoint")
    if (
        data["profile"] != "browser-vm-chromium"
        or data["image"] != "browser-vm-chromium"
    ):
        fail("stream profile/image does not identify browser-vm-chromium")
    if data["workload"] != "browser-vm":
        fail("stream workload is not browser-vm")
    if (
        not isinstance(data["session_id"], str)
        or SESSION_ID_RE.fullmatch(data["session_id"]) is None
    ):
        fail("stream session_id is malformed")
    if data["source_commit"] != source_commit:
        fail("stream source_commit does not match the requested source")
    if data["image_digest"] != image_digest:
        fail("stream image_digest does not match the requested image")
    if data["transport"] != transport:
        fail("stream transport does not match the requested transport")


def validate_latency_list(value: Any, label: str) -> list[int]:
    if not isinstance(value, list) or len(value) > MAX_LATENCIES_PER_SAMPLE:
        fail(f"{label} must be a bounded list")
    return [
        bounded_uint(item, MAX_LATENCY_MS, f"{label}[{index}]", minimum=1)
        for index, item in enumerate(value)
    ]


def validate_sample(data: dict[str, Any], index: int) -> dict[str, Any]:
    label = f"sample {index}"
    exact_fields(data, SAMPLE_FIELDS, label)
    if data["type"] != "sample":
        fail(f"{label} type is not sample")
    sample = {
        "type": "sample",
        "elapsed_ms": bounded_uint(data["elapsed_ms"], MAX_DURATION_MS, f"{label}.elapsed_ms"),
        "tab_count": bounded_uint(data["tab_count"], MAX_TABS, f"{label}.tab_count"),
        "viewport_width": bounded_uint(
            data["viewport_width"], MAX_DIMENSION, f"{label}.viewport_width"
        ),
        "viewport_height": bounded_uint(
            data["viewport_height"], MAX_DIMENSION, f"{label}.viewport_height"
        ),
        "frames_received": bounded_uint(
            data["frames_received"], MAX_FRAME_COUNTER, f"{label}.frames_received"
        ),
        "max_frame_gap_ms": bounded_uint(
            data["max_frame_gap_ms"], MAX_STALL_MS, f"{label}.max_frame_gap_ms"
        ),
        "pointer_updates": bounded_uint(
            data["pointer_updates"], MAX_COUNTER, f"{label}.pointer_updates"
        ),
        "navigation_latencies_ms": validate_latency_list(
            data["navigation_latencies_ms"], f"{label}.navigation_latencies_ms"
        ),
        "session_latencies_ms": validate_latency_list(
            data["session_latencies_ms"], f"{label}.session_latencies_ms"
        ),
        "partial_uploads": bounded_uint(
            data["partial_uploads"], MAX_COUNTER, f"{label}.partial_uploads"
        ),
        "hidden_repaints": bounded_uint(
            data["hidden_repaints"], MAX_COUNTER, f"{label}.hidden_repaints"
        ),
        "reconnects": bounded_uint(data["reconnects"], MAX_COUNTER, f"{label}.reconnects"),
        "connection_state": data["connection_state"],
    }
    if sample["connection_state"] not in {
        "connected",
        "reconnecting",
        "failed",
        "unavailable",
    }:
        fail(f"{label}.connection_state is not admitted")
    return sample


def validate_complete(data: dict[str, Any]) -> str:
    exact_fields(data, COMPLETE_FIELDS, "complete record")
    if data["type"] != "complete":
        fail("terminal record type is not complete")
    if data["status"] not in {"completed", "failed", "unavailable"}:
        fail("complete status must be completed, failed, or unavailable")
    return data["status"]


@dataclass(frozen=True)
class Observation:
    samples: tuple[dict[str, Any], ...]
    receipt_seconds: tuple[float, ...]
    completion_status: str


def parse_records(
    records: Iterable[tuple[dict[str, Any], float]],
    source_commit: str,
    image_digest: str,
    transport: str,
) -> Observation:
    materialized = list(records)
    if not materialized:
        fail("live endpoint returned no stream records")
    if len(materialized) > MAX_STREAM_RECORDS:
        fail(f"live endpoint exceeds {MAX_STREAM_RECORDS} stream records")
    header, _ = materialized[0]
    validate_header(header, source_commit, image_digest, transport)

    samples: list[dict[str, Any]] = []
    receipts: list[float] = []
    completion_status: str | None = None
    latency_count = 0
    previous: dict[str, Any] | None = None
    for record_number, (record, receipt) in enumerate(materialized[1:], start=2):
        if completion_status is not None:
            fail("stream contains records after its terminal complete marker")
        if record.get("type") == "complete":
            completion_status = validate_complete(record)
            continue
        sample = validate_sample(record, len(samples) + 1)
        if previous is not None:
            if sample["elapsed_ms"] <= previous["elapsed_ms"]:
                fail("sample elapsed_ms values must increase strictly")
            for field in (
                "frames_received",
                "pointer_updates",
                "partial_uploads",
                "hidden_repaints",
                "reconnects",
            ):
                if sample[field] < previous[field]:
                    fail(f"sample cumulative counter decreased: {field}")
        if receipts and receipt < receipts[-1]:
            fail("sample receipt clock moved backwards")
        latency_count += len(sample["navigation_latencies_ms"])
        latency_count += len(sample["session_latencies_ms"])
        if latency_count > MAX_LATENCY_SAMPLES:
            fail(f"stream exceeds {MAX_LATENCY_SAMPLES} latency observations")
        samples.append(sample)
        receipts.append(receipt)
        previous = sample
    if completion_status is None:
        fail("stream ended without a terminal complete marker")
    return Observation(tuple(samples), tuple(receipts), completion_status)


def read_stream_lines(response: Any, opened_at: float) -> list[tuple[dict[str, Any], float]]:
    content_length = response.headers.get("Content-Length")
    if content_length is not None:
        try:
            declared = int(content_length)
        except ValueError:
            fail("live endpoint Content-Length is malformed")
        if declared < 0 or declared > MAX_STREAM_BYTES:
            fail(f"live endpoint exceeds {MAX_STREAM_BYTES} bytes")

    records: list[tuple[dict[str, Any], float]] = []
    total = 0
    while True:
        if time.monotonic() - opened_at > MAX_COLLECTION_SECONDS:
            raise EndpointUnavailable(
                f"live endpoint exceeded the bounded {MAX_COLLECTION_SECONDS}-second window"
            )
        raw = response.readline(MAX_LINE_BYTES + 1)
        if not raw:
            break
        if len(raw) > MAX_LINE_BYTES:
            fail(f"stream record exceeds {MAX_LINE_BYTES} bytes")
        total += len(raw)
        if total > MAX_STREAM_BYTES:
            fail(f"live endpoint exceeds {MAX_STREAM_BYTES} bytes")
        if not raw.strip():
            fail("blank NDJSON records are not allowed")
        records.append((decode_record(raw, len(records) + 1), time.monotonic()))
        if len(records) > MAX_STREAM_RECORDS:
            fail(f"live endpoint exceeds {MAX_STREAM_RECORDS} stream records")
    return records


def consume_endpoint(
    endpoint: str,
    source_commit: str,
    image_digest: str,
    transport: str,
    read_timeout_seconds: int,
) -> Observation:
    validate_endpoint(endpoint)
    request = urllib.request.Request(
        endpoint,
        headers={
            "Accept": "application/x-ndjson, application/json",
            "User-Agent": "magic-mesh-browser-vm-performance-collector/1",
        },
        method="GET",
    )
    opened_at = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=read_timeout_seconds) as response:
            validate_endpoint(response.geturl())
            content_type = response.headers.get_content_type()
            if content_type not in {"application/x-ndjson", "application/json"}:
                raise EndpointFailed("live endpoint did not return JSON/NDJSON telemetry")
            records = read_stream_lines(response, opened_at)
    except urllib.error.HTTPError as exc:
        if 500 <= exc.code <= 599:
            raise EndpointUnavailable(
                f"live endpoint returned temporary HTTP status {exc.code}"
            ) from None
        raise EndpointFailed(f"live endpoint returned HTTP status {exc.code}") from None
    except (urllib.error.URLError, socket.timeout, TimeoutError, ConnectionError, OSError):
        raise EndpointUnavailable("live endpoint was unreachable or timed out") from None
    try:
        return parse_records(records, source_commit, image_digest, transport)
    except CollectorError as exc:
        raise EndpointFailed(f"live endpoint stream is inadmissible: {exc}") from None


def read_fixture(
    path: Path, source_commit: str, image_digest: str, transport: str
) -> Observation:
    try:
        st = path.lstat()
    except OSError as exc:
        fail(f"fixture is not readable: {exc}")
    if stat.S_ISLNK(st.st_mode) or not stat.S_ISREG(st.st_mode):
        fail("fixture must be a regular non-symlink file")
    if st.st_size > MAX_STREAM_BYTES:
        fail(f"fixture exceeds {MAX_STREAM_BYTES} bytes")
    try:
        lines = path.read_bytes().splitlines()
    except OSError as exc:
        fail(f"fixture is not readable: {exc}")
    records: list[tuple[dict[str, Any], float]] = []
    last_receipt = 0.0
    for index, raw in enumerate(lines, start=1):
        if not raw.strip():
            fail("blank NDJSON records are not allowed")
        record = decode_record(raw, index)
        if record.get("type") == "sample":
            elapsed = record.get("elapsed_ms")
            if isinstance(elapsed, int) and not isinstance(elapsed, bool) and elapsed >= 0:
                last_receipt = elapsed / 1000.0
        records.append((record, last_receipt))
    return parse_records(records, source_commit, image_digest, transport)


def percentile_95(values: list[int]) -> int:
    if not values:
        return 0
    ordered = sorted(values)
    return ordered[math.ceil(len(ordered) * 0.95) - 1]


def counter_delta(samples: tuple[dict[str, Any], ...], field: str) -> int:
    return samples[-1][field] - samples[0][field] if samples else 0


@dataclass(frozen=True)
class HiddenRepaintBehavior:
    active_intervals: int
    interval_count: int
    longest_active_ms: int
    longest_quiescent_ms: int

    @property
    def active_fraction(self) -> float:
        if self.interval_count == 0:
            return 1.0
        return self.active_intervals / self.interval_count


def hidden_repaint_behavior(
    samples: tuple[dict[str, Any], ...],
) -> HiddenRepaintBehavior:
    """Measure repaint-counter activity over endpoint-time intervals.

    A single nonzero cumulative counter cannot establish that hidden content
    quiesces.  This derives bounded bursts and a contiguous idle window from
    counter deltas instead, so periodic or every-sample repainting fails.
    """

    active_intervals = 0
    longest_active_ms = 0
    longest_quiescent_ms = 0
    active_run_ms = 0
    quiescent_run_ms = 0
    intervals = tuple(zip(samples, samples[1:]))
    for previous, current in intervals:
        elapsed_ms = current["elapsed_ms"] - previous["elapsed_ms"]
        repaint_delta = current["hidden_repaints"] - previous["hidden_repaints"]
        if repaint_delta > 0:
            active_intervals += 1
            active_run_ms += elapsed_ms
            quiescent_run_ms = 0
            longest_active_ms = max(longest_active_ms, active_run_ms)
        else:
            quiescent_run_ms += elapsed_ms
            active_run_ms = 0
            longest_quiescent_ms = max(longest_quiescent_ms, quiescent_run_ms)
    return HiddenRepaintBehavior(
        active_intervals=active_intervals,
        interval_count=len(intervals),
        longest_active_ms=longest_active_ms,
        longest_quiescent_ms=longest_quiescent_ms,
    )


def reconnect_recovered(samples: tuple[dict[str, Any], ...]) -> bool:
    connected_before = False
    reconnecting = False
    for sample in samples:
        state = sample["connection_state"]
        if state == "connected" and reconnecting:
            return True
        if state == "connected":
            connected_before = True
        elif state == "reconnecting" and connected_before:
            reconnecting = True
    return False


@dataclass(frozen=True)
class Assessment:
    duration_seconds: int
    tab_count: int
    viewport_width: int
    viewport_height: int
    min_fps: int
    max_stall_ms: int
    pointer_updates: int
    navigation_latency_sample_count: int
    navigation_p95_ms: int
    navigation_max_ms: int
    session_latency_sample_count: int
    session_latency_p95_ms: int
    session_latency_max_ms: int
    partial_uploads: int
    hidden_repaints: int
    hidden_repaint_active_intervals: int
    hidden_repaint_interval_count: int
    hidden_repaint_longest_burst_ms: int
    hidden_repaint_quiescent_ms: int
    reconnects: int
    recovery_observed: bool
    criteria_met: bool
    failures: tuple[str, ...]


def assess(observation: Observation) -> Assessment:
    samples = observation.samples
    if not samples:
        return Assessment(
            duration_seconds=0,
            tab_count=0,
            viewport_width=0,
            viewport_height=0,
            min_fps=0,
            max_stall_ms=0,
            pointer_updates=0,
            navigation_latency_sample_count=0,
            navigation_p95_ms=0,
            navigation_max_ms=0,
            session_latency_sample_count=0,
            session_latency_p95_ms=0,
            session_latency_max_ms=0,
            partial_uploads=0,
            hidden_repaints=0,
            hidden_repaint_active_intervals=0,
            hidden_repaint_interval_count=0,
            hidden_repaint_longest_burst_ms=0,
            hidden_repaint_quiescent_ms=0,
            reconnects=0,
            recovery_observed=False,
            criteria_met=False,
            failures=("samples",),
        )

    endpoint_span_ms = samples[-1]["elapsed_ms"] - samples[0]["elapsed_ms"]
    receipt_span_seconds = observation.receipt_seconds[-1] - observation.receipt_seconds[0]
    duration_seconds = max(
        0, math.floor(min(endpoint_span_ms / 1000.0, receipt_span_seconds))
    )
    endpoint_gaps = [
        current["elapsed_ms"] - previous["elapsed_ms"]
        for previous, current in zip(samples, samples[1:])
    ]
    receipt_gaps = [
        current - previous
        for previous, current in zip(
            observation.receipt_seconds, observation.receipt_seconds[1:]
        )
    ]
    cadence_observed = (
        len(samples) >= 2
        and samples[0]["elapsed_ms"] <= MAX_SAMPLE_GAP_MS
        and all(gap <= MAX_SAMPLE_GAP_MS for gap in endpoint_gaps)
        and all(gap <= MAX_RECEIPT_GAP_SECONDS for gap in receipt_gaps)
    )
    frame_rates = []
    for previous, current in zip(samples, samples[1:]):
        elapsed = current["elapsed_ms"] - previous["elapsed_ms"]
        frame_delta = current["frames_received"] - previous["frames_received"]
        frame_rates.append(frame_delta * 1000.0 / elapsed)
    min_fps = min(MAX_FPS, math.floor(min(frame_rates))) if frame_rates else 0
    navigation_latencies = [
        latency for sample in samples for latency in sample["navigation_latencies_ms"]
    ]
    session_latencies = [
        latency for sample in samples for latency in sample["session_latencies_ms"]
    ]
    recovery = reconnect_recovered(samples)
    repaint_behavior = hidden_repaint_behavior(samples)
    metrics = {
        "duration_seconds": duration_seconds,
        "tab_count": min(sample["tab_count"] for sample in samples),
        "viewport_width": min(sample["viewport_width"] for sample in samples),
        "viewport_height": min(sample["viewport_height"] for sample in samples),
        "min_fps": min_fps,
        "max_stall_ms": max(sample["max_frame_gap_ms"] for sample in samples),
        "pointer_updates": counter_delta(samples, "pointer_updates"),
        "navigation_latency_sample_count": len(navigation_latencies),
        "navigation_p95_ms": percentile_95(navigation_latencies),
        "navigation_max_ms": max(navigation_latencies, default=0),
        "session_latency_sample_count": len(session_latencies),
        "session_latency_p95_ms": percentile_95(session_latencies),
        "session_latency_max_ms": max(session_latencies, default=0),
        "partial_uploads": counter_delta(samples, "partial_uploads"),
        "hidden_repaints": counter_delta(samples, "hidden_repaints"),
        "hidden_repaint_active_intervals": repaint_behavior.active_intervals,
        "hidden_repaint_interval_count": repaint_behavior.interval_count,
        "hidden_repaint_longest_burst_ms": repaint_behavior.longest_active_ms,
        "hidden_repaint_quiescent_ms": repaint_behavior.longest_quiescent_ms,
        "reconnects": counter_delta(samples, "reconnects"),
        "recovery_observed": recovery,
    }
    checks = (
        ("endpoint_completed", observation.completion_status == "completed"),
        ("sample_cadence", cadence_observed),
        ("duration_seconds", metrics["duration_seconds"] >= PASS_DURATION_SECONDS),
        ("tab_count", metrics["tab_count"] >= 5),
        (
            "viewport",
            metrics["viewport_width"] >= 1920 and metrics["viewport_height"] >= 1080,
        ),
        ("min_fps", metrics["min_fps"] >= 30),
        ("max_stall_ms", metrics["max_stall_ms"] <= 500),
        ("pointer_updates", metrics["pointer_updates"] > 0),
        (
            "navigation_latency_coverage",
            metrics["navigation_latency_sample_count"]
            >= MIN_NAVIGATION_LATENCY_SAMPLES,
        ),
        (
            "navigation_p95_ms",
            0 < metrics["navigation_p95_ms"] <= LATENCY_P95_LIMIT_MS,
        ),
        (
            "navigation_max_ms",
            0 < metrics["navigation_max_ms"] <= LATENCY_MAX_LIMIT_MS,
        ),
        (
            "session_latency_coverage",
            metrics["session_latency_sample_count"] >= MIN_SESSION_LATENCY_SAMPLES,
        ),
        (
            "session_latency_p95_ms",
            0 < metrics["session_latency_p95_ms"] <= LATENCY_P95_LIMIT_MS,
        ),
        (
            "session_latency_max_ms",
            0 < metrics["session_latency_max_ms"] <= LATENCY_MAX_LIMIT_MS,
        ),
        ("partial_uploads", metrics["partial_uploads"] > 0),
        (
            "hidden_repaint_burst",
            metrics["hidden_repaint_longest_burst_ms"]
            <= MAX_HIDDEN_REPAINT_BURST_MS,
        ),
        (
            "hidden_repaint_frequency",
            repaint_behavior.active_fraction <= MAX_HIDDEN_REPAINT_ACTIVE_FRACTION,
        ),
        (
            "hidden_repaint_quiescence",
            metrics["hidden_repaint_quiescent_ms"] >= HIDDEN_QUIESCENT_WINDOW_MS,
        ),
        ("reconnects", metrics["reconnects"] > 0),
        ("recovery_observed", metrics["recovery_observed"]),
        (
            "connection_state",
            samples[-1]["connection_state"] == "connected"
            and all(
                sample["connection_state"] not in {"failed", "unavailable"}
                for sample in samples
            ),
        ),
    )
    failures = tuple(name for name, met in checks if not met)
    return Assessment(**metrics, criteria_met=not failures, failures=failures)


def evidence_status(observation: Observation, assessment: Assessment, live: bool) -> str:
    if observation.completion_status == "unavailable":
        return "unavailable"
    if observation.completion_status == "failed":
        return "failed"
    if not live:
        return "unavailable"
    return "passed" if assessment.criteria_met else "failed"


def make_evidence(
    assessment: Assessment,
    status: str,
    source_commit: str,
    image_digest: str,
    transport: str,
    recorded_at: str,
) -> dict[str, Any]:
    return {
        "schema_version": EVIDENCE_SCHEMA_VERSION,
        "kind": "browser_vm_performance",
        "profile": "browser-vm-chromium",
        "image": "browser-vm-chromium",
        "status": status,
        "source": "live-browser-vm-acceptance",
        "source_commit": source_commit,
        "image_digest": image_digest,
        "transport": transport,
        "duration_seconds": assessment.duration_seconds,
        "tab_count": assessment.tab_count,
        "viewport_width": assessment.viewport_width,
        "viewport_height": assessment.viewport_height,
        "min_fps": assessment.min_fps,
        "max_stall_ms": assessment.max_stall_ms,
        "pointer_updates": assessment.pointer_updates,
        "navigation_latency_sample_count": assessment.navigation_latency_sample_count,
        "navigation_p95_ms": assessment.navigation_p95_ms,
        "navigation_max_ms": assessment.navigation_max_ms,
        "session_latency_sample_count": assessment.session_latency_sample_count,
        "session_latency_p95_ms": assessment.session_latency_p95_ms,
        "session_latency_max_ms": assessment.session_latency_max_ms,
        "partial_uploads": assessment.partial_uploads,
        "hidden_repaints": assessment.hidden_repaints,
        "hidden_repaint_active_intervals": assessment.hidden_repaint_active_intervals,
        "hidden_repaint_interval_count": assessment.hidden_repaint_interval_count,
        "hidden_repaint_longest_burst_ms": assessment.hidden_repaint_longest_burst_ms,
        "hidden_repaint_quiescent_ms": assessment.hidden_repaint_quiescent_ms,
        "reconnects": assessment.reconnects,
        "recovery_observed": assessment.recovery_observed,
        "recorded_at": recorded_at,
    }


def load_verifier() -> Any:
    path = Path(__file__).with_name("verify-browser-vm-performance.py")
    spec = importlib.util.spec_from_file_location("browser_vm_performance_verifier", path)
    if spec is None or spec.loader is None:
        fail("cannot load the Browser VM performance verifier")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def validate_evidence(evidence: dict[str, Any]) -> dict[str, Any]:
    if evidence.get("transport") not in ADMITTED_TRANSPORTS:
        fail("generated evidence has an unsupported Browser VM transport")
    verifier = load_verifier()
    if verifier.SCHEMA_VERSION != EVIDENCE_SCHEMA_VERSION:
        fail("collector and verifier evidence schema versions differ")
    if tuple(verifier.ADMITTED_TRANSPORTS) != ADMITTED_TRANSPORTS:
        fail("collector and verifier transport contracts differ")
    synchronized_thresholds = (
        ("MAX_LATENCY_SAMPLES", MAX_LATENCY_SAMPLES),
        ("MAX_SAMPLE_INTERVALS", MAX_STREAM_RECORDS),
        ("LATENCY_P95_LIMIT_MS", LATENCY_P95_LIMIT_MS),
        ("LATENCY_MAX_LIMIT_MS", LATENCY_MAX_LIMIT_MS),
        ("MIN_NAVIGATION_LATENCY_SAMPLES", MIN_NAVIGATION_LATENCY_SAMPLES),
        ("MIN_SESSION_LATENCY_SAMPLES", MIN_SESSION_LATENCY_SAMPLES),
        ("HIDDEN_QUIESCENT_WINDOW_MS", HIDDEN_QUIESCENT_WINDOW_MS),
        ("MAX_HIDDEN_REPAINT_BURST_MS", MAX_HIDDEN_REPAINT_BURST_MS),
        (
            "MAX_HIDDEN_REPAINT_ACTIVE_FRACTION",
            MAX_HIDDEN_REPAINT_ACTIVE_FRACTION,
        ),
    )
    for name, expected_value in synchronized_thresholds:
        if getattr(verifier, name, None) != expected_value:
            fail(f"collector and verifier {name} contracts differ")
    try:
        result = verifier.validate_document(evidence)
    except Exception as exc:
        fail(f"generated evidence was rejected by the verifier: {exc}")
    expected = "validated" if evidence["status"] == "passed" else evidence["status"]
    if result.get("status") != expected:
        fail("generated evidence verifier status does not match the collector status")
    return result


def prepare_output(path: Path) -> None:
    try:
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        if path.is_symlink():
            fail("output path must not be a symlink")
        if path.exists() and not path.is_file():
            fail("output path must be a regular file")
    except OSError as exc:
        fail(f"output path is not writable: {exc}")


def atomic_write_evidence(path: Path, evidence: dict[str, Any]) -> None:
    prepare_output(path)
    payload = (json.dumps(evidence, indent=2, sort_keys=True) + "\n").encode("utf-8")
    if len(payload) > MAX_EVIDENCE_BYTES:
        fail(f"evidence exceeds {MAX_EVIDENCE_BYTES} bytes")
    fd = -1
    temporary: Path | None = None
    try:
        fd, name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
        temporary = Path(name)
        os.fchmod(fd, 0o600)
        with os.fdopen(fd, "wb", closefd=True) as handle:
            fd = -1
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        temporary = None
        directory_fd = os.open(path.parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    except OSError as exc:
        fail(f"cannot atomically write private evidence: {exc}")
    finally:
        if fd >= 0:
            os.close(fd)
        if temporary is not None:
            try:
                temporary.unlink()
            except FileNotFoundError:
                pass


def empty_observation(status: str) -> Observation:
    return Observation((), (), status)


def utc_now() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).strftime("%Y-%m-%dT%H:%M:%SZ")


def collect(args: argparse.Namespace) -> int:
    validate_source_binding(args.source_commit, args.image_digest)
    validate_endpoint(args.endpoint)
    prepare_output(args.out)
    diagnostic: str | None = None
    try:
        observation = consume_endpoint(
            args.endpoint,
            args.source_commit,
            args.image_digest,
            args.transport,
            args.read_timeout_seconds,
        )
    except EndpointUnavailable as exc:
        observation = empty_observation("unavailable")
        diagnostic = str(exc)
    except (EndpointFailed, CollectorError) as exc:
        observation = empty_observation("failed")
        diagnostic = str(exc)
    assessment = assess(observation)
    status = evidence_status(observation, assessment, live=True)
    evidence = make_evidence(
        assessment,
        status,
        args.source_commit,
        args.image_digest,
        args.transport,
        utc_now(),
    )
    validate_evidence(evidence)
    atomic_write_evidence(args.out, evidence)
    if diagnostic:
        print(f"collect-browser-vm-performance: {diagnostic}", file=sys.stderr)
    elif assessment.failures:
        print(
            "collect-browser-vm-performance: live thresholds not met: "
            + ", ".join(assessment.failures),
            file=sys.stderr,
        )
    print(json.dumps(evidence, indent=2, sort_keys=True))
    return 0 if status == "passed" else 1


def dry_run(args: argparse.Namespace) -> int:
    validate_source_binding(args.source_commit, args.image_digest)
    observation = read_fixture(
        args.fixture, args.source_commit, args.image_digest, args.transport
    )
    assessment = assess(observation)
    status = evidence_status(observation, assessment, live=False)
    if status == "passed":
        fail("fixture path attempted to synthesize passing evidence")
    evidence = make_evidence(
        assessment,
        status,
        args.source_commit,
        args.image_digest,
        args.transport,
        DRY_RUN_RECORDED_AT,
    )
    validate_evidence(evidence)
    if args.out is not None:
        atomic_write_evidence(args.out, evidence)
    result = {
        "dry_run": True,
        "evidence": evidence,
        "fixture_threshold_coverage": "complete" if assessment.criteria_met else "incomplete",
        "threshold_failures": list(assessment.failures),
    }
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


def assert_raises(callback: Any, needle: str) -> None:
    try:
        callback()
    except CollectorError as exc:
        if needle not in str(exc):
            raise AssertionError((needle, str(exc))) from exc
    else:
        raise AssertionError(f"expected CollectorError containing {needle!r}")


def self_test() -> None:
    fixture_root = Path(__file__).with_name("fixtures") / "browser-vm-performance"
    source_commit = "0123456789abcdef0123456789abcdef01234567"
    image_digest = "sha256:" + "a" * 64
    passing = read_fixture(
        fixture_root / "passing.ndjson", source_commit, image_digest, DEFAULT_TRANSPORT
    )
    assessment = assess(passing)
    assert assessment.criteria_met, assessment.failures
    assert assessment.duration_seconds == 900
    assert assessment.tab_count == 5
    assert assessment.viewport_width == 1920
    assert assessment.viewport_height == 1080
    assert assessment.min_fps == 30
    assert assessment.max_stall_ms == 34
    assert assessment.pointer_updates == 180
    assert assessment.partial_uploads == 180
    assert assessment.navigation_latency_sample_count >= MIN_NAVIGATION_LATENCY_SAMPLES
    assert assessment.navigation_p95_ms <= LATENCY_P95_LIMIT_MS
    assert assessment.navigation_max_ms <= LATENCY_MAX_LIMIT_MS
    assert assessment.session_latency_sample_count >= MIN_SESSION_LATENCY_SAMPLES
    assert assessment.session_latency_p95_ms <= LATENCY_P95_LIMIT_MS
    assert assessment.session_latency_max_ms <= LATENCY_MAX_LIMIT_MS
    assert assessment.hidden_repaints == 1
    assert assessment.hidden_repaint_active_intervals == 1
    assert assessment.hidden_repaint_interval_count == len(passing.samples) - 1
    assert assessment.hidden_repaint_longest_burst_ms == 5_000
    assert assessment.hidden_repaint_quiescent_ms >= HIDDEN_QUIESCENT_WINDOW_MS
    assert assessment.reconnects == 1
    assert assessment.recovery_observed

    fixture_status = evidence_status(passing, assessment, live=False)
    assert fixture_status == "unavailable"
    fixture_evidence = make_evidence(
        assessment,
        fixture_status,
        source_commit,
        image_digest,
        DEFAULT_TRANSPORT,
        DRY_RUN_RECORDED_AT,
    )
    validate_evidence(fixture_evidence)

    live_sunshine_evidence = make_evidence(
        assessment,
        "passed",
        source_commit,
        image_digest,
        DEFAULT_TRANSPORT,
        DRY_RUN_RECORDED_AT,
    )
    sunshine_result = validate_evidence(live_sunshine_evidence)
    assert sunshine_result["status"] == "validated"
    assert sunshine_result["transport"] == DEFAULT_TRANSPORT
    assert (
        sunshine_result["navigation_latency_sample_count"]
        == assessment.navigation_latency_sample_count
    )
    assert sunshine_result["navigation_max_ms"] == assessment.navigation_max_ms
    assert (
        sunshine_result["hidden_repaint_quiescent_ms"]
        == assessment.hidden_repaint_quiescent_ms
    )

    rdp_evidence = make_evidence(
        assessment,
        fixture_status,
        source_commit,
        image_digest,
        "rdp",
        DRY_RUN_RECORDED_AT,
    )
    validate_evidence(rdp_evidence)
    live_rdp_result = validate_evidence(dict(live_sunshine_evidence, transport="rdp"))
    assert live_rdp_result["status"] == "validated"
    assert live_rdp_result["transport"] == "rdp"
    assert_raises(
        lambda: validate_evidence(dict(live_sunshine_evidence, transport="spice")),
        "unsupported Browser VM transport",
    )
    assert_raises(
        lambda: validate_evidence(dict(live_sunshine_evidence, schema_version=1)),
        "schema_version",
    )
    assert_raises(
        lambda: validate_evidence(
            dict(live_sunshine_evidence, navigation_max_ms=LATENCY_MAX_LIMIT_MS + 1)
        ),
        "navigation_max_ms",
    )

    rdp_header = decode_record(
        (fixture_root / "passing.ndjson").read_bytes().splitlines()[0], 1
    )
    rdp_header["transport"] = "rdp"
    validate_header(rdp_header, source_commit, image_digest, "rdp")

    unavailable = read_fixture(
        fixture_root / "unavailable.ndjson",
        source_commit,
        image_digest,
        DEFAULT_TRANSPORT,
    )
    unavailable_assessment = assess(unavailable)
    assert evidence_status(unavailable, unavailable_assessment, live=False) == "unavailable"
    assert not unavailable_assessment.criteria_met

    failed = read_fixture(
        fixture_root / "failed.ndjson", source_commit, image_digest, DEFAULT_TRANSPORT
    )
    failed_assessment = assess(failed)
    failed_status = evidence_status(failed, failed_assessment, live=False)
    assert failed_status == "failed"
    failed_evidence = make_evidence(
        failed_assessment,
        failed_status,
        source_commit,
        image_digest,
        DEFAULT_TRANSPORT,
        DRY_RUN_RECORDED_AT,
    )
    validate_evidence(failed_evidence)

    degraded_samples = [dict(sample) for sample in passing.samples]
    degraded_samples[len(degraded_samples) // 2]["tab_count"] = 4
    degraded = Observation(
        tuple(degraded_samples), passing.receipt_seconds, passing.completion_status
    )
    degraded_assessment = assess(degraded)
    assert not degraded_assessment.criteria_met
    assert "tab_count" in degraded_assessment.failures
    assert evidence_status(degraded, degraded_assessment, live=False) == "unavailable"

    high_p95_samples = [dict(sample) for sample in passing.samples]
    for sample in high_p95_samples:
        if sample["navigation_latencies_ms"]:
            sample["navigation_latencies_ms"] = [LATENCY_P95_LIMIT_MS + 1]
    high_p95 = assess(
        Observation(
            tuple(high_p95_samples), passing.receipt_seconds, passing.completion_status
        )
    )
    assert "navigation_p95_ms" in high_p95.failures

    high_session_p95_samples = [dict(sample) for sample in passing.samples]
    for sample in high_session_p95_samples:
        if sample["session_latencies_ms"]:
            sample["session_latencies_ms"] = [LATENCY_P95_LIMIT_MS + 1]
    high_session_p95 = assess(
        Observation(
            tuple(high_session_p95_samples),
            passing.receipt_seconds,
            passing.completion_status,
        )
    )
    assert "session_latency_p95_ms" in high_session_p95.failures

    high_max_samples = [dict(sample) for sample in passing.samples]
    high_max_samples[-1]["session_latencies_ms"] = [LATENCY_MAX_LIMIT_MS + 1]
    high_max = assess(
        Observation(
            tuple(high_max_samples), passing.receipt_seconds, passing.completion_status
        )
    )
    assert "session_latency_max_ms" in high_max.failures

    high_navigation_max_samples = [dict(sample) for sample in passing.samples]
    high_navigation_max_samples[-1]["navigation_latencies_ms"] = [
        LATENCY_MAX_LIMIT_MS + 1
    ]
    high_navigation_max = assess(
        Observation(
            tuple(high_navigation_max_samples),
            passing.receipt_seconds,
            passing.completion_status,
        )
    )
    assert "navigation_max_ms" in high_navigation_max.failures

    sparse_latency_samples = [dict(sample) for sample in passing.samples]
    retained = False
    for sample in sparse_latency_samples:
        if sample["navigation_latencies_ms"] and not retained:
            retained = True
        else:
            sample["navigation_latencies_ms"] = []
    sparse_latency = assess(
        Observation(
            tuple(sparse_latency_samples),
            passing.receipt_seconds,
            passing.completion_status,
        )
    )
    assert "navigation_latency_coverage" in sparse_latency.failures

    continuous_repaint_samples = [dict(sample) for sample in passing.samples]
    base_repaints = continuous_repaint_samples[0]["hidden_repaints"]
    for index, sample in enumerate(continuous_repaint_samples):
        sample["hidden_repaints"] = base_repaints + index
    continuous_repaint = assess(
        Observation(
            tuple(continuous_repaint_samples),
            passing.receipt_seconds,
            passing.completion_status,
        )
    )
    assert "hidden_repaint_frequency" in continuous_repaint.failures
    assert "hidden_repaint_quiescence" in continuous_repaint.failures

    assert_raises(
        lambda: decode_record(b'{"type":"sample","type":"sample"}', 1),
        "duplicate JSON field",
    )
    assert_raises(
        lambda: read_fixture(
            fixture_root / "passing.ndjson",
            "f" * 40,
            image_digest,
            DEFAULT_TRANSPORT,
        ),
        "source_commit does not match",
    )
    for unsafe in (
        "file:///tmp/fixture",
        "http://user:password@127.0.0.1:9000/session",
        "http://127.0.0.1:9000/session?token=no",
    ):
        assert_raises(lambda unsafe=unsafe: validate_endpoint(unsafe), "endpoint")

    with tempfile.TemporaryDirectory(prefix="browser-vm-performance-self-test-") as root:
        output = Path(root) / "private" / "evidence.json"
        atomic_write_evidence(output, fixture_evidence)
        assert stat.S_IMODE(output.stat().st_mode) == 0o600
        assert output.stat().st_size <= MAX_EVIDENCE_BYTES
        loaded = json.loads(output.read_text(encoding="utf-8"))
        assert loaded == fixture_evidence
        validate_evidence(loaded)
        verifier = load_verifier()
        verifier_loaded = verifier.read_json(output)
        assert verifier_loaded == fixture_evidence
        assert verifier.validate_document(verifier_loaded)["status"] == fixture_status

        blocked_target = Path(root) / "blocked-target.json"
        blocked_target.write_text("do not replace\n", encoding="utf-8")
        blocked = Path(root) / "blocked.json"
        blocked.symlink_to(blocked_target)
        assert_raises(lambda: atomic_write_evidence(blocked, fixture_evidence), "symlink")
        assert blocked_target.read_text(encoding="utf-8") == "do not replace\n"

    print("collect-browser-vm-performance.py: self-test passed")


def add_binding_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--image-digest", required=True)
    parser.add_argument(
        "--transport",
        choices=ADMITTED_TRANSPORTS,
        default=DEFAULT_TRANSPORT,
        help="sunshine (Sunshine/Moonlight, default) or explicit rdp",
    )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    subparsers = parser.add_subparsers(dest="command")

    live_parser = subparsers.add_parser("collect", help="consume one explicit live endpoint")
    live_parser.add_argument("--endpoint", required=True)
    live_parser.add_argument("--out", required=True, type=Path)
    live_parser.add_argument(
        "--read-timeout-seconds",
        type=int,
        choices=range(1, 61),
        default=20,
        metavar="1..60",
        help="maximum silence while waiting for the next telemetry record",
    )
    add_binding_arguments(live_parser)

    fixture_parser = subparsers.add_parser(
        "dry-run", help="exercise thresholds with a deterministic fixture; never passes"
    )
    fixture_parser.add_argument("--fixture", required=True, type=Path)
    fixture_parser.add_argument("--out", type=Path)
    add_binding_arguments(fixture_parser)

    args = parser.parse_args(argv)
    try:
        if args.self_test:
            if args.command is not None:
                parser.error("--self-test does not accept a subcommand")
            self_test()
            return 0
        if args.command == "collect":
            return collect(args)
        if args.command == "dry-run":
            return dry_run(args)
        parser.error("choose collect, dry-run, or --self-test")
    except CollectorError as exc:
        print(f"collect-browser-vm-performance: rejected: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
