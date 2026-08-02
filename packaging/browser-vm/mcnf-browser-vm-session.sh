#!/bin/sh
# Keep the guest media graph tied to the authenticated xrdp desktop session.
# These are user processes inside the VM; no host PipeWire socket is imported.
set -eu

[ "$#" -gt 0 ] || { echo 'FATAL: Browser VM session needs a compositor command' >&2; exit 2; }

pipewire &
pipewire_pid=$!
pipewire-pulse &
pipewire_pulse_pid=$!
wireplumber &
wireplumber_pid=$!

cleanup() {
    kill "$wireplumber_pid" "$pipewire_pulse_pid" "$pipewire_pid" 2>/dev/null || true
    wait "$wireplumber_pid" "$pipewire_pulse_pid" "$pipewire_pid" 2>/dev/null || true
}
trap cleanup EXIT HUP INT TERM

"$@"
