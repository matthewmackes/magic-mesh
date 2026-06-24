#!/usr/bin/env bash
# Enable the STANDING FARM-AUTOSCALE reconciler timer on the .192 control host.
#
# This is the one step the AI cannot self-perform: installing an unattended,
# continuously-applying (FA_APPLY=1) reconcile loop on the shared dom0 fleet
# removes the per-apply human gate, so it requires an explicit operator action.
# Running this script IS that authorization.
#
# It installs the LIVE reconcile service (FA_APPLY=1) + its 5-min timer, enables
# them, and fires one tick now. Idempotent — safe to re-run. Reverse with:
#   systemctl disable --now mcnf-farm-autoscale-reconcile.timer
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "run as root: sudo bash $0"; exit 1; }

WT=/root/magic-mesh/.claude/worktrees/calm-ray-dcr8
SVC=/etc/systemd/system/mcnf-farm-autoscale-reconcile.service

cat > "$SVC" <<UNITEOF
# FARM-AUTOSCALE L2 — elastic-farm reconcile, LIVE on the .192 control host.
# Demand -> per-dom0 shapes -> (gate green) tofu apply -> build-ready (toolchain
# check + clean-baseline snapshot). Cutover 2026-06-24: always-on decommissioned,
# farm elastic from MDE-VM-golden-tc. Operator authorized continuous auto-scaling.
[Unit]
Description=MCNF FARM-AUTOSCALE reconciler (demand -> per-dom0 VM shapes; LIVE apply)
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
Environment=HOME=/root
Environment=FA_APPLY=1
WorkingDirectory=$WT
ExecStart=/bin/bash -lc 'cd $WT && source infra/tofu/env.sh && exec install-helpers/farm-reconciler.sh --once'
TimeoutStartSec=3600
Nice=10
UNITEOF

cp "$WT/packaging/systemd/mcnf-farm-autoscale-reconcile.timer" /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now mcnf-farm-autoscale-reconcile.timer
echo "== timer enabled; firing one reconcile now =="
systemctl start mcnf-farm-autoscale-reconcile.service || true
systemctl list-timers mcnf-farm-autoscale-reconcile.timer --no-pager || true
echo "ENABLED: autoscale reconciler runs every 5 min (FA_APPLY=1, OnBootSec=3min)."
