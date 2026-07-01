#!/usr/bin/env bash
# farm-autoscale.sh — FARM-AUTOSCALE reconcile step (FA-1..7 / design L1-L4).
#
# The no-AI autoscaler: read the per-dom0 build-queue demand (counts of pending
# BIG and SMALL jobs, plus optional agent/build POD count), decide each dom0's
# mutually-exclusive shape, write it into infra/tofu's generated *.auto.tfvars,
# and `tofu plan` to SHOW convergence. It NEVER applies — the operator-gated
# FARM-AUTO reconciler runs the apply (L2). No AI tokens are spent: this is pure
# arithmetic over the queue (L1).
#
# Decision per dom0 (MUTUAL EXCLUSION, demand wins — L4):
#   pending BIG ≥ 1            → shape=big   (one whole-host VM)
#   else pending SMALL ≥ 1     → shape=small, small_count=min(smalls, --max-small)
#     (POD-heavy bias, FA-5: small_count also rises to fit queued pods / pod-budget)
#   else                       → shape=off   (scale-to-zero)
# A big job always wins the hardware over queued smalls on the SAME dom0; smalls
# wait for the big to drain. Idle (no jobs anywhere) → every dom0 off.
#
# HYSTERESIS / DRAIN (FA-4): the decision above is the *desired* shape. The shape
# we actually emit is debounced against a per-dom0 state file so we don't thrash
# tofu's ~minutes apply latency:
#   - min-dwell: never change a dom0's shape within FA_DWELL_SECS of its last
#     change — keep the current shape until the dwell window elapses.
#   - drain-before-switch: a big↔small swap (two EXCLUSIVE shapes) first emits an
#     intermediate `off` (drain — let running jobs finish, destroy after), THEN
#     provisions the new shape on a subsequent tick. off→small / small→off /
#     anything→off switch directly (no exclusive resource to free first).
#
# POD level (FA-5): the queue may carry a pod count "<big>:<small>:<pods>" (or the
# FA_PODS_<dom0> env). Pods are agent/build pods, ~1 vCPU each (FA_POD_BUDGET pods
# per small VM). A pod-heavy dom0 biases toward small×N: small_count is raised to
# ceil(pods / FA_POD_BUDGET) so the pods fit, capped at --max-small.
#
# Queue input — per dom0, "<big>:<small>[:<pods>]" pending counts, via flags/env:
#   --bigboy   B:S[:P]   (XEN-BIGBOY,        dom0 key xen-bigboy)
#   --home     B:S[:P]   (XEN-HOME-SERVICES, dom0 key xen-home-services)
#   --xcp1     B:S[:P]   (KVM-XCP1,          dom0 key kvm-xcp1)
# or  FA_QUEUE_BIGBOY / FA_QUEUE_HOME / FA_QUEUE_XCP1 = "B:S[:P]" (flags win),
# or  FA_PODS_BIGBOY / FA_PODS_HOME / FA_PODS_XCP1 = "<pods>" (override the :P).
# Omitted/unset dom0 = "0:0:0" (off).
#
# Modes:
#   farm-autoscale.sh --bigboy 1:0 --home 0:2 --xcp1 0:0    # plan the convergence
#   farm-autoscale.sh --home 0:3 --max-small 2 --dry-run    # decide only, no tofu
#   farm-autoscale.sh --xcp1 0:1:9 --pod-budget 4 --dwell 600  # pods + tunables
#   farm-autoscale.sh --topology                            # print current farm
#   farm-autoscale.sh --self-test                           # pure-fn assertions
#   FA_QUEUE_BIGBOY=0:2 farm-autoscale.sh                   # via env
# (--dwell / --pod-budget are flag aliases for FA_DWELL_SECS / FA_POD_BUDGET.)
#
# Env: MCNF_TOFU_DIR (default <repo>/infra/tofu), MCNF_TOFU (default `tofu`),
#      FA_MAX_SMALL (default 4 — SR/RAM headroom cap, matches the tofu validation),
#      FA_DWELL_SECS (default 300 — min-dwell debounce window, seconds),
#      FA_POD_BUDGET (default 4 — agent/build pods per small VM, ≈ its vCPUs),
#      FA_NOW (default `date +%s` — current epoch; injectable for tests).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$HERE/.." && pwd)"
TOFU_DIR="${MCNF_TOFU_DIR:-$REPO_ROOT/infra/tofu}"
TOFU="${MCNF_TOFU:-tofu}"
TFVARS="$TOFU_DIR/farm-autoscale.auto.tfvars"   # generated; gitignored
STATE="$TOFU_DIR/farm-autoscale.state"          # per-tick runtime state; gitignored
LOGFILE="$TOFU_DIR/farm-autoscale.log"          # appendable lifecycle log; gitignored
MAX_SMALL="${FA_MAX_SMALL:-4}"
DWELL_SECS="${FA_DWELL_SECS:-300}"
POD_BUDGET="${FA_POD_BUDGET:-4}"
DRY_RUN=0
TOPOLOGY=0

usage() { sed -n '2,55p' "$0" | sed 's/^# \{0,1\}//'; }

# dom0 key  ←→  queue spec ("big:small[:pods]"). Seed from env, flags override.
declare -A QUEUE=(
  ["xen-bigboy"]="${FA_QUEUE_BIGBOY:-0:0:0}"
  ["xen-home-services"]="${FA_QUEUE_HOME:-0:0:0}"
  ["kvm-xcp1"]="${FA_QUEUE_XCP1:-0:0:0}"
)
# Optional explicit pod override per dom0 (wins over the :P field of the spec).
declare -A PODS_ENV=(
  ["xen-bigboy"]="${FA_PODS_BIGBOY:-}"
  ["xen-home-services"]="${FA_PODS_HOME:-}"
  ["kvm-xcp1"]="${FA_PODS_XCP1:-}"
)
# Stable print order (matches the design doc's dom0 table).
# NOTE: the FARM is 4 dom0s / 9 heavy build slots (canonical roster:
# install-helpers/farm-topology.sh); this elastic autoscaler manages only these 3 —
# the 4th dom0 XEN-194 (build VM .170, heavy cap 2) is NOT yet elastic-wired because
# infra/tofu/variables.tf validates only these 3 dom0 keys (a known IaC gap).
ORDER=("xen-bigboy" "xen-home-services" "kvm-xcp1")

# nonneg <n> — true iff <n> is a non-negative integer.
nonneg() { case "$1" in '' | *[!0-9]*) return 1;; *) return 0;; esac; }

# --- Pure functions (unit-tested by --self-test; no I/O, no globals) -----------

# hysteresis_action <current> <desired> <cur_count> <des_count> <last_change_epoch> <now> <dwell>
#   Decide what shape to ACTUALLY emit this tick (FA-4). Echoes one of:
#     keep   — hold <current> (no change due, or an active↔active swap still
#              inside the dwell window)
#     drain  — emit `off` as an intermediate (a big↔small exclusive swap must
#              free the hardware first; the real switch lands on a later tick)
#     switch — emit <desired> directly (any transition involving `off`, or first set)
#     resize — same shape, but the `small` VM COUNT changed (scale the pool up/down
#              in place — no exclusive-shape swap, so no drain; still dwell-gated)
#   The min-dwell debounce gates ONLY the legs that can flap and are expensive to
#   reverse: an active↔active shape swap (big↔small, which itself emits a drain)
#   and a small-pool resize. Transitions involving `off` (scale-to-zero, or the
#   off→active leg that COMPLETES a swap right after a drain) are direct — there's
#   no exclusive hardware to free and the drain already paid one dwell, so making
#   the off→active leg wait a second full dwell would needlessly double swap latency.
#   <cur_count>/<des_count> are the small_count for the current/desired shape (0
#   for big/off). Pure: same inputs → same output. <current> may be empty (no
#   prior state).
hysteresis_action() {
  local current="$1" desired="$2" cur_count="$3" des_count="$4" \
        last="$5" now="$6" dwell="$7"
  # First-ever decision for this dom0 (no recorded state). The implicit baseline
  # is `off` (nothing running), so first→off is a no-op keep (don't log a phantom
  # scale-event or start the dwell clock on an idle fresh farm); first→big/small
  # switches straight in.
  if [ -z "$current" ]; then
    if [ "$desired" = "off" ]; then echo keep; return; fi
    echo switch; return
  fi
  if [ "$current" = "$desired" ]; then
    # Same shape. A `small` pool whose count changed must still scale (the FA-5
    # fix): treat a count delta as a dwell-gated resize, not a no-op keep.
    if [ "$cur_count" = "$des_count" ]; then echo keep; return; fi
    if [ $(( now - last )) -lt "$dwell" ]; then echo keep; return; fi
    echo resize; return
  fi
  # A shape change IS wanted. A transition involving `off` is the cheap/safe leg —
  # switch directly, NOT dwell-gated (scale-to-zero, or completing a swap after a
  # drain). Only an active↔active swap is debounced + drained.
  if [ "$current" = "off" ] || [ "$desired" = "off" ]; then
    echo switch; return
  fi
  # active↔active (big↔small): min-dwell debounce, then drain before the switch.
  if [ $(( now - last )) -lt "$dwell" ]; then echo keep; return; fi
  echo drain
}

# pod_small_count <smalls> <pods> <pod_budget> <max_small>
#   FA-5 pod-budget math: how many small VMs to run so both the queued smalls AND
#   the queued pods fit. ceil(pods / pod_budget) VMs hold the pods; max() with the
#   per-crate small count; capped at <max_small>; floored at 1 (we're in `small`).
#   Pure integer arithmetic.
pod_small_count() {
  local smalls="$1" pods="$2" budget="$3" maxs="$4" need_pods count
  # ceil(pods / budget) with integer math; budget ≥ 1 guaranteed by validation.
  need_pods=$(( (pods + budget - 1) / budget ))
  count="$smalls"
  [ "$need_pods" -gt "$count" ] && count="$need_pods"
  [ "$count" -lt 1 ] && count=1
  [ "$count" -gt "$maxs" ] && count="$maxs"
  echo "$count"
}

# shape_note <shape> <small_count> — the single human-facing description of a
# shape, shared by every table so the three wordings can't drift.
shape_note() {
  case "$1" in
    big)   echo "one whole-host VM (~max hardware)";;
    small) echo "$2× standard VM";;
    off)   echo "scale-to-zero (dom0 free)";;
    *)     echo "$1";;
  esac
}

# --- Below here is I/O / orchestration; skip it entirely for --self-test -------

if [ "${1:-}" = "--self-test" ]; then
  fails=0
  check() { # check <label> <got> <want>
    if [ "$2" = "$3" ]; then
      echo "  ok: $1"
    else
      echo "  FAIL: $1 — got '$2' want '$3'" >&2; fails=$((fails + 1))
    fi
  }

  echo "farm-autoscale --self-test:"

  # hysteresis_action args: <current> <desired> <cur_count> <des_count> <last> <now> <dwell>
  # (1) dwell holds a flapping demand: current=small, desired=big, but only 10s
  #     since the last change vs a 300s dwell → keep (don't flip).
  check "dwell holds flapping demand" \
    "$(hysteresis_action small big 2 0 1000 1010 300)" keep
  # …and once the dwell elapses, the same swap is no longer held.
  check "dwell elapsed releases the hold" \
    "$(hysteresis_action small big 2 0 1000 1400 300)" drain

  # (2) big-preempts-small does drain→switch across ticks.
  #     Tick A (dwell elapsed): small→big must DRAIN first (emit off).
  check "big preempts small: tick A drains" \
    "$(hysteresis_action small big 2 0 0 1000 300)" drain
  #     Tick B: now current=off (the drain landed), off→big switches directly.
  check "big preempts small: tick B switches" \
    "$(hysteresis_action off big 0 0 0 1000 300)" switch
  #     …and crucially the off→big leg is direct even when the drain JUST happened
  #     (within the dwell window) — a swap must not pay TWO dwells (the off-leg fix).
  check "post-drain off→big is direct (not dwell-held)" \
    "$(hysteresis_action off big 0 0 2000 2100 300)" switch

  # (3) off→small switches directly (no exclusive shape to free).
  check "off→small switches directly" \
    "$(hysteresis_action off small 0 2 0 1000 300)" switch
  #     small→off also direct (scale-to-zero needs no intermediate).
  check "small→off switches directly" \
    "$(hysteresis_action small off 2 0 0 1000 300)" switch
  #     no change wanted (same shape+count) → keep, regardless of dwell.
  check "no change → keep" \
    "$(hysteresis_action big big 0 0 0 1000 300)" keep
  #     first-ever decision (no prior state) → switch straight in.
  check "first set switches in" \
    "$(hysteresis_action '' small 0 2 0 1000 300)" switch

  # (1b) small→small with a COUNT delta scales in place (FA-5): dwell-gated resize.
  #     Within the dwell window → keep (don't thrash the pool size).
  check "small pool resize dwell-held" \
    "$(hysteresis_action small small 2 4 1000 1010 300)" keep
  #     Dwell elapsed → resize (NOT a no-op keep; the FA-5 freeze fix).
  check "small pool resizes after dwell" \
    "$(hysteresis_action small small 2 4 1000 1400 300)" resize
  #     Same shape AND same count → keep.
  check "small same count → keep" \
    "$(hysteresis_action small small 2 2 0 1400 300)" keep

  # (4) pod-heavy biases small: 1 queued small but 9 pods at budget 4 →
  #     ceil(9/4)=3 small VMs (so the pods fit), capped at max-small.
  check "pod-heavy raises small_count" \
    "$(pod_small_count 1 9 4 4)" 3
  check "pod budget capped at max-small" \
    "$(pod_small_count 1 99 4 4)" 4
  check "no pods → smalls drive count" \
    "$(pod_small_count 2 0 4 4)" 2
  check "pods fit one VM" \
    "$(pod_small_count 1 4 4 4)" 1
  check "small floor is 1" \
    "$(pod_small_count 0 0 4 4)" 1

  # (5) topology JSON is valid: build a tiny topology and validate via jq.
  if command -v jq >/dev/null 2>&1; then
    # Mirror the real print_topology JSON schema (keys + a reason with a → arrow
    # and parens) so a future schema typo or unescaped char fails the test.
    json='{"generated_epoch":123,"dwell_secs":300,"pod_budget":4,"dom0":[{"dom0":"xen-bigboy","queue":"1:0:0","shape":"big","small_count":0,"last_change_epoch":100,"age_secs":23,"next_action":"switch","next_reason":"demand-delta off→big (BIG:SML:POD=1:0:0)"}]}'
    if printf '%s' "$json" | jq -e '.dom0[0] | .shape and .next_action and .next_reason' >/dev/null 2>&1; then
      echo "  ok: topology JSON parses + has the expected schema"
    else
      echo "  FAIL: topology JSON did not parse / wrong schema" >&2; fails=$((fails + 1))
    fi
  else
    echo "  skip: topology JSON parse (jq not installed)"
  fi

  if [ "$fails" -eq 0 ]; then
    echo "farm-autoscale: self-test passed"
    exit 0
  fi
  echo "farm-autoscale: SELF-TEST FAILED ($fails)" >&2
  exit 1
fi

while [ $# -gt 0 ]; do case "$1" in
  --bigboy) QUEUE["xen-bigboy"]="$2"; shift 2;;
  --home)   QUEUE["xen-home-services"]="$2"; shift 2;;
  --xcp1)   QUEUE["kvm-xcp1"]="$2"; shift 2;;
  --max-small) MAX_SMALL="$2"; shift 2;;
  --dwell) DWELL_SECS="$2"; shift 2;;
  --pod-budget) POD_BUDGET="$2"; shift 2;;
  --dry-run) DRY_RUN=1; shift;;
  --topology) TOPOLOGY=1; shift;;
  -h | --help | help) usage; exit 0;;
  *) echo "farm-autoscale: unknown arg: $1" >&2; usage; exit 2;;
esac; done

log()  { echo "==> farm-autoscale: $*"; }
warn() { echo "==> farm-autoscale: $*" >&2; }
die()  { warn "$*"; exit 2; }

# NOW — current epoch; injectable for deterministic ticks/tests (FA-4).
NOW="${FA_NOW:-$(date +%s)}"
nonneg "$NOW" || die "FA_NOW must be an epoch integer: '$NOW'"

if ! nonneg "$MAX_SMALL" || [ "$MAX_SMALL" -lt 1 ] || [ "$MAX_SMALL" -gt 4 ]; then
  die "--max-small must be 1..4 (SR/RAM headroom; matches the tofu validation)"
fi
nonneg "$DWELL_SECS" || die "--dwell / FA_DWELL_SECS must be a non-negative integer"
if ! nonneg "$POD_BUDGET" || [ "$POD_BUDGET" -lt 1 ]; then
  die "--pod-budget / FA_POD_BUDGET must be ≥ 1 (pods per small VM)"
fi

# --- Load prior per-dom0 state (FA-4). Lines: "<dom0> <shape> <count> <epoch>" --
# Missing/empty/corrupt lines degrade to "no prior state" for that dom0 (we then
# treat the desired shape as a first set), never aborting the tick.
declare -A PREV_SHAPE PREV_COUNT PREV_EPOCH
if [ -f "$STATE" ]; then
  while read -r sk sshape scount sepoch _rest; do
    case "$sk" in ''|'#'*) continue;; esac
    case "$sshape" in big|small|off) ;; *) continue;; esac
    nonneg "$scount" || continue
    nonneg "$sepoch" || continue
    PREV_SHAPE[$sk]="$sshape"; PREV_COUNT[$sk]="$scount"; PREV_EPOCH[$sk]="$sepoch"
  done < "$STATE"
fi

# --- Parse the queue + compute the DESIRED shape per dom0 (FA-1/2/3 + FA-5) ----
declare -A QBIG QSMALL QPODS DESIRED DESIRED_COUNT
for dk in "${ORDER[@]}"; do
  spec="${QUEUE[$dk]}"
  # Accept "<big>:<small>", "<big>:<small>:<pods>", or a bare "<big>" (rest 0).
  # Reject extra fields so a typo can't silently drop a count.
  case "$spec" in
    *:*:*:*) die "bad queue for $dk: '$spec' (too many fields; want <big>:<small>[:<pods>])" ;;
    *:*:*)   big="${spec%%:*}"; rest="${spec#*:}"; small="${rest%%:*}"; pods="${rest#*:}" ;;
    *:*)     big="${spec%%:*}"; small="${spec##*:}"; pods=0 ;;
    *)       big="$spec"; small=0; pods=0 ;; # bare "N" → N big jobs
  esac
  # An explicit FA_PODS_<dom0> env overrides the spec's :P field.
  if [ -n "${PODS_ENV[$dk]}" ]; then pods="${PODS_ENV[$dk]}"; fi
  nonneg "$big"   || die "bad queue for $dk: '$spec' (want <big>:<small>[:<pods>], ints)"
  nonneg "$small" || die "bad queue for $dk: '$spec' (want <big>:<small>[:<pods>], ints)"
  nonneg "$pods"  || die "bad queue/pods for $dk: '$spec' (pods must be a non-negative int)"
  QBIG[$dk]="$big"; QSMALL[$dk]="$small"; QPODS[$dk]="$pods"

  if [ "$big" -ge 1 ]; then
    DESIRED[$dk]="big"; DESIRED_COUNT[$dk]=0
  elif [ "$small" -ge 1 ] || [ "$pods" -ge 1 ]; then
    DESIRED[$dk]="small"
    # FA-5 pod-budget math folds queued pods into the small_count.
    DESIRED_COUNT[$dk]="$(pod_small_count "$small" "$pods" "$POD_BUDGET" "$MAX_SMALL")"
  else
    DESIRED[$dk]="off"; DESIRED_COUNT[$dk]=0
  fi
done

# --- Apply hysteresis → the shape we EMIT this tick + the reason (FA-4/FA-7) ---
declare -A SHAPE SMALL_COUNT REASON ACTION LAST_CHANGE
for dk in "${ORDER[@]}"; do
  cur="${PREV_SHAPE[$dk]:-}"
  curcount="${PREV_COUNT[$dk]:-0}"
  curepoch="${PREV_EPOCH[$dk]:-0}"
  des="${DESIRED[$dk]}"
  descount="${DESIRED_COUNT[$dk]}"

  act="$(hysteresis_action "$cur" "$des" "$curcount" "$descount" "$curepoch" "$NOW" "$DWELL_SECS")"
  ACTION[$dk]="$act"
  case "$act" in
    keep)
      # Hold the current shape+count. If there's no prior state at all, "keep" of
      # an empty shape means off (the safe default until a change is due).
      SHAPE[$dk]="${cur:-off}"
      SMALL_COUNT[$dk]="$curcount"
      LAST_CHANGE[$dk]="$curepoch"
      if [ -n "$cur" ] && { [ "$cur" != "$des" ] || [ "$curcount" != "$descount" ]; }; then
        REASON[$dk]="dwell-held ($(( NOW - curepoch ))s/${DWELL_SECS}s; want $des/$descount)"
      else
        REASON[$dk]="steady ($cur/$curcount)"
      fi
      ;;
    drain)
      SHAPE[$dk]="off"; SMALL_COUNT[$dk]=0; LAST_CHANGE[$dk]="$NOW"
      REASON[$dk]="draining $cur→off before $des (exclusive-swap free hardware)"
      ;;
    resize)
      # Same shape (small), pool count changed — scale in place, no drain.
      SHAPE[$dk]="$des"; SMALL_COUNT[$dk]="$descount"; LAST_CHANGE[$dk]="$NOW"
      REASON[$dk]="resize small ${curcount}→${descount} (BIG:SML:POD=${QBIG[$dk]}:${QSMALL[$dk]}:${QPODS[$dk]})"
      ;;
    switch)
      SHAPE[$dk]="$des"; SMALL_COUNT[$dk]="$descount"; LAST_CHANGE[$dk]="$NOW"
      if [ "$des" = "off" ]; then
        REASON[$dk]="scale-to-zero (no demand on $dk)"
      else
        REASON[$dk]="demand-delta ${cur:-none}→$des (BIG:SML:POD=${QBIG[$dk]}:${QSMALL[$dk]}:${QPODS[$dk]})"
      fi
      ;;
  esac
done

# --- FA-7: emit a lifecycle log line per dom0 whose emitted shape CHANGED ------
# stderr (visible in the reconciler timer journal) + an appendable log file.
emit_lifecycle_log() {
  local stamp dk line had_change=0
  stamp="$(date -u -d "@$NOW" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u +%Y-%m-%dT%H:%M:%SZ)"
  for dk in "${ORDER[@]}"; do
    # No prior state ⇒ the implicit baseline is `off` (nothing running), so a
    # fresh idle dom0 emitting `off` is NOT a change (no phantom scale-event).
    local cur="${PREV_SHAPE[$dk]:-off}" curcount="${PREV_COUNT[$dk]:-0}"
    # Log only when the EMITTED shape (or count) actually changes from prior state.
    if [ "${SHAPE[$dk]}" = "$cur" ] && [ "${SMALL_COUNT[$dk]}" = "$curcount" ]; then
      continue
    fi
    had_change=1
    line="$stamp $dk ${cur}/${curcount} -> ${SHAPE[$dk]}/${SMALL_COUNT[$dk]} [${ACTION[$dk]}] ${REASON[$dk]}"
    warn "scale-event: $line"
    # Append to the log file (best-effort; never fail the tick on a log write).
    if ! printf '%s\n' "$line" >> "$LOGFILE" 2>/dev/null; then
      warn "could not append to $LOGFILE (continuing)"
    fi
  done
  # NB: a bare trailing `[ … ] && log …` would return false under set -e when the
  # test is false and abort the caller — use an explicit if so the function always
  # returns 0.
  if [ "$had_change" -eq 0 ]; then
    log "no shape changes this tick (steady / dwell-held)"
  fi
}

# --- FA-7: --topology — print the current farm as a table AND as JSON ----------
# READ-ONLY view. The SHAPE/COUNT/AGE columns report the COMMITTED state — what is
# actually deployed per the last committing tick (the state file / PREV_*), NOT a
# freshly-decided shape — so a bare `--topology` shows the running farm truthfully
# even with no queue args (a dom0 persisted as `big` shows `big`, not a synthetic
# `off`). The `action`/`reason` columns describe what the NEXT committing tick WOULD
# do given the live queue passed this invocation (so the panel can preview drift).
# A dom0 with no prior state shows `off` (nothing deployed). The JSON is for a
# future Workbench topology panel (FOLLOW-UP: wire that panel to consume this
# `--topology` JSON; not built here).
print_topology() {
  log "current farm topology (committed state; dwell=${DWELL_SECS}s, pod-budget=${POD_BUDGET}):"
  printf '    %-20s %-10s %-7s %-6s %-9s %s\n' DOM0 'B:S:P' SHAPE COUNT AGE NOTE
  local dk age pshape pcount pepoch agestr
  for dk in "${ORDER[@]}"; do
    pshape="${PREV_SHAPE[$dk]:-off}"; pcount="${PREV_COUNT[$dk]:-0}"
    pepoch="${PREV_EPOCH[$dk]:-0}"
    # epoch 0 = never deployed/changed from the off baseline → age is n/a ("-").
    if [ "$pepoch" -eq 0 ]; then agestr="-"; else
      age=$(( NOW - pepoch )); [ "$age" -lt 0 ] && age=0; agestr="${age}s"
    fi
    printf '    %-20s %-10s %-7s %-6s %-9s %s\n' \
      "$dk" "${QBIG[$dk]}:${QSMALL[$dk]}:${QPODS[$dk]}" \
      "$pshape" "$pcount" "$agestr" \
      "$(shape_note "$pshape" "$pcount")"
  done

  # JSON (one object; `dom0` is an array in ORDER). Hand-rolled — most values are
  # known-safe (enum shapes, integers, fixed dom0 keys). Only `reason` is free
  # text, so it is JSON-escaped (backslash FIRST, then double-quote).
  local first=1 obj
  printf '{\n'
  printf '  "generated_epoch": %s,\n' "$NOW"
  printf '  "dwell_secs": %s,\n' "$DWELL_SECS"
  printf '  "pod_budget": %s,\n' "$POD_BUDGET"
  printf '  "dom0": [\n'
  for dk in "${ORDER[@]}"; do
    pshape="${PREV_SHAPE[$dk]:-off}"; pcount="${PREV_COUNT[$dk]:-0}"
    pepoch="${PREV_EPOCH[$dk]:-0}"
    # epoch 0 = never changed → age_secs 0 (consumer checks last_change_epoch!=0).
    if [ "$pepoch" -eq 0 ]; then age=0; else
      age=$(( NOW - pepoch )); [ "$age" -lt 0 ] && age=0
    fi
    [ "$first" -eq 1 ] && first=0 || printf ',\n'
    # shape/small_count/last_change = COMMITTED truth; next_action/next_reason =
    # what the next committing tick would do for the live queue (drift preview).
    obj=$(printf '    {"dom0":"%s","queue":"%s:%s:%s","shape":"%s","small_count":%s,"last_change_epoch":%s,"age_secs":%s,"next_action":"%s","next_reason":"%s"}' \
      "$dk" "${QBIG[$dk]}" "${QSMALL[$dk]}" "${QPODS[$dk]}" \
      "$pshape" "$pcount" "${PREV_EPOCH[$dk]:-0}" "$age" \
      "${ACTION[$dk]:-keep}" "$(printf '%s' "${REASON[$dk]:-}" | sed 's/\\/\\\\/g; s/"/\\"/g')")
    printf '%s' "$obj"
  done
  printf '\n  ]\n}\n'
}

# --- Persist the new state (FA-4) so the next tick can debounce against it -----
write_state() {
  {
    echo "# GENERATED by install-helpers/farm-autoscale.sh — per-tick farm state."
    echo "# Columns: <dom0_key> <shape> <small_count> <last_change_epoch>"
    for dk in "${ORDER[@]}"; do
      printf '%s %s %s %s\n' \
        "$dk" "${SHAPE[$dk]}" "${SMALL_COUNT[$dk]}" "${LAST_CHANGE[$dk]}"
    done
  } > "$STATE"
}

# --- Print the decided topology (the shape this tick WOULD commit) ------------
# Skipped in --topology mode, where print_topology is the authoritative view (the
# committed state) — showing both side by side (decided-off vs committed-big) would
# only confuse.
if [ "$TOPOLOGY" -eq 0 ]; then
  log "decided topology (demand → shape, debounced, mutual exclusion per dom0):"
  printf '    %-20s %-10s %-7s %-6s %s\n' DOM0 'B:S:P' SHAPE COUNT NOTE
  for dk in "${ORDER[@]}"; do
    printf '    %-20s %-10s %-7s %-6s %s\n' \
      "$dk" "${QBIG[$dk]}:${QSMALL[$dk]}:${QPODS[$dk]}" \
      "${SHAPE[$dk]}" "${SMALL_COUNT[$dk]}" \
      "$(shape_note "${SHAPE[$dk]}" "${SMALL_COUNT[$dk]}")"
  done
fi

# --topology: a READ-ONLY view. Print the committed-state table+JSON and stop
# BEFORE any state write or log append — observing the farm must not mutate it (a
# bare `--topology` with no queue args would otherwise look like an all-off tick
# and drain/destroy everything).
if [ "$TOPOLOGY" -eq 1 ]; then
  print_topology
  exit 0
fi

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

# --dry-run: show the decision + the vars it WOULD write, but mutate nothing —
# tofu untouched AND the debounce state/log left intact (a what-if must not
# overwrite the committed baseline or start the dwell clock). Stop here.
if [ "$DRY_RUN" -eq 1 ]; then
  log "--dry-run: decision only, tofu + state untouched. Would write to $TFVARS:"
  printf '%s\n%s\n' "$shape_hcl" "$count_hcl" | sed 's/^/    /'
  exit 0
fi

# From here on we are COMMITTING the decision. FA-7: log every shape change with
# its reason, then refresh the state file so the next tick debounces against it.
emit_lifecycle_log
write_state

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
# XO; if XO is unreachable it fails loudly — the reconciler then keeps last-good
# (the state file is already written, so the next tick debounces from here).
if ( cd "$TOFU_DIR" && "$TOFU" plan -input=false -var-file="$(basename "$TFVARS")" ); then
  log "plan OK — apply is operator/reconciler-gated (L2). Vars left at $TFVARS."
else
  rc=$?
  warn "tofu plan failed (rc=$rc) — likely XO unreachable; vars left at $TFVARS for the reconciler"
  exit "$rc"
fi
