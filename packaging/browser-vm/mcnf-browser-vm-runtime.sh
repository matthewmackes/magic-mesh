#!/bin/sh
# Image-owned Browser VM runtime. Host input is identity-only and is validated
# before any compositor or Chromium process starts.
set -eu

runtime_log=/var/lib/mcnf-browser/runtime.log
if : >> "$runtime_log" 2>/dev/null; then
    chmod 0600 "$runtime_log"
    exec 2>>"$runtime_log"
fi
log() {
    printf 'mcnf-browser-vm-runtime: %s\n' "$*"
}
trap 'status=$?; log "exited status=$status"' EXIT

log 'starting guest-owned runtime'
/usr/local/libexec/mcnf-browser-vm-validate
log 'runtime inputs validated'
input_root=${MCNF_BROWSER_VM_INPUT_ROOT:-/etc/mackesd/browser-vm}
transport=$(cat "$input_root/transport")
case "$transport" in
    rdp)
        # The system unit is enabled for boot ordering, but the actual desktop
        # is created by xrdp per authenticated session. Avoid starting a second
        # compositor without DISPLAY from the system unit.
        if [ -z "${DISPLAY:-}" ]; then
            echo 'Browser VM RDP runtime is waiting for an authenticated xrdp session'
            exit 0
        fi
        ;;
    spice)
        # SPICE is the direct QXL/pixman console path. The system unit owns
        # this compositor, while the broker supplies the loopback console to
        # the Construct VDI client over the mesh.
        ;;
    *)
        echo "FATAL: Browser VM transport is not implemented by this image: $transport" >&2
        exit 1
        ;;
esac
runtime_dir=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
install -d -o "$(id -u)" -g "$(id -g)" -m 0700 "$runtime_dir"
export XDG_RUNTIME_DIR="$runtime_dir"
# Prefer the hardware wlroots renderer when the VM exposes a DRM render node;
# retain pixman as the explicit compatibility fallback for hosts that do not.
render_node=
for node in /dev/dri/renderD*; do
    if [ -e "$node" ]; then
        render_node=$node
        break
    fi
done
if [ -z "${WLR_RENDERER:-}" ]; then
    if [ -n "$render_node" ]; then
        WLR_RENDERER=gles2
    else
        WLR_RENDERER=pixman
    fi
fi
export WLR_RENDERER
log "using wlroots renderer $WLR_RENDERER${render_node:+ with $render_node}"
export WLR_NO_HARDWARE_CURSORS=1
chromium_bin=$(command -v chromium || command -v chromium-browser)
log "using Chromium binary $chromium_bin"

# Capture capability evidence from the actual Browser user. QEMU Guest Agent
# probes run under a confined root domain and cannot prove the compositor user's
# device permissions, so these diagnostics are intentionally image-owned and
# best-effort. They never gate startup or accept host-provided commands.
gpu_probe=/var/lib/mcnf-browser/gpu-vainfo.log
if [ -n "$render_node" ] && command -v vainfo >/dev/null 2>&1; then
    if vainfo --display drm --device "$render_node" >"$gpu_probe" 2>&1; then
        chmod 0600 "$gpu_probe"
        log 'VA-API probe passed for the Browser user'
    else
        chmod 0600 "$gpu_probe" 2>/dev/null || true
        log 'VA-API probe unavailable for the Browser user'
    fi
elif command -v vainfo >/dev/null 2>&1; then
    printf '%s\n' 'no DRM render node exposed to the Browser VM' >"$gpu_probe"
    chmod 0600 "$gpu_probe"
    log 'VA-API probe unavailable because no DRM render node is exposed'
fi

media_probe=/var/lib/mcnf-browser/pipewire-probe.log
{
    printf '%s\n' '=== pw-cli info ==='
    pw-cli info 2>&1 || true
    printf '%s\n' '=== pactl sinks ==='
    pactl list short sinks 2>&1 || true
    printf '%s\n' '=== pactl sources ==='
    pactl list short sources 2>&1 || true
} >"$media_probe"
chmod 0600 "$media_probe"
log 'PipeWire/Pulse compatibility diagnostics captured'

mkdir -p "$HOME/.config/sway"
cat > "$HOME/.config/sway/config" <<'EOF'
default_border none
default_floating_border none
# QEMU's virtio display advertises a named Virtual-1 output. Explicitly enable
# the first safe mode so a fresh guest does not leave the compositor with a
# connected-but-unconfigured scanout (which presents as a black SPICE frame).
output * enable
output * mode 1024x768
# QEMU's virtio-vga connector is named Virtual-1. Keep the wildcard defaults
# for other display backends, but explicitly configure the live SPICE path so
# wlroots cannot leave the connected scanout unconfigured.
output Virtual-1 enable
output Virtual-1 mode 1024x768
exec @CHROMIUM_BIN@ --ozone-platform=wayland --enable-features=UseOzonePlatform --start-maximized --no-first-run --disable-session-crashed-bubble --user-data-dir=/var/lib/mcnf-browser/chromium
EOF

# Chromium and the compositor are guest-owned. No URL, command, path, or
# browser state is accepted from the host declaration.
sed -i "s#@CHROMIUM_BIN@#$chromium_bin#" "$HOME/.config/sway/config"
log 'starting PipeWire, WirePlumber, and Sway session'
exec /usr/bin/dbus-run-session -- /usr/local/libexec/mcnf-browser-vm-session \
    /usr/bin/sway --unsupported-gpu --config "$HOME/.config/sway/config"
