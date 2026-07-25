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
        --require-ready --max-age-seconds 300
    verify-live-mirrors.py --vehicle-node workstation --overlay \
        state/overlay/usgs-earthquakes/workstation --require-online \
        --require-same-host
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
VEHICLE_PREFIX = "state/vehicle/"
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


class MirrorError(Exception):
    """A fail-closed mirror validation error."""


def _now_ms() -> int:
    return int(time.time() * 1000)


def _finite_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def _read_latest(bus_root: Path, topic: str) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any], str]:
    """Return (index row, envelope, payload, sha256) for the indexed latest row."""
    if not topic or topic.startswith("/") or ".." in Path(topic).parts:
        raise MirrorError(f"invalid topic: {topic!r}")
    root = bus_root.resolve(strict=True)
    db_path = root / "index.sqlite"
    if not db_path.is_file():
        raise MirrorError(f"Bus index missing: {db_path}")

    # URI mode=ro is intentional: this helper must not initialize, migrate, or
    # otherwise mutate a live Bus while producing evidence.
    uri = f"file:{quote(str(db_path), safe='/')}?mode=ro"
    try:
        with sqlite3.connect(uri, uri=True, timeout=5.0) as conn:
            row = conn.execute(
                "SELECT ulid, topic, ts_unix_ms, file_path, body "
                "FROM messages WHERE topic = ? ORDER BY ulid DESC LIMIT 1",
                (topic,),
            ).fetchone()
    except sqlite3.Error as exc:
        raise MirrorError(f"read-only Bus index query failed: {exc}") from exc
    if row is None:
        raise MirrorError(f"no indexed message for {topic}")

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
    for key in ("alerts", "aircraft", "cameras", "events", "features", "forecasts", "stations", "transit", "hotspots", "quakes"):
        value = payload.get(key)
        if isinstance(value, list):
            return len(value)
    return None


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


def validate_overlay(
    bus_root: Path,
    topic: str,
    now_ms: int,
    max_age_ms: int,
    require_ready: bool,
    expected_host: str | None = None,
) -> dict[str, Any]:
    if not topic.startswith(OVERLAY_PREFIX):
        raise MirrorError(f"overlay topic must start with {OVERLAY_PREFIX!r}: {topic}")
    row, _envelope, payload, digest = _read_latest(bus_root, topic)
    result = _base_result(topic, row, payload, digest)
    errors: list[str] = []
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
    fetched_at = payload.get("fetched_at_ms")
    if fetched_at is None:
        errors.append("missing fetched_at_ms; publication alone is not feed evidence")
        age_ms = 0
    else:
        age_ms, age_errors = _age_check(payload, now_ms, max_age_ms, required="fetched_at_ms")
        errors.extend(age_errors)
    availability = payload.get("availability")
    availability_text = availability if isinstance(availability, str) else None
    ready = fetched_at is not None and availability_text not in {"unconfigured", "secret_store_error", "error", "paused"}
    license_tier_errors = _license_tier_errors(payload)
    errors.extend(license_tier_errors)
    if require_ready:
        if not ready:
            errors.append(f"overlay is not ready (availability={availability_text!r})")
        if not isinstance(payload.get("attribution"), str) or not payload["attribution"]:
            errors.append("missing attribution")
    result.update(
        {
            "kind": "overlay",
            "age_ms": age_ms,
            "fresh": fetched_at is not None and not any("stale" in error or "future" in error for error in errors),
            "ready": ready,
            "availability": availability_text,
            "fetched_at_ms": fetched_at,
            "record_count": _collection_count(payload),
            "license_tier": payload.get("license_tier"),
            "license_tier_allowed": not license_tier_errors,
            "attribution": payload.get("attribution"),
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
    for topic in args.overlay:
        try:
            result = validate_overlay(
                root,
                topic,
                now_ms,
                max_age_ms,
                args.require_ready,
                args.vehicle_node if args.require_same_host else None,
            )
        except MirrorError as exc:
            result = {"kind": "overlay", "topic": topic, "errors": [str(exc)]}
        results.append(result)
        failures.extend(result.get("errors", []))
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

        def add(ulid: str, topic: str, payload: dict[str, Any]) -> None:
            rel = f"{topic}/{ulid}.json"
            path = root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            body = json.dumps(payload, separators=(",", ":"))
            path.write_text(
                json.dumps(
                    {
                        "ulid": ulid,
                        "topic": topic,
                        "priority": "default",
                        "title": None,
                        "body": body,
                        "ts_unix_ms": now_ms - 10_000,
                        "file_path": rel,
                        "actions": [],
                        "reply_to": None,
                    }
                )
            )
            conn.execute(
                "INSERT INTO messages VALUES (?, ?, ?, ?, ?, ?, ?)",
                (ulid, topic, "default", None, body, now_ms - 10_000, rel),
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
            "state/overlay/test-feed/test-node",
            {
                "host": "test-node",
                "fetched_at_ms": now_ms - 10_000,
                "events": [],
                "license_tier": "open-data-attribution",
                "attribution": "fixture",
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
        conn.commit()
        conn.close()

        vehicle = validate_vehicle(root, "test-node", now_ms, 30_000, True, False, "MG90")
        assert not vehicle["errors"], vehicle
        assert vehicle["online"] is True and vehicle["has_fix"] is False
        overlay = validate_overlay(root, "state/overlay/test-feed/test-node", now_ms, 30_000, True)
        assert not overlay["errors"], overlay
        assert overlay["ready"] is True and overlay["record_count"] == 0
        blocked_license = validate_overlay(
            root, "state/overlay/non-commercial/test-node", now_ms, 30_000, True
        )
        assert blocked_license["license_tier_allowed"] is False, blocked_license
        assert any("zero-cost" in error for error in blocked_license["errors"]), blocked_license
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
            root, missing_host_topic, now_ms, 30_000, True
        )
        assert any(
            "payload host is missing" in error for error in missing_host_result["errors"]
        ), missing_host_result
        same_host = validate_overlay(
            root,
            "state/overlay/test-feed/test-node",
            now_ms,
            30_000,
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
            overlay=["state/overlay/test-feed/test-node"],
            require_online=True,
            require_fix=False,
            expected_model="MG90",
            require_ready=True,
            require_same_host=True,
        )
        with redirect_stdout(cli_output):
            assert run(cli_args) == 0
        cli_report = json.loads(cli_output.getvalue())
        assert cli_report["same_host_required"] is True, cli_report
        assert cli_report["ok"] is True, cli_report
        mismatched = validate_overlay(
            root,
            "state/overlay/test-feed/test-node",
            now_ms,
            30_000,
            True,
            "other-node",
        )
        assert any("vehicle host" in error for error in mismatched["errors"]), mismatched
        stale = validate_overlay(root, "state/overlay/test-feed/test-node", now_ms, 1_000, True)
        assert stale["errors"], stale

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
        "--require-same-host",
        action="store_true",
        help="when validating a vehicle plus overlays, require every overlay host to match the vehicle node",
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test:
        _self_test()
        return 0
    if not args.vehicle_node and not args.overlay:
        parser.error("provide --vehicle-node, --overlay, or --self-test")
    if args.require_same_host and not args.vehicle_node:
        parser.error("--require-same-host requires --vehicle-node")
    if args.require_same_host and not args.overlay:
        parser.error("--require-same-host requires at least one --overlay")
    if args.max_age_seconds < 0:
        parser.error("--max-age-seconds must be non-negative")
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
