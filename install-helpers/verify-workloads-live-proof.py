#!/usr/bin/env python3
"""Read-only Workloads live evidence collector for WL-ARCH-010.

This helper is deliberately a verifier, not an actuator.  It never publishes Bus
messages, never calls mutating `virsh`/`podman`/`systemctl` verbs, and never reads
the cloud-arm secret bytes.  It answers the narrow release-gate question: which
parts of the live Workloads path are actually observable on this placement host?

Typical closure run on a placement host::

    install-helpers/verify-workloads-live-proof.py --node "$(hostname)" --require-all

Useful partial proof while KVM firmware is known unavailable::

    install-helpers/verify-workloads-live-proof.py --node "$(hostname)" \
        --require-cloud-arm --require-cloud-mirror --require-podman

Exit status is non-zero only when a requested requirement is blocked or errored.
Without `--require-*` flags the helper prints an honest read-only inventory and
returns success unless the helper itself was invoked incorrectly.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass, field
import hashlib
import json
import os
from pathlib import Path
from urllib.parse import quote
import shutil
import socket
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
from typing import Any


DEFAULT_BUS_ROOT = Path("/run/mde-bus")
DEFAULT_CREDENTIAL_PATH = Path("/etc/credstore.encrypted/cloud-arm-key")
DEFAULT_LIBVIRT_URI = "qemu:///system"
DEFAULT_NETWORK = "default"
DEFAULT_POOL = "mde-vms"
DEFAULT_ROLE_PATH = Path("/var/lib/mde/role.toml")

CLOUD_PREFIX = "state/cloud/"
WORKLOAD_OPERATION_TOPIC = "action/workload/operation"
ONBOARD_APPLY_ACTION_TOPIC = "action/onboard/apply"
ONBOARD_APPLY_EVENT_TOPIC = "event/onboard/apply"
WORKLOAD_OPERATION_ACTIONS = {
    "start_and_attach",
    "start",
    "stop",
    "restart",
    "destroy",
    "pause",
    "resume",
    "open",
    "reconcile",
    "cancel",
}

MAX_MESSAGE_BYTES = 1_048_576
MAX_DROPIN_BYTES = 16 * 1024
MAX_TOPIC_SCAN_ROWS = 256
REQUIRED_CLOUD_TOOLS = ("opentofu", "ansible", "libvirt")
WORKLOAD_OPERATION_FUTURE_SKEW_MS = 30 * 1000
ONBOARD_ACK_FUTURE_SKEW_MS = 30 * 1000
WORKLOAD_STATE_PREFIX = "state/workloads/"
WORKLOAD_STATE_FUTURE_SKEW_MS = 30 * 1000
WORKLOAD_CONTRACT_SCHEMA_VERSION = 1
MAX_WORKLOADS_PER_NODE = 256
MAX_WORKLOAD_IDENTIFIER_BYTES = 128
MAX_WORKLOAD_TEXT_BYTES = 512
MAX_WORKLOAD_ATTEMPTS = 32

WORKLOAD_BACKENDS = {"libvirt_virtqemud", "quadlet_systemd"}
WORKLOAD_PHASES = {
    "queued",
    "validating",
    "admitting",
    "defining",
    "starting",
    "waiting_for_guest",
    "waiting_for_service",
    "preparing_display",
    "waiting_for_first_frame",
    "ready",
    "stopping",
    "completed",
    "failed",
    "cancelled",
}
WORKLOAD_TERMINAL_PHASES = {"completed", "failed", "cancelled"}
WORKLOAD_ADMITTED_PHASES = {
    "admitting",
    "defining",
    "starting",
    "waiting_for_guest",
    "waiting_for_service",
    "preparing_display",
    "waiting_for_first_frame",
    "ready",
    "completed",
}
WORKLOAD_POWER_STATES = {
    "defined",
    "starting",
    "running",
    "paused",
    "stopping",
    "stopped",
    "failed",
}
WORKLOAD_READINESS = {
    "unknown",
    "waiting_for_placement",
    "waiting_for_guest",
    "waiting_for_service",
    "preparing_display",
    "ready",
    "degraded",
    "unavailable",
    "failed",
}
WORKLOAD_HEALTH = {"unknown", "healthy", "degraded", "failed"}
WORKLOAD_PRESSURE = {"normal", "constrained", "saturated"}
WORKLOAD_ATTACHMENT_PROTOCOLS = {
    "qemu_display1_dmabuf",
    "rdp",
    "spice",
    "vnc",
    "sunshine",
    "web_rtc",
    "logs",
    "terminal",
    "ports",
}
WORKSTATION_ROLE_ALIASES = {
    "workstation",
    "full",
    "xcpng",
    "xcp-ng",
    "server",
    "headless",
}

BAD_REQUIRED_STATUSES = {"blocked", "error"}


class ProofError(Exception):
    """A fail-closed proof/inspection error."""


@dataclass
class Check:
    name: str
    status: str
    detail: str
    required: bool = False
    evidence: dict[str, Any] = field(default_factory=dict)

    def to_json(self) -> dict[str, Any]:
        out: dict[str, Any] = {
            "name": self.name,
            "status": self.status,
            "required": self.required,
            "detail": self.detail,
        }
        if self.evidence:
            out["evidence"] = self.evidence
        return out


@dataclass
class CommandResult:
    argv: list[str]
    returncode: int
    stdout: str
    stderr: str
    timed_out: bool = False

    @property
    def ok(self) -> bool:
        return self.returncode == 0 and not self.timed_out

    def one_line(self) -> str:
        text = (self.stdout or self.stderr).strip().splitlines()
        if not text:
            return f"exit {self.returncode}"
        return text[0][:240]


def now_ms() -> int:
    return int(time.time() * 1000)


def _short_host() -> str:
    host = socket.gethostname().strip()
    return host.split(".", 1)[0] if host else "unknown"


def _append_node_candidate(candidates: list[str], value: str) -> None:
    value = value.strip()
    if not value:
        return
    variants = [value]
    if value.startswith("peer:"):
        variants.append(value.removeprefix("peer:"))
    else:
        variants.append(f"peer:{value}")
    for variant in variants:
        if variant and variant not in candidates:
            candidates.append(variant)


def node_candidates(node: str | None) -> list[str]:
    """Return plausible node ids without inventing evidence."""
    candidates: list[str] = []
    if node:
        _append_node_candidate(candidates, node)
        return candidates
    host = _short_host()
    fqdn = socket.gethostname().strip()
    _append_node_candidate(candidates, host)
    if fqdn:
        _append_node_candidate(candidates, fqdn)
    return candidates


def run_command(argv: list[str], timeout: float) -> CommandResult:
    try:
        completed = subprocess.run(
            argv,
            text=True,
            capture_output=True,
            timeout=timeout,
            check=False,
        )
        return CommandResult(argv, completed.returncode, completed.stdout, completed.stderr)
    except FileNotFoundError as exc:
        return CommandResult(argv, 127, "", str(exc))
    except subprocess.TimeoutExpired as exc:
        stdout = exc.stdout if isinstance(exc.stdout, str) else ""
        stderr = exc.stderr if isinstance(exc.stderr, str) else ""
        return CommandResult(argv, 124, stdout, stderr or "timeout", timed_out=True)


def _split_key_values(text: str) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in text.splitlines():
        if ":" not in line:
            continue
        key, value = line.split(":", 1)
        values[key.strip().lower()] = value.strip()
    return values


def _read_small_proc_file(path: Path, max_bytes: int) -> str:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
    fd: int | None = None
    try:
        fd = os.open(path, flags)
        with os.fdopen(fd, "rb") as handle:
            fd = None
            raw = handle.read(max_bytes + 1)
            if len(raw) > max_bytes:
                raise ProofError(f"{path} exceeds {max_bytes} bytes")
            return raw.decode("utf-8", errors="replace")
    finally:
        if fd is not None:
            os.close(fd)


def _bounded_text(path: Path, max_bytes: int) -> str:
    st = path.lstat()
    if stat.S_ISLNK(st.st_mode):
        raise ProofError(f"{path} is a symlink")
    if not stat.S_ISREG(st.st_mode):
        raise ProofError(f"{path} is not a regular file")
    if st.st_size > max_bytes:
        raise ProofError(f"{path} exceeds {max_bytes} bytes")
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    fd: int | None = None
    try:
        fd = os.open(path, flags)
        with os.fdopen(fd, "rb") as handle:
            fd = None
            raw = handle.read(max_bytes + 1)
            if len(raw) > max_bytes:
                raise ProofError(f"{path} exceeds {max_bytes} bytes")
            return raw.decode("utf-8")
    finally:
        if fd is not None:
            os.close(fd)


def _validate_topic(topic: str) -> None:
    if not topic or topic.startswith("/") or ".." in Path(topic).parts:
        raise ProofError(f"invalid topic: {topic!r}")


def _open_bus_root(bus_root: Path) -> tuple[Path, Path]:
    try:
        root = bus_root.resolve(strict=True)
    except FileNotFoundError as exc:
        raise ProofError(f"Bus root missing: {bus_root}") from exc
    except OSError as exc:
        raise ProofError(f"cannot inspect Bus root {bus_root}: {exc}") from exc
    db_path = root / "index.sqlite"
    if not db_path.is_file():
        raise ProofError(f"Bus index missing: {db_path}")
    return root, db_path


def _read_indexed_row(
    root: Path, expected_topic: str, row: tuple[Any, ...]
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any] | None, str]:
    ulid, topic, ts_unix_ms, file_path, body = row
    if topic != expected_topic:
        raise ProofError(f"index returned unexpected topic {topic!r}")
    if not isinstance(file_path, str):
        raise ProofError(f"indexed file path is not a string: {file_path!r}")
    relative = Path(file_path)
    if (
        relative.is_absolute()
        or not relative.parts
        or any(part in {"", ".", ".."} for part in relative.parts)
        or any("\x00" in part for part in relative.parts)
    ):
        raise ProofError(f"indexed file path is not relative: {file_path!r}")
    message_path = root.joinpath(*relative.parts)
    current = root
    for index, part in enumerate(relative.parts):
        current = current / part
        try:
            metadata = current.lstat()
        except OSError as exc:
            raise ProofError(f"indexed message file is missing: {file_path!r}") from exc
        if stat.S_ISLNK(metadata.st_mode):
            raise ProofError(f"indexed message path contains a symlink: {file_path!r}")
        if index < len(relative.parts) - 1 and not stat.S_ISDIR(metadata.st_mode):
            raise ProofError(f"indexed message parent is not a directory: {file_path!r}")
    if not message_path.is_file():
        raise ProofError(f"indexed message file is not a regular file: {file_path!r}")
    st = message_path.stat()
    if st.st_size > MAX_MESSAGE_BYTES:
        raise ProofError(f"indexed message file exceeds {MAX_MESSAGE_BYTES} bytes: {file_path!r}")
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    fd: int | None = None
    try:
        fd = os.open(message_path, flags)
        with os.fdopen(fd, "rb") as handle:
            fd = None
            if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
                raise ProofError(f"indexed message file is not regular: {file_path!r}")
            raw = handle.read(MAX_MESSAGE_BYTES + 1)
            if len(raw) > MAX_MESSAGE_BYTES:
                raise ProofError(f"indexed message file exceeds {MAX_MESSAGE_BYTES} bytes")
    finally:
        if fd is not None:
            os.close(fd)
    try:
        envelope = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ProofError(f"cannot decode indexed message {file_path}: {exc}") from exc
    if not isinstance(envelope, dict):
        raise ProofError(f"indexed message {file_path} is not a JSON object")
    if envelope.get("ulid") != ulid or envelope.get("topic") != topic:
        raise ProofError(f"index/envelope identity mismatch for {expected_topic}")
    if envelope.get("file_path") != file_path:
        raise ProofError(f"envelope file_path mismatch for {expected_topic}")
    if envelope.get("ts_unix_ms") != ts_unix_ms or envelope.get("body") != body:
        raise ProofError(f"index/envelope payload mismatch for {expected_topic}")
    payload = None
    if body is not None:
        if not isinstance(body, str):
            raise ProofError(f"indexed body is not JSON text for {expected_topic}")
        try:
            decoded = json.loads(body)
        except json.JSONDecodeError as exc:
            raise ProofError(f"message body is not valid JSON for {expected_topic}: {exc}") from exc
        if isinstance(decoded, dict):
            payload = decoded
        else:
            raise ProofError(f"message body is not a JSON object for {expected_topic}")
    return (
        {
            "ulid": ulid,
            "topic": topic,
            "ts_unix_ms": ts_unix_ms,
            "file_path": file_path,
        },
        envelope,
        payload,
        hashlib.sha256(raw).hexdigest(),
    )


def read_topic_rows(
    bus_root: Path, topic: str, limit: int = 1
) -> list[tuple[dict[str, Any], dict[str, Any], dict[str, Any] | None, str]]:
    _validate_topic(topic)
    if limit <= 0 or limit > MAX_TOPIC_SCAN_ROWS:
        raise ProofError(f"invalid scan limit {limit}; max {MAX_TOPIC_SCAN_ROWS}")
    root, db_path = _open_bus_root(bus_root)
    uri = f"file:{quote(str(db_path), safe='/')}?mode=ro"
    try:
        with sqlite3.connect(uri, uri=True, timeout=5.0) as conn:
            rows = conn.execute(
                "SELECT ulid, topic, ts_unix_ms, file_path, body "
                "FROM messages WHERE topic = ? ORDER BY ulid DESC LIMIT ?",
                (topic, limit),
            ).fetchall()
    except sqlite3.Error as exc:
        raise ProofError(f"read-only Bus index query failed: {exc}") from exc
    return [_read_indexed_row(root, topic, row) for row in rows]


def list_topics(bus_root: Path, prefix: str, limit: int = MAX_TOPIC_SCAN_ROWS) -> list[str]:
    if limit <= 0 or limit > MAX_TOPIC_SCAN_ROWS:
        raise ProofError(f"invalid topic scan limit {limit}; max {MAX_TOPIC_SCAN_ROWS}")
    root, db_path = _open_bus_root(bus_root)
    _ = root
    uri = f"file:{quote(str(db_path), safe='/')}?mode=ro"
    try:
        with sqlite3.connect(uri, uri=True, timeout=5.0) as conn:
            rows = conn.execute(
                "SELECT DISTINCT topic FROM messages WHERE topic LIKE ? "
                "ORDER BY topic LIMIT ?",
                (f"{prefix}%", limit),
            ).fetchall()
    except sqlite3.Error as exc:
        raise ProofError(f"read-only Bus topic query failed: {exc}") from exc
    return [row[0] for row in rows if isinstance(row[0], str)]


def select_cloud_topic(bus_root: Path, node: str | None) -> tuple[str | None, list[str], list[str]]:
    topics = list_topics(bus_root, CLOUD_PREFIX)
    candidates = node_candidates(node)
    for candidate in candidates:
        topic = f"{CLOUD_PREFIX}{candidate}"
        if topic in topics:
            return topic, topics, candidates
    if len(topics) == 1:
        return topics[0], topics, candidates
    return None, topics, candidates


def first_matching_payload(
    rows: list[tuple[dict[str, Any], dict[str, Any], dict[str, Any] | None, str]],
    candidates: list[str],
    host_field: str,
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any], str] | None:
    for index, envelope, payload, digest in rows:
        if payload is None:
            continue
        host = payload.get(host_field)
        if isinstance(host, str) and host in candidates:
            return index, envelope, payload, digest
    return None


def check_services(args: argparse.Namespace, checks: list[Check], require_mackesd: bool) -> None:
    if shutil.which("systemctl") is None:
        status = "blocked" if require_mackesd or args.require_seat else "warn"
        checks.append(Check("systemd", status, "systemctl is unavailable", require_mackesd or args.require_seat))
        return
    service_requirements = {
        **{
            f"mackesd-{group}.service": require_mackesd
            for group in ("control", "observation", "actions", "data", "compute", "integrations")
        },
        "mde-shell-egui.service": args.require_all or args.require_seat,
    }
    for unit, required in service_requirements.items():
        result = run_command(["systemctl", "is-active", unit], args.command_timeout)
        nrestarts = run_command(
            ["systemctl", "show", "--property=NRestarts", "--value", unit],
            args.command_timeout,
        )
        restart_detail = ""
        if nrestarts.ok and nrestarts.stdout.strip():
            restart_detail = f", NRestarts={nrestarts.stdout.strip()}"
        if result.ok and result.stdout.strip() == "active":
            checks.append(Check(unit, "ok", f"active{restart_detail}", required))
        else:
            status = "blocked" if required else "warn"
            checks.append(Check(unit, status, f"not active ({result.one_line()}){restart_detail}", required))


def check_cloud_arm(args: argparse.Namespace, checks: list[Check]) -> None:
    required = args.require_all or args.require_cloud_arm
    path = Path(args.credential_path)
    try:
        st = path.lstat()
        if stat.S_ISLNK(st.st_mode):
            checks.append(Check("cloud-arm credential", "blocked", f"{path} is a symlink", required))
            return
        if not stat.S_ISREG(st.st_mode):
            checks.append(Check("cloud-arm credential", "blocked", f"{path} is not a regular file", required))
            return
        mode = stat.S_IMODE(st.st_mode)
        owner = "root" if st.st_uid == 0 else f"uid:{st.st_uid}"
        if mode & 0o077:
            status = "blocked" if required else "warn"
            checks.append(
                Check(
                    "cloud-arm credential",
                    status,
                    f"{path} exists but mode {mode:04o} is group/world accessible",
                    required,
                    {"path": str(path), "mode": f"{mode:04o}", "owner": owner},
                )
            )
            return
        checks.append(
            Check(
                "cloud-arm credential",
                "ok",
                f"{path} exists as a root-only regular encrypted credential ({owner}, {mode:04o}); secret bytes not read",
                required,
                {"path": str(path), "mode": f"{mode:04o}", "owner": owner},
            )
        )
    except FileNotFoundError:
        checks.append(Check("cloud-arm credential", "blocked", f"{path} is missing", required))
        return
    except OSError as exc:
        checks.append(Check("cloud-arm credential", "error", f"cannot inspect {path}: {exc}", required))
        return

    dropins = [
        Path("/etc/systemd/system/mackesd-compute.service.d/50-cloud-arm-credential.conf"),
        Path("/etc/systemd/system/mde-shell-egui.service.d/50-cloud-arm-credential.conf"),
    ]
    missing: list[str] = []
    malformed: list[str] = []
    for dropin in dropins:
        try:
            text = _bounded_text(dropin, MAX_DROPIN_BYTES)
        except FileNotFoundError:
            missing.append(str(dropin))
            continue
        except ProofError as exc:
            malformed.append(str(exc))
            continue
        if "LoadCredentialEncrypted=cloud-arm-key:" not in text:
            malformed.append(f"{dropin} lacks LoadCredentialEncrypted=cloud-arm-key")
    if missing or malformed:
        status = "blocked" if required else "warn"
        detail_parts = []
        if missing:
            detail_parts.append("missing " + ", ".join(missing))
        if malformed:
            detail_parts.append("; ".join(malformed))
        checks.append(Check("cloud-arm systemd drop-ins", status, "; ".join(detail_parts), required))
    else:
        checks.append(Check("cloud-arm systemd drop-ins", "ok", "mackesd + shell drop-ins reference cloud-arm-key", required))


def check_cloud_mirror(args: argparse.Namespace, checks: list[Check]) -> None:
    required = args.require_all or args.require_cloud_mirror
    bus_root = Path(args.bus_root)
    try:
        topic, topics, candidates = select_cloud_topic(bus_root, args.node)
        if topic is None:
            detail = "no matching state/cloud mirror"
            if topics:
                detail += f" for {candidates}; available: {', '.join(topics[:8])}"
            checks.append(Check("state/cloud mirror", "blocked" if required else "warn", detail, required))
            return
        rows = read_topic_rows(bus_root, topic, 1)
        if not rows:
            checks.append(Check("state/cloud mirror", "blocked" if required else "warn", f"no indexed rows for {topic}", required))
            return
        index, _envelope, payload, digest = rows[0]
        if payload is None:
            checks.append(Check("state/cloud mirror", "blocked" if required else "warn", f"{topic} has no JSON body", required))
            return
        published = payload.get("published_at_ms")
        if not isinstance(published, int):
            checks.append(Check("state/cloud mirror", "blocked" if required else "warn", f"{topic} has no integer published_at_ms", required))
            return
        age_ms = now_ms() - published
        age_s = max(0.0, age_ms / 1000.0)
        adapter = payload.get("adapter")
        health_rows = payload.get("health") if isinstance(payload.get("health"), list) else []
        health = {
            row.get("service_type"): row.get("state")
            for row in health_rows
            if isinstance(row, dict) and isinstance(row.get("service_type"), str)
        }
        missing_tools = [tool for tool in REQUIRED_CLOUD_TOOLS if tool not in health]
        down_tools = [f"{tool}={health.get(tool)}" for tool in REQUIRED_CLOUD_TOOLS if health.get(tool) != "up"]
        apply_armed = payload.get("apply_armed") is True
        workloads = payload.get("workloads") if isinstance(payload.get("workloads"), list) else []
        resources = payload.get("resources") if isinstance(payload.get("resources"), list) else []
        blockers: list[str] = []
        if adapter != "construct_cloud":
            blockers.append(f"adapter={adapter!r}, expected construct_cloud")
        if age_s > args.max_cloud_age_seconds:
            blockers.append(f"stale age={age_s:.1f}s > {args.max_cloud_age_seconds}s")
        if missing_tools:
            blockers.append("missing health rows: " + ", ".join(missing_tools))
        if down_tools:
            blockers.append("backend not up: " + ", ".join(down_tools))
        if not apply_armed:
            blockers.append("apply_armed=false")
        evidence = {
            "topic": topic,
            "ulid": index["ulid"],
            "sha256": digest,
            "age_seconds": round(age_s, 3),
            "adapter": adapter,
            "health": health,
            "apply_armed": apply_armed,
            "workload_count": len(workloads),
            "resource_table_count": len(resources),
        }
        if blockers:
            checks.append(Check("state/cloud mirror", "blocked" if required else "warn", "; ".join(blockers), required, evidence))
        else:
            checks.append(Check("state/cloud mirror", "ok", f"{topic} fresh, construct_cloud, backends up, apply armed", required, evidence))
    except ProofError as exc:
        checks.append(Check("state/cloud mirror", "error" if required else "warn", str(exc), required))


def select_workload_topic(
    bus_root: Path, node: str | None
) -> tuple[str | None, list[str], list[str]]:
    """Select one node-scoped typed Workload projection without guessing."""
    topics = list_topics(bus_root, WORKLOAD_STATE_PREFIX)
    candidates = node_candidates(node)
    for candidate in candidates:
        topic = f"{WORKLOAD_STATE_PREFIX}{candidate}"
        if topic in topics:
            return topic, topics, candidates
    if len(topics) == 1:
        return topics[0], topics, candidates
    return None, topics, candidates


def _valid_workload_identifier(value: Any) -> bool:
    return (
        isinstance(value, str)
        and 0 < len(value.encode("utf-8")) <= MAX_WORKLOAD_IDENTIFIER_BYTES
        and all(char.isascii() and (char.isalnum() or char in "-_.:") for char in value)
    )


def _valid_workload_text(value: Any) -> bool:
    return (
        isinstance(value, str)
        and bool(value.strip())
        and len(value.encode("utf-8")) <= MAX_WORKLOAD_TEXT_BYTES
        and not any(ord(char) < 0x20 for char in value)
    )


def _is_workload_integer(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _key_blockers(
    value: Any, required: set[str], optional: set[str], label: str
) -> list[str]:
    if not isinstance(value, dict):
        return [f"{label} is not an object"]
    keys = set(value)
    blockers = [f"{label} missing {key}" for key in sorted(required - keys)]
    blockers.extend(f"{label} has unknown field {key}" for key in sorted(keys - required - optional))
    return blockers


def _validate_workload_resources(value: Any, label: str) -> list[str]:
    blockers = _key_blockers(value, {"vcpu", "memory_mb", "disk_gb"}, set(), label)
    if blockers or not isinstance(value, dict):
        return blockers
    bounds = {"vcpu": (1, 64), "memory_mb": (512, 262_144), "disk_gb": (1, 4_096)}
    for field, (minimum, maximum) in bounds.items():
        number = value.get(field)
        if not _is_workload_integer(number) or not minimum <= number <= maximum:
            blockers.append(f"{label}.{field} is outside the bounded contract")
    return blockers


def _validate_workload_signals(value: Any, label: str) -> list[str]:
    required = {
        "guest_agent",
        "network",
        "service",
        "display",
        "application",
        "health",
        "pressure",
        "progress_percent",
    }
    blockers = _key_blockers(value, required, set(), label)
    if blockers or not isinstance(value, dict):
        return blockers
    for field in ("guest_agent", "network", "service", "display", "application"):
        if value.get(field) not in WORKLOAD_READINESS:
            blockers.append(f"{label}.{field} is not a known readiness value")
    if value.get("health") not in WORKLOAD_HEALTH:
        blockers.append(f"{label}.health is not a known health value")
    if value.get("pressure") not in WORKLOAD_PRESSURE:
        blockers.append(f"{label}.pressure is not a known pressure value")
    progress = value.get("progress_percent")
    if not _is_workload_integer(progress) or not 0 <= progress <= 100:
        blockers.append(f"{label}.progress_percent is outside 0..100")
    return blockers


def _validate_workload_attachment(
    value: Any, status_workload_id: str, now: int, label: str
) -> list[str]:
    required = {
        "schema_version",
        "lease_id",
        "nonce",
        "workload_id",
        "generation",
        "protocol",
        "expires_at_ms",
    }
    blockers = _key_blockers(value, required, set(), label)
    if blockers or not isinstance(value, dict):
        return blockers
    if value.get("schema_version") != WORKLOAD_CONTRACT_SCHEMA_VERSION:
        blockers.append(f"{label}.schema_version is unsupported")
    for field in ("lease_id", "nonce", "workload_id"):
        if not _valid_workload_identifier(value.get(field)):
            blockers.append(f"{label}.{field} is not a bounded identifier")
    if value.get("workload_id") != status_workload_id:
        blockers.append(f"{label}.workload_id does not match its status")
    if value.get("protocol") not in WORKLOAD_ATTACHMENT_PROTOCOLS:
        blockers.append(f"{label}.protocol is unknown")
    generation = value.get("generation")
    if not _is_workload_integer(generation) or generation <= 0:
        blockers.append(f"{label}.generation is invalid")
    expires_at_ms = value.get("expires_at_ms")
    if not _is_workload_integer(expires_at_ms) or expires_at_ms <= now:
        blockers.append(f"{label}.expires_at_ms is missing or expired")
    return blockers


def _validate_workload_status(status: Any, index: int, now: int) -> list[str]:
    required = {
        "schema_version",
        "request_id",
        "workload_id",
        "backend",
        "resources",
        "generation",
        "phase",
        "power",
        "readiness",
        "retryable",
    }
    optional = {"image_ref", "signals", "attempt", "next_retry_at_ms", "reason", "remediation", "attachment"}
    label = f"workloads[{index}]"
    blockers = _key_blockers(status, required, optional, label)
    if blockers or not isinstance(status, dict):
        return blockers
    if status.get("schema_version") != WORKLOAD_CONTRACT_SCHEMA_VERSION:
        blockers.append(f"{label}.schema_version is unsupported")
    for field in ("request_id", "workload_id"):
        if not _valid_workload_identifier(status.get(field)):
            blockers.append(f"{label}.{field} is not a bounded identifier")
    if status.get("backend") not in WORKLOAD_BACKENDS:
        blockers.append(f"{label}.backend is unknown")
    blockers.extend(_validate_workload_resources(status.get("resources"), f"{label}.resources"))
    generation = status.get("generation")
    if not _is_workload_integer(generation) or generation <= 0:
        blockers.append(f"{label}.generation is invalid")
    phase = status.get("phase")
    if phase not in WORKLOAD_PHASES:
        blockers.append(f"{label}.phase is unknown")
    for field, allowed in (
        ("power", WORKLOAD_POWER_STATES),
        ("readiness", WORKLOAD_READINESS),
    ):
        if status.get(field) not in allowed:
            blockers.append(f"{label}.{field} is unknown")
    retryable = status.get("retryable")
    if not isinstance(retryable, bool):
        blockers.append(f"{label}.retryable is not boolean")
    attempt = status.get("attempt", 0)
    if not _is_workload_integer(attempt) or not 0 <= attempt <= MAX_WORKLOAD_ATTEMPTS:
        blockers.append(f"{label}.attempt is outside 0..{MAX_WORKLOAD_ATTEMPTS}")
    next_retry_at_ms = status.get("next_retry_at_ms", 0)
    if not _is_workload_integer(next_retry_at_ms) or next_retry_at_ms < 0:
        blockers.append(f"{label}.next_retry_at_ms is invalid")
    if phase in WORKLOAD_TERMINAL_PHASES and retryable:
        blockers.append(f"{label} is terminal but marked retryable")
    if phase in WORKLOAD_TERMINAL_PHASES and next_retry_at_ms:
        blockers.append(f"{label} is terminal but has a retry schedule")
    if phase == "failed" and not _valid_workload_text(status.get("reason")):
        blockers.append(f"{label}.reason is required for failed status")
    for field in ("reason", "remediation"):
        value = status.get(field)
        if value is not None and not _valid_workload_text(value):
            blockers.append(f"{label}.{field} is not bounded text")
    image_ref = status.get("image_ref")
    if image_ref is not None and not _valid_workload_identifier(image_ref):
        blockers.append(f"{label}.image_ref is not a catalog identifier")
    if "signals" in status:
        blockers.extend(_validate_workload_signals(status["signals"], f"{label}.signals"))
    attachment = status.get("attachment")
    if attachment is not None:
        workload_id = status.get("workload_id")
        blockers.extend(
            _validate_workload_attachment(
                attachment,
                workload_id if isinstance(workload_id, str) else "",
                now,
                f"{label}.attachment",
            )
        )
    return blockers


def _parse_role_pin(text: str) -> str | None:
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        rest = line.removeprefix("role")
        if rest != line and rest.lstrip().startswith("="):
            value = rest.lstrip()[1:].strip().strip('"').strip()
            return value.lower() if value else None
    return None


def check_workload_placement(args: argparse.Namespace, checks: list[Check]) -> None:
    required = (
        args.require_all
        or args.require_workload_placement
        or args.require_workload_admission
        or args.require_workload_recovery
    )
    path = Path(args.role_path)
    try:
        role = _parse_role_pin(_bounded_text(path, MAX_DROPIN_BYTES))
        if role is None:
            checks.append(
                Check("Workload placement role", "blocked" if required else "warn", "role pin is missing or malformed", required)
            )
            return
        if role == "lighthouse":
            checks.append(
                Check("Workload placement role", "blocked" if required else "warn", "pinned lighthouse role cannot host Workloads", required, {"role": role, "path": str(path)})
            )
            return
        if role not in WORKSTATION_ROLE_ALIASES:
            checks.append(
                Check("Workload placement role", "blocked" if required else "warn", "pinned role is not a Workstation-compatible role", required, {"role": role, "path": str(path)})
            )
            return
        checks.append(
            Check("Workload placement role", "ok", "pinned role resolves to Workstation; Lighthouse placement is refused", required, {"role": "workstation", "path": str(path)})
        )
    except (FileNotFoundError, ProofError, OSError) as exc:
        checks.append(Check("Workload placement role", "blocked" if required else "warn", str(exc), required))


def check_workload_state(args: argparse.Namespace, checks: list[Check]) -> None:
    required = (
        args.require_all
        or args.require_workload_state
        or args.require_workload_admission
        or args.require_workload_recovery
    )
    bus_root = Path(args.bus_root)
    try:
        topic, topics, candidates = select_workload_topic(bus_root, args.node)
        if topic is None:
            detail = "no unambiguous state/workloads/<node> projection"
            if topics:
                detail += f" for {candidates}; available: {', '.join(topics[:8])}"
            checks.append(Check("typed Workload state", "blocked" if required else "warn", detail, required))
            return
        rows = read_topic_rows(bus_root, topic, 1)
        if not rows:
            checks.append(Check("typed Workload state", "blocked" if required else "warn", f"no indexed rows for {topic}", required))
            return
        index, _envelope, payload, digest = rows[0]
        if payload is None:
            checks.append(Check("typed Workload state", "blocked" if required else "warn", f"{topic} has no JSON body", required))
            return
        topic_node = topic.removeprefix(WORKLOAD_STATE_PREFIX)
        blockers = _key_blockers(payload, {"schema_version", "node", "observed_at_ms", "workloads"}, set(), "snapshot")
        if payload.get("schema_version") != WORKLOAD_CONTRACT_SCHEMA_VERSION:
            blockers.append("snapshot.schema_version is unsupported")
        snapshot_node = payload.get("node")
        if not _valid_workload_identifier(snapshot_node):
            blockers.append("snapshot.node is not a bounded identifier")
        if snapshot_node != topic_node:
            blockers.append("snapshot.node does not match its node-scoped topic")
        observed_at_ms = payload.get("observed_at_ms")
        age_s: float | None = None
        if not _is_workload_integer(observed_at_ms) or observed_at_ms <= 0:
            blockers.append("snapshot.observed_at_ms is invalid")
        else:
            age_ms = now_ms() - observed_at_ms
            if age_ms < -WORKLOAD_STATE_FUTURE_SKEW_MS:
                blockers.append("snapshot.observed_at_ms is too far in the future")
            age_s = max(0.0, age_ms / 1000.0)
            if age_s > args.max_workload_age_seconds:
                blockers.append(f"snapshot stale age={age_s:.1f}s > {args.max_workload_age_seconds}s")
        workloads = payload.get("workloads")
        if not isinstance(workloads, list):
            blockers.append("snapshot.workloads is not a list")
            workloads = []
        elif len(workloads) > MAX_WORKLOADS_PER_NODE:
            blockers.append(f"snapshot.workloads exceeds {MAX_WORKLOADS_PER_NODE}")
        seen_ids: set[str] = set()
        admission_count = 0
        attempted_count = 0
        retryable_count = 0
        lease_count = 0
        workload_summaries: list[str] = []
        for item, status in enumerate(workloads):
            blockers.extend(_validate_workload_status(status, item, now_ms()))
            if not isinstance(status, dict):
                continue
            workload_id = status.get("workload_id")
            if isinstance(workload_id, str):
                if workload_id in seen_ids:
                    blockers.append(f"duplicate workload_id {workload_id!r} in snapshot")
                seen_ids.add(workload_id)
                workload_summaries.append(f"{workload_id}:{status.get('phase')}")
            if status.get("phase") in WORKLOAD_ADMITTED_PHASES:
                admission_count += 1
            if _is_workload_integer(status.get("attempt", 0)) and status.get("attempt", 0) > 0:
                attempted_count += 1
            if status.get("retryable") is True:
                retryable_count += 1
            if status.get("attachment") is not None:
                lease_count += 1
        if args.expect_workload_id:
            expected = next(
                (status for status in workloads if isinstance(status, dict) and status.get("workload_id") == args.expect_workload_id),
                None,
            )
            if expected is None:
                blockers.append(f"expected workload {args.expect_workload_id!r} absent from snapshot")
            elif args.expect_workload_phase and expected.get("phase") != args.expect_workload_phase:
                blockers.append(
                    f"expected workload phase={expected.get('phase')!r}, expected {args.expect_workload_phase!r}"
                )
        elif args.expect_workload_phase:
            blockers.append("--expect-workload-phase requires --expect-workload-id")
        if args.require_workload_admission and admission_count == 0:
            blockers.append("no workload has reached the typed admitting/reconciliation phases")
        if args.require_workload_recovery and attempted_count == 0:
            blockers.append("no persisted adapter attempt was observed; restart/recovery is not proven")
        evidence = {
            "topic": topic,
            "ulid": index["ulid"],
            "sha256": digest,
            "node": snapshot_node,
            "age_seconds": round(age_s, 3) if age_s is not None else None,
            "workload_count": len(workloads),
            "workloads": workload_summaries[:32],
            "admission_phase_count": admission_count,
            "attempted_workload_count": attempted_count,
            "retryable_workload_count": retryable_count,
            "attachment_count": lease_count,
        }
        if blockers:
            checks.append(Check("typed Workload state", "blocked" if required else "warn", "; ".join(blockers), required, evidence))
        else:
            checks.append(Check("typed Workload state", "ok", f"fresh schema-{WORKLOAD_CONTRACT_SCHEMA_VERSION} projection for {snapshot_node}; bounded fields and node placement verified", required, evidence))
    except ProofError as exc:
        checks.append(Check("typed Workload state", "error" if required else "warn", str(exc), required))


def _fresh_workload_operation_target(
    args: argparse.Namespace,
    candidates: list[str],
) -> tuple[str | None, str | None, dict[str, Any]]:
    """Return the fresh authorized Workload target this proof should correlate."""
    rows = read_topic_rows(Path(args.bus_root), WORKLOAD_OPERATION_TOPIC, args.bus_scan_limit)
    for index, _envelope, payload, digest in rows:
        if payload is None:
            continue
        target_node = payload.get("target_node")
        if not isinstance(target_node, str) or target_node not in candidates:
            continue
        action = payload.get("action")
        workload_id = payload.get("workload_id")
        schema = payload.get("schema_version")
        has_token = isinstance(payload.get("armed_token"), str) and bool(payload.get("armed_token"))
        ts_unix_ms = index.get("ts_unix_ms")
        if (
            not isinstance(ts_unix_ms, int)
            or schema != 1
            or not has_token
            or not isinstance(action, str)
            or action not in WORKLOAD_OPERATION_ACTIONS
            or not _valid_workload_identifier(workload_id)
        ):
            continue
        age_ms = now_ms() - ts_unix_ms
        if age_ms < -WORKLOAD_OPERATION_FUTURE_SKEW_MS:
            continue
        if max(0.0, age_ms / 1000.0) > args.max_workload_operation_age_seconds:
            continue
        evidence = {
            "topic": WORKLOAD_OPERATION_TOPIC,
            "ulid": index["ulid"],
            "sha256": digest,
            "target_node": target_node,
            "action": action,
            "target": workload_id,
        }
        return workload_id, action, evidence
    return None, None, {}


def _short_json(value: Any, max_chars: int = 160) -> str:
    try:
        text = json.dumps(value, sort_keys=True)
    except (TypeError, ValueError):
        text = repr(value)
    if len(text) <= max_chars:
        return text
    return text[: max_chars - 1] + "…"


def _open_broker_session_ids(applied: Any) -> tuple[list[str], list[str]]:
    blockers: list[str] = []
    sessions: list[str] = []
    if not isinstance(applied, list):
        return sessions, ["applied is not a list"]
    non_string = sum(1 for item in applied if not isinstance(item, str))
    if non_string:
        suffix = "y" if non_string == 1 else "ies"
        blockers.append(f"applied contains {non_string} non-string entr{suffix}")
    for item in applied:
        if not isinstance(item, str) or not item.startswith("open-broker "):
            continue
        session_id = item.removeprefix("open-broker ")
        if (
            not session_id
            or session_id.strip() != session_id
            or any(ch.isspace() or ord(ch) < 0x20 for ch in session_id)
        ):
            blockers.append(
                f"invalid open-broker session id in applied entry {_short_json(item)!r}"
            )
            continue
        sessions.append(session_id)
    if not sessions:
        blockers.append("no open-broker action in applied")
    return sessions, blockers


def _open_broker_session_ids_from_actions(actions: Any) -> list[str]:
    if not isinstance(actions, list):
        return []
    sessions: list[str] = []
    for action in actions:
        body = None
        if isinstance(action, dict):
            if isinstance(action.get("OpenBroker"), dict):
                body = action["OpenBroker"]
            elif all(
                isinstance(action.get(key), str)
                for key in ("session_id", "serving_peer", "vm_id", "client_peer")
            ):
                body = action
        if not isinstance(body, dict):
            continue
        session_id = body.get("session_id")
        if isinstance(session_id, str) and session_id:
            sessions.append(session_id)
    return sessions


def _matching_onboard_apply_action(
    bus_root: Path,
    event_index: dict[str, Any],
    event_payload: dict[str, Any],
    event_sessions: list[str],
    args: argparse.Namespace,
) -> tuple[dict[str, Any] | None, str | None]:
    rows = read_topic_rows(bus_root, ONBOARD_APPLY_ACTION_TOPIC, args.bus_scan_limit)
    if not rows:
        return None, "no retained action/onboard/apply rows"

    event_ts = event_index.get("ts_unix_ms")
    event_issuer = event_payload.get("issuer")
    event_target = event_payload.get("target")
    event_nonce = event_payload.get("nonce")
    inspected = 0
    candidates = 0
    for action_index, _envelope, action_payload, action_digest in rows:
        inspected += 1
        if not isinstance(action_payload, dict):
            continue
        bundle = action_payload.get("bundle")
        if not isinstance(bundle, dict):
            continue
        if (
            action_payload.get("issuer") != event_issuer
            or bundle.get("target_node") != event_target
            or bundle.get("nonce") != event_nonce
        ):
            continue
        candidates += 1
        blockers: list[str] = []
        action_ts = action_index.get("ts_unix_ms")
        action_age_s: float | None = None
        if not isinstance(action_ts, int):
            blockers.append(f"action Bus timestamp is not an integer: {action_ts!r}")
        else:
            action_age_ms = now_ms() - action_ts
            if action_age_ms < -ONBOARD_ACK_FUTURE_SKEW_MS:
                blockers.append(
                    f"future action Bus timestamp age={action_age_ms / 1000.0:.1f}s "
                    f"< -{ONBOARD_ACK_FUTURE_SKEW_MS / 1000.0:.0f}s"
                )
            action_age_s = max(0.0, action_age_ms / 1000.0)
            if action_age_s > args.max_onboard_ack_age_seconds:
                blockers.append(
                    f"stale action age={action_age_s:.1f}s > {args.max_onboard_ack_age_seconds}s"
                )
            if isinstance(event_ts, int) and action_ts > event_ts:
                blockers.append("retained action timestamp is after acknowledgement")
        sig_hex = action_payload.get("sig_hex")
        if not isinstance(sig_hex, str) or not sig_hex.strip():
            blockers.append("sig_hex missing")
        issued_at = bundle.get("issued_at")
        if not isinstance(issued_at, int):
            blockers.append("bundle issued_at missing")
        action_sessions = _open_broker_session_ids_from_actions(bundle.get("actions"))
        matching_sessions = [
            session for session in event_sessions if session in set(action_sessions)
        ]
        if not matching_sessions:
            blockers.append(
                "retained action bundle lacks matching OpenBroker session "
                f"for {event_sessions}"
            )
        evidence = {
            "action_topic": ONBOARD_APPLY_ACTION_TOPIC,
            "action_ulid": action_index["ulid"],
            "action_sha256": action_digest,
            "action_age_seconds": round(action_age_s, 3)
            if action_age_s is not None
            else None,
            "bundle_issued_at": issued_at,
            "sig_hex": "present-redacted"
            if isinstance(sig_hex, str) and sig_hex.strip()
            else "missing",
            "action_open_broker_sessions": action_sessions,
            "matching_open_broker_sessions": matching_sessions,
        }
        if blockers:
            return evidence, "; ".join(blockers)
        return evidence, None

    return (
        {
            "retained_action_count": inspected,
            "matching_action_candidates": candidates,
            "event_issuer": event_issuer,
            "event_target": event_target,
            "event_nonce": event_nonce,
        },
        "no retained action/onboard/apply row matches issuer, target, and nonce",
    )


def check_workload_operation(args: argparse.Namespace, checks: list[Check]) -> None:
    required = args.require_all or args.require_workload_operation
    bus_root = Path(args.bus_root)
    candidates = node_candidates(args.node)
    try:
        rows = read_topic_rows(bus_root, WORKLOAD_OPERATION_TOPIC, args.bus_scan_limit)
        selected: tuple[dict[str, Any], dict[str, Any], str] | None = None
        for index, _envelope, payload, digest in rows:
            if payload is None:
                continue
            target_node = payload.get("target_node")
            if isinstance(target_node, str) and target_node in candidates:
                selected = (index, payload, digest)
                break
        if selected is None:
            checks.append(
                Check(
                    "typed Workload operation",
                    "blocked" if required else "warn",
                    f"no retained action/workload/operation message for {candidates}",
                    required,
                )
            )
            return
        index, payload, digest = selected
        action = payload.get("action")
        workload_id = payload.get("workload_id")
        schema = payload.get("schema_version")
        has_token = isinstance(payload.get("armed_token"), str) and bool(payload.get("armed_token"))
        ts_unix_ms = index.get("ts_unix_ms")
        age_s: float | None = None
        blockers: list[str] = []
        if not isinstance(ts_unix_ms, int):
            blockers.append(f"Bus timestamp is not an integer: {ts_unix_ms!r}")
        else:
            age_ms = now_ms() - ts_unix_ms
            if age_ms < -WORKLOAD_OPERATION_FUTURE_SKEW_MS:
                blockers.append(
                    f"future Bus timestamp age={age_ms / 1000.0:.1f}s "
                    f"< -{WORKLOAD_OPERATION_FUTURE_SKEW_MS / 1000.0:.0f}s"
                )
            age_s = max(0.0, age_ms / 1000.0)
            if age_s > args.max_workload_operation_age_seconds:
                blockers.append(
                    f"stale operation age={age_s:.1f}s > {args.max_workload_operation_age_seconds}s"
                )
        if schema != 1:
            blockers.append(f"schema_version={schema!r}, expected 1")
        if not has_token:
            blockers.append("armed_token missing")
        if not isinstance(action, str):
            blockers.append("action missing")
        elif action not in WORKLOAD_OPERATION_ACTIONS:
            blockers.append(f"unknown Workload action {action!r}")
        if not _valid_workload_identifier(workload_id):
            blockers.append("workload_id is missing or unbounded")
        deadline_at_ms = payload.get("deadline_at_ms")
        if not _is_workload_integer(deadline_at_ms) or deadline_at_ms <= now_ms():
            blockers.append("deadline_at_ms is missing or expired")
        evidence = {
            "topic": WORKLOAD_OPERATION_TOPIC,
            "ulid": index["ulid"],
            "sha256": digest,
            "target_node": payload.get("target_node"),
            "action": action,
            "target": workload_id,
            "schema_version": schema,
            "age_seconds": round(age_s, 3) if age_s is not None else None,
            "armed_token": "present-redacted" if has_token else "missing",
        }
        if blockers:
            checks.append(Check("typed Workload operation", "blocked" if required else "warn", "; ".join(blockers), required, evidence))
        else:
            checks.append(Check("typed Workload operation", "ok", f"retained authorized {action} for {workload_id} (token redacted)", required, evidence))
    except ProofError as exc:
        checks.append(Check("typed Workload operation", "error" if required else "warn", str(exc), required))


def check_onboard_open_broker(args: argparse.Namespace, checks: list[Check]) -> None:
    required = args.require_all or args.require_onboard_open_broker
    bus_root = Path(args.bus_root)
    candidates = node_candidates(args.node)
    try:
        rows = read_topic_rows(bus_root, ONBOARD_APPLY_EVENT_TOPIC, args.bus_scan_limit)
        selected: tuple[dict[str, Any], dict[str, Any], str, list[str], float] | None = None
        rejected: tuple[str, dict[str, Any]] | None = None
        for index, _envelope, payload, digest in rows:
            if payload is None:
                continue
            if payload.get("target") not in candidates:
                continue
            blockers: list[str] = []
            ts_unix_ms = index.get("ts_unix_ms")
            age_s: float | None = None
            if not isinstance(ts_unix_ms, int):
                blockers.append(f"Bus timestamp is not an integer: {ts_unix_ms!r}")
            else:
                age_ms = now_ms() - ts_unix_ms
                if age_ms < -ONBOARD_ACK_FUTURE_SKEW_MS:
                    blockers.append(
                        f"future Bus timestamp age={age_ms / 1000.0:.1f}s "
                        f"< -{ONBOARD_ACK_FUTURE_SKEW_MS / 1000.0:.0f}s"
                    )
                age_s = max(0.0, age_ms / 1000.0)
                if age_s > args.max_onboard_ack_age_seconds:
                    blockers.append(
                        f"stale acknowledgement age={age_s:.1f}s > {args.max_onboard_ack_age_seconds}s"
                    )
            issuer = payload.get("issuer")
            if not isinstance(issuer, str) or not issuer.strip():
                blockers.append("issuer missing")
            target = payload.get("target")
            if not isinstance(target, str):
                blockers.append("target missing")
            nonce = payload.get("nonce")
            if not isinstance(nonce, str) or not nonce.strip():
                blockers.append("nonce missing")
            if payload.get("error") is not None:
                blockers.append(f"event error present: {_short_json(payload.get('error'))}")
            sessions, applied_blockers = _open_broker_session_ids(payload.get("applied"))
            blockers.extend(applied_blockers)
            evidence = {
                "topic": ONBOARD_APPLY_EVENT_TOPIC,
                "ulid": index["ulid"],
                "sha256": digest,
                "issuer": issuer,
                "target": target,
                "nonce": nonce if isinstance(nonce, str) and nonce else None,
                "age_seconds": round(age_s, 3) if age_s is not None else None,
                "open_broker_sessions": sessions,
            }
            if blockers:
                if rejected is None:
                    rejected = ("; ".join(blockers), evidence)
                continue
            selected = (index, payload, digest, sessions, age_s or 0.0)
            break
        if selected is None:
            action_rows = read_topic_rows(bus_root, ONBOARD_APPLY_ACTION_TOPIC, min(args.bus_scan_limit, 16))
            action_hint = f"; retained onboard apply actions={len(action_rows)}" if action_rows else ""
            evidence: dict[str, Any] = {"candidate_targets": candidates}
            reject_hint = ""
            if rejected is not None:
                reject_hint = f"; newest matching acknowledgement rejected: {rejected[0]}"
                evidence["rejected_acknowledgement"] = rejected[1]
            checks.append(
                Check(
                    "onboard open-broker acknowledgement",
                    "blocked" if required else "warn",
                    f"no successful event/onboard/apply open-broker acknowledgement for {candidates}{action_hint}{reject_hint}",
                    required,
                    evidence,
                )
            )
            return
        index, payload, digest, sessions, age_s = selected
        applied = [item for item in payload.get("applied", []) if isinstance(item, str)]
        action_evidence, action_blocker = _matching_onboard_apply_action(
            bus_root, index, payload, sessions, args
        )
        event_evidence = {
            "topic": ONBOARD_APPLY_EVENT_TOPIC,
            "ulid": index["ulid"],
            "sha256": digest,
            "issuer": payload.get("issuer"),
            "target": payload.get("target"),
            "nonce": payload.get("nonce"),
            "age_seconds": round(age_s, 3),
            "applied": applied,
            "open_broker_sessions": sessions,
        }
        if action_evidence:
            event_evidence["correlated_apply_action"] = action_evidence
        if action_blocker:
            checks.append(
                Check(
                    "onboard open-broker acknowledgement",
                    "blocked" if required else "warn",
                    "fresh open-broker acknowledgement is not tied to a retained signed apply request: "
                    + action_blocker,
                    required,
                    event_evidence,
                )
            )
            return
        checks.append(
            Check(
                "onboard open-broker acknowledgement",
                "ok",
                f"target {payload.get('target')} acknowledged retained signed open-broker session(s) {', '.join(sessions)}",
                required,
                event_evidence,
            )
        )
    except ProofError as exc:
        checks.append(Check("onboard open-broker acknowledgement", "error" if required else "warn", str(exc), required))


def _check_systemd_socket(name: str, args: argparse.Namespace) -> tuple[str, str]:
    if shutil.which("systemctl") is None:
        return "warn", "systemctl unavailable"
    result = run_command(["systemctl", "is-active", name], args.command_timeout)
    if result.ok and result.stdout.strip() == "active":
        return "ok", f"{name} active"
    return "warn", f"{name} not active ({result.one_line()})"


def check_podman(args: argparse.Namespace, checks: list[Check]) -> None:
    required = args.require_all or args.require_podman
    if shutil.which("podman") is None:
        checks.append(Check("Podman", "blocked" if required else "warn", "podman command is unavailable", required))
        return
    version = run_command(["podman", "--version"], args.command_timeout)
    if not version.ok:
        checks.append(Check("Podman", "blocked" if required else "warn", f"podman --version failed: {version.one_line()}", required))
        return
    socket_status, socket_detail = _check_systemd_socket("podman.socket", args)
    status = "ok" if socket_status == "ok" else ("blocked" if required else "warn")
    detail = f"{version.stdout.strip()}; {socket_detail}"
    evidence = {"version": version.stdout.strip(), "podman_socket": socket_detail}
    checks.append(Check("Podman", status, detail, required, evidence))


def check_libvirt(args: argparse.Namespace, checks: list[Check]) -> None:
    required = args.require_all or args.require_libvirt
    if shutil.which("virsh") is None:
        checks.append(Check("libvirt", "blocked" if required else "warn", "virsh command is unavailable", required))
        return
    evidence: dict[str, Any] = {"uri": args.libvirt_uri}
    blockers: list[str] = []
    version = run_command(["virsh", "-c", args.libvirt_uri, "version"], args.command_timeout)
    if version.ok:
        evidence["version"] = version.stdout.strip().splitlines()[:8]
    else:
        blockers.append(f"virsh version failed: {version.one_line()}")
    list_domains = run_command(["virsh", "-c", args.libvirt_uri, "list", "--all", "--name"], args.command_timeout)
    if list_domains.ok:
        domains = [line.strip() for line in list_domains.stdout.splitlines() if line.strip()]
        evidence["domains"] = domains[:32]
        evidence["domain_count"] = len(domains)
    else:
        blockers.append(f"virsh list failed: {list_domains.one_line()}")
    net = run_command(["virsh", "-c", args.libvirt_uri, "net-info", args.libvirt_network], args.command_timeout)
    if net.ok:
        net_values = _split_key_values(net.stdout)
        evidence["network"] = net_values
        if net_values.get("active", "").lower() != "yes":
            blockers.append(f"network {args.libvirt_network} not active")
    else:
        blockers.append(f"network {args.libvirt_network} unavailable: {net.one_line()}")
    pool = run_command(["virsh", "-c", args.libvirt_uri, "pool-info", args.libvirt_pool], args.command_timeout)
    if pool.ok:
        pool_values = _split_key_values(pool.stdout)
        evidence["pool"] = pool_values
        if pool_values.get("state", "").lower() != "running":
            blockers.append(f"pool {args.libvirt_pool} not running")
    else:
        blockers.append(f"pool {args.libvirt_pool} unavailable: {pool.one_line()}")
    try:
        kvm_stat = Path("/dev/kvm").stat()
        if stat.S_ISCHR(kvm_stat.st_mode):
            evidence["dev_kvm"] = "char-device"
        else:
            blockers.append("/dev/kvm exists but is not a character device")
    except FileNotFoundError:
        blockers.append("/dev/kvm absent")
    except OSError as exc:
        blockers.append(f"cannot inspect /dev/kvm: {exc}")
    try:
        cpuinfo = _read_small_proc_file(Path("/proc/cpuinfo"), MAX_MESSAGE_BYTES)
        has_hwvirt = " vmx " in f" {cpuinfo} " or " svm " in f" {cpuinfo} "
        evidence["cpu_virtualization_flag"] = has_hwvirt
        if not has_hwvirt:
            blockers.append("CPU exposes no vmx/svm virtualization flag")
    except (OSError, ProofError) as exc:
        blockers.append(f"cannot inspect /proc/cpuinfo: {exc}")
    if blockers:
        checks.append(Check("libvirt/KVM", "blocked" if required else "warn", "; ".join(blockers), required, evidence))
    else:
        checks.append(Check("libvirt/KVM", "ok", f"{args.libvirt_uri} reachable, network {args.libvirt_network} active, pool {args.libvirt_pool} running, /dev/kvm present", required, evidence))


def check_bootstrap_ssh(args: argparse.Namespace, checks: list[Check]) -> None:
    required = args.require_all or args.require_bootstrap_ssh
    if args.bootstrap_host:
        try:
            with socket.create_connection((args.bootstrap_host, args.bootstrap_port), timeout=args.tcp_timeout):
                pass
            checks.append(Check("bootstrap SSH reachability", "ok", f"TCP {args.bootstrap_host}:{args.bootstrap_port} accepted", required))
        except OSError as exc:
            checks.append(Check("bootstrap SSH reachability", "blocked" if required else "warn", f"TCP {args.bootstrap_host}:{args.bootstrap_port} failed: {exc}", required))
        return
    if shutil.which("systemctl") is None:
        checks.append(Check("local sshd", "blocked" if required else "warn", "systemctl unavailable and no --bootstrap-host supplied", required))
        return
    result = run_command(["systemctl", "is-active", "sshd.service"], args.command_timeout)
    if result.ok and result.stdout.strip() == "active":
        checks.append(Check("local sshd", "ok", "sshd.service active; pass --bootstrap-host to prove a target TCP path", required))
    else:
        checks.append(Check("local sshd", "blocked" if required else "warn", f"sshd.service not active ({result.one_line()}); pass --bootstrap-host for remote TCP proof", required))


def collect_checks(args: argparse.Namespace) -> list[Check]:
    checks: list[Check] = []
    require_mackesd = (
        args.require_all
        or args.require_cloud_mirror
        or args.require_workload_operation
        or args.require_onboard_open_broker
        or args.require_workload_state
        or args.require_workload_admission
        or args.require_workload_recovery
    )
    check_services(args, checks, require_mackesd)
    check_cloud_arm(args, checks)
    check_cloud_mirror(args, checks)
    check_workload_placement(args, checks)
    check_workload_state(args, checks)
    check_workload_operation(args, checks)
    check_onboard_open_broker(args, checks)
    check_podman(args, checks)
    check_libvirt(args, checks)
    check_bootstrap_ssh(args, checks)
    return checks


def emit_text(args: argparse.Namespace, checks: list[Check]) -> None:
    target = args.node or "/".join(node_candidates(None))
    print("workloads-live-proof: read-only evidence report")
    print(f"node candidates: {target}")
    print(f"bus root: {args.bus_root}")
    print(f"libvirt URI: {args.libvirt_uri}")
    for check in checks:
        req = " required" if check.required else ""
        print(f"[{check.status.upper()}] {check.name}{req}: {check.detail}")
        if args.verbose and check.evidence:
            for key, value in sorted(check.evidence.items()):
                print(f"  - {key}: {value}")


def emit_json(args: argparse.Namespace, checks: list[Check]) -> None:
    failed = [check for check in checks if check.required and check.status in BAD_REQUIRED_STATUSES]
    print(
        json.dumps(
            {
                "generated_at_ms": now_ms(),
                "node_candidates": node_candidates(args.node),
                "bus_root": args.bus_root,
                "libvirt_uri": args.libvirt_uri,
                "required_blockers": [check.to_json() for check in failed],
                "checks": [check.to_json() for check in checks],
            },
            indent=2,
            sort_keys=True,
        )
    )


def _write_bus_message(root: Path, topic: str, ulid: str, payload: dict[str, Any], ts: int) -> None:
    rel = Path(topic) / f"{ulid}.json"
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    body = json.dumps(payload, sort_keys=True)
    envelope = {
        "ulid": ulid,
        "topic": topic,
        "priority": "default",
        "title": None,
        "body": body,
        "ts_unix_ms": ts,
        "file_path": str(rel),
        "actions": [],
        "reply_to": None,
    }
    path.write_text(json.dumps(envelope, sort_keys=True), encoding="utf-8")
    with sqlite3.connect(root / "index.sqlite") as conn:
        conn.execute(
            "INSERT INTO messages(ulid, topic, priority, title, body, ts_unix_ms, file_path) "
            "VALUES (?, ?, 'default', NULL, ?, ?, ?)",
            (ulid, topic, body, ts, str(rel)),
        )
        conn.commit()


def self_test() -> int:
    with tempfile.TemporaryDirectory(prefix="workloads-live-proof.") as temp:
        root = Path(temp)
        assert node_candidates("node-a") == ["node-a", "peer:node-a"]
        assert node_candidates("peer:node-a") == ["peer:node-a", "node-a"]
        with sqlite3.connect(root / "index.sqlite") as conn:
            conn.execute(
                "CREATE TABLE messages("
                "ulid TEXT NOT NULL PRIMARY KEY,"
                "topic TEXT NOT NULL,"
                "priority TEXT NOT NULL DEFAULT 'default',"
                "title TEXT,"
                "body TEXT,"
                "ts_unix_ms INTEGER NOT NULL,"
                "file_path TEXT NOT NULL UNIQUE)"
            )
            conn.execute("CREATE INDEX idx_messages_topic_ulid ON messages(topic, ulid)")
        ts = now_ms()
        _write_bus_message(
            root,
            "state/cloud/node-a",
            "01JWORKLOADSLIVEPROOF0001",
            {
                "host": "node-a",
                "adapter": "construct_cloud",
                "health": [
                    {"service_type": "opentofu", "state": "up"},
                    {"service_type": "ansible", "state": "up"},
                    {"service_type": "libvirt", "state": "up"},
                ],
                "resources": [],
                "apply_armed": True,
                "published_at_ms": ts,
                "workloads": [],
            },
            ts,
        )
        rows = read_topic_rows(root, "state/cloud/node-a", 1)
        assert rows[0][2] is not None
        assert rows[0][2]["adapter"] == "construct_cloud"
        workload_ts = now_ms()
        workload_status = {
            "schema_version": WORKLOAD_CONTRACT_SCHEMA_VERSION,
            "request_id": "op-workload-1",
            "workload_id": "demo-workload",
            "backend": "libvirt_virtqemud",
            "resources": {"vcpu": 2, "memory_mb": 4_096, "disk_gb": 32},
            "image_ref": "fedora:1.0",
            "generation": 1,
            "phase": "waiting_for_guest",
            "power": "starting",
            "readiness": "waiting_for_guest",
            "signals": {
                "guest_agent": "waiting_for_guest",
                "network": "unknown",
                "service": "unknown",
                "display": "unknown",
                "application": "unknown",
                "health": "unknown",
                "pressure": "normal",
                "progress_percent": 60,
            },
            "retryable": True,
            "attempt": 2,
            "next_retry_at_ms": workload_ts + 5_000,
            "reason": "temporary adapter failure",
            "remediation": "adapter will retry with bounded backoff",
            "attachment": None,
        }
        _write_bus_message(
            root,
            f"{WORKLOAD_STATE_PREFIX}node-a",
            "ZZZWORKLOADSLIVEPROOF0012",
            {
                "schema_version": WORKLOAD_CONTRACT_SCHEMA_VERSION,
                "node": "node-a",
                "observed_at_ms": workload_ts,
                "workloads": [workload_status],
            },
            workload_ts,
        )
        workload_args = argparse.Namespace(
            require_all=False,
            require_workload_state=True,
            require_workload_placement=False,
            require_workload_admission=True,
            require_workload_recovery=True,
            bus_root=str(root),
            node="node-a",
            bus_scan_limit=8,
            max_workload_age_seconds=120.0,
            expect_workload_id="demo-workload",
            expect_workload_phase="waiting_for_guest",
        )
        workload_checks: list[Check] = []
        check_workload_state(workload_args, workload_checks)
        assert workload_checks[0].status == "ok"
        assert workload_checks[0].evidence["admission_phase_count"] == 1
        assert workload_checks[0].evidence["attempted_workload_count"] == 1
        assert workload_checks[0].evidence["node"] == "node-a"
        workload_bad = dict(workload_status)
        workload_bad["resources"] = {"vcpu": 65, "memory_mb": 4_096, "disk_gb": 32}
        _write_bus_message(
            root,
            f"{WORKLOAD_STATE_PREFIX}node-a",
            "ZZZWORKLOADSLIVEPROOF0013",
            {
                "schema_version": WORKLOAD_CONTRACT_SCHEMA_VERSION,
                "node": "peer:node-a",
                "observed_at_ms": workload_ts,
                "workloads": [workload_bad],
            },
            workload_ts,
        )
        workload_checks = []
        check_workload_state(workload_args, workload_checks)
        assert workload_checks[0].status == "blocked"
        assert "snapshot.node does not match" in workload_checks[0].detail
        assert "resources.vcpu" in workload_checks[0].detail
        role_path = root / "role.toml"
        role_path.write_text('role = "workstation"\n', encoding="utf-8")
        placement_args = argparse.Namespace(
            require_all=False,
            require_workload_placement=True,
            require_workload_admission=False,
            require_workload_recovery=False,
            role_path=str(role_path),
        )
        placement_checks: list[Check] = []
        check_workload_placement(placement_args, placement_checks)
        assert placement_checks[0].status == "ok"
        role_path.write_text('role = "lighthouse"\n', encoding="utf-8")
        placement_checks = []
        check_workload_placement(placement_args, placement_checks)
        assert placement_checks[0].status == "blocked"
        assert "cannot host Workloads" in placement_checks[0].detail
        link = root / "state" / "cloud" / "node-a" / "01JWORKLOADSLIVEPROOF0002.json"
        link.symlink_to(root / "state" / "cloud" / "node-a" / "01JWORKLOADSLIVEPROOF0001.json")
        with sqlite3.connect(root / "index.sqlite") as conn:
            conn.execute(
                "INSERT INTO messages(ulid, topic, priority, title, body, ts_unix_ms, file_path) "
                "VALUES (?, 'state/cloud/node-a', 'default', NULL, '{}', ?, ?)",
                ("ZZZWORKLOADSLIVEPROOF0002", ts, "state/cloud/node-a/01JWORKLOADSLIVEPROOF0002.json"),
            )
            conn.commit()
        try:
            read_topic_rows(root, "state/cloud/node-a", 1)
            raise AssertionError("symlinked indexed message was accepted")
        except ProofError as exc:
            assert "symlink" in str(exc)
        operation_base = {
            "schema_version": 1,
            "request_id": "proof-operation-1",
            "workload_id": "demo-workload",
            "backend": "libvirt_virtqemud",
            "resources": {"vcpu": 2, "memory_mb": 4_096, "disk_gb": 32},
            "target_node": "node-a",
            "expected_generation": 0,
            "action": "start",
            "target_request_id": None,
            "deadline_at_ms": now_ms() + 20_000,
            "preferred_attachment": None,
            "armed_token": "super-secret-token",
        }
        _write_bus_message(
            root,
            WORKLOAD_OPERATION_TOPIC,
            "ZZZWORKLOADSLIVEPROOF0003",
            operation_base,
            ts - 200_000,
        )
        operation_args = argparse.Namespace(
            require_all=False,
            require_workload_operation=True,
            bus_root=str(root),
            node="node-a",
            bus_scan_limit=8,
            max_workload_operation_age_seconds=120.0,
        )
        operation_checks: list[Check] = []
        check_workload_operation(operation_args, operation_checks)
        assert operation_checks[0].status == "blocked"
        assert "stale operation age" in operation_checks[0].detail
        stale_json = json.dumps(operation_checks[0].to_json(), sort_keys=True)
        assert "super-secret-token" not in stale_json
        assert "present-redacted" in stale_json
        invalid_operation = dict(operation_base)
        invalid_operation["request_id"] = "proof-operation-2"
        invalid_operation["action"] = "teleport"
        invalid_operation["armed_token"] = "invalid-op-super-secret-token"
        _write_bus_message(
            root,
            WORKLOAD_OPERATION_TOPIC,
            "ZZZWORKLOADSLIVEPROOF0004",
            invalid_operation,
            now_ms(),
        )
        operation_checks = []
        check_workload_operation(operation_args, operation_checks)
        assert operation_checks[0].status == "blocked"
        assert "unknown Workload action" in operation_checks[0].detail
        invalid_json = json.dumps(operation_checks[0].to_json(), sort_keys=True)
        assert "invalid-op-super-secret-token" not in invalid_json
        assert "present-redacted" in invalid_json
        fresh_operation = dict(operation_base)
        fresh_operation["request_id"] = "proof-operation-3"
        fresh_operation["armed_token"] = "new-super-secret-token"
        _write_bus_message(
            root,
            WORKLOAD_OPERATION_TOPIC,
            "ZZZWORKLOADSLIVEPROOF0005",
            fresh_operation,
            now_ms(),
        )
        operation_checks = []
        check_workload_operation(operation_args, operation_checks)
        assert operation_checks[0].status == "ok"
        fresh_json = json.dumps(operation_checks[0].to_json(), sort_keys=True)
        assert "new-super-secret-token" not in fresh_json
        assert "present-redacted" in fresh_json
        onboard_args = lambda node: argparse.Namespace(
            require_all=False,
            require_onboard_open_broker=True,
            bus_root=str(root),
            node=node,
            bus_scan_limit=8,
            max_onboard_ack_age_seconds=120.0,
        )
        _write_bus_message(
            root,
            ONBOARD_APPLY_EVENT_TOPIC,
            "ZZZWORKLOADSLIVEPROOF0007",
            {
                "issuer": "peer:issuer",
                "target": "node-b",
                "nonce": "nonce-b",
                "applied": ["open-broker "],
                "error": None,
            },
            now_ms(),
        )
        onboard_checks: list[Check] = []
        check_onboard_open_broker(onboard_args("node-b"), onboard_checks)
        assert onboard_checks[0].status == "blocked"
        assert "invalid open-broker session id" in onboard_checks[0].detail
        _write_bus_message(
            root,
            ONBOARD_APPLY_EVENT_TOPIC,
            "ZZZWORKLOADSLIVEPROOF0008",
            {
                "issuer": "peer:issuer",
                "target": "node-c",
                "nonce": "nonce-c",
                "applied": ["open-broker stale-session"],
                "error": None,
            },
            now_ms() - 200_000,
        )
        onboard_checks = []
        check_onboard_open_broker(onboard_args("node-c"), onboard_checks)
        assert onboard_checks[0].status == "blocked"
        assert "stale acknowledgement age" in onboard_checks[0].detail
        _write_bus_message(
            root,
            ONBOARD_APPLY_EVENT_TOPIC,
            "ZZZWORKLOADSLIVEPROOF0009",
            {
                "issuer": "peer:issuer",
                "target": "node-d",
                "nonce": "nonce-d",
                "applied": ["open-broker orphan-session"],
                "error": None,
            },
            now_ms(),
        )
        onboard_checks = []
        check_onboard_open_broker(onboard_args("node-d"), onboard_checks)
        assert onboard_checks[0].status == "blocked"
        assert "not tied to a retained signed apply request" in onboard_checks[0].detail
        valid_action_ts = now_ms()
        _write_bus_message(
            root,
            ONBOARD_APPLY_ACTION_TOPIC,
            "ZZZWORKLOADSLIVEPROOF0010",
            {
                "issuer": "peer:issuer",
                "bundle": {
                    "target_node": "peer:node-a",
                    "actions": [
                        {
                            "OpenBroker": {
                                "session_id": "test-session",
                                "serving_peer": "peer:node-a",
                                "vm_id": "demo-vm",
                                "client_peer": "peer:issuer",
                            }
                        }
                    ],
                    "issued_at": valid_action_ts // 1000,
                    "nonce": "nonce-a",
                },
                "sig_hex": "present-redacted",
            },
            valid_action_ts,
        )
        _write_bus_message(
            root,
            ONBOARD_APPLY_EVENT_TOPIC,
            "ZZZWORKLOADSLIVEPROOF0011",
            {
                "issuer": "peer:issuer",
                "target": "peer:node-a",
                "nonce": "nonce-a",
                "applied": ["open-broker test-session"],
                "error": None,
            },
            valid_action_ts + 1,
        )
        onboard_checks = []
        check_onboard_open_broker(onboard_args("node-a"), onboard_checks)
        assert onboard_checks[0].status == "ok"
        assert onboard_checks[0].evidence["target"] == "peer:node-a"
        assert onboard_checks[0].evidence["open_broker_sessions"] == ["test-session"]
        assert (
            onboard_checks[0].evidence["correlated_apply_action"][
                "matching_open_broker_sessions"
            ]
            == ["test-session"]
        )
    print("verify-workloads-live-proof: self-test passed")
    return 0


def parse_args(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--node", help="placement node id to match in Bus mirrors (default: hostname candidates)")
    parser.add_argument("--bus-root", default=str(Path(os.environ.get("MDE_BUS_ROOT", str(DEFAULT_BUS_ROOT)))))
    parser.add_argument("--credential-path", default=os.environ.get("MCNF_CLOUD_ARM_CREDENTIAL_PATH", str(DEFAULT_CREDENTIAL_PATH)))
    parser.add_argument("--role-path", default=os.environ.get("MDE_ROLE_PATH", str(DEFAULT_ROLE_PATH)))
    parser.add_argument("--libvirt-uri", default=os.environ.get("MDE_LIBVIRT_URI", DEFAULT_LIBVIRT_URI))
    parser.add_argument("--libvirt-network", default=DEFAULT_NETWORK)
    parser.add_argument("--libvirt-pool", default=DEFAULT_POOL)
    parser.add_argument("--bootstrap-host", help="optional remote host for a TCP-only SSH reachability proof")
    parser.add_argument("--bootstrap-port", type=int, default=22)
    parser.add_argument("--max-cloud-age-seconds", type=float, default=120.0)
    parser.add_argument("--max-workload-operation-age-seconds", type=float, default=120.0)
    parser.add_argument("--max-onboard-ack-age-seconds", type=float, default=120.0)
    parser.add_argument("--max-workload-age-seconds", type=float, default=120.0)
    parser.add_argument("--bus-scan-limit", type=int, default=64)
    parser.add_argument("--command-timeout", type=float, default=5.0)
    parser.add_argument("--tcp-timeout", type=float, default=3.0)
    parser.add_argument("--require-all", action="store_true", help="require every live Workloads proof seam this helper can inspect")
    parser.add_argument("--require-cloud-arm", action="store_true")
    parser.add_argument("--require-cloud-mirror", action="store_true")
    parser.add_argument("--require-workload-operation", action="store_true", help="require a fresh authorized typed Workload operation")
    parser.add_argument("--require-onboard-open-broker", action="store_true")
    parser.add_argument("--require-workload-state", action="store_true", help="require a fresh typed state/workloads/<node> projection")
    parser.add_argument("--require-workload-placement", action="store_true", help="require a pinned Workstation-compatible role")
    parser.add_argument("--require-workload-admission", action="store_true", help="require a workload observed at or beyond the typed admitting phase")
    parser.add_argument("--require-workload-recovery", action="store_true", help="require persisted adapter-attempt evidence; does not simulate a restart")
    parser.add_argument("--expect-workload-id", help="require this workload id in the typed Workload projection")
    parser.add_argument("--expect-workload-phase", help="when --expect-workload-id is set, require this exact phase")
    parser.add_argument("--require-podman", action="store_true")
    parser.add_argument("--require-libvirt", action="store_true")
    parser.add_argument("--require-bootstrap-ssh", action="store_true")
    parser.add_argument("--require-seat", action="store_true", help="require mde-shell-egui.service to be active")
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    parser.add_argument("--verbose", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.bus_scan_limit <= 0 or args.bus_scan_limit > MAX_TOPIC_SCAN_ROWS:
        parser.error(f"--bus-scan-limit must be 1..{MAX_TOPIC_SCAN_ROWS}")
    if args.bootstrap_port <= 0 or args.bootstrap_port > 65535:
        parser.error("--bootstrap-port must be 1..65535")
    if not args.max_workload_operation_age_seconds >= 0:
        parser.error("--max-workload-operation-age-seconds must be non-negative")
    if not args.max_onboard_ack_age_seconds >= 0:
        parser.error("--max-onboard-ack-age-seconds must be non-negative")
    if not args.max_workload_age_seconds >= 0:
        parser.error("--max-workload-age-seconds must be non-negative")
    if args.expect_workload_phase and args.expect_workload_phase not in WORKLOAD_PHASES:
        parser.error("--expect-workload-phase must name a known Workload phase")
    if args.expect_workload_id and not _valid_workload_identifier(args.expect_workload_id):
        parser.error("--expect-workload-id must be a bounded Workload identifier")
    if args.expect_workload_phase and not args.expect_workload_id:
        parser.error("--expect-workload-phase requires --expect-workload-id")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.self_test:
        return self_test()
    checks = collect_checks(args)
    if args.json:
        emit_json(args, checks)
    else:
        emit_text(args, checks)
    failed_required = [check for check in checks if check.required and check.status in BAD_REQUIRED_STATUSES]
    return 2 if failed_required else 0


if __name__ == "__main__":
    raise SystemExit(main())
