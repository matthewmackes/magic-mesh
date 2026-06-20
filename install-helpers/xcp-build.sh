#!/usr/bin/env bash
# xcp-build.sh — farm workspace builds out to the XCP build VM, keeping heavy
# compute OFF the local AI/dev host (operator directive 2026-06-20: "only AI
# work local; farm all other work to XCP; make better use of the compute you
# have access to"). The local host kept hitting 100% disk + slow contended
# builds; the build VM (mcnf-build) is a dedicated 4-vCPU / 16 GB Fedora guest
# on the idle XCP host XEN-HOME-SERVICES (172.20.0.9).
#
# It rsyncs the working tree (dirty or clean — no commit needed) to the VM,
# runs the build there, and pulls artifacts back. target*/ stay on the VM (the
# 200 GB+ of build output never touches the local disk again).
#
# Usage:
#   xcp-build.sh sync                 rsync the working tree to the VM
#   xcp-build.sh cargo <args...>      sync + run `cargo <args>` on the VM
#   xcp-build.sh gates                sync + fmt-check + clippy + test (the ship/release gates)
#   xcp-build.sh rpm                  sync + release build + cargo generate-rpm; pull the RPM local
#   xcp-build.sh pull <remote-glob>   rsync artifacts back (relative to the remote repo)
#   xcp-build.sh shell                interactive ssh into the build VM
#
# Env overrides: MCNF_BUILD_HOST (172.20.0.50), MCNF_BUILD_USER (mm).
set -euo pipefail

BUILD_HOST="${MCNF_BUILD_HOST:-172.20.0.50}"
BUILD_USER="${MCNF_BUILD_USER:-mm}"
KEY="${MCNF_BUILD_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
REMOTE_DIR="magic-mesh"
ARTIFACTS="${MCNF_BUILD_ARTIFACTS:-$HOME/mcnf-release-artifacts}"
SSH=(ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 -o BatchMode=yes)
DEST="$BUILD_USER@$BUILD_HOST"

log() { echo "==> xcp-build: $*"; }

do_sync() {
  log "rsync working tree → $DEST:$REMOTE_DIR (excluding target*/)"
  rsync -az --delete -e "${SSH[*]}" \
    --exclude '/target' --exclude '/target-f43' --exclude '/target-f44' \
    --exclude '/.git/objects/pack/tmp_*' \
    "$REPO/" "$DEST:$REMOTE_DIR/"
}

# Run a command in the remote repo with the cargo env + the workspace config
# (mold linker, CMAKE policy) already present via the synced .cargo/config.toml.
remote() {
  "${SSH[@]}" "$DEST" "source \$HOME/.cargo/env 2>/dev/null; cd $REMOTE_DIR && $*"
}

case "${1:-}" in
  sync) do_sync ;;

  cargo) shift; do_sync; remote "cargo $*" ;;

  gates)
    do_sync
    remote "cargo fmt --all --check" \
      && remote "cargo clippy --all-targets" \
      && remote "cargo test --workspace"
    ;;

  rpm)
    do_sync
    log "release build + generate-rpm on the VM (heavy — runs on XCP, not local)"
    remote "cargo build --workspace --release && cargo generate-rpm -p crates/mesh/mackesd"
    mkdir -p "$ARTIFACTS"
    log "pulling RPM(s) → $ARTIFACTS"
    rsync -az -e "${SSH[*]}" "$DEST:$REMOTE_DIR/target/generate-rpm/*.rpm" "$ARTIFACTS/"
    ls -la "$ARTIFACTS"/*.rpm
    ;;

  pull)
    shift; mkdir -p "$ARTIFACTS"
    rsync -az -e "${SSH[*]}" "$DEST:$REMOTE_DIR/$1" "$ARTIFACTS/"
    ;;

  shell) exec "${SSH[@]}" -o BatchMode=no "$DEST" ;;

  *) sed -n '11,21p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
