#!/usr/bin/env bash
# ci-gate.sh — the always-on farm CI gate (review finding test-obs-1, P0).
#
# WHY: this workspace's ONLY real build path is the farm (install-helpers/
# xcp-build.sh); local `cargo` is a no-op shim and the old GitHub Actions runner
# has been dead ~26 days (it can't build this workspace without the farm). That
# left NO always-on gate for the ~41 crates / ~8,400 tests — the root of the
# recurring "green-tests-but-shipped-broken" pattern. This script is that gate:
# it runs the repository policy lints locally, then fmt + clippy + the full test
# pyramid ON THE FARM (routed to BigBoy, the long-pole node), captures a
# structured pass/fail, and publishes the result to the Bus so a RED gate raises
# a KIRON operator toast and a GREEN gate is a healthy heartbeat with a last-run
# timestamp (staleness is detectable).
#
# It deliberately mirrors automation/testbed/nightly.sh (BUILD-PLATFORM-7): same
# best-effort `bus_publish` (local `mde-bus` → else sshpass to the live shell
# node), same `automation/.state` result files, same "never fail on a publish
# miss" posture. A publish miss must NEVER fail or hang the gate.
#
# Usage:
#   ci-gate.sh [run]     run the full gate on the CURRENT checkout, publish result
#   ci-gate.sh policy    run the maintained policy-lint suite only (no farm I/O)
#   ci-gate.sh --self-test  prove policy-stage failures propagate (no farm I/O)
#   ci-gate.sh poll      run only if origin/master advanced past the last-gated SHA
#                        (the master-push trigger — cheap no-op when unchanged)
#   ci-gate.sh liveness  alert if the gate hasn't produced a result within N days
#                        (a silently-stopped gate must NOT look green) — no farm I/O
#
# Env overrides:
#   MCNF_BUILD_HOST      farm host for every stage        (default 172.20.0.130 = BigBoy)
#   MCNF_BUILD_SLOT      dedicated warm remote slot        (default "ci")
#   MCNF_CI_BUS_HOST     node whose Bus the operator shell reads (default Eagle .13)
#   MCNF_CI_BUS_USER     ssh user for the Bus fallback     (default mm)
#   MCNF_CI_BUS_PASS_FILE  password file for the fallback  (default /root/.mcnf-xapi-cred)
#   MCNF_CI_MAX_STALE_DAYS staleness threshold for liveness (default 2)
#   MCNF_FARM_STATE      state dir                         (default $REPO/automation/.state)
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
XCP="$HERE/xcp-build.sh"

STATE_DIR="${MCNF_FARM_STATE:-$REPO/automation/.state}"
STATUS_JSON="$STATE_DIR/ci-gate-status.json"
LAST_SHA_FILE="$STATE_DIR/ci-gate-last-sha"
MARKER="$STATE_DIR/ci-gate-last-run"      # mtime = last COMPLETED gate run
LOG="$STATE_DIR/ci-gate.log"
mkdir -p "$STATE_DIR"

# Route EVERY stage to BigBoy (memory: "BigBoy takes the heaviest builds"; it is
# the 12-vCPU long-pole node) on a dedicated warm CI slot so the gate keeps its
# own target/ cache and never collides with in-flight farm agent builds. An
# operator MCNF_BUILD_HOST pin still wins (exported values only default here).
export MCNF_BUILD_HOST="${MCNF_BUILD_HOST:-172.20.0.130}"
export MCNF_BUILD_SLOT="${MCNF_BUILD_SLOT:-ci}"

# Bus publish target (best-effort; mirrors nightly.sh). Point this at whatever
# node runs the operator's live shell if Eagle is not it.
BUS_HOST="${MCNF_CI_BUS_HOST:-172.20.146.13}"
BUS_USER="${MCNF_CI_BUS_USER:-mm}"
BUS_PASS_FILE="${MCNF_CI_BUS_PASS_FILE:-/root/.mcnf-xapi-cred}"

MAX_STALE_DAYS="${MCNF_CI_MAX_STALE_DAYS:-2}"

# The PTY-driven suites that HANG under cargo's default parallelism on the farm
# (memory: "mde-term-egui/mackesd hang under default-parallel on a 4-vCPU node →
# use --test-threads=1"). Run these serially; the rest of the workspace runs at
# full parallelism.
CRATES_SERIAL=(mackesd mde-term-egui)

# One maintained policy suite for both the farm gate and GitHub Actions. Keep
# repository-structure checks here rather than duplicating an incomplete list in
# each runner. Every lint with a planted-failure self-test is exercised before it
# scans the real tree; lints without a self-test still run as hard checks.
POLICY_LINTS=(
  lint-bus-names.sh
  lint-layered-tiers.sh
  lint-style-leaks.sh
  lint-brand-identity.sh
  lint-shared-substrate.sh
  lint-doc-supersession.sh
  lint-worklist.sh
)
POLICY_SELF_TESTS=(
  lint-bus-names.sh
  lint-layered-tiers.sh
  lint-brand-identity.sh
  lint-doc-supersession.sh
  lint-worklist.sh
)
POLICY_ROOT="$HERE"

# ── result state (globals; filled by cmd_run, read by finish) ────────────────
SHA="" ; SHORT="" ; STARTED="" ; FINISHED=""
STAGE_POLICY="skipped" ; STAGE_FMT="skipped" ; STAGE_CLIPPY="skipped" ; STAGE_TEST="skipped"
FAILED_STAGE="" ; OVERALL="green"
TESTS_PASSED=0 ; TESTS_FAILED=0

ts()  { date -u +%Y-%m-%dT%H:%M:%SZ; }
say() { echo "==> ci-gate: $*"; }
json_escape() { local s="${1//\\/\\\\}"; s="${s//\"/\\\"}"; printf '%s' "$s"; }

# bus_publish <topic> <json-body> — best-effort, identical contract to
# nightly.sh: publish locally if `mde-bus` is on PATH, else ssh to the shell node
# and publish there. NEVER fails the gate (a missing Bus is not a gate failure).
bus_publish() {
  local topic="$1" body="$2" qbody
  say "Bus → $topic  $body"
  if command -v mde-bus >/dev/null 2>&1; then
    mde-bus publish "$topic" --body-flag "$body" >/dev/null 2>&1 || true
    return 0
  fi
  command -v sshpass >/dev/null 2>&1 || { say "(no local mde-bus / no sshpass — logged only)"; return 0; }
  [ -f "$BUS_PASS_FILE" ] || { say "(no Bus pass file — logged only)"; return 0; }
  qbody="$(printf '%q' "$body")"
  sshpass -f "$BUS_PASS_FILE" ssh \
    -o PreferredAuthentications=password -o PubkeyAuthentication=no \
    -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 \
    "$BUS_USER@$BUS_HOST" \
    "command -v mde-bus >/dev/null 2>&1 && mde-bus publish $topic --body-flag $qbody" \
    >/dev/null 2>&1 || say "(Bus publish to $BUS_HOST unreachable — result still recorded in $STATUS_JSON)"
}

# publish_toast <severity> <headline> — raise a KIRON operator toast on the
# canonical event/toast/show lane (flag "BUILD"). severity in info|warning|critical.
publish_toast() {
  local sev="$1" headline="$2" host
  host="$(hostname 2>/dev/null || echo ci-gate)"
  bus_publish event/toast/show \
    "{\"severity\":\"$sev\",\"source_host\":\"$(json_escape "$host")\",\"flag\":\"BUILD\",\"headline\":\"$(json_escape "$headline")\"}"
}

# run_cargo <stage-label> <cargo-args...> — sync + run one cargo invocation on the
# farm via xcp-build.sh, tee to the run log, return the REMOTE cargo exit code.
run_cargo() {
  local label="$1"; shift
  { echo; echo "─────────── stage: $label ───────────  ($(ts))  cargo $*"; } | tee -a "$LOG"
  "$XCP" cargo "$@" 2>&1 | tee -a "$LOG"
  return "${PIPESTATUS[0]}"
}

# run_policy_check <label> <lint> [args...] — run one local repository policy
# check and append all output to the same authoritative gate log.
run_policy_check() {
  local label="$1" lint="$2"; shift 2
  { echo; echo "─────────── stage: $label ───────────  ($(ts))  $lint $*"; } | tee -a "$LOG"
  "$POLICY_ROOT/$lint" "$@" 2>&1 | tee -a "$LOG"
  return "${PIPESTATUS[0]}"
}

# run_policy_stage — run every planted-failure self-test and every real-tree
# policy lint. Do not short-circuit within the stage: one invocation reports the
# complete policy state, but any failed check makes the stage (and gate) fail.
run_policy_stage() {
  local rc=0 lint
  for lint in "${POLICY_SELF_TESTS[@]}"; do
    run_policy_check "policy-self-test-$lint" "$lint" --self-test || rc=1
  done
  for lint in "${POLICY_LINTS[@]}"; do
    run_policy_check "policy-$lint" "$lint" || rc=1
  done
  return "$rc"
}

# parse_test_counts — sum passed/failed across every "test result:" line in the
# accumulated log (anchored to that phrase so clippy/build noise never counts).
parse_test_counts() {
  local line p f
  TESTS_PASSED=0 ; TESTS_FAILED=0
  while IFS= read -r line; do
    p="$(printf '%s' "$line" | grep -oE '[0-9]+ passed' | grep -oE '[0-9]+' | head -1)"
    f="$(printf '%s' "$line" | grep -oE '[0-9]+ failed' | grep -oE '[0-9]+' | head -1)"
    TESTS_PASSED=$(( TESTS_PASSED + ${p:-0} ))
    TESTS_FAILED=$(( TESTS_FAILED + ${f:-0} ))
  done < <(grep 'test result:' "$LOG" 2>/dev/null)
}

# run_test_stage — the full test pyramid: the bulk of the workspace at default
# parallelism, then the PTY-hang crates one at a time (--test-threads=1). Runs ALL
# sub-stages regardless of individual failures so the counts are complete; returns
# non-zero if ANY sub-stage failed.
run_test_stage() {
  local rc=0 c exclude=()
  for c in "${CRATES_SERIAL[@]}"; do exclude+=(--exclude "$c"); done
  run_cargo test-bulk +1.94.0 test --workspace "${exclude[@]}" --locked || rc=1
  for c in "${CRATES_SERIAL[@]}"; do
    if [ "$c" = "mackesd" ]; then
      run_cargo "test-$c" +1.94.0 test -p "$c" \
        --features async-services --locked -- --test-threads=1 || rc=1
    else
      run_cargo "test-$c" +1.94.0 test -p "$c" --locked -- --test-threads=1 || rc=1
    fi
  done
  parse_test_counts
  return "$rc"
}

# finish — record the structured result to state and publish it to the Bus.
finish() {
  FINISHED="$(ts)"
  OVERALL="green"; [ -z "$FAILED_STAGE" ] || OVERALL="RED"
  local alert=false; [ "$OVERALL" = green ] || alert=true

  cat > "$STATUS_JSON" <<JSON
{
  "overall": "$OVERALL",
  "alert": $alert,
  "failed_stage": "$(json_escape "$FAILED_STAGE")",
  "stages": { "policy": "$STAGE_POLICY", "fmt": "$STAGE_FMT", "clippy": "$STAGE_CLIPPY", "test": "$STAGE_TEST" },
  "tests_passed": $TESTS_PASSED,
  "tests_failed": $TESTS_FAILED,
  "sha": "$SHA",
  "short_sha": "$SHORT",
  "build_host": "$MCNF_BUILD_HOST",
  "started": "$STARTED",
  "finished": "$FINISHED",
  "source": "ci-gate"
}
JSON
  printf '%s\n' "$SHA" > "$LAST_SHA_FILE"
  # MARKER mtime IS the last-run time the liveness check reads.
  printf 'ci-gate last run %s  sha=%s  overall=%s\n' "$FINISHED" "$SHORT" "$OVERALL" > "$MARKER"

  {
    echo
    echo "=== CI GATE SUMMARY $FINISHED → $OVERALL ==="
    printf '  %-8s %s\n' policy "$STAGE_POLICY"
    printf '  %-8s %s\n' fmt "$STAGE_FMT"
    printf '  %-8s %s\n' clippy "$STAGE_CLIPPY"
    printf '  %-8s %s  (%s passed, %s failed)\n' test "$STAGE_TEST" "$TESTS_PASSED" "$TESTS_FAILED"
    printf '  %-8s %s\n' sha "$SHORT"
  } | tee -a "$LOG"

  # Machine-readable result lane (mirrors event/test/nightly): every run, green or red.
  bus_publish event/ci/gate \
    "{\"overall\":\"$OVERALL\",\"policy\":\"$STAGE_POLICY\",\"fmt\":\"$STAGE_FMT\",\"clippy\":\"$STAGE_CLIPPY\",\"test\":\"$STAGE_TEST\",\"tests_passed\":$TESTS_PASSED,\"tests_failed\":$TESTS_FAILED,\"sha\":\"$SHORT\",\"finished\":\"$FINISHED\",\"source\":\"ci-gate\",\"alert\":$alert}"

  # RED → KIRON operator toast (critical breaks through suppression); GREEN is a
  # quiet heartbeat (the result lane above), no toast spam.
  if [ "$OVERALL" != green ]; then
    publish_toast critical "CI gate RED on $SHORT — $FAILED_STAGE failed (${TESTS_FAILED} test failures)"
  fi
}

# cmd_run — gate the current checkout. Fail-fast across stages so a policy or
# formatting failure does not burn an hour of farm test time.
cmd_run() {
  SHA="$(git -C "$REPO" rev-parse HEAD 2>/dev/null || echo unknown)"
  SHORT="$(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  STARTED="$(ts)"
  : > "$LOG"
  {
    echo "MCNF CI gate — $STARTED"
    echo "  sha=$SHORT  host=$MCNF_BUILD_HOST (slot=$MCNF_BUILD_SLOT)"
  } | tee "$LOG"

  if run_policy_stage; then
    STAGE_POLICY="pass"
    if run_cargo fmt +1.94.0 fmt --all --check; then
      STAGE_FMT="pass"
      if run_cargo clippy +1.94.0 clippy --workspace --all-targets --locked; then
        STAGE_CLIPPY="pass"
        if run_test_stage; then STAGE_TEST="pass"; else STAGE_TEST="fail"; FAILED_STAGE="test"; fi
      else
        STAGE_CLIPPY="fail"; FAILED_STAGE="clippy"
      fi
    else
      STAGE_FMT="fail"; FAILED_STAGE="fmt"
    fi
  else
    STAGE_POLICY="fail"; FAILED_STAGE="policy"
  fi

  finish
  [ "$OVERALL" = green ]   # rc reflects the gate result for CLI/manual use
}

# cmd_policy — expose the exact maintained lint suite to GitHub Actions and
# focused local verification without contacting the build farm.
cmd_policy() {
  : > "$LOG"
  run_policy_stage
}

# cmd_self_test — prove the policy-stage aggregator returns failure when any
# constituent check fails and success only when all checks pass. Coreutils
# true/false make this deterministic without modifying the checkout.
cmd_self_test() {
  : > "$LOG"
  POLICY_ROOT=/bin
  POLICY_SELF_TESTS=()
  POLICY_LINTS=(true false true)
  if run_policy_stage; then
    echo "ci-gate.sh: SELF-TEST FAILED — a failed policy check was swallowed" >&2
    return 1
  fi
  POLICY_LINTS=(true)
  if ! run_policy_stage; then
    echo "ci-gate.sh: SELF-TEST FAILED — an all-green policy stage failed" >&2
    return 1
  fi
  echo "ci-gate.sh: self-test passed — policy failures propagate"
}

# cmd_poll — the master-push trigger. Run the gate only when origin/master has
# advanced past the last-gated SHA; otherwise a cheap no-op. Resets the checkout
# to origin/master first ONLY when the tree is clean (a CI checkout should be).
cmd_poll() {
  if ! git -C "$REPO" fetch --quiet origin master 2>>"$LOG"; then
    say "poll: git fetch failed — skipping this tick"; return 0
  fi
  local target last
  target="$(git -C "$REPO" rev-parse origin/master 2>/dev/null || echo)"
  [ -n "$target" ] || { say "poll: cannot resolve origin/master — skipping"; return 0; }
  last="$(cat "$LAST_SHA_FILE" 2>/dev/null || echo)"
  if [ "$target" = "$last" ]; then
    say "poll: master unchanged ($(git -C "$REPO" rev-parse --short origin/master 2>/dev/null)) — already gated; skip"
    return 0
  fi
  say "poll: master advanced ${last:0:12}${last:+ }→ ${target:0:12} — gating"
  if [ -z "$(git -C "$REPO" status --porcelain 2>/dev/null)" ]; then
    git -C "$REPO" checkout -q master 2>>"$LOG" || true
    git -C "$REPO" reset --hard "$target" 2>>"$LOG" || true
  else
    say "poll: working tree DIRTY — gating current HEAD without reset"
  fi
  cmd_run
}

# cmd_liveness — a silently-stopped gate must NOT look green. Independent of the
# gate run itself (its own timer), no farm I/O: read the last-run marker and alert
# if it is missing or older than the staleness threshold.
cmd_liveness() {
  local now mtime age_h age_d
  now="$(date +%s)"
  if [ ! -f "$MARKER" ]; then
    say "liveness: ci-gate has NEVER produced a result"
    publish_toast warning "CI gate has never run — no gate result on record"
    bus_publish event/ci/gate "{\"overall\":\"unknown\",\"reason\":\"never-run\",\"source\":\"ci-gate-liveness\",\"alert\":true}"
    return 0
  fi
  mtime="$(stat -c %Y "$MARKER" 2>/dev/null || echo 0)"
  age_h=$(( (now - mtime) / 3600 ))
  age_d=$(( (now - mtime) / 86400 ))
  if [ "$age_d" -ge "$MAX_STALE_DAYS" ]; then
    say "liveness: STALE — last gate run ${age_h}h ago (>= ${MAX_STALE_DAYS}d) — alerting"
    publish_toast warning "CI gate STALE — last ran ${age_h}h ago (>= ${MAX_STALE_DAYS}d); the gate may be stopped"
    bus_publish event/ci/gate "{\"overall\":\"stale\",\"age_hours\":$age_h,\"source\":\"ci-gate-liveness\",\"alert\":true}"
  else
    say "liveness: fresh (${age_h}h old) — ok"
  fi
}

usage() { sed -n '/^# Usage:/,/^# Env overrides:/p' "$0" | sed 's/^# \{0,1\}//'; }

case "${1:-run}" in
  run)      cmd_run ;;
  policy)   cmd_policy ;;
  --self-test) cmd_self_test ;;
  poll)     cmd_poll ;;
  liveness) cmd_liveness ;;
  -h | --help | help) usage ;;
  *) usage; exit 1 ;;
esac
