#!/usr/bin/env bash
# Read-only Browser VM audio wiring evidence.
#
# This checks the declarative QEMU/libvirt boundary only: a virtio sound card,
# an allowed host audio backend, and both guest playback and capture endpoints.
# It deliberately does not claim that audio is audible, captured, or recovered;
# those require a booted guest and live media traffic.
set -euo pipefail

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

devices = root.find("devices")
sound = devices.find("sound") if devices is not None else None
audio = devices.find("audio") if devices is not None else None
model = sound.get("model") if sound is not None else None
backend = audio.get("type") if audio is not None else None
output = audio.find("output") if audio is not None else None
input_ = audio.find("input") if audio is not None else None

result = {
    "status": "ready",
    "source": label,
    "sound_model": model,
    "backend": backend,
    "playback_endpoint": output is not None,
    "capture_endpoint": input_ is not None,
}
reasons = []
if model != "virtio":
    reasons.append("virtio sound device is missing")
if backend not in {"pipewire", "pulseaudio"}:
    reasons.append("audio backend is not PipeWire or PulseAudio")
if output is None:
    reasons.append("guest playback endpoint is missing")
if input_ is None:
    reasons.append("guest capture endpoint is missing")
if reasons:
    result["status"] = "failed"
    result["reason"] = "; ".join(reasons)
print(json.dumps(result, sort_keys=True))
raise SystemExit(1 if reasons else 0)
    ' "$label"
}

self_test() {
    local valid="<domain><devices><sound model='virtio'/><audio id='1' type='pipewire'><input name='in'/><output name='out'/></audio></devices></domain>"
    local missing_capture="<domain><devices><sound model='virtio'/><audio id='1' type='pulseaudio'><output name='out'/></audio></devices></domain>"
    printf '%s' "$valid" | check_xml self-test-valid >/dev/null
    if printf '%s' "$missing_capture" | check_xml self-test-missing-capture >/dev/null 2>&1; then
        echo 'verify-browser-vm-audio: self-test accepted missing capture endpoint' >&2
        exit 1
    fi
    echo 'verify-browser-vm-audio: self-test passed'
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
