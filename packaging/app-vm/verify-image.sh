#!/usr/bin/env bash
# WL-FUNC-018 — static acceptance checks for a built App VM image.
#
# This inspects image contents without booting a guest. Live Flatpak, Sway, and
# VDI convergence remain separate runtime gates; an image missing the fixed
# guest contract must never reach those gates.
set -euo pipefail

TAG="${1:-localhost/magic-mesh-app-vm-wayland:latest}"

valid_sha256_digest() {
    [[ "$1" =~ ^sha256:[0-9a-fA-F]{64}$ ]]
}

if [[ "${1:-}" == "--self-test" ]]; then
    valid_sha256_digest "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" || {
        echo "FATAL: valid sha256 digest rejected" >&2
        exit 1
    }
    for invalid in \
        "" \
        "sha256:" \
        "sha256:deadbeef" \
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg" \
        "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef-extra"; do
        if valid_sha256_digest "$invalid"; then
            echo "FATAL: malformed sha256 digest accepted: $invalid" >&2
            exit 1
        fi
    done
    echo "App VM image provenance self-tests passed"
    exit 0
fi

command -v podman >/dev/null 2>&1 || {
    echo "FATAL: podman not on PATH" >&2
    exit 2
}
podman image exists "$TAG" || {
    echo "FATAL: App VM image is not in local storage: $TAG (build it first)" >&2
    exit 1
}

profile="$(podman image inspect --format '{{index .Config.Labels "org.mcnf.app-vm.profile"}}' "$TAG" 2>/dev/null || true)"
base_id="$(podman image inspect --format '{{index .Config.Labels "org.mcnf.app-vm.base-image-id"}}' "$TAG" 2>/dev/null || true)"
[ "$profile" = "wayland-standard-v1" ] || {
    echo "FATAL: App VM image is missing immutable profile provenance" >&2
    exit 1
}
if ! valid_sha256_digest "$base_id"; then
    echo "FATAL: App VM image is missing a complete immutable base-image digest" >&2
    exit 1
fi
echo "  OK   immutable image provenance: profile=$profile base_id=$base_id"

INNER_SCRIPT="$(cat <<'INNER'
set -u
fail=0
ok()  { echo "  OK   $1"; }
bad() { echo "  FAIL $1"; fail=1; }

for path in \
    /usr/local/libexec/mcnf-app-vm-validate \
    /usr/local/libexec/mcnf-app-vm-launch \
    /usr/share/mcnf/app-vm/wayland-standard.profile \
    /usr/share/mcnf/app-vm/image-contract.json; do
    [ -f "$path" ] && ok "image file present: $path" || bad "image file missing: $path"
done

for binary in flatpak sway dbus-run-session; do
    command -v "$binary" >/dev/null 2>&1 \
        && ok "runtime binary present: $binary" \
        || bad "runtime binary missing: $binary"
done

for package in \
    magic-mesh flatpak sway xdg-desktop-portal xdg-desktop-portal-wlr \
    xdg-desktop-portal-gtk pipewire pipewire-pulseaudio wireplumber \
    libinput libxkbcommon; do
    rpm -q "$package" >/dev/null 2>&1 \
        && ok "package installed: $package" \
        || bad "package missing: $package"
done

grep -Fxq 'profile=wayland-standard' /usr/share/mcnf/app-vm/wayland-standard.profile \
    && ok 'profile selects wayland-standard' \
    || bad 'profile does not select wayland-standard'
grep -Fq '"schema_version":1' /usr/share/mcnf/app-vm/image-contract.json \
    && ok 'image contract schema is version 1' \
    || bad 'image contract schema marker missing'
grep -Fq '"profile":"wayland-standard"' /usr/share/mcnf/app-vm/image-contract.json \
    && ok 'image contract identifies wayland-standard' \
    || bad 'image contract profile missing'
grep -Fq '"compositor":"sway"' /usr/share/mcnf/app-vm/image-contract.json \
    && ok 'image contract identifies Sway' \
    || bad 'image contract compositor missing'
grep -Fq '"flatpak_remote":"curated"' /usr/share/mcnf/app-vm/image-contract.json \
    && ok 'image contract identifies curated remote' \
    || bad 'image contract remote policy missing'
! flatpak remotes --system --columns=name 2>/dev/null | grep -Fxq flathub \
    && ok 'image does not pre-admit public flathub' \
    || bad 'image pre-admits public flathub'
! grep -R -Fq 'flatpak remote-add' /usr/local/libexec /usr/share/mcnf/app-vm 2>/dev/null \
    && ok 'image has no unsigned remote-add helper' \
    || bad 'image contains an unsigned remote-add helper'

exit "$fail"
INNER
)"

rc=0
out="$(printf '%s\n' "$INNER_SCRIPT" | podman run --rm -i "$TAG" /bin/bash -s)" || rc=$?
printf '%s\n' "$out"
grep -q '^  OK ' <<<"$out" || {
    echo "FATAL: no App VM image checks executed" >&2
    rc=1
}
grep -q '^  FAIL ' <<<"$out" && rc=1
if [ "$rc" -eq 0 ]; then
    echo "==> verify-app-vm-image: ALL STATIC CHECKS PASS for $TAG"
else
    echo "==> verify-app-vm-image: FAILURES above for $TAG" >&2
fi
exit "$rc"
