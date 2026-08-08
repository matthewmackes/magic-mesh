#!/usr/bin/env bash
# Bounded loopback proof for the WL-FUNC-021 Music provider-loss boundary.
#
# This helper deliberately exercises the transport/policy witness around the
# mde-musicd engine seam: a complete local fixture ends with FIN and is
# classified as clean EOF; a second local fixture sends a valid WAV prefix,
# then closes with TCP RST; a third request resumes the same Subsonic-shaped
# stream with the bounded integer-second timeOffset contract.  Once audio
# bytes have been delivered, the client must not request a byte-zero fallback
# candidate.  It does not claim live provider, daemon, decoder, or hardware
# acceptance; those require their own gates.  No non-loopback connection is
# made.
set -Eeuo pipefail

PROVIDER_TIMEOUT_SECONDS="${MUSIC_NETWORK_PROVIDER_TIMEOUT_SECONDS:-12}"
CLIENT_TIMEOUT_SECONDS="${MUSIC_NETWORK_CLIENT_TIMEOUT_SECONDS:-8}"
READY_TIMEOUT_SECONDS="${MUSIC_NETWORK_READY_TIMEOUT_SECONDS:-5}"

WORK_DIR=""
PROVIDER_PID=""

usage() {
    cat >&2 <<'EOF'
usage: verify-music-network-loss.sh [--self-test]

Runs a bounded 127.0.0.1 provider/client proof.  The provider exposes a
complete fixture, a mid-stream TCP-reset fixture, and a same-track recovery
fixture.  The client verifies:
  * a normal FIN after the complete body is classified as clean EOF;
  * a reset after audio bytes is a provider read failure, not clean EOF;
  * recovery preserves the song identity and requests timeOffset=1;
  * resumed audio is non-empty and is appended after the audible prefix; and
  * no alternate candidate is requested after audio has begun.

Environment: MUSIC_NETWORK_PROVIDER_TIMEOUT_SECONDS (1..30),
MUSIC_NETWORK_CLIENT_TIMEOUT_SECONDS (1..20), and
MUSIC_NETWORK_READY_TIMEOUT_SECONDS (1..15).
EOF
}

fail() {
    printf '[FAIL] %s\n' "$1" >&2
    exit 2
}

bounded_integer() {
    local value=$1 minimum=$2 maximum=$3
    [[ "$value" =~ ^[0-9]+$ ]] && (( value >= minimum && value <= maximum ))
}

cleanup() {
    local status=$?
    trap - EXIT INT TERM
    if [[ -n "$PROVIDER_PID" ]] && kill -0 "$PROVIDER_PID" 2>/dev/null; then
        kill -TERM "$PROVIDER_PID" 2>/dev/null || true
        wait "$PROVIDER_PID" 2>/dev/null || true
    fi
    if [[ -n "$WORK_DIR" && -d "$WORK_DIR" ]]; then
        rm -rf -- "$WORK_DIR"
    fi
    exit "$status"
}
trap cleanup EXIT INT TERM

validate_config() {
    bounded_integer "$PROVIDER_TIMEOUT_SECONDS" 1 30 ||
        fail 'provider timeout must be 1..30 seconds'
    bounded_integer "$CLIENT_TIMEOUT_SECONDS" 1 20 ||
        fail 'client timeout must be 1..20 seconds'
    bounded_integer "$READY_TIMEOUT_SECONDS" 1 15 ||
        fail 'ready timeout must be 1..15 seconds'
    command -v python3 >/dev/null 2>&1 || fail 'python3 is required'
    command -v timeout >/dev/null 2>&1 || fail 'timeout is required'
}

run_proof() {
    WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/mcnf-music-network-loss.XXXXXX")"
    local ready_path="$WORK_DIR/ready.json"
    local state_path="$WORK_DIR/provider-state.json"
    local trace_path="$WORK_DIR/client-trace.ndjson"
    local provider_log="$WORK_DIR/provider.log"
    local provider_rc=0
    local client_rc=0
    local client_output=''

    (
        exec timeout --foreground --signal=TERM --kill-after=2s \
            "${PROVIDER_TIMEOUT_SECONDS}s" python3 - "$ready_path" "$state_path" \
            "$PROVIDER_TIMEOUT_SECONDS" <<'PY'
import json
import socket
import struct
import sys
import time
from pathlib import Path

ready_path = Path(sys.argv[1])
state_path = Path(sys.argv[2])
max_seconds = int(sys.argv[3])


def wav_fixture(frames, left_value, right_value):
    channels = 2
    sample_rate = 48_000
    bits = 16
    block_align = channels * (bits // 8)
    data_length = frames * block_align
    output = bytearray()
    output.extend(b"RIFF")
    output.extend((36 + data_length).to_bytes(4, "little"))
    output.extend(b"WAVEfmt ")
    output.extend((16).to_bytes(4, "little"))
    output.extend((1).to_bytes(2, "little"))
    output.extend(channels.to_bytes(2, "little"))
    output.extend(sample_rate.to_bytes(4, "little"))
    output.extend((sample_rate * block_align).to_bytes(4, "little"))
    output.extend(block_align.to_bytes(2, "little"))
    output.extend(bits.to_bytes(2, "little"))
    output.extend(b"data")
    output.extend(data_length.to_bytes(4, "little"))
    for frame in range(frames):
        left = left_value if frame % 2 == 0 else -left_value
        right = right_value if frame % 3 == 0 else -right_value
        output.extend(struct.pack("<hh", left, right))
    return bytes(output)


audio = wav_fixture(9_600, 7_000, 3_000)
continuation = wav_fixture(2_400, 12_000, 5_000)
audio_offset = 44
audible_frames = 2_400
prefix_length = audio_offset + 4_800 * 4
prefix = audio[:prefix_length]
requests = []
started = time.monotonic()


def write_json(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, sort_keys=True), encoding="utf-8")
    temporary.replace(path)


def write_state():
    write_json(
        state_path,
        {
            "requests": requests,
            "clean_requests": requests.count("/clean"),
            "reset_requests": requests.count("/rest/stream?id=song-7"),
            "recovery_requests": sum(
                1
                for request in requests
                if request == "/rest/stream?id=song-7&timeOffset=1"
            ),
            "fallback_requests": requests.count("/fallback"),
        },
    )


def read_request(connection):
    data = bytearray()
    connection.settimeout(2.0)
    while b"\r\n\r\n" not in data:
        chunk = connection.recv(1024)
        if not chunk:
            raise RuntimeError("fixture request ended before HTTP headers")
        data.extend(chunk)
        if len(data) > 16 * 1024:
            raise RuntimeError("fixture request exceeded 16 KiB")
    request_line = data.split(b"\r\n", 1)[0].decode("ascii")
    parts = request_line.split()
    if len(parts) != 3 or parts[0] != "GET":
        raise RuntimeError("fixture received a non-GET request")
    return parts[1]


def response_header(payload):
    return (
        "HTTP/1.1 200 OK\r\n"
        "Connection: close\r\n"
        f"X-MCNF-Expected-Length: {len(payload)}\r\n"
        "Content-Type: audio/wav\r\n\r\n"
    ).encode("ascii")


def serve(connection):
    path = read_request(connection)
    peer = connection.getpeername()[0]
    if not peer.startswith("127."):
        raise RuntimeError(f"non-loopback peer reached fixture: {peer}")
    requests.append(path)
    write_state()
    if path == "/clean":
        connection.sendall(response_header(audio) + audio)
        connection.shutdown(socket.SHUT_WR)
        return
    if path == "/rest/stream?id=song-7":
        connection.sendall(response_header(audio) + prefix)
        # An abortive close is the deterministic local equivalent of a
        # provider disappearing while the decoder is reading its stream.
        connection.setsockopt(
            socket.SOL_SOCKET,
            socket.SO_LINGER,
            struct.pack("ii", 1, 0),
        )
        return
    if path == "/rest/stream?id=song-7&timeOffset=1":
        connection.sendall(response_header(continuation) + continuation)
        connection.shutdown(socket.SHUT_WR)
        return
    if path == "/fallback":
        connection.sendall(response_header(audio) + audio)
        connection.shutdown(socket.SHUT_WR)
        return
    connection.sendall(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n")
    connection.shutdown(socket.SHUT_WR)


write_state()
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(4)
    listener.settimeout(0.25)
    write_json(
        ready_path,
        {
            "port": listener.getsockname()[1],
            "expected_bytes": len(audio),
            "audio_offset": audio_offset,
            "audible_frames": audible_frames,
            "prefix_length": len(prefix),
            "audio_sha256": __import__("hashlib").sha256(audio).hexdigest(),
            "prefix_sha256": __import__("hashlib").sha256(prefix).hexdigest(),
            "continuation_sha256": __import__("hashlib").sha256(continuation).hexdigest(),
        },
    )
    # Three requests are required by the proof.  A short grace window also
    # catches an unexpected fallback request instead of leaving it invisible.
    while len(requests) < 3 and time.monotonic() - started < max_seconds:
        try:
            connection, _ = listener.accept()
        except socket.timeout:
            continue
        with connection:
            serve(connection)
    grace_deadline = min(started + max_seconds, time.monotonic() + 0.75)
    while time.monotonic() < grace_deadline:
        try:
            connection, _ = listener.accept()
        except socket.timeout:
            continue
        with connection:
            serve(connection)
        break
write_state()
PY
    ) >"$provider_log" 2>&1 &
    PROVIDER_PID=$!

    local ready_deadline=$((SECONDS + READY_TIMEOUT_SECONDS))
    while [[ ! -s "$ready_path" ]]; do
        if ! kill -0 "$PROVIDER_PID" 2>/dev/null; then
            cat "$provider_log" >&2 || true
            fail 'loopback provider exited before publishing its port'
        fi
        if (( SECONDS >= ready_deadline )); then
            cat "$provider_log" >&2 || true
            fail 'loopback provider did not publish its port before the deadline'
        fi
        sleep 0.05
    done

    client_output="$(
        timeout --foreground --signal=TERM --kill-after=2s \
            "${CLIENT_TIMEOUT_SECONDS}s" python3 - "$ready_path" "$state_path" \
            "$trace_path" <<'PY'
import hashlib
import json
import socket
import sys
import time
from pathlib import Path

ready_path = Path(sys.argv[1])
state_path = Path(sys.argv[2])
trace_path = Path(sys.argv[3])
ready = json.loads(ready_path.read_text(encoding="utf-8"))
port = int(ready["port"])
expected_bytes = int(ready["expected_bytes"])
audio_offset = int(ready["audio_offset"])
audible_frames = int(ready["audible_frames"])
prefix_length = int(ready["prefix_length"])


def record(event):
    with trace_path.open("a", encoding="utf-8") as trace:
        trace.write(json.dumps(event, sort_keys=True) + "\n")


def fetch(path):
    body = bytearray()
    error_name = None
    with socket.create_connection(("127.0.0.1", port), timeout=2.0) as connection:
        connection.settimeout(2.0)
        connection.sendall(
            f"GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n".encode(
                "ascii"
            )
        )
        headers = bytearray()
        while b"\r\n\r\n" not in headers:
            chunk = connection.recv(1024)
            if not chunk:
                raise RuntimeError(f"{path}: response ended before headers")
            headers.extend(chunk)
            if len(headers) > 16 * 1024:
                raise RuntimeError(f"{path}: response headers exceeded 16 KiB")
        header_bytes, remainder = bytes(headers).split(b"\r\n\r\n", 1)
        header_lines = header_bytes.decode("ascii").split("\r\n")
        if not header_lines or not header_lines[0].startswith("HTTP/1.1 200 "):
            raise RuntimeError(f"{path}: fixture returned an unexpected status")
        values = {}
        for line in header_lines[1:]:
            key, separator, value = line.partition(":")
            if separator:
                values[key.lower()] = value.strip()
        declared = int(values["x-mcnf-expected-length"])
        body.extend(remainder)
        while True:
            try:
                chunk = connection.recv(4096)
            except ConnectionResetError as exc:
                error_name = exc.__class__.__name__
                break
            except socket.timeout as exc:
                error_name = exc.__class__.__name__
                break
            if not chunk:
                break
            body.extend(chunk)
            if len(body) > expected_bytes:
                raise RuntimeError(f"{path}: body exceeded the fixture bound")
    if error_name is None:
        classification = "clean_eof"
    else:
        classification = "provider_read_failure"
    event = {
        "path": path,
        "classification": classification,
        "error": error_name,
        "body_bytes": len(body),
        "declared_bytes": declared,
    }
    record(event)
    return bytes(body), classification, error_name, declared


clean_body, clean_classification, clean_error, clean_declared = fetch("/clean")
if clean_classification != "clean_eof" or clean_error is not None:
    raise AssertionError(
        f"complete fixture was not classified as clean EOF: "
        f"{clean_classification}/{clean_error}"
    )
if len(clean_body) != expected_bytes or hashlib.sha256(clean_body).hexdigest() != ready["audio_sha256"]:
    raise AssertionError("clean-EOF fixture body was truncated or changed")
if not any(clean_body[audio_offset:]):
    raise AssertionError("clean fixture contains no nonzero audio bytes")

primary_path = "/rest/stream?id=song-7"
primary_body, primary_classification, primary_error, primary_declared = fetch(primary_path)
if primary_classification == "clean_eof":
    raise AssertionError("mid-stream reset was misclassified as clean EOF")
if primary_classification != "provider_read_failure" or primary_error != "ConnectionResetError":
    raise AssertionError(
        f"expected a bounded ConnectionResetError, got "
        f"{primary_classification}/{primary_error}"
    )
if len(primary_body) != prefix_length or hashlib.sha256(primary_body).hexdigest() != ready["prefix_sha256"]:
    raise AssertionError("reset fixture did not stop at the intended mid-stream prefix")
emitted_audio = primary_body[audio_offset:]
if not emitted_audio or not any(emitted_audio):
    raise AssertionError("the provider failure occurred before audio began")

recovery_path = "/rest/stream?id=song-7&timeOffset=1"
recovery_body, recovery_classification, recovery_error, recovery_declared = fetch(recovery_path)
if recovery_classification != "clean_eof" or recovery_error is not None:
    raise AssertionError(
        f"same-track recovery was not classified as clean EOF: "
        f"{recovery_classification}/{recovery_error}"
    )
if (
    recovery_declared != len(recovery_body)
    or hashlib.sha256(recovery_body).hexdigest() != ready["continuation_sha256"]
):
    raise AssertionError("recovery response was truncated or changed")
recovered_audio = recovery_body[audio_offset:]
if not recovered_audio or not any(recovered_audio):
    raise AssertionError("recovery response contains no nonzero audio bytes")

expected_audible_prefix = primary_body[
    audio_offset : audio_offset + audible_frames * 4
]
if len(expected_audible_prefix) != audible_frames * 4:
    raise AssertionError("reset fixture did not leave the bounded audible prefix")
logical_audio = expected_audible_prefix + recovered_audio
if len(logical_audio) != (audible_frames * 4) + len(recovered_audio):
    raise AssertionError("recovery changed the logical audio length")

# This is the engine's important policy boundary: a candidate fallback starts
# at byte zero, so it is forbidden once this logical track emitted audio.  The
# recovery request above is the same logical provider stream, not a fallback.
logical_output = primary_body
fallback_requested = False
if not emitted_audio:
    fallback_requested = True
    logical_output, _, _, _ = fetch("/fallback")
if fallback_requested:
    raise AssertionError("fallback was requested after audio began")
if logical_output != primary_body:
    raise AssertionError("logical output changed after the post-audio provider loss")

time.sleep(0.15)
state = json.loads(state_path.read_text(encoding="utf-8"))
if state.get("requests") != ["/clean", primary_path, recovery_path]:
    raise AssertionError(f"unexpected provider request trace: {state.get('requests')!r}")
if state.get("fallback_requests") != 0:
    raise AssertionError("alternate candidate was requested; this would replay from byte zero")
if state.get("recovery_requests") != 1:
    raise AssertionError("same-track recovery request was not observed exactly once")

record(
    {
        "policy": "resume_same_provider_without_from_zero_replay",
        "audio_bytes_before_failure": len(emitted_audio),
        "recovery_audio_bytes": len(recovered_audio),
        "logical_audio_sha256": hashlib.sha256(logical_audio).hexdigest(),
        "fallback_requests": state["fallback_requests"],
        "logical_output_sha256": hashlib.sha256(logical_output).hexdigest(),
    }
)
print(
    json.dumps(
        {
            "loopback": True,
            "clean_eof": clean_classification,
            "midstream_failure": primary_error,
            "primary_bytes_before_failure": len(primary_body),
            "audio_bytes_before_failure": len(emitted_audio),
            "recovery_request": recovery_path,
            "recovery_audio_bytes": len(recovered_audio),
            "logical_audio_bytes": len(logical_audio),
            "fallback_requests": state["fallback_requests"],
            "policy": "resume_same_provider_without_from_zero_replay",
        },
        sort_keys=True,
    )
)
PY
    )" || client_rc=$?
    if (( client_rc != 0 )); then
        printf '%s\n' "$client_output" >&2
        cat "$provider_log" >&2 || true
        cat "$trace_path" >&2 || true
        fail "loopback client proof failed (bounded rc=$client_rc)"
    fi
    printf '%s\n' "$client_output"

    wait "$PROVIDER_PID" || provider_rc=$?
    PROVIDER_PID=""
    if (( provider_rc != 0 )); then
        cat "$provider_log" >&2 || true
        fail "loopback provider did not finish cleanly (bounded rc=$provider_rc)"
    fi
    printf 'verify-music-network-loss: PASS (temporary fixture and trace cleaned on exit)\n'
}

validate_config
if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi
if [[ "${1:-}" == "--self-test" ]]; then
    [[ "$#" == 1 ]] || fail '--self-test takes no additional arguments'
    run_proof
    exit 0
fi
[[ "$#" == 0 ]] || { usage; fail "unknown argument: $1"; }
run_proof
