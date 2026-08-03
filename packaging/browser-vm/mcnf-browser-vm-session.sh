#!/bin/sh
# Start the guest user's media services once, wait for the complete audio graph,
# and only then enter the Browser VM runtime stage that probes and launches the
# compositor. No host PipeWire socket is imported.
set -eu

[ "$#" -gt 0 ] || {
    echo 'FATAL: Browser VM session needs a runtime command' >&2
    exit 2
}

log() {
    printf 'mcnf-browser-vm-session: %s\n' "$*" >&2
}

ready_attempts=50
ready_delay=0.1
if [ "${MCNF_BROWSER_VM_TEST_MODE:-0}" = 1 ]; then
    ready_attempts=${MCNF_BROWSER_VM_AUDIO_READY_ATTEMPTS:-3}
    ready_delay=${MCNF_BROWSER_VM_AUDIO_READY_DELAY:-0}
    case "$ready_attempts" in
        ''|*[!0-9]*|0) echo 'FATAL: invalid test readiness attempt count' >&2; exit 2 ;;
    esac
    case "$ready_delay" in
        0|0.[0-9]|0.[0-9][0-9]) ;;
        *) echo 'FATAL: invalid test readiness delay' >&2; exit 2 ;;
    esac
fi

audio_ready() {
    command -v pw-cli >/dev/null 2>&1 &&
        command -v pactl >/dev/null 2>&1 &&
        command -v wpctl >/dev/null 2>&1 &&
        pw-cli info 0 >/dev/null 2>&1 &&
        pactl info >/dev/null 2>&1 &&
        wpctl status >/dev/null 2>&1 &&
        pactl list short sinks 2>/dev/null |
            awk 'NF { found=1 } END { exit(found ? 0 : 1) }' &&
        pactl list short sources 2>/dev/null |
            awk 'NF && $2 !~ /[.]monitor$/ { found=1 } END { exit(found ? 0 : 1) }'
}

wait_for_audio() {
    attempt=1
    while [ "$attempt" -le "$ready_attempts" ]; do
        if audio_ready; then
            return 0
        fi
        if [ "$attempt" -lt "$ready_attempts" ]; then
            sleep "$ready_delay"
        fi
        attempt=$((attempt + 1))
    done
    return 1
}

manual_owner=0
pipewire_pid=
pipewire_pulse_pid=
wireplumber_pid=
cleanup() {
    if [ "$manual_owner" -eq 1 ]; then
        kill "$wireplumber_pid" "$pipewire_pulse_pid" "$pipewire_pid" 2>/dev/null || true
        wait "$wireplumber_pid" "$pipewire_pulse_pid" "$pipewire_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

start_systemd_user_services() {
    command -v systemctl >/dev/null 2>&1 || return 1
    command -v timeout >/dev/null 2>&1 || return 1
    timeout 3 systemctl --user show-environment >/dev/null 2>&1 || return 1
    if ! timeout 8 systemctl --user start \
        pipewire.service pipewire-pulse.service wireplumber.service; then
        return 2
    fi
    return 0
}

start_session_owned_services() {
    command -v flock >/dev/null 2>&1 || return 1
    command -v pgrep >/dev/null 2>&1 || return 1
    for daemon in pipewire pipewire-pulse wireplumber; do
        command -v "$daemon" >/dev/null 2>&1 || return 1
    done

    runtime_dir=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
    [ -d "$runtime_dir" ] || return 1
    lock_file=$runtime_dir/mcnf-browser-audio.lock
    exec 9>"$lock_file"
    # A second manually supervised session must not create a second graph.
    # Authenticated xrdp sessions normally use the systemd --user path above;
    # this exclusive fallback is for the direct SPICE system session.
    flock -n 9 || return 1

    user_id=$(id -u)
    for daemon in pipewire pipewire-pulse wireplumber; do
        # A partial graph is not safe to reuse, but starting over it would
        # create duplicate long-lived user daemons. Fail closed instead.
        if pgrep -u "$user_id" -x "$daemon" >/dev/null 2>&1; then
            return 1
        fi
    done

    pipewire 9>&- &
    pipewire_pid=$!
    pipewire-pulse 9>&- &
    pipewire_pulse_pid=$!
    wireplumber 9>&- &
    wireplumber_pid=$!
    manual_owner=1
    return 0
}

if audio_ready; then
    log 'reusing the ready per-user PipeWire audio graph'
else
    systemd_status=0
    start_systemd_user_services || systemd_status=$?
    case "$systemd_status" in
        0)
            log 'started PipeWire, PipeWire-Pulse, and WirePlumber as systemd user services'
            ;;
        1)
            if ! start_session_owned_services; then
                log 'audio services are unavailable and no duplicate-safe fallback can start'
                exit 70
            fi
            log 'started one session-owned PipeWire audio graph'
            ;;
        *)
            log 'the systemd user manager refused the required audio services'
            exit 70
            ;;
    esac

    if ! wait_for_audio; then
        log 'audio graph did not become ready before the bounded deadline'
        exit 70
    fi
fi

# The command is the runtime's private --audio-ready stage. Exporting the
# marker does not create trust: that stage independently re-probes PipeWire,
# WirePlumber, Pulse compatibility, and endpoint presence before Chromium.
MCNF_BROWSER_VM_AUDIO_READY=1
export MCNF_BROWSER_VM_AUDIO_READY
log 'audio graph ready; entering Browser runtime probes and compositor'
"$@"
