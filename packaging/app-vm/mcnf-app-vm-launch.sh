#!/bin/sh
# Launch exactly the admitted Flatpak identity inside the guest compositor.
# This helper is image-owned; cloud-init supplies identities and policy files,
# never an executable command.
set -eu

# The launcher owns the single guest application process.  A systemd stop or
# compositor shutdown must not leave that process behind to paint a stale VDI
# surface or survive into a later session generation.
app_pid=
handle_shutdown() {
    signal=$1
    if test -n "${app_pid}"; then
        kill -TERM "$app_pid" >/dev/null 2>&1 || true
        wait "$app_pid" >/dev/null 2>&1 || true
    fi
    publish_runtime failed "application stopped by guest supervisor ($signal)"
    exit 143
}

input_root=/etc/mackesd/app-vm
app_id=$(cat "$input_root/app-id")
session_id=$(cat "$input_root/session-id")
vm_id=$(cat /etc/hostname)
app_id=${app_id#\"}
app_id=${app_id%\"}
session_id=${session_id#\"}
session_id=${session_id%\"}
generation_file=/var/lib/mackesd/app-vm/generation
mkdir -p "${generation_file%/*}"
next_generation() {
    current=0
    if test -r "$generation_file"; then current=$(cat "$generation_file"); fi
    case "$current" in ''|*[!0-9]*) current=0 ;; esac
    if [ "${#current}" -gt 18 ]; then
        echo "FATAL: App VM runtime generation is invalid" >&2
        exit 1
    fi
    if [ "$current" -ge 9223372036854775806 ] 2>/dev/null; then
        echo "FATAL: App VM runtime generation exhausted" >&2
        exit 1
    fi
    generation=$((current + 1))
    printf '%s\n' "$generation" > "$generation_file.tmp"
    mv -f "$generation_file.tmp" "$generation_file"
}

publish_runtime() {
    state=$1
    reason=${2:-}
    next_generation
    if command -v mde-bus >/dev/null 2>&1; then
        body=$(printf '{"session_id":"%s","vm_id":"%s","app_id":"%s","generation":%s,"state":"%s","reason":"%s"}' \
            "$session_id" "$vm_id" "$app_id" "$generation" "$state" "$reason")
        mde-bus publish state/vdi/app-runtime --body-flag "$body" >/dev/null 2>&1 || true
    fi
}

/usr/local/libexec/mcnf-app-vm-validate

trap 'handle_shutdown TERM' TERM
trap 'handle_shutdown INT' INT
trap 'handle_shutdown HUP' HUP

# Sway has to be live, the session bus/portal and PipeWire have to answer, and
# the admitted Flatpak must be installed before this launcher can publish a
# connected state. The probe writes an unavailable record and exits non-zero
# when any guest-owned prerequisite is absent; there is no host fallback.
if ! preflight_report=$(/usr/local/libexec/mcnf-app-vm-runtime-probe); then
    printf '%s\n' "$preflight_report" >&2
    publish_runtime unavailable "guest runtime preflight unavailable"
    exit 1
fi
printf '%s\n' "$preflight_report" >&2

set +e
/usr/bin/flatpak run --system curated "$app_id" &
app_pid=$!
set -e
publish_runtime connected "application process started"
set +e
wait "$app_pid"
status=$?
set -e

publish_runtime failed "application process exited with status $status"

# A single-app guest has no useful desktop after its app exits. End the fixed
# compositor so systemd records the terminal app failure and VDI stops painting
# a stale surface.
if command -v swaymsg >/dev/null 2>&1; then
    swaymsg exit >/dev/null 2>&1 || true
fi
exit "$status"
