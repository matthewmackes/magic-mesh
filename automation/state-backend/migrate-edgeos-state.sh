#!/usr/bin/env bash
# migrate-edgeos-state.sh — DAR-9b: the ONE-TIME, OPERATOR-RUN, LIVE-GATED
# migration of edgeos's on-disk LOCAL tofu state into the etcd state plane at
# /tofu/state/edgeos. Defaults to --check (dry-run): it runs the prechecks and
# PRINTS the migrate command but mutates nothing. Pass --migrate (in a
# maintenance window) to actually move state.
#
# WHY gated: this is a state-MOVE (not destroy/recreate). If /tofu/state/edgeos
# already holds something, or the post-migrate plan shows ANY add/destroy against
# the live EdgeRouter reservations, the migration is unsafe and the script aborts.
#
# Prechecks (all must pass before --migrate proceeds):
#   1. an on-disk local edgeos state exists (terraform.tfstate, the source).
#   2. the target etcd key /tofu/state/edgeos is EMPTY (nothing to clobber).
#   3. the state backend answers on the control IP (DAR-7/DAR-11 ran).
# Parity gate (after migrate): `tofu plan` is 0-add / 0-change / 0-destroy.
#
# Usage:
#   migrate-edgeos-state.sh --control-ip <overlay-ip> [--migrate]
set -euo pipefail

CONTROL_IP=""
DO_MIGRATE=0
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
EDGEOS_DIR="$REPO/infra/tofu/edgeos"

while [ $# -gt 0 ]; do case "$1" in
  --control-ip) CONTROL_IP="$2"; shift 2;;
  --migrate)    DO_MIGRATE=1; shift;;
  --check)      DO_MIGRATE=0; shift;;
  -h|--help)    sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "migrate-edgeos-state: unknown arg: $1" >&2; exit 1;;
esac; done

log() { echo "==> $*"; }
die() { echo "migrate-edgeos-state: $*" >&2; exit 1; }

[ -n "$CONTROL_IP" ] || die "--control-ip <overlay-ip> is required"
TOFU="$(command -v tofu || command -v terraform || true)"
[ -n "$TOFU" ] || die "neither tofu nor terraform on PATH"

# shellcheck source=../lib/etcd-endpoints.sh
. "$REPO/automation/lib/etcd-endpoints.sh"
ENDPOINTS="$(mcnf_resolve_etcd)" || exit 1
FIRST_EP="${ENDPOINTS%%,*}"

# Precheck 1: the local source state must exist.
log "precheck 1: local edgeos state present"
[ -f "$EDGEOS_DIR/terraform.tfstate" ] \
  || die "no $EDGEOS_DIR/terraform.tfstate — nothing to migrate (edgeos already on etcd?)"

# Precheck 2: the target etcd key must be EMPTY (do not clobber existing state).
log "precheck 2: /tofu/state/edgeos is empty on etcd"
key_b64="$(printf '%s' '/tofu/state/edgeos' | base64 -w0)"
range="$(curl -fsS --max-time 5 -X POST "$FIRST_EP/v3/kv/range" \
  -d "{\"key\":\"$key_b64\"}" 2>/dev/null || true)"
# `kvs` present in the response means the key already holds state.
if printf '%s' "$range" | grep -q '"kvs"'; then
  die "/tofu/state/edgeos already has state on etcd — refusing to clobber. \
Inspect it before migrating (etcdctl get /tofu/state/edgeos)."
fi

# Precheck 3: the state backend is reachable.
log "precheck 3: state backend reachable at http://$CONTROL_IP:8390"
code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 \
  "http://$CONTROL_IP:8390/state/edgeos" || true)"
[ "$code" = "404" ] || [ "$code" = "200" ] \
  || die "state backend not answering at $CONTROL_IP:8390 (got '$code') — run state-backend-bootstrap.sh first"

# Generate the edgeos backend config from the control IP.
"$HERE/gen-backend-config.sh" --control-ip "$CONTROL_IP" --roots edgeos
CFG="$EDGEOS_DIR/edgeos.backend.hcl"

MIGRATE_CMD="$TOFU -chdir=$EDGEOS_DIR init -input=false -migrate-state -backend-config=$CFG"
PLAN_CMD="$TOFU -chdir=$EDGEOS_DIR plan -input=false -detailed-exitcode"

if [ "$DO_MIGRATE" -eq 0 ]; then
  log "CHECK mode (prechecks passed). To migrate, re-run with --migrate."
  log "  would run: $MIGRATE_CMD"
  log "  parity gate: $PLAN_CMD  (must be 0-add/0-change/0-destroy)"
  exit 0
fi

# --migrate: actually move state, then assert parity.
log "MIGRATE: $MIGRATE_CMD"
eval "$MIGRATE_CMD"

log "PARITY GATE: $PLAN_CMD"
set +e
eval "$PLAN_CMD"
rc=$?
set -e
# tofu -detailed-exitcode: 0 = no changes (parity OK), 2 = changes, 1 = error.
case "$rc" in
  0) log "PARITY OK: 0-add/0-change/0-destroy — edgeos migrated to /tofu/state/edgeos";;
  2) die "PARITY FAIL: plan proposes changes after migrate — investigate before applying (DO NOT apply)";;
  *) die "plan errored (rc=$rc)";;
esac
