#!/bin/sh
# Run the image-owned Chromium media fixture and emit bounded guest-local
# evidence. This is a decode probe, not live VDI acceptance; it never accepts
# a URL, command, or fixture path from Workloads or the host.
set -eu

evidence=/var/lib/mcnf-browser/media-evidence.json
log_file=/var/lib/mcnf-browser/media-probe.log
chromium_bin=${1:-}

fail_closed() {
    recorded_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    printf '%s\n' "{\"schema_version\":1,\"kind\":\"browser_vm_media_probe\",\"profile\":\"browser-vm-chromium\",\"image\":\"browser-vm-chromium\",\"status\":\"unavailable\",\"source\":\"guest-local-fixed-mkv\",\"video_ready_state\":0,\"video_total_frames\":0,\"video_dropped_frames\":0,\"video_width\":0,\"video_height\":0,\"audio_ready_state\":0,\"recorded_at\":\"$recorded_at\"}" > "$evidence"
    chmod 0600 "$evidence"
}

[ -n "$chromium_bin" ] || { fail_closed; exit 0; }
[ -x "$chromium_bin" ] || { fail_closed; exit 0; }

probe_dir=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/mcnf-browser-media-probe
mkdir -p "$probe_dir"
chmod 0700 "$probe_dir"
dom="$probe_dir/dom.html"
profile="$probe_dir/profile"
rm -rf "$profile"
mkdir -m 0700 "$profile"

if ! timeout 20 "$chromium_bin" \
    --headless=new \
    --no-first-run \
    --no-default-browser-check \
    --autoplay-policy=no-user-gesture-required \
    --allow-file-access-from-files \
    --virtual-time-budget=7000 \
    --user-data-dir="$profile" \
    --dump-dom \
    file:///usr/share/mcnf/browser-vm/mcnf-browser-vm-media-fixture.html \
    > "$dom" 2> "$log_file"; then
    chmod 0600 "$log_file" 2>/dev/null || true
    fail_closed
    exit 0
fi
chmod 0600 "$log_file" 2>/dev/null || true

marker=$(sed -n 's:.*<pre id="mcnf-result">MCNF_MEDIA_PROBE=\([^<]*\)</pre>.*:\1:p' "$dom" | tail -n 1)
case "$marker" in
    status=passed\|video_ready_state=*\|video_total_frames=*\|video_dropped_frames=*\|video_width=*\|video_height=*\|audio_ready_state=*)
        ;;
    status=unavailable\|video_ready_state=*\|video_total_frames=*\|video_dropped_frames=*\|video_width=*\|video_height=*\|audio_ready_state=*)
        ;;
    *)
        fail_closed
        exit 0
        ;;
esac

status=unavailable
ready=0
total=0
dropped=0
width=0
height=0
audio=0
old_ifs=$IFS
IFS='|'
set -- $marker
IFS=$old_ifs
for field in "$@"; do
    case "$field" in
        status=passed) status=passed ;;
        status=unavailable) status=unavailable ;;
        video_ready_state=*) ready=${field#*=} ;;
        video_total_frames=*) total=${field#*=} ;;
        video_dropped_frames=*) dropped=${field#*=} ;;
        video_width=*) width=${field#*=} ;;
        video_height=*) height=${field#*=} ;;
        audio_ready_state=*) audio=${field#*=} ;;
    esac
done

is_uint() {
    case "$1" in
        ''|*[!0-9]*) return 1 ;;
        *) return 0 ;;
    esac
}
for value in "$ready" "$total" "$dropped" "$width" "$height" "$audio"; do
    is_uint "$value" || { fail_closed; exit 0; }
done

recorded_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
printf '%s\n' "{\"schema_version\":1,\"kind\":\"browser_vm_media_probe\",\"profile\":\"browser-vm-chromium\",\"image\":\"browser-vm-chromium\",\"status\":\"$status\",\"source\":\"guest-local-fixed-mkv\",\"video_ready_state\":$ready,\"video_total_frames\":$total,\"video_dropped_frames\":$dropped,\"video_width\":$width,\"video_height\":$height,\"audio_ready_state\":$audio,\"recorded_at\":\"$recorded_at\"}" > "$evidence"
chmod 0600 "$evidence"
