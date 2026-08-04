#!/usr/bin/env bash
# Collect sample-backed Browser VM audio evidence without crossing the guest
# user boundary through QGA. QGA is used only for ping and immutable provenance;
# a trusted Browser-owned RDP/WebAudio control hook supplies user-gesture
# playback and getUserMedia capture.
set -euo pipefail
set +x
umask 077
ulimit -S -c 0 2>/dev/null || true
ulimit -H -c 0 2>/dev/null || true

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
readonly VALIDATOR="$SCRIPT_DIR/verify-browser-vm-live-audio.py"
readonly LIBVIRT_URI="qemu:///system"
readonly PLAYBACK_STREAM_NAME="MCNF-Browser-VM"
readonly SAMPLE_RATE=48000
readonly SAMPLE_CHANNELS=2
readonly SAMPLE_FRAMES=96000
readonly STIMULUS_SECONDS=8
readonly OPERATION_TIMEOUT_SECONDS=90
readonly RECONNECT_TIMEOUT_SECONDS=240
readonly QGA_TIMEOUT_SECONDS=8
readonly MAX_QGA_FILE_BYTES=4096
readonly CONTROL_FILE_WAIT_ATTEMPTS=$((OPERATION_TIMEOUT_SECONDS * 10))

domain="browser-vm"
seat_user="mm"
output_dir=""
source_commit=""
image_digest=""
transport=""
guest_probe_hook=""
reconnect_hook=""
warning_helper=""
qemu_pid_file=""

test_mode=0
test_state=""
run_nonce=""
stage_dir=""
host_runtime=""
guest_hook_pid=""
host_async_pid=""
host_async_unit=""
host_async_log=""
injection_module=""
injection_sink=""
injection_monitor=""
capture_stream_id=""
capture_original_source=""
finalized=0
COLLECTED_AT=""
DISCONNECT_AT=""
RECONNECT_AT=""

VIRSH_BIN=""
PYTHON_BIN=""
BASE64_BIN=""
PACTL_BIN=""
PW_RECORD_BIN=""
PW_PLAY_BIN=""
SYSTEMD_RUN_BIN=""
SYSTEMCTL_BIN=""
TIMEOUT_BIN=""
SHA256SUM_BIN=""

usage() {
    cat >&2 <<'EOF'
usage: collect-browser-vm-live-audio.sh \
         --output DIR \
         --source-commit FULL_SHA \
         --image-digest sha256:HEX \
         --transport rdp|sunshine \
         --guest-probe-hook ABSOLUTE_PATH \
         --reconnect-hook ABSOLUTE_PATH \
         [--domain NAME] [--seat-user USER]

       collect-browser-vm-live-audio.sh --self-test

The guest probe hook is mandatory. It must drive a Browser-owned RDP/WebAudio
page with a real user gesture; QGA, guest SSH, runuser, setpriv, and guest
systemd-run are not accepted as substitutes. See this file's
guest_probe_receipt_valid() function for the private receipt contract.

The reconnect hook must write its private receipt to
$MCNF_BROWSER_VM_RECONNECT_RECEIPT. Neither hook receives credentials from this
collector, and hook output is never copied to the console.
EOF
}

log() {
    printf 'collect-browser-vm-live-audio: %s\n' "$*" >&2
}

die() {
    log "$*"
    exit 1
}

valid_domain() {
    [[ $1 =~ ^[A-Za-z0-9._-]{1,128}$ && $1 != "." && $1 != ".." ]]
}

valid_user() {
    [[ $1 =~ ^[a-z_][a-z0-9_-]{0,31}$ ]]
}

valid_commit() {
    [[ $1 =~ ^[0-9a-f]{40}$ && $1 != "0000000000000000000000000000000000000000" ]]
}

valid_digest() {
    [[ $1 =~ ^sha256:[0-9a-f]{64}$ && $1 != "sha256:0000000000000000000000000000000000000000000000000000000000000000" ]]
}

valid_transport() {
    [[ $1 == rdp || $1 == sunshine ]]
}

valid_timestamp() {
    [[ $1 =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$ ]]
}

utc_now() {
    date -u +%Y-%m-%dT%H:%M:%SZ
}

timestamp_after() {
    local previous=$1 current attempt
    for ((attempt = 0; attempt < 30; attempt += 1)); do
        current=$(utc_now)
        if [[ $current > $previous ]]; then
            printf '%s\n' "$current"
            return 0
        fi
        sleep 0.1
    done
    return 1
}

resolve_commands() {
    VIRSH_BIN=$(command -v virsh || true)
    PYTHON_BIN=$(command -v python3 || true)
    BASE64_BIN=$(command -v base64 || true)
    PACTL_BIN=$(command -v pactl || true)
    PW_RECORD_BIN=$(command -v pw-record || true)
    PW_PLAY_BIN=$(command -v pw-play || true)
    SYSTEMD_RUN_BIN=$(command -v systemd-run || true)
    SYSTEMCTL_BIN=$(command -v systemctl || true)
    TIMEOUT_BIN=$(command -v timeout || true)
    SHA256SUM_BIN=$(command -v sha256sum || true)
    local name value
    for name in VIRSH PYTHON BASE64 PACTL PW_RECORD PW_PLAY SYSTEMD_RUN SYSTEMCTL TIMEOUT SHA256SUM; do
        value=${name}_BIN
        [[ -n ${!value} ]] || die "required command is unavailable: ${name,,}"
    done
}

private_regular_file() {
    local path=$1 maximum=${2:-33554432}
    [[ -f $path && ! -L $path ]] || return 1
    local mode size
    mode=$(stat -Lc '%a' "$path") || return 1
    size=$(stat -Lc '%s' "$path") || return 1
    (( (8#$mode & 0077) == 0 && (8#$mode & 0111) == 0 )) || return 1
    (( size > 0 && size <= maximum ))
}

trusted_executable() {
    local path=$1 owner mode
    [[ $path == /* && -f $path && ! -L $path && -x $path ]] || return 1
    owner=$(stat -Lc '%u' "$path") || return 1
    mode=$(stat -Lc '%a' "$path") || return 1
    [[ $owner == "$EUID" ]] || return 1
    (( (8#$mode & 0022) == 0 ))
}

validate_warning_helper() {
    trusted_executable "$warning_helper" ||
        die "mandatory five-second warning helper is not a trusted executable: $warning_helper"
    grep -Fxq 'readonly WAIT_SECONDS=5' "$warning_helper" ||
        die "mandatory warning helper does not pin a five-second interval"
    grep -Fxq "sleep \"\$WAIT_SECONDS\"" "$warning_helper" ||
        die "mandatory warning helper does not enforce its interval"
    grep -Fq '"flag":"AI-GENERATED-ALERT"' "$warning_helper" ||
        die "mandatory warning helper lacks the AI-GENERATED-ALERT contract"
}

invoke_warning() {
    local error_file="$host_runtime/warning-error"
    if ((test_mode)); then
        "$warning_helper" >"$host_runtime/warning-output" 2>"$error_file" ||
            die "mandatory five-second warning failed; live mutation was not started"
    else
        /usr/bin/env -i PATH=/usr/bin:/usr/sbin \
            "$warning_helper" >"$host_runtime/warning-output" 2>"$error_file" ||
            die "mandatory five-second warning failed; live mutation was not started"
    fi
}

prepare_output() {
    [[ $output_dir == /* ]] || die "--output must be an absolute path"
    [[ ! -e $output_dir && ! -L $output_dir ]] || die "output path already exists"

    local parent base canonical owner mode
    parent=$(dirname "$output_dir")
    base=$(basename "$output_dir")
    [[ $base != "." && $base != ".." && $base != *$'\n'* ]] || die "invalid output basename"
    [[ -d $parent && ! -L $parent ]] || die "output parent is not a real directory"
    canonical=$(realpath -e "$parent") || die "cannot resolve output parent"
    [[ $canonical == "$parent" ]] || die "output parent must use its canonical non-symlink path"
    owner=$(stat -Lc '%u' "$parent")
    mode=$(stat -Lc '%a' "$parent")
    [[ $owner == "$EUID" ]] || die "output parent must be owned by the collector account"
    (( (8#$mode & 0022) == 0 )) || die "output parent must not be group/world writable"

    stage_dir=$(mktemp -d "$parent/.${base}.staging.XXXXXX")
    chmod 0700 "$stage_dir"
    mkdir -m 0700 "$stage_dir/samples" "$stage_dir/control"
}

prepare_host_runtime() {
    local seat_uid seat_gid runtime_parent owner
    seat_uid=$(id -u "$seat_user") || die "seat user does not exist: $seat_user"
    seat_gid=$(id -g "$seat_user") || die "cannot resolve seat user's group"
    if ((test_mode)); then
        runtime_parent="$test_state/runtime"
        mkdir -p "$runtime_parent"
        chmod 0700 "$runtime_parent"
    else
        runtime_parent="/run/user/$seat_uid"
        [[ -d $runtime_parent && ! -L $runtime_parent ]] || die "seat user runtime is unavailable"
        owner=$(stat -Lc '%u' "$runtime_parent")
        [[ $owner == "$seat_uid" ]] || die "seat runtime has the wrong owner"
    fi
    host_runtime=$(mktemp -d "$runtime_parent/mcnf-browser-live-audio.XXXXXX")
    chmod 0700 "$host_runtime"
    if ((!test_mode)); then
        chown "$seat_uid:$seat_gid" "$host_runtime"
    fi
    run_nonce=${host_runtime##*.}
    [[ $run_nonce =~ ^[A-Za-z0-9]+$ ]] || die "could not derive a safe run nonce"
}

host_run() {
    local seat_uid unit
    seat_uid=$(id -u "$seat_user")
    unit="mcnf-audio-query-${run_nonce}-${RANDOM}"
    "$TIMEOUT_BIN" --signal=KILL 12 \
        "$SYSTEMD_RUN_BIN" --quiet --wait --collect --pipe \
        --unit="$unit" --uid="$seat_uid" --property=UMask=0077 \
        --property=RuntimeMaxSec=10s \
        --setenv="XDG_RUNTIME_DIR=/run/user/$seat_uid" \
        --setenv="PULSE_SERVER=unix:/run/user/$seat_uid/pulse/native" \
        -- "$@"
}

host_async_start() {
    local max_seconds=$1
    shift
    [[ -z $host_async_pid ]] || die "internal error: overlapping host audio operation"
    local seat_uid
    seat_uid=$(id -u "$seat_user")
    host_async_unit="mcnf-audio-${run_nonce}-${RANDOM}"
    host_async_log="$host_runtime/${host_async_unit}.log"
    "$SYSTEMD_RUN_BIN" --quiet --wait --collect --pipe \
        --unit="$host_async_unit" --uid="$seat_uid" --property=UMask=0077 \
        --property="RuntimeMaxSec=${max_seconds}s" \
        --setenv="XDG_RUNTIME_DIR=/run/user/$seat_uid" \
        --setenv="PULSE_SERVER=unix:/run/user/$seat_uid/pulse/native" \
        -- "$@" >"$host_async_log" 2>&1 &
    host_async_pid=$!
}

host_async_wait() {
    local label=$1 pid=$host_async_pid
    [[ -n $pid ]] || die "internal error: no host operation to wait for"
    if ! wait "$pid"; then
        host_async_pid=""
        host_async_unit=""
        die "$label failed"
    fi
    host_async_pid=""
    host_async_unit=""
}

qga_call() {
    local payload=$1 result error_file="$host_runtime/qga-error"
    if ! result=$("$TIMEOUT_BIN" --signal=KILL "$QGA_TIMEOUT_SECONDS" \
        "$VIRSH_BIN" --connect "$LIBVIRT_URI" qemu-agent-command \
        "$domain" "$payload" 2>"$error_file"); then
        return 1
    fi
    ((${#result} <= 65536)) || return 1
    printf '%s' "$result" | "$PYTHON_BIN" -c '
import json
import sys
value = json.load(sys.stdin)
if not isinstance(value, dict) or "return" not in value or "error" in value:
    raise SystemExit(1)
' || return 1
    printf '%s\n' "$result"
}

qga_ping() {
    qga_call '{"execute":"guest-ping"}' >/dev/null
}

qga_read_small_file() {
    local path=$1 open_payload opened handle read_payload result data count close_payload
    local -a read_values
    open_payload=$("$PYTHON_BIN" - "$path" <<'PY'
import json
import sys
print(json.dumps({"execute":"guest-file-open","arguments":{"path":sys.argv[1],"mode":"r"}}, separators=(",",":")))
PY
)
    opened=$(qga_call "$open_payload") || return 1
    handle=$(printf '%s' "$opened" | "$PYTHON_BIN" -c '
import json
import sys
value = json.load(sys.stdin).get("return")
if isinstance(value, bool) or not isinstance(value, int) or value < 0:
    raise SystemExit(1)
print(value)
') || return 1
    read_payload=$("$PYTHON_BIN" - "$handle" "$MAX_QGA_FILE_BYTES" <<'PY'
import json
import sys
print(json.dumps({"execute":"guest-file-read","arguments":{"handle":int(sys.argv[1]),"count":int(sys.argv[2])}}, separators=(",",":")))
PY
)
    if ! result=$(qga_call "$read_payload"); then
        close_payload=$("$PYTHON_BIN" - "$handle" <<'PY'
import json
import sys
print(json.dumps({"execute":"guest-file-close","arguments":{"handle":int(sys.argv[1])}}, separators=(",",":")))
PY
)
        qga_call "$close_payload" >/dev/null 2>&1 || true
        return 1
    fi
    close_payload=$("$PYTHON_BIN" - "$handle" <<'PY'
import json
import sys
print(json.dumps({"execute":"guest-file-close","arguments":{"handle":int(sys.argv[1])}}, separators=(",",":")))
PY
)
    qga_call "$close_payload" >/dev/null || return 1
    mapfile -t read_values < <(printf '%s' "$result" | "$PYTHON_BIN" -c '
import json
import sys
value = json.load(sys.stdin).get("return")
if not isinstance(value, dict):
    raise SystemExit(1)
data = value.get("buf-b64")
count = value.get("count")
eof = value.get("eof")
if not isinstance(data, str) or isinstance(count, bool) or not isinstance(count, int) or count < 0 or eof is not True:
    raise SystemExit(1)
print(count)
print(data)
')
    [[ ${#read_values[@]} -eq 2 ]] || return 1
    count=${read_values[0]}
    data=${read_values[1]}
    ((count > 0 && count < MAX_QGA_FILE_BYTES)) || return 1
    printf '%s' "$data" | "$BASE64_BIN" -d
}

verify_guest_provenance() {
    local observed_commit observed_digest observed_transport
    qga_ping || die "Browser VM QGA ping failed"
    observed_commit=$(qga_read_small_file /usr/share/mcnf/browser-vm/source-commit) ||
        die "QGA could not read Browser VM source provenance"
    observed_digest=$(qga_read_small_file /etc/mcnf-browser-vm/image-digest) ||
        die "QGA could not read Browser VM image provenance"
    observed_transport=$(qga_read_small_file /etc/mcnf-browser-vm/transport) ||
        die "QGA could not read Browser VM transport provenance"
    observed_commit=${observed_commit//$'\r'/}
    observed_commit=${observed_commit//$'\n'/}
    observed_digest=${observed_digest//$'\r'/}
    observed_digest=${observed_digest//$'\n'/}
    observed_transport=${observed_transport//$'\r'/}
    observed_transport=${observed_transport//$'\n'/}
    [[ $observed_commit == "$source_commit" ]] || die "guest source commit does not match the requested release"
    [[ $observed_digest == "$image_digest" ]] || die "guest image digest does not match the requested release"
    [[ $observed_transport == "$transport" ]] || die "guest transport does not match the requested transport"
}

verify_domain_contract() {
    local state xml="$host_runtime/domain.xml"
    state=$("$VIRSH_BIN" --connect "$LIBVIRT_URI" domstate "$domain" 2>/dev/null | tr -d '[:space:]') ||
        die "cannot read Browser VM domain state"
    [[ $state == running ]] || die "Browser VM domain is not running"
    "$VIRSH_BIN" --connect "$LIBVIRT_URI" dumpxml "$domain" >"$xml" 2>"$host_runtime/domain-error" ||
        die "cannot read active Browser VM domain XML"
    chmod 0600 "$xml"
    "$PYTHON_BIN" - "$xml" "$PLAYBACK_STREAM_NAME" <<'PY' || exit 1
import sys
import xml.etree.ElementTree as ET

root = ET.parse(sys.argv[1]).getroot()
audios = root.findall("./devices/audio")
sounds = root.findall("./devices/sound")
if len(audios) != 1 or len(sounds) != 1 or sounds[0].get("model") != "virtio":
    raise SystemExit("Browser VM domain lacks one exact virtio audio contract")
audio = audios[0]
if audio.get("type") != "pulseaudio" or audio.get("serverName") != "tcp:127.0.0.1:4713":
    raise SystemExit("Browser VM audio backend is not the loopback Pulse endpoint")
inputs = audio.findall("input")
outputs = audio.findall("output")
if len(inputs) != 1 or len(outputs) != 1:
    raise SystemExit("Browser VM domain lacks one input and one output")
if not inputs[0].get("name") or not outputs[0].get("name"):
    raise SystemExit("Browser VM domain audio nodes are unnamed")
if outputs[0].get("streamName") != sys.argv[2]:
    raise SystemExit("Browser VM playback stream identity is not release-owned")
PY
}

read_qemu_pid() {
    local pid owner mode command_line
    [[ -f $qemu_pid_file && ! -L $qemu_pid_file ]] || die "Browser VM QEMU pid file is unavailable"
    owner=$(stat -Lc '%u' "$qemu_pid_file")
    mode=$(stat -Lc '%a' "$qemu_pid_file")
    [[ $owner == 0 || $owner == "$EUID" ]] || die "Browser VM QEMU pid file has an unexpected owner"
    (( (8#$mode & 0022) == 0 )) || die "Browser VM QEMU pid file is writable by an untrusted account"
    read -r pid <"$qemu_pid_file" || die "cannot read Browser VM QEMU pid"
    [[ $pid =~ ^[1-9][0-9]{0,9}$ ]] || die "Browser VM QEMU pid is malformed"
    if ((!test_mode)); then
        [[ -r /proc/$pid/cmdline ]] || die "Browser VM QEMU process is absent"
        command_line=$(tr '\0' '\n' <"/proc/$pid/cmdline")
        grep -Eq '(^|/)qemu-system-' <<<"$command_line" || die "Browser VM pid does not identify QEMU"
        grep -Fq "$domain" <<<"$command_line" || die "Browser VM QEMU pid is not bound to the requested domain"
    fi
    printf '%s\n' "$pid"
}

find_qemu_stream() {
    local direction=$1 pid=$2 clients_file="$host_runtime/clients" streams_file="$host_runtime/streams"
    host_run "$PACTL_BIN" list clients >"$clients_file" || return 1
    if [[ $direction == playback ]]; then
        host_run "$PACTL_BIN" list sink-inputs >"$streams_file" || return 1
    else
        host_run "$PACTL_BIN" list source-outputs >"$streams_file" || return 1
    fi
    chmod 0600 "$clients_file" "$streams_file"
    "$PYTHON_BIN" - "$clients_file" "$streams_file" "$direction" "$pid" "$PLAYBACK_STREAM_NAME" <<'PY'
import re
import sys

def blocks(path, heading):
    result = []
    current = None
    for raw in open(path, encoding="utf-8", errors="strict"):
        line = raw.rstrip("\n")
        match = re.fullmatch(rf"{re.escape(heading)} #(\d+)", line)
        if match:
            if current is not None:
                result.append(current)
            current = {"id": match.group(1), "props": {}}
            continue
        if current is None:
            continue
        field = re.match(r"\s*(Client|Sink|Source):\s*(\d+|n/a)\s*$", line)
        if field:
            current[field.group(1).lower()] = field.group(2)
        prop = re.match(r'\s*([A-Za-z0-9_.-]+) = "(.*)"\s*$', line)
        if prop:
            current["props"][prop.group(1)] = prop.group(2)
    if current is not None:
        result.append(current)
    return result

pid = sys.argv[4]
client_ids = set()
for client in blocks(sys.argv[1], "Client"):
    props = client["props"]
    binary = props.get("application.process.binary", "")
    if props.get("application.process.id") == pid and binary.startswith("qemu-system-"):
        client_ids.add(client["id"])
if not client_ids:
    raise SystemExit(1)

direction = sys.argv[3]
heading = "Sink Input" if direction == "playback" else "Source Output"
route_field = "sink" if direction == "playback" else "source"
candidates = []
for stream in blocks(sys.argv[2], heading):
    if stream.get("client") not in client_ids:
        continue
    if direction == "playback" and stream["props"].get("media.name") != sys.argv[5]:
        continue
    route = stream.get(route_field, "")
    if route.isdigit():
        candidates.append((stream["id"], route))
if len(candidates) != 1:
    raise SystemExit(1)
print("\t".join(candidates[0]))
PY
}

wait_qemu_stream() {
    local direction=$1 pid=$2 attempt result
    for ((attempt = 0; attempt < 50; attempt += 1)); do
        if result=$(find_qemu_stream "$direction" "$pid"); then
            printf '%s\n' "$result"
            return 0
        fi
        sleep 0.1
    done
    return 1
}

resolve_monitor_for_sink() {
    local sink_id=$1 sinks="$host_runtime/sinks" sources="$host_runtime/sources"
    host_run "$PACTL_BIN" list short sinks >"$sinks" || return 1
    host_run "$PACTL_BIN" list sources >"$sources" || return 1
    chmod 0600 "$sinks" "$sources"
    "$PYTHON_BIN" - "$sinks" "$sources" "$sink_id" <<'PY'
import re
import sys

sink_names = []
for line in open(sys.argv[1], encoding="utf-8"):
    fields = line.rstrip("\n").split("\t")
    if len(fields) >= 2 and fields[0] == sys.argv[3]:
        sink_names.append(fields[1])
if len(sink_names) != 1:
    raise SystemExit(1)
sink_name = sink_names[0]

matches = []
current_name = None
current_monitor = None
for raw in open(sys.argv[2], encoding="utf-8"):
    line = raw.rstrip("\n")
    if re.fullmatch(r"Source #\d+", line):
        if current_name and current_monitor == sink_name:
            matches.append(current_name)
        current_name = None
        current_monitor = None
        continue
    name = re.match(r"\s*Name:\s*(\S+)\s*$", line)
    if name:
        current_name = name.group(1)
    monitor = re.match(r"\s*Monitor of Sink:\s*(\S+)\s*$", line)
    if monitor:
        current_monitor = monitor.group(1)
if current_name and current_monitor == sink_name:
    matches.append(current_name)
if len(matches) != 1:
    raise SystemExit(1)
print(matches[0])
PY
}

resolve_short_node() {
    local kind=$1 expected=$2 listing result
    listing=$(host_run "$PACTL_BIN" list short "$kind") || return 1
    result=$(printf '%s\n' "$listing" | awk -F '\t' -v expected="$expected" '
        $2 == expected { count += 1; id = $1 }
        END { if (count == 1 && id ~ /^[0-9]+$/) print id; else exit 1 }
    ') || return 1
    printf '%s\n' "$result"
}

source_output_is_on() {
    local stream_id=$1 source_id=$2 listing
    listing=$(host_run "$PACTL_BIN" list short source-outputs) || return 1
    printf '%s\n' "$listing" | awk -F '\t' -v stream="$stream_id" -v source="$source_id" '
        $1 == stream && $2 == source { count += 1 }
        END { exit(count == 1 ? 0 : 1) }
    '
}

wait_sink_input_on() {
    local sink_id=$1 attempt listing
    for ((attempt = 0; attempt < 40; attempt += 1)); do
        listing=$(host_run "$PACTL_BIN" list short sink-inputs 2>/dev/null || true)
        if printf '%s\n' "$listing" | awk -F '\t' -v sink="$sink_id" '
            $2 == sink { count += 1 }
            END { exit(count == 1 ? 0 : 1) }
        '; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

write_tone() {
    local path=$1 frequency=$2 duration=$3
    "$PYTHON_BIN" - "$path" "$frequency" "$duration" "$SAMPLE_RATE" <<'PY'
from array import array
import math
from pathlib import Path
import sys
import wave

path = Path(sys.argv[1])
frequency = int(sys.argv[2])
duration = int(sys.argv[3])
rate = int(sys.argv[4])
samples = array("h")
for frame in range(rate * duration):
    value = round(9000 * math.sin(2.0 * math.pi * frequency * frame / rate))
    samples.extend((value, value))
with wave.open(str(path), "wb") as destination:
    destination.setnchannels(2)
    destination.setsampwidth(2)
    destination.setframerate(rate)
    destination.writeframes(samples.tobytes())
path.chmod(0o600)
PY
}

hook_environment() {
    local -n destination=$1
    destination=(
        /usr/bin/env -i
        PATH=/usr/bin:/usr/sbin
        "MCNF_BROWSER_VM_DOMAIN=$domain"
        "MCNF_BROWSER_VM_TRANSPORT=$transport"
        "MCNF_BROWSER_VM_SOURCE_COMMIT=$source_commit"
        "MCNF_BROWSER_VM_IMAGE_DIGEST=$image_digest"
    )
    if ((test_mode)); then
        destination+=("MCNF_LIVE_AUDIO_TEST_STATE=$test_state")
    fi
}

guest_hook_start() {
    [[ -z $guest_hook_pid ]] || die "internal error: overlapping guest control operations"
    local -a environment
    hook_environment environment
    "$TIMEOUT_BIN" --signal=KILL "$OPERATION_TIMEOUT_SECONDS" \
        "${environment[@]}" "$guest_probe_hook" "$@" >/dev/null 2>&1 &
    guest_hook_pid=$!
}

guest_hook_wait() {
    local label=$1 pid=$guest_hook_pid
    [[ -n $pid ]] || die "internal error: guest control hook is not running"
    if ! wait "$pid"; then
        guest_hook_pid=""
        die "$label guest control hook failed"
    fi
    guest_hook_pid=""
}

wait_private_file() {
    local path=$1 attempt
    for ((attempt = 0; attempt < CONTROL_FILE_WAIT_ATTEMPTS; attempt += 1)); do
        if private_regular_file "$path" 262144; then
            return 0
        fi
        if [[ -n $guest_hook_pid ]] && ! kill -0 "$guest_hook_pid" 2>/dev/null; then
            return 1
        fi
        sleep 0.1
    done
    return 1
}

guest_probe_receipt_valid() {
    local receipt=$1 operation=$2 state=$3 phase=$4 tone=$5
    private_regular_file "$receipt" 262144 || return 1
    "$PYTHON_BIN" - "$receipt" "$operation" "$state" "$phase" "$tone" \
        "$source_commit" "$image_digest" "$transport" <<'PY'
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import sys

path = Path(sys.argv[1])
data = json.loads(path.read_text(encoding="utf-8"))
expected_fields = {
    "schema_version", "kind", "operation", "state", "phase",
    "expected_tone_hz", "profile", "source_commit", "image_digest",
    "transport", "control_channel", "browser_api",
    "user_gesture_observed", "capture_point", "channels", "recorded_at",
}
if not isinstance(data, dict) or set(data) != expected_fields:
    raise SystemExit(1)
operation, state, phase, tone = sys.argv[2], sys.argv[3], sys.argv[4], int(sys.argv[5])
if data["schema_version"] != 1 or isinstance(data["schema_version"], bool):
    raise SystemExit(1)
if data["kind"] != "browser_vm_guest_audio_probe_receipt":
    raise SystemExit(1)
if (data["operation"], data["state"], data["phase"], data["expected_tone_hz"]) != (operation, state, phase, tone):
    raise SystemExit(1)
if data["profile"] != "browser-vm-chromium":
    raise SystemExit(1)
if (data["source_commit"], data["image_digest"], data["transport"]) != tuple(sys.argv[6:9]):
    raise SystemExit(1)
if data["control_channel"] != "rdp-webaudio" or data["user_gesture_observed"] is not True:
    raise SystemExit(1)
if data["channels"] != 2 or isinstance(data["channels"], bool):
    raise SystemExit(1)
if operation == "playback":
    if data["browser_api"] != "WebAudio" or data["capture_point"] != "guest-browser-webaudio-output":
        raise SystemExit(1)
elif operation == "capture":
    if data["browser_api"] != "getUserMedia+WebAudio" or data["capture_point"] != "guest-browser-vm-capture-input":
        raise SystemExit(1)
else:
    raise SystemExit(1)
try:
    recorded = datetime.strptime(data["recorded_at"], "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
except (TypeError, ValueError):
    raise SystemExit(1)
age = datetime.now(timezone.utc) - recorded
if age < -timedelta(minutes=5) or age > timedelta(minutes=5):
    raise SystemExit(1)
PY
}

write_signal() {
    local path=$1
    local temporary="${path}.tmp.$$"
    printf '%s\n' start >"$temporary"
    chmod 0600 "$temporary"
    mv -f "$temporary" "$path"
}

collect_playback() {
    local phase=$1 tone=$2 index=$3
    local control="$stage_dir/control/${index}-${phase}-playback"
    local ready="${control}-ready.json" started="${control}-started.json"
    local completed="${control}-completed.json" signal="${control}-start"
    local work_wav="$host_runtime/${phase}-playback.wav"
    local final_wav="$stage_dir/samples/${index}-${phase}-playback.wav"
    local qemu_pid stream sink_id monitor captured_at

    guest_hook_start playback \
        --phase "$phase" --tone-hz "$tone" --duration-seconds "$STIMULUS_SECONDS" \
        --ready-receipt "$ready" --start-signal "$signal" \
        --started-receipt "$started" --completed-receipt "$completed"
    wait_private_file "$ready" || die "$phase playback control did not become ready"
    guest_probe_receipt_valid "$ready" playback ready "$phase" "$tone" ||
        die "$phase playback ready receipt is invalid"
    write_signal "$signal"
    wait_private_file "$started" || die "$phase playback did not report a started Browser tone"
    guest_probe_receipt_valid "$started" playback started "$phase" "$tone" ||
        die "$phase playback started receipt is invalid"

    qemu_pid=$(read_qemu_pid)
    stream=$(wait_qemu_stream playback "$qemu_pid") ||
        die "$phase playback could not resolve one exact Browser QEMU sink-input"
    IFS=$'\t' read -r _ sink_id <<<"$stream"
    monitor=$(resolve_monitor_for_sink "$sink_id") ||
        die "$phase playback could not resolve the exact monitor of the active QEMU sink"

    host_async_start 10 "$PW_RECORD_BIN" \
        --target "$monitor" --rate "$SAMPLE_RATE" --channels "$SAMPLE_CHANNELS" \
        --channel-map FL,FR --format s16 --container wav -n "$SAMPLE_FRAMES" "$work_wav"
    host_async_wait "$phase host playback monitor capture"
    guest_hook_wait "$phase playback"
    wait_private_file "$completed" || die "$phase playback completion receipt is missing"
    guest_probe_receipt_valid "$completed" playback completed "$phase" "$tone" ||
        die "$phase playback completion receipt is invalid"
    private_regular_file "$work_wav" || die "$phase playback monitor did not produce a private WAV"
    install -m 0600 "$work_wav" "$final_wav"
    captured_at=$(utc_now)
    COLLECTED_AT=$captured_at
}

load_injection_sink() {
    local phase=$1 module_output sink_id
    injection_sink="mcnf_live_audio_${run_nonce}_${phase//-/_}"
    module_output=$(host_run "$PACTL_BIN" load-module module-null-sink \
        "sink_name=$injection_sink" "rate=$SAMPLE_RATE" "channels=$SAMPLE_CHANNELS" \
        'sink_properties=device.description=MCNF-Live-Audio-Test') ||
        die "$phase capture could not create its bounded private stimulus sink"
    module_output=${module_output//$'\r'/}
    module_output=${module_output//$'\n'/}
    [[ $module_output =~ ^[0-9]+$ ]] || die "$phase capture received an invalid PipeWire module id"
    injection_module=$module_output
    sink_id=$(resolve_short_node sinks "$injection_sink") ||
        die "$phase capture could not resolve its exact stimulus sink"
    injection_monitor=$(resolve_monitor_for_sink "$sink_id") ||
        die "$phase capture could not resolve its exact stimulus monitor"
}

restore_capture_route() {
    if [[ -n $capture_stream_id && -n $capture_original_source ]]; then
        host_run "$PACTL_BIN" move-source-output "$capture_stream_id" "$capture_original_source" >/dev/null 2>&1 || true
    fi
    capture_stream_id=""
    capture_original_source=""
    if [[ -n $injection_module ]]; then
        host_run "$PACTL_BIN" unload-module "$injection_module" >/dev/null 2>&1 || true
    fi
    injection_module=""
    injection_sink=""
    injection_monitor=""
}

collect_capture() {
    local phase=$1 tone=$2 index=$3
    local control="$stage_dir/control/${index}-${phase}-capture"
    local ready="${control}-ready.json" completed="${control}-completed.json"
    local signal="${control}-start" release="${control}-release"
    local final_wav="$stage_dir/samples/${index}-${phase}-capture.wav"
    local tone_wav="$host_runtime/${phase}-capture-tone.wav"
    local qemu_pid stream source_id injection_sink_id injection_monitor_id captured_at

    write_tone "$tone_wav" "$tone" "$STIMULUS_SECONDS"
    if ((!test_mode)); then
        chown "$(id -u "$seat_user"):$(id -g "$seat_user")" "$tone_wav"
    fi

    guest_hook_start capture \
        --phase "$phase" --tone-hz "$tone" --duration-seconds 2 \
        --ready-receipt "$ready" --start-signal "$signal" \
        --completed-receipt "$completed" --release-signal "$release" \
        --output-wav "$final_wav"
    wait_private_file "$ready" || die "$phase capture control did not open a Browser-owned microphone"
    guest_probe_receipt_valid "$ready" capture ready "$phase" "$tone" ||
        die "$phase capture ready receipt is invalid"

    qemu_pid=$(read_qemu_pid)
    stream=$(wait_qemu_stream capture "$qemu_pid") ||
        die "$phase capture could not resolve one exact Browser QEMU source-output"
    IFS=$'\t' read -r capture_stream_id source_id <<<"$stream"
    capture_original_source=$source_id
    load_injection_sink "$phase"
    injection_sink_id=$(resolve_short_node sinks "$injection_sink") || die "stimulus sink disappeared"
    injection_monitor_id=$(resolve_short_node sources "$injection_monitor") || die "stimulus monitor disappeared"
    host_run "$PACTL_BIN" move-source-output "$capture_stream_id" "$injection_monitor" >/dev/null ||
        die "$phase capture could not route the exact QEMU source-output to the stimulus monitor"
    source_output_is_on "$capture_stream_id" "$injection_monitor_id" ||
        die "$phase capture source-output route did not take effect"

    host_async_start 12 "$PW_PLAY_BIN" --target "$injection_sink" --volume 0.25 "$tone_wav"
    wait_sink_input_on "$injection_sink_id" || die "$phase capture stimulus did not reach its exact sink"
    write_signal "$signal"
    wait_private_file "$completed" || die "$phase capture completion receipt is missing"
    guest_probe_receipt_valid "$completed" capture completed "$phase" "$tone" ||
        die "$phase capture completion receipt is invalid"
    private_regular_file "$final_wav" || die "$phase Browser getUserMedia probe did not produce a private WAV"

    host_async_wait "$phase host capture stimulus"
    host_run "$PACTL_BIN" move-source-output "$capture_stream_id" "$capture_original_source" >/dev/null ||
        die "$phase capture could not restore the QEMU source-output"
    source_output_is_on "$capture_stream_id" "$capture_original_source" ||
        die "$phase capture could not verify the restored QEMU source-output"
    capture_stream_id=""
    capture_original_source=""
    host_run "$PACTL_BIN" unload-module "$injection_module" >/dev/null ||
        die "$phase capture could not remove its private stimulus sink"
    injection_module=""
    injection_sink=""
    injection_monitor=""
    write_signal "$release"
    guest_hook_wait "$phase capture"
    captured_at=$(utc_now)
    COLLECTED_AT=$captured_at
}

reconnect_receipt_valid() {
    local receipt=$1 lower_bound=$2 upper_bound=$3 before_capture=$4
    private_regular_file "$receipt" 262144 || return 1
    "$PYTHON_BIN" - "$receipt" "$domain" "$transport" "$source_commit" "$image_digest" \
        "$lower_bound" "$upper_bound" "$before_capture" <<'PY'
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import sys

data = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
fields = {
    "schema_version", "kind", "domain", "profile", "source_commit",
    "image_digest", "transport", "status", "disconnect_observed_at",
    "reconnect_observed_at",
}
if not isinstance(data, dict) or set(data) != fields:
    raise SystemExit(1)
if data["schema_version"] != 1 or isinstance(data["schema_version"], bool):
    raise SystemExit(1)
if data["kind"] != "browser_vm_transport_reconnect_receipt" or data["status"] != "observed":
    raise SystemExit(1)
if data["domain"] != sys.argv[2] or data["profile"] != "browser-vm-chromium":
    raise SystemExit(1)
if (data["transport"], data["source_commit"], data["image_digest"]) != tuple(sys.argv[3:6]):
    raise SystemExit(1)

def stamp(value):
    return datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)

try:
    disconnect = stamp(data["disconnect_observed_at"])
    reconnect = stamp(data["reconnect_observed_at"])
    lower = stamp(sys.argv[6])
    upper = stamp(sys.argv[7])
    before = stamp(sys.argv[8])
except (TypeError, ValueError):
    raise SystemExit(1)
if not (before < disconnect < reconnect):
    raise SystemExit(1)
if disconnect < lower or reconnect > upper + timedelta(seconds=1):
    raise SystemExit(1)
if reconnect - disconnect > timedelta(minutes=5):
    raise SystemExit(1)
print(data["disconnect_observed_at"])
print(data["reconnect_observed_at"])
PY
}

run_reconnect() {
    local before_capture=$1 receipt="$stage_dir/control/reconnect.json"
    local hook_started hook_finished parsed
    local -a reconnect_values
    hook_started=$(timestamp_after "$before_capture") || die "could not establish a timestamp after pre-recovery capture"
    local -a environment
    hook_environment environment
    environment+=("MCNF_BROWSER_VM_RECONNECT_RECEIPT=$receipt")
    if ! "$TIMEOUT_BIN" --signal=KILL "$RECONNECT_TIMEOUT_SECONDS" \
        "${environment[@]}" "$reconnect_hook" >/dev/null 2>&1; then
        die "explicit transport reconnect hook failed"
    fi
    hook_finished=$(utc_now)
    parsed=$(reconnect_receipt_valid "$receipt" "$hook_started" "$hook_finished" "$before_capture") ||
        die "explicit transport reconnect receipt is missing or invalid"
    verify_domain_contract
    verify_guest_provenance
    mapfile -t reconnect_values <<<"$parsed"
    [[ ${#reconnect_values[@]} -eq 2 ]] || die "reconnect receipt omitted an observed timestamp"
    DISCONNECT_AT=${reconnect_values[0]}
    RECONNECT_AT=${reconnect_values[1]}
}

write_manifest() {
    local manifest=$1 disconnect=$2 reconnect=$3 recorded=$4
    shift 4
    "$PYTHON_BIN" - "$manifest" "$source_commit" "$image_digest" "$transport" \
        "$disconnect" "$reconnect" "$recorded" "$@" <<'PY'
import json
from pathlib import Path
import sys

manifest = Path(sys.argv[1])
source_commit, image_digest, transport = sys.argv[2:5]
disconnect, reconnect, recorded = sys.argv[5:8]
rows = sys.argv[8:]
if len(rows) != 28:
    raise SystemExit("collector internal capture row mismatch")
captures = []
for offset in range(0, len(rows), 7):
    phase, direction, point, path, digest, captured_at, tone = rows[offset:offset + 7]
    captures.append({
        "phase": phase,
        "direction": direction,
        "capture_point": point,
        "path": path,
        "sha256": digest,
        "captured_at": captured_at,
        "expected_tone_hz": int(tone),
    })
document = {
    "schema_version": 1,
    "kind": "browser_vm_live_audio_samples",
    "profile": "browser-vm-chromium",
    "image": "browser-vm-chromium",
    "source_commit": source_commit,
    "image_digest": image_digest,
    "status": "observed",
    "source": "live-browser-vm-audio-capture",
    "transport": transport,
    "disconnect_observed_at": disconnect,
    "reconnect_observed_at": reconnect,
    "captures": captures,
    "recorded_at": recorded,
}
temporary = manifest.with_name(manifest.name + ".tmp")
temporary.write_text(json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")
temporary.chmod(0o600)
temporary.replace(manifest)
PY
}

validate_and_publish() {
    local manifest=$1 validation="$host_runtime/validation.json"
    "$PYTHON_BIN" "$VALIDATOR" validate "$manifest" >"$validation" ||
        die "live Browser VM audio validator rejected the collected samples"
    chmod 0600 "$validation"
    "$PYTHON_BIN" - "$validation" <<'PY' || exit 1
import json
import sys

result = json.load(open(sys.argv[1], encoding="utf-8"))
claims = result.get("claims", {})
if result.get("status") != "validated":
    raise SystemExit("audio validation did not return validated")
if claims.get("scope") != "digital-pcm-path-only":
    raise SystemExit("audio validator returned an unexpected evidence scope")
if claims.get("physical_audibility") != "operator-confirmation-required":
    raise SystemExit("audio validator weakened the physical audibility boundary")
if claims.get("production_audio_acceptance") != "not-proven-by-this-validator":
    raise SystemExit("audio validator overclaimed production acceptance")
PY
    [[ ! -e $output_dir && ! -L $output_dir ]] || die "output path appeared during collection"
    mv -T "$stage_dir" "$output_dir"
    stage_dir=""
    finalized=1
}

cleanup() {
    local rc=$?
    trap - EXIT HUP INT TERM
    set +e
    if [[ -n $guest_hook_pid ]]; then
        kill "$guest_hook_pid" 2>/dev/null
        wait "$guest_hook_pid" 2>/dev/null
    fi
    if [[ -n $host_async_unit ]]; then
        "$SYSTEMCTL_BIN" stop "$host_async_unit.service" >/dev/null 2>&1
    fi
    if [[ -n $host_async_pid ]]; then
        kill "$host_async_pid" 2>/dev/null
        wait "$host_async_pid" 2>/dev/null
    fi
    restore_capture_route
    if [[ -n $host_runtime && $host_runtime == */mcnf-browser-live-audio.* ]]; then
        rm -rf -- "$host_runtime"
    fi
    if [[ -n $stage_dir && $stage_dir == */.*.staging.* ]]; then
        rm -rf -- "$stage_dir"
    fi
    exit "$rc"
}

collect() {
    ((test_mode || EUID == 0)) || die "live collection must run as root"
    valid_domain "$domain" || die "invalid Browser VM domain"
    valid_user "$seat_user" || die "invalid seat user"
    valid_commit "$source_commit" || die "--source-commit must be a full non-null lowercase SHA"
    valid_digest "$image_digest" || die "--image-digest must be a full non-null lowercase SHA-256 digest"
    valid_transport "$transport" || die "--transport must be rdp or sunshine"
    [[ -x $VALIDATOR && -f $VALIDATOR && ! -L $VALIDATOR ]] || die "live audio validator is unavailable"
    trusted_executable "$guest_probe_hook" || die "guest Browser control hook is not a trusted executable"
    trusted_executable "$reconnect_hook" || die "transport reconnect hook is not a trusted executable"
    validate_warning_helper
    resolve_commands
    prepare_output
    prepare_host_runtime
    trap cleanup EXIT HUP INT TERM

    verify_domain_contract
    verify_guest_provenance

    # The first guest-owned user gesture, tone, recorder, or graph mutation is
    # forbidden until the visible five-second warning has completed.
    invoke_warning

    local before_playback_at before_capture_at after_playback_at after_capture_at
    local disconnect_at reconnect_at recorded_at manifest
    local before_latest after_floor
    local -a rows=()

    collect_playback before-recovery 523 0
    before_playback_at=$COLLECTED_AT
    rows+=(before-recovery playback host-pipewire-browser-vm-playback \
        samples/0-before-recovery-playback.wav \
        "$("$SHA256SUM_BIN" "$stage_dir/samples/0-before-recovery-playback.wav" | awk '{print $1}')" \
        "$before_playback_at" 523)
    collect_capture before-recovery 719 1
    before_capture_at=$COLLECTED_AT
    rows+=(before-recovery capture guest-browser-vm-capture-input \
        samples/1-before-recovery-capture.wav \
        "$("$SHA256SUM_BIN" "$stage_dir/samples/1-before-recovery-capture.wav" | awk '{print $1}')" \
        "$before_capture_at" 719)
    before_latest=$before_capture_at
    [[ $before_playback_at > $before_latest ]] && before_latest=$before_playback_at

    # Transport mutation gets its own warning window. The reconnect hook must
    # return observed timestamps in a private receipt; exit status alone is not
    # promoted into recovery evidence.
    invoke_warning
    run_reconnect "$before_latest"
    disconnect_at=$DISCONNECT_AT
    reconnect_at=$RECONNECT_AT
    if ! valid_timestamp "$disconnect_at" || ! valid_timestamp "$reconnect_at"; then
        die "reconnect timestamps are malformed"
    fi

    after_floor=$(timestamp_after "$reconnect_at") || die "could not establish a post-reconnect capture window"
    # after_floor is intentionally obtained before starting the after phase;
    # each captured_at value is produced only after its WAV completes.
    : "$after_floor"
    collect_playback after-recovery 977 2
    after_playback_at=$COLLECTED_AT
    [[ $after_playback_at > $reconnect_at ]] || die "post-recovery playback timestamp is not after reconnect"
    rows+=(after-recovery playback host-pipewire-browser-vm-playback \
        samples/2-after-recovery-playback.wav \
        "$("$SHA256SUM_BIN" "$stage_dir/samples/2-after-recovery-playback.wav" | awk '{print $1}')" \
        "$after_playback_at" 977)
    collect_capture after-recovery 1301 3
    after_capture_at=$COLLECTED_AT
    [[ $after_capture_at > $reconnect_at ]] || die "post-recovery capture timestamp is not after reconnect"
    rows+=(after-recovery capture guest-browser-vm-capture-input \
        samples/3-after-recovery-capture.wav \
        "$("$SHA256SUM_BIN" "$stage_dir/samples/3-after-recovery-capture.wav" | awk '{print $1}')" \
        "$after_capture_at" 1301)

    recorded_at=$(utc_now)
    manifest="$stage_dir/audio-evidence.json"
    write_manifest "$manifest" "$disconnect_at" "$reconnect_at" "$recorded_at" "${rows[@]}"
    validate_and_publish "$manifest"
    log "validated digital PCM evidence written to $output_dir; physical audibility is not claimed"
}

self_test() {
    local fixture mock_bin evidence_parent manifest validation events
    local marker mutation_after_marker
    fixture=$(mktemp -d)
    chmod 0700 "$fixture"
    mock_bin="$fixture/bin"
    test_state="$fixture/state"
    evidence_parent="$fixture/evidence"
    mkdir -m 0700 "$mock_bin" "$test_state" "$test_state/runtime" "$evidence_parent"
    events="$test_state/events"
    : >"$events"
    chmod 0600 "$events"

    trap 'rm -rf -- "$fixture"' EXIT HUP INT TERM

    cat >"$mock_bin/seat-update-warning" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
readonly WAIT_SECONDS=5
readonly TOAST_BODY='{"severity":"warning","flag":"AI-GENERATED-ALERT"}'
sleep() { :; }
state=${MCNF_LIVE_AUDIO_TEST_STATE:?}
[[ ! -e "$state/warning-fail" ]] || exit 1
count=0
[[ ! -f "$state/warning-count" ]] || read -r count <"$state/warning-count"
count=$((count + 1))
printf '%s\n' "$count" >"$state/warning-count"
printf 'warning:%s\n' "$count" >>"$state/events"
sleep "$WAIT_SECONDS"
EOF

    cat >"$mock_bin/virsh" <<'PY'
#!/usr/bin/env python3
import base64
import json
import os
from pathlib import Path
import sys

state = Path(os.environ["MCNF_LIVE_AUDIO_TEST_STATE"])
events = state / "events"

def event(value):
    with events.open("a", encoding="utf-8") as out:
        out.write(value + "\n")

args = sys.argv[1:]
if args[:2] == ["--connect", "qemu:///system"]:
    args = args[2:]
if not args:
    raise SystemExit(2)
command = args[0]
if command == "domstate":
    print("running")
    raise SystemExit(0)
if command == "dumpxml":
    print("""<domain><devices><sound model='virtio'/><audio id='1' type='pulseaudio' serverName='tcp:127.0.0.1:4713'><input name='browser-vm-capture'/><output name='browser-vm' streamName='MCNF-Browser-VM'/></audio></devices></domain>""")
    raise SystemExit(0)
if command != "qemu-agent-command" or len(args) != 3:
    raise SystemExit(2)
if (state / "qga-fail").exists():
    raise SystemExit(1)
payload = json.loads(args[2])
execute = payload.get("execute")
arguments = payload.get("arguments", {})
if execute == "guest-exec":
    event("forbidden:qga-guest-exec")
    raise SystemExit(1)
if execute == "guest-ping":
    event("qga:ping")
    print('{"return":{}}')
    raise SystemExit(0)
handles = state / "handles"
handles.mkdir(exist_ok=True)
counter = state / "next-handle"
if execute == "guest-file-open":
    path = arguments.get("path")
    allowed = {
        "/usr/share/mcnf/browser-vm/source-commit": os.environ["MCNF_TEST_SOURCE_COMMIT"] + "\n",
        "/etc/mcnf-browser-vm/image-digest": os.environ["MCNF_TEST_IMAGE_DIGEST"] + "\n",
        "/etc/mcnf-browser-vm/transport": os.environ["MCNF_TEST_TRANSPORT"] + "\n",
    }
    if arguments.get("mode") != "r" or path not in allowed:
        raise SystemExit(1)
    number = int(counter.read_text() if counter.exists() else "1")
    counter.write_text(str(number + 1), encoding="utf-8")
    (handles / f"{number}.json").write_text(json.dumps({"path": path, "data": allowed[path]}), encoding="utf-8")
    event("qga:read:" + path)
    print(json.dumps({"return": number}))
    raise SystemExit(0)
if execute in {"guest-file-read", "guest-file-close"}:
    number = int(arguments.get("handle", -1))
    record = handles / f"{number}.json"
    if not record.exists():
        raise SystemExit(1)
    if execute == "guest-file-close":
        record.unlink()
        print('{"return":{}}')
        raise SystemExit(0)
    data = json.loads(record.read_text(encoding="utf-8"))["data"].encode()
    print(json.dumps({"return": {"count": len(data), "buf-b64": base64.b64encode(data).decode(), "eof": True}}))
    raise SystemExit(0)
raise SystemExit(1)
PY

    cat >"$mock_bin/systemd-run" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
while (($#)); do
    if [[ $1 == -- ]]; then
        shift
        exec "$@"
    fi
    shift
done
exit 2
EOF

    cat >"$mock_bin/systemctl" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF

    cat >"$mock_bin/pactl" <<'PY'
#!/usr/bin/env python3
from pathlib import Path
import os
import sys

state = Path(os.environ["MCNF_LIVE_AUDIO_TEST_STATE"])
args = sys.argv[1:]

def event(value):
    with (state / "events").open("a", encoding="utf-8") as out:
        out.write(value + "\n")

def warned():
    return (state / "warning-count").exists()

injection = (state / "injection-name").read_text().strip() if (state / "injection-name").exists() else ""
current_source = (state / "current-source").read_text().strip() if (state / "current-source").exists() else "12"
if args == ["list", "clients"]:
    print('''Client #88
\tProperties:
\t\tapplication.name = "QEMU"
\t\tapplication.process.id = "4242"
\t\tapplication.process.binary = "qemu-system-x86_64"

Client #99
\tProperties:
\t\tapplication.name = "pw-play"
\t\tapplication.process.id = "9001"
\t\tapplication.process.binary = "pw-play"''')
elif args == ["list", "sink-inputs"]:
    if (state / "playback-active").exists():
        print('''Sink Input #44
\tClient: 88
\tSink: 10
\tProperties:
\t\tmedia.name = "MCNF-Browser-VM"''')
        if (state / "ambiguous-playback").exists():
            print('''
Sink Input #45
\tClient: 88
\tSink: 10
\tProperties:
\t\tmedia.name = "MCNF-Browser-VM"''')
    if injection and (state / "stimulus-active").exists():
        print('''
Sink Input #66
\tClient: 99
\tSink: 20
\tProperties:
\t\tmedia.name = "mcnf-live-audio-stimulus"''')
elif args == ["list", "source-outputs"]:
    if (state / "capture-active").exists():
        print(f'''Source Output #55
\tClient: 88
\tSource: {current_source}
\tProperties:
\t\tmedia.name = "browser-vm-capture"''')
elif args == ["list", "short", "sinks"]:
    print("10\talsa_output.test.analog-stereo\tPipeWire\ts16le 2ch 48000Hz\tRUNNING")
    if injection:
        print(f"20\t{injection}\tPipeWire\ts16le 2ch 48000Hz\tIDLE")
elif args == ["list", "short", "sources"]:
    print("11\talsa_output.test.analog-stereo.monitor\tPipeWire\ts16le 2ch 48000Hz\tRUNNING")
    print("12\talsa_input.test.analog-stereo\tPipeWire\ts16le 2ch 48000Hz\tIDLE")
    if injection:
        print(f"21\t{injection}.monitor\tPipeWire\ts16le 2ch 48000Hz\tRUNNING")
elif args == ["list", "sources"]:
    print('''Source #11
\tName: alsa_output.test.analog-stereo.monitor
\tMonitor of Sink: alsa_output.test.analog-stereo
Source #12
\tName: alsa_input.test.analog-stereo
\tMonitor of Sink: n/a''')
    if injection:
        print(f'''Source #21
\tName: {injection}.monitor
\tMonitor of Sink: {injection}''')
elif args == ["list", "short", "sink-inputs"]:
    if (state / "playback-active").exists():
        print("44\t10\t88\tPipeWire\ts16le 2ch 48000Hz")
    if injection and (state / "stimulus-active").exists():
        print("66\t20\t99\tPipeWire\ts16le 2ch 48000Hz")
elif args == ["list", "short", "source-outputs"]:
    if (state / "capture-active").exists():
        print(f"55\t{current_source}\t88\tPipeWire\ts16le 2ch 48000Hz")
elif args[:2] == ["load-module", "module-null-sink"]:
    if not warned():
        event("forbidden:unwarned-pactl-load")
        raise SystemExit(1)
    names = [value.split("=", 1)[1] for value in args[2:] if value.startswith("sink_name=")]
    if len(names) != 1:
        raise SystemExit(1)
    (state / "injection-name").write_text(names[0] + "\n", encoding="utf-8")
    event("mutate:pactl-load:" + names[0])
    print("77")
elif args[:2] == ["move-source-output", "55"] and len(args) == 3:
    target = args[2]
    if target.endswith(".monitor"):
        source = "21"
    elif target == "12":
        source = "12"
    else:
        raise SystemExit(1)
    (state / "current-source").write_text(source + "\n", encoding="utf-8")
    event("mutate:pactl-move:" + source)
elif args == ["unload-module", "77"]:
    event("mutate:pactl-unload")
    (state / "injection-name").unlink(missing_ok=True)
else:
    raise SystemExit("unhandled pactl mock: " + repr(args))
PY

    cat >"$mock_bin/pw-record" <<'PY'
#!/usr/bin/env python3
from array import array
import math
import os
from pathlib import Path
import sys
import wave

state = Path(os.environ["MCNF_LIVE_AUDIO_TEST_STATE"])
if not (state / "warning-count").exists():
    raise SystemExit(1)
path = Path(sys.argv[-1])
if "before-recovery" in path.name:
    tone = 523
elif "after-recovery" in path.name:
    tone = 977
else:
    raise SystemExit(1)
rate = 48000
samples = array("h")
for frame in range(rate * 2):
    value = round(9000 * math.sin(2 * math.pi * tone * frame / rate))
    samples.extend((value, value))
with wave.open(str(path), "wb") as out:
    out.setnchannels(2)
    out.setsampwidth(2)
    out.setframerate(rate)
    out.writeframes(samples.tobytes())
path.chmod(0o600)
with (state / "events").open("a", encoding="utf-8") as out:
    out.write(f"mutate:host-monitor-record:{tone}\n")
PY

    cat >"$mock_bin/pw-play" <<'PY'
#!/usr/bin/env python3
from pathlib import Path
import os
import time

state = Path(os.environ["MCNF_LIVE_AUDIO_TEST_STATE"])
if not (state / "warning-count").exists():
    raise SystemExit(1)
(state / "stimulus-active").touch()
with (state / "events").open("a", encoding="utf-8") as out:
    out.write("mutate:host-capture-stimulus\n")
try:
    time.sleep(1.0)
finally:
    (state / "stimulus-active").unlink(missing_ok=True)
PY

    cat >"$mock_bin/guest-probe" <<'PY'
#!/usr/bin/env python3
from array import array
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import sys
import time
import wave

state = Path(os.environ["MCNF_LIVE_AUDIO_TEST_STATE"])
args = sys.argv[1:]
operation = args.pop(0)
options = {}
while args:
    key = args.pop(0)
    if not key.startswith("--") or not args:
        raise SystemExit(2)
    options[key[2:]] = args.pop(0)
phase = options["phase"]
tone = int(options["tone-hz"])

def now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

def receipt(path, state_value):
    if (state / "bad-probe-receipt").exists():
        channel = "qga-audio"
    else:
        channel = "rdp-webaudio"
    capture = "guest-browser-webaudio-output" if operation == "playback" else "guest-browser-vm-capture-input"
    api = "WebAudio" if operation == "playback" else "getUserMedia+WebAudio"
    data = {
        "schema_version": 1,
        "kind": "browser_vm_guest_audio_probe_receipt",
        "operation": operation,
        "state": state_value,
        "phase": phase,
        "expected_tone_hz": tone,
        "profile": "browser-vm-chromium",
        "source_commit": os.environ["MCNF_BROWSER_VM_SOURCE_COMMIT"],
        "image_digest": os.environ["MCNF_BROWSER_VM_IMAGE_DIGEST"],
        "transport": os.environ["MCNF_BROWSER_VM_TRANSPORT"],
        "control_channel": channel,
        "browser_api": api,
        "user_gesture_observed": True,
        "capture_point": capture,
        "channels": 2,
        "recorded_at": now(),
    }
    target = Path(path)
    temporary = target.with_name(target.name + ".tmp")
    temporary.write_text(json.dumps(data, sort_keys=True) + "\n", encoding="utf-8")
    temporary.chmod(0o600)
    temporary.replace(target)

def wait_for(path):
    for _ in range(500):
        if Path(path).is_file():
            return
        time.sleep(0.01)
    raise SystemExit(1)

def write_wav(path):
    samples = array("h")
    rate = 48000
    for frame in range(rate * 2):
        value = round(9000 * math.sin(2 * math.pi * tone * frame / rate))
        samples.extend((value, value))
    with wave.open(path, "wb") as out:
        out.setnchannels(2)
        out.setsampwidth(2)
        out.setframerate(rate)
        out.writeframes(samples.tobytes())
    Path(path).chmod(0o600)

if not (state / "warning-count").exists():
    raise SystemExit(1)
with (state / "events").open("a", encoding="utf-8") as out:
    out.write(f"mutate:guest-{operation}-ready:{phase}:{tone}\n")
if operation == "playback":
    receipt(options["ready-receipt"], "ready")
    wait_for(options["start-signal"])
    (state / "playback-active").touch()
    receipt(options["started-receipt"], "started")
    time.sleep(0.7)
    receipt(options["completed-receipt"], "completed")
    (state / "playback-active").unlink(missing_ok=True)
elif operation == "capture":
    (state / "capture-active").touch()
    receipt(options["ready-receipt"], "ready")
    wait_for(options["start-signal"])
    write_wav(options["output-wav"])
    receipt(options["completed-receipt"], "completed")
    wait_for(options["release-signal"])
    (state / "capture-active").unlink(missing_ok=True)
else:
    raise SystemExit(2)
PY

    cat >"$mock_bin/reconnect" <<'PY'
#!/usr/bin/env python3
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import time

state = Path(os.environ["MCNF_LIVE_AUDIO_TEST_STATE"])
count = int((state / "warning-count").read_text())
if count < 2 or (state / "reconnect-fail").exists():
    raise SystemExit(1)
with (state / "events").open("a", encoding="utf-8") as out:
    out.write("mutate:reconnect\n")
stamp = lambda: datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
disconnect = stamp()
time.sleep(1.05)
reconnect = stamp()
receipt = Path(os.environ["MCNF_BROWSER_VM_RECONNECT_RECEIPT"])
data = {
    "schema_version": 1,
    "kind": "browser_vm_transport_reconnect_receipt",
    "domain": os.environ["MCNF_BROWSER_VM_DOMAIN"],
    "profile": "browser-vm-chromium",
    "source_commit": os.environ["MCNF_BROWSER_VM_SOURCE_COMMIT"],
    "image_digest": os.environ["MCNF_BROWSER_VM_IMAGE_DIGEST"],
    "transport": os.environ["MCNF_BROWSER_VM_TRANSPORT"],
    "status": "observed",
    "disconnect_observed_at": disconnect,
    "reconnect_observed_at": reconnect,
}
receipt.write_text(json.dumps(data, sort_keys=True) + "\n", encoding="utf-8")
receipt.chmod(0o600)
PY

    chmod 0755 "$mock_bin"/*
    printf '%s\n' 4242 >"$test_state/qemu.pid"
    chmod 0600 "$test_state/qemu.pid"

    test_mode=1
    domain=browser-vm
    seat_user=$(id -un)
    source_commit=0123456789abcdef0123456789abcdef01234567
    image_digest=sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
    transport=rdp
    guest_probe_hook="$mock_bin/guest-probe"
    reconnect_hook="$mock_bin/reconnect"
    warning_helper="$mock_bin/seat-update-warning"
    qemu_pid_file="$test_state/qemu.pid"
    output_dir="$evidence_parent/pass"
    export PATH="$mock_bin:/usr/bin:/bin"
    export MCNF_LIVE_AUDIO_TEST_STATE="$test_state"
    export MCNF_TEST_SOURCE_COMMIT="$source_commit"
    export MCNF_TEST_IMAGE_DIGEST="$image_digest"
    export MCNF_TEST_TRANSPORT="$transport"

    collect
    trap - EXIT HUP INT TERM
    [[ $finalized -eq 1 && -d $output_dir && ! -L $output_dir ]] ||
        die "self-test did not publish its private evidence directory"
    manifest="$output_dir/audio-evidence.json"
    private_regular_file "$manifest" 262144 || die "self-test manifest is not private"
    validation=$("$PYTHON_BIN" "$VALIDATOR" validate "$manifest") ||
        die "self-test manifest was not accepted by the validator"
    printf '%s' "$validation" | "$PYTHON_BIN" -c '
import json, sys
r = json.load(sys.stdin)
assert r["status"] == "validated"
assert r["claims"]["scope"] == "digital-pcm-path-only"
assert r["claims"]["physical_audibility"] == "operator-confirmation-required"
assert r["claims"]["production_audio_acceptance"] == "not-proven-by-this-validator"
assert [c["expected_tone_hz"] for c in r["captures"]] == [523, 719, 977, 1301]
assert all(c["channels"] == 2 for c in r["captures"])
'

    "$PYTHON_BIN" - "$events" <<'PY'
from pathlib import Path
import sys

events = Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()
if "forbidden:qga-guest-exec" in events:
    raise SystemExit("collector attempted guest audio through QGA guest-exec")
qga = [value for value in events if value.startswith("qga:")]
if not qga or any(not (value == "qga:ping" or value.startswith("qga:read:")) for value in qga):
    raise SystemExit("QGA use exceeded ping and immutable provenance reads")
first_warning = events.index("warning:1")
second_warning = events.index("warning:2")
first_mutation = next(index for index, value in enumerate(events) if value.startswith("mutate:"))
reconnect = events.index("mutate:reconnect")
after = next(index for index, value in enumerate(events) if value.startswith("mutate:guest-playback-ready:after-recovery"))
if not (first_warning < first_mutation < second_warning < reconnect < after):
    raise SystemExit("warning/reconnect/recovery event ordering is invalid")
if not any("guest-capture-ready:before-recovery:719" in value for value in events):
    raise SystemExit("before-recovery getUserMedia control was not exercised")
if not any("guest-capture-ready:after-recovery:1301" in value for value in events):
    raise SystemExit("after-recovery getUserMedia control was not exercised")
PY

    # The capture source-output must not be accepted before the Browser hook
    # opens getUserMedia. Once active, it must bind to the exact QEMU PID.
    rm -f "$test_state/capture-active"
    if find_qemu_stream capture 4242 >/dev/null 2>&1; then
        die "self-test resolved a QEMU capture stream before getUserMedia opened"
    fi
    : >"$test_state/capture-active"
    [[ $(find_qemu_stream capture 4242) == $'55\t12' ]] ||
        die "self-test did not resolve the exact QEMU capture source-output"
    rm -f "$test_state/capture-active"

    # Ambiguous Browser playback streams are rejected rather than guessed.
    : >"$test_state/playback-active"
    : >"$test_state/ambiguous-playback"
    if find_qemu_stream playback 4242 >/dev/null 2>&1; then
        die "self-test accepted ambiguous Browser QEMU playback streams"
    fi
    rm -f "$test_state/playback-active" "$test_state/ambiguous-playback"

    # A failed mandatory warning must stop before every guest/audio mutation.
    marker=$(wc -l <"$events")
    : >"$test_state/warning-fail"
    output_dir="$evidence_parent/warning-failure"
    stage_dir=""
    host_runtime=""
    finalized=0
    if (collect >/dev/null 2>&1); then
        die "self-test collected evidence after a failed mandatory warning"
    fi
    [[ ! -e $output_dir ]] || die "self-test published evidence after warning failure"
    mutation_after_marker=$(tail -n "+$((marker + 1))" "$events" | grep -c '^mutate:' || true)
    [[ $mutation_after_marker -eq 0 ]] || die "self-test mutated live state after warning failure"
    rm -f "$test_state/warning-fail"

    rm -rf -- "$fixture"
    trap - EXIT HUP INT TERM
    echo "collect-browser-vm-live-audio: self-test passed (full four-tone flow + 5 fail-closed controls)"
}

main() {
    if [[ ${1:-} == --self-test ]]; then
        (($# == 1)) || { usage; exit 2; }
        self_test
        return
    fi

    while (($#)); do
        case "$1" in
            --output) (($# >= 2)) || { usage; exit 2; }; output_dir=$2; shift 2 ;;
            --source-commit) (($# >= 2)) || { usage; exit 2; }; source_commit=$2; shift 2 ;;
            --image-digest) (($# >= 2)) || { usage; exit 2; }; image_digest=$2; shift 2 ;;
            --transport) (($# >= 2)) || { usage; exit 2; }; transport=$2; shift 2 ;;
            --guest-probe-hook) (($# >= 2)) || { usage; exit 2; }; guest_probe_hook=$2; shift 2 ;;
            --reconnect-hook) (($# >= 2)) || { usage; exit 2; }; reconnect_hook=$2; shift 2 ;;
            --domain) (($# >= 2)) || { usage; exit 2; }; domain=$2; shift 2 ;;
            --seat-user) (($# >= 2)) || { usage; exit 2; }; seat_user=$2; shift 2 ;;
            *) usage; exit 2 ;;
        esac
    done

    [[ -n $output_dir && -n $source_commit && -n $image_digest && -n $transport &&
       -n $guest_probe_hook && -n $reconnect_hook ]] || { usage; exit 2; }
    if [[ -x /usr/libexec/mackesd/seat-update-warning && ! -L /usr/libexec/mackesd/seat-update-warning ]]; then
        warning_helper=/usr/libexec/mackesd/seat-update-warning
    else
        warning_helper="$SCRIPT_DIR/seat-update-warning.sh"
    fi
    qemu_pid_file="/run/libvirt/qemu/${domain}.pid"
    collect
}

main "$@"
