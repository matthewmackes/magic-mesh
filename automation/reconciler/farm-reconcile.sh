#!/usr/bin/env bash
# farm-reconcile.sh — FARM-AUTO-4 / DRAIN-4: declarative GitOps reconciler.
#
# Desired state = the worklist's active @farm jobs. Each run converges the farm to
# "every active job has a FRESH result for the current source rev". Idempotent: a
# job whose recorded result already matches HEAD (clean tree) is skipped, so the
# timer is cheap when nothing changed. Jobs that need running are dispatched
# concurrently across the free nodes (dispatch's per-node flock packs them).
#
# The mechanical-build half of the hybrid drain engine (docs/design/autonomous-drain.md
# §B). After convergence, with ZERO AI tokens spent, it:
#   GREEN — opens (or reuses) a PR for the current rev and flips each contributing
#           worklist task to `needs-review` by recording it in $STATE/needs-review.txt
#           (the AI supervisor/coordinator reads that to pick up review work). The human
#           worklist checkbox is left untouched — collision-free, and this script's
#           pathspec is never the canonical platform worklist (AI_GOVERNANCE).
#   RED   — raises a triage task: one line per failed job in $STATE/triage.txt (and a
#           GitHub issue when `gh` is configured), so a red build surfaces work for the
#           AI supervisor instead of silently rotting.
#
# Runs from a systemd timer on the control host (packaging/systemd/
# mcnf-farm-reconcile.{service,timer}) — fleet-side, no AI in the loop.
#
# Usage: farm-reconcile.sh [--dry-run] [--self-test]
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
LIB="$HERE/../lib"
REPO="$(cd "$HERE/../.." && pwd)"
STATE="${MCNF_FARM_STATE:-$REPO/automation/.state}"
RESULTS="$STATE/results"
SUMMARY="$STATE/farm-status.txt"
TRIAGE="$STATE/triage.txt"          # red-build triage tasks (one line per failed job)
REVIEW="$STATE/needs-review.txt"    # green tasks flipped to needs-review (audit log)
DRY=0
[ "${1:-}" = "--dry-run" ] && DRY=1
mkdir -p "$RESULTS"

rev() { local r; r="$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo unknown)"; git -C "$REPO" diff --quiet 2>/dev/null || r="${r}-dirty"; printf '%s' "$r"; }
CUR="$(rev)"
# log → stderr, so command-substituted helpers (open_pr) return ONLY their value on stdout.
log() { echo "[$(date -u +%H:%M:%SZ)] reconcile: $*" >&2; }

# Is this job's result fresh for the current rev?  (a -dirty tree is never fresh)
is_fresh() {
  local jid="$1"; local f="$RESULTS/$jid.json"
  [ -f "$f" ] || return 1
  case "$CUR" in *-dirty) return 1;; esac
  local rc; rc="$(python3 -c "import json,sys;print(json.load(open('$f')).get('commit',''))" 2>/dev/null)"
  [ "$rc" = "$CUR" ]
}

# read result outcome (pass|fail|missing) for a jobid, robust to a missing/garbage file.
job_outcome() {
  python3 -c "import json;print(json.load(open('$RESULTS/$1.json')).get('outcome','?'))" 2>/dev/null || echo missing
}

# have_gh — true iff gh is usable. MCNF_FARM_NO_GH=1 forces the offline path (the
# self-test sets it; an operator can too to suppress PR/issue creation on the timer).
have_gh() { [ "${MCNF_FARM_NO_GH:-0}" = "1" ] && return 1; command -v gh >/dev/null 2>&1; }

# --- GREEN: open/reuse a PR + flip contributing tasks to needs-review (no AI) ---------
# The mechanical half opens a PR for the converged rev. Idempotent: if a PR already
# exists for the rev branch we reuse it (no duplicate). gh is optional — without it
# (or in CI), we record the intent in $REVIEW so nothing is silently dropped.
open_pr() {
  local rev="$1"; shift; local tasks="$*"
  local branch="farm-auto/${rev}"
  local title="farm-auto: ${tasks:-@farm jobs} green @ ${rev}"
  if ! have_gh; then
    log "  pr: gh absent/disabled — recording needs-review intent only (branch=$branch)"
    return 0
  fi
  # Already a PR for this rev branch?  reuse it (idempotent timer).
  local existing
  existing="$(gh pr list --repo "$REMOTE" --head "$branch" --json url -q '.[0].url' 2>/dev/null || true)"
  if [ -n "$existing" ]; then log "  pr: reuse $existing"; printf '%s' "$existing"; return 0; fi
  # Push the converged rev to the branch and open the PR (body = the @farm tasks).
  git -C "$REPO" push -q "$REMOTE" "HEAD:refs/heads/$branch" 2>/dev/null || {
    log "  pr: push failed (branch=$branch) — recording intent only"; return 0; }
  local url
  url="$(gh pr create --repo "$REMOTE" --head "$branch" --base "$BASE" \
           --title "$title" \
           --body "Mechanical farm-auto PR (no AI tokens). Green @ \`${rev}\`. Tasks: ${tasks:-?}. Flip to needs-review." \
         2>/dev/null || true)"
  if [ -n "$url" ]; then log "  pr: opened $url"; else log "  pr: gh pr create failed (branch=$branch)"; fi
  printf '%s' "$url"
}

# flip_needs_review <rev> <pr_url> <task...> — record each contributing task as
# needs-review for the AI supervisor. We do NOT edit the human worklist checkbox
# (collision-free + AI_GOVERNANCE: this file's pathspec must never be the
# canonical platform worklist); the marker lives in $REVIEW (the coordinator/AI
# reads it to pick up review work).
flip_needs_review() {
  local rev="$1" url="$2"; shift 2
  local t
  for t in "$@"; do
    [ -n "$t" ] || continue
    printf '%s\t%s\tneeds-review\t%s\n' "$rev" "$t" "${url:-no-pr}" >> "$REVIEW"
    log "  needs-review: $t @ $rev (${url:-no-pr})"
  done
}

# --- RED: raise a triage task per failed job (no AI) -----------------------------------
# A red build is real work for the AI supervisor, not a silent rot. We append one triage
# line per failed job to $STATE/triage.txt and (when gh is configured) open a triage
# issue, idempotent on the (rev,jobid) key so the timer doesn't spam duplicates.
raise_triage() {
  local rev="$1" jid="$2" task="$3" cmd="$4"
  local key="${rev}:${jid}"
  # Idempotent: already raised for this rev+job?  skip.
  if [ -f "$TRIAGE" ] && grep -qF "	$key	" "$TRIAGE" 2>/dev/null; then
    log "  triage: already raised $task/$jid @ $rev"; return 0
  fi
  local logf; logf="$(python3 -c "import json;print(json.load(open('$RESULTS/$jid.json')).get('log',''))" 2>/dev/null || echo '')"
  printf '%s\t%s\t%s\t%s\t%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$key" "$task" "$cmd" "$logf" >> "$TRIAGE"
  log "  triage: RED $task/$jid @ $rev — $cmd"
  if have_gh; then
    if gh issue create --repo "$REMOTE" \
         --title "farm-triage: $task RED @ $rev ($cmd)" \
         --body "Mechanical farm build failed (no AI). rev=\`$rev\` job=\`$jid\` cmd=\`$cmd\`. Log: $logf. Needs AI triage (autonomous-drain §B)." \
         --label "farm-triage" >/dev/null 2>&1
    then log "  triage: issue opened for $task/$jid"
    else log "  triage: gh issue create skipped/failed (recorded in $TRIAGE)"; fi
  fi
}

# --- self-test: prove the green/red post-converge logic with NO farm + NO GitHub ------
# Forces the offline path (MCNF_FARM_NO_GH) + a temp state dir, then asserts: a green
# result flips the task to needs-review (no PR url required), a red result raises exactly
# one triage line, a re-run is idempotent (no duplicate triage), and a new rev re-raises.
# Exercises the real functions — no farm, no GitHub, no AI.
self_test() {
  local td fails=0
  # Shadow the global state paths + remote so the test never touches real state/GitHub.
  local RESULTS TRIAGE REVIEW REMOTE BASE
  td="$(mktemp -d)"; trap 'rm -rf "$td"' RETURN
  RESULTS="$td/results"; TRIAGE="$td/triage.txt"; REVIEW="$td/needs-review.txt"
  REMOTE="origin"; BASE="master"
  mkdir -p "$RESULTS"
  # Force the offline path so the test never calls gh / pushes / opens an issue.
  export MCNF_FARM_NO_GH=1
  chk() { if [ "$2" = "$3" ]; then echo "  ok: $1"; else echo "  FAIL: $1 — got '$2' want '$3'" >&2; fails=$((fails+1)); fi; }
  echo "farm-reconcile --self-test:"

  # GREEN job → needs-review recorded, no triage.
  echo '{"jobid":"j1","outcome":"pass","node":"n","log":""}' > "$RESULTS/j1.json"
  flip_needs_review "abc1234" "" "TASK-A" >/dev/null 2>&1
  chk "green flips one needs-review line" "$(wc -l <"$REVIEW" 2>/dev/null | tr -d ' ')" "1"
  chk "needs-review line names the task" "$(awk -F'\t' '{print $2}' "$REVIEW")" "TASK-A"

  # RED job → exactly one triage line.
  echo '{"jobid":"j2","outcome":"fail","node":"n","log":"/x.log"}' > "$RESULTS/j2.json"
  raise_triage "abc1234" "j2" "TASK-B" "cargo build -p x" >/dev/null 2>&1
  chk "red raises one triage line" "$(wc -l <"$TRIAGE" 2>/dev/null | tr -d ' ')" "1"
  # Idempotent: re-raise same rev+job → still one line.
  raise_triage "abc1234" "j2" "TASK-B" "cargo build -p x" >/dev/null 2>&1
  chk "triage is idempotent on (rev,job)" "$(wc -l <"$TRIAGE" 2>/dev/null | tr -d ' ')" "1"
  # A new rev for the same job → a fresh triage line.
  raise_triage "def5678" "j2" "TASK-B" "cargo build -p x" >/dev/null 2>&1
  chk "new rev raises a new triage line" "$(wc -l <"$TRIAGE" 2>/dev/null | tr -d ' ')" "2"

  # job_outcome reads the JSON.
  chk "job_outcome pass" "$(job_outcome j1)" "pass"
  chk "job_outcome fail" "$(job_outcome j2)" "fail"
  chk "job_outcome missing" "$(job_outcome nope)" "missing"

  if [ "$fails" -eq 0 ]; then echo "farm-reconcile: self-test passed"; return 0; fi
  echo "farm-reconcile: SELF-TEST FAILED ($fails)" >&2; return 1
}

# Remote/base for the mechanical PR (overridable for a fork / non-default branch).
REMOTE="${MCNF_FARM_REMOTE:-origin}"
BASE="${MCNF_FARM_BASE:-master}"

case "${1:-}" in
  --self-test) self_test; exit $? ;;
  -h|--help) sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
esac

log "rev=$CUR  jobs from docs/platform/WORKLIST.md"
declare -a NEED=()
declare -A CMD=()
declare -A TASK=()
while IFS=$'\t' read -r jid _ task cmd; do
  [ -n "$jid" ] || continue
  TASK["$jid"]="$task"
  if is_fresh "$jid"; then
    log "  skip  $task/$jid (fresh @ $CUR)"
  else
    log "  need  $task/$jid : $cmd"
    NEED+=("$jid"); CMD["$jid"]="$cmd"
  fi
done < <("$LIB/farm-jobs.sh" active)

if [ "${#NEED[@]}" -eq 0 ]; then log "nothing to do — farm converged @ $CUR"; exit 0; fi
if [ "$DRY" -eq 1 ]; then log "dry-run: would dispatch ${#NEED[@]} job(s)"; exit 0; fi

# Dispatch each needed job in the background; retry-on-busy (EX_TEMPFAIL=75) so
# jobs queue onto nodes as they free up. dispatch's flock serializes per node.
for jid in "${NEED[@]}"; do
  ( while :; do "$LIB/farm-dispatch.sh" run "$jid" "${CMD[$jid]}"; rc=$?; [ "$rc" -eq 75 ] || break; sleep 5; done ) &
done
wait

# Summary (the report-back; the result JSONs are the per-job record).
{
  echo "MCNF build-farm status @ $(date -u +%Y-%m-%dT%H:%M:%SZ)  rev=$CUR"
  for jid in "${NEED[@]}"; do
    o="$(job_outcome "$jid")"
    n="$(python3 -c "import json;print(json.load(open('$RESULTS/$jid.json')).get('node','?'))" 2>/dev/null || echo '?')"
    printf '  %-12s %-5s %s  (%s)\n' "$jid" "$o" "${CMD[$jid]}" "$n"
  done
} | tee "$SUMMARY"

# --- post-converge: GREEN → PR + needs-review · RED → triage task (no AI tokens) ------
fails=0
declare -A GREEN_TASKS=()
for jid in "${NEED[@]}"; do
  o="$(job_outcome "$jid")"
  task="${TASK[$jid]:-?}"
  if [ "$o" = "pass" ]; then
    GREEN_TASKS["$task"]=1                       # task contributed at least one green job
  else
    fails=$((fails + 1))
    raise_triage "$CUR" "$jid" "$task" "${CMD[$jid]}"   # a red build raises a triage task
  fi
done

# Open the mechanical PR + flip tasks to needs-review only when the converge is fully
# green (every dispatched job passed) — a half-red rev isn't a reviewable slice.
if [ "$fails" -eq 0 ] && [ "${#GREEN_TASKS[@]}" -gt 0 ]; then
  tasks="$(printf '%s ' "${!GREEN_TASKS[@]}")"; tasks="${tasks% }"
  # shellcheck disable=SC2086
  pr_url="$(open_pr "$CUR" $tasks)"
  # shellcheck disable=SC2086
  flip_needs_review "$CUR" "$pr_url" $tasks
fi

log "done: ${#NEED[@]} dispatched, $fails failed"
[ "$fails" -eq 0 ]
