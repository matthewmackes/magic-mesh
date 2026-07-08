#!/usr/bin/env bash
# adopt-live-backoffice.sh — DAR-50: reconstitute the hand-built
# 172.20.145.192 backoffice into the managed control-VM model.
#
# Default mode is check/read-only: print the exact live sequence and run only
# probes that do not mutate the backoffice. Pass --live during the maintenance
# window to run `backoffice-up.sh --adopt`, verify the live state backend,
# confirm farm reachability, run the xen-xapi parity plan, and create/verify the
# DR artifact that proves reconstitution can happen from backup.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BACKOFFICE_UP="$REPO/automation/backoffice/backoffice-up.sh"
BACKOFFICE_STATUS="$REPO/automation/backoffice/backoffice-status.sh"
MIGRATE_STATE="$HERE/migrate-state.sh"
DR_BACKUP="$REPO/automation/dr/dr-backup.sh"
DR_RECONSTITUTE="$REPO/automation/dr/dr-reconstitute.sh"
TOFU_ENV="$REPO/automation/lib/tofu-env.sh"
FARM="$REPO/install-helpers/farm.sh"
TOFU_DIR="$REPO/infra/tofu"

CONTROL_IP="${MCNF_CONTROL_IP:-172.20.145.192}"
SOURCE_IP="${MCNF_LEGACY_STATE_IP:-172.20.145.192}"
TIER="${MCNF_BACKOFFICE_TIER:-full}"
ROOTS="${MCNF_DAR50_ROOTS:-xen-xapi zone1-do edgeos control-vm}"
LIVE=0
SKIP_DR=0
SKIP_PLAN=0
SKIP_FARM=0
PROVE_NO_FALLBACK=0
DR_FILE="${MCNF_DAR50_DR_FILE:-}"
BACKUP_DIR="${MCNF_STATE_MIGRATION_BACKUP_DIR:-$REPO/.state-migration-backups}"
TOFU="${MCNF_TOFU:-$(command -v tofu || command -v terraform || true)}"

usage() {
  sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
  cat <<EOF

Options:
  --live                 Run the adopt convergence and live verification.
  --control-ip <ip>      Backoffice/control endpoint (default: $CONTROL_IP).
  --source-ip <ip>       Legacy state backend for migration check (default: $SOURCE_IP).
  --tier <minimal|full>  Backoffice tier to converge (default: $TIER).
  --roots "a b"          Tofu roots to check/migrate (default: $ROOTS).
  --dr-file <path>       Existing DR artifact to verify/reconstitute.
  --skip-dr              Skip DR backup/verify.
  --skip-plan            Skip xen-xapi tofu parity plan.
  --skip-farm            Skip build farm status probe.
  --prove-no-fallback    Temporarily move /root/.mcnf-* fallback files during plan.

Default check mode mutates nothing. --live runs the DAR-50 proof sequence.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --live) LIVE=1; shift ;;
    --check) LIVE=0; shift ;;
    --control-ip) CONTROL_IP="$2"; shift 2 ;;
    --source-ip) SOURCE_IP="$2"; shift 2 ;;
    --tier) TIER="$2"; shift 2 ;;
    --roots) ROOTS="$2"; shift 2 ;;
    --dr-file) DR_FILE="$2"; shift 2 ;;
    --skip-dr) SKIP_DR=1; shift ;;
    --skip-plan) SKIP_PLAN=1; shift ;;
    --skip-farm) SKIP_FARM=1; shift ;;
    --prove-no-fallback) PROVE_NO_FALLBACK=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "adopt-live-backoffice: unknown arg: $1" >&2; usage >&2; exit 2 ;;
  esac
done

log() { echo "==> dar50: $*"; }
die() { echo "adopt-live-backoffice: $*" >&2; exit 1; }

case "$TIER" in
  minimal|full) ;;
  *) die "--tier must be minimal or full" ;;
esac

[ -x "$BACKOFFICE_UP" ] || die "missing executable $BACKOFFICE_UP"
[ -x "$BACKOFFICE_STATUS" ] || die "missing executable $BACKOFFICE_STATUS"
[ -x "$MIGRATE_STATE" ] || die "missing executable $MIGRATE_STATE"
[ -r "$TOFU_ENV" ] || die "missing $TOFU_ENV"

http_state_probe() { # <root>
  local root="$1" code
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://$CONTROL_IP:8390/state/$root" 2>/dev/null || echo 000)"
  case "$code" in
    2*) log "state backend /state/$root readable (HTTP $code)" ;;
    404) die "state backend /state/$root is missing (HTTP 404)" ;;
    *) die "state backend /state/$root not readable (HTTP $code)" ;;
  esac
}

run_state_readiness() {
  log "backoffice status on $CONTROL_IP"
  "$BACKOFFICE_STATUS" --json --host "$CONTROL_IP"
  for root in $ROOTS; do
    http_state_probe "$root"
  done
}

fallback_files=()
fallback_backup_dir=""
restore_fallbacks() {
  [ -n "$fallback_backup_dir" ] || return 0
  local f base
  for f in "${fallback_files[@]}"; do
    base="$(basename "$f")"
    [ -e "$fallback_backup_dir/$base" ] && mv "$fallback_backup_dir/$base" "$f"
  done
  rmdir "$fallback_backup_dir" 2>/dev/null || true
}

hide_fallbacks_for_plan() {
  [ "$PROVE_NO_FALLBACK" -eq 1 ] || return 0
  [ "$LIVE" -eq 1 ] || die "--prove-no-fallback is only allowed with --live"
  fallback_backup_dir="$(mktemp -d /root/mcnf-fallback-proof.XXXXXX)"
  chmod 700 "$fallback_backup_dir"
  trap restore_fallbacks EXIT
  local f
  for f in /root/.mcnf-*; do
    [ -e "$f" ] || continue
    fallback_files+=("$f")
    mv "$f" "$fallback_backup_dir/$(basename "$f")"
  done
  log "fallback proof armed: moved ${#fallback_files[@]} /root/.mcnf-* files aside until exit"
}

run_xen_parity_plan() {
  [ "$SKIP_PLAN" -eq 0 ] || { log "skip xen-xapi parity plan"; return 0; }
  [ -n "$TOFU" ] || die "neither tofu nor terraform found on PATH"
  local rootdir="$TOFU_DIR/xen-xapi"
  local backend="$rootdir/xen-xapi.backend.hcl"
  [ -d "$rootdir" ] || die "missing $rootdir"
  hide_fallbacks_for_plan
  log "generate backend configs for $CONTROL_IP"
  "$REPO/automation/state-backend/gen-backend-config.sh" --control-ip "$CONTROL_IP" --roots "$ROOTS"
  log "xen-xapi backend init"
  bash -lc ". '$TOFU_ENV'; tofu_env_load xen-xapi >/dev/null; '$TOFU' -chdir='$rootdir' init -input=false -reconfigure -backend-config='$backend'"
  log "xen-xapi parity plan (requires 0-add/0-change/0-destroy)"
  set +e
  bash -lc ". '$TOFU_ENV'; tofu_env_load xen-xapi >/dev/null; '$TOFU' -chdir='$rootdir' plan -input=false -detailed-exitcode"
  rc=$?
  set -e
  case "$rc" in
    0) log "xen-xapi parity OK (0-add/0-change/0-destroy)" ;;
    2) die "xen-xapi parity FAIL: plan proposes changes" ;;
    *) die "xen-xapi parity errored rc=$rc" ;;
  esac
}

run_farm_probe() {
  [ "$SKIP_FARM" -eq 0 ] || { log "skip farm status probe"; return 0; }
  [ -x "$FARM" ] || die "missing executable $FARM"
  log "build farm status"
  "$FARM" status
}

latest_dr_file() {
  local dir="${MCNF_DR_DIR:-$HOME/mcnf-dr-backups}"
  ls -1t "$dir"/dr-*.age 2>/dev/null | sed -n '1p'
}

run_dr_proof() {
  [ "$SKIP_DR" -eq 0 ] || { log "skip DR proof"; return 0; }
  if [ -z "$DR_FILE" ]; then
    [ "$LIVE" -eq 1 ] || { log "check mode: would run $DR_BACKUP, then verify latest dr-*.age"; return 0; }
    log "create DR backup from live store"
    "$DR_BACKUP"
    DR_FILE="$(latest_dr_file)"
  fi
  [ -n "$DR_FILE" ] || die "no DR file found; pass --dr-file or allow dr-backup.sh to create one"
  [ -f "$DR_FILE" ] || die "DR file not found: $DR_FILE"
  log "verify DR artifact $DR_FILE"
  "$DR_RECONSTITUTE" --verify "$DR_FILE"
  log "DR throwaway restore command (operator-run when a throwaway etcd is ready):"
  echo "  MCNF_ETCD=http://<throwaway-etcd>:2379 $DR_RECONSTITUTE --restore '$DR_FILE' --prefix /dar50-restore-test/"
}

print_live_sequence() {
  cat <<EOF
DAR-50 live sequence:
  MCNF_CONTROL_IP=$CONTROL_IP $BACKOFFICE_UP --tier $TIER --adopt
  $MIGRATE_STATE --control-ip $CONTROL_IP --source-ip $SOURCE_IP --roots "$ROOTS" --check
  $0 --live --control-ip $CONTROL_IP --source-ip $SOURCE_IP --tier $TIER --roots "$ROOTS"

Optional final state migration, once reviewed:
  $MIGRATE_STATE --control-ip $CONTROL_IP --source-ip $SOURCE_IP --roots "$ROOTS" --migrate
EOF
}

log "control-ip=$CONTROL_IP source-ip=$SOURCE_IP tier=$TIER roots=[$ROOTS] live=$LIVE"
if [ "$LIVE" -eq 0 ]; then
  print_live_sequence
  log "read-only adopt dry run"
  MCNF_CONTROL_IP="$CONTROL_IP" "$BACKOFFICE_UP" --tier "$TIER" --adopt --dry-run
  log "state migration check"
  "$MIGRATE_STATE" --control-ip "$CONTROL_IP" --source-ip "$SOURCE_IP" --roots "$ROOTS" --check
  run_farm_probe || true
  run_dr_proof
  log "check complete; no destructive changes performed"
  exit 0
fi

log "LIVE adopt convergence"
MCNF_CONTROL_IP="$CONTROL_IP" "$BACKOFFICE_UP" --tier "$TIER" --adopt
run_state_readiness
run_farm_probe
run_xen_parity_plan
run_dr_proof

cat <<EOF

DAR-50 live adoption proof complete for $CONTROL_IP.
Remaining operator evidence before closing the worklist item:
  - If state still lives on the legacy backend, run migrate-state.sh --migrate.
  - Restore the DR artifact into a real throwaway etcd and run the parity plan there.
  - Capture the final reconciler convergence and operator bug-hunt declaration.
EOF
