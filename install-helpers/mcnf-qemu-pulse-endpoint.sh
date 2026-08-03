#!/usr/bin/env bash
# Maintain the Browser VM's localhost-only QEMU audio bridge inside the real
# seat user's PipeWire-Pulse graph. The QEMU/libvirt contract connects to
# tcp:127.0.0.1:4713 and cannot consume the user's private native socket.
set -euo pipefail

readonly ENDPOINT_ADDRESS="127.0.0.1"
readonly ENDPOINT_PORT="4713"
readonly ENDPOINT_MODULE="module-native-protocol-tcp"
readonly COMMAND_TIMEOUT_SECONDS="2"
readonly GRAPH_READY_ATTEMPTS="10"
readonly ENDPOINT_READY_ATTEMPTS="12"
readonly HEALTH_INTERVAL_SECONDS="5"
readonly MAX_TRANSIENT_HEALTH_FAILURES="3"

owned_module_id=""
health_reason="unavailable"

usage() {
    printf '%s\n' \
        "usage: $0 --run" \
        "       $0 --health" \
        "       $0 --self-test" >&2
}

log() {
    printf 'mcnf-qemu-pulse-endpoint: %s\n' "$*" >&2
}

die() {
    log "$*"
    exit 1
}

bounded() {
    timeout --signal=KILL "${COMMAND_TIMEOUT_SECONDS}s" "$@"
}

module_args_are_exact() {
    local args=$1
    local token
    local saw_port=0
    local saw_listen=0
    local saw_anonymous=0
    local -a tokens=()

    read -r -a tokens <<<"$args"
    [[ ${#tokens[@]} -eq 3 ]] || return 1

    for token in "${tokens[@]}"; do
        case "$token" in
            "port=${ENDPOINT_PORT}")
                ((saw_port += 1))
                ;;
            "listen=${ENDPOINT_ADDRESS}")
                ((saw_listen += 1))
                ;;
            "auth-anonymous=1")
                ((saw_anonymous += 1))
                ;;
            *)
                return 1
                ;;
        esac
    done

    [[ $saw_port -eq 1 && $saw_listen -eq 1 && $saw_anonymous -eq 1 ]]
}

# Print the one exact endpoint module ID. Return 3 when no module can own the
# endpoint and 1 when a default-port, duplicate, or malformed candidate exists.
find_endpoint_module() {
    local listing=$1
    local module_id module_name module_args _ignored token
    local candidate_count=0
    local exact_count=0
    local exact_id=""
    local has_explicit_port
    local targets_endpoint

    while IFS=$'\t' read -r module_id module_name module_args _ignored; do
        [[ -n ${module_name:-} ]] || continue
        [[ $module_name == "$ENDPOINT_MODULE" ]] || continue

        has_explicit_port=0
        targets_endpoint=0
        for token in $module_args; do
            case "$token" in
                port=*)
                    has_explicit_port=1
                    [[ $token == "port=${ENDPOINT_PORT}" ]] && targets_endpoint=1
                    ;;
            esac
        done

        # Pulse's native TCP module defaults to 4713. A missing port is thus an
        # endpoint candidate and must never be ignored as an unrelated module.
        if [[ $has_explicit_port -eq 0 || $targets_endpoint -eq 1 ]]; then
            ((candidate_count += 1))
            if [[ $module_id =~ ^[0-9]+$ ]] && module_args_are_exact "$module_args"; then
                ((exact_count += 1))
                exact_id=$module_id
            fi
        fi
    done <<<"$listing"

    if [[ $candidate_count -eq 0 ]]; then
        return 3
    fi
    if [[ $candidate_count -eq 1 && $exact_count -eq 1 ]]; then
        printf '%s\n' "$exact_id"
        return 0
    fi
    return 1
}

one_numeric_pid() {
    local listing=$1
    local pid selected_pid=""
    local pid_count=0

    while IFS= read -r pid; do
        [[ -n $pid ]] || continue
        [[ $pid =~ ^[0-9]+$ ]] || return 1
        ((pid_count += 1))
        selected_pid=$pid
    done <<<"$listing"

    [[ $pid_count -eq 1 ]] || return 1
    printf '%s\n' "$selected_pid"
}

# Print the one PipeWire-Pulse PID owned by this seat user. Return 3 when it is
# absent and 1 when the graph is ambiguous. Read /proc directly so the endpoint
# helper does not add an undeclared procps-ng dependency to the host package.
seat_pipewire_pulse_pid() {
    local seat_uid process_dir process_name owner pid
    local listing=""

    seat_uid="$(id -u)"
    for process_dir in /proc/[0-9]*; do
        [[ -d $process_dir ]] || continue
        IFS= read -r process_name <"$process_dir/comm" 2>/dev/null || continue
        [[ $process_name == pipewire-pulse ]] || continue
        owner="$(stat -Lc '%u' "$process_dir" 2>/dev/null)" || continue
        [[ $owner == "$seat_uid" ]] || continue
        pid=${process_dir##*/}
        if [[ -n $listing ]]; then
            listing+=$'\n'
        fi
        listing+=$pid
    done

    [[ -n $listing ]] || return 3
    one_numeric_pid "$listing"
}

listener_listing_is_exact() {
    local listing=$1
    local expected_pid=$2
    local line state _recv_q _send_q local_address _peer_address owner
    local expected_owner
    local listener_count=0

    [[ $expected_pid =~ ^[0-9]+$ ]] || return 1
    expected_owner="users:((\"pipewire-pulse\",pid=${expected_pid},fd="
    while IFS= read -r line; do
        [[ -n $line ]] || continue
        read -r state _recv_q _send_q local_address _peer_address owner <<<"$line"
        [[ $state == "LISTEN" ]] || return 1
        [[ $local_address == "${ENDPOINT_ADDRESS}:${ENDPOINT_PORT}" ]] || return 1
        # A same-named server from another account must not make this user
        # report healthy. Bind the listener receipt to this graph's sole PID.
        [[ ${owner:-} == *"$expected_owner"* ]] || return 1
        ((listener_count += 1))
    done <<<"$listing"

    [[ $listener_count -eq 1 ]]
}

listing_has_content() {
    local listing=$1
    [[ -n ${listing//[$' \t\r\n']/} ]]
}

module_listing() {
    bounded pactl list modules short 2>/dev/null
}

listener_listing() {
    bounded ss -H -ltnp "sport = :${ENDPOINT_PORT}" 2>/dev/null
}

# Return 0 for healthy, 1 for transiently unavailable, and 2 for an unsafe or
# ambiguous endpoint. Never print the module list: other modules may carry
# operator-specific paths or identifiers that do not belong in service logs.
probe_endpoint() {
    local modules module_id module_rc listeners pulse_pid pulse_pid_rc

    health_reason="PipeWire-Pulse module query failed"
    modules="$(module_listing)" || return 1

    if module_id="$(find_endpoint_module "$modules")"; then
        :
    else
        module_rc=$?
        if [[ $module_rc -eq 3 ]]; then
            health_reason="endpoint module is absent"
            return 1
        fi
        health_reason="endpoint module is ambiguous or not loopback-only"
        return 2
    fi

    health_reason="listener query failed"
    listeners="$(listener_listing)" || return 1
    if pulse_pid="$(seat_pipewire_pulse_pid)"; then
        :
    else
        pulse_pid_rc=$?
        case "$pulse_pid_rc" in
            3)
                health_reason="seat PipeWire-Pulse process is absent"
                return 1
                ;;
            *)
                health_reason="seat PipeWire-Pulse process is ambiguous"
                return 2
                ;;
        esac
    fi
    if listener_listing_is_exact "$listeners" "$pulse_pid"; then
        health_reason="healthy"
        printf '%s\n' "$module_id"
        return 0
    fi
    if listing_has_content "$listeners"; then
        health_reason="port 4713 has a non-loopback, duplicate, or non-PipeWire listener"
        return 2
    fi
    health_reason="endpoint listener is not ready"
    return 1
}

assert_runtime_context() {
    local seat_uid expected_runtime expected_server owner

    seat_uid="$(id -u)"
    [[ $seat_uid =~ ^[0-9]+$ && $seat_uid -ne 0 ]] || \
        die "must run in the non-root seat user's systemd manager"

    expected_runtime="/run/user/${seat_uid}"
    expected_server="unix:${expected_runtime}/pulse/native"
    if [[ -n ${XDG_RUNTIME_DIR:-} && $XDG_RUNTIME_DIR != "$expected_runtime" ]]; then
        die "XDG_RUNTIME_DIR does not identify the invoking seat user"
    fi
    if [[ -n ${PULSE_SERVER:-} && $PULSE_SERVER != "$expected_server" ]]; then
        die "PULSE_SERVER must identify the invoking seat user's native socket"
    fi
    export XDG_RUNTIME_DIR="$expected_runtime"
    export PULSE_SERVER="$expected_server"

    [[ -d $expected_runtime && ! -L $expected_runtime ]] || \
        die "seat runtime directory is missing or symlinked"
    owner="$(stat -Lc '%u' "$expected_runtime")"
    [[ $owner == "$seat_uid" ]] || die "seat runtime directory has the wrong owner"
}

wait_for_graph() {
    local seat_uid pulse_dir pulse_socket owner attempt

    seat_uid="$(id -u)"
    pulse_dir="${XDG_RUNTIME_DIR}/pulse"
    pulse_socket="${pulse_dir}/native"

    for ((attempt = 1; attempt <= GRAPH_READY_ATTEMPTS; attempt += 1)); do
        [[ ! -L $pulse_dir && ! -L $pulse_socket ]] || \
            die "PipeWire-Pulse runtime path is symlinked"
        if [[ -e $pulse_dir && ! -d $pulse_dir ]]; then
            die "PipeWire-Pulse runtime path is not a directory"
        fi
        if [[ -d $pulse_dir ]]; then
            owner="$(stat -Lc '%u' "$pulse_dir")"
            [[ $owner == "$seat_uid" ]] || die "PipeWire-Pulse directory has the wrong owner"
        fi
        if [[ -e $pulse_socket && ! -S $pulse_socket ]]; then
            die "PipeWire-Pulse native path is not a socket"
        fi
        if [[ -S $pulse_socket ]]; then
            owner="$(stat -Lc '%u' "$pulse_socket")"
            [[ $owner == "$seat_uid" ]] || die "PipeWire-Pulse socket has the wrong owner"
            if bounded pactl info >/dev/null 2>&1; then
                return 0
            fi
        fi
        [[ $attempt -eq $GRAPH_READY_ATTEMPTS ]] || sleep 0.5
    done

    die "seat PipeWire-Pulse graph did not become ready within the bounded window"
}

acquire_endpoint() {
    local modules module_id module_rc listeners attempt probe_rc

    modules="$(module_listing)" || die "cannot inspect PipeWire-Pulse modules"
    if module_id="$(find_endpoint_module "$modules")"; then
        log "adopting the existing exact localhost endpoint"
    else
        module_rc=$?
        [[ $module_rc -eq 3 ]] || \
            die "refusing an ambiguous or non-loopback native TCP module on port 4713"

        listeners="$(listener_listing)" || die "cannot inspect TCP listeners"
        listing_has_content "$listeners" && \
            die "refusing to load over an existing listener on port 4713"

        module_id="$(bounded pactl load-module "$ENDPOINT_MODULE" \
            "port=${ENDPOINT_PORT}" \
            "listen=${ENDPOINT_ADDRESS}" \
            "auth-anonymous=1" 2>/dev/null)" || \
            die "PipeWire-Pulse refused the localhost endpoint"
        [[ $module_id =~ ^[0-9]+$ ]] || die "PipeWire-Pulse returned an invalid module ID"
        owned_module_id=$module_id
        log "loaded localhost-only endpoint module"
    fi

    for ((attempt = 1; attempt <= ENDPOINT_READY_ATTEMPTS; attempt += 1)); do
        if probe_endpoint >/dev/null; then
            return 0
        else
            probe_rc=$?
            [[ $probe_rc -ne 2 ]] || die "$health_reason"
        fi
        [[ $attempt -eq $ENDPOINT_READY_ATTEMPTS ]] || sleep 0.25
    done

    die "endpoint did not become healthy within the bounded readiness window"
}

notify_systemd() {
    [[ -n ${NOTIFY_SOCKET:-} ]] || return 0
    bounded systemd-notify "$@" >/dev/null 2>&1
}

cleanup() {
    local rc=$?
    trap - EXIT HUP INT TERM
    if [[ -n $owned_module_id ]]; then
        if bounded pactl unload-module "$owned_module_id" >/dev/null 2>&1; then
            log "unloaded owned endpoint module"
        else
            log "owned endpoint module was already unavailable during cleanup"
        fi
    fi
    exit "$rc"
}

stop_cleanly() {
    notify_systemd --stopping --status="Stopping localhost QEMU audio endpoint" || true
    exit 0
}

run_endpoint() {
    local failures=0
    local probe_rc

    for command in timeout pactl ss stat systemd-notify; do
        command -v "$command" >/dev/null 2>&1 || die "required command is missing: $command"
    done
    assert_runtime_context
    trap cleanup EXIT
    trap stop_cleanly HUP INT TERM

    wait_for_graph
    acquire_endpoint
    notify_systemd --ready --status="QEMU audio endpoint healthy on 127.0.0.1:4713" || \
        die "systemd readiness notification failed"

    while sleep "$HEALTH_INTERVAL_SECONDS"; do
        if probe_endpoint >/dev/null; then
            if notify_systemd --status="QEMU audio endpoint healthy on 127.0.0.1:4713" WATCHDOG=1; then
                failures=0
                continue
            fi
            health_reason="systemd watchdog notification failed"
            probe_rc=1
        else
            probe_rc=$?
        fi

        if [[ $probe_rc -eq 2 ]]; then
            die "$health_reason"
        fi
        ((failures += 1))
        log "transient health failure ${failures}/${MAX_TRANSIENT_HEALTH_FAILURES}: ${health_reason}"
        [[ $failures -lt $MAX_TRANSIENT_HEALTH_FAILURES ]] || \
            die "endpoint remained unhealthy for the bounded health window"
    done
}

health_check() {
    local probe_rc

    for command in timeout pactl ss stat; do
        command -v "$command" >/dev/null 2>&1 || die "required command is missing: $command"
    done
    assert_runtime_context
    if probe_endpoint >/dev/null; then
        printf 'mcnf-qemu-pulse-endpoint: healthy user=%s address=%s port=%s\n' \
            "$(id -un)" "$ENDPOINT_ADDRESS" "$ENDPOINT_PORT"
        return 0
    else
        probe_rc=$?
    fi
    log "unhealthy: $health_reason"
    return "$probe_rc"
}

self_test() {
    local good_modules good_listener module_id module_rc no_endpoint selected_pid
    local bad_default bad_any bad_duplicate_module
    local bad_listener_any bad_listener_v6 bad_listener_owner
    local bad_listener_other_user bad_listener_duplicate

    good_modules=$'23\tmodule-always-sink\tsink_name=other\t\n42\tmodule-native-protocol-tcp\tauth-anonymous=1 listen=127.0.0.1 port=4713\t'
    good_listener='LISTEN 0 32 127.0.0.1:4713 0.0.0.0:* users:(("pipewire-pulse",pid=123,fd=24))'
    module_id="$(find_endpoint_module "$good_modules")"
    [[ $module_id == "42" ]]
    selected_pid="$(one_numeric_pid $'123\n')"
    [[ $selected_pid == 123 ]]
    listener_listing_is_exact "$good_listener" "$selected_pid"
    listing_has_content "$good_listener"
    if listing_has_content $' \t\r\n'; then
        die "self-test treated whitespace as a listener"
    fi

    no_endpoint=$'23\tmodule-always-sink\tsink_name=other\t'
    if find_endpoint_module "$no_endpoint" >/dev/null 2>&1; then
        die "self-test found an endpoint in an unrelated module"
    else
        module_rc=$?
    fi
    [[ $module_rc -eq 3 ]] || die "self-test did not preserve the absent-module result"

    bad_default=$'7\tmodule-native-protocol-tcp\tauth-anonymous=1\t'
    bad_any=$'7\tmodule-native-protocol-tcp\tport=4713 listen=0.0.0.0 auth-anonymous=1\t'
    bad_duplicate_module=$'7\tmodule-native-protocol-tcp\tport=4713 listen=127.0.0.1 auth-anonymous=1\t\n8\tmodule-native-protocol-tcp\tport=4713 listen=127.0.0.1 auth-anonymous=1\t'
    if find_endpoint_module "$bad_default" >/dev/null 2>&1; then
        die "self-test accepted a default-port module"
    fi
    if find_endpoint_module "$bad_any" >/dev/null 2>&1; then
        die "self-test accepted an anonymous non-loopback module"
    fi
    if find_endpoint_module "$bad_duplicate_module" >/dev/null 2>&1; then
        die "self-test accepted duplicate endpoint modules"
    fi

    bad_listener_any='LISTEN 0 32 0.0.0.0:4713 0.0.0.0:* users:(("pipewire-pulse",pid=123,fd=24))'
    bad_listener_v6='LISTEN 0 32 [::]:4713 [::]:* users:(("pipewire-pulse",pid=123,fd=24))'
    bad_listener_owner='LISTEN 0 32 127.0.0.1:4713 0.0.0.0:* users:(("other-daemon",pid=123,fd=24))'
    bad_listener_other_user='LISTEN 0 32 127.0.0.1:4713 0.0.0.0:* users:(("pipewire-pulse",pid=124,fd=24))'
    bad_listener_duplicate="${good_listener}"$'\n'"${good_listener}"
    for bad_listener in \
        "$bad_listener_any" \
        "$bad_listener_v6" \
        "$bad_listener_owner" \
        "$bad_listener_other_user" \
        "$bad_listener_duplicate"; do
        if listener_listing_is_exact "$bad_listener" "$selected_pid"; then
            die "self-test accepted an unsafe listener fixture"
        fi
    done
    if one_numeric_pid $'123\n124\n' >/dev/null 2>&1; then
        die "self-test accepted multiple PipeWire-Pulse processes"
    fi

    [[ $GRAPH_READY_ATTEMPTS -le 10 ]]
    [[ $ENDPOINT_READY_ATTEMPTS -le 12 ]]
    [[ $MAX_TRANSIENT_HEALTH_FAILURES -eq 3 ]]
    printf '%s\n' "mcnf-qemu-pulse-endpoint: self-test passed"
}

case "${1:-}" in
    --run)
        [[ $# -eq 1 ]] || { usage; exit 2; }
        run_endpoint
        ;;
    --health)
        [[ $# -eq 1 ]] || { usage; exit 2; }
        health_check
        ;;
    --self-test)
        [[ $# -eq 1 ]] || { usage; exit 2; }
        self_test
        ;;
    *)
        usage
        exit 2
        ;;
esac
