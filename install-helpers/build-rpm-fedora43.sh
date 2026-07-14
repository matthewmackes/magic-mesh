#!/bin/bash
# build-rpm-fedora43.sh — ONBOARD-7: roll a Fedora-43 magic-mesh RPM.
#
# The F44 dev host builds binaries that *run* on F43 unchanged, but the RPM's
# auto-generated glibc `Requires` pins the host's newer glibc symbol version, so
# `dnf install` refuses on F43 (and older DO images). Building inside a
# fedora:43 container produces an RPM whose glibc deps match F43, so it installs
# cleanly — the artifact `do-lighthouse-*.sh --rpm-url` (and the F43 cloud
# droplets) need.
#
# Reproducible: pulls fedora:43, installs the workspace build deps + the pinned
# rustup toolchain (rust-toolchain.toml → 1.94.0), builds the full workspace
# release, and runs cargo-generate-rpm. Reuses the host's ~/.cargo crate caches.
# Output: target-f43/generate-rpm/magic-mesh-*.x86_64.rpm (host-owned, rootless).
#
# XPA-6 — the GUI-less headless package. With `--server` this builds ONLY the
# daemon + mesh-substrate crates (mackesd/magic-fleet/mde-enroll/mde-bus — none
# pull libcosmic/iced), so a headless build skips the ~100 MB workbench/files/
# music/voice-hud/applet GUI compile entirely, then rolls the `server` variant
# (`cargo generate-rpm --variant server`) → a small `magic-mesh-server-*.rpm`
# with no GUI bins and no gtk3/libcosmic ELF Requires. The default (no flag)
# still builds the full workspace + the monolithic `magic-mesh` RPM unchanged.
#
# Usage: install-helpers/build-rpm-fedora43.sh [--server] [fedora_version]
#        install-helpers/build-rpm-fedora43.sh            # full GUI RPM, F43
#        install-helpers/build-rpm-fedora43.sh --server   # headless RPM, F43
set -euo pipefail

# XPA-6 — parse the optional --server flag (position-independent) so the
# remaining positional arg stays the Fedora version (back-compat with the
# original `[fedora_version]` calling convention).
MODE="full"
ARGS=()
for a in "$@"; do
  case "$a" in
    --server) MODE="server" ;;
    --full)   MODE="full" ;;
    *)        ARGS+=("$a") ;;
  esac
done
set -- "${ARGS[@]+"${ARGS[@]}"}"

FEDORA="${1:-43}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"

# build-deploy-7 — reproducible-cut pins (single source, overridable per cut).
# cargo-generate-rpm is pinned to the canonical version (docs/BUILD-ENVIRONMENT.md
# §2 + install-helpers/setup-build-vm-toolchain.sh + infra/ansible
# build-vm-toolchain.yml all pin 0.21.0 — the farm VMs already run it, so the
# container cut must match). Bump all four sites together. RUSTUP_INIT_SHA256 lets
# the operator checksum-pin the rustup installer (empty = warn + proceed). Both
# are exported into the fedora container via `podman run -e` below.
CGR_VERSION="${CGR_VERSION:-0.21.0}"
RUSTUP_INIT_SHA256="${RUSTUP_INIT_SHA256:-}"

# build-deploy-7 — base-image pinning (hermeticity). A bare `fedora:43` tag is
# MUTABLE: the Fedora registry re-publishes it on every point-release, so two
# cuts weeks apart can pull different base layers (different glibc/gcc/system libs
# the Servo/CEF helpers link against) and the "reproducible" cut is not. For a
# fully reproducible cut, pin the base by DIGEST. We CANNOT resolve a digest here
# on the airgapped farm (no registry egress at author time), so this is an
# OPERATOR TODO rather than an invented value:
#   1. On a networked host, resolve the current fedora:43 digest:
#        skopeo inspect docker://registry.fedoraproject.org/fedora:43 | jq -r .Digest
#        # or: podman pull fedora:43 && podman image inspect fedora:43 -f '{{.Digest}}'
#   2. Pin it — either export per cut, or set the default below:
#        BASE_IMAGE_DIGEST=sha256:<hex> install-helpers/build-rpm-fedora43.sh
#      and record the digest in the release evidence log.
# Mirrors the repo's existing pin discipline (rust-toolchain.toml pins rustc;
# vendor-birthright-blobs.sh sha256-verifies every fetched blob). NOTE: the bootc
# lane's base (packaging/bootc/Containerfile: quay.io/fedora/fedora-bootc:42) has
# the SAME open gap — pin both when you resolve digests.
BASE_IMAGE_DIGEST="${BASE_IMAGE_DIGEST:-}"
if [ -n "$BASE_IMAGE_DIGEST" ]; then
  IMAGE="registry.fedoraproject.org/fedora:${FEDORA}@${BASE_IMAGE_DIGEST}"
else
  IMAGE="registry.fedoraproject.org/fedora:${FEDORA}"
  echo "!! build-deploy-7: base image is TAG-pinned (fedora:${FEDORA}), not digest-pinned — this cut is NOT fully reproducible." >&2
  echo "   Set BASE_IMAGE_DIGEST=sha256:… to pin (see header comment for how to resolve it)." >&2
fi
command -v podman >/dev/null || { echo "podman required" >&2; exit 1; }

# BIRTHRIGHT-2 — stage the bundled air-gapped first-boot blobs on the host
# (has network) before the container build, so the generate-rpm assets exist.
echo "==> staging bundled birthright blobs (ntfy, starship)"
"$REPO/install-helpers/vendor-birthright-blobs.sh"

echo "==> pulling $IMAGE"
podman pull "$IMAGE" >/dev/null

# The in-container build. Runs as container-root == host-user (rootless podman),
# so target-f43/ + the RPM come out owned by the invoking user.
IN_CONTAINER='
set -euo pipefail
echo "[f43] installing build deps"
# mold is REQUIRED: .cargo/config.toml forces `-C link-arg=-fuse-ld=mold` for
# x86_64-unknown-linux-gnu, so a container without mold dies at the first link
# with `collect2: fatal error: cannot find 'ld'` / mold not found (hit on the
# 2026-06-20 11.0 fc43 build). binutils gives the `ld` fallback; protobuf-compiler
# is the etcd-client (SUBSTRATE-V2) build-time protoc dep.
# build-deploy-7 — these dnf deps are intentionally NOT version-pinned: the base
# image fixes their versions, so a DIGEST-pinned base (BASE_IMAGE_DIGEST above)
# makes this set reproducible. Pin the base rather than every package here.
dnf install -y --setopt=install_weak_deps=False \
    gcc gcc-c++ cmake pkg-config git curl findutils which gzip tar xz \
    mold binutils protobuf-compiler \
    gtk3-devel alsa-lib-devel openssl-devel opus-devel \
    libinput-devel mpv-libs-devel >/tmp/dnf.log 2>&1 || { tail -20 /tmp/dnf.log; exit 1; }

echo "[f43] installing rustup + the pinned toolchain"
export RUSTUP_HOME=/root/.rustup CARGO_HOME=/root/.cargo
# Pin the default to the rust-toolchain.toml channel so every cargo invocation
# (incl. `cargo install`, which ran before any repo-dir override took effect)
# resolves a version. Read the channel out of the repo so it never drifts.
CHANNEL="$(sed -n "s/^channel *= *\"\([^\"]*\)\".*/\1/p" /src/rust-toolchain.toml | head -1)"
CHANNEL="${CHANNEL:-1.94.0}"
# build-deploy-7 — the TOOLCHAIN version is pinned (read from the committed
# rust-toolchain.toml above → 1.94.0), so rustc/cargo are reproducible. The
# residual gap is the rustup INSTALLER script itself, fetched live from
# sh.rustup.rs. Fetch it to a file and verify its sha256 when the operator has
# pinned one (resolve once on a networked host:
#   curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sha256sum
# then export RUSTUP_INIT_SHA256=<hex>). Empty = warn + proceed so the cut still
# works airgapped-first. Mirrors vendor-birthright-blobs.sh sha256 discipline.
curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
if [ -n "${RUSTUP_INIT_SHA256:-}" ]; then
  echo "${RUSTUP_INIT_SHA256}  /tmp/rustup-init.sh" | sha256sum -c - \
    || { echo "!! build-deploy-7: rustup-init.sh sha256 MISMATCH — refusing the cut"; exit 1; }
else
  echo "!! build-deploy-7: rustup-init.sh fetched live WITHOUT a checksum pin (set RUSTUP_INIT_SHA256=<hex> to verify)." >&2
fi
sh /tmp/rustup-init.sh -y --default-toolchain "$CHANNEL" --profile minimal >/tmp/rustup.log 2>&1
export PATH=/root/.cargo/bin:$PATH
cd /src
echo "[f43] toolchain: $(rustc --version)"

echo "[f43] installing cargo-generate-rpm ${CGR_VERSION:-0.21.0}"
# build-deploy-7 — pin the packager to an EXACT version (CGR_VERSION, exported
# from the host default 0.21.0). --version makes the cut reproducible; --locked
# builds cargo-generate-rpm from its own pinned Cargo.lock. This matches the
# provisioning pin in setup-build-vm-toolchain.sh so container + farm cuts agree.
cargo install cargo-generate-rpm --version "${CGR_VERSION:-0.21.0}" --locked >/tmp/cgr.log 2>&1 || { tail -20 /tmp/cgr.log; exit 1; }

export CARGO_TARGET_DIR=/src/target-f43
export CMAKE_POLICY_VERSION_MINIMUM=3.5
# build-deploy-3 — the mde-shell-egui feature list + the --locked policy come
# from ONE canonical fragment, shared with xcp-build.sh, so the two RPM cut paths
# cannot drift. The repo is bind-mounted at /src, so it is present in-container.
source /src/install-helpers/rpm-features.sh
# XPA-6 — MODE (full|server) is passed in via `podman run -e MODE=…`.
if [ "${MODE:-full}" = "server" ]; then
  echo "[f43] building HEADLESS crates only (release) — no libcosmic GUIs"
  # Just the daemon + mesh-substrate crates. mde-enroll yields BOTH the
  # mde-enroll + magic-setup bins; mde-bus is the shared-bus daemon. None pull
  # libcosmic/iced, so the long GUI compile is skipped entirely.
  cargo build --release $MDE_RPM_LOCKED \
      -p mackesd -p magic-fleet -p mde-enroll -p mde-bus
  echo "[f43] generating headless RPM (--variant server)"
  cargo generate-rpm -p crates/mesh/mackesd --variant server
else
  echo "[f43] building workspace (release) — this is the long part"
  cargo build --workspace --release $MDE_RPM_LOCKED
  # BOOKMARKS-9 — the Servo browser helper (mde-web-preview) is its OWN workspace
  # root (excluded from the parent workspace: Servo drags a conflicting native
  # sqlite link — see the crate manifest), so `--workspace` above did NOT build it.
  # Build it here into the SAME CARGO_TARGET_DIR so it lands at
  # $CARGO_TARGET_DIR/release/mde-web-preview, which the generate-rpm asset
  # `target/release/mde-web-preview` resolves to (cargo-generate-rpm rewrites the
  # `target/` prefix to the active target dir). Servo needs the system graphics/
  # text -devel headers + libclang (mozjs bindgen) at build time; the container has
  # network here, so its crates fetch. A hard step: the full RPM ships the browser.
  echo "[f43] installing the Servo browser-helper build deps"
  dnf install -y --setopt=install_weak_deps=False \
      clang llvm python3 \
      fontconfig-devel freetype-devel harfbuzz-devel \
      mesa-libEGL-devel mesa-libGL-devel mesa-libgbm-devel \
      libxkbcommon-devel >/tmp/dnf-servo.log 2>&1 || { tail -20 /tmp/dnf-servo.log; exit 1; }
  echo "[f43] building the Servo browser helper (mde-web-preview) — heavy; needs network"
  cargo build --release $MDE_RPM_LOCKED \
      --manifest-path crates/desktop/mde-web-preview/Cargo.toml || {
    echo "GATED[BOOKMARKS-9/servo]: mde-web-preview (Servo) failed to build."
    echo "  The full magic-mesh RPM ships the browser helper, so this is a hard stop."
    echo "  The builder needs egress to crates.io for the servo crate tree + the"
    echo "  graphics/text -devel headers installed above; point at a LAN mirror on"
    echo "  an airgapped builder. (A headless RPM has no browser: use --server.)"
    exit 1; }
  # BROWSER-DD-1 - the Chromium/CEF helper is another workspace-excluded browser
  # root. This first slice is a lean scaffold with the shared BOOKMARKS-6 wire
  # protocol and an honest CEF_MISSING runtime gate; build it now so the full RPM
  # installs /usr/bin/mde-web-cef and the shell Engine -> CEF selection resolves
  # to the real helper path once a pinned CEF bundle is present. The same crate
  # also emits mde-web-cef-renderer, the native bridge process shipped under
  # /usr/libexec/mackesd for the Chrome-engine handoff.
  echo "[f43] building the Chromium/CEF browser helper + renderer bridge (mde-web-cef)"
  cargo build --release $MDE_RPM_LOCKED \
      --manifest-path crates/desktop/mde-web-cef/Cargo.toml
  # E12-3 DRM + BOOKMARKS-6 live path — re-link the ONE shell binary with the
  # features the shipped seat needs: `drm` so it owns the bare KMS/DRM seat,
  # `live-helper` so the Browser surface really spawns the sandboxed
  # `mde-web-preview` shipped right above (without it the surface is permanently
  # the gated EmptyState — the RPM would ship a browser helper no shell can ever
  # start), `live-vdi` so the Desktop surface can pump live RDP in-shell, and
  # `media-mpv` (BUG-VIDEO-1 / MEDIA-2 phase 1, docs/gpu_encoder.md) so the
  # embedded Media surface links the real mpv engine instead of silently
  # shipping FakeMpv (simulated playback, no real A/V — the live-verified
  # 2026-07-03 Eagle finding). The workspace build compiled all deps; this
  # only re-links one bin.
  echo "[f43] re-linking mde-shell-egui --features $MDE_RPM_SHELL_FEATURES"
  cargo build --release $MDE_RPM_LOCKED -p mde-shell-egui --features "$MDE_RPM_SHELL_FEATURES"
  echo "[f43] generating RPM"
  cargo generate-rpm -p crates/mesh/mackesd
fi

echo "[f43] DONE — artifact:"
ls -la /src/target-f43/generate-rpm/*.rpm
'

echo "==> building in $IMAGE (mode=$MODE; release + RPM; reuses ~/.cargo caches)"
# --security-opt label=disable: skip SELinux confinement for this trusted
# local build so the container can read the bind-mounted repo + crate caches
# without relabeling the host trees.
# XPA-6 — MODE selects full (GUI) vs server (headless) inside the container.
podman run --rm \
    --security-opt label=disable \
    -e "MODE=$MODE" \
    -e "CGR_VERSION=$CGR_VERSION" \
    -e "RUSTUP_INIT_SHA256=$RUSTUP_INIT_SHA256" \
    -v "$REPO:/src" \
    -v "$HOME/.cargo/registry:/root/.cargo/registry" \
    -v "$HOME/.cargo/git:/root/.cargo/git" \
    -w /src \
    "$IMAGE" bash -c "$IN_CONTAINER"

# XPA-6 — pick the artifact for THIS mode. `magic-mesh-server-*` sorts after
# `magic-mesh-*`, and a stale full RPM can sit beside it, so glob on the exact
# name prefix instead of taking the first *.rpm.
if [ "$MODE" = "server" ]; then
  GLOB="$REPO/target-f43/generate-rpm/magic-mesh-server-*.rpm"
else
  # The full package: magic-mesh-<ver>… but NOT magic-mesh-server-…
  GLOB="$REPO/target-f43/generate-rpm/magic-mesh-[0-9]*.rpm"
fi
# shellcheck disable=SC2086,SC2012  # $GLOB MUST stay unquoted to expand the
# wildcard; the existing artifact-pick uses the same `ls` glob idiom.
RPM="$(ls -1t $GLOB 2>/dev/null | head -1 || true)"
[ -n "$RPM" ] || { echo "!! no RPM produced (mode=$MODE)" >&2; exit 1; }
echo
echo "✅ Fedora $FEDORA RPM (mode=$MODE): $RPM"
echo "   install on F$FEDORA:  sudo dnf install $RPM"
echo "   or via Option A:      do-lighthouse-up.sh <mesh> --rpm-url <served-url-of-this-rpm>"
