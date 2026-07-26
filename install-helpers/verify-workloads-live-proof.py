#!/usr/bin/env python3
"""Read-only Workloads live evidence collector for WL-ARCH-007.

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

CLOUD_PREFIX = "state/cloud/"
VM_LIFECYCLE_ACTION_TOPIC = "action/vm/lifecycle"
VM_INSTANCES_TOPIC = "event/vm/instances"
ONBOARD_APPLY_ACTION_TOPIC = "action/onboard/apply"
ONBOARD_APPLY_EVENT_TOPIC = "event/onboard/apply"
VM_LIFECYCLE_MUTATION_OPS = {
    "attach_usb",
    "create",
    "destroy",
    "detach_usb",
    "pause",
    "resume",
    "start",
    "stop",
}
VM_LIFECYCLE_ACTION_OPS = VM_LIFECYCLE_MUTATION_OPS | {"refresh"}

MAX_MESSAGE_BYTES = 1_048_576
MAX_DROPIN_BYTES = 16 * 1024
MAX_TOPIC_SCAN_ROWS = 256
REQUIRED_CLOUD_TOOLS = ("opentofu", "ansible", "libvirt")
LIFECYCLE_ACTION_FUTURE_SKEW_MS = 30 * 1000
ONBOARD_ACK_FUTURE_SKEW_MS = 30 * 1000

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
        "mackesd.service": require_mackesd,
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
        Path("/etc/systemd/system/mackesd.service.d/50-cloud-arm-credential.conf"),
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


def _instance_name(action: dict[str, Any]) -> str:
    if isinstance(action.get("name"), str):
        return action["name"]
    spec = action.get("spec")
    if isinstance(spec, dict) and isinstance(spec.get("name"), str):
        return spec["name"]
    return ""


def _fresh_lifecycle_action_target(
    args: argparse.Namespace,
    candidates: list[str],
) -> tuple[str | None, str | None, dict[str, Any]]:
    """Return the fresh authorized lifecycle target this proof should correlate."""
    rows = read_topic_rows(Path(args.bus_root), VM_LIFECYCLE_ACTION_TOPIC, args.bus_scan_limit)
    for index, _envelope, payload, digest in rows:
        if payload is None:
            continue
        host = payload.get("host")
        if not isinstance(host, str) or host not in candidates:
            continue
        op = payload.get("op")
        name = _instance_name(payload)
        schema = payload.get("schema_version")
        has_token = isinstance(payload.get("armed_token"), str) and bool(payload.get("armed_token"))
        ts_unix_ms = index.get("ts_unix_ms")
        if (
            not isinstance(ts_unix_ms, int)
            or schema != 1
            or not has_token
            or not isinstance(op, str)
            or op not in VM_LIFECYCLE_ACTION_OPS
            or not name
        ):
            continue
        age_ms = now_ms() - ts_unix_ms
        if age_ms < -LIFECYCLE_ACTION_FUTURE_SKEW_MS:
            continue
        if max(0.0, age_ms / 1000.0) > args.max_lifecycle_action_age_seconds:
            continue
        evidence = {
            "topic": VM_LIFECYCLE_ACTION_TOPIC,
            "ulid": index["ulid"],
            "sha256": digest,
            "host": host,
            "op": op,
            "target": name,
        }
        return name, op, evidence
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


def check_lifecycle_action(args: argparse.Namespace, checks: list[Check]) -> None:
    required = args.require_all or args.require_lifecycle_action
    bus_root = Path(args.bus_root)
    candidates = node_candidates(args.node)
    try:
        rows = read_topic_rows(bus_root, VM_LIFECYCLE_ACTION_TOPIC, args.bus_scan_limit)
        selected: tuple[dict[str, Any], dict[str, Any], str] | None = None
        for index, _envelope, payload, digest in rows:
            if payload is None:
                continue
            host = payload.get("host")
            if isinstance(host, str) and host in candidates:
                selected = (index, payload, digest)
                break
        if selected is None:
            checks.append(
                Check(
                    "vm_lifecycle action",
                    "blocked" if required else "warn",
                    f"no retained action/vm/lifecycle message for {candidates}",
                    required,
                )
            )
            return
        index, payload, digest = selected
        op = payload.get("op")
        name = _instance_name(payload)
        schema = payload.get("schema_version")
        has_token = isinstance(payload.get("armed_token"), str) and bool(payload.get("armed_token"))
        ts_unix_ms = index.get("ts_unix_ms")
        age_s: float | None = None
        blockers: list[str] = []
        if not isinstance(ts_unix_ms, int):
            blockers.append(f"Bus timestamp is not an integer: {ts_unix_ms!r}")
        else:
            age_ms = now_ms() - ts_unix_ms
            if age_ms < -LIFECYCLE_ACTION_FUTURE_SKEW_MS:
                blockers.append(
                    f"future Bus timestamp age={age_ms / 1000.0:.1f}s "
                    f"< -{LIFECYCLE_ACTION_FUTURE_SKEW_MS / 1000.0:.0f}s"
                )
            age_s = max(0.0, age_ms / 1000.0)
            if age_s > args.max_lifecycle_action_age_seconds:
                blockers.append(
                    f"stale action age={age_s:.1f}s > {args.max_lifecycle_action_age_seconds}s"
                )
        if schema != 1:
            blockers.append(f"schema_version={schema!r}, expected 1")
        if not has_token:
            blockers.append("armed_token missing")
        if not isinstance(op, str):
            blockers.append("op missing")
        elif op not in VM_LIFECYCLE_ACTION_OPS:
            blockers.append(f"unknown lifecycle op {op!r}")
        if not name and op != "refresh":
            blockers.append("target VM name missing")
        evidence = {
            "topic": VM_LIFECYCLE_ACTION_TOPIC,
            "ulid": index["ulid"],
            "sha256": digest,
            "host": payload.get("host"),
            "op": op,
            "target": name,
            "schema_version": schema,
            "age_seconds": round(age_s, 3) if age_s is not None else None,
            "armed_token": "present-redacted" if has_token else "missing",
        }
        if blockers:
            checks.append(Check("vm_lifecycle action", "blocked" if required else "warn", "; ".join(blockers), required, evidence))
        else:
            checks.append(Check("vm_lifecycle action", "ok", f"retained authorized {op} for {name or payload.get('host')} (token redacted)", required, evidence))
    except ProofError as exc:
        checks.append(Check("vm_lifecycle action", "error" if required else "warn", str(exc), required))


def check_vm_roster(args: argparse.Namespace, checks: list[Check]) -> None:
    required = args.require_all or args.require_vm_roster
    bus_root = Path(args.bus_root)
    candidates = node_candidates(args.node)
    try:
        rows = read_topic_rows(bus_root, VM_INSTANCES_TOPIC, args.bus_scan_limit)
        selected = first_matching_payload(rows, candidates, "host")
        if selected is None:
            checks.append(Check("vm_lifecycle roster", "blocked" if required else "warn", f"no event/vm/instances report for {candidates}", required))
            return
        index, _envelope, payload, digest = selected
        published = payload.get("published_at_ms")
        instances = payload.get("instances") if isinstance(payload.get("instances"), list) else []
        if not isinstance(published, int):
            checks.append(Check("vm_lifecycle roster", "blocked" if required else "warn", "roster has no integer published_at_ms", required))
            return
        age_s = max(0.0, (now_ms() - published) / 1000.0)
        names = [
            f"{item.get('name')}:{item.get('state')}"
            for item in instances
            if isinstance(item, dict) and isinstance(item.get("name"), str)
        ]
        inferred_vm: str | None = None
        inferred_op: str | None = None
        inferred_action: dict[str, Any] = {}
        should_correlate_lifecycle = (
            required
            and not args.expect_vm
            and (args.require_all or getattr(args, "require_lifecycle_action", False))
        )
        if should_correlate_lifecycle:
            inferred_vm, inferred_op, inferred_action = _fresh_lifecycle_action_target(
                args, candidates
            )
        blockers: list[str] = []
        if age_s > args.max_roster_age_seconds:
            blockers.append(f"stale age={age_s:.1f}s > {args.max_roster_age_seconds}s")
        expected_vm = args.expect_vm or inferred_vm
        if expected_vm:
            found = False
            for item in instances:
                if not isinstance(item, dict):
                    continue
                if item.get("name") == expected_vm:
                    found = True
                    if args.expect_vm_state and item.get("state") != args.expect_vm_state:
                        blockers.append(
                            f"{expected_vm} state={item.get('state')!r}, expected {args.expect_vm_state!r}"
                        )
                    break
            if not found:
                if inferred_vm:
                    blockers.append(
                        f"lifecycle action target {inferred_vm!r} absent from roster"
                    )
                else:
                    blockers.append(f"expected VM {expected_vm!r} absent from roster")
        if args.require_running_vm and not any(
            isinstance(item, dict) and item.get("state") == "running" for item in instances
        ):
            blockers.append("no running VM in roster")
        evidence = {
            "topic": VM_INSTANCES_TOPIC,
            "ulid": index["ulid"],
            "sha256": digest,
            "host": payload.get("host"),
            "age_seconds": round(age_s, 3),
            "instances": names[:32],
            "instance_count": len(instances),
        }
        if inferred_action:
            evidence["correlated_lifecycle_action"] = inferred_action
        if blockers:
            checks.append(Check("vm_lifecycle roster", "blocked" if required else "warn", "; ".join(blockers), required, evidence))
        else:
            detail = f"fresh roster for {payload.get('host')} with {len(instances)} instance(s)"
            if expected_vm:
                detail += f"; {expected_vm} observed"
                if inferred_op:
                    detail += f" for retained {inferred_op} action"
            checks.append(Check("vm_lifecycle roster", "ok", detail, required, evidence))
    except ProofError as exc:
        checks.append(Check("vm_lifecycle roster", "error" if required else "warn", str(exc), required))


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
        or args.require_vm_roster
        or args.require_lifecycle_action
        or args.require_onboard_open_broker
    )
    check_services(args, checks, require_mackesd)
    check_cloud_arm(args, checks)
    check_cloud_mirror(args, checks)
    check_lifecycle_action(args, checks)
    check_vm_roster(args, checks)
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
        _write_bus_message(
            root,
            VM_LIFECYCLE_ACTION_TOPIC,
            "ZZZWORKLOADSLIVEPROOF0003",
            {
                "schema_version": 1,
                "host": "node-a",
                "op": "start",
                "name": "demo-vm",
                "armed_token": "super-secret-token",
            },
            ts - 200_000,
        )
        lifecycle_args = argparse.Namespace(
            require_all=False,
            require_lifecycle_action=True,
            bus_root=str(root),
            node="node-a",
            bus_scan_limit=8,
            max_lifecycle_action_age_seconds=120.0,
        )
        lifecycle_checks: list[Check] = []
        check_lifecycle_action(lifecycle_args, lifecycle_checks)
        assert lifecycle_checks[0].status == "blocked"
        assert "stale action age" in lifecycle_checks[0].detail
        stale_json = json.dumps(lifecycle_checks[0].to_json(), sort_keys=True)
        assert "super-secret-token" not in stale_json
        assert "present-redacted" in stale_json
        _write_bus_message(
            root,
            VM_LIFECYCLE_ACTION_TOPIC,
            "ZZZWORKLOADSLIVEPROOF0004",
            {
                "schema_version": 1,
                "host": "node-a",
                "op": "teleport",
                "name": "demo-vm",
                "armed_token": "invalid-op-super-secret-token",
            },
            now_ms(),
        )
        lifecycle_checks = []
        check_lifecycle_action(lifecycle_args, lifecycle_checks)
        assert lifecycle_checks[0].status == "blocked"
        assert "unknown lifecycle op" in lifecycle_checks[0].detail
        invalid_op_json = json.dumps(lifecycle_checks[0].to_json(), sort_keys=True)
        assert "invalid-op-super-secret-token" not in invalid_op_json
        assert "present-redacted" in invalid_op_json
        _write_bus_message(
            root,
            VM_LIFECYCLE_ACTION_TOPIC,
            "ZZZWORKLOADSLIVEPROOF0005",
            {
                "schema_version": 1,
                "host": "node-a",
                "op": "start",
                "name": "demo-vm",
                "armed_token": "new-super-secret-token",
            },
            now_ms(),
        )
        lifecycle_checks = []
        check_lifecycle_action(lifecycle_args, lifecycle_checks)
        assert lifecycle_checks[0].status == "ok"
        fresh_json = json.dumps(lifecycle_checks[0].to_json(), sort_keys=True)
        assert "new-super-secret-token" not in fresh_json
        assert "present-redacted" in fresh_json
        _write_bus_message(
            root,
            VM_INSTANCES_TOPIC,
            "ZZZWORKLOADSLIVEPROOF0006",
            {
                "host": "peer:node-a",
                "published_at_ms": now_ms(),
                "instances": [{"name": "demo-vm", "state": "running"}],
            },
            now_ms(),
        )
        roster_args = argparse.Namespace(
            require_all=False,
            require_vm_roster=True,
            require_lifecycle_action=False,
            bus_root=str(root),
            node="node-a",
            bus_scan_limit=8,
            max_roster_age_seconds=120.0,
            max_lifecycle_action_age_seconds=120.0,
            expect_vm="demo-vm",
            expect_vm_state="running",
            require_running_vm=True,
        )
        roster_checks: list[Check] = []
        check_vm_roster(roster_args, roster_checks)
        assert roster_checks[0].status == "ok"
        assert roster_checks[0].evidence["host"] == "peer:node-a"
        roster_args.expect_vm = None
        roster_args.expect_vm_state = None
        roster_args.require_lifecycle_action = True
        roster_checks = []
        check_vm_roster(roster_args, roster_checks)
        assert roster_checks[0].status == "ok"
        assert roster_checks[0].evidence["correlated_lifecycle_action"]["target"] == "demo-vm"
        _write_bus_message(
            root,
            VM_INSTANCES_TOPIC,
            "ZZZWORKLOADSLIVEPROOF0006M",
            {
                "host": "peer:node-a",
                "published_at_ms": now_ms(),
                "instances": [{"name": "other-vm", "state": "running"}],
            },
            now_ms(),
        )
        roster_checks = []
        check_vm_roster(roster_args, roster_checks)
        assert roster_checks[0].status == "blocked"
        assert "lifecycle action target 'demo-vm' absent from roster" in roster_checks[0].detail
        _write_bus_message(
            root,
            VM_INSTANCES_TOPIC,
            "ZZZWORKLOADSLIVEPROOF0006Z",
            {
                "host": "peer:node-a",
                "published_at_ms": now_ms(),
                "instances": [{"name": "demo-vm", "state": "running"}],
            },
            now_ms(),
        )
        roster_checks = []
        check_vm_roster(roster_args, roster_checks)
        assert roster_checks[0].status == "ok"
        assert "retained start action" in roster_checks[0].detail
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
    parser.add_argument("--libvirt-uri", default=os.environ.get("MDE_LIBVIRT_URI", DEFAULT_LIBVIRT_URI))
    parser.add_argument("--libvirt-network", default=DEFAULT_NETWORK)
    parser.add_argument("--libvirt-pool", default=DEFAULT_POOL)
    parser.add_argument("--bootstrap-host", help="optional remote host for a TCP-only SSH reachability proof")
    parser.add_argument("--bootstrap-port", type=int, default=22)
    parser.add_argument("--expect-vm", help="require this VM name in event/vm/instances")
    parser.add_argument("--expect-vm-state", help="when --expect-vm is set, require this exact virsh state")
    parser.add_argument("--max-cloud-age-seconds", type=float, default=120.0)
    parser.add_argument("--max-lifecycle-action-age-seconds", type=float, default=120.0)
    parser.add_argument("--max-onboard-ack-age-seconds", type=float, default=120.0)
    parser.add_argument("--max-roster-age-seconds", type=float, default=120.0)
    parser.add_argument("--bus-scan-limit", type=int, default=64)
    parser.add_argument("--command-timeout", type=float, default=5.0)
    parser.add_argument("--tcp-timeout", type=float, default=3.0)
    parser.add_argument("--require-all", action="store_true", help="require every live Workloads proof seam this helper can inspect")
    parser.add_argument("--require-cloud-arm", action="store_true")
    parser.add_argument("--require-cloud-mirror", action="store_true")
    parser.add_argument("--require-lifecycle-action", action="store_true")
    parser.add_argument("--require-vm-roster", action="store_true")
    parser.add_argument("--require-onboard-open-broker", action="store_true")
    parser.add_argument("--require-podman", action="store_true")
    parser.add_argument("--require-libvirt", action="store_true")
    parser.add_argument("--require-bootstrap-ssh", action="store_true")
    parser.add_argument("--require-running-vm", action="store_true")
    parser.add_argument("--require-seat", action="store_true", help="require mde-shell-egui.service to be active")
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    parser.add_argument("--verbose", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.bus_scan_limit <= 0 or args.bus_scan_limit > MAX_TOPIC_SCAN_ROWS:
        parser.error(f"--bus-scan-limit must be 1..{MAX_TOPIC_SCAN_ROWS}")
    if args.bootstrap_port <= 0 or args.bootstrap_port > 65535:
        parser.error("--bootstrap-port must be 1..65535")
    if args.expect_vm_state and not args.expect_vm:
        parser.error("--expect-vm-state requires --expect-vm")
    if not args.max_lifecycle_action_age_seconds >= 0:
        parser.error("--max-lifecycle-action-age-seconds must be non-negative")
    if not args.max_onboard_ack_age_seconds >= 0:
        parser.error("--max-onboard-ack-age-seconds must be non-negative")
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
