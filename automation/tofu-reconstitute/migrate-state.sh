#!/usr/bin/env bash
# migrate-state.sh — DAR-48: relocate live OpenTofu root state from the legacy
# .192 backend/local edgeos state onto the control VM backend.
#
# This is OPERATOR-RUN and LIVE-GATED. Default is --check: it prints the exact
# migration plan and mutates nothing. Pass --migrate during a maintenance window.
# State move only: pull/backup/re-init/push through OpenTofu, then require a
# no-change parity plan before accepting cutover.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
TOFU_DIR="$REPO/infra/tofu"
GEN_BACKEND="$REPO/automation/state-backend/gen-backend-config.sh"
EDGEOS_MIGRATE="$REPO/automation/state-backend/migrate-edgeos-state.sh"
TOFU_ENV="$REPO/automation/lib/tofu-env.sh"

SOURCE_IP="${MCNF_LEGACY_STATE_IP:-172.20.145.192}"
CONTROL_IP="${MCNF_CONTROL_IP:-}"
ROOTS="xen-xapi zone1-do edgeos control-vm"
DO_MIGRATE=0
QUIESCE=1
BACKUP_DIR="${MCNF_STATE_MIGRATION_BACKUP_DIR:-$REPO/.state-migration-backups}"
TOFU="${MCNF_TOFU:-$(command -v tofu || command -v terraform || true)}"

usage() {
  sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
  cat <<EOF

Options:
  --control-ip <ip>    Required. New control VM/state-backend overlay IP.
  --source-ip <ip>     Legacy state-backend host (default: $SOURCE_IP).
  --roots "a b"        Roots to migrate (default: $ROOTS).
  --migrate            Actually perform the move. Default is check/print only.
  --no-quiesce         Do not stop/start reconciler timers around --migrate.
  --backup-dir <dir>   Local copy of pulled states (default: $BACKUP_DIR).
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --control-ip) CONTROL_IP="$2"; shift 2 ;;
    --source-ip) SOURCE_IP="$2"; shift 2 ;;
    --roots) ROOTS="$2"; shift 2 ;;
    --migrate) DO_MIGRATE=1; shift ;;
    --check) DO_MIGRATE=0; shift ;;
    --no-quiesce) QUIESCE=0; shift ;;
    --backup-dir) BACKUP_DIR="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "migrate-state: unknown arg: $1" >&2; usage >&2; exit 2 ;;
  esac
done

log() { echo "==> migrate-state: $*"; }
die() { echo "migrate-state: $*" >&2; exit 1; }

[ -n "$CONTROL_IP" ] || die "--control-ip <overlay-ip> is required"
[ -n "$TOFU" ] || die "neither tofu nor terraform found on PATH"
[ -x "$GEN_BACKEND" ] || die "missing executable $GEN_BACKEND"
[ -r "$TOFU_ENV" ] || die "missing $TOFU_ENV"

TS="$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$BACKUP_DIR/$TS"

backend_hcl() { # <ip> <root> <file>
  local ip="$1" root="$2" out="$3"
  cat >"$out" <<EOF
address        = "http://$ip:8390/state/$root"
lock_address   = "http://$ip:8390/state/$root"
unlock_address = "http://$ip:8390/state/$root"
lock_method    = "LOCK"
unlock_method  = "UNLOCK"
EOF
}

state_backend_put_backup() { # <root> <state-file>
  local root="$1" state="$2" key="${root}-precontrol-${TS}"
  curl -fsS -X POST --data-binary "@$state" \
    "http://$CONTROL_IP:8390/state/$key" >/dev/null
  log "backup copied to control backend /tofu/state/$key"
}

root_env_prefix() { # <root>
  local root="$1"
  printf ". '%s'; tofu_env_load %q >/dev/null" "$TOFU_ENV" "$root"
}

stop_timers() {
  [ "$DO_MIGRATE" -eq 1 ] && [ "$QUIESCE" -eq 1 ] || return 0
  if command -v systemctl >/dev/null 2>&1; then
    log "quiesce reconciler timers"
    systemctl stop mcnf-farm-autoscale-reconcile.timer mcnf-farm-reconcile.timer 2>/dev/null || true
  fi
}

start_timers() {
  [ "$DO_MIGRATE" -eq 1 ] && [ "$QUIESCE" -eq 1 ] || return 0
  if command -v systemctl >/dev/null 2>&1; then
    log "resume reconciler timers"
    systemctl start mcnf-farm-autoscale-reconcile.timer mcnf-farm-reconcile.timer 2>/dev/null || true
  fi
}

trap start_timers EXIT

log "source backend: http://$SOURCE_IP:8390/state/<root>"
log "target backend: http://$CONTROL_IP:8390/state/<root>"
log "roots: $ROOTS"

# Generate target backend configs in the roots. This is safe and idempotent; in
# --check mode it gives the operator the files that --migrate will use.
"$GEN_BACKEND" --control-ip "$CONTROL_IP" --roots "$ROOTS"

if [ "$DO_MIGRATE" -eq 0 ]; then
  log "CHECK mode only. Re-run with --migrate after reviewing."
fi

stop_timers

for root in $ROOTS; do
  rootdir="$TOFU_DIR/$root"
  [ -d "$rootdir" ] || { log "skip $root (no $rootdir)"; continue; }

  if [ "$root" = "edgeos" ]; then
    if [ "$DO_MIGRATE" -eq 1 ]; then
      "$EDGEOS_MIGRATE" --control-ip "$CONTROL_IP" --migrate
    else
      "$EDGEOS_MIGRATE" --control-ip "$CONTROL_IP" --check || true
    fi
    continue
  fi

  src_cfg="$BACKUP_DIR/$TS/$root.source.backend.hcl"
  tgt_cfg="$rootdir/$root.backend.hcl"
  state_file="$BACKUP_DIR/$TS/$root.tfstate"
  backend_hcl "$SOURCE_IP" "$root" "$src_cfg"

  log "$root: source init and state pull"
  if [ "$DO_MIGRATE" -eq 1 ]; then
    bash -lc "$(root_env_prefix "$root"); '$TOFU' -chdir='$rootdir' init -input=false -reconfigure -backend-config='$src_cfg'"
    bash -lc "$(root_env_prefix "$root"); '$TOFU' -chdir='$rootdir' state pull" >"$state_file"
    [ -s "$state_file" ] || die "$root: pulled empty state"
    state_backend_put_backup "$root" "$state_file"

    log "$root: migrate backend to control VM"
    bash -lc "$(root_env_prefix "$root"); '$TOFU' -chdir='$rootdir' init -input=false -migrate-state -backend-config='$tgt_cfg'"

    log "$root: parity plan"
    set +e
    bash -lc "$(root_env_prefix "$root"); '$TOFU' -chdir='$rootdir' plan -input=false -detailed-exitcode"
    rc=$?
    set -e
    case "$rc" in
      0) log "$root: parity OK (0-add/0-change/0-destroy)" ;;
      2) die "$root: parity FAIL, plan proposes changes after migration; do not cut over" ;;
      *) die "$root: parity plan errored rc=$rc" ;;
    esac
  else
    log "  would init source: $TOFU -chdir=$rootdir init -reconfigure -backend-config=$src_cfg"
    log "  would pull state to: $state_file"
    log "  would copy backup to: http://$CONTROL_IP:8390/state/${root}-precontrol-${TS}"
    log "  would migrate: $TOFU -chdir=$rootdir init -migrate-state -backend-config=$tgt_cfg"
    log "  would parity: $TOFU -chdir=$rootdir plan -detailed-exitcode"
  fi
done

log "complete (migrate=$DO_MIGRATE); local backups under $BACKUP_DIR/$TS"
