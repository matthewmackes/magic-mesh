#!/usr/bin/env bash
# farm-autoscale.sh — FARM-AUTOSCALE reconcile step (FA-3 / design L1-L4).
#
# The no-AI autoscaler: read the per-dom0 build-queue demand (counts of pending
# BIG and SMALL jobs), decide each dom0's mutually-exclusive shape, write it into
# infra/tofu's generated *.auto.tfvars, and `tofu plan` to SHOW convergence. It
# NEVER applies — the operator-gated FARM-AUTO reconciler runs the apply (L2). No
# AI tokens are spent: this is pure arithmetic over the queue (L1).
#
# Decision per dom0 (MUTUAL EXCLUSION, demand wins — L4):
#   pending BIG ≥ 1            → shape=big   (one whole-host VM)
#   else pending SMALL ≥ 1     → shape=small, small_count=min(smalls, --max-small)
#   else                       → shape=off   (scale-to-zero)
# A big job always wins the hardware over queued smalls on the SAME dom0; smalls
# wait for the big to drain. Idle (no jobs anywhere) → every dom0 off.
#
# Queue input — per dom0, "<big>:<small>" pending counts, via flags or env:
#   --bigboy   B:S   (XEN-BIGBOY,        dom0 key xen-bigboy)
#   --home     B:S   (XEN-HOME-SERVICES, dom0 key xen-home-services)
#   --xcp1     B:S   (KVM-XCP1,          dom0 key kvm-xcp1)
# or  FA_QUEUE_BIGBOY / FA_QUEUE_HOME / FA_QUEUE_XCP1 = "B:S" (flags win).
# Omitted/unset dom0 = "0:0" (off).
#
# Usage:
#   farm-autoscale.sh --bigboy 1:0 --home 0:2 --xcp1 0:0      # plan the convergence
#   farm-autoscale.sh --home 0:3 --max-small 2 --dry-run      # decide only, no tofu
#   FA_QUEUE_BIGBOY=0:2 farm-autoscale.sh                     # via env
#
# Env: MCNF_TOFU_DIR (default <repo>/infra/tofu), MCNF_TOFU (default `tofu`),
#      FA_MAX_SMALL (default 4 — SR/RAM headroom cap, matches the tofu validation).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$HERE/.." && pwd)"
TOFU_DIR="${MCNF_TOFU_DIR:-$REPO_ROOT/infra/tofu}"
TOFU="${MCNF_TOFU:-tofu}"
TFVARS="$TOFU_DIR/farm-autoscale.auto.tfvars" # generated; gitignored
MAX_SMALL="${FA_MAX_SMALL:-4}"
DRY_RUN=0

usage() { sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'; }

# dom0 key  ←→  queue spec ("big:small"). Seed from env, flags override.
declare -A QUEUE=(
  ["xen-bigboy"]="${FA_QUEUE_BIGBOY:-0:0}"
  ["xen-home-services"]="${FA_QUEUE_HOME:-0:0}"
  ["kvm-xcp1"]="${FA_QUEUE_XCP1:-0:0}"
)
# Stable print order (matches the design doc's dom0 table).
ORDER=("xen-bigboy" "xen-home-services" "kvm-xcp1")

while [ $# -gt 0 ]; do case "$1" in
  --bigboy) QUEUE["xen-bigboy"]="$2"; shift 2;;
  --home)   QUEUE["xen-home-services"]="$2"; shift 2;;
  --xcp1)   QUEUE["kvm-xcp1"]="$2"; shift 2;;
  --max-small) MAX_SMALL="$2"; shift 2;;
  --dry-run) DRY_RUN=1; shift;;
  -h | --help | help) usage; exit 0;;
  *) echo "farm-autoscale: unknown arg: $1" >&2; usage; exit 2;;
esac; done

log()  { echo "==> farm-autoscale: $*"; }
warn() { echo "==> farm-autoscale: $*" >&2; }
die()  { warn "$*"; exit 2; }

# nonneg <n> — validate a non-negative integer queue count.
nonneg() { case "$1" in '' | *[!0-9]*) return 1;; *) return 0;; esac; }

if ! nonneg "$MAX_SMALL" || [ "$MAX_SMALL" -lt 1 ] || [ "$MAX_SMALL" -gt 4 ]; then
  die "--max-small must be 1..4 (SR/RAM headroom; matches the tofu validation)"
fi

# --- Decide each dom0's shape from its queue (the core L4 arithmetic) ----------
declare -A SHAPE SMALL_COUNT
for dk in "${ORDER[@]}"; do
  spec="${QUEUE[$dk]}"
  # Accept exactly "<big>:<small>" or a bare "<big>" (small=0). Reject extra
  # fields (e.g. "1:2:3") so a typo can't silently drop a count.
  case "$spec" in
    *:*:*) die "bad queue for $dk: '$spec' (too many fields; want <big>:<small>)" ;;
    *:*)   big="${spec%%:*}"; small="${spec##*:}" ;;
    *)     big="$spec"; small=0 ;; # bare "N" → N big jobs, 0 small
  esac
  nonneg "$big"   || die "bad queue for $dk: '$spec' (want <big>:<small>, ints)"
  nonneg "$small" || die "bad queue for $dk: '$spec' (want <big>:<small>, ints)"
  if [ "$big" -ge 1 ]; then
    SHAPE[$dk]="big"; SMALL_COUNT[$dk]=0
  elif [ "$small" -ge 1 ]; then
    SHAPE[$dk]="small"
    SMALL_COUNT[$dk]=$(( small < MAX_SMALL ? small : MAX_SMALL ))
  else
    SHAPE[$dk]="off"; SMALL_COUNT[$dk]=0
  fi
done

# --- Print the decided topology -----------------------------------------------
log "decided topology (demand → shape, mutual exclusion per dom0):"
printf '    %-20s %-8s %-7s %-6s %s\n' DOM0 'BIG:SML' SHAPE COUNT NOTE
for dk in "${ORDER[@]}"; do
  note=""
  case "${SHAPE[$dk]}" in
    big)   note="one whole-host VM (~max hardware)";;
    small) note="${SMALL_COUNT[$dk]}× standard VM";;
    off)   note="scale-to-zero (dom0 free)";;
  esac
  printf '    %-20s %-8s %-7s %-6s %s\n' \
    "$dk" "${QUEUE[$dk]}" "${SHAPE[$dk]}" "${SMALL_COUNT[$dk]}" "$note"
done

# --- Emit the tofu var maps (HCL) ---------------------------------------------
# shape = { "<dom0>" = "<shape>", ... } and small_count only for the small dom0s.
shape_hcl="shape = {"
count_hcl="small_count = {"
for dk in "${ORDER[@]}"; do
  shape_hcl="$shape_hcl"$'\n'"  \"$dk\" = \"${SHAPE[$dk]}\""
  if [ "${SHAPE[$dk]}" = "small" ]; then
    count_hcl="$count_hcl"$'\n'"  \"$dk\" = ${SMALL_COUNT[$dk]}"
  fi
done
shape_hcl="$shape_hcl"$'\n'"}"
count_hcl="$count_hcl"$'\n'"}"

if [ "$DRY_RUN" -eq 1 ]; then
  log "--dry-run: decision only, tofu untouched. Would write to $TFVARS:"
  printf '%s\n%s\n' "$shape_hcl" "$count_hcl" | sed 's/^/    /'
  exit 0
fi

[ -d "$TOFU_DIR" ] || die "tofu dir not found: $TOFU_DIR (set MCNF_TOFU_DIR)"
command -v "$TOFU" >/dev/null || die "tofu not on PATH (set MCNF_TOFU): $TOFU"

log "writing generated vars → $TFVARS"
{
  echo "# GENERATED by install-helpers/farm-autoscale.sh — DO NOT EDIT."
  echo "# Demand-driven per-dom0 shapes (FARM-AUTOSCALE). Regenerated each tick."
  echo "$shape_hcl"
  echo "$count_hcl"
} > "$TFVARS"

log "tofu plan (convergence preview — NOT apply; the reconciler applies, L2):"
# -input=false so an unattended tick never blocks on a prompt. The plan reads live
# XO; if XO is unreachable it fails loudly — the reconciler then keeps last-good.
if ( cd "$TOFU_DIR" && "$TOFU" plan -input=false -var-file="$(basename "$TFVARS")" ); then
  log "plan OK — apply is operator/reconciler-gated (L2). Vars left at $TFVARS."
else
  rc=$?
  warn "tofu plan failed (rc=$rc) — likely XO unreachable; vars left at $TFVARS for the reconciler"
  exit "$rc"
fi
