#!/usr/bin/env bash
# Start the stable Browser VM through the sole Workloads authority.
#
# This helper deliberately has no libvirt, QEMU, console, or placement
# implementation. It creates one capability-bound StartAndAttach request and
# waits for the authoritative Browser RDP lease. The compute worker owns every
# provider side effect and publishes only after the fixed guest endpoint is
# ready.
set -euo pipefail
set +x
umask 077
ulimit -S -c 0
ulimit -H -c 0

readonly PROGRAM_NAME=request-browser-vm-workload
readonly PYTHON_BIN=/usr/bin/python3
readonly MDE_BUS_BIN=/usr/bin/mde-bus
readonly BUS_ROOT=/run/mde-bus
readonly DEFAULT_ACTION=start_and_attach

usage() {
  cat >&2 <<'EOF'
usage: request-browser-vm-workload --node NODE \
  [--action ACTION] [--image-ref browser-vm-chromium:VERSION] \
  [--credential-path PATH]
       request-browser-vm-workload --self-test

Starts, stops, or recovers the fixed browser-vm through
action/workload/operation. ACTION is one of start_and_attach (the default),
start, stop, restart, resume, or destroy. The image reference is required for
start and start_and_attach, and is always an approved catalog reference.
IMAGE-REF must name an already approved, promoted VM image in the local mesh
catalog. This command never accepts a VM path, SPICE/RDP endpoint, password,
or provider command. Existing browser-vm domains must first be converted with
packaging/browser-vm/migrate-display1-domain.sh; conversion preserves the
guest overlay and never force-destroys a running VM.
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
import stat
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace

ACTION_TOPIC = "action/workload/operation"
ACTION_VERB = "workload-operation"
STATE_PREFIX = "state/workloads/"
SCHEMA_VERSION = 1
WORKLOAD_NAME = "browser-vm"
BACKEND = "libvirt_virtqemud"
DEFAULT_ACTION = "start_and_attach"
SUPPORTED_ACTIONS = frozenset({
    "start_and_attach", "start", "stop", "restart", "resume", "destroy",
})
IMAGE_REQUIRED_ACTIONS = frozenset({"start_and_attach", "start"})
EXISTING_WORKLOAD_ACTIONS = frozenset({"stop", "restart", "resume", "destroy"})
ATTACHMENT = "rdp"
# Keep one hardware thread available to Dom0 on the four-thread Dell seat.
# Three guest cores preserve interactive parallelism without letting QEMU
# contend for every host thread during shell, sync, and Bus activity.
# Match the typed WorkloadProfile::Small contract so a four-thread host keeps
# its reserved CPU available and can still exercise the live attachment path.
BROWSER_VCPU = 2
BROWSER_MEMORY_MB = 4096
BROWSER_DISK_GB = 32
TOKEN_TTL_MS = 25_000
RDP_OPERATION_TTL_MS = 15 * 60 * 1_000
OPERATION_TIMEOUT_SECONDS = 330.0
BUS_COMMAND_TIMEOUT_SECONDS = 8.0
POLL_SECONDS = 0.5
MAX_BYTES = 256 * 1024
MAX_CREDENTIAL_BYTES = 65
ULID_RE = re.compile(r"[0-9A-HJKMNP-TV-Z]{26}\Z")
IDENTIFIER_RE = re.compile(r"[A-Za-z0-9._:-]{1,128}\Z")
IMAGE_REF_RE = re.compile(r"[A-Za-z0-9._-]{1,63}:[A-Za-z0-9._-]{1,63}\Z")


class SafeFailure(Exception):
    def __init__(self, reason):
        self.reason = reason
        super().__init__(reason)


def canonical_json(value):
    return json.dumps(value, ensure_ascii=False, allow_nan=False, sort_keys=True,
                      separators=(",", ":"))


def current_ms():
    return time.time_ns() // 1_000_000


def workload_id(node):
    del node
    return WORKLOAD_NAME


def validate_node(node):
    if not IDENTIFIER_RE.fullmatch(node) or node in {".", ".."} or node.startswith("-"):
        raise SafeFailure("invalid-node")


def validate_image_ref(image_ref):
    if not IMAGE_REF_RE.fullmatch(image_ref) or image_ref.startswith("-"):
        raise SafeFailure("invalid-image-ref")
    name, _version = image_ref.split(":", 1)
    if name != "browser-vm-chromium":
        raise SafeFailure("invalid-image-ref")


def read_arm_key(path, expected_uid):
    if not os.path.isabs(path):
        raise SafeFailure("credential-unavailable")
    pieces = path.split("/")[1:]
    if not pieces or any(piece in {"", ".", ".."} for piece in pieces):
        raise SafeFailure("credential-unavailable")
    directory = os.open("/", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        for piece in pieces[:-1]:
            next_directory = os.open(piece, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW,
                                     dir_fd=directory)
            os.close(directory)
            directory = next_directory
        descriptor = os.open(pieces[-1], os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW,
                             dir_fd=directory)
    except OSError as error:
        raise SafeFailure("credential-unavailable") from error
    finally:
        os.close(directory)
    try:
        metadata = os.fstat(descriptor)
        if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != expected_uid
                or metadata.st_mode & 0o077 or metadata.st_size > MAX_CREDENTIAL_BYTES):
            raise SafeFailure("credential-unavailable")
        data = bytearray()
        while True:
            chunk = os.read(descriptor, MAX_CREDENTIAL_BYTES + 1 - len(data))
            if not chunk:
                break
            data.extend(chunk)
            if len(data) > MAX_CREDENTIAL_BYTES:
                raise SafeFailure("credential-oversized")
    except OSError as error:
        raise SafeFailure("credential-unavailable") from error
    finally:
        os.close(descriptor)
    raw = bytes(data).strip()
    if re.fullmatch(rb"[0-9A-Fa-f]{64}", raw) is None:
        raise SafeFailure("credential-malformed")
    return bytearray.fromhex(raw.decode("ascii"))


def request_digest(request):
    unsigned = dict(request)
    unsigned.pop("armed_token", None)
    return hashlib.sha256(canonical_json(unsigned).encode("utf-8")).hexdigest()


def validate_action(action):
    if action not in SUPPORTED_ACTIONS:
        raise SafeFailure("invalid-action")


def requires_existing_generation(action):
    return action in EXISTING_WORKLOAD_ACTIONS


def build_request(key, node, image_ref, generation, nonce, now, action):
    validate_action(action)
    if action in IMAGE_REQUIRED_ACTIONS:
        validate_image_ref(image_ref)
    elif image_ref:
        validate_image_ref(image_ref)
    request = {
        "schema_version": SCHEMA_VERSION,
        "request_id": f"browser-vm-op-{nonce}",
        "workload_id": workload_id(node),
        "backend": BACKEND,
        "resources": {"vcpu": BROWSER_VCPU, "memory_mb": BROWSER_MEMORY_MB,
                      "disk_gb": BROWSER_DISK_GB},
        "image_ref": image_ref or None,
        "target_node": node,
        "expected_generation": generation,
        "action": action,
        "deadline_at_ms": now + (RDP_OPERATION_TTL_MS if action == "start_and_attach" else 20_000),
        "preferred_attachment": ATTACHMENT if action == "start_and_attach" else None,
    }
    digest = request_digest(request)
    expires = now + TOKEN_TTL_MS
    target = f"workload:{request['workload_id']}"
    payload = f"v2|{nonce}|{expires}|{ACTION_VERB}|{node}|{target}|{digest}"
    signature = hmac.new(key, payload.encode("utf-8"), hashlib.sha256).hexdigest()
    request["armed_token"] = f"{payload}|{signature}"
    return request


def check_bus_paths(bus, root):
    if not os.path.isabs(bus) or not os.access(bus, os.X_OK):
        raise SafeFailure("bus-unavailable")
    try:
        metadata = os.lstat(root)
        index = os.lstat(os.path.join(root, "index.sqlite"))
    except OSError as error:
        raise SafeFailure("bus-unavailable") from error
    if not stat.S_ISDIR(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode):
        raise SafeFailure("bus-unavailable")
    if not stat.S_ISREG(index.st_mode) or stat.S_ISLNK(index.st_mode):
        raise SafeFailure("bus-unavailable")


def publish(bus, root, body, runner=subprocess.run):
    try:
        completed = runner([bus, "publish", ACTION_TOPIC, "--bus-root", root],
                           input=body.encode("utf-8"), stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, timeout=BUS_COMMAND_TIMEOUT_SECONDS,
                           check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise SafeFailure("bus-publish-failed") from error
    if completed.returncode != 0:
        raise SafeFailure("bus-publish-failed")
    try:
        receipt = completed.stdout.decode("ascii").strip()
    except UnicodeDecodeError as error:
        raise SafeFailure("bus-receipt-invalid") from error
    if not ULID_RE.fullmatch(receipt):
        raise SafeFailure("bus-receipt-invalid")
    return receipt


def read_latest(bus, root, topic, timeout=BUS_COMMAND_TIMEOUT_SECONDS):
    try:
        completed = subprocess.run([bus, "history", topic, "--count", "1", "--json",
                                    "--bus-root", root], stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                   timeout=timeout, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise SafeFailure("history-read-failed") from error
    if completed.returncode != 0:
        raise SafeFailure("history-read-failed")
    if len(completed.stdout) > MAX_BYTES:
        raise SafeFailure("history-oversized")
    return completed.stdout


def decode_latest(raw, topic):
    if not raw.strip():
        return None
    try:
        lines = [line for line in raw.splitlines() if line.strip()]
        if len(lines) != 1:
            return None
        envelope = json.loads(lines[0])
        if not isinstance(envelope, dict) or envelope.get("topic") != topic:
            return None
        if not ULID_RE.fullmatch(envelope.get("ulid", "")):
            return None
        body = envelope.get("body")
        if not isinstance(body, str) or len(body.encode()) > MAX_BYTES:
            return None
        value = json.loads(body)
        return value if isinstance(value, dict) else None
    except (UnicodeDecodeError, ValueError, json.JSONDecodeError):
        return None


def generation_from_snapshot(value, node):
    if value is None:
        return 0
    if value.get("schema_version") != SCHEMA_VERSION or value.get("node") != node:
        raise SafeFailure("workload-state-malformed")
    workloads = value.get("workloads")
    if not isinstance(workloads, list):
        raise SafeFailure("workload-state-malformed")
    matches = [status for status in workloads if isinstance(status, dict)
               and status.get("workload_id") == workload_id(node)]
    if len(matches) > 1:
        raise SafeFailure("workload-state-malformed")
    if not matches:
        return 0
    generation = matches[0].get("generation")
    if isinstance(generation, bool) or not isinstance(generation, int) or generation < 1:
        raise SafeFailure("workload-state-malformed")
    return generation


def classify_reply(value, request_id):
    if value is None:
        return "pending"
    if value.get("schema_version") != SCHEMA_VERSION or value.get("request_id") != request_id:
        return "malformed"
    accepted = value.get("accepted")
    if accepted is True:
        return "accepted" if isinstance(value.get("status"), dict) else "malformed"
    if accepted is False and isinstance(value.get("error_code"), str):
        return "rejected"
    return "malformed"


def status_from_snapshot(value, node, request_id, action):
    if value is None or value.get("schema_version") != SCHEMA_VERSION or value.get("node") != node:
        return "pending", None
    workloads = value.get("workloads")
    if not isinstance(workloads, list):
        return "malformed", None
    matches = [status for status in workloads if isinstance(status, dict)
               and status.get("workload_id") == workload_id(node)]
    if len(matches) != 1:
        return ("pending", None) if not matches else ("malformed", None)
    status = matches[0]
    if status.get("request_id") != request_id:
        return "pending", None
    if status.get("phase") == "failed":
        return "failed", None
    if action in {"stop", "destroy"}:
        if status.get("phase") == "completed" and status.get("power") == "stopped":
            return "complete", None
        return "pending", None
    if action in {"start", "restart", "resume"}:
        if (status.get("phase") == "completed"
                and status.get("power") == "running"
                and status.get("readiness") == "ready"):
            return "complete", None
        return "pending", None
    attachment = status.get("attachment")
    if (status.get("phase") == "completed" and status.get("power") == "running"
            and status.get("readiness") == "ready"
            and isinstance(attachment, dict) and attachment.get("protocol") == ATTACHMENT
            and attachment.get("workload_id") == workload_id(node)):
        return "ready", attachment
    return "pending", None


def wait_for(bus, root, node, request_receipt, request_id, action):
    reply_topic = f"reply/{request_receipt}"
    state_topic = f"{STATE_PREFIX}{node}"
    deadline = time.monotonic() + OPERATION_TIMEOUT_SECONDS
    saw_reply = False
    while time.monotonic() < deadline:
        reply = decode_latest(read_latest(bus, root, reply_topic), reply_topic)
        reply_state = classify_reply(reply, request_id)
        if reply_state == "rejected":
            raise SafeFailure("operation-rejected")
        if reply_state == "malformed":
            raise SafeFailure("operation-reply-malformed")
        saw_reply = saw_reply or reply_state == "accepted"
        snapshot = decode_latest(read_latest(bus, root, state_topic), state_topic)
        status, attachment = status_from_snapshot(snapshot, node, request_id, action)
        if status == "ready":
            return attachment
        if status == "complete":
            return None
        if status == "failed":
            raise SafeFailure("operation-failed")
        if status == "malformed":
            raise SafeFailure("workload-state-malformed")
        time.sleep(POLL_SECONDS)
    raise SafeFailure("browser-rdp-ready-timeout" if saw_reply else "operation-reply-timeout")


def live(credential_path, node, action, image_ref, bus, root):
    validate_node(node)
    validate_action(action)
    if action in IMAGE_REQUIRED_ACTIONS:
        validate_image_ref(image_ref)
    elif image_ref:
        validate_image_ref(image_ref)
    check_bus_paths(bus, root)
    state_topic = f"{STATE_PREFIX}{node}"
    generation = generation_from_snapshot(decode_latest(read_latest(bus, root, state_topic), state_topic), node)
    if requires_existing_generation(action) and generation == 0:
        raise SafeFailure("workload-not-admitted")
    key = read_arm_key(credential_path, os.geteuid())
    try:
        nonce = secrets.token_hex(16)
        request = build_request(key, node, image_ref, generation, nonce, current_ms(), action)
        body = canonical_json(request)
    finally:
        for index in range(len(key)):
            key[index] = 0
    receipt = publish(bus, root, body)
    attachment = wait_for(bus, root, node, receipt, request["request_id"], action)
    result = {
        "schema_version": SCHEMA_VERSION,
        "kind": "browser_vm_workload_operation_receipt",
        "status": "ready" if action == "start_and_attach" else "completed",
        "action": action,
        "workload_id": workload_id(node),
        "node": node,
        "request_receipt": receipt,
        "request_id": request["request_id"],
        "generation": generation,
    }
    if attachment is not None:
        result["attachment_protocol"] = attachment["protocol"]
    print(canonical_json(result))


def self_test():
    assert ACTION_TOPIC == "action/workload/operation"
    assert DEFAULT_ACTION == "start_and_attach"
    assert SUPPORTED_ACTIONS == frozenset({
        "start_and_attach", "start", "stop", "restart", "resume", "destroy",
    })
    assert EXISTING_WORKLOAD_ACTIONS == frozenset({"stop", "restart", "resume", "destroy"})
    assert requires_existing_generation("restart")
    assert not requires_existing_generation("start_and_attach")
    assert ATTACHMENT == "rdp"
    node = "seat15"
    image_ref = "browser-vm-chromium:20260806"
    validate_node(node)
    validate_image_ref(image_ref)
    for bad in ("", "..", "bad node", "-option"):
        try:
            validate_node(bad)
        except SafeFailure:
            pass
        else:
            raise AssertionError("unsafe node accepted")
    for bad in ("browser-vm-chromium", "other:1", "browser-vm-chromium:bad/path"):
        try:
            validate_image_ref(bad)
        except SafeFailure:
            pass
        else:
            raise AssertionError("unsafe image reference accepted")
    key = bytearray.fromhex("00" * 32)
    request = build_request(key, node, image_ref, 7, "00112233445566778899aabbccddeeff", 1_893_456_000_000, DEFAULT_ACTION)
    assert request["workload_id"] == "browser-vm"
    assert request["expected_generation"] == 7
    assert request["action"] == DEFAULT_ACTION
    assert request["preferred_attachment"] == ATTACHMENT
    assert request["deadline_at_ms"] == 1_893_456_000_000 + RDP_OPERATION_TTL_MS
    assert request["backend"] == BACKEND
    assert request["resources"] == {"vcpu": 2, "memory_mb": 4096, "disk_gb": 32}
    assert request["armed_token"].split("|")[3:6] == [ACTION_VERB, node, "workload:browser-vm"]
    assert "armed_token" not in canonical_json({key: value for key, value in request.items() if key != "armed_token"})
    assert generation_from_snapshot(None, node) == 0
    snapshot = {"schema_version": 1, "node": node, "observed_at_ms": 1,
                "workloads": [{"workload_id": workload_id(node), "generation": 7}]}
    assert generation_from_snapshot(snapshot, node) == 7
    request_id = request["request_id"]
    reply = {"schema_version": 1, "request_id": request_id, "accepted": True, "status": {}}
    assert classify_reply(reply, request_id) == "accepted"
    rejected = {"schema_version": 1, "request_id": request_id, "accepted": False, "status": None, "error_code": "unauthorized"}
    assert classify_reply(rejected, request_id) == "rejected"
    ready = {"schema_version": 1, "node": node, "observed_at_ms": 2, "workloads": [{
        "workload_id": workload_id(node), "request_id": request_id, "phase": "completed",
        "power": "running", "readiness": "ready", "attachment": {"protocol": ATTACHMENT,
        "workload_id": workload_id(node)}}]}
    assert status_from_snapshot(ready, node, request_id, DEFAULT_ACTION)[0] == "ready"
    failed = json.loads(canonical_json(ready))
    failed["workloads"][0]["phase"] = "failed"
    assert status_from_snapshot(failed, node, request_id, DEFAULT_ACTION)[0] == "failed"
    for action, power, readiness in (
        ("stop", "stopped", "unavailable"),
        ("destroy", "stopped", "unavailable"),
        ("restart", "running", "ready"),
        ("resume", "running", "ready"),
    ):
        completed = {
            "schema_version": 1, "node": node, "observed_at_ms": 2,
            "workloads": [{
                "workload_id": workload_id(node), "request_id": request_id,
                "phase": "completed", "power": power, "readiness": readiness,
            }],
        }
        assert status_from_snapshot(completed, node, request_id, action)[0] == "complete"
    stop_request = build_request(key, node, "", 7, "00112233445566778899aabbccddef00", 1_893_456_000_000, "stop")
    assert stop_request.get("image_ref") is None
    assert stop_request["preferred_attachment"] is None
    calls = []
    def fake_runner(argv, **kwargs):
        calls.append((argv, kwargs))
        return SimpleNamespace(returncode=0, stdout=b"01ARZ3NDEKTSV4RRFFQ69G5FAV\n", stderr=b"")
    receipt = publish("/usr/bin/mde-bus", "/run/mde-bus", canonical_json(request), fake_runner)
    assert receipt == "01ARZ3NDEKTSV4RRFFQ69G5FAV"
    assert calls[0][0] == ["/usr/bin/mde-bus", "publish", ACTION_TOPIC, "--bus-root", "/run/mde-bus"]
    assert calls[0][1]["input"] == canonical_json(request).encode()
    assert request["armed_token"].encode() not in b" ".join(value.encode() for value in calls[0][0])
    with tempfile.TemporaryDirectory() as directory:
        path = os.path.join(directory, "key")
        with open(path, "wb") as handle:
            handle.write(b"00" * 32 + b"\n")
        os.chmod(path, 0o600)
        assert read_arm_key(path, os.geteuid()) == bytearray(32)
    assert resource.getrlimit(resource.RLIMIT_CORE) == (0, 0)


def main():
    if sys.argv[1:] == ["self-test"]:
        self_test()
        return
    if len(sys.argv) == 8 and sys.argv[1] == "live":
        live(*sys.argv[2:])
        return
    raise SafeFailure("invalid-invocation")


try:
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    main()
except SafeFailure as error:
    print(f"request-browser-vm-workload: status=failed reason={error.reason}", file=sys.stderr)
    raise SystemExit(1)
except KeyboardInterrupt:
    print("request-browser-vm-workload: status=interrupted mutation_status=unknown", file=sys.stderr)
    raise SystemExit(130)
except Exception:
    print("request-browser-vm-workload: status=failed reason=internal-error", file=sys.stderr)
    raise SystemExit(1)
PY
}

if [[ "${1:-}" == --self-test ]]; then
  (($# == 1)) || { usage; exit 2; }
  [[ -x "$PYTHON_BIN" ]] || fail python-unavailable
  run_python self-test >/dev/null || fail self-test-failed
  printf '%s: self-test passed\n' "$PROGRAM_NAME"
  exit 0
fi

node=
action=$DEFAULT_ACTION
action_seen=0
image_ref=
credential_path="${CREDENTIALS_DIRECTORY:+${CREDENTIALS_DIRECTORY}/cloud-arm-key}"
while (($#)); do
  case "$1" in
    --node) (($# >= 2)) || { usage; exit 2; }; [[ -z "$node" ]] || fail duplicate-node; node=$2; shift 2 ;;
    --action) (($# >= 2)) || { usage; exit 2; }; ((action_seen == 0)) || fail duplicate-action; action=$2; action_seen=1; shift 2 ;;
    --image-ref) (($# >= 2)) || { usage; exit 2; }; [[ -z "$image_ref" ]] || fail duplicate-image-ref; image_ref=$2; shift 2 ;;
    --credential-path) (($# >= 2)) || { usage; exit 2; }; credential_path=$2; shift 2 ;;
    *) usage; exit 2 ;;
  esac
done
[[ -n "$node" ]] || { usage; exit 2; }
case "$action" in
  start_and_attach|start|stop|restart|resume|destroy) ;;
  *) fail invalid-action ;;
esac
if [[ "$action" == start_and_attach || "$action" == start ]]; then
  [[ -n "$image_ref" ]] || { usage; exit 2; }
fi
[[ "$(id -u)" == 0 ]] || fail root-required
[[ -x "$PYTHON_BIN" && -x "$MDE_BUS_BIN" ]] || fail bus-unavailable
[[ -n "$credential_path" ]] || fail credential-directory-required
run_python live "$credential_path" "$node" "$action" "$image_ref" "$MDE_BUS_BIN" "$BUS_ROOT"
