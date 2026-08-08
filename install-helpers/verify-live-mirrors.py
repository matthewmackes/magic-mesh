#!/usr/bin/env python3
"""Read-only provenance and freshness checks for live Bus mirrors.

This helper is a proof aid, not a probe.  It never contacts a gateway or feed,
opens the Bus SQLite index read-only, and reads only the exact JSON envelope
selected by the index.  A passing result therefore proves the local mirror's
contents and age, not that an external service is reachable right now.

Examples::

    verify-live-mirrors.py --vehicle-node Basement-Test-Workstation \
        --require-online --expected-model MG90
    verify-live-mirrors.py --overlay state/overlay/usgs-earthquakes/workstation \
        --require-ready --require-catalog-feed --max-age-seconds 300
    verify-live-mirrors.py --vehicle-node workstation --overlay \
        state/overlay/usgs-earthquakes/workstation --require-online \
        --require-same-host
    verify-live-mirrors.py --catalog-overlay-node workstation \
        --require-catalog-complete --require-ready --max-age-seconds 300
    verify-live-mirrors.py --catalog-overlay-node workstation \
        --require-installed-overlay-catalog --max-age-seconds 300
    verify-live-mirrors.py --airspace-node workstation --require-airspace-ready
    verify-live-mirrors.py --vdi-console-session vdi-1-win11 \
        --require-vdi-brokered --expected-vdi-protocol spice \
        --require-vdi-status-rail-live
"""

from __future__ import annotations

import argparse
from contextlib import redirect_stdout
import hashlib
from io import StringIO
import json
import math
import os
import sqlite3
import stat
import sys
import tempfile
import time
from pathlib import Path
from typing import Any
from urllib.parse import quote


DEFAULT_BUS_ROOT = Path("/run/mde-bus")
OVERLAY_PREFIX = "state/overlay/"
AIRSPACE_PREFIX = "state/airspace/"
VEHICLE_PREFIX = "state/vehicle/"
VDI_SESSION_TOPIC = "action/vdi/session"
MAX_TOPIC_SCAN_ROWS = 256
ALLOWED_OVERLAY_LICENSE_TIERS = frozenset(
    {
        # Locked zero-cost overlay catalog tiers. Keep this list in sync with the
        # `LICENSE_TIER` constants under crates/mesh/mackes-mesh-types/src/*.
        "free-key-gov",
        "open-data-attribution",
        "public-data-attribution",
        "public-domain",
        "public-domain-courtesy-attribution",
        "us-government-public-domain",
    }
)
DISALLOWED_LICENSE_TIER_HINTS = (
    "non-commercial",
    "noncommercial",
    "paid",
    "personal",
    "trial",
    "educational",
    "education",
    "research-only",
    "evaluation",
)
CATALOG_OVERLAYS: dict[str, dict[str, str]] = {
    # WL-FUNC-012 locked zero-cost live overlay catalog. Keep this list in sync
    # with the `*_STATE_PREFIX`, `LICENSE_TIER`, and `ATTRIBUTION` constants in
    # crates/mesh/mackes-mesh-types/src/*.rs.
    "adsb-aircraft": {
        "license_tier": "open-data-attribution",
        "attribution_contains": "ODbL",
    },
    "airnow-aqi": {
        "license_tier": "free-key-gov",
        "attribution_contains": "AirNow",
    },
    "caltrans-cameras": {
        "license_tier": "public-data-attribution",
        "attribution_contains": "Caltrans",
    },
    "firms-hotspots": {
        "license_tier": "free-key-gov",
        "attribution_contains": "NASA FIRMS",
    },
    "gtfs-transit": {
        "license_tier": "open-data-attribution",
        "attribution_contains": "MassDOT",
    },
    "iem-nexrad": {
        "license_tier": "public-domain-courtesy-attribution",
        "attribution_contains": "NEXRAD",
    },
    "ncdot-traffic": {
        "license_tier": "open-data-attribution",
        "attribution_contains": "NCDOT",
    },
    "nifc-wildfire": {
        "license_tier": "public-domain",
        "attribution_contains": "NIFC",
    },
    "nws-alerts": {
        "license_tier": "public-domain",
        "attribution_contains": "NWS",
    },
    "nws-hourly": {
        "license_tier": "us-government-public-domain",
        "attribution_contains": "National Weather Service",
    },
    "usgs-earthquakes": {
        "license_tier": "public-domain",
        "attribution_contains": "USGS",
    },
}


class MirrorError(Exception):
    """A fail-closed mirror validation error."""


def _now_ms() -> int:
    return int(time.time() * 1000)


def _finite_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def _validate_topic(topic: str) -> None:
    if not topic or topic.startswith("/") or ".." in Path(topic).parts:
        raise MirrorError(f"invalid topic: {topic!r}")


def _open_bus_root(bus_root: Path) -> tuple[Path, Path]:
    root = bus_root.resolve(strict=True)
    db_path = root / "index.sqlite"
    if not db_path.is_file():
        raise MirrorError(f"Bus index missing: {db_path}")
    return root, db_path


def _read_indexed_row(
    root: Path, topic: str, row: tuple[Any, ...]
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any], str]:
    """Return (index row, envelope, payload, sha256) for one indexed row."""
    ulid, indexed_topic, ts_unix_ms, file_path, body = row
    if not isinstance(file_path, str):
        raise MirrorError(f"indexed file path is not a relative path: {file_path!r}")
    relative_path = Path(file_path)
    if (
        relative_path.is_absolute()
        or not relative_path.parts
        or any(part in {"", ".", ".."} for part in relative_path.parts)
        or any("\x00" in part for part in relative_path.parts)
    ):
        raise MirrorError(f"indexed file path is not relative: {file_path!r}")
    message_path = root.joinpath(*relative_path.parts)
    current = root
    for index, part in enumerate(relative_path.parts):
        current /= part
        try:
            metadata = current.lstat()
        except OSError as exc:
            raise MirrorError(f"indexed message file is missing: {file_path!r}") from exc
        if stat.S_ISLNK(metadata.st_mode):
            raise MirrorError(f"indexed message path contains a symlink: {file_path!r}")
        if index < len(relative_path.parts) - 1 and not stat.S_ISDIR(metadata.st_mode):
            raise MirrorError(f"indexed message parent is not a directory: {file_path!r}")
    if not message_path.is_file():
        raise MirrorError(f"indexed message file is not a regular file: {file_path!r}")
    fd: int | None = None
    try:
        open_flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
        fd = os.open(message_path, open_flags)
        with os.fdopen(fd, "rb") as handle:
            fd = None
            if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
                raise MirrorError(f"indexed message file is not a regular file: {file_path!r}")
            raw = handle.read()
        envelope = json.loads(raw)
    except MirrorError:
        raise
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise MirrorError(f"cannot read indexed message {file_path}: {exc}") from exc
    finally:
        if fd is not None:
            os.close(fd)
    if not isinstance(envelope, dict):
        raise MirrorError(f"indexed message {file_path} is not a JSON object")
    if envelope.get("ulid") != ulid or envelope.get("topic") != indexed_topic:
        raise MirrorError(f"index/envelope identity mismatch for {topic}")
    if envelope.get("file_path") != file_path:
        raise MirrorError(f"envelope file_path mismatch for {topic}")
    if envelope.get("ts_unix_ms") != ts_unix_ms or envelope.get("body") != body:
        raise MirrorError(f"index/envelope payload mismatch for {topic}")
    if indexed_topic != topic:
        raise MirrorError(f"index returned unexpected topic {indexed_topic!r}")
    if not isinstance(body, str):
        raise MirrorError(f"indexed mirror body is not JSON text for {topic}")
    try:
        payload = json.loads(body)
    except json.JSONDecodeError as exc:
        raise MirrorError(f"mirror body is not valid JSON for {topic}: {exc}") from exc
    if not isinstance(payload, dict):
        raise MirrorError(f"mirror body is not a JSON object for {topic}")
    return (
        {
            "ulid": ulid,
            "topic": indexed_topic,
            "ts_unix_ms": ts_unix_ms,
            "file_path": file_path,
        },
        envelope,
        payload,
        hashlib.sha256(raw).hexdigest(),
    )


def _read_topic_rows(
    bus_root: Path, topic: str, limit: int = 1
) -> list[tuple[dict[str, Any], dict[str, Any], dict[str, Any], str]]:
    """Return newest indexed rows for one Bus topic, validating every envelope."""
    _validate_topic(topic)
    if limit <= 0 or limit > MAX_TOPIC_SCAN_ROWS:
        raise MirrorError(f"invalid scan limit {limit}; max {MAX_TOPIC_SCAN_ROWS}")
    root, db_path = _open_bus_root(bus_root)
    # URI mode=ro is intentional: this helper must not initialize, migrate, or
    # otherwise mutate a live Bus while producing evidence.
    uri = f"file:{quote(str(db_path), safe='/')}?mode=ro"
    try:
        with sqlite3.connect(uri, uri=True, timeout=5.0) as conn:
            rows = conn.execute(
                "SELECT ulid, topic, ts_unix_ms, file_path, body "
                "FROM messages WHERE topic = ? ORDER BY ulid DESC LIMIT ?",
                (topic, limit),
            ).fetchall()
    except sqlite3.Error as exc:
        raise MirrorError(f"read-only Bus index query failed: {exc}") from exc
    return [_read_indexed_row(root, topic, row) for row in rows]


def _read_latest(bus_root: Path, topic: str) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any], str]:
    """Return (index row, envelope, payload, sha256) for the indexed latest row."""
    rows = _read_topic_rows(bus_root, topic, 1)
    if not rows:
        raise MirrorError(f"no indexed message for {topic}")
    return rows[0]


def _age_check(payload: dict[str, Any], now_ms: int, max_age_ms: int, *, required: str) -> tuple[int, list[str]]:
    errors: list[str] = []
    stamp = payload.get(required)
    if not isinstance(stamp, int) or isinstance(stamp, bool):
        return 0, [f"missing integer {required}"]
    age = now_ms - stamp
    if age < 0:
        errors.append(f"{required} is in the future by {-age} ms")
    elif max_age_ms >= 0 and age > max_age_ms:
        errors.append(f"{required} is stale by {age} ms (limit {max_age_ms} ms)")
    return max(age, 0), errors


def _base_result(topic: str, row: dict[str, Any], payload: dict[str, Any], digest: str) -> dict[str, Any]:
    return {
        "topic": topic,
        "ulid": row["ulid"],
        "bus_ts_unix_ms": row["ts_unix_ms"],
        "source_file": row["file_path"],
        "envelope_sha256": digest,
        "host": payload.get("host"),
    }


def validate_vehicle(
    bus_root: Path,
    node: str,
    now_ms: int,
    max_age_ms: int,
    require_online: bool,
    require_fix: bool,
    expected_model: str | None,
) -> dict[str, Any]:
    topic = f"{VEHICLE_PREFIX}{node}"
    row, _envelope, payload, digest = _read_latest(bus_root, topic)
    result = _base_result(topic, row, payload, digest)
    errors: list[str] = []
    if payload.get("host") != node:
        errors.append(f"payload host {payload.get('host')!r} does not match node {node!r}")
    age_ms, age_errors = _age_check(payload, now_ms, max_age_ms, required="published_at_ms")
    errors.extend(age_errors)
    gps = payload.get("gps")
    telem = payload.get("telem")
    if not isinstance(gps, dict):
        errors.append("gps object missing")
        gps = {}
    if not isinstance(telem, dict):
        errors.append("telem object missing")
        telem = {}
    online = payload.get("online")
    if not isinstance(online, bool):
        errors.append("online is not boolean")
        online = False
    fix_type = gps.get("fix_type")
    satellites = gps.get("satellites")
    has_fix = fix_type != "no-fix" and isinstance(satellites, int) and satellites > 0
    if require_online and not online:
        errors.append("vehicle mirror is not online")
    if require_fix and not has_fix:
        errors.append("vehicle mirror has no GNSS fix")
    model = payload.get("model")
    if expected_model is not None and model != expected_model:
        errors.append(f"model {model!r} does not match {expected_model!r}")
    for key, low, high in (("latitude", -90, 90), ("longitude", -180, 180)):
        value = gps.get(key)
        if value is not None and (not _finite_number(value) or not low <= value <= high):
            errors.append(f"gps.{key} is outside its valid range")

    result.update(
        {
            "kind": "vehicle",
            "age_ms": age_ms,
            "fresh": not age_errors,
            "online": online,
            "model": model,
            "mgos_version": payload.get("mgos_version"),
            "fix_type": fix_type,
            "satellites": satellites,
            "has_fix": has_fix,
            "speed_mph": telem.get("speed_mph"),
            "moving": telem.get("moving"),
            "gaps": payload.get("gaps", []),
            "errors": errors,
        }
    )
    return result


def _collection_count(payload: dict[str, Any]) -> int | None:
    for key in (
        "alerts",
        "aircraft",
        "cameras",
        "contacts",
        "events",
        "features",
        "forecasts",
        "frames",
        "hotspots",
        "perimeters",
        "quakes",
        "samples",
        "stations",
        "transit",
        "vehicles",
    ):
        value = payload.get(key)
        if isinstance(value, list):
            return len(value)
    return None


def _overlay_catalog_feed(topic: str) -> str | None:
    if not topic.startswith(OVERLAY_PREFIX):
        return None
    suffix = topic[len(OVERLAY_PREFIX) :]
    feed, separator, node = suffix.partition("/")
    if not separator or not feed or not node:
        return None
    return feed


def _overlay_catalog_errors(topic: str, payload: dict[str, Any]) -> list[str]:
    feed = _overlay_catalog_feed(topic)
    if feed is None:
        return [f"overlay topic is not a per-node catalog topic: {topic!r}"]
    expected = CATALOG_OVERLAYS.get(feed)
    if expected is None:
        catalog = ", ".join(sorted(CATALOG_OVERLAYS))
        return [f"overlay feed {feed!r} is not in the WL-FUNC-012 zero-cost catalog: {catalog}"]
    errors: list[str] = []
    tier = payload.get("license_tier")
    if tier != expected["license_tier"]:
        errors.append(
            f"overlay feed {feed!r} license_tier {tier!r} does not match catalog "
            f"{expected['license_tier']!r}"
        )
    attribution = payload.get("attribution")
    if not isinstance(attribution, str) or expected["attribution_contains"] not in attribution:
        errors.append(
            f"overlay feed {feed!r} attribution does not contain "
            f"{expected['attribution_contains']!r}"
        )
    return errors


def _catalog_overlay_topic(feed: str, node: str) -> str:
    if (
        not isinstance(node, str)
        or not node
        or "/" in node
        or "\\" in node
        or "\x00" in node
        or node in {".", ".."}
    ):
        raise MirrorError(f"invalid catalog overlay node: {node!r}")
    return f"{OVERLAY_PREFIX}{feed}/{node}"


def _license_tier_errors(payload: dict[str, Any]) -> list[str]:
    tier = payload.get("license_tier")
    if not isinstance(tier, str) or not tier:
        return ["missing license_tier"]
    if tier in ALLOWED_OVERLAY_LICENSE_TIERS:
        return []
    normalized = tier.strip().lower()
    if any(hint in normalized for hint in DISALLOWED_LICENSE_TIER_HINTS):
        return [
            f"license_tier {tier!r} is disallowed by the zero-cost overlay audit"
        ]
    allowed = ", ".join(sorted(ALLOWED_OVERLAY_LICENSE_TIERS))
    return [f"license_tier {tier!r} is not in the release allowlist: {allowed}"]


def _explicit_status_only_overlay(payload: dict[str, Any]) -> bool:
    """Whether a mirror is an honest installed/status snapshot, not feed evidence."""
    availability = payload.get("availability")
    if isinstance(availability, str) and availability:
        return True
    gaps = payload.get("gaps")
    return isinstance(gaps, list) and any(isinstance(gap, str) and gap for gap in gaps)


def validate_overlay(
    bus_root: Path,
    topic: str,
    now_ms: int,
    max_age_ms: int,
    require_ready: bool,
    require_catalog_feed: bool,
    expected_host: str | None = None,
    allow_status_only: bool = False,
    require_mirror_fresh: bool = False,
) -> dict[str, Any]:
    if not topic.startswith(OVERLAY_PREFIX):
        raise MirrorError(f"overlay topic must start with {OVERLAY_PREFIX!r}: {topic}")
    row, _envelope, payload, digest = _read_latest(bus_root, topic)
    result = _base_result(topic, row, payload, digest)
    errors: list[str] = []
    mirror_age_ms, mirror_age_errors = _indexed_age_check(row, now_ms, max_age_ms)
    if require_mirror_fresh:
        errors.extend(mirror_age_errors)
    topic_node = topic.rsplit("/", 1)[-1]
    payload_host = payload.get("host")
    if not isinstance(payload_host, str) or not payload_host:
        errors.append("payload host is missing or not a non-empty string")
    elif payload_host != topic_node:
        errors.append(f"payload host {payload_host!r} does not match topic node {topic_node!r}")
    if expected_host is not None and payload.get("host") != expected_host:
        errors.append(
            f"payload host {payload.get('host')!r} does not match vehicle host {expected_host!r}"
        )
    age_errors: list[str] = []
    fetched_at = payload.get("fetched_at_ms")
    status_only = False
    if fetched_at is None:
        if allow_status_only and _explicit_status_only_overlay(payload):
            status_only = True
        else:
            errors.append("missing fetched_at_ms; publication alone is not feed evidence")
        age_ms = 0
    else:
        age_ms, age_errors = _age_check(payload, now_ms, max_age_ms, required="fetched_at_ms")
        errors.extend(age_errors)
    availability = payload.get("availability")
    availability_text = availability if isinstance(availability, str) else None
    fresh = fetched_at is not None and not age_errors
    available = availability_text not in {"unconfigured", "secret_store_error", "error", "paused"}
    ready = fresh and available
    license_tier_errors = _license_tier_errors(payload)
    errors.extend(license_tier_errors)
    catalog_feed = _overlay_catalog_feed(topic)
    catalog_errors = _overlay_catalog_errors(topic, payload) if require_catalog_feed else []
    errors.extend(catalog_errors)
    if require_ready:
        if not ready:
            errors.append(f"overlay is not ready (availability={availability_text!r})")
        if not isinstance(payload.get("attribution"), str) or not payload["attribution"]:
            errors.append("missing attribution")
    result.update(
        {
            "kind": "overlay",
            "age_ms": age_ms,
            "fresh": fresh,
            "mirror_age_ms": mirror_age_ms,
            "mirror_fresh": not mirror_age_errors,
            "status_only": status_only,
            "available": available,
            "ready": ready,
            "availability": availability_text,
            "fetched_at_ms": fetched_at,
            "record_count": _collection_count(payload),
            "catalog_feed": catalog_feed,
            "catalog_feed_allowed": not catalog_errors,
            "license_tier": payload.get("license_tier"),
            "license_tier_allowed": not license_tier_errors,
            "attribution": payload.get("attribution"),
            "gaps": payload.get("gaps", []),
            "errors": errors,
        }
    )
    return result


def validate_overlay_catalog_node(
    bus_root: Path,
    node: str,
    now_ms: int,
    max_age_ms: int,
    require_ready: bool,
    require_catalog_complete: bool,
    expected_host: str | None = None,
    require_installed_catalog: bool = False,
) -> dict[str, Any]:
    """Audit every locked WL-FUNC-012 zero-cost overlay mirror for one node."""
    feeds = sorted(CATALOG_OVERLAYS)
    per_feed: dict[str, dict[str, Any]] = {}
    feeds_present: list[str] = []
    feeds_absent: list[str] = []
    feeds_invalid: list[str] = []
    errors: list[str] = []

    for feed in feeds:
        topic = _catalog_overlay_topic(feed, node)
        try:
            result = validate_overlay(
                bus_root,
                topic,
                now_ms,
                max_age_ms,
                require_ready,
                True,
                expected_host,
                require_installed_catalog and not require_ready,
                require_installed_catalog,
            )
        except MirrorError as exc:
            message = str(exc)
            if message == f"no indexed message for {topic}":
                feeds_absent.append(feed)
                per_feed[feed] = {
                    "topic": topic,
                    "state": "absent",
                    "errors": [],
                }
            else:
                feeds_invalid.append(feed)
                per_feed[feed] = {
                    "topic": topic,
                    "state": "invalid",
                    "errors": [message],
                }
                errors.append(f"{feed}: {message}")
            continue

        feeds_present.append(feed)
        feed_errors = result.get("errors", [])
        if feed_errors:
            feeds_invalid.append(feed)
            per_feed[feed] = {
                "topic": topic,
                "state": "invalid",
                "ready": result.get("ready"),
                "fresh": result.get("fresh"),
                "age_ms": result.get("age_ms"),
                "mirror_fresh": result.get("mirror_fresh"),
                "mirror_age_ms": result.get("mirror_age_ms"),
                "status_only": result.get("status_only"),
                "availability": result.get("availability"),
                "record_count": result.get("record_count"),
                "license_tier": result.get("license_tier"),
                "errors": feed_errors,
            }
            errors.extend(f"{feed}: {error}" for error in feed_errors)
        else:
            per_feed[feed] = {
                "topic": topic,
                "state": "present",
                "ready": result.get("ready"),
                "fresh": result.get("fresh"),
                "age_ms": result.get("age_ms"),
                "mirror_fresh": result.get("mirror_fresh"),
                "mirror_age_ms": result.get("mirror_age_ms"),
                "status_only": result.get("status_only"),
                "availability": result.get("availability"),
                "record_count": result.get("record_count"),
                "license_tier": result.get("license_tier"),
                "errors": [],
            }

    if (require_catalog_complete or require_installed_catalog) and feeds_absent:
        errors.append(
            f"missing zero-cost catalog overlay mirrors for node {node!r}: "
            f"{', '.join(feeds_absent)}"
        )
    if require_installed_catalog and feeds_invalid:
        errors.append(
            f"invalid installed zero-cost catalog overlay mirrors for node {node!r}: "
            f"{', '.join(feeds_invalid)}"
        )

    return {
        "kind": "overlay_catalog",
        "topic": f"{OVERLAY_PREFIX}*/{node}",
        "node": node,
        "catalog_count": len(feeds),
        "catalog_feeds": feeds,
        "feeds_present": feeds_present,
        "feeds_absent": feeds_absent,
        "feeds_invalid": feeds_invalid,
        "present_count": len(feeds_present),
        "absent_count": len(feeds_absent),
        "invalid_count": len(feeds_invalid),
        "catalog_complete": not feeds_absent,
        "catalog_valid": not feeds_absent and not feeds_invalid,
        "installed_catalog_present": not feeds_absent and not feeds_invalid,
        "require_catalog_complete": require_catalog_complete,
        "require_installed_catalog": require_installed_catalog,
        "per_feed": per_feed,
        "errors": errors,
    }


def _indexed_age_check(row: dict[str, Any], now_ms: int, max_age_ms: int) -> tuple[int, list[str]]:
    errors: list[str] = []
    stamp = row.get("ts_unix_ms")
    if not isinstance(stamp, int) or isinstance(stamp, bool):
        return 0, ["missing integer bus ts_unix_ms"]
    age = now_ms - stamp
    if age < 0:
        errors.append(f"bus ts_unix_ms is in the future by {-age} ms")
    elif max_age_ms >= 0 and age > max_age_ms:
        errors.append(f"bus ts_unix_ms is stale by {age} ms (limit {max_age_ms} ms)")
    return max(age, 0), errors


def validate_vdi_session_rail(
    bus_root: Path,
    session_id: str,
    now_ms: int,
    max_age_ms: int,
    require_visible: bool,
    require_active: bool,
    expected_serving_node: str | None = None,
    expected_vm_id: str | None = None,
    expected_client_peer: str | None = None,
) -> dict[str, Any]:
    """Validate the shell/status-rail lifecycle proof for one brokered session."""
    rows = _read_topic_rows(bus_root, VDI_SESSION_TOPIC, MAX_TOPIC_SCAN_ROWS)
    matches: list[tuple[dict[str, Any], dict[str, Any], dict[str, Any], str]] = []
    for row, envelope, payload, digest in rows:
        if payload.get("id") == session_id:
            matches.append((row, envelope, payload, digest))
    if not matches:
        raise MirrorError(
            f"no {VDI_SESSION_TOPIC} lifecycle row for session {session_id!r} "
            f"in newest {MAX_TOPIC_SCAN_ROWS} rows"
        )

    matches.sort(key=lambda item: str(item[0].get("ulid", "")))
    errors: list[str] = []
    open_row: dict[str, Any] | None = None
    open_payload: dict[str, Any] | None = None
    active_row: dict[str, Any] | None = None
    state_row: dict[str, Any] | None = None
    state_payload: dict[str, Any] | None = None
    state_digest = ""
    state = "unknown"
    ops_seen: list[str] = []
    for row, _envelope, payload, digest in matches:
        op = payload.get("op")
        if not isinstance(op, str):
            errors.append("VDI lifecycle row has no string op")
            continue
        ops_seen.append(op)
        if op == "open":
            open_row = row
            open_payload = payload
            state = "requested"
            state_row = row
            state_payload = payload
            state_digest = digest
        elif op == "active":
            if open_payload is None:
                errors.append("VDI lifecycle active row appeared before an open row")
            active_row = row
            state = "active"
            state_row = row
            state_payload = payload
            state_digest = digest
        elif op == "disconnect":
            state = "disconnected"
            state_row = row
            state_payload = payload
            state_digest = digest
        elif op == "close":
            state = "closed"
            state_row = row
            state_payload = payload
            state_digest = digest
        else:
            errors.append(f"unknown VDI lifecycle op {op!r}")

    if open_payload is None or open_row is None:
        errors.append("VDI status rail has no open lifecycle row")
    else:
        serving_peer = open_payload.get("serving_peer")
        vm_id = open_payload.get("vm_id")
        client_peer = open_payload.get("client_peer")
        if not isinstance(serving_peer, str) or not serving_peer:
            errors.append("open lifecycle serving_peer is missing or not a non-empty string")
        elif expected_serving_node is not None and serving_peer != expected_serving_node:
            errors.append(
                f"status-rail serving_peer {serving_peer!r} does not match "
                f"console serving_node {expected_serving_node!r}"
            )
        if not isinstance(vm_id, str) or not vm_id:
            errors.append("open lifecycle vm_id is missing or not a non-empty string")
        elif expected_vm_id is not None and vm_id != expected_vm_id:
            errors.append(
                f"status-rail vm_id {vm_id!r} does not match console vm_id {expected_vm_id!r}"
            )
        if not isinstance(client_peer, str) or not client_peer:
            errors.append("open lifecycle client_peer is missing or not a non-empty string")
        elif expected_client_peer is not None and client_peer != expected_client_peer:
            errors.append(
                f"status-rail client_peer {client_peer!r} does not match "
                f"{expected_client_peer!r}"
            )

    age_ms = 0
    age_errors: list[str] = []
    if state_row is not None:
        age_ms, age_errors = _indexed_age_check(state_row, now_ms, max_age_ms)
        errors.extend(age_errors)

    visible = state in {"requested", "active", "disconnected"}
    if require_visible and not visible:
        errors.append(f"VDI status rail is not visible (latest lifecycle state is {state!r})")
    if require_active and state != "active":
        errors.append(f"VDI status rail is not LIVE (latest lifecycle state is {state!r})")
    if require_active and active_row is None:
        errors.append("VDI status rail has no active lifecycle row")
    rail_badge = {
        "requested": "VDI",
        "active": "LIVE",
        "disconnected": "DISC",
    }.get(state)
    result_row = state_row or open_row or matches[-1][0]
    result_payload = state_payload or open_payload or matches[-1][2]
    result_digest = state_digest or matches[-1][3]
    result = _base_result(VDI_SESSION_TOPIC, result_row, result_payload, result_digest)
    result.update(
        {
            "kind": "vdi_status_rail",
            "age_ms": age_ms,
            "fresh": not age_errors,
            "session_id": session_id,
            "state": state,
            "visible_in_status_rail": visible,
            "rail_badge": rail_badge,
            "serving_peer": open_payload.get("serving_peer") if open_payload else None,
            "vm_id": open_payload.get("vm_id") if open_payload else None,
            "client_peer": open_payload.get("client_peer") if open_payload else None,
            "opened": open_payload is not None,
            "active": state == "active",
            "ops_seen": ops_seen,
            "errors": errors,
        }
    )
    return result


def validate_airspace(
    bus_root: Path,
    node: str,
    now_ms: int,
    max_age_ms: int,
    require_ready: bool,
    require_contacts: bool,
    expected_host: str | None = None,
) -> dict[str, Any]:
    topic = f"{AIRSPACE_PREFIX}{node}"
    row, _envelope, payload, digest = _read_latest(bus_root, topic)
    result = _base_result(topic, row, payload, digest)
    errors: list[str] = []
    payload_host = payload.get("host")
    if not isinstance(payload_host, str) or not payload_host:
        errors.append("payload host is missing or not a non-empty string")
    elif payload_host != node:
        errors.append(f"payload host {payload_host!r} does not match airspace node {node!r}")
    if expected_host is not None and payload_host != expected_host:
        errors.append(
            f"payload host {payload_host!r} does not match vehicle host {expected_host!r}"
        )

    published_age_ms, published_age_errors = _age_check(
        payload, now_ms, max_age_ms, required="published_at_ms"
    )
    errors.extend(published_age_errors)
    availability = payload.get("availability")
    availability_text = availability if isinstance(availability, str) else None
    if availability_text not in {"no_source", "offline", "ready"}:
        errors.append(f"airspace availability {availability_text!r} is not recognized")
    contacts = payload.get("contacts")
    if not isinstance(contacts, list):
        errors.append("contacts is not an array")
        contacts = []
    contact_count = len(contacts)
    if contact_count > 256:
        errors.append("airspace retained contacts exceed the 256-contact wire bound")
    if availability_text in {"no_source", "offline"} and contact_count:
        errors.append(f"airspace availability {availability_text!r} must not carry contacts")

    scanned_at = payload.get("scanned_at_ms")
    scanner_age_ms: int | None = None
    scanner_fresh = False
    if scanned_at is not None:
        scanner_age_ms, scanner_age_errors = _age_check(
            payload, now_ms, max_age_ms, required="scanned_at_ms"
        )
        errors.extend(scanner_age_errors)
        scanner_fresh = not scanner_age_errors
    if require_ready:
        if availability_text != "ready":
            errors.append(f"airspace scanner is not ready (availability={availability_text!r})")
        if scanned_at is None:
            errors.append("missing scanned_at_ms; publication alone is not scanner evidence")
        elif not scanner_fresh:
            errors.append("airspace scanner observation is not fresh")
    if require_contacts and contact_count == 0:
        errors.append("airspace scanner has no retained contacts")

    result.update(
        {
            "kind": "airspace",
            "age_ms": published_age_ms,
            "fresh": not published_age_errors and (availability_text != "ready" or scanner_fresh),
            "ready": availability_text == "ready",
            "availability": availability_text,
            "scanned_at_ms": scanned_at,
            "scanner_age_ms": scanner_age_ms,
            "scanner_fresh": scanner_fresh,
            "record_count": contact_count,
            "omitted_contacts": payload.get("omitted_contacts"),
            "gaps": payload.get("gaps", []),
            "errors": errors,
        }
    )
    return result


def run(args: argparse.Namespace) -> int:
    root = Path(args.bus_root)
    now_ms = args.now_ms if args.now_ms is not None else _now_ms()
    max_age_ms = int(args.max_age_seconds * 1000)
    results: list[dict[str, Any]] = []
    failures: list[str] = []
    if args.vehicle_node:
        try:
            result = validate_vehicle(
                root,
                args.vehicle_node,
                now_ms,
                max_age_ms,
                args.require_online,
                args.require_fix,
                args.expected_model,
            )
        except MirrorError as exc:
            result = {"kind": "vehicle", "topic": f"{VEHICLE_PREFIX}{args.vehicle_node}", "errors": [str(exc)]}
        results.append(result)
        failures.extend(result.get("errors", []))
    for node in args.catalog_overlay_node:
        try:
            result = validate_overlay_catalog_node(
                root,
                node,
                now_ms,
                max_age_ms,
                args.require_ready,
                args.require_catalog_complete,
                args.vehicle_node if args.require_same_host else None,
                args.require_installed_overlay_catalog,
            )
        except MirrorError as exc:
            result = {
                "kind": "overlay_catalog",
                "topic": f"{OVERLAY_PREFIX}*/{node}",
                "errors": [str(exc)],
            }
        results.append(result)
        failures.extend(result.get("errors", []))
    for topic in args.overlay:
        try:
            result = validate_overlay(
                root,
                topic,
                now_ms,
                max_age_ms,
                args.require_ready,
                args.require_catalog_feed,
                args.vehicle_node if args.require_same_host else None,
            )
        except MirrorError as exc:
            result = {"kind": "overlay", "topic": topic, "errors": [str(exc)]}
        results.append(result)
        failures.extend(result.get("errors", []))
    for node in args.airspace_node:
        try:
            result = validate_airspace(
                root,
                node,
                now_ms,
                max_age_ms,
                args.require_airspace_ready,
                args.require_airspace_contacts,
                args.vehicle_node if args.require_same_host else None,
            )
        except MirrorError as exc:
            result = {"kind": "airspace", "topic": f"{AIRSPACE_PREFIX}{node}", "errors": [str(exc)]}
        results.append(result)
        failures.extend(result.get("errors", []))
    if args.vdi_session:
        try:
            rail_result = validate_vdi_session_rail(
                root,
                args.vdi_session,
                now_ms,
                max_age_ms,
                args.require_vdi_status_rail or args.require_vdi_status_rail_live,
                args.require_vdi_status_rail_live,
                args.expected_vdi_serving_node,
                args.expected_vdi_vm,
                args.expected_vdi_client_peer,
            )
        except MirrorError as exc:
            rail_result = {
                "kind": "vdi_status_rail",
                "topic": VDI_SESSION_TOPIC,
                "errors": [str(exc)],
            }
        results.append(rail_result)
        failures.extend(rail_result.get("errors", []))
    report = {
        "observed_at_ms": now_ms,
        "bus_root": str(root.resolve()) if root.exists() else str(root),
        "read_only": True,
        "same_host_required": args.require_same_host,
        "results": results,
        "ok": not failures and bool(results),
    }
    print(json.dumps(report, sort_keys=True, indent=2))
    return 0 if report["ok"] else 1


def _self_test() -> None:
    now_ms = 1_700_000_000_000
    with tempfile.TemporaryDirectory(prefix="verify-live-mirrors-") as temp:
        root = Path(temp)
        db = root / "index.sqlite"
        conn = sqlite3.connect(db)
        conn.execute(
            "CREATE TABLE messages (ulid TEXT PRIMARY KEY, topic TEXT, priority TEXT, "
            "title TEXT, body TEXT, ts_unix_ms INTEGER, file_path TEXT)"
        )

        def add(
            ulid: str,
            topic: str,
            payload: dict[str, Any],
            *,
            bus_ts_ms: int | None = None,
        ) -> None:
            rel = f"{topic}/{ulid}.json"
            path = root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            body = json.dumps(payload, separators=(",", ":"))
            ts_unix_ms = now_ms - 10_000 if bus_ts_ms is None else bus_ts_ms
            path.write_text(
                json.dumps(
                    {
                        "ulid": ulid,
                        "topic": topic,
                        "priority": "default",
                        "title": None,
                        "body": body,
                        "ts_unix_ms": ts_unix_ms,
                        "file_path": rel,
                        "actions": [],
                        "reply_to": None,
                    }
                )
            )
            conn.execute(
                "INSERT INTO messages VALUES (?, ?, ?, ?, ?, ?, ?)",
                (ulid, topic, "default", None, body, ts_unix_ms, rel),
            )

        def fixture_ulid(serial: int) -> str:
            return f"01J{serial:023d}"

        def catalog_payload(
            feed: str,
            node: str,
            fetched_at_ms: int | None = None,
            *,
            status_only: bool = False,
        ) -> dict[str, Any]:
            expected = CATALOG_OVERLAYS[feed]
            payload: dict[str, Any] = {
                "host": node,
                "events": [],
                "license_tier": expected["license_tier"],
                "attribution": f"{expected['attribution_contains']} fixture",
            }
            if status_only:
                payload["availability"] = "unconfigured"
                payload["gaps"] = [f"{feed} fixture is installed but has no feed evidence"]
            else:
                payload["fetched_at_ms"] = (
                    now_ms - 10_000 if fetched_at_ms is None else fetched_at_ms
                )
            return payload

        def add_catalog_node(
            node: str,
            start_serial: int,
            *,
            stale_feed: str | None = None,
            future_feed: str | None = None,
            status_only_feeds: set[str] | None = None,
            stale_bus_feed: str | None = None,
        ) -> None:
            status_only_feeds = status_only_feeds or set()
            for offset, feed in enumerate(sorted(CATALOG_OVERLAYS)):
                fetched_at_ms = now_ms - 10_000
                if feed == stale_feed:
                    fetched_at_ms = now_ms - 60_000
                if feed == future_feed:
                    fetched_at_ms = now_ms + 1_000
                bus_ts_ms = now_ms - 10_000
                if feed == stale_bus_feed:
                    bus_ts_ms = now_ms - 60_000
                add(
                    fixture_ulid(start_serial + offset),
                    _catalog_overlay_topic(feed, node),
                    catalog_payload(
                        feed,
                        node,
                        fetched_at_ms,
                        status_only=feed in status_only_feeds,
                    ),
                    bus_ts_ms=bus_ts_ms,
                )

        add(
            "01J00000000000000000000000",
            "state/vehicle/test-node",
            {
                "host": "test-node",
                "model": "MG90",
                "mgos_version": "4.3.0.1",
                "online": True,
                "gps": {"fix_type": "no-fix", "satellites": 0, "latitude": 0, "longitude": 0},
                "telem": {"speed_mph": 0, "moving": False},
                "published_at_ms": now_ms - 10_000,
            },
        )
        add(
            "01J00000000000000000000001",
            "state/overlay/usgs-earthquakes/test-node",
            {
                "host": "test-node",
                "fetched_at_ms": now_ms - 10_000,
                "events": [],
                "license_tier": "public-domain",
                "attribution": "USGS fixture",
            },
        )
        add(
            "01J00000000000000000000004",
            "state/overlay/non-commercial/test-node",
            {
                "host": "test-node",
                "fetched_at_ms": now_ms - 10_000,
                "events": [],
                "license_tier": "non-commercial-free",
                "attribution": "fixture",
            },
        )
        add(
            "01J00000000000000000000008",
            "state/airspace/test-node",
            {
                "host": "test-node",
                "published_at_ms": now_ms - 5_000,
                "scanned_at_ms": now_ms - 6_000,
                "availability": "ready",
                "contacts": [
                    {
                        "id": "fixture-ap",
                        "kind": "wifi",
                        "signal_dbm": -64,
                        "bearing_deg": 90.0,
                    }
                ],
                "omitted_contacts": 0,
            },
        )
        add(
            "01J00000000000000000000009",
            VDI_SESSION_TOPIC,
            {
                "schema_version": 1,
                "armed_token": "fixture",
                "op": "open",
                "id": "vdi-1-win11",
                "serving_peer": "peer:oak",
                "vm_id": "win11",
                "client_peer": "seat-15",
            },
        )
        add(
            "01J00000000000000000000010",
            VDI_SESSION_TOPIC,
            {
                "schema_version": 1,
                "armed_token": "fixture",
                "op": "active",
                "id": "vdi-1-win11",
            },
        )
        add(
            "01J00000000000000000000011",
            VDI_SESSION_TOPIC,
            {
                "schema_version": 1,
                "armed_token": "fixture",
                "op": "open",
                "id": "vdi-2-off",
                "serving_peer": "peer:oak",
                "vm_id": "off",
                "client_peer": "seat-15",
            },
        )
        add_catalog_node(
            "installed-node",
            100,
            status_only_feeds={"airnow-aqi", "firms-hotspots"},
        )
        add_catalog_node("stale-installed-node", 200, stale_feed="usgs-earthquakes")
        add_catalog_node("future-installed-node", 300, future_feed="usgs-earthquakes")
        add_catalog_node(
            "stale-status-node",
            400,
            status_only_feeds={"airnow-aqi"},
            stale_bus_feed="airnow-aqi",
        )
        conn.commit()
        conn.close()

        vehicle = validate_vehicle(root, "test-node", now_ms, 30_000, True, False, "MG90")
        assert not vehicle["errors"], vehicle
        assert vehicle["online"] is True and vehicle["has_fix"] is False
        overlay = validate_overlay(
            root,
            "state/overlay/usgs-earthquakes/test-node",
            now_ms,
            30_000,
            True,
            False,
        )
        assert not overlay["errors"], overlay
        assert overlay["ready"] is True and overlay["record_count"] == 0
        catalog_overlay = validate_overlay(
            root,
            "state/overlay/usgs-earthquakes/test-node",
            now_ms,
            30_000,
            True,
            True,
        )
        assert not catalog_overlay["errors"], catalog_overlay
        assert catalog_overlay["catalog_feed"] == "usgs-earthquakes", catalog_overlay
        catalog_audit = validate_overlay_catalog_node(
            root,
            "test-node",
            now_ms,
            30_000,
            False,
            False,
        )
        assert catalog_audit["catalog_count"] == len(CATALOG_OVERLAYS), catalog_audit
        assert catalog_audit["feeds_present"] == ["usgs-earthquakes"], catalog_audit
        assert "adsb-aircraft" in catalog_audit["feeds_absent"], catalog_audit
        assert not catalog_audit["errors"], catalog_audit
        required_catalog = validate_overlay_catalog_node(
            root,
            "test-node",
            now_ms,
            30_000,
            False,
            True,
        )
        assert required_catalog["catalog_complete"] is False, required_catalog
        assert any("missing zero-cost catalog overlay mirrors" in error for error in required_catalog["errors"]), required_catalog
        assert "usgs-earthquakes" not in required_catalog["feeds_absent"], required_catalog
        missing_installed_catalog = validate_overlay_catalog_node(
            root,
            "test-node",
            now_ms,
            30_000,
            False,
            False,
            None,
            True,
        )
        assert missing_installed_catalog["require_installed_catalog"] is True, missing_installed_catalog
        assert missing_installed_catalog["installed_catalog_present"] is False, missing_installed_catalog
        assert "airnow-aqi" in missing_installed_catalog["feeds_absent"], missing_installed_catalog
        assert any(
            "missing zero-cost catalog overlay mirrors" in error
            for error in missing_installed_catalog["errors"]
        ), missing_installed_catalog
        installed_catalog = validate_overlay_catalog_node(
            root,
            "installed-node",
            now_ms,
            30_000,
            False,
            False,
            None,
            True,
        )
        assert installed_catalog["installed_catalog_present"] is True, installed_catalog
        assert installed_catalog["catalog_complete"] is True, installed_catalog
        assert not installed_catalog["errors"], installed_catalog
        assert installed_catalog["present_count"] == len(CATALOG_OVERLAYS), installed_catalog
        airnow_status = installed_catalog["per_feed"]["airnow-aqi"]
        assert airnow_status["status_only"] is True, installed_catalog
        assert airnow_status["mirror_fresh"] is True, installed_catalog
        assert airnow_status["fresh"] is False and airnow_status["ready"] is False, installed_catalog
        stale_installed_catalog = validate_overlay_catalog_node(
            root,
            "stale-installed-node",
            now_ms,
            30_000,
            False,
            False,
            None,
            True,
        )
        assert stale_installed_catalog["installed_catalog_present"] is False, stale_installed_catalog
        assert "usgs-earthquakes" in stale_installed_catalog["feeds_invalid"], stale_installed_catalog
        assert stale_installed_catalog["per_feed"]["usgs-earthquakes"]["ready"] is False, stale_installed_catalog
        assert any("stale" in error for error in stale_installed_catalog["errors"]), stale_installed_catalog
        future_installed_catalog = validate_overlay_catalog_node(
            root,
            "future-installed-node",
            now_ms,
            30_000,
            False,
            False,
            None,
            True,
        )
        assert future_installed_catalog["installed_catalog_present"] is False, future_installed_catalog
        assert "usgs-earthquakes" in future_installed_catalog["feeds_invalid"], future_installed_catalog
        assert future_installed_catalog["per_feed"]["usgs-earthquakes"]["ready"] is False, future_installed_catalog
        assert any("future" in error for error in future_installed_catalog["errors"]), future_installed_catalog
        stale_status_catalog = validate_overlay_catalog_node(
            root,
            "stale-status-node",
            now_ms,
            30_000,
            False,
            False,
            None,
            True,
        )
        assert stale_status_catalog["installed_catalog_present"] is False, stale_status_catalog
        assert "airnow-aqi" in stale_status_catalog["feeds_invalid"], stale_status_catalog
        assert stale_status_catalog["per_feed"]["airnow-aqi"]["mirror_fresh"] is False, stale_status_catalog
        assert any("bus ts_unix_ms is stale" in error for error in stale_status_catalog["errors"]), stale_status_catalog
        stale_status_ready_catalog = validate_overlay_catalog_node(
            root,
            "stale-status-node",
            now_ms,
            30_000,
            True,
            False,
            None,
            True,
        )
        assert stale_status_ready_catalog["installed_catalog_present"] is False, stale_status_ready_catalog
        assert "airnow-aqi" in stale_status_ready_catalog["feeds_invalid"], stale_status_ready_catalog
        assert any("bus ts_unix_ms is stale" in error for error in stale_status_ready_catalog["errors"]), stale_status_ready_catalog
        generic_overlay = validate_overlay(
            root,
            "state/overlay/non-commercial/test-node",
            now_ms,
            30_000,
            False,
            True,
        )
        assert generic_overlay["catalog_feed_allowed"] is False, generic_overlay
        blocked_license = validate_overlay(
            root, "state/overlay/non-commercial/test-node", now_ms, 30_000, True, False
        )
        assert blocked_license["license_tier_allowed"] is False, blocked_license
        assert any("zero-cost" in error for error in blocked_license["errors"]), blocked_license
        airspace = validate_airspace(root, "test-node", now_ms, 30_000, True, True)
        assert not airspace["errors"], airspace
        assert airspace["ready"] is True and airspace["scanner_fresh"] is True, airspace
        rail = validate_vdi_session_rail(
            root,
            "vdi-1-win11",
            now_ms,
            30_000,
            True,
            True,
            "peer:oak",
            "win11",
            "seat-15",
        )
        assert not rail["errors"], rail
        assert rail["active"] is True and rail["rail_badge"] == "LIVE", rail
        requested_rail = validate_vdi_session_rail(
            root,
            "vdi-2-off",
            now_ms,
            30_000,
            True,
            True,
            "peer:oak",
            "off",
            "seat-15",
        )
        assert requested_rail["visible_in_status_rail"] is True, requested_rail
        assert requested_rail["rail_badge"] == "VDI", requested_rail
        assert any("not LIVE" in error for error in requested_rail["errors"]), requested_rail
        missing_host_payload = {
            "fetched_at_ms": now_ms - 10_000,
            "events": [],
            "license_tier": "open-data-attribution",
            "attribution": "fixture",
        }
        # Reuse the same indexed envelope contract with a missing host body;
        # host identity is mandatory even without the cross-topic handoff flag.
        missing_host_topic = "state/overlay/missing-host/test-node"
        missing_host_path = root / f"{missing_host_topic}/missing-host.json"
        missing_host_path.parent.mkdir(parents=True, exist_ok=True)
        missing_host_body = json.dumps(missing_host_payload, separators=(",", ":"))
        missing_host_path.write_text(
            json.dumps(
                {
                    "ulid": "01J00000000000000000000002",
                    "topic": missing_host_topic,
                    "priority": "default",
                    "title": None,
                    "body": missing_host_body,
                    "ts_unix_ms": now_ms - 1_000,
                    "file_path": f"{missing_host_topic}/missing-host.json",
                    "actions": [],
                    "reply_to": None,
                }
            )
        )
        conn = sqlite3.connect(db)
        conn.execute(
            "INSERT INTO messages VALUES (?, ?, ?, ?, ?, ?, ?)",
            (
                "01J00000000000000000000002",
                missing_host_topic,
                "default",
                None,
                missing_host_body,
                now_ms - 1_000,
                f"{missing_host_topic}/missing-host.json",
            ),
        )
        conn.commit()
        conn.close()
        missing_host_result = validate_overlay(
            root, missing_host_topic, now_ms, 30_000, True, False
        )
        assert any(
            "payload host is missing" in error for error in missing_host_result["errors"]
        ), missing_host_result
        same_host = validate_overlay(
            root,
            "state/overlay/usgs-earthquakes/test-node",
            now_ms,
            30_000,
            True,
            True,
            "test-node",
        )
        assert not same_host["errors"], same_host
        cli_output = StringIO()
        cli_args = argparse.Namespace(
            bus_root=str(root),
            now_ms=now_ms,
            max_age_seconds=30.0,
            vehicle_node="test-node",
            catalog_overlay_node=[],
            overlay=["state/overlay/usgs-earthquakes/test-node"],
            airspace_node=["test-node"],
            vdi_session="vdi-1-win11",
            require_online=True,
            require_fix=False,
            expected_model="MG90",
            require_ready=True,
            require_catalog_feed=True,
            require_catalog_complete=False,
            require_installed_overlay_catalog=False,
            require_same_host=True,
            require_airspace_ready=True,
            require_airspace_contacts=True,
            require_vdi_status_rail=True,
            require_vdi_status_rail_live=True,
            expected_vdi_serving_node="peer:oak",
            expected_vdi_vm="win11",
            expected_vdi_client_peer="seat-15",
        )
        with redirect_stdout(cli_output):
            assert run(cli_args) == 0
        cli_report = json.loads(cli_output.getvalue())
        assert cli_report["same_host_required"] is True, cli_report
        assert cli_report["ok"] is True, cli_report
        catalog_cli_output = StringIO()
        with redirect_stdout(catalog_cli_output):
            assert (
                main(
                    [
                        "--bus-root",
                        str(root),
                        "--now-ms",
                        str(now_ms),
                        "--max-age-seconds",
                        "30",
                        "--catalog-overlay-node",
                        "test-node",
                    ]
                )
                == 0
            )
        catalog_cli_report = json.loads(catalog_cli_output.getvalue())
        catalog_cli_result = catalog_cli_report["results"][0]
        assert catalog_cli_report["ok"] is True, catalog_cli_report
        assert catalog_cli_result["feeds_present"] == ["usgs-earthquakes"], catalog_cli_result
        assert "airnow-aqi" in catalog_cli_result["feeds_absent"], catalog_cli_result
        installed_cli_output = StringIO()
        with redirect_stdout(installed_cli_output):
            assert (
                main(
                    [
                        "--bus-root",
                        str(root),
                        "--now-ms",
                        str(now_ms),
                        "--max-age-seconds",
                        "30",
                        "--catalog-overlay-node",
                        "installed-node",
                        "--require-installed-overlay-catalog",
                    ]
                )
                == 0
            )
        installed_cli_report = json.loads(installed_cli_output.getvalue())
        installed_cli_result = installed_cli_report["results"][0]
        assert installed_cli_report["ok"] is True, installed_cli_report
        assert installed_cli_result["installed_catalog_present"] is True, installed_cli_result
        assert installed_cli_result["per_feed"]["airnow-aqi"]["ready"] is False, installed_cli_result
        stale_installed_cli_output = StringIO()
        with redirect_stdout(stale_installed_cli_output):
            assert (
                main(
                    [
                        "--bus-root",
                        str(root),
                        "--now-ms",
                        str(now_ms),
                        "--max-age-seconds",
                        "30",
                        "--catalog-overlay-node",
                        "stale-installed-node",
                        "--require-installed-overlay-catalog",
                    ]
                )
                == 1
            )
        stale_installed_cli_report = json.loads(stale_installed_cli_output.getvalue())
        assert stale_installed_cli_report["ok"] is False, stale_installed_cli_report
        assert (
            stale_installed_cli_report["results"][0]["per_feed"]["usgs-earthquakes"]["ready"]
            is False
        ), stale_installed_cli_report
        mismatched = validate_overlay(
            root,
            "state/overlay/usgs-earthquakes/test-node",
            now_ms,
            30_000,
            True,
            True,
            "other-node",
        )
        assert any("vehicle host" in error for error in mismatched["errors"]), mismatched
        stale = validate_overlay(
            root, "state/overlay/usgs-earthquakes/test-node", now_ms, 1_000, True, False
        )
        assert stale["errors"], stale
        assert stale["fresh"] is False, stale
        assert stale["ready"] is False, stale
        assert any("overlay is not ready" in error for error in stale["errors"]), stale

        symlink_topic = "state/overlay/symlink/test-node"
        outside = root.parent / f"{root.name}-outside.json"
        outside.write_text("{}")
        symlink_path = root / f"{symlink_topic}/message.json"
        symlink_path.parent.mkdir(parents=True, exist_ok=True)
        symlink_path.symlink_to(outside)
        symlink_body = json.dumps(
            {
                "host": "test-node",
                "fetched_at_ms": now_ms - 1_000,
                "events": [],
                "license_tier": "fixture",
                "attribution": "fixture",
            },
            separators=(",", ":"),
        )
        conn = sqlite3.connect(db)
        conn.execute(
            "INSERT INTO messages VALUES (?, ?, ?, ?, ?, ?, ?)",
            (
                "01J00000000000000000000003",
                symlink_topic,
                "default",
                None,
                symlink_body,
                now_ms - 1_000,
                f"{symlink_topic}/message.json",
            ),
        )
        conn.commit()
        conn.close()
        try:
            _read_latest(root, symlink_topic)
        except MirrorError as exc:
            assert "symlink" in str(exc), exc
        else:
            raise AssertionError("indexed symlink path was accepted")
    print("verify-live-mirrors: self-test passed")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bus-root", default=os.environ.get("MDE_BUS_ROOT", str(DEFAULT_BUS_ROOT)))
    parser.add_argument("--vehicle-node", help="validate state/vehicle/<node>")
    parser.add_argument("--overlay", action="append", default=[], metavar="TOPIC")
    parser.add_argument("--max-age-seconds", type=float, default=60.0)
    parser.add_argument("--now-ms", type=int, help="fixed observation time for reproducible checks")
    parser.add_argument("--require-online", action="store_true")
    parser.add_argument("--require-fix", action="store_true")
    parser.add_argument("--expected-model")
    parser.add_argument("--require-ready", action="store_true")
    parser.add_argument(
        "--require-catalog-feed",
        action="store_true",
        help="require overlay topics to match the locked zero-cost catalog",
    )
    parser.add_argument(
        "--catalog-overlay-node",
        action="append",
        default=[],
        metavar="NODE",
        help="audit every locked zero-cost state/overlay/<feed>/<node> topic and report exact present/absent feeds",
    )
    parser.add_argument(
        "--require-catalog-complete",
        action="store_true",
        help="when used with --catalog-overlay-node, fail if any zero-cost catalog feed has no indexed mirror",
    )
    parser.add_argument(
        "--require-installed-overlay-catalog",
        action="store_true",
        help=(
            "when used with --catalog-overlay-node, require every default-on zero-cost "
            "overlay topic to have a fresh installed-seat mirror; status-only mirrors "
            "prove presence but remain ready=false without fresh fetched_at_ms"
        ),
    )
    parser.add_argument("--airspace-node", action="append", default=[], metavar="NODE")
    parser.add_argument(
        "--require-airspace-ready",
        action="store_true",
        help="require state/airspace/<node> to carry a fresh ready scanner observation",
    )
    parser.add_argument(
        "--require-airspace-contacts",
        action="store_true",
        help="require state/airspace/<node> to retain at least one contact",
    )
    parser.add_argument(
        "--vdi-session",
        metavar="SESSION_ID",
        help=f"validate a retained session lifecycle on {VDI_SESSION_TOPIC}",
    )
    parser.add_argument("--expected-vdi-serving-node")
    parser.add_argument("--expected-vdi-vm")
    parser.add_argument("--expected-vdi-client-peer")
    parser.add_argument(
        "--require-vdi-status-rail",
        action="store_true",
        help=(
            f"require a matching {VDI_SESSION_TOPIC} open lifecycle row that the "
            "Construct status rail can project"
        ),
    )
    parser.add_argument(
        "--require-vdi-status-rail-live",
        action="store_true",
        help=(
            f"require {VDI_SESSION_TOPIC} to fold the session to the LIVE rail "
            "state (a transport-connected acceptance proof, not a pixel proof)"
        ),
    )
    parser.add_argument(
        "--require-same-host",
        action="store_true",
        help="when validating a vehicle plus overlays or airspace, require every mirror host to match the vehicle node",
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test:
        _self_test()
        return 0
    if (
        not args.vehicle_node
        and not args.overlay
        and not args.catalog_overlay_node
        and not args.airspace_node
        and not args.vdi_session
    ):
        parser.error(
            "provide --vehicle-node, --overlay, --catalog-overlay-node, --airspace-node, "
            "--vdi-session, or --self-test"
        )
    if args.require_same_host and not args.vehicle_node:
        parser.error("--require-same-host requires --vehicle-node")
    if args.require_same_host and not (args.overlay or args.catalog_overlay_node or args.airspace_node):
        parser.error("--require-same-host requires at least one --overlay, --catalog-overlay-node, or --airspace-node")
    if args.require_catalog_feed and not args.overlay:
        parser.error("--require-catalog-feed requires at least one --overlay")
    if args.require_catalog_complete and not args.catalog_overlay_node:
        parser.error("--require-catalog-complete requires --catalog-overlay-node")
    if args.require_installed_overlay_catalog and not args.catalog_overlay_node:
        parser.error("--require-installed-overlay-catalog requires --catalog-overlay-node")
    if (args.require_airspace_ready or args.require_airspace_contacts) and not args.airspace_node:
        parser.error("airspace expectations require --airspace-node")
    vdi_args = (
        args.require_vdi_status_rail,
        args.require_vdi_status_rail_live,
        args.expected_vdi_serving_node,
        args.expected_vdi_vm,
        args.expected_vdi_client_peer,
    )
    if any(vdi_args) and not args.vdi_session:
        parser.error("VDI session expectations require --vdi-session")
    if args.max_age_seconds < 0:
        parser.error("--max-age-seconds must be non-negative")
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
