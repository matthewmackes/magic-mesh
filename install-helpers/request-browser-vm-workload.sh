#!/usr/bin/env bash
# Admit the stable Browser VM into the typed Workloads desired-state plane.
# The armed capability is created in memory and delivered only over mde-bus
# stdin; neither the key, token, request body, nor raw reply is printed.
set -euo pipefail
umask 077

readonly PROGRAM_NAME="request-browser-vm-workload"
readonly PYTHON_BIN="/usr/bin/python3"
readonly MDE_BUS_BIN="/usr/bin/mde-bus"
readonly BUS_ROOT="/run/mde-bus"

usage() {
  cat >&2 <<EOF
usage: $0 --node NODE --name browser-vm --image-digest sha256:HEX [--credential-path PATH]
       $0 --self-test

Run the live request from a systemd unit carrying LoadCredentialEncrypted=cloud-arm-key,
or pass the absolute path of an already-loaded plaintext systemd credential.
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
SCHEMA_VERSION = 1
WAIT_SECONDS = 5
TOKEN_TTL_MS = 25_000
REPLY_TIMEOUT_SECONDS = 20.0
BUS_COMMAND_TIMEOUT_SECONDS = 8.0
REPLY_POLL_SECONDS = 0.25
MAX_CREDENTIAL_BYTES = 65
MAX_REPLY_BYTES = 256 * 1024
ULID_RE = re.compile(r"[0-9A-HJKMNP-TV-Z]{26}\Z")
SEGMENT_RE = re.compile(r"[A-Za-z0-9._-]{1,255}\Z")
DIGEST_RE = re.compile(r"sha256:[0-9A-Fa-f]{64}\Z")


class SafeFailure(Exception):
    def __init__(self, reason):
        super().__init__(reason)
        self.reason = reason


def validate_inputs(node, name, image_digest):
    if not SEGMENT_RE.fullmatch(node) or node in {".", ".."}:
        raise SafeFailure("invalid-node")
    if name != STABLE_NAME:
        raise SafeFailure("invalid-name")
    if not DIGEST_RE.fullmatch(image_digest):
        raise SafeFailure("invalid-image-digest")


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


def classify_reply(raw, expected_topic, node, name, image_digest):
    if not raw.strip():
        return "pending"
    if len(raw) > MAX_REPLY_BYTES:
        return "malformed"
    lines = [line for line in raw.splitlines() if line.strip()]
    if len(lines) != 1:
        return "malformed"
    try:
        envelope = json.loads(lines[0])
        if not isinstance(envelope, dict) or envelope.get("topic") != expected_topic:
            return "malformed"
        body_raw = envelope.get("body")
        if not isinstance(body_raw, str) or len(body_raw) > MAX_REPLY_BYTES:
            return "malformed"
        body = json.loads(body_raw)
    except (TypeError, ValueError):
        return "malformed"
    if not isinstance(body, dict) or body.get("verb") != ACTION_VERB:
        return "malformed"
    if body.get("ok") is True:
        desired = body.get("desired")
        if not isinstance(desired, list):
            return "malformed"
        admitted = any(
            isinstance(row, dict)
            and row.get("node") == node
            and row.get("name") == name
            and row.get("image_digest") == image_digest
            for row in desired
        )
        return "accepted" if admitted else "malformed"
    if body.get("ok") is not False:
        return "malformed"
    if isinstance(body.get("gated"), str) and body["gated"]:
        return "gated"
    if isinstance(body.get("error"), str) and body["error"]:
        return "rejected"
    return "failed"


def limit_reply_output():
    resource.setrlimit(
        resource.RLIMIT_FSIZE,
        (MAX_REPLY_BYTES + 1, MAX_REPLY_BYTES + 1),
    )


def wait_for_reply(bus_bin, bus_root, receipt, node, name, image_digest):
    topic = f"reply/{receipt}"
    deadline = time.monotonic() + REPLY_TIMEOUT_SECONDS
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise SafeFailure("reply-timeout")
        try:
            with tempfile.TemporaryFile() as bounded_stdout:
                completed = subprocess.run(
                    [
                        bus_bin,
                        "history",
                        topic,
                        "--count",
                        "1",
                        "--json",
                        "--bus-root",
                        bus_root,
                    ],
                    stdout=bounded_stdout,
                    stderr=subprocess.DEVNULL,
                    timeout=min(2.0, remaining),
                    check=False,
                    preexec_fn=limit_reply_output,
                )
                bounded_stdout.seek(0)
                raw_reply = bounded_stdout.read(MAX_REPLY_BYTES + 1)
        except subprocess.TimeoutExpired:
            continue
        except (OSError, subprocess.SubprocessError) as exc:
            raise SafeFailure("reply-read-failed") from exc
        if len(raw_reply) > MAX_REPLY_BYTES:
            raise SafeFailure("reply-oversized")
        if completed.returncode != 0:
            raise SafeFailure("reply-read-failed")
        status = classify_reply(
            raw_reply, topic, node, name, image_digest
        )
        if status == "accepted":
            return
        if status != "pending":
            raise SafeFailure(f"reply-{status}")
        time.sleep(min(REPLY_POLL_SECONDS, max(0.0, deadline - time.monotonic())))


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


def live_request(credential_path, node, name, image_digest, bus_bin, bus_root):
    validate_inputs(node, name, image_digest)
    verify_runtime_paths(bus_bin, bus_root)
    key = read_cloud_arm_key(credential_path, 0)
    try:
        alert_body = canonical_json(ALERT_BODY)
        publish_body(bus_bin, bus_root, ALERT_TOPIC, alert_body)
        print(
            f"request-browser-vm-workload: alert=published wait_seconds={WAIT_SECONDS}",
            flush=True,
        )
        time.sleep(WAIT_SECONDS)

        nonce = secrets.token_hex(16)
        expires_at_ms = time.time_ns() // 1_000_000 + TOKEN_TTL_MS
        request_body = build_armed_request(
            key, node, name, image_digest, nonce, expires_at_ms
        )
    finally:
        for index in range(len(key)):
            key[index] = 0

    receipt = publish_body(bus_bin, bus_root, ACTION_TOPIC, request_body)
    request_body = None
    wait_for_reply(bus_bin, bus_root, receipt, node, name, image_digest)
    print("request-browser-vm-workload: request=accepted reply=verified")


def expect_safe_failure(call):
    try:
        call()
    except SafeFailure:
        return
    raise AssertionError("unsafe input was accepted")


def self_test():
    assert ACTION_TOPIC == "action/cloud/browser-provision"
    assert ALERT_TOPIC == "event/toast/show"
    assert ALERT_BODY["flag"] == "AI-GENERATED-ALERT"
    assert ALERT_BODY["headline"] == "This seat will update in 5 seconds."
    assert WAIT_SECONDS == 5
    assert 0 < TOKEN_TTL_MS <= 30_000

    node = "DELL-LAPTOP"
    name = STABLE_NAME
    image_digest = "sha256:" + "0123456789abcdef" * 4
    validate_inputs(node, name, image_digest)
    for bad_node in ("", ".", "..", "Dell/other", "Dell seat", "x" * 256):
        expect_safe_failure(lambda bad_node=bad_node: validate_inputs(bad_node, name, image_digest))
    expect_safe_failure(lambda: validate_inputs(node, "browser-dell", image_digest))
    expect_safe_failure(lambda: validate_inputs(node, name, "sha256:abcd"))

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
    reply_body = canonical_json(
        {
            "ok": True,
            "verb": ACTION_VERB,
            "desired": [
                {"node": node, "name": name, "image_digest": image_digest}
            ],
        }
    )
    reply_envelope = canonical_json({"topic": topic, "body": reply_body}).encode()
    assert classify_reply(reply_envelope, topic, node, name, image_digest) == "accepted"
    assert classify_reply(b"", topic, node, name, image_digest) == "pending"
    assert classify_reply(b"not-json", topic, node, name, image_digest) == "malformed"
    assert classify_reply(b"x" * (MAX_REPLY_BYTES + 1), topic, node, name, image_digest) == "malformed"


def main():
    if len(sys.argv) == 2 and sys.argv[1] == "self-test":
        self_test()
        return
    if len(sys.argv) == 8 and sys.argv[1] == "live":
        live_request(*sys.argv[2:])
        return
    raise SafeFailure("invalid-invocation")


try:
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
name=""
image_digest=""
credential_path="${CREDENTIALS_DIRECTORY:+${CREDENTIALS_DIRECTORY}/cloud-arm-key}"
node_seen=0
name_seen=0
digest_seen=0
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

((node_seen == 1 && name_seen == 1 && digest_seen == 1)) || {
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
  "$name" \
  "$image_digest" \
  "$MDE_BUS_BIN" \
  "$BUS_ROOT"
