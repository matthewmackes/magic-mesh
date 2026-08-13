#!/usr/bin/env bash
# Static acceptance checks for a built Browser VM image; this is not live proof.
set -euo pipefail
TAG="${1:-localhost/magic-mesh-browser-vm-chromium:latest}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SESSION_INPUT_VERIFY="$ROOT/packaging/browser-vm/verify-session-input-contract.sh"
PRODUCTION_CONTROL_VERIFY="$ROOT/packaging/browser-vm/verify-production-control-image.py"
MANIFEST_VERIFY="$ROOT/packaging/browser-vm/verify-image-manifest.py"

if [[ "${1:-}" == "--self-test" ]]; then
    [ "$#" -eq 1 ] || { echo 'usage: verify-image.sh --self-test' >&2; exit 2; }
    # shellcheck disable=SC2050 # Deliberate fixed positive provenance fixture.
    [[ "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" =~ ^sha256:[0-9a-fA-F]{64}$ ]]
    "$MANIFEST_VERIFY" self-test --repo-root "$ROOT" \
        --profile "$ROOT/packaging/browser-vm/profile.env" >/dev/null
    "$SESSION_INPUT_VERIFY" --self-test >/dev/null
    "$PRODUCTION_CONTROL_VERIFY" --self-test >/dev/null
    echo 'Browser VM image provenance/manifest self-tests passed'
    exit 0
fi
if [[ "${1:-}" == "--artifact" ]]; then
    [ "$#" -eq 3 ] || {
        echo 'usage: verify-image.sh --artifact IMAGE MANIFEST' >&2
        exit 2
    }
    "$ROOT/packaging/browser-vm/verify-profile.sh" --source \
        --manifest "$3" --image "$2" \
        "$ROOT/packaging/browser-vm/profile.env"
    exit 0
fi
command -v podman >/dev/null 2>&1 || { echo 'FATAL: podman is required' >&2; exit 2; }
podman image exists "$TAG" || { echo "FATAL: image is missing: $TAG" >&2; exit 1; }
profile="$(podman image inspect --format '{{index .Config.Labels "org.mcnf.browser-vm.profile"}}' "$TAG")"
[[ "$profile" == browser-vm-chromium-v1 ]] || { echo 'FATAL: immutable Browser VM profile label missing' >&2; exit 1; }
profile_source_commit="$(sed -n 's/^BROWSER_VM_SOURCE_COMMIT=//p' "$ROOT/packaging/browser-vm/profile.env")"
image_source_commit="$(podman image inspect --format '{{index .Config.Labels "org.mcnf.browser-vm.source-commit"}}' "$TAG")"
lighthouse_rpm_sha256="$(podman image inspect --format '{{index .Config.Labels "org.mcnf.browser-vm.lighthouse-rpm-sha256"}}' "$TAG")"
[[ "$profile_source_commit" =~ ^[0-9a-f]{40}$ ]] || { echo 'FATAL: profile source commit is malformed' >&2; exit 1; }
[[ "$image_source_commit" == "$profile_source_commit" ]] || {
    echo "FATAL: image source commit label does not match profile ($image_source_commit != $profile_source_commit)" >&2
    exit 1
}
[[ "$lighthouse_rpm_sha256" =~ ^[0-9a-f]{64}$ ]] || { echo 'FATAL: immutable Lighthouse RPM digest label missing' >&2; exit 1; }

# shellcheck disable=SC2016 # Expanded by Bash inside the image, not by this shell.
inner='set -u
fail=0
ok(){ echo "  OK   $1"; }
bad(){ echo "  FAIL $1"; fail=1; }
for path in /usr/local/libexec/mcnf-browser-vm-validate /usr/local/libexec/mcnf-browser-vm-runtime /usr/local/libexec/mcnf-browser-vm-session /usr/local/libexec/mcnf-browser-vm-verify-session-input /usr/local/libexec/mcnf-browser-vm-media-probe /usr/local/libexec/mcnf-sway /usr/local/lib64/libmcnf-x11-present-copy.so /usr/libexec/xrdp/startwm.sh /etc/systemd/system/mcnf-browser-vm-runtime.service /usr/share/mcnf/browser-vm/image-contract.json /usr/share/mcnf/browser-vm/source-commit /usr/share/mcnf/browser-vm/lighthouse-rpm.sha256 /usr/share/mcnf/browser-vm/mcnf-browser-vm-media-fixture.html /usr/share/mcnf/browser-vm/fixtures/tiny_clip.mkv; do
  [ -f "$path" ] && ok "image file present: $path" || bad "image file missing: $path"
done
guest_source_commit="$(cat /usr/share/mcnf/browser-vm/source-commit 2>/dev/null || true)"
expected_source_commit="${MCNF_EXPECTED_SOURCE_COMMIT:-}"
[ "$guest_source_commit" = "$expected_source_commit" ] && ok "guest runtime provenance matches image label" || bad "guest runtime provenance does not match image label"
guest_lighthouse_rpm_sha256="$(cat /usr/share/mcnf/browser-vm/lighthouse-rpm.sha256 2>/dev/null || true)"
expected_lighthouse_rpm_sha256="${MCNF_EXPECTED_LIGHTHOUSE_RPM_SHA256:-}"
[ "$guest_lighthouse_rpm_sha256" = "$expected_lighthouse_rpm_sha256" ] && ok "guest Lighthouse RPM provenance matches image label" || bad "guest Lighthouse RPM provenance does not match image label"
chromium_bin="$(command -v chromium || command -v chromium-browser || true)"
[ -n "$chromium_bin" ] && ok "runtime binary present: chromium ($chromium_bin)" || bad "runtime binary missing: chromium"
for binary in mackesd meshctl nebula sway dbus-run-session pipewire pipewire-pulse wireplumber pw-cli pactl aplay arecord vainfo xrdp; do
  command -v "$binary" >/dev/null 2>&1 && ok "runtime binary present: $binary" || bad "runtime binary missing: $binary"
done
for package in magic-mesh-lighthouse chromium sway pipewire pipewire-utils pipewire-pulseaudio wireplumber spice-vdagent pipewire-alsa pulseaudio-utils alsa-lib alsa-ucm alsa-utils mesa-dri-drivers libva-utils libinput xrdp xrdp-selinux xorgxrdp-glamor qemu-guest-agent; do
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
grep -Fxq "DefaultWindowManager=startwm.sh" /etc/xrdp/sesman.ini \
  && ok "xrdp authenticated sessions enter the Browser runtime" \
  || bad "xrdp authenticated sessions do not enter the Browser runtime"
grep -Fxq "use_fastpath=input" /etc/xrdp/xrdp.ini \
  && ok "xrdp uses FastPath input with slow-path bitmap graphics" \
  || bad "xrdp graphics may enter an unsupported FastPath output path"
/usr/local/libexec/mcnf-browser-vm-verify-session-input --image-root / \
  && ok "xrdp/Xorg/Sway/Chromium session-input contract" \
  || bad "xrdp/Xorg/Sway/Chromium session-input contract"
semodule -l 2>/dev/null | grep -Eq "^xrdp([[:space:]]|$)" \
  && ok "xrdp SELinux policy module is active" \
  || bad "xrdp SELinux policy module is inactive"
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
out="$(printf '%s\n' "$inner" | podman run --rm -i -e "MCNF_EXPECTED_SOURCE_COMMIT=$image_source_commit" -e "MCNF_EXPECTED_LIGHTHOUSE_RPM_SHA256=$lighthouse_rpm_sha256" "$TAG" /bin/bash -s)" || rc=$?
printf '%s\n' "$out"
grep -q '^  OK ' <<<"$out" || rc=1
grep -q '^  FAIL ' <<<"$out" && rc=1
control_rc=0
control_out="$(podman run --rm -i "$TAG" /usr/bin/python3 - --image-root / \
    < "$PRODUCTION_CONTROL_VERIFY" 2>&1)" || control_rc=$?
printf '%s\n' "$control_out"
(( control_rc == 0 )) || rc=1
grep -Fxq 'Browser VM production-control image contract passed' <<<"$control_out" || rc=1
exit "$rc"
