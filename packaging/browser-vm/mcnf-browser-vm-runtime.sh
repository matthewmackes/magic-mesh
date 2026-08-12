#!/bin/sh
# Image-owned Browser VM runtime. Host input is identity-only and is validated
# before any compositor or Chromium process starts.
set -eu
umask 077

# xrdp carries an authenticated session environment into this process. Keep
# that environment from selecting a host-provisioned helper or Browser binary:
# every executable used below must come from the immutable guest image.
PATH=/usr/sbin:/usr/bin
export PATH

case "${1:-}" in
    '')
        [ "$#" -eq 0 ] || { echo 'FATAL: unexpected Browser VM runtime arguments' >&2; exit 2; }
        runtime_phase=bootstrap
        ;;
    --audio-ready)
        [ "$#" -eq 1 ] || { echo 'FATAL: unexpected Browser VM runtime arguments' >&2; exit 2; }
        runtime_phase=audio-ready
        ;;
    *)
        echo 'FATAL: unexpected Browser VM runtime argument' >&2
        exit 2
        ;;
esac

runtime_log=/var/lib/mcnf-browser/runtime.log
if : >> "$runtime_log" 2>/dev/null; then
    chmod 0600 "$runtime_log"
    exec 2>>"$runtime_log"
fi
# Keep a bounded, non-secret startup trace available to the host acceptance
# collector.  The private runtime log above remains the detailed guest record;
# this projection contains only the phase/status lines emitted below.
diagnostic_log=/var/tmp/mcnf-browser-runtime-diagnostic.log
if : >> "$diagnostic_log" 2>/dev/null; then
    chmod 0644 "$diagnostic_log"
    diagnostic_size=$(wc -c <"$diagnostic_log")
    case "$diagnostic_size" in
        ''|*[!0-9]*) diagnostic_size=0 ;;
    esac
    if [ "$diagnostic_size" -gt 65536 ]; then
        : >"$diagnostic_log"
    fi
fi
log() {
    printf 'mcnf-browser-vm-runtime: %s\n' "$*"
    if [ -w "$diagnostic_log" ]; then
        printf '%s mcnf-browser-vm-runtime: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" >>"$diagnostic_log"
    fi
}
trap 'status=$?; log "exited status=$status"' EXIT

log "starting guest-owned runtime phase=$runtime_phase"
runtime_evidence=/var/lib/mcnf-browser/runtime-evidence.json
media_evidence=/var/lib/mcnf-browser/media-evidence.json
gpu_probe=/var/lib/mcnf-browser/gpu-vainfo.log
media_probe=/var/lib/mcnf-browser/pipewire-probe.log
if [ "$runtime_phase" = bootstrap ]; then
    # Invalidate every prior-session success before even admission/provenance
    # validation. A malformed or missing input must fail without leaving an old
    # wired record available for collection as evidence of the new attempt.
    rm -f "$runtime_evidence" "$media_evidence" "$gpu_probe" "$media_probe"
    printf '%s\n' 'audio graph unavailable; runtime admission has not completed' \
        >"$media_probe"
    chmod 0600 "$media_probe"
    log 'prior session evidence invalidated before runtime admission'
fi

# The validator retains an explicit override for disposable contract fixtures,
# but an authenticated xrdp environment must not redirect the production
# runtime to a caller-selected identity directory.
unset MCNF_BROWSER_VM_INPUT_ROOT
input_root=/etc/mcnf-browser-vm
/usr/local/libexec/mcnf-browser-vm-validate
log 'runtime inputs validated'
transport=$(cat "$input_root/transport")
transport_health=$(cat "$input_root/transport-health")
source_commit=$(cat /usr/share/mcnf/browser-vm/source-commit)
image_digest=$(tr 'A-F' 'a-f' <"$input_root/image-digest")
case "$source_commit" in ''|*[!0-9a-f]*)
    echo 'FATAL: Browser VM source provenance is malformed' >&2
    exit 1
esac
[ "${#source_commit}" -eq 40 ] || {
    echo 'FATAL: Browser VM source provenance has the wrong length' >&2
    exit 1
}
case "$image_digest" in
    sha256:*) image_digest_hex=${image_digest#sha256:} ;;
    *) echo 'FATAL: Browser VM image provenance is malformed' >&2; exit 1 ;;
esac
case "$image_digest_hex" in ''|*[!0-9a-f]*)
    echo 'FATAL: Browser VM image provenance is malformed' >&2
    exit 1
esac
[ "${#image_digest_hex}" -eq 64 ] || {
    echo 'FATAL: Browser VM image provenance has the wrong length' >&2
    exit 1
}
rdp_waiting=0
case "$transport" in
    rdp)
        # The system unit is enabled for boot ordering, but the actual desktop
        # is created by xrdp per authenticated session. Avoid starting a second
        # compositor without DISPLAY from the system unit.
        if [ -z "${DISPLAY:-}" ]; then
            if [ "$runtime_phase" = bootstrap ]; then
                rdp_waiting=1
            else
                echo 'FATAL: Browser VM RDP audio-ready stage lost its xrdp display' >&2
                exit 1
            fi
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
runtime_dir=/run/mcnf-browser
install -d -o "$(id -u)" -g "$(id -g)" -m 0700 "$runtime_dir"
export XDG_RUNTIME_DIR="$runtime_dir"

write_runtime_evidence() {
    gpu_status=$1
    audio_status=$2
    audio_sink_count=$3
    audio_source_count=$4
    recorded_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    evidence_tmp=$runtime_evidence.tmp.$$
    printf '%s\n' \
        "{\"schema_version\":1,\"kind\":\"browser_vm_runtime_evidence\",\"profile\":\"browser-vm-chromium\",\"image\":\"browser-vm-chromium\",\"source_commit\":\"$source_commit\",\"image_digest\":\"$image_digest\",\"transport\":\"$transport\",\"transport_health\":\"$transport_health\",\"gpu_status\":\"$gpu_status\",\"audio_status\":\"$audio_status\",\"audio_playback_endpoints\":$audio_sink_count,\"audio_capture_endpoints\":$audio_source_count,\"recorded_at\":\"$recorded_at\"}" \
        >"$evidence_tmp"
    chmod 0600 "$evidence_tmp"
    mv -f "$evidence_tmp" "$runtime_evidence"
}

audio_graph_ready() {
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

if [ "$runtime_phase" = bootstrap ]; then
    # Provenance is now admitted, so replace the invalidated state with a
    # bounded unavailable record before audio startup. It remains truthful if
    # the private ready stage is never reached.
    printf '%s\n' 'audio graph unavailable; Chromium and media probes not started' \
        >"$media_probe"
    chmod 0600 "$media_probe"
    write_runtime_evidence unavailable unavailable 0 0
    log 'provisional runtime evidence records audio unavailable'

    if [ "$rdp_waiting" -eq 1 ]; then
        log 'RDP runtime is waiting for an authenticated xrdp session'
        exit 0
    fi

    if /usr/bin/dbus-run-session -- \
        /usr/local/libexec/mcnf-browser-vm-session \
        /usr/local/libexec/mcnf-browser-vm-runtime --audio-ready; then
        exit 0
    else
        status=$?
        log "audio/session supervisor failed closed status=$status"
        exit "$status"
    fi
fi

[ "${MCNF_BROWSER_VM_AUDIO_READY:-0}" = 1 ] || {
    write_runtime_evidence unavailable unavailable 0 0
    echo 'FATAL: Browser VM audio-ready stage was not admitted by the session supervisor' >&2
    exit 1
}
if ! audio_graph_ready; then
    write_runtime_evidence unavailable unavailable 0 0
    echo 'FATAL: Browser VM audio graph lost readiness before probes' >&2
    exit 1
fi
log 'PipeWire, WirePlumber, Pulse compatibility, playback, and capture are ready'

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
chromium_bin=
for candidate in /usr/bin/chromium /usr/bin/chromium-browser; do
    if [ -x "$candidate" ]; then
        chromium_bin=$candidate
        break
    fi
done
if [ -z "$chromium_bin" ]; then
    write_runtime_evidence unavailable unavailable 0 0
    echo 'FATAL: Browser VM Chromium binary is unavailable' >&2
    exit 1
fi
log "using Chromium binary $chromium_bin"

# The fixed guest-local media probe now runs only after the complete audio graph
# is ready. Its bounded record remains decode-state evidence only.
/usr/local/libexec/mcnf-browser-vm-media-probe "$chromium_bin" ||
    log 'guest-local Chromium media probe exited unexpectedly'

# Capture capability evidence from the actual Browser user. QEMU Guest Agent
# probes run under a confined root domain and cannot prove the compositor user's
# device permissions, so these diagnostics are image-owned and bounded.
gpu_status=unavailable
if [ -n "$render_node" ] && command -v vainfo >/dev/null 2>&1; then
    if vainfo --display drm --device "$render_node" >"$gpu_probe" 2>&1; then
        gpu_status=passed
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

audio_sinks=
audio_sources=
{
    printf '%s\n' '=== pw-cli info ==='
    pw-cli info 0 2>&1 || true
    printf '%s\n' '=== wpctl status ==='
    wpctl status 2>&1 || true
    printf '%s\n' '=== pactl sinks ==='
    audio_sinks=$(pactl list short sinks 2>/dev/null || true)
    printf '%s\n' "$audio_sinks"
    printf '%s\n' '=== pactl sources ==='
    audio_sources=$(pactl list short sources 2>/dev/null || true)
    printf '%s\n' "$audio_sources"
} >"$media_probe"
chmod 0600 "$media_probe"
log 'PipeWire/Pulse compatibility diagnostics captured after readiness'

# `audio_status=wired` means only that playback and capture endpoints were
# visible to the ready guest session. It never claims audible Chromium media.
audio_sink_count=$(printf '%s\n' "$audio_sinks" | awk 'NF { count++ } END { print count + 0 }')
audio_source_count=$(
    printf '%s\n' "$audio_sources" |
        awk 'NF && $2 !~ /[.]monitor$/ { count++ } END { print count + 0 }'
)
audio_status=unavailable
if [ "$audio_sink_count" -gt 0 ] && [ "$audio_source_count" -gt 0 ]; then
    audio_status=wired
fi
write_runtime_evidence "$gpu_status" "$audio_status" "$audio_sink_count" "$audio_source_count"
log "bounded runtime evidence written: gpu=$gpu_status audio=$audio_status"
if [ "$audio_status" != wired ]; then
    echo 'FATAL: Browser VM audio endpoints disappeared before Chromium startup' >&2
    exit 1
fi

mkdir -p "$HOME/.config/sway"
cat > "$HOME/.config/sway/config" <<'EOF'
default_border none
default_floating_border none
# QEMU's virtio display advertises a named Virtual-1 output. Explicitly enable
# the first safe mode so a fresh guest does not leave the compositor with a
# connected-but-unconfigured scanout (which presents as a black SPICE frame).
output * enable
output * mode 1920x1080
# wlroots' nested X11 backend advertises no modes, so the ordinary wildcard
# mode request above cannot resize its default 1024x768 window.  The explicit
# custom mode keeps Sway, Chromium, Xorg, and the negotiated RDP desktop at one
# geometry; absent output names are ignored by the other transport backend.
output X11-1 enable
output X11-1 mode --custom 1920x1080
# QEMU's virtio-vga connector is named Virtual-1. Keep the wildcard defaults
# for other display backends, but explicitly configure the live SPICE path so
# wlroots cannot leave the connected scanout unconfigured.
output Virtual-1 enable
output Virtual-1 mode 1920x1080
# Restore a persisted workload automatically after an unclean seat/VM restart,
# but keep Chromium's crash-recovery prompt out of the dedicated workspace.
# The prompt is anchored to the app-menu control and can otherwise obscure the
# first interaction after a seat or VM reboot. Reduced motion also makes popup
# surfaces opaque on their first frame, avoiding a stale translucent menu when
# xrdp's classic-bitmap stream coalesces Chromium's opening animation.
exec @CHROMIUM_BIN@ --ozone-platform=wayland --enable-features=UseOzonePlatform --start-maximized --no-first-run --restore-last-session --hide-crash-restore-bubble --force-prefers-reduced-motion --user-data-dir=/var/lib/mcnf-browser/chromium
EOF

# Chromium and the compositor are guest-owned. No URL, command, path, or
# browser state is accepted from the host declaration.
sed -i "s#@CHROMIUM_BIN@#$chromium_bin#" "$HOME/.config/sway/config"
log 'starting Sway and guest-owned Chromium after audio readiness'
exec /usr/local/libexec/mcnf-sway --unsupported-gpu --config "$HOME/.config/sway/config"
