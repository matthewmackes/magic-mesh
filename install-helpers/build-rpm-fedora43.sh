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
# Usage: install-helpers/build-rpm-fedora43.sh [fedora_version]   # default 43
set -euo pipefail

FEDORA="${1:-43}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="registry.fedoraproject.org/fedora:${FEDORA}"
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
dnf install -y --setopt=install_weak_deps=False \
    gcc gcc-c++ cmake pkg-config git curl findutils which gzip tar xz \
    gtk3-devel alsa-lib-devel openssl-devel opus-devel >/tmp/dnf.log 2>&1 || { tail -20 /tmp/dnf.log; exit 1; }

echo "[f43] installing rustup + the pinned toolchain"
export RUSTUP_HOME=/root/.rustup CARGO_HOME=/root/.cargo
# Pin the default to the rust-toolchain.toml channel so every cargo invocation
# (incl. `cargo install`, which ran before any repo-dir override took effect)
# resolves a version. Read the channel out of the repo so it never drifts.
CHANNEL="$(sed -n "s/^channel *= *\"\([^\"]*\)\".*/\1/p" /src/rust-toolchain.toml | head -1)"
CHANNEL="${CHANNEL:-1.94.0}"
curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain "$CHANNEL" --profile minimal >/tmp/rustup.log 2>&1
export PATH=/root/.cargo/bin:$PATH
cd /src
echo "[f43] toolchain: $(rustc --version)"

echo "[f43] installing cargo-generate-rpm"
cargo install cargo-generate-rpm --locked >/tmp/cgr.log 2>&1 || { tail -20 /tmp/cgr.log; exit 1; }

echo "[f43] building workspace (release) — this is the long part"
export CARGO_TARGET_DIR=/src/target-f43
export CMAKE_POLICY_VERSION_MINIMUM=3.5
cargo build --workspace --release --locked

echo "[f43] generating RPM"
cargo generate-rpm -p crates/mesh/mackesd

echo "[f43] DONE — artifact:"
ls -la /src/target-f43/generate-rpm/*.rpm
'

echo "==> building in $IMAGE (workspace release + RPM; reuses ~/.cargo caches)"
# --security-opt label=disable: skip SELinux confinement for this trusted
# local build so the container can read the bind-mounted repo + crate caches
# without relabeling the host trees.
podman run --rm \
    --security-opt label=disable \
    -v "$REPO:/src" \
    -v "$HOME/.cargo/registry:/root/.cargo/registry" \
    -v "$HOME/.cargo/git:/root/.cargo/git" \
    -w /src \
    "$IMAGE" bash -c "$IN_CONTAINER"

RPM="$(ls -1 "$REPO"/target-f43/generate-rpm/*.rpm 2>/dev/null | head -1 || true)"
[ -n "$RPM" ] || { echo "!! no RPM produced" >&2; exit 1; }
echo
echo "✅ Fedora $FEDORA RPM: $RPM"
echo "   install on F$FEDORA:  sudo dnf install $RPM"
echo "   or via Option A:      do-lighthouse-up.sh <mesh> --rpm-url <served-url-of-this-rpm>"
