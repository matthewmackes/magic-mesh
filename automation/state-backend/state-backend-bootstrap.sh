#!/usr/bin/env bash
# state-backend-bootstrap.sh — DAR-11: the ordered come-along hook that stands the
# Tofu state backend up in the CORRECT order and breaks the chicken-and-egg
# deterministically. Idempotent; safe to re-run.
#
# Order (design §2.2 / §2.7 PHASE 0–2):
#   PHASE 0  PRECHECK   the runner is an enrolled OVERLAY member AND ≥1 etcd
#                       quorum member (resolved from /etc/mackesd/etcd-endpoints
#                       via DAR-1b) answers /version. The founder etcd binds the
#                       lighthouse OVERLAY IP and is reachable only over Nebula,
#                       so a non-member runner can NEVER reach it — fail loud with
#                       the `mackesd join` remediation BEFORE touching anything.
#   PHASE 1  SERVICE    state-backend-up.sh (overlay bind, endpoints from file).
#   PHASE 2  BACKENDS   gen-backend-config.sh writes each root's <root>.backend.hcl
#                       from the control IP, then `tofu init -migrate-state` per
#                       root so they relocate onto the etcd backend.
#
# This script does the SAFE half by default (precheck + service + generate). The
# per-root `tofu init -migrate-state` is LIVE-GATED behind --init-roots because it
# mutates live state; without the flag it only PRINTS the init commands.
#
# Usage:
#   state-backend-bootstrap.sh --control-ip <overlay-ip> [--init-roots] \
#                              [--roots "r1 r2 ..."] [--skip-service]
set -euo pipefail

CONTROL_IP=""
ROOTS="xen-xapi zone1-do edgeos control-vm"
INIT_ROOTS=0
SKIP_SERVICE=0
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
TOFU_DIR="$REPO/infra/tofu"

while [ $# -gt 0 ]; do case "$1" in
  --control-ip) CONTROL_IP="$2"; shift 2;;
  --roots)      ROOTS="$2"; shift 2;;
  --init-roots) INIT_ROOTS=1; shift;;
  --skip-service) SKIP_SERVICE=1; shift;;
  -h|--help)    sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "state-backend-bootstrap: unknown arg: $1" >&2; exit 1;;
esac; done

log() { echo "==> $*"; }
die() { echo "state-backend-bootstrap: $*" >&2; exit 1; }

# shellcheck source=../lib/etcd-endpoints.sh
. "$REPO/automation/lib/etcd-endpoints.sh"

# ---- PHASE 0: precheck -----------------------------------------------------
log "PHASE 0 precheck: overlay membership + founder etcd reachability"

# 0a. this runner must own a nebula/mde-neb overlay interface (it can only reach
#     the founder etcd, which binds the lighthouse overlay IP, over Nebula).
overlay_ip="$(ip -o -4 addr show 2>/dev/null \
  | awk '$2 ~ /nebula|mde-neb/ {split($4,a,"/"); print a[1]; exit}')"
[ -n "$overlay_ip" ] || die \
  "not an overlay member (no nebula/mde-neb interface). The founder etcd is only
  reachable over Nebula. Remediation: run \`mackesd join <token> --role server\`
  on this node BEFORE bootstrapping the state backend."

# 0b. resolve the quorum endpoints (DAR-1b) and probe each /version; require ≥1.
endpoints="$(mcnf_resolve_etcd)" || exit 1   # fail-loud already printed by the lib
reachable=0
IFS=',' read -ra eps <<< "$endpoints"
for ep in "${eps[@]}"; do
  ep="${ep// /}"
  [ -n "$ep" ] || continue
  if curl -fsS --max-time 5 "$ep/version" >/dev/null 2>&1; then
    log "  etcd OK: $ep"
    reachable=$((reachable + 1))
  else
    log "  etcd UNREACHABLE: $ep"
  fi
done
[ "$reachable" -ge 1 ] || die \
  "no etcd quorum member answered /version (endpoints: $endpoints). Check the
  overlay route to the lighthouses and that setup-etcd.sh has run."
log "PHASE 0 OK: overlay $overlay_ip, $reachable/${#eps[@]} etcd members reachable"

# Default the control IP to this node's overlay IP if not given (the state backend
# binds here; the four roots will point at it).
CONTROL_IP="${CONTROL_IP:-$overlay_ip}"

# ---- PHASE 1: service ------------------------------------------------------
if [ "$SKIP_SERVICE" -eq 0 ]; then
  log "PHASE 1: state-backend-up.sh (overlay bind $overlay_ip:8390)"
  STATE_BACKEND_BIND="$overlay_ip" MCNF_ETCD="$endpoints" \
    "$HERE/state-backend-up.sh"
  # readiness: the backend answers a GET for a nonexistent root with 404.
  for _ in $(seq 1 15); do
    code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 \
      "http://$overlay_ip:8390/state/__readiness__" || true)"
    [ "$code" = "404" ] && break
    sleep 1
  done
  log "PHASE 1 OK: state backend serving on $overlay_ip:8390 (readiness=$code)"
else
  log "PHASE 1: --skip-service (assuming the backend is already up)"
fi

# ---- PHASE 2: backends -----------------------------------------------------
log "PHASE 2: generate per-root backend config (control-ip $CONTROL_IP)"
"$HERE/gen-backend-config.sh" --control-ip "$CONTROL_IP" --roots "$ROOTS"

for root in $ROOTS; do
  rootdir="$TOFU_DIR/$root"
  [ -d "$rootdir" ] || { log "  skip $root (no $rootdir)"; continue; }
  cfg="$rootdir/$root.backend.hcl"
  initcmd="tofu -chdir=$rootdir init -input=false -migrate-state -backend-config=$cfg"
  if [ "$INIT_ROOTS" -eq 1 ]; then
    log "PHASE 2: $initcmd"
    eval "$initcmd"
  else
    log "PHASE 2 (PLAN-ONLY, --init-roots to run): $initcmd"
  fi
done

log "state-backend-bootstrap complete (init-roots=$INIT_ROOTS)"
