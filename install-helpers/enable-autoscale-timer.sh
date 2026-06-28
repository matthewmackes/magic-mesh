#!/usr/bin/env bash
# Enable the STANDING FARM-AUTOSCALE reconciler timer with the LIVE apply armed.
#
# This is the one step the AI cannot self-perform: installing an unattended,
# continuously-applying (FA_APPLY=1) reconcile loop on the control VM removes the
# per-apply human gate, so it requires an explicit operator action. Running this
# script IS that authorization.
#
# It rewrites the LIVE reconcile service with FA_APPLY=1 (over the plan-only unit
# reconciler-up.sh installed), enables its 5-min timer, and fires one tick now.
# Idempotent — safe to re-run. Reverse with:
#   systemctl disable --now mcnf-farm-autoscale-reconcile.timer
#
# DAR-30: de-hardcoded — the repo slot is ${MCNF_REPO:-/opt/mcnf} (the DEDICATED
# release slot, NEVER the resettable .52 build dir / a .claude/worktrees path), and
# the golden template is the canonical MDE-VM-golden (the -tc drift is retired,
# DAR-34). Per-mesh values otherwise come from /etc/mcnf/reconciler.env (DAR-27).
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "run as root: sudo bash $0"; exit 1; }

# The deployed checkout — a dedicated release slot. Override with MCNF_REPO.
REPO="${MCNF_REPO:-/opt/mcnf}"
ENV_OUT="${RECONCILER_ENV_OUT:-/etc/mcnf/reconciler.env}"
SYSTEMD_DIR="${MCNF_SYSTEMD_DIR:-/etc/systemd/system}"
SVC="$SYSTEMD_DIR/mcnf-farm-autoscale-reconcile.service"

[ -d "$REPO" ] || { echo "MCNF_REPO=$REPO does not exist — set MCNF_REPO to the deployed slot" >&2; exit 1; }

cat > "$SVC" <<UNITEOF
# FARM-AUTOSCALE — elastic-farm reconcile, LIVE on the control VM (operator-armed).
# Demand -> per-dom0 shapes -> (gate green) tofu apply -> build-ready (toolchain
# check + clean-baseline snapshot). Cutover 2026-06-24: always-on decommissioned,
# farm elastic from MDE-VM-golden (DAR-34: toolchain baked in; no -tc name drift).
# Per-mesh config (etcd quorum, XAPI gate host, golden=MDE-VM-golden) from $ENV_OUT.
# Operator authorized continuous auto-scaling.
[Unit]
Description=MCNF FARM-AUTOSCALE reconciler (demand -> per-dom0 VM shapes; LIVE apply)
After=network-online.target mcnf-reconciler-bootstrap.service
Wants=network-online.target mcnf-reconciler-bootstrap.service

[Service]
Type=oneshot
Environment=HOME=/root
# THE one explicit arming: FA_APPLY=1 (over the plan-only unit). The XAPI :443 +
# tofu-state + golden prerequisites still gate each apply (farm-reconciler.sh).
Environment=FA_APPLY=1
# The canonical golden template (DAR-34) — never MDE-VM-golden-tc.
Environment=TF_VAR_golden_template_name=MDE-VM-golden
EnvironmentFile=$ENV_OUT
WorkingDirectory=$REPO
ExecStart=/bin/bash -lc 'source infra/tofu/env.sh 2>/dev/null; exec install-helpers/farm-reconciler.sh --once'
TimeoutStartSec=3600
Nice=10
UNITEOF

# The timer is static — copy it from the packaged unit if not already present.
[ -f "$SYSTEMD_DIR/mcnf-farm-autoscale-reconcile.timer" ] \
  || cp "$REPO/packaging/systemd/mcnf-farm-autoscale-reconcile.timer" "$SYSTEMD_DIR/"

systemctl daemon-reload
systemctl enable --now mcnf-farm-autoscale-reconcile.timer
echo "== timer enabled (FA_APPLY=1); firing one reconcile now =="
systemctl start mcnf-farm-autoscale-reconcile.service || true
systemctl list-timers mcnf-farm-autoscale-reconcile.timer --no-pager || true
echo "ENABLED: autoscale reconciler runs every 5 min (FA_APPLY=1, OnBootSec=3min)."
echo "golden template: MDE-VM-golden (canonical) · repo slot: $REPO"
