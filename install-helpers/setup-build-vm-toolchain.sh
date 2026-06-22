#!/usr/bin/env bash
# setup-build-vm-toolchain.sh — install the full MCNF Rust build toolchain on a
# fresh Fedora build VM (BUILD-FARM-2), driven over SSH from the dev host.
# Idempotent; run after setup-xcp-build-vm.sh creates + boots the VM.
#
# The system dev libs mirror what the workspace links: the cosmic/iced GUI
# (gtk3/xkbcommon), the audio chain (alsa + opus via audiopus_sys' vendored
# build needs cmake + a C++ compiler), etcd-client (protobuf), and release
# packaging (rpm-build/createrepo_c/cargo-generate-rpm). `mold` is the fast
# linker the `.cargo/config.toml` selects.
#
# Usage:
#   setup-build-vm-toolchain.sh [--host 172.20.0.50] [--user mm] [--key <priv>]
set -euo pipefail

HOST="172.20.0.50"; USER_="mm"
KEY="${MCNF_BUILD_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
RUST_VER="1.94.0"
while [ $# -gt 0 ]; do case "$1" in
  --host) HOST="$2"; shift 2;;
  --user) USER_="$2"; shift 2;;
  --key)  KEY="$2";  shift 2;;
  -h|--help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done

SSH=(ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=20 "$USER_@$HOST")
log() { echo "==> toolchain: $*"; }

log "waiting for $USER_@$HOST to answer SSH"
for i in $(seq 1 30); do
  "${SSH[@]}" true 2>/dev/null && break
  [ "$i" = 30 ] && { echo "VM $HOST not reachable over SSH" >&2; exit 1; }
  sleep 5
done

log "system dev libs (Fedora dnf)"
"${SSH[@]}" 'sudo dnf install -y \
    gcc gcc-c++ cmake mold git rsync pkgconf-pkg-config genisoimage cloud-utils \
    protobuf-compiler openssl-devel \
    alsa-lib-devel opus-devel \
    gtk3-devel libxkbcommon-devel'

log "rustup + pinned Rust $RUST_VER + clippy/rustfmt"
"${SSH[@]}" "command -v rustup >/dev/null || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain $RUST_VER
  . \"\$HOME/.cargo/env\"; rustup toolchain install $RUST_VER; rustup default $RUST_VER; rustup component add clippy rustfmt"

log "cargo-generate-rpm (release packaging)"
"${SSH[@]}" '. "$HOME/.cargo/env"; cargo install cargo-generate-rpm --version 0.21.0 || true'

log "verify"
"${SSH[@]}" '. "$HOME/.cargo/env"
  echo "  rustc:   $(rustc --version 2>/dev/null)"
  echo "  g++:     $(g++ --version 2>/dev/null | head -1)"
  echo "  cmake:   $(cmake --version 2>/dev/null | head -1)"
  echo "  mold:    $(mold --version 2>/dev/null | head -1)"
  echo "  opus:    $(pkg-config --modversion opus 2>/dev/null)"'
log "DONE — build VM $HOST is toolchain-ready (drive builds with install-helpers/xcp-build.sh)"
