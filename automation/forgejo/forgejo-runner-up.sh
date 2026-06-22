#!/usr/bin/env bash
# forgejo-runner-up.sh — FARM-AUTO-2: install the Forgejo Actions runner
# HOST-NATIVE on the control host (NOT in a container) so workflow steps inherit
# the mesh key + the farm substrate (a containerised runner can't reach the build
# VMs / etcd / XO). Registers with the token forgejo-up.sh minted and runs as a
# systemd service.
#
# Usage:  forgejo-runner-up.sh   (run on the control host, after forgejo-up.sh)
set -euo pipefail
INSTANCE="${MCNF_FORGEJO_URL:-http://172.20.145.192:3000}"
TOKEN="${MCNF_RUNNER_TOKEN:-$(cat /var/lib/mcnf-forgejo/.runner-token 2>/dev/null)}"
VER="${MCNF_RUNNER_VERSION:-v6.3.1}"
BIN=/usr/local/bin/act_runner
WORKDIR="${MCNF_RUNNER_WORKDIR:-/var/lib/mcnf-forgejo-runner}"
REPO="${MCNF_REPO:-/root/magic-mesh}"
[ -n "$TOKEN" ] || { echo "no runner token (run forgejo-up.sh first)" >&2; exit 1; }

if [ ! -x "$BIN" ]; then
  echo "==> download act_runner $VER"
  curl -fsSL -o "$BIN" "https://code.forgejo.org/forgejo/runner/releases/download/${VER}/forgejo-runner-${VER#v}-linux-amd64"
  chmod +x "$BIN"
fi

mkdir -p "$WORKDIR"; cd "$WORKDIR"
if [ ! -f .runner ]; then
  echo "==> register runner (labels: farm:host — steps run natively on this host)"
  "$BIN" register --no-interactive --instance "$INSTANCE" --token "$TOKEN" \
    --name "control-host" --labels "farm:host"
fi

cat > /etc/systemd/system/mcnf-forgejo-runner.service <<EOF
[Unit]
Description=MCNF Forgejo Actions runner (host-native, drives the build farm)
After=network-online.target
Wants=network-online.target
[Service]
Type=simple
WorkingDirectory=$WORKDIR
Environment=HOME=/root
ExecStart=$BIN daemon
Restart=always
RestartSec=10
[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now mcnf-forgejo-runner
echo "==> runner: $(systemctl is-active mcnf-forgejo-runner)"
echo "Now mirror the repo to Forgejo + push so the .forgejo/workflows/farm-gate.yml runs:"
echo "  git -C $REPO remote add forgejo $INSTANCE/mcnfadmin/magic-mesh.git   # create the repo in the UI/API first"
echo "  git -C $REPO push forgejo master"
