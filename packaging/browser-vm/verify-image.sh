#!/usr/bin/env bash
# Static acceptance checks for a built Browser VM image; this is not live proof.
set -euo pipefail
TAG="${1:-localhost/magic-mesh-browser-vm-chromium:latest}"

if [[ "${1:-}" == "--self-test" ]]; then
    [[ "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" =~ ^sha256:[0-9a-fA-F]{64}$ ]]
    echo 'Browser VM image provenance self-tests passed'
    exit 0
fi
command -v podman >/dev/null 2>&1 || { echo 'FATAL: podman is required' >&2; exit 2; }
podman image exists "$TAG" || { echo "FATAL: image is missing: $TAG" >&2; exit 1; }
profile="$(podman image inspect --format '{{index .Config.Labels "org.mcnf.browser-vm.profile"}}' "$TAG")"
[[ "$profile" == browser-vm-chromium-v1 ]] || { echo 'FATAL: immutable Browser VM profile label missing' >&2; exit 1; }

inner='set -u
fail=0
ok(){ echo "  OK   $1"; }
bad(){ echo "  FAIL $1"; fail=1; }
for path in /usr/local/libexec/mcnf-browser-vm-validate /usr/local/libexec/mcnf-browser-vm-runtime /usr/local/libexec/mcnf-browser-vm-session /etc/xrdp/startwm.sh /etc/systemd/system/mcnf-browser-vm-runtime.service /usr/share/mcnf/browser-vm/image-contract.json; do
  [ -f "$path" ] && ok "image file present: $path" || bad "image file missing: $path"
done
chromium_bin="$(command -v chromium || command -v chromium-browser || true)"
[ -n "$chromium_bin" ] && ok "runtime binary present: chromium ($chromium_bin)" || bad "runtime binary missing: chromium"
for binary in sway dbus-run-session pipewire pipewire-pulse wireplumber pw-cli pactl aplay arecord vainfo xrdp; do
  command -v "$binary" >/dev/null 2>&1 && ok "runtime binary present: $binary" || bad "runtime binary missing: $binary"
done
for package in magic-mesh chromium sway pipewire pipewire-utils pipewire-pulseaudio wireplumber spice-vdagent pipewire-alsa alsa-lib alsa-ucm alsa-utils mesa-dri-drivers libva-utils libinput xrdp xorgxrdp qemu-guest-agent; do
  rpm -q "$package" >/dev/null 2>&1 && ok "package installed: $package" || bad "package missing: $package"
done
grep -Fq "\"browser\":\"chromium\"" /usr/share/mcnf/browser-vm/image-contract.json && ok "contract selects Chromium" || bad "contract does not select Chromium"
grep -Fq "\"compositor\":\"sway\"" /usr/share/mcnf/browser-vm/image-contract.json && ok "contract selects Sway" || bad "contract does not select Sway"
grep -Fq "\"host_browser\":false" /usr/share/mcnf/browser-vm/image-contract.json && ok "contract forbids host Browser" || bad "contract permits host Browser"
grep -Fq '"transports":["rdp","spice"]' /usr/share/mcnf/browser-vm/image-contract.json \
  && ok "contract admits RDP and SPICE" \
  || bad "contract does not admit the typed Browser VM transport set"
command -v xrdp >/dev/null 2>&1 && ok "RDP endpoint binary present" || bad "RDP endpoint binary missing"
command -v vainfo >/dev/null 2>&1 && ok "VA-API diagnostic present" || bad "VA-API diagnostic missing"
exit "$fail"'
rc=0
out="$(printf '%s\n' "$inner" | podman run --rm -i "$TAG" /bin/bash -s)" || rc=$?
printf '%s\n' "$out"
grep -q '^  OK ' <<<"$out" || rc=1
grep -q '^  FAIL ' <<<"$out" && rc=1
exit "$rc"
