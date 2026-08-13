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
# Output for the default full mode: target-f43/generate-rpm/magic-mesh-*.rpm plus
# magic-mesh-lighthouse-*.rpm (host-owned, rootless).
#
# XPA-6 — the GUI-less headless package. With `--server` this builds ONLY the
# daemon + mesh-substrate crates (mackesd/magic-fleet/mde-enroll/mde-bus — none
# pull libcosmic/iced), so a headless build skips the ~100 MB workbench/files/
# music/voice-hud/applet GUI compile entirely, then rolls the `server` variant
# (`cargo generate-rpm --variant server`) → a small `magic-mesh-server-*.rpm`
# with no GUI bins and no gtk3/libcosmic ELF Requires. The default (no flag)
# builds the full workspace and emits the base `magic-mesh` RPM. `--lighthouse`
# is the thin DO lane: it compiles only mackesd/meshctl and emits
# `magic-mesh-lighthouse`, whose
# manifest intentionally excludes media and Syncthing file-sharing assets.
#
# Usage: install-helpers/build-rpm-fedora43.sh [--server|--lighthouse] [fedora_version]
#        install-helpers/build-rpm-fedora43.sh            # full GUI RPM, F43
#        install-helpers/build-rpm-fedora43.sh --server   # headless RPM, F43
#        install-helpers/build-rpm-fedora43.sh --lighthouse # thin DO RPM, F43
set -euo pipefail

# WL-ARCH-010 — both the base and server RPMs run the sole typed Workloads
# actuator.  cargo-generate-rpm cannot merge a variant's dependency table with
# the base table: the server variant replaces it wholesale.  Keep the source
# manifest human-readable, but promote the exact runtime providers into both
# generated compute manifests before packaging.  This is a build transform,
# committed here and performed only in the disposable farm checkout.
WORKLOAD_RPM_REQUIRES=(
  podman libvirt qemu-kvm qemu-ui-dbus libvirt-daemon-kvm
  libvirt-daemon-driver-storage guestfs-tools genisoimage virtiofsd
)

prepare_workload_rpm_manifest() {
  local input=$1 output=$2
  awk -v required="${WORKLOAD_RPM_REQUIRES[*]}" '
    BEGIN {
      split(required, names, " ")
      target["[package.metadata.generate-rpm.requires]"] = 1
      target["[package.metadata.generate-rpm.variants.server.requires]"] = 1
    }
    function emit_missing(    i, name) {
      if (!inside) return
      for (i = 1; i <= length(names); i++) {
        name = names[i]
        if (!seen[name]) print name " = \"*\""
      }
    }
    /^\[/ {
      emit_missing()
      inside = target[$0]
      delete seen
    }
    {
      if (inside && match($0, /^[A-Za-z0-9_-]+[[:space:]]*=/)) {
        name = substr($0, RSTART, RLENGTH)
        sub(/[[:space:]]*=.*/, "", name)
        seen[name] = 1
      }
      print
    }
    END { emit_missing() }
  ' "$input" >"$output"
}

workload_rpm_manifest_self_test() {
  local fixture transformed dep section count
  fixture=$(mktemp)
  transformed=$(mktemp)
  trap 'rm -f "$fixture" "$transformed"' RETURN
  cat >"$fixture" <<'EOF'
[package.metadata.generate-rpm.requires]
libvirt = "*"
[package.metadata.generate-rpm.recommends]
podman = "*"
[package.metadata.generate-rpm.variants.server.requires]
qemu-img = "*"
[package.metadata.generate-rpm.variants.server.recommends]
libvirt = "*"
EOF
  prepare_workload_rpm_manifest "$fixture" "$transformed"
  for section in \
    '[package.metadata.generate-rpm.requires]' \
    '[package.metadata.generate-rpm.variants.server.requires]'; do
    for dep in "${WORKLOAD_RPM_REQUIRES[@]}"; do
      count=$(awk -v section="$section" -v dep="$dep" '
        /^\[/ { inside = ($0 == section) }
        inside && $0 ~ ("^" dep "[[:space:]]*=") { count++ }
        END { print count + 0 }
      ' "$transformed")
      [ "$count" -eq 1 ] || {
        echo "workload RPM manifest self-test: $section has $count hard $dep entries" >&2
        return 1
      }
    done
  done
  grep -Fqx 'podman = "*"' "$transformed" || {
    echo "workload RPM manifest self-test: weak dependency fixture was corrupted" >&2
    return 1
  }
  echo "workload RPM manifest self-test: both compute packages hard-require all ${#WORKLOAD_RPM_REQUIRES[@]} runtime providers"
}

verify_workload_rpm_requires() {
  local rpm_path=$1 dep requires
  command -v rpm >/dev/null || {
    echo "workload RPM dependency gate: rpm query tool is unavailable" >&2
    return 1
  }
  requires=$(rpm -qp --requires "$rpm_path") || return
  for dep in "${WORKLOAD_RPM_REQUIRES[@]}"; do
    grep -Eq "^${dep}([[:space:]]|$)" <<<"$requires" || {
      echo "workload RPM dependency gate: $rpm_path lacks hard Requires: $dep" >&2
      return 1
    }
  done
  echo "workload RPM dependency gate: $rpm_path carries all ${#WORKLOAD_RPM_REQUIRES[@]} runtime providers"
}

if [ "${1:-}" = "--self-test" ]; then
  workload_rpm_manifest_self_test
  exit
fi
if [ "${1:-}" = "--prepare-workload-rpm-manifest" ]; then
  [ "$#" -eq 3 ] || {
    echo "usage: $0 --prepare-workload-rpm-manifest INPUT OUTPUT" >&2
    exit 2
  }
  prepare_workload_rpm_manifest "$2" "$3"
  exit
fi
if [ "${1:-}" = "--verify-workload-rpm-requires" ]; then
  [ "$#" -eq 2 ] || {
    echo "usage: $0 --verify-workload-rpm-requires RPM" >&2
    exit 2
  }
  verify_workload_rpm_requires "$2"
  exit
fi

# The container RPM lane is farm-only. Keep direct invocations convenient, but
# dispatch them through xcp-build so Podman and its storage never run locally.
if [ "${MCNF_FARM_REMOTE:-0}" != 1 ]; then
  SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
  exec "$SCRIPT_DIR/xcp-build.sh" container-rpm "$@"
fi

# XPA-6/DO-LIGHTHOUSE-THIN — parse the optional mode flag (position-independent) so the
# remaining positional arg stays the Fedora version (back-compat with the
# original `[fedora_version]` calling convention).
MODE="full"
ARGS=()
for a in "$@"; do
  case "$a" in
    --server) MODE="server" ;;
    --lighthouse) MODE="lighthouse" ;;
    --full)   MODE="full" ;;
    *)        ARGS+=("$a") ;;
  esac
done
set -- "${ARGS[@]+"${ARGS[@]}"}"

FEDORA="${1:-43}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"

# The outer farm dispatcher resolves this from one clean checkout before its
# git-free rsync. Never let the container infer or invent candidate identity.
: "${MCNF_BUILD_SOURCE_REVISION:?promotable RPM build requires an immutable source revision receipt}"
: "${SOURCE_DATE_EPOCH:?promotable RPM build requires the receipt commit epoch}"
"$REPO/install-helpers/source-revision-receipt.sh" --verify \
  "$MCNF_BUILD_SOURCE_REVISION" "$SOURCE_DATE_EPOCH" >/dev/null
export MCNF_BUILD_PROMOTABLE=1 MCNF_BUILD_SOURCE_REVISION SOURCE_DATE_EPOCH

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
# the native media and desktop helpers link against) and the "reproducible" cut is not. For a
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
mkdir -p /src/target-f43/generate-rpm
rm -f /src/target-f43/generate-rpm/magic-mesh*.rpm

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
# The server variant replaces the base Requires table, which previously left a
# freshly installed headless compute node without its Quadlet/libvirt runtime.
# Apply the committed deterministic transform in this disposable farm checkout;
# restore the source manifest even when cargo-generate-rpm fails.
RPM_MANIFEST=/src/crates/mesh/mackesd/Cargo.toml
RPM_MANIFEST_ORIGINAL=$(mktemp)
RPM_MANIFEST_GENERATED=$(mktemp)
cp -- "$RPM_MANIFEST" "$RPM_MANIFEST_ORIGINAL"
restore_rpm_manifest() {
  cp -- "$RPM_MANIFEST_ORIGINAL" "$RPM_MANIFEST"
  rm -f -- "$RPM_MANIFEST_ORIGINAL" "$RPM_MANIFEST_GENERATED"
}
trap restore_rpm_manifest EXIT
/src/install-helpers/build-rpm-fedora43.sh --prepare-workload-rpm-manifest \
  "$RPM_MANIFEST" "$RPM_MANIFEST_GENERATED"
mv -- "$RPM_MANIFEST_GENERATED" "$RPM_MANIFEST"
# XPA-6/DO-LIGHTHOUSE-THIN — MODE (full|server|lighthouse) is passed in via
# `podman run -e MODE=…`.
if [ "${MODE:-full}" = "lighthouse" ]; then
  echo "[f43] building THIN DigitalOcean lighthouse RPM (control plane only)"
  cargo build --release $MDE_RPM_LOCKED -p mackesd
  echo "[f43] generating thin lighthouse RPM (--variant lighthouse)"
  cargo generate-rpm -p crates/mesh/mackesd --variant lighthouse
  /src/install-helpers/verify-rpm-payload.sh size /src/target-f43/generate-rpm/magic-mesh-lighthouse-*.rpm
elif [ "${MODE:-full}" = "server" ]; then
  echo "[f43] building HEADLESS crates only (release) — no libcosmic GUIs"
  # Just the daemon + mesh-substrate crates. mde-enroll yields BOTH the
  # mde-enroll + magic-setup bins; mde-bus is the shared-bus daemon. None pull
  # libcosmic/iced, so the long GUI compile is skipped entirely.
  cargo build --release $MDE_RPM_LOCKED \
      -p mackesd -p magic-fleet -p mde-enroll -p mde-bus
  echo "[f43] generating headless RPM (--variant server)"
  cargo generate-rpm -p crates/mesh/mackesd --variant server
  echo "[f43] generating thin lighthouse RPM (--variant lighthouse)"
  cargo generate-rpm -p crates/mesh/mackesd --variant lighthouse
  /src/install-helpers/build-rpm-fedora43.sh --verify-workload-rpm-requires \
    /src/target-f43/generate-rpm/magic-mesh-server-*.rpm
  /src/install-helpers/verify-rpm-payload.sh size /src/target-f43/generate-rpm/magic-mesh-server-*.rpm
  /src/install-helpers/verify-rpm-payload.sh size /src/target-f43/generate-rpm/magic-mesh-lighthouse-*.rpm
else
  echo "[f43] building workspace (release) — this is the long part"
  cargo build --workspace --release $MDE_RPM_LOCKED
  # E12-3 DRM + E12-5 live-vdi + BUG-VIDEO-1 media-mpv — re-link the ONE shell
  # binary with the features the shipped seat needs: `drm` so it owns the bare
  # KMS/DRM seat, `live-vdi` so the Desktop surface can pump live RDP in-shell,
  # and `media-mpv` (BUG-VIDEO-1 / MEDIA-2 phase 1, docs/gpu_encoder.md) so the
  # embedded Media surface links the real mpv engine instead of silently
  # shipping FakeMpv (simulated playback, no real A/V — the live-verified
  # 2026-07-03 Eagle finding). The workspace build compiled all deps; this
  # only re-links one bin.
  echo "[f43] re-linking mde-shell-egui --features $MDE_RPM_SHELL_FEATURES"
  cargo build --release $MDE_RPM_LOCKED -p mde-shell-egui --features "$MDE_RPM_SHELL_FEATURES"
  echo "[f43] generating base RPM"
  cargo generate-rpm -p crates/mesh/mackesd
  echo "[f43] generating thin lighthouse RPM (--variant lighthouse)"
  cargo generate-rpm -p crates/mesh/mackesd --variant lighthouse
  /src/install-helpers/build-rpm-fedora43.sh --verify-workload-rpm-requires \
    /src/target-f43/generate-rpm/magic-mesh-[0-9]*.rpm
  /src/install-helpers/verify-rpm-payload.sh size /src/target-f43/generate-rpm/magic-mesh-[0-9]*.rpm
  /src/install-helpers/verify-rpm-payload.sh size /src/target-f43/generate-rpm/magic-mesh-lighthouse-*.rpm
fi

echo "[f43] DONE — artifact(s):"
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
    -e "MCNF_BUILD_SOURCE_REVISION=$MCNF_BUILD_SOURCE_REVISION" \
    -e "MCNF_BUILD_PROMOTABLE=1" \
    -e "SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH" \
    -v "$REPO:/src" \
    -v "$HOME/.cargo/registry:/root/.cargo/registry" \
    -v "$HOME/.cargo/git:/root/.cargo/git" \
    -w /src \
    "$IMAGE" bash -c "$IN_CONTAINER"

# XPA-6 / DO-LIGHTHOUSE-THIN — pick the artifacts for THIS
# mode. Exact prefixes avoid `magic-mesh-*` catching a sibling variant.
if [ "$MODE" = "lighthouse" ]; then
  GLOB="$REPO/target-f43/generate-rpm/magic-mesh-lighthouse-*.rpm"
  # shellcheck disable=SC2086,SC2012
  RPM="$(ls -1t $GLOB 2>/dev/null | head -1 || true)"
  [ -n "$RPM" ] || { echo "!! no thin lighthouse RPM produced (mode=$MODE)" >&2; exit 1; }
  "$REPO/install-helpers/verify-rpm-payload.sh" size "$RPM"
  echo
  echo "✅ Fedora $FEDORA RPM (mode=$MODE): $RPM"
  echo "   install on F$FEDORA:  sudo dnf install $RPM"
  echo "   use with DO:          do-lighthouse-up.sh <mesh> --rpm-url <served-thin-rpm>"
elif [ "$MODE" = "server" ]; then
  GLOB="$REPO/target-f43/generate-rpm/magic-mesh-server-*.rpm"
  # shellcheck disable=SC2086,SC2012  # $GLOB MUST stay unquoted to expand.
  RPM="$(ls -1t $GLOB 2>/dev/null | head -1 || true)"
  [ -n "$RPM" ] || { echo "!! no RPM produced (mode=$MODE)" >&2; exit 1; }
  "$REPO/install-helpers/verify-rpm-payload.sh" size "$RPM"
  echo
  echo "✅ Fedora $FEDORA RPM (mode=$MODE): $RPM"
  echo "   install on F$FEDORA:  sudo dnf install $RPM"
  echo "   (DO lighthouses use --lighthouse, not this server variant)"
else
  BASE_GLOB="$REPO/target-f43/generate-rpm/magic-mesh-[0-9]*.rpm"
  LIGHTHOUSE_GLOB="$REPO/target-f43/generate-rpm/magic-mesh-lighthouse-*.rpm"
  # shellcheck disable=SC2086,SC2012
  BASE_RPM="$(ls -1t $BASE_GLOB 2>/dev/null | head -1 || true)"
  # shellcheck disable=SC2086,SC2012
  LIGHTHOUSE_RPM="$(ls -1t $LIGHTHOUSE_GLOB 2>/dev/null | head -1 || true)"
  [ -n "$BASE_RPM" ] || { echo "!! no base RPM produced (mode=$MODE)" >&2; exit 1; }
  [ -n "$LIGHTHOUSE_RPM" ] || { echo "!! no thin lighthouse RPM produced (mode=$MODE)" >&2; exit 1; }
  "$REPO/install-helpers/verify-rpm-payload.sh" size "$BASE_RPM"
  "$REPO/install-helpers/verify-rpm-payload.sh" size "$LIGHTHOUSE_RPM"
  echo
  echo "✅ Fedora $FEDORA RPMs (mode=$MODE):"
  echo "   base:    $BASE_RPM"
  echo "   lighthouse: $LIGHTHOUSE_RPM"
  echo "   install on F$FEDORA:  sudo dnf install $BASE_RPM"
fi
