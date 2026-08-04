#!/usr/bin/env bash
# Admit the stable Browser VM into the typed Workloads desired-state plane.
# This helper emits exactly one capability-bound browser-provision request. It
# never chooses placement, invokes a provider/virsh/raw shell, provisions or
# starts a domain, resolves a guest credential, or selects a VDI fallback.
# The armed capability is created in memory and delivered only over mde-bus
# stdin; neither the key, token, request body, nor raw reply is printed.
set -euo pipefail
set +x
umask 077
ulimit -S -c 0
ulimit -H -c 0

readonly PROGRAM_NAME="request-browser-vm-workload"
readonly PYTHON_BIN="/usr/bin/python3"
readonly MDE_BUS_BIN="/usr/bin/mde-bus"
readonly BUS_ROOT="/run/mde-bus"

usage() {
  cat >&2 <<EOF
usage: $0 --node NODE --placement-receipt ULID --name browser-vm \\
          --profile-id browser-vm-chromium --source-commit GIT_SHA \\
          --image-digest sha256:HEX --credential-ref desktop/browser-vm/rdp \\
          [--credential-path PATH]
       $0 --self-test

Prerequisites (no fallback is inferred):
  * --placement-receipt is the latest fresh state/cloud/NODE Workloads receipt;
    that projection must report Construct Cloud, an armed mutation plane,
    opentofu/ansible/libvirt up, and at least 4 vCPU + 8192 MiB free.
  * --source-commit and --image-digest identify the immutable
    browser-vm-chromium artifact selected by the operator.
  * --credential-ref is the opaque guest-login store identity; this helper never
    accepts or resolves the guest username/password.
  * Run from a systemd unit carrying LoadCredentialEncrypted=cloud-arm-key, or
    pass the absolute path of an already-loaded plaintext arming credential.

Success proves only typed desired-state admission and a fresh Workloads
projection. First provisioning, domain lifecycle, guest-credential resolution,
and VDI readiness remain separate capability-bound prerequisites.
EOF
}

fail() {
  printf '%s: status=failed reason=%s\n' "$PROGRAM_NAME" "$1" >&2
  exit 1
}

run_python() {
  "$PYTHON_BIN" - "$@" <<'PY'
import hashlib
import hmac
import json
import os
import re
import resource
import secrets
import selectors
import stat
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace

ACTION_TOPIC = "action/cloud/browser-provision"
ACTION_VERB = "browser-provision"
ALERT_TOPIC = "event/toast/show"
ALERT_BODY = {
    "severity": "warning",
    "source_host": "deployment-controller",
    "flag": "AI-GENERATED-ALERT",
    "headline": "This seat will update in 5 seconds.",
}
STABLE_NAME = "browser-vm"
PROFILE_ID = "browser-vm-chromium"
SOURCE_REPOSITORY = "https://github.com/matthewmackes/magic-mesh.git"
SOURCE_PATH = "packaging/browser-vm/profile.env"
GUEST_CREDENTIAL_REF = "desktop/browser-vm/rdp"
SCHEMA_VERSION = 1
WAIT_SECONDS = 5
TOKEN_TTL_MS = 25_000
REPLY_TIMEOUT_SECONDS = 20.0
WORKLOAD_ROW_TIMEOUT_SECONDS = 330.0
PLACEMENT_STATE_MAX_AGE_MS = 120_000
PLACEMENT_STATE_FUTURE_SKEW_MS = 30_000
BUS_COMMAND_TIMEOUT_SECONDS = 8.0
REPLY_POLL_SECONDS = 0.25
WORKLOAD_ROW_POLL_SECONDS = 1.0
MAX_CREDENTIAL_BYTES = 65
MAX_REPLY_BYTES = 256 * 1024
HISTORY_READ_CHUNK_BYTES = 64 * 1024
HISTORY_PROCESS_TIMEOUT_SECONDS = 2.0
HISTORY_STOP_TIMEOUT_SECONDS = 0.25
ULID_RE = re.compile(r"[0-9A-HJKMNP-TV-Z]{26}\Z")
SEGMENT_RE = re.compile(r"[A-Za-z0-9._-]{1,255}\Z")
DIGEST_RE = re.compile(r"sha256:[0-9a-f]{64}\Z")
SOURCE_COMMIT_RE = re.compile(r"[0-9a-f]{40}\Z")
REQUIRED_PLACEMENT_TOOLS = ("opentofu", "ansible", "libvirt")
BROWSER_VCPU = 4
BROWSER_MEMORY_MB = 8192
BROWSER_DISK_GB = 64


class SafeFailure(Exception):
    def __init__(self, reason):
        super().__init__(reason)
        self.reason = reason


def validate_inputs(
    node,
    placement_receipt,
    name,
    profile_id,
    source_commit,
    image_digest,
    credential_ref,
):
    if (
        not SEGMENT_RE.fullmatch(node)
        or node in {".", ".."}
        or node.startswith("-")
    ):
        raise SafeFailure("invalid-node")
    if not ULID_RE.fullmatch(placement_receipt):
        raise SafeFailure("invalid-placement-receipt")
    if name != STABLE_NAME:
        raise SafeFailure("invalid-name")
    if profile_id != PROFILE_ID:
        raise SafeFailure("invalid-profile-id")
    if (
        not SOURCE_COMMIT_RE.fullmatch(source_commit)
        or source_commit == "0" * 40
    ):
        raise SafeFailure("invalid-source-commit")
    if not DIGEST_RE.fullmatch(image_digest):
        raise SafeFailure("invalid-image-digest")
    if image_digest == "sha256:" + "0" * 64:
        raise SafeFailure("invalid-image-digest")
    if credential_ref != GUEST_CREDENTIAL_REF:
        raise SafeFailure("invalid-credential-ref")


def browser_operator_intent(
    node,
    placement_receipt,
    name,
    profile_id,
    source_commit,
    image_digest,
    credential_ref,
):
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": "browser_vm_workload_operator_intent",
        "verb": ACTION_VERB,
        "node": node,
        "placement_receipt": placement_receipt,
        "name": name,
        "profile_id": profile_id,
        "source_repository": SOURCE_REPOSITORY,
        "source_path": SOURCE_PATH,
        "source_commit": source_commit,
        "image_digest": image_digest,
        "credential_ref": credential_ref,
    }


def object_sha256(value):
    return hashlib.sha256(canonical_json(value).encode("utf-8")).hexdigest()


def open_nofollow(path):
    if not os.path.isabs(path):
        raise SafeFailure("credential-unavailable")
    components = path.split("/")[1:]
    if not components or any(component in {"", ".", ".."} for component in components):
        raise SafeFailure("credential-unavailable")
    directory_fd = os.open("/", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        for component in components[:-1]:
            next_fd = os.open(
                component,
                os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW,
                dir_fd=directory_fd,
            )
            os.close(directory_fd)
            directory_fd = next_fd
        return os.open(
            components[-1],
            os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK,
            dir_fd=directory_fd,
        )
    except OSError as exc:
        raise SafeFailure("credential-unavailable") from exc
    finally:
        os.close(directory_fd)


def read_cloud_arm_key(path, expected_uid):
    fd = open_nofollow(path)
    try:
        metadata = os.fstat(fd)
        if not stat.S_ISREG(metadata.st_mode):
            raise SafeFailure("credential-unavailable")
        if (
            metadata.st_uid != expected_uid
            or metadata.st_nlink != 1
            or stat.S_IMODE(metadata.st_mode) & 0o077
        ):
            raise SafeFailure("credential-insecure")
        if metadata.st_size > MAX_CREDENTIAL_BYTES:
            raise SafeFailure("credential-oversized")

        chunks = []
        total = 0
        while total <= MAX_CREDENTIAL_BYTES:
            chunk = os.read(fd, min(4096, MAX_CREDENTIAL_BYTES + 1 - total))
            if not chunk:
                break
            chunks.append(chunk)
            total += len(chunk)
        if total > MAX_CREDENTIAL_BYTES:
            raise SafeFailure("credential-oversized")
        raw = b"".join(chunks).strip()
    except OSError as exc:
        raise SafeFailure("credential-unavailable") from exc
    finally:
        os.close(fd)

    if re.fullmatch(rb"[0-9A-Fa-f]{64}", raw) is None:
        raise SafeFailure("credential-malformed")
    return bytearray.fromhex(raw.decode("ascii"))


def canonical_json(value):
    return json.dumps(
        value,
        ensure_ascii=False,
        allow_nan=False,
        sort_keys=True,
        separators=(",", ":"),
    )


def cloud_request_digest(body):
    if not isinstance(body, dict):
        raise SafeFailure("request-invalid")
    unsigned = dict(body)
    unsigned.pop("armed_token", None)
    return hashlib.sha256(canonical_json(unsigned).encode("utf-8")).hexdigest()


def build_armed_request(key, node, name, image_digest, nonce, expires_at_ms):
    request = {
        "schema_version": SCHEMA_VERSION,
        "node": node,
        "name": name,
        "image_digest": image_digest,
    }
    request_sha256 = cloud_request_digest(request)
    signing_payload = (
        f"v2|{nonce}|{expires_at_ms}|{ACTION_VERB}|{node}|{name}|{request_sha256}"
    )
    signature = hmac.new(key, signing_payload.encode("utf-8"), hashlib.sha256).hexdigest()
    request["armed_token"] = f"{signing_payload}|{signature}"
    return canonical_json(request)


def publish_body(bus_bin, bus_root, topic, body, runner=subprocess.run):
    argv = [bus_bin, "publish", topic, "--bus-root", bus_root]
    try:
        completed = runner(
            argv,
            input=body.encode("utf-8"),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=BUS_COMMAND_TIMEOUT_SECONDS,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise SafeFailure("bus-publish-failed") from exc
    if completed.returncode != 0:
        raise SafeFailure("bus-publish-failed")
    try:
        receipt = completed.stdout.decode("ascii").strip()
    except UnicodeDecodeError as exc:
        raise SafeFailure("bus-receipt-invalid") from exc
    if ULID_RE.fullmatch(receipt) is None:
        raise SafeFailure("bus-receipt-invalid")
    return receipt


def decode_message(raw, expected_topic):
    if not raw.strip():
        return None
    if len(raw) > MAX_REPLY_BYTES:
        raise ValueError("oversized message")
    lines = [line for line in raw.splitlines() if line.strip()]
    if len(lines) != 1:
        raise ValueError("message history is not singular")
    envelope = json.loads(lines[0])
    if not isinstance(envelope, dict) or envelope.get("topic") != expected_topic:
        raise ValueError("message topic mismatch")
    receipt = envelope.get("ulid")
    if not isinstance(receipt, str) or ULID_RE.fullmatch(receipt) is None:
        raise ValueError("message receipt is invalid")
    timestamp = envelope.get("ts_unix_ms")
    if isinstance(timestamp, bool) or not isinstance(timestamp, int) or timestamp < 0:
        raise ValueError("message timestamp is invalid")
    body_raw = envelope.get("body")
    if not isinstance(body_raw, str) or len(body_raw.encode("utf-8")) > MAX_REPLY_BYTES:
        raise ValueError("message body is invalid")
    body = json.loads(body_raw)
    if not isinstance(body, dict):
        raise ValueError("message body is not an object")
    return envelope, body


def admitted_browser_spec(row, node, name, image_digest):
    return (
        isinstance(row, dict)
        and row.get("node") == node
        and row.get("name") == name
        and row.get("delivery_type") == "desktop_vm"
        and row.get("vcpu") == BROWSER_VCPU
        and row.get("memory_mb") == BROWSER_MEMORY_MB
        and row.get("disk_gb") == BROWSER_DISK_GB
        and row.get("image") == PROFILE_ID
        and row.get("image_digest") == image_digest
        and row.get("network_isolation") is False
        and row.get("raw_hcl") is None
        and row.get("app") is None
    )


def classify_reply(raw, expected_topic, node, name, image_digest):
    try:
        decoded = decode_message(raw, expected_topic)
    except (TypeError, ValueError, UnicodeError, json.JSONDecodeError):
        return "malformed", None
    if decoded is None:
        return "pending", None
    envelope, body = decoded
    receipt = envelope["ulid"]
    if body.get("verb") != ACTION_VERB:
        return "malformed", receipt
    if body.get("ok") is True:
        desired = body.get("desired")
        if not isinstance(desired, list) or len(desired) != 1:
            return "malformed", receipt
        admitted = admitted_browser_spec(desired[0], node, name, image_digest)
        return ("accepted" if admitted else "malformed"), receipt
    if body.get("ok") is not False:
        return "malformed", receipt
    if isinstance(body.get("gated"), str) and body["gated"]:
        return "gated", receipt
    if isinstance(body.get("error"), str) and body["error"]:
        return "rejected", receipt
    return "failed", receipt


def history_argv(bus_bin, bus_root, topic, since=None):
    argv = [bus_bin, "history", topic]
    if since is not None:
        argv.extend(["--since", since])
    argv.extend(["--count", "1", "--json", "--bus-root", bus_root])
    return argv


def stop_history_process(process):
    if process.poll() is None:
        try:
            process.terminate()
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=HISTORY_STOP_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            try:
                process.kill()
            except ProcessLookupError:
                pass
            process.wait()
    else:
        process.wait()


def read_latest_history(
    bus_bin,
    bus_root,
    topic,
    remaining,
    since=None,
    process_factory=subprocess.Popen,
):
    timeout = min(HISTORY_PROCESS_TIMEOUT_SECONDS, max(0.0, remaining))
    if timeout <= 0:
        return None
    try:
        process = process_factory(
            history_argv(bus_bin, bus_root, topic, since),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            bufsize=0,
        )
    except OSError as exc:
        raise SafeFailure("history-read-failed") from exc

    raw = bytearray()
    selector = selectors.DefaultSelector()
    try:
        if process.stdout is None:
            raise SafeFailure("history-read-failed")
        selector.register(process.stdout, selectors.EVENT_READ)
        deadline = time.monotonic() + timeout
        while True:
            poll_seconds = deadline - time.monotonic()
            if poll_seconds <= 0:
                return None
            if not selector.select(poll_seconds):
                return None
            read_size = min(
                HISTORY_READ_CHUNK_BYTES,
                MAX_REPLY_BYTES + 1 - len(raw),
            )
            chunk = os.read(process.stdout.fileno(), read_size)
            if not chunk:
                return_code = process.wait()
                if return_code != 0:
                    raise SafeFailure("history-read-failed")
                return bytes(raw)
            raw.extend(chunk)
            if len(raw) > MAX_REPLY_BYTES:
                raise SafeFailure("history-oversized")
    except OSError as exc:
        raise SafeFailure("history-read-failed") from exc
    finally:
        selector.close()
        if process.stdout is not None:
            process.stdout.close()
        stop_history_process(process)


def wait_for_reply(bus_bin, bus_root, receipt, node, name, image_digest):
    topic = f"reply/{receipt}"
    deadline = time.monotonic() + REPLY_TIMEOUT_SECONDS
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise SafeFailure("reply-timeout")
        raw_reply = read_latest_history(bus_bin, bus_root, topic, remaining)
        if raw_reply is None:
            continue
        status, reply_receipt = classify_reply(
            raw_reply, topic, node, name, image_digest
        )
        if status == "accepted":
            return reply_receipt
        if status != "pending":
            raise SafeFailure(f"reply-{status}")
        time.sleep(min(REPLY_POLL_SECONDS, max(0.0, deadline - time.monotonic())))


def strict_nonnegative_int(value):
    return not isinstance(value, bool) and isinstance(value, int) and value >= 0


def placement_state_status(raw, expected_topic, node, placement_receipt, now_ms):
    try:
        decoded = decode_message(raw, expected_topic)
    except (TypeError, ValueError, UnicodeError, json.JSONDecodeError):
        return "malformed"
    if decoded is None:
        return "unavailable"
    envelope, body = decoded
    if envelope["ulid"] != placement_receipt:
        return "receipt-not-latest"
    if body.get("host") != node:
        return "host-mismatch"
    if body.get("adapter") != "construct_cloud":
        return "construct-cloud-required"

    published_at_ms = body.get("published_at_ms")
    if not strict_nonnegative_int(published_at_ms):
        return "timestamp-invalid"
    for timestamp in (envelope["ts_unix_ms"], published_at_ms):
        age_ms = now_ms - timestamp
        if age_ms < -PLACEMENT_STATE_FUTURE_SKEW_MS:
            return "timestamp-future"
        if age_ms > PLACEMENT_STATE_MAX_AGE_MS:
            return "state-stale"

    if body.get("apply_armed") is not True:
        return "mutation-arming-required"
    health = body.get("health")
    if not isinstance(health, list):
        return "health-invalid"
    for tool in REQUIRED_PLACEMENT_TOOLS:
        matches = [
            row
            for row in health
            if isinstance(row, dict) and row.get("service_type") == tool
        ]
        if len(matches) != 1:
            return f"{tool}-health-required"
        if matches[0].get("state") != "up":
            return f"{tool}-not-up"

    capacity = body.get("node_capacity")
    if not isinstance(capacity, dict):
        return "capacity-required"
    values = {
        field: capacity.get(field)
        for field in ("vcpu_total", "vcpu_used", "mem_total_mb", "mem_used_mb")
    }
    if not all(strict_nonnegative_int(value) for value in values.values()):
        return "capacity-invalid"
    if values["vcpu_used"] > values["vcpu_total"]:
        return "capacity-invalid"
    if values["mem_used_mb"] > values["mem_total_mb"]:
        return "capacity-invalid"
    if values["vcpu_total"] - values["vcpu_used"] < BROWSER_VCPU:
        return "vcpu-capacity-required"
    if values["mem_total_mb"] - values["mem_used_mb"] < BROWSER_MEMORY_MB:
        return "memory-capacity-required"

    workloads = body.get("workloads")
    if not isinstance(workloads, list):
        return "workloads-projection-required"
    matches = [
        row
        for row in workloads
        if isinstance(row, dict) and row.get("name") == STABLE_NAME
    ]
    if len(matches) > 1:
        return "duplicate-browser-workload"
    if matches and (
        matches[0].get("node") != node
        or matches[0].get("delivery_type") != "desktop_vm"
    ):
        return "browser-workload-identity-conflict"
    return "ready"


def verify_placement_prerequisites(
    bus_bin, bus_root, node, placement_receipt, now_ms=None
):
    topic = f"state/cloud/{node}"
    raw = read_latest_history(
        bus_bin,
        bus_root,
        topic,
        HISTORY_PROCESS_TIMEOUT_SECONDS,
    )
    if raw is None:
        raise SafeFailure("placement-state-unavailable")
    if now_ms is None:
        now_ms = time.time_ns() // 1_000_000
    status = placement_state_status(
        raw, topic, node, placement_receipt, now_ms
    )
    if status != "ready":
        raise SafeFailure(f"placement-{status}")
    return topic


def classify_workload_row(raw, expected_topic, request_receipt, node, name):
    try:
        decoded = decode_message(raw, expected_topic)
    except (TypeError, ValueError, UnicodeError, json.JSONDecodeError):
        return "malformed", None, None
    if decoded is None:
        return "pending", None, None
    envelope, body = decoded
    projection_receipt = envelope["ulid"]
    if projection_receipt <= request_receipt:
        return "malformed", projection_receipt, None
    if body.get("host") != node:
        return "malformed", projection_receipt, None
    workloads = body.get("workloads")
    if not isinstance(workloads, list):
        return "malformed", projection_receipt, None
    matches = [
        row
        for row in workloads
        if isinstance(row, dict)
        and row.get("node") == node
        and row.get("name") == name
    ]
    if not matches:
        return "pending", projection_receipt, None
    if len(matches) != 1:
        return "malformed", projection_receipt, None
    row = matches[0]
    status = row.get("status")
    if (
        row.get("delivery_type") != "desktop_vm"
        or row.get("disk_gb") != BROWSER_DISK_GB
        or not isinstance(status, str)
        or not status
        or len(status.encode("utf-8")) > 64
        or any(not character.isprintable() for character in status)
        or not isinstance(row.get("reachable"), bool)
    ):
        return "malformed", projection_receipt, None
    projection = {
        "status": status,
        "reachable": row["reachable"],
        "disk_gb": row["disk_gb"],
    }
    return "projected", projection_receipt, projection


def wait_for_workload_row(bus_bin, bus_root, receipt, node, name):
    topic = f"state/cloud/{node}"
    deadline = time.monotonic() + WORKLOAD_ROW_TIMEOUT_SECONDS
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise SafeFailure("workload-row-timeout")
        raw_state = read_latest_history(
            bus_bin, bus_root, topic, remaining, since=receipt
        )
        if raw_state is None:
            continue
        status, projection_receipt, projection = classify_workload_row(
            raw_state, topic, receipt, node, name
        )
        if status == "projected":
            return projection_receipt, projection
        if status != "pending":
            raise SafeFailure(f"workload-row-{status}")
        time.sleep(
            min(WORKLOAD_ROW_POLL_SECONDS, max(0.0, deadline - time.monotonic()))
        )


def verify_runtime_paths(bus_bin, bus_root):
    if not os.path.isabs(bus_bin) or not os.access(bus_bin, os.X_OK):
        raise SafeFailure("bus-unavailable")
    try:
        root_metadata = os.lstat(bus_root)
        index_metadata = os.lstat(os.path.join(bus_root, "index.sqlite"))
    except OSError as exc:
        raise SafeFailure("bus-unavailable") from exc
    if not stat.S_ISDIR(root_metadata.st_mode) or stat.S_ISLNK(root_metadata.st_mode):
        raise SafeFailure("bus-unavailable")
    if not stat.S_ISREG(index_metadata.st_mode) or stat.S_ISLNK(index_metadata.st_mode):
        raise SafeFailure("bus-unavailable")


def success_receipt(
    operator_intent,
    intent_sha256,
    request_sha256,
    placement_topic,
    alert_receipt,
    request_receipt,
    reply_receipt,
    projection_receipt,
    projection,
):
    node = operator_intent["node"]
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": "browser_vm_workload_request_receipt",
        "status": "desired_state_projected",
        "operator_intent": operator_intent,
        "operator_intent_sha256": intent_sha256,
        "workloads_confirmation": {
            "verb": ACTION_VERB,
            "delivery_type": "desktop_vm",
            "profile_id": PROFILE_ID,
            "vcpu": BROWSER_VCPU,
            "memory_mb": BROWSER_MEMORY_MB,
            "disk_gb": BROWSER_DISK_GB,
            "image_digest": operator_intent["image_digest"],
            "projected_status": projection["status"],
            "projected_reachable": projection["reachable"],
        },
        "correlation": {
            "placement": {
                "topic": placement_topic,
                "receipt": operator_intent["placement_receipt"],
            },
            "alert": {"topic": ALERT_TOPIC, "receipt": alert_receipt},
            "request": {
                "topic": ACTION_TOPIC,
                "receipt": request_receipt,
                "request_sha256": request_sha256,
            },
            "reply": {
                "topic": f"reply/{request_receipt}",
                "receipt": reply_receipt,
            },
            "projection": {
                "topic": f"state/cloud/{node}",
                "receipt": projection_receipt,
            },
        },
        "remaining_live_prerequisites": [
            {
                "id": "artifact-source-attestation",
                "required": (
                    "placement-local browser-vm-chromium source must be the immutable "
                    "operator_intent source_commit and image_digest; the Workloads "
                    "projection does not attest the host-local qcow2 source"
                ),
            },
            {
                "id": "kvm-and-storage-admission",
                "required": (
                    f"placement must expose KVM and at least {BROWSER_DISK_GB} GiB usable "
                    "workload storage; those capabilities are absent from state/cloud"
                ),
            },
            {
                "id": "first-provisioning",
                "required": (
                    "a separate capability-bound action/cloud/provision request and "
                    "successful correlated reply; this helper never invokes it"
                ),
            },
            {
                "id": "guest-credential-resolution",
                "required": (
                    f"the session boundary must resolve opaque reference "
                    f"{operator_intent['credential_ref']}; no credential value is accepted here"
                ),
            },
            {
                "id": "lifecycle-and-vdi-readiness",
                "required": (
                    "an admitted domain must be active/reachable (using only a typed, "
                    "target-bound lifecycle verb when needed) and advertise the explicitly "
                    "selected RDP VDI source; no protocol fallback is inferred"
                ),
            },
        ],
    }


def live_request(
    credential_path,
    node,
    placement_receipt,
    name,
    profile_id,
    source_commit,
    image_digest,
    credential_ref,
    bus_bin,
    bus_root,
):
    validate_inputs(
        node,
        placement_receipt,
        name,
        profile_id,
        source_commit,
        image_digest,
        credential_ref,
    )
    verify_runtime_paths(bus_bin, bus_root)
    placement_topic = verify_placement_prerequisites(
        bus_bin, bus_root, node, placement_receipt
    )
    operator_intent = browser_operator_intent(
        node,
        placement_receipt,
        name,
        profile_id,
        source_commit,
        image_digest,
        credential_ref,
    )
    intent_sha256 = object_sha256(operator_intent)
    key = read_cloud_arm_key(credential_path, 0)
    try:
        alert_body = canonical_json(ALERT_BODY)
        alert_receipt = publish_body(bus_bin, bus_root, ALERT_TOPIC, alert_body)
        print(
            "request-browser-vm-workload: alert=published "
            f"receipt={alert_receipt} wait_seconds={WAIT_SECONDS}",
            file=sys.stderr,
            flush=True,
        )
        time.sleep(WAIT_SECONDS)

        nonce = secrets.token_hex(16)
        expires_at_ms = time.time_ns() // 1_000_000 + TOKEN_TTL_MS
        request_body = build_armed_request(
            key, node, name, image_digest, nonce, expires_at_ms
        )
        request_sha256 = cloud_request_digest(json.loads(request_body))
    finally:
        for index in range(len(key)):
            key[index] = 0

    request_receipt = publish_body(bus_bin, bus_root, ACTION_TOPIC, request_body)
    request_body = None
    reply_receipt = wait_for_reply(
        bus_bin, bus_root, request_receipt, node, name, image_digest
    )
    projection_receipt, projection = wait_for_workload_row(
        bus_bin, bus_root, request_receipt, node, name
    )
    receipt = success_receipt(
        operator_intent,
        intent_sha256,
        request_sha256,
        placement_topic,
        alert_receipt,
        request_receipt,
        reply_receipt,
        projection_receipt,
        projection,
    )
    print(canonical_json(receipt))


def expect_safe_failure(call):
    try:
        call()
    except SafeFailure:
        return
    raise AssertionError("unsafe input was accepted")


def expect_failure_reason(call, reason):
    try:
        call()
    except SafeFailure as exc:
        assert exc.reason == reason, (exc.reason, reason)
        return
    raise AssertionError(f"expected failure {reason} was not raised")


def self_test():
    assert ACTION_TOPIC == "action/cloud/browser-provision"
    assert ACTION_VERB == "browser-provision"
    assert ALERT_TOPIC == "event/toast/show"
    assert ALERT_BODY["flag"] == "AI-GENERATED-ALERT"
    assert ALERT_BODY["headline"] == "This seat will update in 5 seconds."
    assert PROFILE_ID == "browser-vm-chromium"
    assert GUEST_CREDENTIAL_REF == "desktop/browser-vm/rdp"
    assert WAIT_SECONDS == 5
    assert 0 < TOKEN_TTL_MS <= 30_000
    assert WORKLOAD_ROW_TIMEOUT_SECONDS >= 300.0
    assert resource.getrlimit(resource.RLIMIT_CORE) == (0, 0)

    node = "DELL-LAPTOP"
    placement_receipt = "01ARZ3NDEKTSV4RRFFQ69G5FAA"
    name = STABLE_NAME
    source_commit = "0123456789abcdef0123456789abcdef01234567"
    image_digest = "sha256:" + "0123456789abcdef" * 4
    validate_inputs(
        node,
        placement_receipt,
        name,
        PROFILE_ID,
        source_commit,
        image_digest,
        GUEST_CREDENTIAL_REF,
    )
    for bad_node in (
        "",
        ".",
        "..",
        "-option",
        "Dell/other",
        "Dell seat",
        "x" * 256,
    ):
        expect_safe_failure(
            lambda bad_node=bad_node: validate_inputs(
                bad_node,
                placement_receipt,
                name,
                PROFILE_ID,
                source_commit,
                image_digest,
                GUEST_CREDENTIAL_REF,
            )
        )
    invalid_cases = [
        ("bad-placement", name, PROFILE_ID, source_commit, image_digest, GUEST_CREDENTIAL_REF),
        (placement_receipt, "browser-dell", PROFILE_ID, source_commit, image_digest, GUEST_CREDENTIAL_REF),
        (placement_receipt, name, "generic-desktop", source_commit, image_digest, GUEST_CREDENTIAL_REF),
        (placement_receipt, name, PROFILE_ID, "0" * 40, image_digest, GUEST_CREDENTIAL_REF),
        (placement_receipt, name, PROFILE_ID, source_commit.upper(), image_digest, GUEST_CREDENTIAL_REF),
        (placement_receipt, name, PROFILE_ID, source_commit, "sha256:abcd", GUEST_CREDENTIAL_REF),
        (placement_receipt, name, PROFILE_ID, source_commit, "sha256:" + "0" * 64, GUEST_CREDENTIAL_REF),
        (placement_receipt, name, PROFILE_ID, source_commit, image_digest.upper(), GUEST_CREDENTIAL_REF),
        (placement_receipt, name, PROFILE_ID, source_commit, image_digest, "plaintext-password"),
    ]
    for case in invalid_cases:
        expect_safe_failure(lambda case=case: validate_inputs(node, *case))

    operator_intent = browser_operator_intent(
        node,
        placement_receipt,
        name,
        PROFILE_ID,
        source_commit,
        image_digest,
        GUEST_CREDENTIAL_REF,
    )
    assert operator_intent["source_repository"] == SOURCE_REPOSITORY
    assert operator_intent["source_path"] == SOURCE_PATH
    assert operator_intent["source_commit"] == source_commit
    assert operator_intent["credential_ref"] == GUEST_CREDENTIAL_REF
    serialized_intent = canonical_json(operator_intent)
    assert "password" not in serialized_intent
    assert "raw_hcl" not in serialized_intent
    assert "command" not in serialized_intent
    assert object_sha256(operator_intent) == hashlib.sha256(
        serialized_intent.encode("utf-8")
    ).hexdigest()

    nested = {
        "z": [{"b": 2, "a": 1}],
        "a": {"y": True, "x": None, "armed_token": "nested"},
        "armed_token": "top-level",
    }
    unsigned_nested = dict(nested)
    unsigned_nested.pop("armed_token")
    assert canonical_json(unsigned_nested) == (
        '{"a":{"armed_token":"nested","x":null,"y":true},'
        '"z":[{"a":1,"b":2}]}'
    )
    assert cloud_request_digest(nested) == hashlib.sha256(
        canonical_json(unsigned_nested).encode("utf-8")
    ).hexdigest()

    fixture_key = bytearray.fromhex(
        "000102030405060708090a0b0c0d0e0f"
        "101112131415161718191a1b1c1d1e1f"
    )
    request = build_armed_request(
        fixture_key,
        node,
        name,
        image_digest,
        "00112233445566778899aabbccddeeff",
        1_893_456_000_123,
    )
    request_object = json.loads(request)
    assert set(request_object) == {
        "schema_version",
        "node",
        "name",
        "image_digest",
        "armed_token",
    }
    assert not ({"command", "argv", "raw_hcl", "placement"} & set(request_object))
    assert cloud_request_digest(request_object) == (
        "070acdcb32515efc52cd878e5c7ba9e920baa9d61aec11b81b2a09284becd415"
    )
    assert request_object["armed_token"] == (
        "v2|00112233445566778899aabbccddeeff|1893456000123|"
        "browser-provision|DELL-LAPTOP|browser-vm|"
        "070acdcb32515efc52cd878e5c7ba9e920baa9d61aec11b81b2a09284becd415|"
        "59111ea4b3f0bb55b7d00777b375bfc74aa62d114f9d1c4fee12280e351f9ba2"
    )

    with tempfile.TemporaryDirectory() as directory:
        good = os.path.join(directory, "cloud-arm-key")
        with open(good, "wb") as stream:
            stream.write(b"00" * 32 + b"\n")
        os.chmod(good, 0o600)
        loaded = read_cloud_arm_key(good, os.geteuid())
        assert loaded == bytearray(32)

        link = os.path.join(directory, "link")
        os.symlink(good, link)
        expect_safe_failure(lambda: read_cloud_arm_key(link, os.geteuid()))

        parent_link = os.path.join(directory, "parent-link")
        os.symlink(directory, parent_link)
        expect_safe_failure(
            lambda: read_cloud_arm_key(os.path.join(parent_link, "cloud-arm-key"), os.geteuid())
        )

        os.chmod(good, 0o644)
        expect_safe_failure(lambda: read_cloud_arm_key(good, os.geteuid()))
        os.chmod(good, 0o600)

        oversized = os.path.join(directory, "oversized")
        with open(oversized, "wb") as stream:
            stream.write(b"0" * (MAX_CREDENTIAL_BYTES + 1))
        os.chmod(oversized, 0o600)
        expect_safe_failure(lambda: read_cloud_arm_key(oversized, os.geteuid()))

        malformed = os.path.join(directory, "malformed")
        with open(malformed, "wb") as stream:
            stream.write(b"g" * 64)
        os.chmod(malformed, 0o600)
        expect_safe_failure(lambda: read_cloud_arm_key(malformed, os.geteuid()))
        expect_safe_failure(lambda: read_cloud_arm_key("relative/key", os.geteuid()))

    calls = []

    def fake_runner(argv, **kwargs):
        calls.append((argv, kwargs))
        return SimpleNamespace(
            returncode=0,
            stdout=b"01ARZ3NDEKTSV4RRFFQ69G5FAV\n",
            stderr=b"",
        )

    sensitive_body = '{"armed_token":"must-not-enter-argv-or-output"}'
    receipt = publish_body(
        "/usr/bin/mde-bus",
        "/run/mde-bus",
        ACTION_TOPIC,
        sensitive_body,
        runner=fake_runner,
    )
    assert receipt == "01ARZ3NDEKTSV4RRFFQ69G5FAV"
    argv, kwargs = calls[0]
    assert sensitive_body.encode("utf-8") == kwargs["input"]
    assert all("must-not-enter-argv-or-output" not in argument for argument in argv)
    assert sensitive_body.encode("utf-8") not in receipt.encode("ascii")

    topic = f"reply/{receipt}"
    reply_receipt = "01ARZ3NDEKTSV4RRFFQ69G5FAW"
    reply_body = canonical_json(
        {
            "ok": True,
            "verb": ACTION_VERB,
            "desired": [
                {
                    "node": node,
                    "name": name,
                    "delivery_type": "desktop_vm",
                    "vcpu": BROWSER_VCPU,
                    "memory_mb": BROWSER_MEMORY_MB,
                    "disk_gb": BROWSER_DISK_GB,
                    "image": PROFILE_ID,
                    "image_digest": image_digest,
                    "network_isolation": False,
                }
            ],
        }
    )
    reply_envelope = canonical_json(
        {
            "ulid": reply_receipt,
            "topic": topic,
            "ts_unix_ms": 1_893_456_000_124,
            "body": reply_body,
        }
    ).encode()
    assert classify_reply(reply_envelope, topic, node, name, image_digest) == (
        "accepted",
        reply_receipt,
    )
    assert classify_reply(b"", topic, node, name, image_digest) == (
        "pending",
        None,
    )
    assert classify_reply(b"not-json", topic, node, name, image_digest) == (
        "malformed",
        None,
    )
    assert classify_reply(
        b"x" * (MAX_REPLY_BYTES + 1), topic, node, name, image_digest
    ) == ("malformed", None)
    wrong_profile_body = json.loads(reply_body)
    wrong_profile_body["desired"][0]["image"] = "generic-desktop"
    wrong_profile = canonical_json(
        {
            "ulid": reply_receipt,
            "topic": topic,
            "ts_unix_ms": 1_893_456_000_124,
            "body": canonical_json(wrong_profile_body),
        }
    ).encode()
    assert classify_reply(wrong_profile, topic, node, name, image_digest)[0] == "malformed"
    raw_hcl_body = json.loads(reply_body)
    raw_hcl_body["desired"][0]["raw_hcl"] = "command = true"
    raw_hcl_reply = canonical_json(
        {
            "ulid": reply_receipt,
            "topic": topic,
            "ts_unix_ms": 1_893_456_000_124,
            "body": canonical_json(raw_hcl_body),
        }
    ).encode()
    assert classify_reply(raw_hcl_reply, topic, node, name, image_digest)[0] == "malformed"

    state_topic = f"state/cloud/{node}"
    placement_now_ms = 1_893_456_000_123
    placement_body = {
        "host": node,
        "adapter": "construct_cloud",
        "health": [
            {"service_type": tool, "state": "up"}
            for tool in REQUIRED_PLACEMENT_TOOLS
        ],
        "apply_armed": True,
        "published_at_ms": placement_now_ms,
        "workloads": [],
        "node_capacity": {
            "vcpu_total": 12,
            "vcpu_used": 4,
            "mem_total_mb": 24_576,
            "mem_used_mb": 8_192,
        },
    }
    placement_envelope = canonical_json(
        {
            "ulid": placement_receipt,
            "topic": state_topic,
            "ts_unix_ms": placement_now_ms,
            "body": canonical_json(placement_body),
        }
    ).encode()
    assert placement_state_status(
        placement_envelope,
        state_topic,
        node,
        placement_receipt,
        placement_now_ms,
    ) == "ready"

    def placement_status_with(change, *, now_ms=placement_now_ms):
        body = json.loads(canonical_json(placement_body))
        change(body)
        envelope = canonical_json(
            {
                "ulid": placement_receipt,
                "topic": state_topic,
                "ts_unix_ms": placement_now_ms,
                "body": canonical_json(body),
            }
        ).encode()
        return placement_state_status(
            envelope, state_topic, node, placement_receipt, now_ms
        )

    assert placement_status_with(
        lambda body: body.update(adapter="simulator")
    ) == "construct-cloud-required"
    assert placement_status_with(
        lambda body: body.update(apply_armed=False)
    ) == "mutation-arming-required"
    assert placement_status_with(
        lambda body: body["health"][0].update(state="down")
    ) == "opentofu-not-up"
    assert placement_status_with(
        lambda body: body["node_capacity"].update(vcpu_used=9)
    ) == "vcpu-capacity-required"
    assert placement_status_with(
        lambda body: body["node_capacity"].update(mem_used_mb=17_000)
    ) == "memory-capacity-required"
    assert placement_status_with(
        lambda body: None,
        now_ms=placement_now_ms + PLACEMENT_STATE_MAX_AGE_MS + 1,
    ) == "state-stale"
    assert placement_state_status(
        placement_envelope,
        state_topic,
        node,
        "01ARZ3NDEKTSV4RRFFQ69G5FAB",
        placement_now_ms,
    ) == "receipt-not-latest"

    projection_receipt = "01ARZ3NDEKTSV4RRFFQ69G5FAX"
    state_body = canonical_json(
        {
            "host": node,
            "workloads": [
                {
                    "name": name,
                    "delivery_type": "desktop_vm",
                    "node": node,
                    "status": "active",
                    "reachable": True,
                    "disk_gb": BROWSER_DISK_GB,
                }
            ],
        }
    )
    state_envelope = canonical_json(
        {
            "ulid": projection_receipt,
            "topic": state_topic,
            "ts_unix_ms": 1_893_456_000_130,
            "body": state_body,
        }
    ).encode()
    assert classify_workload_row(
        state_envelope, state_topic, receipt, node, name
    ) == (
        "projected",
        projection_receipt,
        {"status": "active", "reachable": True, "disk_gb": BROWSER_DISK_GB},
    )
    empty_state = canonical_json(
        {
            "ulid": projection_receipt,
            "topic": state_topic,
            "ts_unix_ms": 1_893_456_000_130,
            "body": canonical_json({"host": node, "workloads": []}),
        }
    ).encode()
    assert classify_workload_row(empty_state, state_topic, receipt, node, name)[0] == "pending"
    wrong_delivery_body = json.loads(state_body)
    wrong_delivery_body["workloads"][0]["delivery_type"] = "service_vm"
    wrong_delivery = canonical_json(
        {
            "ulid": projection_receipt,
            "topic": state_topic,
            "ts_unix_ms": 1_893_456_000_130,
            "body": canonical_json(wrong_delivery_body),
        }
    ).encode()
    assert classify_workload_row(
        wrong_delivery, state_topic, receipt, node, name
    )[0] == "malformed"
    duplicate_body = json.loads(state_body)
    duplicate_body["workloads"].append(dict(duplicate_body["workloads"][0]))
    duplicate = canonical_json(
        {
            "ulid": projection_receipt,
            "topic": state_topic,
            "ts_unix_ms": 1_893_456_000_130,
            "body": canonical_json(duplicate_body),
        }
    ).encode()
    assert classify_workload_row(
        duplicate, state_topic, receipt, node, name
    )[0] == "malformed"
    stale_projection = canonical_json(
        {
            "ulid": placement_receipt,
            "topic": state_topic,
            "ts_unix_ms": 1_893_456_000_130,
            "body": state_body,
        }
    ).encode()
    assert classify_workload_row(
        stale_projection, state_topic, receipt, node, name
    )[0] == "malformed"

    final_receipt = success_receipt(
        operator_intent,
        object_sha256(operator_intent),
        cloud_request_digest(request_object),
        state_topic,
        "01ARZ3NDEKTSV4RRFFQ69G5FAB",
        receipt,
        reply_receipt,
        projection_receipt,
        {"status": "active", "reachable": True, "disk_gb": BROWSER_DISK_GB},
    )
    assert final_receipt["status"] == "desired_state_projected"
    assert final_receipt["correlation"]["request"]["receipt"] == receipt
    assert final_receipt["correlation"]["reply"]["topic"] == f"reply/{receipt}"
    assert final_receipt["correlation"]["reply"]["receipt"] == reply_receipt
    assert final_receipt["correlation"]["projection"]["receipt"] == projection_receipt
    assert final_receipt["operator_intent"]["credential_ref"] == GUEST_CREDENTIAL_REF
    assert len(final_receipt["remaining_live_prerequisites"]) == 5
    serialized_receipt = canonical_json(final_receipt)
    assert "armed_token" not in serialized_receipt
    assert "must-not-enter-argv-or-output" not in serialized_receipt
    assert '"password"' not in serialized_receipt
    assert '"command"' not in serialized_receipt
    history = history_argv(
        "/usr/bin/mde-bus", "/run/mde-bus", state_topic, receipt
    )
    assert history == [
        "/usr/bin/mde-bus",
        "history",
        state_topic,
        "--since",
        receipt,
        "--count",
        "1",
        "--json",
        "--bus-root",
        "/run/mde-bus",
    ]

    with tempfile.TemporaryDirectory() as history_root:
        mock_bus = os.path.join(history_root, "mde-bus")
        mock_source = (
            "#!/usr/bin/python3\n"
            "import os\n"
            "import sys\n"
            "import time\n"
            f"maximum = {MAX_REPLY_BYTES}\n"
            "topic = sys.argv[2]\n"
            "bus_root = sys.argv[sys.argv.index('--bus-root') + 1]\n"
            "if topic == 'side-file':\n"
            "    with open(os.path.join(bus_root, 'side-file'), 'wb') as stream:\n"
            "        stream.write(b's' * (maximum + 4096))\n"
            "    os.write(1, b'bounded-history\\n')\n"
            "elif topic == 'oversized':\n"
            "    emitted = 0\n"
            "    while emitted <= maximum:\n"
            "        emitted += os.write(1, b'x' * 65536)\n"
            "    time.sleep(10)\n"
            "elif topic == 'timeout':\n"
            "    time.sleep(10)\n"
            "elif topic == 'failure':\n"
            "    raise SystemExit(7)\n"
            "else:\n"
            "    raise SystemExit(8)\n"
        )
        with open(mock_bus, "w", encoding="utf-8") as stream:
            stream.write(mock_source)
        os.chmod(mock_bus, 0o700)

        spawned_processes = []

        def tracked_process_factory(*args, **kwargs):
            process = subprocess.Popen(*args, **kwargs)
            spawned_processes.append(process)
            return process

        def assert_process_reaped(process):
            assert process.returncode is not None
            try:
                wait_result = os.waitpid(process.pid, os.WNOHANG)
            except ChildProcessError:
                return
            raise AssertionError(
                f"history subprocess remained waitable: {wait_result}"
            )

        small_history = read_latest_history(
            mock_bus,
            history_root,
            "side-file",
            1.0,
            process_factory=tracked_process_factory,
        )
        assert small_history == b"bounded-history\n"
        assert os.path.getsize(os.path.join(history_root, "side-file")) == (
            MAX_REPLY_BYTES + 4096
        )
        assert len(spawned_processes) == 1
        assert_process_reaped(spawned_processes[-1])

        try:
            read_latest_history(
                mock_bus,
                history_root,
                "oversized",
                1.0,
                process_factory=tracked_process_factory,
            )
        except SafeFailure as exc:
            assert exc.reason == "history-oversized"
        else:
            raise AssertionError("oversized history subprocess was accepted")
        assert len(spawned_processes) == 2
        assert_process_reaped(spawned_processes[-1])

        assert read_latest_history(
            mock_bus,
            history_root,
            "timeout",
            0.05,
            process_factory=tracked_process_factory,
        ) is None
        assert len(spawned_processes) == 3
        assert_process_reaped(spawned_processes[-1])

        try:
            read_latest_history(
                mock_bus,
                history_root,
                "failure",
                1.0,
                process_factory=tracked_process_factory,
            )
        except SafeFailure as exc:
            assert exc.reason == "history-read-failed"
        else:
            raise AssertionError("failed history subprocess was accepted")
        assert len(spawned_processes) == 4
        assert_process_reaped(spawned_processes[-1])


def main():
    if len(sys.argv) == 2 and sys.argv[1] == "self-test":
        self_test()
        return
    if len(sys.argv) == 12 and sys.argv[1] == "live":
        live_request(*sys.argv[2:])
        return
    raise SafeFailure("invalid-invocation")


try:
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    main()
except SafeFailure as exc:
    print(
        f"request-browser-vm-workload: status=failed reason={exc.reason}",
        file=sys.stderr,
    )
    raise SystemExit(1)
except KeyboardInterrupt:
    print(
        "request-browser-vm-workload: status=interrupted mutation_status=unknown",
        file=sys.stderr,
    )
    raise SystemExit(130)
except Exception:
    print(
        "request-browser-vm-workload: status=failed reason=internal-error",
        file=sys.stderr,
    )
    raise SystemExit(1)
PY
}

if [[ "${1:-}" == "--self-test" ]]; then
  (($# == 1)) || {
    usage
    exit 2
  }
  [[ -x "$PYTHON_BIN" ]] || fail "python-unavailable"
  run_python self-test >/dev/null || fail "self-test-failed"
  printf '%s: self-test passed\n' "$PROGRAM_NAME"
  exit 0
fi

node=""
placement_receipt=""
name=""
profile_id=""
source_commit=""
image_digest=""
credential_ref=""
credential_path="${CREDENTIALS_DIRECTORY:+${CREDENTIALS_DIRECTORY}/cloud-arm-key}"
node_seen=0
placement_seen=0
name_seen=0
profile_seen=0
source_seen=0
digest_seen=0
credential_ref_seen=0
credential_seen=0

while (($#)); do
  case "$1" in
    --node)
      (($# >= 2)) || {
        usage
        exit 2
      }
      ((node_seen == 0)) || fail "duplicate-node"
      node="$2"
      node_seen=1
      shift 2
      ;;
    --placement-receipt)
      (($# >= 2)) || {
        usage
        exit 2
      }
      ((placement_seen == 0)) || fail "duplicate-placement-receipt"
      placement_receipt="$2"
      placement_seen=1
      shift 2
      ;;
    --name)
      (($# >= 2)) || {
        usage
        exit 2
      }
      ((name_seen == 0)) || fail "duplicate-name"
      name="$2"
      name_seen=1
      shift 2
      ;;
    --profile-id)
      (($# >= 2)) || {
        usage
        exit 2
      }
      ((profile_seen == 0)) || fail "duplicate-profile-id"
      profile_id="$2"
      profile_seen=1
      shift 2
      ;;
    --source-commit)
      (($# >= 2)) || {
        usage
        exit 2
      }
      ((source_seen == 0)) || fail "duplicate-source-commit"
      source_commit="$2"
      source_seen=1
      shift 2
      ;;
    --image-digest)
      (($# >= 2)) || {
        usage
        exit 2
      }
      ((digest_seen == 0)) || fail "duplicate-image-digest"
      image_digest="$2"
      digest_seen=1
      shift 2
      ;;
    --credential-ref)
      (($# >= 2)) || {
        usage
        exit 2
      }
      ((credential_ref_seen == 0)) || fail "duplicate-credential-ref"
      credential_ref="$2"
      credential_ref_seen=1
      shift 2
      ;;
    --credential-path)
      (($# >= 2)) || {
        usage
        exit 2
      }
      ((credential_seen == 0)) || fail "duplicate-credential-path"
      credential_path="$2"
      credential_seen=1
      shift 2
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

((
  node_seen == 1 &&
    placement_seen == 1 &&
    name_seen == 1 &&
    profile_seen == 1 &&
    source_seen == 1 &&
    digest_seen == 1 &&
    credential_ref_seen == 1
)) || {
  usage
  exit 2
}
[[ "$(id -u)" == "0" ]] || fail "root-required"
[[ -x "$PYTHON_BIN" ]] || fail "python-unavailable"
[[ -x "$MDE_BUS_BIN" ]] || fail "bus-unavailable"
[[ -n "$credential_path" ]] || fail "credential-directory-required"

run_python live \
  "$credential_path" \
  "$node" \
  "$placement_receipt" \
  "$name" \
  "$profile_id" \
  "$source_commit" \
  "$image_digest" \
  "$credential_ref" \
  "$MDE_BUS_BIN" \
  "$BUS_ROOT"
