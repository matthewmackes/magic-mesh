#!/bin/sh
# Probe the guest-owned App VM runtime before the launcher admits a connected
# application session. Every external probe is bounded; missing guest
# services produce an unavailable record and a non-zero exit, never a fallback.
set -eu
umask 077

input_root=${MCNF_APP_VM_INPUT_ROOT:-/etc/mackesd/app-vm}
hostname_file=${MCNF_APP_VM_HOSTNAME_FILE:-/etc/hostname}
contract_file=${MCNF_APP_VM_CONTRACT_FILE:-/usr/share/mcnf/app-vm/image-contract.json}
readiness_file=${MCNF_APP_VM_READINESS_FILE:-/usr/share/mcnf/app-vm/runtime-readiness}
evidence_file=${MCNF_APP_VM_PREFLIGHT_EVIDENCE:-/run/mcnf-app-vm/runtime-preflight.json}
probe_timeout=${MCNF_APP_VM_PROBE_TIMEOUT:-2}
max_identity_bytes=128

case "$probe_timeout" in
    ''|*[!0-9]*|0) probe_timeout=2 ;;
esac

read_identity() {
    file=$1
    value=
    if [ -f "$file" ]; then
        bytes=$(wc -c < "$file" 2>/dev/null || printf '%s\n' 129)
        case "$bytes" in
            ''|*[!0-9]*) bytes=129 ;;
        esac
        if [ "$bytes" -le "$max_identity_bytes" ] 2>/dev/null; then
            IFS= read -r value < "$file" || true
        fi
    fi
    case "$value" in
        \"*\")
            value=${value#\"}
            value=${value%\"}
            ;;
    esac
    # The launcher runs the full validator before this helper. Reapply the
    # safe character boundary here so standalone probe use cannot turn an
    # identity into JSON or process-argument syntax.
    case "$value" in
        ''|*[!A-Za-z0-9._:-]*) value= ;;
    esac
    printf '%s' "$value"
}

app_id=$(read_identity "$input_root/app-id")
session_id=$(read_identity "$input_root/session-id")
vm_id=$(read_identity "$hostname_file")

reason=
if [ -z "$app_id" ] || [ -z "$session_id" ] || [ -z "$vm_id" ]; then
    reason=missing-runtime-input
else
    case "$app_id" in
        *[!A-Za-z0-9._-]*|.*|*.|*..*)
            # The validator owns the complete reverse-DNS policy. This narrow
            # check prevents standalone use from accepting a non-Flatpak
            # identity while keeping the probe dependency-free.
            reason=invalid-app-identity
            ;;
        *.*) ;;
        *) reason=invalid-app-identity ;;
    esac
fi

timeout_bin=$(command -v timeout 2>/dev/null || true)
run_bounded() {
    "$timeout_bin" --kill-after=1s "$probe_timeout" "$@"
}

contract_has() {
    grep -Fq -- "$1" "$contract_file"
}

write_evidence() {
    state=$1
    state_reason=$2
    evidence_dir=${evidence_file%/*}
    [ "$evidence_dir" = "$evidence_file" ] && evidence_dir=.
    mkdir -p "$evidence_dir" || return 1
    evidence_tmp="$evidence_file.tmp.$$"
    printf '%s\n' \
        "{\"schema_version\":1,\"kind\":\"app_vm_runtime_preflight\",\"profile\":\"wayland-standard\",\"session_id\":\"$session_id\",\"vm_id\":\"$vm_id\",\"app_id\":\"$app_id\",\"state\":\"$state\",\"reason\":\"$state_reason\"}" \
        > "$evidence_tmp" || {
            rm -f "$evidence_tmp"
            return 1
        }
    chmod 0600 "$evidence_tmp" || {
        rm -f "$evidence_tmp"
        return 1
    }
    mv -f "$evidence_tmp" "$evidence_file"
}

finish() {
    state=$1
    state_reason=$2
    payload="{\"schema_version\":1,\"kind\":\"app_vm_runtime_preflight\",\"profile\":\"wayland-standard\",\"session_id\":\"$session_id\",\"vm_id\":\"$vm_id\",\"app_id\":\"$app_id\",\"state\":\"$state\",\"reason\":\"$state_reason\"}"
    if [ "${#payload}" -gt 2048 ]; then
        state=unavailable
        state_reason=evidence-too-large
        payload="{\"schema_version\":1,\"kind\":\"app_vm_runtime_preflight\",\"profile\":\"wayland-standard\",\"session_id\":\"$session_id\",\"vm_id\":\"$vm_id\",\"app_id\":\"$app_id\",\"state\":\"$state\",\"reason\":\"$state_reason\"}"
    fi
    if ! write_evidence "$state" "$state_reason"; then
        state=unavailable
        state_reason=evidence-write-failed
        payload="{\"schema_version\":1,\"kind\":\"app_vm_runtime_preflight\",\"profile\":\"wayland-standard\",\"session_id\":\"$session_id\",\"vm_id\":\"$vm_id\",\"app_id\":\"$app_id\",\"state\":\"$state\",\"reason\":\"$state_reason\"}"
        printf '%s\n' 'mcnf-app-vm-runtime-probe: could not persist bounded evidence' >&2
    fi
    printf '%s\n' "$payload"
    [ "$state" = ready ]
}

if [ -z "$reason" ] && [ ! -r "$contract_file" ]; then
    reason=missing-image-contract
fi
if [ -z "$reason" ] && ! contract_has '"schema_version":1'; then
    reason=invalid-image-contract
fi
if [ -z "$reason" ] && ! contract_has '"profile":"wayland-standard"'; then
    reason=invalid-image-contract
fi
if [ -z "$reason" ] && ! contract_has '"compositor":"sway"'; then
    reason=invalid-image-contract
fi
if [ -z "$reason" ] && ! contract_has '"flatpak_remote":"curated"'; then
    reason=invalid-image-contract
fi
if [ -z "$reason" ] && [ ! -r "$readiness_file" ]; then
    reason=missing-readiness-manifest
fi
if [ -z "$reason" ] && ! grep -Fq 'ready_state=connected' "$readiness_file"; then
    reason=invalid-readiness-manifest
fi
if [ -z "$reason" ] && ! grep -Fq 'host_fallback=disabled' "$readiness_file"; then
    reason=host-fallback-not-disabled
fi
if [ -z "$reason" ] && [ -z "$timeout_bin" ]; then
    reason=bounded-timeout-unavailable
fi

if [ -z "$reason" ]; then
    for binary in flatpak swaymsg dbus-send pw-cli pactl; do
        if ! command -v "$binary" >/dev/null 2>&1; then
            reason="missing-$binary"
            break
        fi
    done
fi

if [ -z "$reason" ]; then
    case "${WAYLAND_DISPLAY:-}" in
        ''|/*|*[!A-Za-z0-9._-]*) reason=wayland-session-unavailable ;;
    esac
fi
if [ -z "$reason" ]; then
    case "${SWAYSOCK:-}" in
        /*) ;;
        *) reason=sway-session-unavailable ;;
    esac
fi
if [ -z "$reason" ]; then
    case "${DBUS_SESSION_BUS_ADDRESS:-}" in
        unix:*) ;;
        *) reason=session-bus-unavailable ;;
    esac
fi

if [ -z "$reason" ] && ! run_bounded swaymsg -t get_version >/dev/null 2>&1; then
    reason=sway-unavailable
fi
if [ -z "$reason" ] && ! run_bounded dbus-send \
    --session --print-reply=literal \
    --dest=org.freedesktop.portal.Desktop \
    /org/freedesktop/portal/desktop \
    org.freedesktop.DBus.Peer.Ping >/dev/null 2>&1; then
    reason=portal-unavailable
fi
if [ -z "$reason" ] && ! run_bounded pw-cli info 0 >/dev/null 2>&1; then
    reason=pipewire-unavailable
fi
if [ -z "$reason" ] && ! run_bounded pactl info >/dev/null 2>&1; then
    reason=pulse-compat-unavailable
fi
if [ -z "$reason" ]; then
    sinks=$(run_bounded pactl list short sinks 2>/dev/null || true)
    if ! printf '%s\n' "$sinks" | awk 'NF { found=1 } END { exit(found ? 0 : 1) }'; then
        reason=audio-sink-unavailable
    fi
fi
if [ -z "$reason" ] && ! run_bounded flatpak remotes --system --columns=name 2>/dev/null |
    awk '$0 == "curated" { found=1 } END { exit(found ? 0 : 1) }'; then
    reason=curated-remote-unavailable
fi
if [ -z "$reason" ] && ! run_bounded flatpak info --system "$app_id" >/dev/null 2>&1; then
    reason=app-not-installed
fi

if [ -n "$reason" ]; then
    finish unavailable "$reason"
    exit 1
fi
finish ready preflight-ready
