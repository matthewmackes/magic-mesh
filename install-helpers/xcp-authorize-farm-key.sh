#!/usr/bin/env bash
# xcp-authorize-farm-key.sh — install the build-farm SSH key on an XCP-ng dom0
# so `xe` / `xcp-build.sh` (and onboard-xcp-host.sh) reach it PASSWORDLESSLY.
#
# This automates the one manual step that used to bite on every XCP host: an
# XCP-ng dom0 is password-only for root out of the box, so the farm key had to be
# pasted into /root/.ssh/authorized_keys by hand before anything could drive `xe`.
# Run this once per host (at onboarding, or to recover an existing host) and the
# farm + management tooling are keyed forever after.
#
# Idempotent + safe to re-run (the key is appended only if not already present).
#
# Usage:
#   # first time — supply the dom0 root password via env (kept out of argv):
#   XCP_PW='<dom0-root-pw>' install-helpers/xcp-authorize-farm-key.sh --host 172.20.0.9
#   # later (key already installed) — no password needed; just verifies:
#   install-helpers/xcp-authorize-farm-key.sh --host 172.20.0.9
#
# Options:
#   --host <ip>    XCP-ng dom0 management IP (required)
#   --key <pub>    public key to install (default: the farm key xcp-build.sh uses)
#   --user <u>     dom0 user (default: root)
set -euo pipefail

HOST=""; USER_="root"
# Default to the SAME key xcp-build.sh drives the farm with, so one install keys
# both management (`xe`) and the build pipeline.
PUB="${MCNF_BUILD_KEY:-$HOME/.ssh/mackes_mesh_ed25519}.pub"

while [ $# -gt 0 ]; do case "$1" in
  --host) HOST="$2"; shift 2;;
  --key)  PUB="$2";  shift 2;;
  --user) USER_="$2"; shift 2;;
  -h|--help) sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done

[ -n "$HOST" ] || { echo "--host <dom0-ip> required" >&2; exit 1; }
[ -f "$PUB" ]  || { echo "pubkey not found: $PUB" >&2; exit 1; }
KEY="$(cat "$PUB")"
PRIV="${PUB%.pub}"

log() { echo "==> xcp-authorize: $*"; }

# Append the key on the dom0 — via sshpass on the FIRST run (XCP_PW set), else
# via the key itself (already installed). The append is idempotent: `grep -qxF`
# adds the line only when the exact key isn't already authorized.
INSTALL_CMD="mkdir -p ~/.ssh && chmod 700 ~/.ssh && touch ~/.ssh/authorized_keys \
  && chmod 600 ~/.ssh/authorized_keys \
  && grep -qxF '$KEY' ~/.ssh/authorized_keys || echo '$KEY' >> ~/.ssh/authorized_keys"

log "installing farm key ($(basename "$PUB")) on $USER_@$HOST"
if [ -n "${XCP_PW:-}" ]; then
  command -v sshpass >/dev/null || { echo "sshpass not installed (needed for the first password login)" >&2; exit 1; }
  sshpass -p "$XCP_PW" ssh -o StrictHostKeyChecking=no -o ConnectTimeout=15 "$USER_@$HOST" "$INSTALL_CMD"
else
  ssh -i "$PRIV" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 "$USER_@$HOST" "$INSTALL_CMD"
fi

# Verify passwordless key-auth now works end-to-end (independent of XCP_PW), and
# that `xe` actually responds — the thing the farm + onboarding depend on.
log "verifying passwordless xe access…"
if ssh -i "$PRIV" -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=15 \
     "$USER_@$HOST" 'xe host-list --minimal >/dev/null 2>&1 && echo XE_OK' 2>/dev/null | grep -q XE_OK; then
  log "SUCCESS — $HOST is keyed: xcp-build.sh + onboard-xcp-host.sh now run passwordless"
else
  echo "==> xcp-authorize: key appended but passwordless verify FAILED — check the dom0 sshd" \
       "(PubkeyAuthentication yes) and that $PRIV matches $PUB" >&2
  exit 1
fi
