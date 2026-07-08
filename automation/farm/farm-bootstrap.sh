#!/usr/bin/env bash
# farm-bootstrap.sh — DAR-36: one reentrant bring-up for the MCNF build farm.
#
# It coordinates the already-owned primitives in the required order:
#   1. state backend on the control overlay
#   2. shared sccache backend
#   3. canonical MDE-VM-golden presence check/build hook
#   4. xen-xapi minimal-shape tofu plan/apply
#   5. both reconciler timers, plan-only unless explicitly armed
#
# Default mode is a dry preflight. Use --live to perform safe/idempotent writes.
# Use --arm-autoscale only when the operator wants the standing FA_APPLY=1 loop.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"

LIVE=0
ARM_AUTOSCALE=0
INIT_STATE_ROOTS=0
APPLY_XEN=0
CONTROL_IP="${MCNF_CONTROL_IP:-}"
XCP_HOST="${MCNF_XCP_HOST:-}"
XCP_PASS="${XCP_PASS:-}"
QCOW2="${MCNF_GOLDEN_QCOW2:-}"
REPO_SLOT="${MCNF_REPO:-/opt/mcnf}"
MINIO_PORT="${MCNF_MINIO_PORT:-9000}"
TOFU="${MCNF_TOFU:-tofu}"
SKIP_STATE=0
SKIP_SCCACHE=0
SKIP_GOLDEN=0
SKIP_XEN=0
SKIP_TIMERS=0

usage() {
  sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
  cat <<EOF

Options:
  --live                 Perform writes. Without this, print/check only.
  --control-ip <ip>      Control VM overlay IP for state/sccache endpoints.
  --xcp-host <ip>        Founding dom0 XAPI host for golden-template checks.
  --xcp-pass <pass>      Optional first-run dom0 password for golden creation.
  --qcow2 <path>         Fedora cloud qcow2 used if MDE-VM-golden is absent.
  --repo <dir>           Deployed control-slot repo for systemd units (default: $REPO_SLOT).
  --init-state-roots     Also run tofu init -migrate-state for generated backends.
  --apply-xen            Apply the xen-xapi minimal/adopted shape after planning.
  --arm-autoscale        Rewrite/enable the autoscale timer with FA_APPLY=1.
  --skip-state           Do not run state-backend-bootstrap.sh.
  --skip-sccache         Do not run sccache-backend-up.sh.
  --skip-golden          Do not check/build MDE-VM-golden.
  --skip-xen             Do not run xen-xapi tofu init/plan/apply.
  --skip-timers          Do not install reconciler timers.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --live) LIVE=1; shift ;;
    --control-ip) CONTROL_IP="$2"; shift 2 ;;
    --xcp-host) XCP_HOST="$2"; shift 2 ;;
    --xcp-pass) XCP_PASS="$2"; shift 2 ;;
    --qcow2) QCOW2="$2"; shift 2 ;;
    --repo) REPO_SLOT="$2"; shift 2 ;;
    --init-state-roots) INIT_STATE_ROOTS=1; shift ;;
    --apply-xen) APPLY_XEN=1; shift ;;
    --arm-autoscale) ARM_AUTOSCALE=1; shift ;;
    --skip-state) SKIP_STATE=1; shift ;;
    --skip-sccache) SKIP_SCCACHE=1; shift ;;
    --skip-golden) SKIP_GOLDEN=1; shift ;;
    --skip-xen) SKIP_XEN=1; shift ;;
    --skip-timers) SKIP_TIMERS=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "farm-bootstrap: unknown arg: $1" >&2; usage >&2; exit 2 ;;
  esac
done

log() { echo "==> farm-bootstrap: $*"; }
warn() { echo "==> farm-bootstrap: $*" >&2; }
run() {
  if [ "$LIVE" -eq 1 ]; then
    log "RUN $*"
    "$@"
  else
    log "DRY $*"
  fi
}

need_file() { [ -e "$1" ] || { echo "farm-bootstrap: missing $1" >&2; exit 1; }; }
need_file "$REPO/automation/state-backend/state-backend-bootstrap.sh"
need_file "$REPO/automation/cache/sccache-backend-up.sh"
need_file "$REPO/automation/reconciler/reconciler-up.sh"
need_file "$REPO/install-helpers/setup-xcp-golden-template.sh"
need_file "$REPO/install-helpers/enable-autoscale-timer.sh"

if [ "$LIVE" -ne 1 ]; then
  log "dry preflight only; pass --live to mutate state/backend/timers"
fi
if [ "$ARM_AUTOSCALE" -eq 1 ] && [ "$LIVE" -ne 1 ]; then
  warn "--arm-autoscale ignored without --live"
  ARM_AUTOSCALE=0
fi

state_args=()
[ -n "$CONTROL_IP" ] && state_args+=(--control-ip "$CONTROL_IP")
[ "$INIT_STATE_ROOTS" -eq 1 ] && state_args+=(--init-roots)

if [ "$SKIP_STATE" -eq 0 ]; then
  log "phase 1: state backend bootstrap"
  run "$REPO/automation/state-backend/state-backend-bootstrap.sh" "${state_args[@]}"
else
  log "phase 1: skipped state backend"
fi

if [ "$SKIP_SCCACHE" -eq 0 ]; then
  log "phase 2: shared sccache backend"
  if [ "$LIVE" -eq 1 ]; then
    MCNF_CONTROL_IP="$CONTROL_IP" "$REPO/automation/cache/sccache-backend-up.sh"
  else
    log "DRY MCNF_CONTROL_IP=${CONTROL_IP:-<resolved>} $REPO/automation/cache/sccache-backend-up.sh"
  fi
else
  log "phase 2: skipped sccache backend"
fi

ssh_dom0() {
  local host="$1"; shift
  ssh -i "${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}" \
    -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=10 \
    "root@$host" "$@"
}

if [ "$SKIP_GOLDEN" -eq 0 ]; then
  log "phase 3: canonical MDE-VM-golden"
  if [ -z "$XCP_HOST" ]; then
    warn "no --xcp-host/MCNF_XCP_HOST; cannot live-check MDE-VM-golden"
  elif ssh_dom0 "$XCP_HOST" "xe template-list name-label=MDE-VM-golden --minimal | grep -q ." 2>/dev/null; then
    log "MDE-VM-golden already present on $XCP_HOST"
  elif [ -n "$QCOW2" ]; then
    run "$REPO/install-helpers/setup-xcp-golden-template.sh" --xcp-host "$XCP_HOST" ${XCP_PASS:+--xcp-pass "$XCP_PASS"} --qcow2 "$QCOW2" --name MDE-VM-golden
  else
    warn "MDE-VM-golden absent or not checkable on $XCP_HOST; provide --qcow2 to build it"
  fi
else
  log "phase 3: skipped golden template"
fi

if [ "$SKIP_XEN" -eq 0 ]; then
  log "phase 4: xen-xapi minimal/adopted farm shape"
  xen_dir="$REPO/infra/tofu/xen-xapi"
  if [ "$LIVE" -eq 1 ]; then
    "$TOFU" -chdir="$xen_dir" init -input=false
    "$TOFU" -chdir="$xen_dir" plan -input=false -out=farm-bootstrap.plan
    if [ "$APPLY_XEN" -eq 1 ]; then
      "$TOFU" -chdir="$xen_dir" apply -input=false farm-bootstrap.plan
    else
      log "xen-xapi apply not armed; rerun with --apply-xen after reviewing the plan"
    fi
  else
    log "DRY $TOFU -chdir=$xen_dir init -input=false"
    log "DRY $TOFU -chdir=$xen_dir plan -input=false -out=farm-bootstrap.plan"
  fi
else
  log "phase 4: skipped xen-xapi tofu"
fi

if [ "$SKIP_TIMERS" -eq 0 ]; then
  log "phase 5: reconciler timers"
  if [ "$LIVE" -eq 1 ]; then
    "$REPO/automation/reconciler/reconciler-up.sh" --repo "$REPO_SLOT" ${XCP_HOST:+--xcp-host "$XCP_HOST"}
    if [ "$ARM_AUTOSCALE" -eq 1 ]; then
      MCNF_REPO="$REPO_SLOT" "$REPO/install-helpers/enable-autoscale-timer.sh"
    else
      log "autoscale remains plan-only; use --arm-autoscale for FA_APPLY=1"
    fi
  else
    log "DRY $REPO/automation/reconciler/reconciler-up.sh --repo $REPO_SLOT${XCP_HOST:+ --xcp-host $XCP_HOST} --dry-run"
  fi
else
  log "phase 5: skipped reconciler timers"
fi

cat <<EOF
==> farm-bootstrap: next live proof for DAR-36
  1. On a fresh build VM: install-helpers/farm-vm-snapshot.sh snapshot <vm> --xcp-host <dom0>
  2. Confirm baseline: install-helpers/farm-vm-snapshot.sh has-clean <vm> --xcp-host <dom0>
  3. Queue two concurrent @farm jobs and verify distinct per-node flocks/hosts.
  4. Let the next reconciler tick reset an idle completed VM; verify snapshot status.
EOF
