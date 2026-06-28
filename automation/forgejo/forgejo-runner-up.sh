#!/usr/bin/env bash
# forgejo-runner-up.sh — FARM-AUTO-2 / DAR-21: install the Forgejo Actions runner
# HOST-NATIVE on the control VM (NOT in a container) so workflow steps inherit the
# mesh SSH key + the farm substrate (a containerised runner can't reach the build
# VMs / etcd / the overlay). Registers with label `farm` and runs as systemd.
#
# DAR-21 corrections over the FARM-AUTO-2 original:
#   - the runner token comes from the mesh secret store (/mcnf/secret/
#     forgejo-runner-token), not a host-local $DATA/.runner-token plaintext.
#   - the instance URL targets the control VM OVERLAY IP (detected nebula/mde-neb),
#     not the hardcoded LAN .192.
#   - label is `farm` so `runs-on: farm` jobs land here and can dispatch to the fleet.
#
# Usage:  forgejo-runner-up.sh [--host <overlay-ip>]   (run on the control VM, after forgejo-up.sh)
# Env: MCNF_FORGEJO_URL (override), MCNF_RUNNER_TOKEN (override the store), MCNF_REPO,
#      MCNF_RUNNER_VERSION (v6.3.1), MCNF_RUNNER_WORKDIR (/var/lib/mcnf-forgejo-runner).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_REPO="$(cd "$HERE/../.." && pwd)"
SECRET="$SRC_REPO/automation/secrets/mcnf-secret.sh"

HOST_IP="${MCNF_HOST_IP:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    --host)    HOST_IP="$2"; shift 2 ;;
    -h|--help) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) shift ;;
  esac
done

detect_overlay() {
  ip -o -4 addr show 2>/dev/null \
    | awk '$2 ~ /nebula|mde-neb/ {split($4,a,"/"); print a[1]; exit}'
}
[ -n "$HOST_IP" ] || HOST_IP="$(detect_overlay)"

# The instance the runner registers against: the control VM overlay IP (the
# Forgejo container publishes 3000 on that IP). Overridable for tests.
if [ -n "${MCNF_FORGEJO_URL:-}" ]; then
  INSTANCE="$MCNF_FORGEJO_URL"
elif [ -n "$HOST_IP" ]; then
  INSTANCE="http://${HOST_IP}:3000"
else
  echo "forgejo-runner-up: no overlay IP found and MCNF_FORGEJO_URL/--host unset" >&2
  exit 1
fi

# DAR-21: token from the store (forgejo-up.sh persisted it there), NOT a dotfile.
TOKEN="${MCNF_RUNNER_TOKEN:-$(bash "$SECRET" get forgejo-runner-token 2>/dev/null || true)}"
[ -n "$TOKEN" ] || { echo "no runner token in /mcnf/secret/forgejo-runner-token (run forgejo-up.sh first)" >&2; exit 1; }

VER="${MCNF_RUNNER_VERSION:-v6.3.1}"
BIN=/usr/local/bin/act_runner
WORKDIR="${MCNF_RUNNER_WORKDIR:-/var/lib/mcnf-forgejo-runner}"

if [ ! -x "$BIN" ]; then
  echo "==> download act_runner $VER"
  curl -fsSL -o "$BIN" "https://code.forgejo.org/forgejo/runner/releases/download/${VER}/forgejo-runner-${VER#v}-linux-amd64"
  chmod +x "$BIN"
fi

mkdir -p "$WORKDIR"; cd "$WORKDIR"
if [ ! -f .runner ]; then
  echo "==> register runner (label: farm — steps run natively on the control VM)"
  "$BIN" register --no-interactive --instance "$INSTANCE" --token "$TOKEN" \
    --name "control-vm" --labels "farm:host"
else
  echo "==> runner already registered (idempotent)"
fi

cat > /etc/systemd/system/mcnf-forgejo-runner.service <<EOF
[Unit]
Description=MCNF Forgejo Actions runner (host-native on the control VM, drives the build farm)
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
echo "==> runner: $(systemctl is-active mcnf-forgejo-runner) (label farm, instance $INSTANCE)"
echo "Next: forgejo-seed.sh seeds the magic-mesh repo so .forgejo/workflows/ runs on label farm."
