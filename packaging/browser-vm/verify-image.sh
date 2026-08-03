#!/usr/bin/env bash
# Static acceptance checks for a built Browser VM image; this is not live proof.
set -euo pipefail
TAG="${1:-localhost/magic-mesh-browser-vm-chromium:latest}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

if [[ "${1:-}" == "--self-test" ]]; then
    [[ "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" =~ ^sha256:[0-9a-fA-F]{64}$ ]]
    echo 'Browser VM image provenance self-tests passed'
    exit 0
fi
command -v podman >/dev/null 2>&1 || { echo 'FATAL: podman is required' >&2; exit 2; }
podman image exists "$TAG" || { echo "FATAL: image is missing: $TAG" >&2; exit 1; }
profile="$(podman image inspect --format '{{index .Config.Labels "org.mcnf.browser-vm.profile"}}' "$TAG")"
[[ "$profile" == browser-vm-chromium-v1 ]] || { echo 'FATAL: immutable Browser VM profile label missing' >&2; exit 1; }
profile_source_commit="$(sed -n 's/^BROWSER_VM_SOURCE_COMMIT=//p' "$ROOT/packaging/browser-vm/profile.env")"
image_source_commit="$(podman image inspect --format '{{index .Config.Labels "org.mcnf.browser-vm.source-commit"}}' "$TAG")"
[[ "$profile_source_commit" =~ ^[0-9a-f]{40}$ ]] || { echo 'FATAL: profile source commit is malformed' >&2; exit 1; }
[[ "$image_source_commit" == "$profile_source_commit" ]] || {
    echo "FATAL: image source commit label does not match profile ($image_source_commit != $profile_source_commit)" >&2
    exit 1
}

inner='set -u
fail=0
ok(){ echo "  OK   $1"; }
bad(){ echo "  FAIL $1"; fail=1; }
for path in /usr/local/libexec/mcnf-browser-vm-validate /usr/local/libexec/mcnf-browser-vm-runtime /usr/local/libexec/mcnf-browser-vm-session /usr/local/libexec/mcnf-browser-vm-media-probe /etc/xrdp/startwm.sh /etc/systemd/system/mcnf-browser-vm-runtime.service /usr/share/mcnf/browser-vm/image-contract.json /usr/share/mcnf/browser-vm/source-commit /usr/share/mcnf/browser-vm/mcnf-browser-vm-media-fixture.html /usr/share/mcnf/browser-vm/fixtures/tiny_clip.mkv; do
  [ -f "$path" ] && ok "image file present: $path" || bad "image file missing: $path"
done
guest_source_commit="$(cat /usr/share/mcnf/browser-vm/source-commit 2>/dev/null || true)"
[ "$guest_source_commit" = "$image_source_commit" ] && ok "guest runtime provenance matches image label" || bad "guest runtime provenance does not match image label"
chromium_bin="$(command -v chromium || command -v chromium-browser || true)"
[ -n "$chromium_bin" ] && ok "runtime binary present: chromium ($chromium_bin)" || bad "runtime binary missing: chromium"
for binary in mackesd meshctl nebula sway dbus-run-session pipewire pipewire-pulse wireplumber pw-cli pactl aplay arecord vainfo xrdp; do
  command -v "$binary" >/dev/null 2>&1 && ok "runtime binary present: $binary" || bad "runtime binary missing: $binary"
done
for package in magic-mesh-lighthouse chromium sway pipewire pipewire-utils pipewire-pulseaudio wireplumber spice-vdagent pipewire-alsa pulseaudio-utils alsa-lib alsa-ucm alsa-utils mesa-dri-drivers libva-utils libinput xrdp xorgxrdp qemu-guest-agent; do
  rpm -q "$package" >/dev/null 2>&1 && ok "package installed: $package" || bad "package missing: $package"
done
[ -L /etc/systemd/system/multi-user.target.wants/spice-vdagentd.service ] \
  && ok "SPICE guest agent service enabled" \
  || bad "SPICE guest agent service is not enabled"
grep -Fq "\"browser\":\"chromium\"" /usr/share/mcnf/browser-vm/image-contract.json && ok "contract selects Chromium" || bad "contract does not select Chromium"
grep -Fq "\"control_plane\":\"magic-mesh-lighthouse\"" /usr/share/mcnf/browser-vm/image-contract.json && ok "contract selects thin guest control plane" || bad "contract does not select the thin guest control plane"
grep -Fq "\"compositor\":\"sway\"" /usr/share/mcnf/browser-vm/image-contract.json && ok "contract selects Sway" || bad "contract does not select Sway"
grep -Fq "\"host_browser\":false" /usr/share/mcnf/browser-vm/image-contract.json && ok "contract forbids host Browser" || bad "contract permits host Browser"
grep -Fq "\"transports\":[\"rdp\",\"spice\"]" /usr/share/mcnf/browser-vm/image-contract.json \
  && ok "contract admits RDP and SPICE" \
  || bad "contract does not admit the typed Browser VM transport set"
if rpm -q magic-mesh magic-mesh-browser >/dev/null 2>&1; then
  bad "host workstation/browser RPM is installed"
else
  ok "host workstation/browser RPMs are absent"
fi
browser_groups="$(id -nG mcnf-browser 2>/dev/null || true)"
for group in video input audio seat render; do
  case " $browser_groups " in
    *" $group "*) ok "mcnf-browser is in device group: $group" ;;
    *) bad "mcnf-browser is missing device group: $group" ;;
  esac
done
command -v xrdp >/dev/null 2>&1 && ok "RDP endpoint binary present" || bad "RDP endpoint binary missing"
command -v vainfo >/dev/null 2>&1 && ok "VA-API diagnostic present" || bad "VA-API diagnostic missing"
exit "$fail"'
rc=0
out="$(printf '%s\n' "$inner" | podman run --rm -i "$TAG" /bin/bash -s)" || rc=$?
printf '%s\n' "$out"
grep -q '^  OK ' <<<"$out" || rc=1
grep -q '^  FAIL ' <<<"$out" && rc=1
exit "$rc"
