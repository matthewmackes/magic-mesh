#!/usr/bin/env python3
"""Collect bounded Browser VM performance evidence from one live session.

The collector consumes a versioned NDJSON telemetry stream from an explicitly
supplied, credential-free HTTP(S) endpoint.  The endpoint must describe the
running ``browser-vm`` session and stream cumulative shell VDI counters plus
guest Chromium counters.  A real pass requires the collector to remain
attached to that live stream for at least 900 seconds; endpoint timestamps
alone cannot shorten the observation.

Fixtures are admitted only by ``dry-run`` and ``--self-test``.  They exercise
the legacy parser without sleeping, but their evidence status is always
``unavailable``.  Live collection uses a fresh 256-bit challenge that the
version-3 endpoint must bind into its terminal stream digest, so replaying a
captured fixture through HTTP is rejected.  No command-line option can supply
a performance counter or turn fixture data, reachability, or a partial stream
into a pass.

Live endpoint protocol (one compact JSON object per line):

* one version-4 ``browser_vm_performance_stream`` header bound to the
  source/image, fresh collector challenge, host/guest boot identities, domain,
  immutable browser/workload/desktop-source instances, the five-or-more CDP tab
  identities, and named shell/guest metric sources;
* ordered ``sample`` records, normally at five-second cadence, carrying shell
  VDI counters, every admitted tab's guest source-frame counter and dimensions,
  pointer coordinates, raw full/partial upload and surface repaint counters,
  host-process CPU and DRM GPU load readings, and visibility state; and
* one terminal ``complete`` record which echoes the challenge and binds its
  status and sample count to the SHA-256 of canonical compact, key-sorted JSON
  for the exact header plus samples (one trailing newline per record).

Usage:
  collect-browser-vm-performance.py collect \
    --endpoint http://127.0.0.1:9080/v1/browser-vm/performance \
    --source-commit <40-hex> --image-digest sha256:<64-hex> \
    --out performance-evidence.json
  collect-browser-vm-performance.py dry-run \
    --fixture install-helpers/fixtures/browser-vm-performance/passing.ndjson \
    --source-commit <40-hex> --image-digest sha256:<64-hex> \
    --transport sunshine
  collect-browser-vm-performance.py --self-test
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
import hashlib
import importlib.util
import ipaddress
import json
import math
import os
from pathlib import Path
import re
import secrets
import socket
import stat
import sys
import tempfile
import time
from typing import Any, Iterable, NoReturn
import urllib.error
import urllib.parse
import urllib.request


LIVE_STREAM_SCHEMA_VERSION = 4
FIXTURE_STREAM_SCHEMA_VERSION = 1
EVIDENCE_SCHEMA_VERSION = 5
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
MAX_HOST_LOAD_PERMILLE = 100_000
MAX_EVIDENCE_BYTES = 64 * 1024
DRY_RUN_RECORDED_AT = "2020-01-02T03:04:05Z"
DRY_RUN_STARTED_AT = "2020-01-02T02:49:05Z"
LATENCY_P95_LIMIT_MS = 100
LATENCY_MAX_LIMIT_MS = 250
MIN_NAVIGATION_LATENCY_SAMPLES = 15
MIN_SESSION_LATENCY_SAMPLES = 15
HIDDEN_QUIESCENT_WINDOW_MS = 5 * 60 * 1000
MAX_HIDDEN_REPAINT_BURST_MS = 2 * MAX_SAMPLE_GAP_MS
MAX_HIDDEN_REPAINT_ACTIVE_FRACTION = 0.05
MIN_CADENCE_RATIO_PERMILLE = 900
STATIONARY_POINTER_WINDOW_MS = 5 * 60 * 1000
MIN_MEDIA_TAB_COUNT = 5
MIN_HOST_LOAD_COVERAGE_PERMILLE = 900
DEFAULT_TRANSPORT = "rdp"
ADMITTED_TRANSPORTS = (DEFAULT_TRANSPORT, "sunshine")
LIVE_COLLECTION_MODE = "live-endpoint-v4"
FIXTURE_COLLECTION_MODE = "fixture-dry-run"
SHELL_METRICS_SOURCE = "mde-shell-egui-vdi"
GUEST_METRICS_SOURCE = "chromium-devtools"
COLLECTION_NONCE_HEADER = "X-MCNF-Collection-Nonce"

COMMON_HEADER_FIELDS = frozenset(
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
LIVE_HEADER_FIELDS = COMMON_HEADER_FIELDS | frozenset(
    {
        "collection_nonce",
        "shell_metrics_source",
        "guest_metrics_source",
        "host_boot_id",
        "guest_boot_id",
        "domain_uuid",
        "browser_instance_id",
        "workload_instance_id",
        "source_instance_id",
        "tab_ids",
        "supported_target_fps",
    }
)
BASE_SAMPLE_FIELDS = frozenset(
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
        "reconnects",
        "connection_state",
    }
)
FIXTURE_SAMPLE_FIELDS = BASE_SAMPLE_FIELDS | frozenset(
    {
        "partial_uploads",
        "hidden_repaints",
    }
)
LIVE_SAMPLE_FIELDS = BASE_SAMPLE_FIELDS | frozenset(
    {
        "tab_source_frames",
        "visible_tab_id",
        "browser_visible",
        "pointer_x",
        "pointer_y",
        "full_uploads",
        "partial_uploads",
        "partial_rects",
        "surface_repaints",
        "host_process_cpu_permille",
        "host_gpu_busy_permille",
    }
)
TAB_SOURCE_FRAME_FIELDS = frozenset({"frames_presented", "width", "height"})
FIXTURE_COMPLETE_FIELDS = frozenset({"type", "status"})
LIVE_COMPLETE_FIELDS = frozenset(
    {"type", "status", "collection_nonce", "sample_count", "stream_sha256"}
)
SOURCE_COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")
IMAGE_DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
SESSION_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$")
COLLECTION_NONCE_RE = re.compile(r"^[0-9a-f]{64}$")
UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
)


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


def optional_bounded_uint(value: Any, maximum: int, label: str) -> int | None:
    if value is None:
        return None
    return bounded_uint(value, maximum, label)


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
    hostname = parsed.hostname.lower()
    loopback = hostname == "localhost"
    if not loopback:
        try:
            loopback = ipaddress.ip_address(hostname).is_loopback
        except ValueError:
            loopback = False
    if parsed.scheme == "http" and not loopback:
        fail("non-loopback performance endpoint must use https")
    return raw


def endpoint_origin(raw: str) -> tuple[str, str, int]:
    parsed = urllib.parse.urlsplit(validate_endpoint(raw))
    port = parsed.port or (443 if parsed.scheme == "https" else 80)
    return parsed.scheme, parsed.hostname.lower(), port


@dataclass(frozen=True)
class StreamIdentity:
    live_protocol: bool
    session_id: str
    collection_nonce: str | None
    host_boot_id: str | None
    guest_boot_id: str | None
    domain_uuid: str | None
    browser_instance_id: str | None
    workload_instance_id: str | None
    source_instance_id: str | None
    tab_ids: tuple[str, ...]
    supported_target_fps: int


def validate_uuid(value: Any, label: str) -> str:
    if not isinstance(value, str) or UUID_RE.fullmatch(value) is None:
        fail(f"{label} must be a canonical lowercase UUID")
    if value == "00000000-0000-0000-0000-000000000000":
        fail(f"{label} must be non-null")
    return value


def validate_tab_ids(value: Any, label: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not MIN_MEDIA_TAB_COUNT <= len(value) <= MAX_TABS:
        fail(
            f"{label} must contain between {MIN_MEDIA_TAB_COUNT} and {MAX_TABS} tab identities"
        )
    tab_ids: list[str] = []
    for index, tab_id in enumerate(value):
        if not isinstance(tab_id, str) or SESSION_ID_RE.fullmatch(tab_id) is None:
            fail(f"{label}[{index}] is malformed")
        if tab_id in tab_ids:
            fail(f"{label} contains duplicate tab identity: {tab_id}")
        tab_ids.append(tab_id)
    return tuple(tab_ids)


def validate_header(
    data: dict[str, Any],
    source_commit: str,
    image_digest: str,
    transport: str,
    *,
    live: bool,
    expected_nonce: str | None,
) -> StreamIdentity:
    exact_fields(
        data,
        LIVE_HEADER_FIELDS if live else COMMON_HEADER_FIELDS,
        "stream header",
    )
    expected_schema = (
        LIVE_STREAM_SCHEMA_VERSION if live else FIXTURE_STREAM_SCHEMA_VERSION
    )
    if data["schema_version"] != expected_schema or isinstance(
        data["schema_version"], bool
    ):
        fail(f"stream schema_version must be integer {expected_schema}")
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
    if not live:
        return StreamIdentity(
            live_protocol=False,
            session_id=data["session_id"],
            collection_nonce=None,
            host_boot_id=None,
            guest_boot_id=None,
            domain_uuid=None,
            browser_instance_id=None,
            workload_instance_id=None,
            source_instance_id=None,
            tab_ids=tuple(f"fixture-tab-{index}" for index in range(1, 6)),
            supported_target_fps=30,
        )

    nonce = data["collection_nonce"]
    if (
        expected_nonce is None
        or not isinstance(nonce, str)
        or COLLECTION_NONCE_RE.fullmatch(nonce) is None
        or nonce != expected_nonce
    ):
        fail("stream collection_nonce does not match the fresh collector challenge")
    if data["shell_metrics_source"] != SHELL_METRICS_SOURCE:
        fail("stream shell_metrics_source is not the Construct VDI seam")
    if data["guest_metrics_source"] != GUEST_METRICS_SOURCE:
        fail("stream guest_metrics_source is not Chromium DevTools")
    host_boot_id = validate_uuid(data["host_boot_id"], "stream host_boot_id")
    guest_boot_id = validate_uuid(data["guest_boot_id"], "stream guest_boot_id")
    domain_uuid = validate_uuid(data["domain_uuid"], "stream domain_uuid")
    browser_instance_id = validate_uuid(
        data["browser_instance_id"], "stream browser_instance_id"
    )
    workload_instance_id = validate_uuid(
        data["workload_instance_id"], "stream workload_instance_id"
    )
    source_instance_id = validate_uuid(
        data["source_instance_id"], "stream source_instance_id"
    )
    runtime_ids = (
        host_boot_id,
        guest_boot_id,
        domain_uuid,
        browser_instance_id,
        workload_instance_id,
        source_instance_id,
    )
    if len(set(runtime_ids)) != len(runtime_ids):
        fail("stream runtime identities must be distinct")
    tab_ids = validate_tab_ids(data["tab_ids"], "stream tab_ids")
    supported_target_fps = bounded_uint(
        data["supported_target_fps"],
        MAX_FPS,
        "stream supported_target_fps",
        minimum=30,
    )
    return StreamIdentity(
        live_protocol=True,
        session_id=data["session_id"],
        collection_nonce=nonce,
        host_boot_id=host_boot_id,
        guest_boot_id=guest_boot_id,
        domain_uuid=domain_uuid,
        browser_instance_id=browser_instance_id,
        workload_instance_id=workload_instance_id,
        source_instance_id=source_instance_id,
        tab_ids=tab_ids,
        supported_target_fps=supported_target_fps,
    )


def validate_latency_list(value: Any, label: str) -> list[int]:
    if not isinstance(value, list) or len(value) > MAX_LATENCIES_PER_SAMPLE:
        fail(f"{label} must be a bounded list")
    return [
        bounded_uint(item, MAX_LATENCY_MS, f"{label}[{index}]", minimum=1)
        for index, item in enumerate(value)
    ]


def validate_tab_source_frames(
    value: Any, tab_ids: tuple[str, ...], label: str
) -> dict[str, dict[str, int]]:
    if not isinstance(value, dict):
        fail(f"{label} must be an object keyed by the immutable tab identities")
    expected = frozenset(tab_ids)
    actual = frozenset(value)
    missing = expected - actual
    extra = actual - expected
    if missing:
        fail(f"{label} misses tab identities: {', '.join(sorted(missing))}")
    if extra:
        fail(f"{label} has unexpected tab identities: {', '.join(sorted(extra))}")
    validated: dict[str, dict[str, int]] = {}
    for tab_id in tab_ids:
        source = value[tab_id]
        if not isinstance(source, dict):
            fail(f"{label}.{tab_id} must be one source-frame object")
        exact_fields(source, TAB_SOURCE_FRAME_FIELDS, f"{label}.{tab_id}")
        validated[tab_id] = {
            "frames_presented": bounded_uint(
                source["frames_presented"],
                MAX_FRAME_COUNTER,
                f"{label}.{tab_id}.frames_presented",
            ),
            "width": bounded_uint(
                source["width"],
                MAX_DIMENSION,
                f"{label}.{tab_id}.width",
                minimum=1,
            ),
            "height": bounded_uint(
                source["height"],
                MAX_DIMENSION,
                f"{label}.{tab_id}.height",
                minimum=1,
            ),
        }
    return validated


def validate_sample(
    data: dict[str, Any], index: int, *, identity: StreamIdentity
) -> dict[str, Any]:
    label = f"sample {index}"
    exact_fields(
        data,
        LIVE_SAMPLE_FIELDS if identity.live_protocol else FIXTURE_SAMPLE_FIELDS,
        label,
    )
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
        "reconnects": bounded_uint(data["reconnects"], MAX_COUNTER, f"{label}.reconnects"),
        "connection_state": data["connection_state"],
    }
    if identity.live_protocol:
        if sample["tab_count"] != len(identity.tab_ids):
            fail(
                f"{label}.tab_count does not match the immutable stream tab identities"
            )
        sample["tab_source_frames"] = validate_tab_source_frames(
            data["tab_source_frames"], identity.tab_ids, f"{label}.tab_source_frames"
        )
        if not isinstance(data["browser_visible"], bool):
            fail(f"{label}.browser_visible must be boolean")
        sample["browser_visible"] = data["browser_visible"]
        visible_tab_id = data["visible_tab_id"]
        if sample["browser_visible"]:
            if not isinstance(visible_tab_id, str) or visible_tab_id not in identity.tab_ids:
                fail(f"{label}.visible_tab_id must name one immutable stream tab")
            sample["visible_tab_id"] = visible_tab_id
        else:
            if visible_tab_id is not None:
                fail(f"{label}.visible_tab_id must be null while Browser is hidden")
            sample["visible_tab_id"] = None
        sample["pointer_x"] = bounded_uint(
            data["pointer_x"], MAX_DIMENSION, f"{label}.pointer_x"
        )
        sample["pointer_y"] = bounded_uint(
            data["pointer_y"], MAX_DIMENSION, f"{label}.pointer_y"
        )
        if (
            sample["viewport_width"] == 0
            or sample["pointer_x"] >= sample["viewport_width"]
            or sample["viewport_height"] == 0
            or sample["pointer_y"] >= sample["viewport_height"]
        ):
            fail(f"{label} pointer coordinates fall outside the viewport")
        sample["full_uploads"] = bounded_uint(
            data["full_uploads"], MAX_COUNTER, f"{label}.full_uploads"
        )
        sample["partial_uploads"] = bounded_uint(
            data["partial_uploads"], MAX_COUNTER, f"{label}.partial_uploads"
        )
        sample["partial_rects"] = bounded_uint(
            data["partial_rects"], MAX_COUNTER, f"{label}.partial_rects"
        )
        sample["surface_repaints"] = bounded_uint(
            data["surface_repaints"], MAX_COUNTER, f"{label}.surface_repaints"
        )
        sample["host_process_cpu_permille"] = optional_bounded_uint(
            data["host_process_cpu_permille"],
            MAX_HOST_LOAD_PERMILLE,
            f"{label}.host_process_cpu_permille",
        )
        sample["host_gpu_busy_permille"] = optional_bounded_uint(
            data["host_gpu_busy_permille"],
            MAX_HOST_LOAD_PERMILLE,
            f"{label}.host_gpu_busy_permille",
        )
    else:
        # Legacy fixtures exercise parser/threshold plumbing only.  They never
        # receive a live status, and they intentionally lack the v4 provenance
        # and visibility fields required for acceptance.
        sample["tab_source_frames"] = {
            tab_id: {
                "frames_presented": sample["frames_received"],
                "width": sample["viewport_width"],
                "height": sample["viewport_height"],
            }
            for tab_id in identity.tab_ids
        }
        sample["visible_tab_id"] = identity.tab_ids[0]
        sample["browser_visible"] = True
        sample["pointer_x"] = sample["pointer_updates"] % max(
            1, sample["viewport_width"]
        )
        sample["pointer_y"] = 0
        partial_uploads = bounded_uint(
            data["partial_uploads"], MAX_COUNTER, f"{label}.partial_uploads"
        )
        sample["full_uploads"] = 0
        sample["partial_uploads"] = partial_uploads
        sample["partial_rects"] = partial_uploads
        sample["surface_repaints"] = bounded_uint(
            data["hidden_repaints"], MAX_COUNTER, f"{label}.hidden_repaints"
        )
        sample["host_process_cpu_permille"] = None
        sample["host_gpu_busy_permille"] = None
    if sample["partial_rects"] < sample["partial_uploads"]:
        fail(f"{label}.partial_rects cannot be less than partial_uploads")
    if (
        sample["full_uploads"] + sample["partial_uploads"]
        > sample["frames_received"]
    ):
        fail(f"{label} upload counters cannot exceed frames_received")
    if sample["connection_state"] not in {
        "connected",
        "reconnecting",
        "failed",
        "unavailable",
    }:
        fail(f"{label}.connection_state is not admitted")
    return sample


def validate_complete(
    data: dict[str, Any],
    *,
    live_protocol: bool,
    expected_nonce: str | None,
    expected_sample_count: int,
    expected_stream_sha256: str,
) -> str:
    exact_fields(
        data,
        LIVE_COMPLETE_FIELDS if live_protocol else FIXTURE_COMPLETE_FIELDS,
        "complete record",
    )
    if data["type"] != "complete":
        fail("terminal record type is not complete")
    if data["status"] not in {"completed", "failed", "unavailable"}:
        fail("complete status must be completed, failed, or unavailable")
    if live_protocol:
        if data["collection_nonce"] != expected_nonce:
            fail("complete record does not echo the fresh collector challenge")
        sample_count = bounded_uint(
            data["sample_count"], MAX_STREAM_RECORDS, "complete record sample_count"
        )
        if sample_count != expected_sample_count:
            fail("complete record sample_count does not match the stream")
        if data["stream_sha256"] != f"sha256:{expected_stream_sha256}":
            fail("complete record stream_sha256 does not bind the exact stream")
    return data["status"]


@dataclass(frozen=True)
class Observation:
    samples: tuple[dict[str, Any], ...]
    receipt_seconds: tuple[float, ...]
    completion_status: str
    identity: StreamIdentity | None
    stream_sha256: str | None


def canonical_stream_sha256(
    records: Iterable[tuple[dict[str, Any], float]],
) -> str:
    digest = hashlib.sha256()
    for record, _receipt in records:
        encoded = json.dumps(
            record,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        ).encode("utf-8")
        digest.update(encoded)
        digest.update(b"\n")
    return digest.hexdigest()


def parse_records(
    records: Iterable[tuple[dict[str, Any], float]],
    source_commit: str,
    image_digest: str,
    transport: str,
    *,
    live: bool,
    expected_nonce: str | None,
) -> Observation:
    materialized = list(records)
    if not materialized:
        fail("live endpoint returned no stream records")
    if len(materialized) > MAX_STREAM_RECORDS:
        fail(f"live endpoint exceeds {MAX_STREAM_RECORDS} stream records")
    header, _ = materialized[0]
    identity = validate_header(
        header,
        source_commit,
        image_digest,
        transport,
        live=live,
        expected_nonce=expected_nonce,
    )

    samples: list[dict[str, Any]] = []
    receipts: list[float] = []
    completion_status: str | None = None
    stream_sha256: str | None = None
    latency_count = 0
    previous: dict[str, Any] | None = None
    for materialized_index, (record, receipt) in enumerate(
        materialized[1:], start=1
    ):
        if completion_status is not None:
            fail("stream contains records after its terminal complete marker")
        if record.get("type") == "complete":
            stream_sha256 = canonical_stream_sha256(
                materialized[:materialized_index]
            )
            completion_status = validate_complete(
                record,
                live_protocol=identity.live_protocol,
                expected_nonce=expected_nonce,
                expected_sample_count=len(samples),
                expected_stream_sha256=stream_sha256,
            )
            continue
        sample = validate_sample(
            record, len(samples) + 1, identity=identity
        )
        if (
            identity.live_protocol
            and not samples
            and (
                sample["navigation_latencies_ms"]
                or sample["session_latencies_ms"]
            )
        ):
            fail("first live sample must not contain pre-challenge latency events")
        if previous is not None:
            if sample["elapsed_ms"] <= previous["elapsed_ms"]:
                fail("sample elapsed_ms values must increase strictly")
            for field in (
                "frames_received",
                "pointer_updates",
                "full_uploads",
                "partial_uploads",
                "partial_rects",
                "surface_repaints",
                "reconnects",
            ):
                if sample[field] < previous[field]:
                    fail(f"sample cumulative counter decreased: {field}")
            for tab_id in identity.tab_ids:
                if (
                    sample["tab_source_frames"][tab_id]["frames_presented"]
                    < previous["tab_source_frames"][tab_id]["frames_presented"]
                ):
                    fail(
                        "sample cumulative tab source counter decreased: "
                        f"{tab_id}"
                    )
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
    if stream_sha256 is None:
        fail("stream ended without a challenge-bound terminal digest")
    return Observation(
        tuple(samples),
        tuple(receipts),
        completion_status,
        identity,
        stream_sha256,
    )


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
    collection_nonce: str,
) -> Observation:
    validate_endpoint(endpoint)
    requested_origin = endpoint_origin(endpoint)
    request = urllib.request.Request(
        endpoint,
        headers={
            "Accept": "application/x-ndjson, application/json",
            "User-Agent": "magic-mesh-browser-vm-performance-collector/4",
            COLLECTION_NONCE_HEADER: collection_nonce,
        },
        method="GET",
    )
    opened_at = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=read_timeout_seconds) as response:
            final_endpoint = response.geturl()
            validate_endpoint(final_endpoint)
            if endpoint_origin(final_endpoint) != requested_origin:
                raise EndpointFailed("live endpoint redirected to a different origin")
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
        return parse_records(
            records,
            source_commit,
            image_digest,
            transport,
            live=True,
            expected_nonce=collection_nonce,
        )
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
    return parse_records(
        records,
        source_commit,
        image_digest,
        transport,
        live=False,
        expected_nonce=None,
    )


def percentile_95(values: list[int]) -> int:
    if not values:
        return 0
    ordered = sorted(values)
    return ordered[math.ceil(len(ordered) * 0.95) - 1]


def counter_delta(samples: tuple[dict[str, Any], ...], field: str) -> int:
    return samples[-1][field] - samples[0][field] if samples else 0


@dataclass(frozen=True)
class HiddenRepaintBehavior:
    repaint_events: int
    active_intervals: int
    interval_count: int
    observed_ms: int
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
    repaint_events = 0
    longest_active_ms = 0
    longest_quiescent_ms = 0
    active_run_ms = 0
    quiescent_run_ms = 0
    interval_count = 0
    observed_ms = 0
    for previous, current in zip(samples, samples[1:]):
        elapsed_ms = current["elapsed_ms"] - previous["elapsed_ms"]
        if previous["browser_visible"] or current["browser_visible"]:
            active_run_ms = 0
            quiescent_run_ms = 0
            continue
        interval_count += 1
        observed_ms += elapsed_ms
        repaint_delta = current["surface_repaints"] - previous["surface_repaints"]
        if repaint_delta > 0:
            repaint_events += repaint_delta
            active_intervals += 1
            active_run_ms += elapsed_ms
            quiescent_run_ms = 0
            longest_active_ms = max(longest_active_ms, active_run_ms)
        else:
            quiescent_run_ms += elapsed_ms
            active_run_ms = 0
            longest_quiescent_ms = max(longest_quiescent_ms, quiescent_run_ms)
    return HiddenRepaintBehavior(
        repaint_events=repaint_events,
        active_intervals=active_intervals,
        interval_count=interval_count,
        observed_ms=observed_ms,
        longest_active_ms=longest_active_ms,
        longest_quiescent_ms=longest_quiescent_ms,
    )


def longest_stationary_visible_window_ms(
    samples: tuple[dict[str, Any], ...],
) -> int:
    longest = 0
    current_run = 0
    for previous, current in zip(samples, samples[1:]):
        elapsed_ms = current["elapsed_ms"] - previous["elapsed_ms"]
        frame_delta = current["frames_received"] - previous["frames_received"]
        if (
            previous["browser_visible"]
            and current["browser_visible"]
            and previous["visible_tab_id"] == current["visible_tab_id"]
            and previous["pointer_x"] == current["pointer_x"]
            and previous["pointer_y"] == current["pointer_y"]
            and previous["pointer_updates"] == current["pointer_updates"]
            and previous["connection_state"] == "connected"
            and current["connection_state"] == "connected"
            and frame_delta > 0
        ):
            current_run += elapsed_ms
            longest = max(longest, current_run)
        else:
            current_run = 0
    return longest


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
    wall_observation_ms: int
    sample_count: int
    tab_count: int
    tab_source_count: int
    tab_set_sha256: str | None
    tab_source_width: int
    tab_source_height: int
    viewport_width: int
    viewport_height: int
    min_fps: int
    source_fps: int
    cadence_ratio_permille: int
    supported_target_fps: int
    max_stall_ms: int
    pointer_updates: int
    stationary_pointer_continuous_ms: int
    navigation_latency_sample_count: int
    navigation_p95_ms: int
    navigation_max_ms: int
    session_latency_sample_count: int
    session_latency_p95_ms: int
    session_latency_max_ms: int
    full_uploads: int
    partial_uploads: int
    partial_rects: int
    host_process_cpu_sample_count: int
    host_process_cpu_p95_permille: int
    host_process_cpu_max_permille: int
    host_gpu_busy_sample_count: int
    host_gpu_busy_p95_permille: int
    host_gpu_busy_max_permille: int
    hidden_repaints: int
    hidden_repaint_active_intervals: int
    hidden_repaint_interval_count: int
    hidden_observation_ms: int
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
            wall_observation_ms=0,
            sample_count=0,
            tab_count=0,
            tab_source_count=0,
            tab_set_sha256=None,
            tab_source_width=0,
            tab_source_height=0,
            viewport_width=0,
            viewport_height=0,
            min_fps=0,
            source_fps=0,
            cadence_ratio_permille=0,
            supported_target_fps=0,
            max_stall_ms=0,
            pointer_updates=0,
            stationary_pointer_continuous_ms=0,
            navigation_latency_sample_count=0,
            navigation_p95_ms=0,
            navigation_max_ms=0,
            session_latency_sample_count=0,
            session_latency_p95_ms=0,
            session_latency_max_ms=0,
            full_uploads=0,
            partial_uploads=0,
            partial_rects=0,
            host_process_cpu_sample_count=0,
            host_process_cpu_p95_permille=0,
            host_process_cpu_max_permille=0,
            host_gpu_busy_sample_count=0,
            host_gpu_busy_p95_permille=0,
            host_gpu_busy_max_permille=0,
            hidden_repaints=0,
            hidden_repaint_active_intervals=0,
            hidden_repaint_interval_count=0,
            hidden_observation_ms=0,
            hidden_repaint_longest_burst_ms=0,
            hidden_repaint_quiescent_ms=0,
            reconnects=0,
            recovery_observed=False,
            criteria_met=False,
            failures=("samples",),
        )

    endpoint_span_ms = samples[-1]["elapsed_ms"] - samples[0]["elapsed_ms"]
    receipt_span_seconds = observation.receipt_seconds[-1] - observation.receipt_seconds[0]
    wall_observation_ms = max(0, math.floor(receipt_span_seconds * 1000.0))
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
    frame_rates: list[float] = []
    source_rates: list[float] = []
    cadence_ratios: list[float] = []
    supported_target_fps = (
        observation.identity.supported_target_fps
        if observation.identity is not None
        else 0
    )
    identity = observation.identity
    tab_ids = identity.tab_ids if identity is not None else ()
    for previous, current in zip(samples, samples[1:]):
        elapsed = current["elapsed_ms"] - previous["elapsed_ms"]
        for tab_id in tab_ids:
            source_delta = (
                current["tab_source_frames"][tab_id]["frames_presented"]
                - previous["tab_source_frames"][tab_id]["frames_presented"]
            )
            source_rates.append(source_delta * 1000.0 / elapsed)
        if (
            previous["browser_visible"]
            and current["browser_visible"]
            and previous["visible_tab_id"] == current["visible_tab_id"]
            and current["visible_tab_id"] is not None
        ):
            frame_delta = current["frames_received"] - previous["frames_received"]
            visible_tab_id = current["visible_tab_id"]
            source_delta = (
                current["tab_source_frames"][visible_tab_id]["frames_presented"]
                - previous["tab_source_frames"][visible_tab_id]["frames_presented"]
            )
            frame_rate = frame_delta * 1000.0 / elapsed
            visible_source_rate = source_delta * 1000.0 / elapsed
            frame_rates.append(frame_rate)
            expected_rate = min(visible_source_rate, float(supported_target_fps))
            cadence_ratios.append(
                frame_rate / expected_rate if expected_rate > 0 else 0.0
            )
    min_fps = min(MAX_FPS, math.floor(min(frame_rates))) if frame_rates else 0
    source_fps = min(MAX_FPS, math.floor(min(source_rates))) if source_rates else 0
    cadence_ratio_permille = (
        min(1_000, max(0, math.floor(min(cadence_ratios) * 1_000.0)))
        if cadence_ratios
        else 0
    )
    navigation_latencies = [
        latency for sample in samples for latency in sample["navigation_latencies_ms"]
    ]
    session_latencies = [
        latency for sample in samples for latency in sample["session_latencies_ms"]
    ]
    recovery = reconnect_recovered(samples)
    repaint_behavior = hidden_repaint_behavior(samples)
    stationary_window_ms = longest_stationary_visible_window_ms(samples)
    visible_samples = [sample for sample in samples if sample["browser_visible"]]
    visible_full_uploads = sum(
        current["full_uploads"] - previous["full_uploads"]
        for previous, current in zip(samples, samples[1:])
        if previous["browser_visible"] and current["browser_visible"]
    )
    visible_partial_uploads = sum(
        current["partial_uploads"] - previous["partial_uploads"]
        for previous, current in zip(samples, samples[1:])
        if previous["browser_visible"] and current["browser_visible"]
    )
    visible_partial_rects = sum(
        current["partial_rects"] - previous["partial_rects"]
        for previous, current in zip(samples, samples[1:])
        if previous["browser_visible"] and current["browser_visible"]
    )
    host_process_cpu_samples = [
        value
        for sample in samples
        if (value := sample["host_process_cpu_permille"]) is not None
    ]
    host_gpu_busy_samples = [
        value
        for sample in samples
        if (value := sample["host_gpu_busy_permille"]) is not None
    ]
    tab_set_sha256 = (
        "sha256:"
        + hashlib.sha256(
            ("\n".join(sorted(tab_ids)) + "\n").encode("utf-8")
        ).hexdigest()
        if tab_ids
        else None
    )
    metrics = {
        "duration_seconds": duration_seconds,
        "wall_observation_ms": wall_observation_ms,
        "sample_count": len(samples),
        "tab_count": min(sample["tab_count"] for sample in samples),
        "tab_source_count": len(tab_ids),
        "tab_set_sha256": tab_set_sha256,
        "tab_source_width": min(
            (
                sample["tab_source_frames"][tab_id]["width"]
                for sample in samples
                for tab_id in tab_ids
            ),
            default=0,
        ),
        "tab_source_height": min(
            (
                sample["tab_source_frames"][tab_id]["height"]
                for sample in samples
                for tab_id in tab_ids
            ),
            default=0,
        ),
        "viewport_width": min(sample["viewport_width"] for sample in samples),
        "viewport_height": min(sample["viewport_height"] for sample in samples),
        "min_fps": min_fps,
        "source_fps": source_fps,
        "cadence_ratio_permille": cadence_ratio_permille,
        "supported_target_fps": supported_target_fps,
        "max_stall_ms": max(
            (sample["max_frame_gap_ms"] for sample in visible_samples), default=0
        ),
        "pointer_updates": counter_delta(samples, "pointer_updates"),
        "stationary_pointer_continuous_ms": stationary_window_ms,
        "navigation_latency_sample_count": len(navigation_latencies),
        "navigation_p95_ms": percentile_95(navigation_latencies),
        "navigation_max_ms": max(navigation_latencies, default=0),
        "session_latency_sample_count": len(session_latencies),
        "session_latency_p95_ms": percentile_95(session_latencies),
        "session_latency_max_ms": max(session_latencies, default=0),
        "full_uploads": visible_full_uploads,
        "partial_uploads": visible_partial_uploads,
        "partial_rects": visible_partial_rects,
        "host_process_cpu_sample_count": len(host_process_cpu_samples),
        "host_process_cpu_p95_permille": percentile_95(host_process_cpu_samples),
        "host_process_cpu_max_permille": max(host_process_cpu_samples, default=0),
        "host_gpu_busy_sample_count": len(host_gpu_busy_samples),
        "host_gpu_busy_p95_permille": percentile_95(host_gpu_busy_samples),
        "host_gpu_busy_max_permille": max(host_gpu_busy_samples, default=0),
        "hidden_repaints": repaint_behavior.repaint_events,
        "hidden_repaint_active_intervals": repaint_behavior.active_intervals,
        "hidden_repaint_interval_count": repaint_behavior.interval_count,
        "hidden_observation_ms": repaint_behavior.observed_ms,
        "hidden_repaint_longest_burst_ms": repaint_behavior.longest_active_ms,
        "hidden_repaint_quiescent_ms": repaint_behavior.longest_quiescent_ms,
        "reconnects": counter_delta(samples, "reconnects"),
        "recovery_observed": recovery,
    }
    checks = (
        (
            "live_stream_schema_v4",
            observation.identity is not None and observation.identity.live_protocol,
        ),
        (
            "challenge_bound_stream",
            observation.stream_sha256 is not None
            and observation.identity is not None
            and COLLECTION_NONCE_RE.fullmatch(
                observation.identity.collection_nonce or ""
            )
            is not None,
        ),
        (
            "immutable_runtime_identity",
            observation.identity is not None
            and all(
                value is not None
                for value in (
                    observation.identity.host_boot_id,
                    observation.identity.guest_boot_id,
                    observation.identity.domain_uuid,
                    observation.identity.browser_instance_id,
                    observation.identity.workload_instance_id,
                    observation.identity.source_instance_id,
                )
            ),
        ),
        ("endpoint_completed", observation.completion_status == "completed"),
        ("sample_cadence", cadence_observed),
        ("duration_seconds", metrics["duration_seconds"] >= PASS_DURATION_SECONDS),
        ("tab_count", metrics["tab_count"] >= MIN_MEDIA_TAB_COUNT),
        (
            "immutable_tab_set",
            metrics["tab_source_count"] == metrics["tab_count"]
            and metrics["tab_source_count"] >= MIN_MEDIA_TAB_COUNT
            and metrics["tab_set_sha256"] is not None,
        ),
        (
            "viewport",
            metrics["viewport_width"] >= 1920 and metrics["viewport_height"] >= 1080,
        ),
        (
            "five_tab_source_resolution",
            metrics["tab_source_width"] >= 1920
            and metrics["tab_source_height"] >= 1080,
        ),
        ("min_fps", metrics["min_fps"] >= 30),
        ("five_tab_source_fps", metrics["source_fps"] >= 30),
        (
            "source_cadence_ratio",
            metrics["cadence_ratio_permille"] >= MIN_CADENCE_RATIO_PERMILLE,
        ),
        ("max_stall_ms", metrics["max_stall_ms"] <= 500),
        ("pointer_updates", metrics["pointer_updates"] > 0),
        (
            "stationary_pointer_window",
            metrics["stationary_pointer_continuous_ms"]
            >= STATIONARY_POINTER_WINDOW_MS,
        ),
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
            "partial_upload_rects",
            metrics["partial_rects"] >= metrics["partial_uploads"],
        ),
        (
            "host_process_cpu_coverage",
            metrics["host_process_cpu_sample_count"] * 1_000
            >= len(samples) * MIN_HOST_LOAD_COVERAGE_PERMILLE,
        ),
        (
            "host_gpu_busy_coverage",
            metrics["host_gpu_busy_sample_count"] * 1_000
            >= len(samples) * MIN_HOST_LOAD_COVERAGE_PERMILLE,
        ),
        (
            "hidden_observation",
            metrics["hidden_observation_ms"] >= HIDDEN_QUIESCENT_WINDOW_MS,
        ),
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
    observation: Observation,
    assessment: Assessment,
    status: str,
    source_commit: str,
    image_digest: str,
    transport: str,
    observation_started_at: str,
    recorded_at: str,
    *,
    live: bool,
    collection_nonce: str | None,
) -> dict[str, Any]:
    identity = observation.identity
    live_identity = identity if identity is not None and identity.live_protocol else None
    if live_identity is not None and live_identity.collection_nonce != collection_nonce:
        fail("evidence collection_nonce does not match the challenged stream")
    return {
        "schema_version": EVIDENCE_SCHEMA_VERSION,
        "kind": "browser_vm_performance",
        "profile": "browser-vm-chromium",
        "image": "browser-vm-chromium",
        "workload": "browser-vm",
        "status": status,
        "source": "live-browser-vm-acceptance",
        "source_commit": source_commit,
        "image_digest": image_digest,
        "transport": transport,
        "collection_mode": LIVE_COLLECTION_MODE if live else FIXTURE_COLLECTION_MODE,
        "collection_nonce": collection_nonce if live else None,
        "stream_sha256": (
            f"sha256:{observation.stream_sha256}"
            if observation.stream_sha256 is not None
            else None
        ),
        "stream_session_id": identity.session_id if identity is not None else None,
        "shell_metrics_source": (
            SHELL_METRICS_SOURCE if live_identity is not None else None
        ),
        "guest_metrics_source": (
            GUEST_METRICS_SOURCE if live_identity is not None else None
        ),
        "host_boot_id": (
            live_identity.host_boot_id if live_identity is not None else None
        ),
        "guest_boot_id": (
            live_identity.guest_boot_id if live_identity is not None else None
        ),
        "domain_uuid": live_identity.domain_uuid if live_identity is not None else None,
        "browser_instance_id": (
            live_identity.browser_instance_id if live_identity is not None else None
        ),
        "workload_instance_id": (
            live_identity.workload_instance_id if live_identity is not None else None
        ),
        "source_instance_id": (
            live_identity.source_instance_id if live_identity is not None else None
        ),
        "observation_started_at": observation_started_at,
        "wall_observation_ms": assessment.wall_observation_ms,
        "sample_count": assessment.sample_count,
        "duration_seconds": assessment.duration_seconds,
        "tab_count": assessment.tab_count,
        "tab_source_count": assessment.tab_source_count,
        "tab_set_sha256": assessment.tab_set_sha256,
        "tab_source_width": assessment.tab_source_width,
        "tab_source_height": assessment.tab_source_height,
        "viewport_width": assessment.viewport_width,
        "viewport_height": assessment.viewport_height,
        "min_fps": assessment.min_fps,
        "source_fps": assessment.source_fps,
        "cadence_ratio_permille": assessment.cadence_ratio_permille,
        "supported_target_fps": assessment.supported_target_fps,
        "max_stall_ms": assessment.max_stall_ms,
        "pointer_updates": assessment.pointer_updates,
        "stationary_pointer_continuous_ms": assessment.stationary_pointer_continuous_ms,
        "navigation_latency_sample_count": assessment.navigation_latency_sample_count,
        "navigation_p95_ms": assessment.navigation_p95_ms,
        "navigation_max_ms": assessment.navigation_max_ms,
        "session_latency_sample_count": assessment.session_latency_sample_count,
        "session_latency_p95_ms": assessment.session_latency_p95_ms,
        "session_latency_max_ms": assessment.session_latency_max_ms,
        "full_uploads": assessment.full_uploads,
        "partial_uploads": assessment.partial_uploads,
        "partial_rects": assessment.partial_rects,
        "host_process_cpu_sample_count": assessment.host_process_cpu_sample_count,
        "host_process_cpu_p95_permille": assessment.host_process_cpu_p95_permille,
        "host_process_cpu_max_permille": assessment.host_process_cpu_max_permille,
        "host_gpu_busy_sample_count": assessment.host_gpu_busy_sample_count,
        "host_gpu_busy_p95_permille": assessment.host_gpu_busy_p95_permille,
        "host_gpu_busy_max_permille": assessment.host_gpu_busy_max_permille,
        "hidden_repaints": assessment.hidden_repaints,
        "hidden_repaint_active_intervals": assessment.hidden_repaint_active_intervals,
        "hidden_repaint_interval_count": assessment.hidden_repaint_interval_count,
        "hidden_observation_ms": assessment.hidden_observation_ms,
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
        ("MIN_CADENCE_RATIO_PERMILLE", MIN_CADENCE_RATIO_PERMILLE),
        ("STATIONARY_POINTER_WINDOW_MS", STATIONARY_POINTER_WINDOW_MS),
        ("MIN_MEDIA_TAB_COUNT", MIN_MEDIA_TAB_COUNT),
        ("MAX_HOST_LOAD_PERMILLE", MAX_HOST_LOAD_PERMILLE),
        (
            "MIN_HOST_LOAD_COVERAGE_PERMILLE",
            MIN_HOST_LOAD_COVERAGE_PERMILLE,
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
    return Observation((), (), status, None, None)


def utc_now() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).strftime("%Y-%m-%dT%H:%M:%SZ")


def collect(args: argparse.Namespace) -> int:
    validate_source_binding(args.source_commit, args.image_digest)
    validate_endpoint(args.endpoint)
    prepare_output(args.out)
    collection_nonce = secrets.token_hex(32)
    observation_started_at = utc_now()
    diagnostic: str | None = None
    try:
        observation = consume_endpoint(
            args.endpoint,
            args.source_commit,
            args.image_digest,
            args.transport,
            args.read_timeout_seconds,
            collection_nonce,
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
        observation,
        assessment,
        status,
        args.source_commit,
        args.image_digest,
        args.transport,
        observation_started_at,
        utc_now(),
        live=True,
        collection_nonce=collection_nonce,
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
        observation,
        assessment,
        status,
        args.source_commit,
        args.image_digest,
        args.transport,
        DRY_RUN_STARTED_AT,
        DRY_RUN_RECORDED_AT,
        live=False,
        collection_nonce=None,
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
    collection_nonce = "d" * 64
    legacy_fixture_transport = "sunshine"
    tab_ids = tuple(f"cdp-tab-{index}" for index in range(1, 6))

    def make_live_records(
        transport: str = DEFAULT_TRANSPORT,
    ) -> list[tuple[dict[str, Any], float]]:
        records: list[tuple[dict[str, Any], float]] = []
        receipt = 0.0
        for index, raw in enumerate(
            (fixture_root / "passing.ndjson").read_bytes().splitlines(), start=1
        ):
            record = decode_record(raw, index)
            if index == 1:
                record.update(
                    {
                        "schema_version": LIVE_STREAM_SCHEMA_VERSION,
                        "collection_nonce": collection_nonce,
                        "shell_metrics_source": SHELL_METRICS_SOURCE,
                        "guest_metrics_source": GUEST_METRICS_SOURCE,
                        "host_boot_id": "11111111-1111-4111-8111-111111111111",
                        "guest_boot_id": "22222222-2222-4222-8222-222222222222",
                        "domain_uuid": "33333333-3333-4333-8333-333333333333",
                        "browser_instance_id": "44444444-4444-4444-8444-444444444444",
                        "workload_instance_id": "55555555-5555-4555-8555-555555555555",
                        "source_instance_id": "66666666-6666-4666-8666-666666666666",
                        "tab_ids": list(tab_ids),
                        "supported_target_fps": 30,
                        "session_id": "browser-vm-a1100",
                        "transport": transport,
                    }
                )
            elif record.get("type") == "sample":
                elapsed_ms = record["elapsed_ms"]
                receipt = elapsed_ms / 1000.0
                browser_visible = elapsed_ms < 600_000
                partial_uploads = record.pop("partial_uploads")
                surface_repaints = record.pop("hidden_repaints")
                if elapsed_ms <= STATIONARY_POINTER_WINDOW_MS:
                    pointer_x = 960
                    pointer_updates = 0
                elif browser_visible:
                    pointer_x = 960 + (elapsed_ms - STATIONARY_POINTER_WINDOW_MS) // 5_000
                    pointer_updates = (
                        elapsed_ms - STATIONARY_POINTER_WINDOW_MS
                    ) // 5_000
                else:
                    pointer_x = 1_019
                    pointer_updates = 59
                record.update(
                    {
                        "tab_source_frames": {
                            tab_id: {
                                "frames_presented": record["frames_received"],
                                "width": 1920,
                                "height": 1080,
                            }
                            for tab_id in tab_ids
                        },
                        "visible_tab_id": tab_ids[0] if browser_visible else None,
                        "browser_visible": browser_visible,
                        "pointer_x": pointer_x,
                        "pointer_y": 540,
                        "pointer_updates": pointer_updates,
                        "full_uploads": 0 if elapsed_ms == 0 else 1,
                        "partial_uploads": partial_uploads,
                        "partial_rects": partial_uploads * 2,
                        "surface_repaints": surface_repaints,
                        "host_process_cpu_permille": (
                            None if elapsed_ms == 0 else 12_000 + elapsed_ms // 5_000
                        ),
                        "host_gpu_busy_permille": (
                            None if elapsed_ms == 0 else 24_000 + elapsed_ms // 5_000
                        ),
                    }
                )
            records.append((record, receipt))
        terminal = records[-1][0]
        if terminal.get("type") != "complete":
            raise AssertionError("passing fixture lost its terminal complete record")
        terminal.update(
            {
                "collection_nonce": collection_nonce,
                "sample_count": sum(
                    1 for record, _receipt in records if record.get("type") == "sample"
                ),
                "stream_sha256": f"sha256:{canonical_stream_sha256(records[:-1])}",
            }
        )
        return records

    def make_live_observation(
        transport: str = DEFAULT_TRANSPORT,
    ) -> Observation:
        return parse_records(
            make_live_records(transport),
            source_commit,
            image_digest,
            transport,
            live=True,
            expected_nonce=collection_nonce,
        )

    fixture = read_fixture(
        fixture_root / "passing.ndjson",
        source_commit,
        image_digest,
        legacy_fixture_transport,
    )
    fixture_assessment = assess(fixture)
    assert not fixture_assessment.criteria_met
    assert "live_stream_schema_v4" in fixture_assessment.failures
    assert "stationary_pointer_window" in fixture_assessment.failures
    assert evidence_status(fixture, fixture_assessment, live=False) == "unavailable"

    passing = make_live_observation()
    assessment = assess(passing)
    assert assessment.criteria_met, assessment.failures
    assert assessment.duration_seconds == 900
    assert assessment.wall_observation_ms == 900_000
    assert assessment.sample_count == 181
    assert assessment.tab_count == 5
    assert assessment.tab_source_count == 5
    assert assessment.tab_set_sha256 is not None
    assert assessment.tab_source_width == 1920
    assert assessment.tab_source_height == 1080
    assert assessment.viewport_width == 1920
    assert assessment.viewport_height == 1080
    assert assessment.min_fps == 30
    assert assessment.source_fps == 30
    assert assessment.cadence_ratio_permille == 1_000
    assert assessment.max_stall_ms == 34
    assert assessment.pointer_updates == 59
    assert assessment.stationary_pointer_continuous_ms == 300_000
    assert assessment.full_uploads == 1
    assert assessment.partial_uploads == 119
    assert assessment.partial_rects == 238
    assert assessment.host_process_cpu_sample_count == 180
    assert (
        assessment.host_process_cpu_p95_permille
        <= assessment.host_process_cpu_max_permille
    )
    assert assessment.host_gpu_busy_sample_count == 180
    assert (
        assessment.host_gpu_busy_p95_permille
        <= assessment.host_gpu_busy_max_permille
    )
    assert assessment.navigation_latency_sample_count >= MIN_NAVIGATION_LATENCY_SAMPLES
    assert assessment.navigation_p95_ms <= LATENCY_P95_LIMIT_MS
    assert assessment.navigation_max_ms <= LATENCY_MAX_LIMIT_MS
    assert assessment.session_latency_sample_count >= MIN_SESSION_LATENCY_SAMPLES
    assert assessment.session_latency_p95_ms <= LATENCY_P95_LIMIT_MS
    assert assessment.session_latency_max_ms <= LATENCY_MAX_LIMIT_MS
    assert assessment.hidden_repaints == 0
    assert assessment.hidden_repaint_active_intervals == 0
    assert assessment.hidden_repaint_interval_count == 60
    assert assessment.hidden_observation_ms == HIDDEN_QUIESCENT_WINDOW_MS
    assert assessment.hidden_repaint_longest_burst_ms == 0
    assert assessment.hidden_repaint_quiescent_ms == HIDDEN_QUIESCENT_WINDOW_MS
    assert assessment.reconnects == 1
    assert assessment.recovery_observed

    fixture_status = evidence_status(fixture, fixture_assessment, live=False)
    fixture_evidence = make_evidence(
        fixture,
        fixture_assessment,
        fixture_status,
        source_commit,
        image_digest,
        legacy_fixture_transport,
        DRY_RUN_STARTED_AT,
        DRY_RUN_RECORDED_AT,
        live=False,
        collection_nonce=None,
    )
    validate_evidence(fixture_evidence)
    assert fixture_evidence["collection_mode"] == FIXTURE_COLLECTION_MODE
    assert fixture_evidence["collection_nonce"] is None

    self_test_recorded = datetime.now(timezone.utc).replace(microsecond=0)
    self_test_started = self_test_recorded - timedelta(seconds=PASS_DURATION_SECONDS)
    live_rdp_evidence = make_evidence(
        passing,
        assessment,
        "passed",
        source_commit,
        image_digest,
        DEFAULT_TRANSPORT,
        self_test_started.strftime("%Y-%m-%dT%H:%M:%SZ"),
        self_test_recorded.strftime("%Y-%m-%dT%H:%M:%SZ"),
        live=True,
        collection_nonce=collection_nonce,
    )
    rdp_result = validate_evidence(live_rdp_evidence)
    assert rdp_result["status"] == "validated"
    assert rdp_result["transport"] == DEFAULT_TRANSPORT
    assert rdp_result["sample_count"] == 181
    assert rdp_result["tab_source_count"] == 5
    assert rdp_result["cadence_ratio_permille"] == 1_000
    assert rdp_result["partial_uploads"] == assessment.partial_uploads
    assert rdp_result["host_process_cpu_sample_count"] == 180
    assert rdp_result["host_gpu_busy_sample_count"] == 180
    assert (
        rdp_result["navigation_latency_sample_count"]
        == assessment.navigation_latency_sample_count
    )
    assert rdp_result["navigation_max_ms"] == assessment.navigation_max_ms
    assert (
        rdp_result["hidden_repaint_quiescent_ms"]
        == assessment.hidden_repaint_quiescent_ms
    )

    sunshine_observation = make_live_observation("sunshine")
    sunshine_assessment = assess(sunshine_observation)
    sunshine_evidence = make_evidence(
        sunshine_observation,
        sunshine_assessment,
        "passed",
        source_commit,
        image_digest,
        "sunshine",
        self_test_started.strftime("%Y-%m-%dT%H:%M:%SZ"),
        self_test_recorded.strftime("%Y-%m-%dT%H:%M:%SZ"),
        live=True,
        collection_nonce=collection_nonce,
    )
    sunshine_result = validate_evidence(sunshine_evidence)
    assert sunshine_result["status"] == "validated"
    assert sunshine_result["transport"] == "sunshine"

    assert_raises(
        lambda: validate_evidence(dict(live_rdp_evidence, transport="spice")),
        "unsupported Browser VM transport",
    )
    assert_raises(
        lambda: validate_evidence(
            dict(live_rdp_evidence, schema_version=EVIDENCE_SCHEMA_VERSION - 1)
        ),
        "schema_version",
    )
    assert_raises(
        lambda: validate_evidence(
            dict(live_rdp_evidence, navigation_max_ms=LATENCY_MAX_LIMIT_MS + 1)
        ),
        "navigation_max_ms",
    )

    rdp_header = make_live_records("rdp")[0][0]
    validate_header(
        rdp_header,
        source_commit,
        image_digest,
        "rdp",
        live=True,
        expected_nonce=collection_nonce,
    )
    fixture_header = decode_record(
        (fixture_root / "passing.ndjson").read_bytes().splitlines()[0], 1
    )
    validate_header(
        fixture_header,
        source_commit,
        image_digest,
        legacy_fixture_transport,
        live=False,
        expected_nonce=None,
    )
    assert_raises(
        lambda: parse_records(
            make_live_records(),
            source_commit,
            image_digest,
            DEFAULT_TRANSPORT,
            live=True,
            expected_nonce="e" * 64,
        ),
        "fresh collector challenge",
    )
    wrong_completion_nonce = make_live_records()
    wrong_completion_nonce[-1][0]["collection_nonce"] = "e" * 64
    assert_raises(
        lambda: parse_records(
            wrong_completion_nonce,
            source_commit,
            image_digest,
            DEFAULT_TRANSPORT,
            live=True,
            expected_nonce=collection_nonce,
        ),
        "does not echo the fresh collector challenge",
    )
    forged_digest = make_live_records()
    forged_digest[-1][0]["stream_sha256"] = "sha256:" + "0" * 64
    assert_raises(
        lambda: parse_records(
            forged_digest,
            source_commit,
            image_digest,
            DEFAULT_TRANSPORT,
            live=True,
            expected_nonce=collection_nonce,
        ),
        "does not bind the exact stream",
    )
    wrong_sample_count = make_live_records()
    wrong_sample_count[-1][0]["sample_count"] -= 1
    assert_raises(
        lambda: parse_records(
            wrong_sample_count,
            source_commit,
            image_digest,
            DEFAULT_TRANSPORT,
            live=True,
            expected_nonce=collection_nonce,
        ),
        "sample_count does not match",
    )
    spliced_identity = make_live_records()
    spliced_identity[0][0]["source_instance_id"] = (
        "77777777-7777-4777-8777-777777777777"
    )
    assert_raises(
        lambda: parse_records(
            spliced_identity,
            source_commit,
            image_digest,
            DEFAULT_TRANSPORT,
            live=True,
            expected_nonce=collection_nonce,
        ),
        "does not bind the exact stream",
    )
    incomplete_tab_sample = make_live_records()
    del incomplete_tab_sample[1][0]["tab_source_frames"][tab_ids[-1]]
    assert_raises(
        lambda: parse_records(
            incomplete_tab_sample,
            source_commit,
            image_digest,
            DEFAULT_TRANSPORT,
            live=True,
            expected_nonce=collection_nonce,
        ),
        "misses tab identities",
    )
    impossible_partial_rects = make_live_records()
    impossible_partial_rects[1][0]["partial_uploads"] = 1
    assert_raises(
        lambda: parse_records(
            impossible_partial_rects,
            source_commit,
            image_digest,
            DEFAULT_TRANSPORT,
            live=True,
            expected_nonce=collection_nonce,
        ),
        "partial_rects cannot be less than partial_uploads",
    )
    invalid_host_load = make_live_records()
    invalid_host_load[1][0]["host_gpu_busy_permille"] = (
        MAX_HOST_LOAD_PERMILLE + 1
    )
    assert_raises(
        lambda: parse_records(
            invalid_host_load,
            source_commit,
            image_digest,
            DEFAULT_TRANSPORT,
            live=True,
            expected_nonce=collection_nonce,
        ),
        "host_gpu_busy_permille",
    )
    stale_latency_sample = make_live_records()
    stale_latency_sample[1][0]["navigation_latencies_ms"] = [1]
    assert_raises(
        lambda: parse_records(
            stale_latency_sample,
            source_commit,
            image_digest,
            DEFAULT_TRANSPORT,
            live=True,
            expected_nonce=collection_nonce,
        ),
        "pre-challenge latency events",
    )
    assert_raises(
        lambda: parse_records(
            make_live_records()[:-1],
            source_commit,
            image_digest,
            DEFAULT_TRANSPORT,
            live=True,
            expected_nonce=collection_nonce,
        ),
        "terminal complete marker",
    )
    rapid_records = [
        (record, 0.0) for record, _receipt in make_live_records()
    ]
    rapid_observation = parse_records(
        rapid_records,
        source_commit,
        image_digest,
        DEFAULT_TRANSPORT,
        live=True,
        expected_nonce=collection_nonce,
    )
    rapid_assessment = assess(rapid_observation)
    assert rapid_assessment.duration_seconds == 0
    assert "duration_seconds" in rapid_assessment.failures
    assert_raises(
        lambda: parse_records(
            [
                (fixture_header, 0.0),
                ({"type": "complete", "status": "completed"}, 0.0),
            ],
            source_commit,
            image_digest,
            DEFAULT_TRANSPORT,
            live=True,
            expected_nonce=collection_nonce,
        ),
        "stream header",
    )

    unavailable = read_fixture(
        fixture_root / "unavailable.ndjson",
        source_commit,
        image_digest,
        legacy_fixture_transport,
    )
    unavailable_assessment = assess(unavailable)
    assert evidence_status(unavailable, unavailable_assessment, live=False) == "unavailable"
    assert not unavailable_assessment.criteria_met

    failed = read_fixture(
        fixture_root / "failed.ndjson",
        source_commit,
        image_digest,
        legacy_fixture_transport,
    )
    failed_assessment = assess(failed)
    failed_status = evidence_status(failed, failed_assessment, live=False)
    assert failed_status == "failed"
    failed_evidence = make_evidence(
        failed,
        failed_assessment,
        failed_status,
        source_commit,
        image_digest,
        legacy_fixture_transport,
        DRY_RUN_STARTED_AT,
        DRY_RUN_RECORDED_AT,
        live=False,
        collection_nonce=None,
    )
    validate_evidence(failed_evidence)

    def with_samples(samples: list[dict[str, Any]]) -> Observation:
        return Observation(
            tuple(samples),
            passing.receipt_seconds,
            passing.completion_status,
            passing.identity,
            passing.stream_sha256,
        )

    def copied_samples() -> list[dict[str, Any]]:
        return [
            dict(
                sample,
                tab_source_frames={
                    tab_id: dict(source)
                    for tab_id, source in sample["tab_source_frames"].items()
                },
            )
            for sample in passing.samples
        ]

    degraded_samples = copied_samples()
    degraded_samples[len(degraded_samples) // 2]["tab_count"] = 4
    degraded_assessment = assess(with_samples(degraded_samples))
    assert "tab_count" in degraded_assessment.failures

    high_p95_samples = copied_samples()
    for sample in high_p95_samples:
        if sample["navigation_latencies_ms"]:
            sample["navigation_latencies_ms"] = [LATENCY_P95_LIMIT_MS + 1]
    assert "navigation_p95_ms" in assess(with_samples(high_p95_samples)).failures

    high_session_p95_samples = copied_samples()
    for sample in high_session_p95_samples:
        if sample["session_latencies_ms"]:
            sample["session_latencies_ms"] = [LATENCY_P95_LIMIT_MS + 1]
    assert (
        "session_latency_p95_ms"
        in assess(with_samples(high_session_p95_samples)).failures
    )

    high_max_samples = copied_samples()
    high_max_samples[-1]["session_latencies_ms"] = [LATENCY_MAX_LIMIT_MS + 1]
    assert "session_latency_max_ms" in assess(with_samples(high_max_samples)).failures

    sparse_latency_samples = copied_samples()
    retained = False
    for sample in sparse_latency_samples:
        if sample["navigation_latencies_ms"] and not retained:
            retained = True
        else:
            sample["navigation_latencies_ms"] = []
    assert (
        "navigation_latency_coverage"
        in assess(with_samples(sparse_latency_samples)).failures
    )

    cadence_drop_samples = copied_samples()
    cadence_drop_samples[10]["frames_received"] -= 50
    cadence_drop = assess(with_samples(cadence_drop_samples))
    assert "min_fps" in cadence_drop.failures
    assert "source_cadence_ratio" in cadence_drop.failures

    slow_fifth_tab_samples = copied_samples()
    for sample in slow_fifth_tab_samples:
        sample["tab_source_frames"][tab_ids[-1]]["frames_presented"] = (
            sample["elapsed_ms"] * 29 // 1_000
        )
    assert (
        "five_tab_source_fps"
        in assess(with_samples(slow_fifth_tab_samples)).failures
    )

    low_resolution_tab_samples = copied_samples()
    for sample in low_resolution_tab_samples:
        sample["tab_source_frames"][tab_ids[-1]]["height"] = 1_079
    assert (
        "five_tab_source_resolution"
        in assess(with_samples(low_resolution_tab_samples)).failures
    )

    moving_pointer_samples = copied_samples()
    for index, sample in enumerate(moving_pointer_samples):
        if sample["browser_visible"]:
            sample["pointer_x"] = 900 + index % 2
    assert (
        "stationary_pointer_window"
        in assess(with_samples(moving_pointer_samples)).failures
    )

    pointer_event_samples = copied_samples()
    for index, sample in enumerate(pointer_event_samples):
        if sample["elapsed_ms"] <= STATIONARY_POINTER_WINDOW_MS:
            sample["pointer_updates"] = index
        else:
            sample["pointer_updates"] += 60
    assert (
        "stationary_pointer_window"
        in assess(with_samples(pointer_event_samples)).failures
    )

    no_partial_upload_samples = copied_samples()
    for sample in no_partial_upload_samples:
        sample["partial_uploads"] = 0
        sample["partial_rects"] = 0
    assert (
        "partial_uploads"
        in assess(with_samples(no_partial_upload_samples)).failures
    )

    hidden_only_partial_samples = copied_samples()
    hidden_partial = 0
    for sample in hidden_only_partial_samples:
        if sample["browser_visible"]:
            sample["partial_uploads"] = 0
            sample["partial_rects"] = 0
        else:
            hidden_partial += 1
            sample["partial_uploads"] = hidden_partial
            sample["partial_rects"] = hidden_partial
    assert (
        "partial_uploads"
        in assess(with_samples(hidden_only_partial_samples)).failures
    )

    missing_cpu_samples = copied_samples()
    for sample in missing_cpu_samples:
        sample["host_process_cpu_permille"] = None
    assert (
        "host_process_cpu_coverage"
        in assess(with_samples(missing_cpu_samples)).failures
    )

    sparse_gpu_samples = copied_samples()
    for index, sample in enumerate(sparse_gpu_samples):
        if index % 2:
            sample["host_gpu_busy_permille"] = None
    assert (
        "host_gpu_busy_coverage"
        in assess(with_samples(sparse_gpu_samples)).failures
    )

    stalled_samples = copied_samples()
    stalled_samples[10]["max_frame_gap_ms"] = 501
    assert "max_stall_ms" in assess(with_samples(stalled_samples)).failures

    no_reconnect_samples = copied_samples()
    for sample in no_reconnect_samples:
        sample["reconnects"] = 0
        sample["connection_state"] = "connected"
    no_reconnect = assess(with_samples(no_reconnect_samples))
    assert "reconnects" in no_reconnect.failures
    assert "recovery_observed" in no_reconnect.failures

    continuous_repaint_samples = copied_samples()
    repaint_counter = continuous_repaint_samples[0]["surface_repaints"]
    for sample in continuous_repaint_samples:
        if not sample["browser_visible"]:
            repaint_counter += 1
        sample["surface_repaints"] = repaint_counter
    continuous_repaint = assess(with_samples(continuous_repaint_samples))
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
            legacy_fixture_transport,
        ),
        "source_commit does not match",
    )
    for unsafe in (
        "file:///tmp/fixture",
        "http://user:password@127.0.0.1:9000/session",
        "http://127.0.0.1:9000/session?token=no",
        "http://192.0.2.1:9000/session",
    ):
        assert_raises(lambda unsafe=unsafe: validate_endpoint(unsafe), "endpoint")
    assert validate_endpoint("https://performance.example.test/session")

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
        help="rdp (default) or explicit sunshine (Sunshine/Moonlight)",
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
