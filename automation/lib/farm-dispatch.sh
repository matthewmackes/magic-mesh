#!/usr/bin/env bash
# farm-dispatch.sh — the shared, SLOT-AWARE "run a job on the fleet" core.
#
# Given a command it reserves ONE BUILD SLOT (not a whole node), rsyncs the
# working tree into that slot's own remote workspace, runs the command THERE,
# and records a JSON result. Used by every build-farm automation capability.
#
# CAPACITY MODEL — why slots, not nodes
# The canonical roster (install-helpers/farm-topology.sh) declares a heavy-build
# CAP per node (.50=2 .90=2 .130=3 .170=2 .196=1 => 10 slots). This dispatcher
# reserves per SLOT so the farm runs up to its real declared capacity. The older
# one-flock-per-node model pinned the whole fleet to 5 concurrent jobs and left
# the queue stalled behind idle capacity even when every node was healthy.
#
# SLOT ISOLATION (required for concurrency)
# Each slot gets its own lock AND its own remote workspace:
#   lock       $LOCKS/<node>-slot<N>.lock       exclusive reservation
#   workspace  ~/magic-mesh-farm-d<N>            own target/, no cross-talk
# Two jobs sharing one remote dir would `rsync --delete` over each other and
# contend on a single target/ — which is exactly why the old model had to
# serialize per node. Per-slot dirs also:
#   * match the `magic-mesh-farm-*` names install-helpers/farm-slot-gc.sh
#     reclaims, so finished slots are garbage-collectable (the previous shared
#     ~/magic-mesh was invisible to the GC and grew to 50G+ per node), and
#   * avoid the stale Forgejo clone at ~/magic-mesh, whose old origin/master can
#     revert a working tree mid-build (the hazard xcp-build.sh documents).
#
# ADMISSION (AI_GOVERNANCE.md §10.0.3) — free space is checked BEFORE sync, and
# the requirement is SHAPE-AWARE because disk, not CPU, is this farm's real
# limit: a cold whole-workspace target/ measured 54G on .170, so a node with
# 79G of /home cannot host three of them no matter what its CPU cap says.
#   whole-workspace / release / rpm job -> HEAVY headroom (default 40 GiB)
#   per-crate job                       -> LIGHT headroom (default  8 GiB)
# plus LIGHT again for every slot already reserved on that node, so concurrent
# slots cannot jointly overcommit /home. A node that fails admission is skipped
# and the next slot is tried; it is never forced to build, and no second copy of
# an already-running job is created.
#
# PLACEMENT spreads before it packs: slot 1 across all nodes (big iron first),
# then slot 2, then slot 3 — so concurrent jobs land on distinct nodes until the
# farm is genuinely full.
#
# Usage:
#   farm-dispatch.sh run <jobid> "<command>"   reserve a slot, sync+run, write result
#   farm-dispatch.sh result <jobid>            print the JSON result (if any)
#   farm-dispatch.sh nodes                     per-node reach/toolchain/space/slots
#   farm-dispatch.sh slots                     per-slot inventory + TOTAL_FREE
#   farm-dispatch.sh --self-test               pure-function assertions (no farm I/O)
#
# IDEMPOTENCE: one in-flight run per jobid. A second dispatcher asked for a job
# already running waits for the owner and adopts its result instead of building
# the same thing on a second slot.
#
# Exit codes: 0 job passed · 1 job failed · 2 usage · 75 EX_TEMPFAIL (no
# admissible free slot, or a job still owned elsewhere after the wait — the
# caller retries; farm-reconcile.sh does).
#
# Env: MCNF_BUILD_NODES (explicit node list; cap from MCNF_BUILD_SLOTS_PER_NODE),
#      MCNF_BUILD_SLOTS_PER_NODE (cap per node when MCNF_BUILD_NODES is set, 1),
#      MCNF_DISPATCH_MIN_FREE_KIB (light headroom, 8 GiB),
#      MCNF_DISPATCH_HEAVY_FREE_KIB (heavy headroom, 40 GiB),
#      MCNF_DISPATCH_DIR_BASE (remote workspace base, magic-mesh-farm),
#      MCNF_DISPATCH_JOB_WAIT_SECS (wait for a duplicate's owner, 5400),
#      MCNF_FARM_KEY, MCNF_FARM_STATE.
set -uo pipefail

KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
STATE="${MCNF_FARM_STATE:-$(cd "$(dirname "$0")/../.." && pwd)/automation/.state}"
RESULTS="$STATE/results"; LOGS="$STATE/logs"; LOCKS="$STATE/locks"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
TOPOLOGY="$REPO/install-helpers/farm-topology.sh"
mkdir -p "$RESULTS" "$LOGS" "$LOCKS"

SSH=(ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=12)
log() { echo "==> dispatch: $*" >&2; }

# Disk headroom envelope. LIGHT is one per-crate build's target/; HEAVY is a cold
# whole-workspace target/ (measured 54G live, so 40G is the admission floor, not
# a guess at the final size).
MIN_FREE_KIB="${MCNF_DISPATCH_MIN_FREE_KIB:-8388608}"
HEAVY_FREE_KIB="${MCNF_DISPATCH_HEAVY_FREE_KIB:-41943040}"
DIR_BASE="${MCNF_DISPATCH_DIR_BASE:-magic-mesh-farm}"
# How long a duplicate request waits for the in-flight owner of the same jobid
# before giving the caller an EX_TEMPFAIL. Sized for a cold workspace gate.
JOB_WAIT_SECS="${MCNF_DISPATCH_JOB_WAIT_SECS:-5400}"

# ============================================================================
# PURE helpers — no I/O, no globals; exercised by --self-test.
# ============================================================================

# node_capacity_spec — emit "<cap> <node>" lines, highest cap first (big iron
# leads so a heavy job meets BigBoy before the small pool). Sourced from the
# canonical roster; an explicit MCNF_BUILD_NODES wins and takes its per-node cap
# from MCNF_BUILD_SLOTS_PER_NODE. The literal fallback keeps a clean checkout
# runnable if the roster file is missing.
node_capacity_spec() {
  local n percap
  if [ -n "${MCNF_BUILD_NODES:-}" ]; then
    percap="${MCNF_BUILD_SLOTS_PER_NODE:-1}"
    for n in $MCNF_BUILD_NODES; do printf '%s %s\n' "$percap" "$n"; done
    return 0
  fi
  if [ -f "$TOPOLOGY" ]; then
    # shellcheck source=../../install-helpers/farm-topology.sh
    . "$TOPOLOGY"
    local i
    for i in "${!FARM_OCTETS[@]}"; do
      printf '%s 172.20.0.%s\n' "${FARM_CAPS[$i]}" "${FARM_OCTETS[$i]}"
    done | sort -k1,1nr -k2,2
    return 0
  fi
  printf '3 172.20.0.130\n2 172.20.0.50\n2 172.20.0.90\n2 172.20.0.170\n1 172.20.0.196\n'
}

# slot_plan <spec> — PURE: turn "<cap> <node>" lines into the ordered candidate
# list "<node> <slot>", SPREAD-FIRST: every node's slot 1 (in spec order), then
# every node's slot 2, and so on. Deterministic in its one argument.
slot_plan() {
  local spec="$1" maxcap=0 cap node i
  while read -r cap node; do
    [ -n "${cap:-}" ] && [ -n "${node:-}" ] || continue
    case "$cap" in ''|*[!0-9]*) continue ;; esac
    [ "$cap" -gt "$maxcap" ] && maxcap="$cap"
  done <<EOF
$spec
EOF
  for (( i = 1; i <= maxcap; i++ )); do
    while read -r cap node; do
      [ -n "${cap:-}" ] && [ -n "${node:-}" ] || continue
      case "$cap" in ''|*[!0-9]*) continue ;; esac
      [ "$i" -le "$cap" ] && printf '%s %s\n' "$node" "$i"
    done <<EOF
$spec
EOF
  done
}

# job_shape <command> — PURE: "heavy" iff the job materializes a whole-workspace
# target tree (--workspace / --release / an rpm cut), else "light". This is a DISK
# classifier and deliberately differs from xcp-build.sh's infer_shape placement
# rule: `cargo test --workspace` is placement-small but compiles everything, so
# for admission it is heavy.
job_shape() {
  local args=" $* "
  case "$args" in
    *" --workspace "*|*" --release "*|*" rpm "*|*" generate-rpm "*) printf 'heavy\n' ;;
    *) printf 'light\n' ;;
  esac
}

# required_kib <shape> <others-reserved> <light> <heavy> — PURE: the admission
# envelope. This job's own headroom by shape, plus one LIGHT unit for every slot
# already reserved on the same node, so N concurrent slots cannot jointly
# overcommit /home.
required_kib() {
  local shape="$1" others="$2" light="$3" heavy="$4" own
  case "$shape" in heavy) own="$heavy" ;; *) own="$light" ;; esac
  printf '%s\n' "$(( own + (others * light) ))"
}

# ============================================================================
# Farm probes — bounded, best-effort; a failure is always the safe "not eligible".
# ============================================================================

reachable()   { timeout 4 bash -c "cat </dev/null >/dev/tcp/$1/22" 2>/dev/null; }
toolchained() { "${SSH[@]}" -n "mm@$1" '. "$HOME/.cargo/env" 2>/dev/null; command -v cargo >/dev/null && command -v g++ >/dev/null' 2>/dev/null; }

# free_kib <node> — /home free space in KiB, or empty when unreachable/garbled.
# `-n` keeps ssh off our stdin so this is safe inside a read loop.
free_kib() {
  local v
  v="$("${SSH[@]}" -n "mm@$1" 'df -Pk "$HOME" | awk "NR == 2 { print \$4 }"' 2>/dev/null)" || return 1
  case "$v" in ''|*[!0-9]*) return 1 ;; esac
  printf '%s\n' "$v"
}

# lock_held <lockfile> — true iff some reservation currently owns it. Probed with
# a non-blocking flock on a fresh fd in a subshell, so failing to take the lock
# means another open file description owns it.
lock_held() {
  [ -e "$1" ] || return 1
  ( exec 9>"$1"; flock -n 9 ) 2>/dev/null && return 1
  return 0
}

# legacy_holds_on <node> — 1 while a pre-slot dispatcher still owns this node's
# whole-node lock, else 0. The previous model took `$LOCKS/<node>.lock` and built
# in the shared ~/magic-mesh tree, so an in-flight legacy job is invisible to the
# per-slot locks. Counting it keeps a migrating farm from oversubscribing a node
# and keeps its (large, warm) shared target/ inside the disk envelope. Becomes a
# permanent no-op once the last legacy job drains.
legacy_holds_on() {
  lock_held "$LOCKS/$1.lock" && printf '1\n' || printf '0\n'
}

# reserved_slots_on <node> [exclude-lockfile] — how many reservations this node
# currently carries: held per-slot locks plus any legacy whole-node hold. The
# caller passes its OWN lockfile as <exclude-lockfile> so the count is strictly
# "other reservations" and never ambiguous about our own fd.
reserved_slots_on() {
  local node="$1" exclude="${2:-}" held f
  held="$(legacy_holds_on "$node")"
  for f in "$LOCKS/$node"-slot*.lock; do
    [ -e "$f" ] || continue
    [ -n "$exclude" ] && [ "$f" = "$exclude" ] && continue
    lock_held "$f" && held=$(( held + 1 ))
  done
  printf '%s\n' "$held"
}

# slot_is_free <node> <slot> — read-only probe for the reporting commands.
slot_is_free() {
  lock_held "$LOCKS/$1-slot$2.lock" && return 1
  return 0
}

# reclaimable_paths_on <node> <cap> — the rebuildable trees on <node> that are
# provably idle right now. PURE with respect to the farm (it only reads locks):
#   * target/ of any slot workspace whose slot lock is FREE — no dispatcher owns
#     that slot, so nothing is building there. Our own candidate slot is excluded
#     automatically (we hold its lock), keeping OUR warm target/ intact.
#   * target/ of the legacy shared tree, only when no legacy hold exists.
# Only ever target/ — never a source tree, never a reserved slot. sccache keeps
# the rebuild cost of a dropped target/ low, which is what makes this cheap
# enough to do on demand.
reclaimable_paths_on() {
  local node="$1" cap="$2" i
  for (( i = 1; i <= cap; i++ )); do
    slot_is_free "$node" "$i" && printf '%s\n' "$DIR_BASE-d$i/target"
  done
  [ "$(legacy_holds_on "$node")" -eq 0 ] && printf '%s\n' "magic-mesh/target"
}

# reclaim_on <node> <cap> — drop the idle rebuildable trees and report the KiB
# freed. Bounded (one ssh, timeout) and best-effort: if it frees nothing the
# caller simply moves to the next slot. This is what makes the dispatcher
# self-healing on a disk-tight farm instead of merely refusing to dispatch.
reclaim_on() {
  local node="$1" cap="$2" paths before after
  paths="$(reclaimable_paths_on "$node" "$cap")"
  [ -n "$paths" ] || return 1
  before="$(free_kib "$node")" || return 1
  # `du`-free: rm is the measurement. Quoted heredoc so the list expands here,
  # not in the remote shell, and each path is removed relative to $HOME.
  timeout 300 "${SSH[@]}" -n "mm@$node" "cd \"\$HOME\" && while IFS= read -r p; do
      [ -n \"\$p\" ] || continue
      case \"\$p\" in /*|*..*) continue ;; esac
      [ -d \"\$p\" ] && rm -rf -- \"\$p\"
    done" <<<"$paths" >/dev/null 2>&1
  after="$(free_kib "$node")" || return 1
  if [ "$after" -gt "$before" ]; then
    log "  $node: reclaimed $(( (after - before) / 1048576 ))G from idle build trees"
    return 0
  fi
  return 1
}

# current_commit — the source rev exactly as a result records it, so freshness
# comparisons are apples-to-apples. A dirty tree is marked, never silently equal.
current_commit() {
  local c; c="$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  git -C "$REPO" diff --quiet 2>/dev/null || c="${c}-dirty"
  printf '%s\n' "$c"
}

# command_key <command> — stable short digest used to name the per-command lock.
command_key() { printf '%s' "$1" | sha256sum | cut -c1-16; }

# find_equivalent_result <command> <commit> — print the path of an existing
# result for the SAME command at the SAME commit. Deliberately mirrors
# farm-reconcile.sh's is_fresh(): a result counts only when the recorded commit
# equals the current one, and a dirty tree is never reusable because two runs of
# one command cannot be proven equal against uncommitted edits.
find_equivalent_result() {
  local command="$1" commit="$2" found
  case "$commit" in *-dirty|unknown) return 1 ;; esac
  found="$(python3 - "$RESULTS" "$command" "$commit" <<'PY'
import json, os, sys
results, cmd, commit = sys.argv[1], sys.argv[2], sys.argv[3]
for name in sorted(os.listdir(results)):
    if not name.endswith('.json'):
        continue
    try:
        with open(os.path.join(results, name)) as fh:
            d = json.load(fh)
    except Exception:
        continue
    if (d.get('command') == cmd and d.get('commit') == commit
            and d.get('outcome') in ('pass', 'fail')):
        print(os.path.join(results, name))
        break
PY
)" || return 1
  [ -n "$found" ] || return 1
  printf '%s\n' "$found"
}

# adopt_result <source-json> <jobid> — republish an equivalent run under this
# jobid so the caller's own result file exists (farm-reconcile.sh reads results
# per jobid, and a missing one reads as a red build). Returns the outcome's code.
adopt_result() {
  local src="$1" jobid="$2" outcome
  outcome="$(python3 - "$src" "$RESULTS/$jobid.json" "$jobid" <<'PY'
import json, sys
src, dst, jobid = sys.argv[1], sys.argv[2], sys.argv[3]
with open(src) as fh:
    d = json.load(fh)
d['jobid'] = jobid
d['reused_from'] = json.load(open(src)).get('jobid')
with open(dst, 'w') as fh:
    json.dump(d, fh)
    fh.write('\n')
print(d.get('outcome', '?'))
PY
)" || return 1
  log "job $jobid reuses an identical run at the same commit ($outcome) — no second slot spent"
  [ "$outcome" = "pass" ]
}

# ============================================================================
# run — reserve one admissible slot, sync, execute, record.
# ============================================================================
cmd_run() {
  local jobid="${1:?jobid}"; shift
  local command="$*"
  [ -n "$command" ] || { echo "empty command" >&2; return 2; }

  local shape; shape="$(job_shape "$command")"

  # IDEMPOTENCE: one in-flight run per jobid. Two supervisor trees reconciling
  # the same worklist resolve the same job id, and without this they raced into
  # one log and result file while burning two slots on identical work.
  #
  # The loser WAITS for the owner rather than returning immediately, then adopts
  # the owner's result. Returning early would leave the caller with no result
  # JSON for the job, and farm-reconcile.sh reads that as a red build and raises
  # a spurious triage task. Waiting costs nothing: no slot is held while blocked.
  local jobfd wait_start rjson owner_outcome
  wait_start="$(date +%s)"
  exec {jobfd}>"$LOCKS/job-$jobid.lock" || { echo "cannot open job lock for $jobid" >&2; return 2; }
  if ! flock -n "$jobfd"; then
    log "job $jobid already in flight — waiting for its owner instead of duplicating it"
    if ! flock -w "$JOB_WAIT_SECS" "$jobfd"; then
      exec {jobfd}>&-
      log "job $jobid still owned after ${JOB_WAIT_SECS}s — retry later"
      return 75
    fi
    rjson="$RESULTS/$jobid.json"
    if [ -f "$rjson" ] && [ "$(stat -c %Y "$rjson" 2>/dev/null || echo 0)" -ge "$wait_start" ]; then
      owner_outcome="$(python3 -c "import json;print(json.load(open('$rjson'))['outcome'])" 2>/dev/null || echo '')"
      flock -u "$jobfd"; exec {jobfd}>&-
      log "job $jobid finished as '$owner_outcome' under its owner — adopting that result"
      [ "$owner_outcome" = "pass" ]
      return $?
    fi
    # Owner released without publishing a fresh result: fall through and run it.
  fi

  # DEDUPLICATION BY COMMAND. Distinct worklist epics tag the same gate, so
  # several unique job ids can carry a byte-identical command — four copies of
  # `cargo test -p mde-collab-egui` once occupied four slots at the same commit.
  # Serialize identical commands on one lock, then reuse the finished run instead
  # of rebuilding it. Lock order is always jobid then command, so two jobs
  # sharing a command cannot deadlock.
  local ckey cfd equiv commit
  commit="$(current_commit)"
  ckey="$(command_key "$command")"
  exec {cfd}>"$LOCKS/cmd-$ckey.lock" || { echo "cannot open command lock" >&2; return 2; }
  if ! flock -n "$cfd"; then
    log "an identical command is already running — waiting to reuse its result"
    if ! flock -w "$JOB_WAIT_SECS" "$cfd"; then
      exec {cfd}>&-; flock -u "$jobfd"; exec {jobfd}>&-
      log "identical command still running after ${JOB_WAIT_SECS}s — retry later"
      return 75
    fi
  fi
  if equiv="$(find_equivalent_result "$command" "$commit")"; then
    flock -u "$cfd"; exec {cfd}>&-
    adopt_result "$equiv" "$jobid"; local adopted=$?
    flock -u "$jobfd"; exec {jobfd}>&-
    return "$adopted"
  fi

  # Materialize the candidate list first: probing inside a `read` loop that is
  # fed by a process substitution would let a stray ssh eat the plan.
  local spec; spec="$(node_capacity_spec)"
  local -a plan=()
  local line c n
  while IFS= read -r line; do [ -n "$line" ] && plan+=("$line"); done \
    < <(slot_plan "$spec")
  local -A capof=()
  while read -r c n; do [ -n "${n:-}" ] && capof["$n"]="$c"; done <<EOF
$spec
EOF

  local node="" slot="" lockfd="" lockfile="" cand_node cand_slot others need have
  for line in "${plan[@]}"; do
    cand_node="${line%% *}"; cand_slot="${line##* }"
    lockfile="$LOCKS/$cand_node-slot$cand_slot.lock"
    exec {lockfd}>"$lockfile" || continue
    if ! flock -n "$lockfd"; then exec {lockfd}>&-; lockfd=""; continue; fi
    # Reservation held. Now prove the node can actually host this job.
    if reachable "$cand_node" && toolchained "$cand_node"; then
      others="$(reserved_slots_on "$cand_node" "$lockfile")"
      need="$(required_kib "$shape" "$others" "$MIN_FREE_KIB" "$HEAVY_FREE_KIB")"
      have="$(free_kib "$cand_node")" || have=""
      # Short of space: reclaim provably idle build trees and re-probe ONCE
      # before giving up on the node. Dispatching into a node we just cleared is
      # how the farm fills instead of stalling behind stale target/ trees.
      if [ -n "$have" ] && [ "$have" -lt "$need" ]; then
        log "  $cand_node slot$cand_slot: ${have} KiB free < ${need} KiB ($shape, ${others} other) — reclaiming"
        if reclaim_on "$cand_node" "${capof[$cand_node]:-1}"; then
          have="$(free_kib "$cand_node")" || have=""
        fi
      fi
      if [ -n "$have" ] && [ "$have" -ge "$need" ]; then
        node="$cand_node"; slot="$cand_slot"; break
      fi
      log "  $cand_node slot$cand_slot: still short of ${need} KiB ($shape) — skip"
    fi
    flock -u "$lockfd"; exec {lockfd}>&-; lockfd=""
  done

  if [ -z "$node" ]; then
    flock -u "$cfd"; exec {cfd}>&-
    flock -u "$jobfd"; exec {jobfd}>&-
    log "no admissible free slot for $jobid ($shape) — all reserved/down/full; retry later"
    return 75   # EX_TEMPFAIL
  fi

  local remote_dir="$DIR_BASE-d$slot"
  local started log_file exit_code ended
  started="$(date -u +%Y-%m-%dT%H:%M:%SZ)"; log_file="$LOGS/$jobid.log"
  log "job $jobid → $node slot$slot ($shape, ~$remote_dir) : $command"

  # Sync into THIS slot's workspace. target*/ stay on the VM (warm rebuilds);
  # /.git is excluded — 1.1G of history per slot that no cargo gate needs, and
  # the same exclusion xcp-build.sh uses so a stale clone cannot reset the tree
  # mid-build. Build identity still stamps honestly (mde-theme's build.rs
  # degrades to a non-promotable marker without Git); promotable RPM cuts use
  # xcp-build.sh's immutable git-archive path, not this dispatcher.
  rsync -az --delete -e "${SSH[*]}" \
    --exclude '/target' --exclude '/target-f43' --exclude '/target-f44' \
    --exclude '/.git' --exclude '/automation/.state' \
    "$REPO/" "mm@$node:$remote_dir/" >>"$log_file" 2>&1
  "${SSH[@]}" "mm@$node" \
    ". \"\$HOME/.cargo/env\"; . \"\$HOME/.sccache.env\" 2>/dev/null || true; cd $remote_dir && $command" \
    >>"$log_file" 2>&1
  exit_code=$?
  ended="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  flock -u "$lockfd"; exec {lockfd}>&-

  local outcome="pass"; [ "$exit_code" -eq 0 ] || outcome="fail"
  # Re-read the source rev at completion (with -dirty marker) so a reconciler can
  # tell stale from fresh even if the tree moved while the job ran.
  commit="$(current_commit)"
  printf '{"jobid":"%s","outcome":"%s","exit":%d,"node":"%s","slot":%d,"workspace":"%s","shape":"%s","commit":"%s","command":%s,"started":"%s","ended":"%s","log":"%s"}\n' \
    "$jobid" "$outcome" "$exit_code" "$node" "$slot" "$remote_dir" "$shape" "$commit" \
    "$(printf '%s' "$command" | python3 -c 'import json,sys;print(json.dumps(sys.stdin.read()))')" \
    "$started" "$ended" "$log_file" > "$RESULTS/$jobid.json"
  log "job $jobid $outcome (exit $exit_code) on $node slot$slot — result $RESULTS/$jobid.json"
  # Release the command lock only after the result is published, so a waiter on
  # the same command finds it and reuses it instead of rebuilding.
  flock -u "$cfd"; exec {cfd}>&-
  flock -u "$jobfd"; exec {jobfd}>&-
  [ "$exit_code" -eq 0 ]
}

cmd_result() { cat "$RESULTS/${1:?jobid}.json" 2>/dev/null || { echo "no result for $1" >&2; return 1; }; }

# ============================================================================
# Reporting
# ============================================================================

# nodes — one row per node: reachability, toolchain, free space, and how many of
# its declared slots are free.
cmd_nodes() {
  local cap node reach tool space free_h freeslots legacy state i total_cap=0 total_free=0
  printf '  %-16s %-6s %-7s %-9s %-7s %s\n' NODE REACH TOOLCH FREE SLOTS STATE
  while read -r cap node; do
    [ -n "${cap:-}" ] && [ -n "${node:-}" ] || continue
    reach="down"; tool="-"; space="-"; free_h="-"
    if reachable "$node"; then
      reach="up"
      toolchained "$node" && tool="ready" || tool="bare"
      if space="$(free_kib "$node")"; then
        free_h="$(awk -v k="$space" 'BEGIN{printf "%.1fG", k/1048576}')"
      else
        space=0; free_h="?"
      fi
    else
      space=0
    fi
    freeslots=0
    for (( i = 1; i <= cap; i++ )); do slot_is_free "$node" "$i" && freeslots=$(( freeslots + 1 )); done
    # A legacy whole-node hold consumes real capacity the slot locks cannot see.
    legacy="$(legacy_holds_on "$node")"
    [ "$legacy" -gt 0 ] && freeslots=$(( freeslots > legacy ? freeslots - legacy : 0 ))
    total_cap=$(( total_cap + cap )); total_free=$(( total_free + freeslots ))
    # A node can be healthy yet admit nothing: report the honest reason.
    if [ "$reach" != "up" ] || [ "$tool" != "ready" ]; then state="unavailable"
    elif [ "$freeslots" -eq 0 ]; then state="saturated"
    elif [ "$space" -lt "$MIN_FREE_KIB" ]; then state="FULL(disk)"
    elif [ "$space" -lt "$HEAVY_FREE_KIB" ]; then state="light-only"
    else state="ready"; fi
    [ "$legacy" -gt 0 ] && state="$state,legacy-job"
    printf '  %-16s %-6s %-7s %-9s %-7s %s\n' \
      "$node" "$reach" "$tool" "$free_h" "$freeslots/$cap" "$state"
  done < <(node_capacity_spec)
  echo "  TOTAL_FREE=$total_free of $total_cap slots"
}

# slots — the per-slot inventory the coordinator plans against. TOTAL_FREE= is
# the same key drain-coordinator.sh emits so existing parsers keep working.
cmd_slots() {
  local node i total=0 free=0 held=0 legacy=0 line seen=""
  local -a plan=()
  while IFS= read -r line; do [ -n "$line" ] && plan+=("$line"); done \
    < <(slot_plan "$(node_capacity_spec)")
  printf '  %-16s %-6s %s\n' NODE SLOT RESERVATION
  for line in "${plan[@]}"; do
    node="${line%% *}"; i="${line##* }"
    total=$(( total + 1 ))
    if slot_is_free "$node" "$i"; then
      free=$(( free + 1 )); printf '  %-16s %-6s %s\n' "$node" "$i" "free"
    else
      held=$(( held + 1 )); printf '  %-16s %-6s %s\n' "$node" "$i" "RESERVED"
    fi
    # Report each node's legacy whole-node hold once, on its first slot row.
    case " $seen " in
      *" $node "*) ;;
      *) seen="$seen $node"
         if [ "$(legacy_holds_on "$node")" -gt 0 ]; then
           legacy=$(( legacy + 1 ))
           printf '  %-16s %-6s %s\n' "$node" "-" "RESERVED (legacy whole-node job)"
         fi ;;
    esac
  done
  echo "TOTAL_SLOTS=$total"
  echo "TOTAL_RESERVED=$(( held + legacy ))"
  echo "LEGACY_HOLDS=$legacy"
  # Legacy whole-node jobs consume capacity the per-slot locks cannot see, so
  # subtract them to keep TOTAL_FREE an honest dispatchable count.
  echo "TOTAL_FREE=$(( free > legacy ? free - legacy : 0 ))"
}

# ============================================================================
# --self-test — pure-function assertions (no farm I/O).
# ============================================================================
self_test() {
  local fails=0
  check() { # <label> <got> <want>
    if [ "$2" = "$3" ]; then echo "  ok: $1"
    else echo "  FAIL: $1 — got '$2' want '$3'" >&2; fails=$(( fails + 1 )); fi
  }
  echo "farm-dispatch --self-test:"

  # --- slot_plan: spread-first ordering over the real roster shape ---
  local SPEC; SPEC="$(printf '3 A\n2 B\n2 C\n1 D\n')"
  check "spread-first: all slot-1s, then 2s, then 3s" \
    "$(slot_plan "$SPEC" | tr '\n' '|')" \
    "A 1|B 1|C 1|D 1|A 2|B 2|C 2|A 3|"
  check "slot count equals summed caps" "$(slot_plan "$SPEC" | grep -c .)" 8
  check "single cap-1 node" "$(slot_plan '1 A' | tr '\n' '|')" "A 1|"
  check "empty spec yields no slots" "$(slot_plan '' | grep -c .)" 0
  check "non-numeric cap ignored" "$(slot_plan "$(printf 'x A\n2 B\n')" | tr '\n' '|')" "B 1|B 2|"
  # The live roster must expose its full declared capacity (2+2+3+2+1).
  if [ -f "$TOPOLOGY" ]; then
    check "canonical roster exposes 10 slots" \
      "$(slot_plan "$(node_capacity_spec)" | grep -c .)" 10
    check "big iron leads the plan" \
      "$(slot_plan "$(node_capacity_spec)" | sed -n 1p)" "172.20.0.130 1"
  else
    echo "  skip: roster assertions (no $TOPOLOGY)"
  fi

  # --- job_shape: a DISK classifier (workspace test is heavy even though
  #     xcp-build.sh places it as small) ---
  check "workspace build → heavy" "$(job_shape 'cargo build --workspace')" heavy
  check "workspace test → heavy"  "$(job_shape 'cargo test --workspace')" heavy
  check "release build → heavy"   "$(job_shape 'cargo build --release')" heavy
  check "rpm cut → heavy"         "$(job_shape 'cargo generate-rpm -p crates/mesh/mackesd')" heavy
  check "per-crate test → light"  "$(job_shape 'cargo test -p mackesd')" light
  check "per-crate build → light" "$(job_shape 'cargo build -p mde-bus')" light

  # --- required_kib: own headroom + one light unit per other reservation ---
  check "light alone"        "$(required_kib light 0 8388608 41943040)" 8388608
  check "light + 2 others"   "$(required_kib light 2 8388608 41943040)" 25165824
  check "heavy alone"        "$(required_kib heavy 0 8388608 41943040)" 41943040
  check "heavy + 1 other"    "$(required_kib heavy 1 8388608 41943040)" 50331648
  # A cold whole-workspace target/ measured 54G live, so heavy must demand far
  # more than the 8G light floor that previously admitted it and hit ENOSPC.
  local light_need heavy_need
  light_need="$(required_kib light 0 "$MIN_FREE_KIB" "$HEAVY_FREE_KIB")"
  heavy_need="$(required_kib heavy 0 "$MIN_FREE_KIB" "$HEAVY_FREE_KIB")"
  if [ "$heavy_need" -gt "$light_need" ]; then check "heavy demands more than light" yes yes
  else check "heavy demands more than light" "$heavy_need vs $light_need" "heavy>light"; fi

  # --- slot locks are per-slot, so one node hosts concurrent reservations ---
  local td; td="$(mktemp -d "${TMPDIR:-/tmp}/farm-dispatch-self.XXXXXX")" || return 1
  local LOCKS="$td"   # shadow the global for this assertion only
  (
    exec 8>"$LOCKS/172.20.0.130-slot1.lock"; flock -n 8 || exit 9
    # A DIFFERENT slot on the SAME node must still be claimable.
    ( exec 9>"$LOCKS/172.20.0.130-slot2.lock"; flock -n 9 ) || exit 10
    # The same slot must NOT be claimable twice.
    if ( exec 9>"$LOCKS/172.20.0.130-slot1.lock"; flock -n 9 ) 2>/dev/null; then exit 11; fi
    exit 0
  )
  case $? in
    0) check "same node, distinct slots both claimable" yes yes ;;
    11) check "same slot refuses a second reservation" "claimed twice" "refused" ;;
    *) check "slot lock isolation" "unexpected rc" "0" ;;
  esac
  # reserved_slots_on counts a held sibling and honours the exclude argument.
  (
    exec 8>"$LOCKS/172.20.0.50-slot1.lock"; flock -n 8 || exit 9
    got="$(reserved_slots_on 172.20.0.50)"
    [ "$got" = "1" ] || exit 12
    got="$(reserved_slots_on 172.20.0.50 "$LOCKS/172.20.0.50-slot1.lock")"
    [ "$got" = "0" ] || exit 13
    exit 0
  )
  case $? in
    0) check "reserved_slots_on counts others, excludes self" yes yes ;;
    12) check "reserved_slots_on counts a held sibling" "miscounted" "1" ;;
    13) check "reserved_slots_on excludes its own lockfile" "miscounted" "0" ;;
    *) check "reserved_slots_on" "unexpected rc" "0" ;;
  esac
  # A legacy whole-node hold must count as a reservation even with no slot lock,
  # so a migrating farm cannot oversubscribe the node.
  (
    exec 8>"$LOCKS/172.20.0.90.lock"; flock -n 8 || exit 9
    [ "$(legacy_holds_on 172.20.0.90)" = "1" ] || exit 12
    [ "$(reserved_slots_on 172.20.0.90)" = "1" ] || exit 13
    exit 0
  )
  case $? in
    0) check "legacy whole-node hold counts as a reservation" yes yes ;;
    *) check "legacy whole-node hold counts as a reservation" "not counted" "counted" ;;
  esac
  check "no legacy hold on an idle node" "$(legacy_holds_on 172.20.0.170)" 0
  # Per-jobid idempotence: the same jobid must not be claimable twice.
  (
    exec 8>"$LOCKS/job-dupe.lock"; flock -n 8 || exit 9
    if ( exec 9>"$LOCKS/job-dupe.lock"; flock -n 9 ) 2>/dev/null; then exit 11; fi
    exit 0
  )
  case $? in
    0) check "same jobid refuses a concurrent second dispatch" yes yes ;;
    *) check "same jobid refuses a concurrent second dispatch" "claimed twice" "refused" ;;
  esac

  # --- reclaim targets only provably idle, rebuildable trees ---
  check "idle node offers every slot target plus the legacy tree" \
    "$(reclaimable_paths_on 172.20.0.130 3 | tr '\n' '|')" \
    "$DIR_BASE-d1/target|$DIR_BASE-d2/target|$DIR_BASE-d3/target|magic-mesh/target|"
  check "reclaim never proposes a source tree" \
    "$(reclaimable_paths_on 172.20.0.130 3 | grep -cv '/target$')" 0
  check "reclaim never proposes an absolute or traversing path" \
    "$(reclaimable_paths_on 172.20.0.130 3 | grep -c -e '^/' -e '\.\.')" 0
  (
    # A RESERVED slot's target must be protected: an in-flight build owns it.
    # Matched with `case`, not a `grep -q` pipe: an early-exiting grep SIGPIPEs
    # the producer and `pipefail` would report that as the assertion's result.
    exec 8>"$LOCKS/172.20.0.130-slot2.lock"; flock -n 8 || exit 9
    got="$(reclaimable_paths_on 172.20.0.130 3)"
    case "$got" in *"d2/target"*) exit 11 ;; esac
    case "$got" in *"d1/target"*) ;; *) exit 12 ;; esac
    exit 0
  )
  case $? in
    0) check "reserved slot's target is protected from reclaim" yes yes ;;
    11) check "reserved slot's target is protected from reclaim" "offered it" "protected" ;;
    *) check "reserved slot's target is protected from reclaim" "unexpected rc" "0" ;;
  esac
  (
    # A live legacy whole-node job owns the shared tree: never reclaim it.
    exec 8>"$LOCKS/172.20.0.130.lock"; flock -n 8 || exit 9
    got="$(reclaimable_paths_on 172.20.0.130 3)"
    case "$got" in *"magic-mesh/target"*) exit 11 ;; esac
    exit 0
  )
  case $? in
    0) check "legacy tree protected while its job holds the node" yes yes ;;
    *) check "legacy tree protected while its job holds the node" "offered it" "protected" ;;
  esac
  rm -rf "$td"

  # --- dedupe by command: same command + same clean commit is reusable ---
  local rd; rd="$(mktemp -d "${TMPDIR:-/tmp}/farm-dispatch-res.XXXXXX")" || return 1
  local RESULTS="$rd"   # shadow the global for these assertions only
  printf '{"jobid":"j1","outcome":"pass","commit":"abc1234","command":"cargo test -p mde-collab-egui"}\n' >"$rd/j1.json"
  printf '{"jobid":"j2","outcome":"fail","commit":"abc1234","command":"cargo test -p mde-files"}\n' >"$rd/j2.json"
  check "same command at same commit is reusable" \
    "$(find_equivalent_result 'cargo test -p mde-collab-egui' abc1234 >/dev/null && echo yes || echo no)" yes
  check "a different command is not reusable" \
    "$(find_equivalent_result 'cargo test -p mackesd' abc1234 >/dev/null && echo yes || echo no)" no
  check "same command at another commit is not reusable" \
    "$(find_equivalent_result 'cargo test -p mde-collab-egui' def5678 >/dev/null && echo yes || echo no)" no
  # A dirty tree must never dedupe: two runs cannot be proven equal.
  check "dirty tree refuses reuse" \
    "$(find_equivalent_result 'cargo test -p mde-collab-egui' abc1234-dirty >/dev/null && echo yes || echo no)" no
  check "a failed run is reusable too (a red gate is still an answer)" \
    "$(find_equivalent_result 'cargo test -p mde-files' abc1234 >/dev/null && echo yes || echo no)" yes
  # Adoption republishes under the new jobid and preserves the outcome.
  adopt_result "$rd/j1.json" j9 >/dev/null 2>&1
  check "adopted result is published under the adopting jobid" \
    "$(python3 -c "import json;d=json.load(open('$rd/j9.json'));print(d['jobid'],d['outcome'],d['reused_from'])" 2>/dev/null)" \
    "j9 pass j1"
  check "identical commands share one lock key" \
    "$([ "$(command_key 'cargo test -p x')" = "$(command_key 'cargo test -p x')" ] && echo same || echo differ)" same
  check "different commands get different lock keys" \
    "$([ "$(command_key 'cargo test -p x')" = "$(command_key 'cargo test -p y')" ] && echo same || echo differ)" differ
  rm -rf "$rd"

  if [ "$fails" -eq 0 ]; then echo "farm-dispatch: self-test passed"; return 0; fi
  echo "farm-dispatch: SELF-TEST FAILED ($fails)" >&2; return 1
}

case "${1:-nodes}" in
  run)    shift; cmd_run "$@" ;;
  result) shift; cmd_result "$@" ;;
  nodes)  cmd_nodes ;;
  slots)  cmd_slots ;;
  --self-test) self_test ;;
  -h|--help) sed -n '2,60p' "$0" | sed 's/^# \{0,1\}//' ;;
  *) echo "usage: farm-dispatch.sh run <jobid> <cmd> | result <jobid> | nodes | slots | --self-test" >&2; exit 2 ;;
esac
