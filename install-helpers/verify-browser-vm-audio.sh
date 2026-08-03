#!/usr/bin/env bash
# Read-only Browser VM audio wiring evidence.
#
# This checks the declarative QEMU/libvirt boundary only: one virtio sound card,
# one Browser-owned Pulse backend on the tracked localhost endpoint, and exactly
# one guest playback and capture endpoint.
# It deliberately does not claim that audio is audible, captured, or recovered;
# those require a booted guest and live media traffic.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SESSION_RUNTIME="$ROOT/packaging/browser-vm/mcnf-browser-vm-session.sh"
GUEST_RUNTIME="$ROOT/packaging/browser-vm/mcnf-browser-vm-runtime.sh"

usage() {
    printf '%s\n' \
        'usage: verify-browser-vm-audio.sh --xml FILE|- | --domain NAME' \
        '       verify-browser-vm-audio.sh --self-test' >&2
}

check_xml() {
    local label=$1
    python3 -c '
import json
import sys
import xml.etree.ElementTree as ET

label = sys.argv[1]
raw = sys.stdin.buffer.read(1_048_577)
if len(raw) > 1_048_576:
    print(json.dumps({"status": "failed", "source": label, "reason": "XML exceeds 1 MiB"}))
    raise SystemExit(1)
try:
    root = ET.fromstring(raw)
except ET.ParseError as exc:
    print(json.dumps({"status": "failed", "source": label, "reason": f"invalid domain XML: {exc}"}))
    raise SystemExit(1)

device_blocks = root.findall("devices") if root.tag == "domain" else []
devices = device_blocks[0] if len(device_blocks) == 1 else None
sounds = devices.findall("sound") if devices is not None else []
audios = devices.findall("audio") if devices is not None else []
sound = sounds[0] if len(sounds) == 1 else None
audio = audios[0] if len(audios) == 1 else None
inputs = audio.findall("input") if audio is not None else []
outputs = audio.findall("output") if audio is not None else []
input_ = inputs[0] if len(inputs) == 1 else None
output = outputs[0] if len(outputs) == 1 else None
model = sound.get("model") if sound is not None else None
audio_id = audio.get("id") if audio is not None else None
backend = audio.get("type") if audio is not None else None
server_name = audio.get("serverName") if audio is not None else None

result = {
    "status": "ready",
    "source": label,
    "sound_device_count": len(sounds),
    "audio_backend_count": len(audios),
    "sound_model": model,
    "audio_id": audio_id,
    "backend": backend,
    "server_name": server_name,
    "playback_endpoint_count": len(outputs),
    "capture_endpoint_count": len(inputs),
}
reasons = []
if root.tag != "domain":
    reasons.append("root element is not domain")
if len(device_blocks) != 1:
    reasons.append("domain must contain exactly one devices block")
if len(sounds) != 1:
    reasons.append("domain must contain exactly one sound device")
if model != "virtio":
    reasons.append("virtio sound device is missing")
if len(audios) != 1:
    reasons.append("domain must contain exactly one audio backend")
if audio_id != "1":
    reasons.append("Browser audio backend id is not 1")
if backend != "pulseaudio":
    reasons.append("Browser audio backend is not PulseAudio")
if server_name != "tcp:127.0.0.1:4713":
    reasons.append("Browser audio backend is not the tracked localhost endpoint")
if len(outputs) != 1:
    reasons.append("domain must contain exactly one guest playback endpoint")
elif output.get("name") != "browser-vm" or output.get("streamName") != "MCNF-Browser-VM":
    reasons.append("guest playback endpoint identity is not Browser-owned")
if len(inputs) != 1:
    reasons.append("domain must contain exactly one guest capture endpoint")
elif input_.get("name") != "browser-vm-capture":
    reasons.append("guest capture endpoint identity is not Browser-owned")
if reasons:
    result["status"] = "failed"
    result["reason"] = "; ".join(reasons)
print(json.dumps(result, sort_keys=True))
raise SystemExit(1 if reasons else 0)
    ' "$label"
}

self_test() {
    local valid="<domain><devices><sound model='virtio'/><audio id='1' type='pulseaudio' serverName='tcp:127.0.0.1:4713'><input name='browser-vm-capture'/><output name='browser-vm' streamName='MCNF-Browser-VM'/></audio></devices></domain>"
    local missing_capture="<domain><devices><sound model='virtio'/><audio id='1' type='pulseaudio' serverName='tcp:127.0.0.1:4713'><output name='browser-vm' streamName='MCNF-Browser-VM'/></audio></devices></domain>"
    local broad_listener="<domain><devices><sound model='virtio'/><audio id='1' type='pulseaudio' serverName='tcp:0.0.0.0:4713'><input name='browser-vm-capture'/><output name='browser-vm' streamName='MCNF-Browser-VM'/></audio></devices></domain>"
    local duplicate_backend="<domain><devices><sound model='virtio'/><audio id='1' type='pulseaudio' serverName='tcp:127.0.0.1:4713'><input name='browser-vm-capture'/><output name='browser-vm' streamName='MCNF-Browser-VM'/></audio><audio id='2' type='pipewire'><input/><output/></audio></devices></domain>"
    printf '%s' "$valid" | check_xml self-test-valid >/dev/null
    if printf '%s' "$missing_capture" | check_xml self-test-missing-capture >/dev/null 2>&1; then
        echo 'verify-browser-vm-audio: self-test accepted missing capture endpoint' >&2
        exit 1
    fi
    if printf '%s' "$broad_listener" | check_xml self-test-broad-listener >/dev/null 2>&1; then
        echo 'verify-browser-vm-audio: self-test accepted a non-loopback Pulse endpoint' >&2
        exit 1
    fi
    if printf '%s' "$duplicate_backend" | check_xml self-test-duplicate-backend >/dev/null 2>&1; then
        echo 'verify-browser-vm-audio: self-test accepted duplicate audio backends' >&2
        exit 1
    fi
    runtime_evidence_order_self_test
    session_startup_self_test
    echo 'verify-browser-vm-audio: self-test passed'
}

runtime_evidence_order_self_test() {
    [[ -f "$GUEST_RUNTIME" && ! -L "$GUEST_RUNTIME" ]] || {
        echo "verify-browser-vm-audio: guest runtime is missing or symlinked: $GUEST_RUNTIME" >&2
        exit 1
    }

    local invalidation_line admission_line unavailable_line
    invalidation_line=$(grep -n -m1 -F 'rm -f "$runtime_evidence" "$media_evidence" "$gpu_probe" "$media_probe"' "$GUEST_RUNTIME" | cut -d: -f1)
    admission_line=$(grep -n -m1 -F '/usr/local/libexec/mcnf-browser-vm-validate' "$GUEST_RUNTIME" | cut -d: -f1)
    unavailable_line=$(grep -n -m1 -F 'write_runtime_evidence unavailable unavailable 0 0' "$GUEST_RUNTIME" | cut -d: -f1)
    if [[ -z $invalidation_line || -z $admission_line || -z $unavailable_line ]] ||
        (( invalidation_line >= admission_line || admission_line >= unavailable_line )); then
        echo 'verify-browser-vm-audio: stale evidence is not invalidated before runtime admission' >&2
        exit 1
    fi
}

session_startup_self_test() {
    [[ -x "$SESSION_RUNTIME" ]] || {
        echo "verify-browser-vm-audio: session runtime is not executable: $SESSION_RUNTIME" >&2
        exit 1
    }

    local fixture mock_bin state events runtime_dir
    fixture=$(mktemp -d)
    self_test_fixture=$fixture
    trap 'rm -rf "$self_test_fixture"' EXIT
    mock_bin=$fixture/bin
    state=$fixture/state
    events=$fixture/events
    runtime_dir=$fixture/runtime
    mkdir -p "$mock_bin" "$state" "$runtime_dir"
    : >"$events"

    cat >"$mock_bin/mock-browser-audio" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

tool=${0##*/}
state=${MCNF_TEST_AUDIO_STATE:?}
events=${MCNF_TEST_AUDIO_EVENTS:?}

record() {
    printf '%s\n' "$1" >>"$events"
}

graph_ready() {
    [[ -f "$state/ready" && ! -f "$state/force-not-ready" ]]
}

case "$tool" in
    systemctl)
        if [[ "${1:-}" == --user && "${2:-}" == show-environment ]]; then
            record 'systemctl:show-environment'
            [[ ! -f "$state/no-user-manager" ]]
            exit
        fi
        if [[ "${1:-}" == --user && "${2:-}" == start ]]; then
            record 'systemctl:start-audio-services'
            [[ ! -f "$state/systemd-start-fails" ]] || exit 1
            [[ -f "$state/force-not-ready" ]] || : >"$state/ready"
            exit 0
        fi
        exit 2
        ;;
    pw-cli)
        if graph_ready; then
            record 'ready:pw-cli'
            exit 0
        fi
        record 'not-ready:pw-cli'
        exit 1
        ;;
    wpctl)
        if graph_ready; then
            record 'ready:wpctl'
            exit 0
        fi
        record 'not-ready:wpctl'
        exit 1
        ;;
    pactl)
        if ! graph_ready; then
            record "not-ready:pactl:$*"
            exit 1
        fi
        case "$*" in
            info)
                record 'ready:pactl-info'
                ;;
            'list short sinks')
                record 'ready:pactl-sinks'
                printf '1\tvirtio_output\tPipeWire\ts16le 2ch 48000Hz\tRUNNING\n'
                ;;
            'list short sources')
                record 'ready:pactl-sources'
                if [[ -f "$state/monitor-only" ]]; then
                    printf '2\tvirtio_output.monitor\tPipeWire\ts16le 2ch 48000Hz\tIDLE\n'
                else
                    printf '2\tvirtio_input\tPipeWire\ts16le 2ch 48000Hz\tIDLE\n'
                fi
                ;;
            *) exit 2 ;;
        esac
        ;;
    pipewire|pipewire-pulse|wireplumber)
        record "spawn:$tool"
        : >"$state/$tool"
        if [[ -f "$state/pipewire" && -f "$state/pipewire-pulse" && -f "$state/wireplumber" && ! -f "$state/force-not-ready" ]]; then
            : >"$state/ready"
        fi
        trap 'exit 0' HUP INT TERM
        while :; do
            /usr/bin/sleep 1
        done
        ;;
    pgrep)
        record "pgrep:$*"
        [[ -f "$state/existing-audio-daemon" ]]
        ;;
    probe-command)
        [[ "${MCNF_BROWSER_VM_AUDIO_READY:-0}" == 1 ]]
        record 'command:runtime-probes-and-chromium'
        : >"$state/command-invoked"
        ;;
    *)
        exit 127
        ;;
esac
EOF
    chmod 0755 "$mock_bin/mock-browser-audio"
    local tool
    for tool in systemctl pw-cli wpctl pactl pgrep pipewire pipewire-pulse wireplumber probe-command; do
        ln -s mock-browser-audio "$mock_bin/$tool"
    done

    run_session_fixture() {
        PATH="$mock_bin:$PATH" \
            XDG_RUNTIME_DIR="$runtime_dir" \
            MCNF_BROWSER_VM_TEST_MODE=1 \
            MCNF_BROWSER_VM_AUDIO_READY_ATTEMPTS=50 \
            MCNF_BROWSER_VM_AUDIO_READY_DELAY=0.01 \
            MCNF_TEST_AUDIO_STATE="$state" \
            MCNF_TEST_AUDIO_EVENTS="$events" \
            "$SESSION_RUNTIME" "$mock_bin/probe-command"
    }

    event_line() {
        local event=$1
        grep -n -m1 -F -x "$event" "$events" | cut -d: -f1
    }

    assert_before() {
        local first=$1 second=$2 first_line second_line
        first_line=$(event_line "$first")
        second_line=$(event_line "$second")
        if (( first_line >= second_line )); then
            echo "verify-browser-vm-audio: event ordering violated: $first before $second" >&2
            exit 1
        fi
    }

    # systemd --user is preferred. The runtime command cannot run until the
    # core, policy manager, Pulse server, playback, and capture checks pass.
    run_session_fixture >/dev/null 2>&1
    assert_before 'systemctl:start-audio-services' 'ready:pw-cli'
    assert_before 'ready:pw-cli' 'ready:pactl-info'
    assert_before 'ready:pactl-info' 'ready:wpctl'
    assert_before 'ready:wpctl' 'ready:pactl-sinks'
    assert_before 'ready:pactl-sinks' 'ready:pactl-sources'
    assert_before 'ready:pactl-sources' 'command:runtime-probes-and-chromium'
    if grep -q '^spawn:' "$events"; then
        echo 'verify-browser-vm-audio: systemd path spawned duplicate session daemons' >&2
        exit 1
    fi

    # A ready graph is reused without either systemd starts or manual daemons.
    : >"$events"
    run_session_fixture >/dev/null 2>&1
    grep -q -F -x 'command:runtime-probes-and-chromium' "$events"
    if grep -Eq '^(systemctl:|spawn:)' "$events"; then
        echo 'verify-browser-vm-audio: ready graph was started a second time' >&2
        exit 1
    fi

    # A user manager that starts but never becomes ready fails closed. The
    # runtime probe/Chromium command must never be invoked.
    rm -f "$state/ready" "$state/command-invoked"
    : >"$state/force-not-ready"
    : >"$events"
    if run_session_fixture >/dev/null 2>&1; then
        echo 'verify-browser-vm-audio: accepted an audio graph that never became ready' >&2
        exit 1
    fi
    if [[ -e "$state/command-invoked" ]] || grep -q '^command:' "$events"; then
        echo 'verify-browser-vm-audio: ran Browser probes after audio readiness failure' >&2
        exit 1
    fi

    # A playback monitor is not a real guest capture endpoint and must never
    # satisfy the wired/readiness contract by itself.
    rm -f "$state/force-not-ready" "$state/command-invoked"
    : >"$state/ready"
    : >"$state/monitor-only"
    : >"$events"
    if run_session_fixture >/dev/null 2>&1; then
        echo 'verify-browser-vm-audio: accepted a playback monitor as a capture endpoint' >&2
        exit 1
    fi
    if [[ -e "$state/command-invoked" ]] || grep -q '^command:' "$events"; then
        echo 'verify-browser-vm-audio: ran Browser probes with monitor-only capture state' >&2
        exit 1
    fi

    # A partial non-systemd graph fails closed instead of starting duplicate
    # long-lived daemons over it.
    rm -f "$state/monitor-only" "$state/ready" "$state/command-invoked"
    : >"$state/no-user-manager"
    : >"$state/existing-audio-daemon"
    : >"$events"
    if run_session_fixture >/dev/null 2>&1; then
        echo 'verify-browser-vm-audio: started over a partial existing audio graph' >&2
        exit 1
    fi
    if grep -Eq '^(spawn:|command:)' "$events"; then
        echo 'verify-browser-vm-audio: duplicate-safe fallback ran after detecting an existing daemon' >&2
        exit 1
    fi

    # The direct SPICE fallback has no user manager. It owns one locked daemon
    # trio for the session and tears that trio down when the command returns.
    rm -f "$state/existing-audio-daemon"
    : >"$events"
    run_session_fixture >/dev/null 2>&1
    for tool in pipewire pipewire-pulse wireplumber; do
        if [[ $(grep -c -F -x "spawn:$tool" "$events") -ne 1 ]]; then
            echo "verify-browser-vm-audio: session fallback did not start exactly one $tool" >&2
            exit 1
        fi
    done
    assert_before 'spawn:wireplumber' 'command:runtime-probes-and-chromium'

    rm -rf "$fixture"
    trap - EXIT
}

if [[ "${1:-}" == "--self-test" ]]; then
    [[ $# -eq 1 ]] || { usage; exit 2; }
    self_test
    exit 0
fi

[[ $# -eq 2 ]] || { usage; exit 2; }
case "$1" in
    --xml)
        if [[ "$2" == "-" ]]; then
            check_xml stdin
        else
            [[ -f "$2" && ! -L "$2" ]] || { echo "verify-browser-vm-audio: XML file is missing or symlinked: $2" >&2; exit 2; }
            # check_xml only reads stdin; its label argument is not an output path.
            # shellcheck disable=SC2094
            check_xml "$2" < "$2"
        fi
        ;;
    --domain)
        command -v virsh >/dev/null 2>&1 || { echo 'verify-browser-vm-audio: virsh is required for --domain' >&2; exit 2; }
        virsh dumpxml "$2" | check_xml "virsh:$2"
        ;;
    *)
        usage
        exit 2
        ;;
esac
