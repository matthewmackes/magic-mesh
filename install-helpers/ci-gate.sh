#!/usr/bin/env bash
# ci-gate.sh — the always-on farm CI gate (review finding test-obs-1, P0).
#
# WHY: this workspace's ONLY real build path is the farm (install-helpers/
# xcp-build.sh); local `cargo` is a no-op shim and the old GitHub Actions runner
# has been dead ~26 days (it can't build this workspace without the farm). That
# left NO always-on gate for the ~41 crates / ~8,400 tests — the root of the
# recurring "green-tests-but-shipped-broken" pattern. This script is that gate:
# it runs the repository policy lints locally, then fmt + clippy + the full test
# pyramid + the hard current-workspace coverage floor ON THE FARM (routed to
# BigBoy, the long-pole node), captures a
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
#   ci-gate.sh verify FILE [EXPECTATION ...]
#                        verify a completed green status artifact and its log
#                        (expectations: --expected-revision SHA
#                         --expected-job-id ID --expected-build-host HOST
#                         --expected-build-slot SLOT)
#   ci-gate.sh bind-release INPUT [STATUS]
#                        bind caller-supplied final release descriptors into the
#                        authenticated gate log and refresh its status digest
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
#   MCNF_BUILD_JOB_ID    upstream farm/job identity (optional; timer fallback is revision-scoped)
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
XCP="$HERE/xcp-build.sh"

STATE_DIR="${MCNF_FARM_STATE:-$REPO/automation/.state}"
STATUS_JSON="$STATE_DIR/ci-gate-status.json"
LAST_SHA_FILE="$STATE_DIR/ci-gate-last-sha"
MARKER="$STATE_DIR/ci-gate-last-run"      # mtime = last COMPLETED gate run
LOG="$STATE_DIR/ci-gate.log"
STATE_LOCK="$STATE_DIR/ci-gate.lock"
mkdir -p "$STATE_DIR"

# A release binding is intentionally small: it is a descriptor set, never an
# artifact transport. Bounding it also makes sparse/hostile inputs cheap to
# reject before jq parses them.
MAX_RELEASE_BINDING_BYTES=$((1024 * 1024))

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
  lint-browser-vm-boundary.sh
  lint-layered-tiers.sh
  lint-style-leaks.sh
  lint-brand-identity.sh
  lint-shared-substrate.sh
  lint-doc-supersession.sh
  lint-workload-authority.sh
  lint-worklist.sh
)
POLICY_SELF_TESTS=(
  lint-bus-names.sh
  lint-browser-vm-boundary.sh
  lint-layered-tiers.sh
  lint-brand-identity.sh
  lint-doc-supersession.sh
  lint-workload-authority.sh
  lint-worklist.sh
)
POLICY_ROOT="$HERE"

# ── result state (globals; filled by cmd_run, read by finish) ────────────────
SHA="" ; SHORT="" ; STARTED="" ; FINISHED="" ; JOB_ID="" ; LOG_SHA256=""
STAGE_POLICY="skipped" ; STAGE_FMT="skipped" ; STAGE_CLIPPY="skipped" ; STAGE_TEST="skipped" ; STAGE_COVERAGE="skipped"
FAILED_STAGE="" ; OVERALL="green"
TESTS_PASSED=0 ; TESTS_FAILED=0

ts()  { date -u +%Y-%m-%dT%H:%M:%SZ; }
say() { echo "==> ci-gate: $*"; }
json_escape() { local s="${1//\\/\\\\}"; s="${s//\"/\\\"}"; printf '%s' "$s"; }
file_sha256() { sha256sum -- "$1" | awk '{print $1}'; }

# A timer-driven gate has no GitHub run number or queue record. Keep that case
# traceable by deriving a stable identity from the gated revision and farm slot;
# an upstream dispatcher may provide its stronger canonical ID explicitly.
derive_job_id() {
  if [ -n "${MCNF_BUILD_JOB_ID:-}" ]; then
    printf '%s\n' "$MCNF_BUILD_JOB_ID"
  else
    printf 'ci-gate:%s:%s:%s\n' "$SHA" "$MCNF_BUILD_HOST" "$MCNF_BUILD_SLOT"
  fi
}

# Serialize writers of the digest-bound log/status pair. A crash between the
# two atomic renames can only leave a digest mismatch, which is fail-closed; the
# next release must rerun the gate rather than guessing how to repair evidence.
with_state_lock() {
  local lock_fd rc
  command -v flock >/dev/null 2>&1 || {
    echo "ci-gate: state updates require flock" >&2
    return 1
  }
  if [ -e "$STATE_LOCK" ] && { [ ! -f "$STATE_LOCK" ] || [ -L "$STATE_LOCK" ]; }; then
    echo "ci-gate: state lock is not a regular, non-symlink file: $STATE_LOCK" >&2
    return 1
  fi
  exec {lock_fd}>"$STATE_LOCK" || return 1
  if ! flock -n "$lock_fd"; then
    echo "ci-gate: another gate state writer is active" >&2
    exec {lock_fd}>&-
    return 1
  fi
  "$@"
  rc=$?
  flock -u "$lock_fd" || rc=1
  exec {lock_fd}>&-
  return "$rc"
}

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

# run_coverage_stage — the farm wrapper provisions the pinned cargo-llvm-cov
# tool/component and executes install-helpers/coverage-command.sh. Keeping the
# command there makes this gate's denominator identical to hosted CI's.
run_coverage_stage() {
  { echo; echo "─────────── stage: coverage ───────────  ($(ts))  canonical llvm-cov floor"; } | tee -a "$LOG"
  "$XCP" coverage 2>&1 | tee -a "$LOG"
  return "${PIPESTATUS[0]}"
}

# finish — record the structured result to state and publish it to the Bus.
finish() {
  FINISHED="$(ts)"
  OVERALL="green"; [ -z "$FAILED_STAGE" ] || OVERALL="RED"
  local alert=false; [ "$OVERALL" = green ] || alert=true

  {
    echo
    echo "=== CI GATE SUMMARY $FINISHED → $OVERALL ==="
    printf '  %-8s %s\n' policy "$STAGE_POLICY"
    printf '  %-8s %s\n' fmt "$STAGE_FMT"
    printf '  %-8s %s\n' clippy "$STAGE_CLIPPY"
    printf '  %-8s %s  (%s passed, %s failed)\n' test "$STAGE_TEST" "$TESTS_PASSED" "$TESTS_FAILED"
    printf '  %-8s %s\n' coverage "$STAGE_COVERAGE"
    printf '  %-8s %s\n' sha "$SHORT"
    printf '  %-8s %s\n' job "$JOB_ID"
  } | tee -a "$LOG"
  # Hash only after the completed summary is in the log, so the evidence
  # fingerprint covers the exact artifact a reviewer receives.
  LOG_SHA256="$(file_sha256 "$LOG")"

  cat > "$STATUS_JSON" <<JSON
{
  "overall": "$OVERALL",
  "alert": $alert,
  "failed_stage": "$(json_escape "$FAILED_STAGE")",
  "stages": { "policy": "$STAGE_POLICY", "fmt": "$STAGE_FMT", "clippy": "$STAGE_CLIPPY", "test": "$STAGE_TEST", "coverage": "$STAGE_COVERAGE" },
  "tests_passed": $TESTS_PASSED,
  "tests_failed": $TESTS_FAILED,
  "sha": "$SHA",
  "short_sha": "$SHORT",
  "job_id": "$(json_escape "$JOB_ID")",
  "build_host": "$(json_escape "$MCNF_BUILD_HOST")",
  "build_slot": "$(json_escape "$MCNF_BUILD_SLOT")",
  "evidence": {
    "revision": "$SHA",
    "gate_log": { "path": "$(json_escape "$(basename "$LOG")")", "sha256": "$LOG_SHA256" }
  },
  "started": "$STARTED",
  "finished": "$FINISHED",
  "source": "ci-gate"
}
JSON
  printf '%s\n' "$SHA" > "$LAST_SHA_FILE"
  # MARKER mtime IS the last-run time the liveness check reads.
  printf 'ci-gate last run %s  sha=%s  overall=%s\n' "$FINISHED" "$SHORT" "$OVERALL" > "$MARKER"

  # Machine-readable result lane (mirrors event/test/nightly): every run, green or red.
  bus_publish event/ci/gate \
    "{\"overall\":\"$OVERALL\",\"policy\":\"$STAGE_POLICY\",\"fmt\":\"$STAGE_FMT\",\"clippy\":\"$STAGE_CLIPPY\",\"test\":\"$STAGE_TEST\",\"coverage\":\"$STAGE_COVERAGE\",\"tests_passed\":$TESTS_PASSED,\"tests_failed\":$TESTS_FAILED,\"sha\":\"$SHORT\",\"revision\":\"$SHA\",\"job_id\":\"$(json_escape "$JOB_ID")\",\"build_host\":\"$(json_escape "$MCNF_BUILD_HOST")\",\"build_slot\":\"$(json_escape "$MCNF_BUILD_SLOT")\",\"artifact\":{\"path\":\"$(json_escape "$LOG")\",\"sha256\":\"$LOG_SHA256\"},\"finished\":\"$FINISHED\",\"source\":\"ci-gate\",\"alert\":$alert}"

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
  JOB_ID="$(derive_job_id)"
  STARTED="$(ts)"
  : > "$LOG"
  {
    echo "MCNF CI gate — $STARTED"
    echo "  sha=$SHORT  revision=$SHA  job_id=$JOB_ID  host=$MCNF_BUILD_HOST (slot=$MCNF_BUILD_SLOT)"
  } | tee "$LOG"

  if run_policy_stage; then
    STAGE_POLICY="pass"
    if run_cargo fmt +1.94.0 fmt --all --check; then
      STAGE_FMT="pass"
      if run_cargo clippy +1.94.0 clippy --workspace --all-targets --locked; then
        STAGE_CLIPPY="pass"
        if run_test_stage; then
          STAGE_TEST="pass"
          if run_coverage_stage; then STAGE_COVERAGE="pass"; else STAGE_COVERAGE="fail"; FAILED_STAGE="coverage"; fi
        else
          STAGE_TEST="fail"; FAILED_STAGE="test"
        fi
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

# cmd_bind_release INPUT [STATUS] — production publisher seam for schema-5
# required-check evidence. INPUT is the exact canonical payload consumed by
# release-evidence.sh: the release flow supplies the final revision and sorted
# artifact descriptors plus the farm identity. This command never discovers,
# rewrites, or invents artifacts.
cmd_bind_release() {
  local input status input_size canonical revision job_id build_host build_slot
  local recorded_revision recorded_job recorded_host recorded_slot status_dir log
  local binding binding_line binding_count status_before log_before staging proposed_digest
  local rc=0
  [ "$#" -ge 1 ] && [ "$#" -le 2 ] || {
    echo "ci-gate: bind-release requires INPUT and accepts one optional STATUS path" >&2
    return 1
  }
  input="$1"
  status="${2:-$STATUS_JSON}"
  [ -f "$input" ] && [ ! -L "$input" ] || {
    echo "ci-gate: release binding input is not a regular, non-symlink file: $input" >&2
    return 1
  }
  input_size="$(stat -c '%s' -- "$input" 2>/dev/null || true)"
  [[ "$input_size" =~ ^[0-9]+$ ]] && [ "$input_size" -gt 0 ] \
    && [ "$input_size" -le "$MAX_RELEASE_BINDING_BYTES" ] || {
      echo "ci-gate: release binding input must be 1..$MAX_RELEASE_BINDING_BYTES bytes: $input" >&2
      return 1
    }
  command -v jq >/dev/null 2>&1 || {
    echo "ci-gate: bind-release requires jq" >&2
    return 1
  }
  jq -e '
    def identity:
      (type == "string") and (length > 0) and (length <= 255) and
      test("^[^[:space:][:cntrl:]]+$");
    (type == "object") and
    (keys == ["artifacts", "farm", "schema_version", "source_commit"]) and
    (.schema_version == 1) and
    (.source_commit | type == "string" and test("^([0-9A-Fa-f]{40}|[0-9A-Fa-f]{64})$")) and
    (.artifacts | type == "array" and length > 0 and length <= 1024 and
      all(.[];
        (type == "object") and
        (keys == ["path", "sha256", "size_bytes"]) and
        (.path | type == "string" and length > 0 and length <= 4096 and
          (test("[[:cntrl:]]") | not)) and
        (.size_bytes | type == "number" and floor == . and . >= 0) and
        (.sha256 | type == "string" and test("^[0-9a-f]{64}$"))) and
      (. == (sort_by(.path))) and
      ((map(.path) | unique | length) == length)) and
    (.farm | type == "object" and
      (keys == ["build_host", "build_slot", "job_id"]) and
      (.job_id | identity) and
      (.build_host | identity) and
      (.build_slot | identity))
  ' "$input" >/dev/null 2>&1 || {
    echo "ci-gate: malformed, incomplete, unsorted, or duplicate release binding input: $input" >&2
    return 1
  }
  canonical="$(jq -cS . "$input")" || return 1
  revision="$(jq -r '.source_commit' <<<"$canonical")"
  job_id="$(jq -r '.farm.job_id' <<<"$canonical")"
  build_host="$(jq -r '.farm.build_host' <<<"$canonical")"
  build_slot="$(jq -r '.farm.build_slot' <<<"$canonical")"

  verify_status "$status" >/dev/null || {
    echo "ci-gate: bind-release requires an unchanged verified green status artifact" >&2
    return 1
  }
  recorded_revision="$(jq -r '.sha' "$status")"
  recorded_job="$(jq -r '.job_id' "$status")"
  recorded_host="$(jq -r '.build_host' "$status")"
  recorded_slot="$(jq -r '.build_slot' "$status")"
  [ "$revision" = "$recorded_revision" ] \
    && [ "$job_id" = "$recorded_job" ] \
    && [ "$build_host" = "$recorded_host" ] \
    && [ "$build_slot" = "$recorded_slot" ] || {
      echo "ci-gate: release binding revision or farm job/host/slot does not match the gate status" >&2
      return 1
    }

  status_dir="$(cd -- "$(dirname -- "$status")" && pwd -P)" || return 1
  log="$status_dir/ci-gate.log"
  binding_count="$(grep -Ec '^  release-evidence-binding([[:space:]]|$)' "$log" || true)"
  [ "$binding_count" -eq 0 ] || {
    echo "ci-gate: gate log already contains release binding input; duplicate publication rejected" >&2
    return 1
  }
  binding="$(printf '%s\n' "$canonical" | sha256sum | awk '{print $1}')"
  binding_line="  release-evidence-binding sha256=$binding"
  status_before="$(file_sha256 "$status")" || return 1
  log_before="$(file_sha256 "$log")" || return 1

  staging="$(mktemp -d "$status_dir/.ci-gate-bind.XXXXXX")" || return 1
  cp -- "$log" "$staging/ci-gate.log" || rc=1
  if [ "$rc" -eq 0 ]; then
    printf '%s\n' "$binding_line" >>"$staging/ci-gate.log" || rc=1
  fi
  if [ "$rc" -eq 0 ]; then
    proposed_digest="$(file_sha256 "$staging/ci-gate.log")" || rc=1
  fi
  if [ "$rc" -eq 0 ]; then
    jq -S --arg digest "$proposed_digest" '.evidence.gate_log.sha256 = $digest' \
      "$status" >"$staging/status.json" || rc=1
    chmod --reference="$status" "$staging/status.json" 2>/dev/null || true
  fi
  if [ "$rc" -eq 0 ]; then
    verify_status "$staging/status.json" >/dev/null || rc=1
  fi
  if [ "$rc" -eq 0 ]; then
    [ "$(file_sha256 "$status")" = "$status_before" ] \
      && [ "$(file_sha256 "$log")" = "$log_before" ] || {
        echo "ci-gate: gate evidence changed while release binding was prepared" >&2
        rc=1
      }
  fi
  if [ "$rc" -eq 0 ]; then
    mv -f -- "$staging/ci-gate.log" "$log" || rc=1
  fi
  if [ "$rc" -eq 0 ]; then
    mv -f -- "$staging/status.json" "$status" || rc=1
  fi
  rm -f -- "$staging/ci-gate.log" "$staging/status.json"
  rmdir -- "$staging" 2>/dev/null || true
  [ "$rc" -eq 0 ] || {
    echo "ci-gate: failed to publish release binding; evidence remains fail-closed" >&2
    return 1
  }
  verify_status "$status" >/dev/null || {
    echo "ci-gate: published release binding did not verify; evidence remains fail-closed" >&2
    return 1
  }
  [ "$(grep -Fxc -- "$binding_line" "$log" || true)" -eq 1 ] || {
    echo "ci-gate: published release binding is not unique in the authenticated log" >&2
    return 1
  }
  echo "ci-gate: bound release evidence $binding for $revision on $build_host/$build_slot"
}

# verify_status — validate the promotion-facing artifact emitted by finish.
# A status file is not authoritative merely because it is parseable: it must
# describe a green result for every stage, include observed test output, bind to
# a revision/job/farm slot, and point at an unchanged log. This is intentionally
# stricter than liveness, which reports stale or red results without rejecting
# them; callers using this command are asking whether the result is usable as a
# required-check input.
verify_status() {
  local file expected_revision="" expected_job_id="" expected_build_host="" expected_build_slot=""
  local log expected actual sha_prefix sha file_dir log_name test_summary
  local log_passed log_failed stage recorded_short_sha recorded_job_id recorded_host recorded_slot
  local identity_line identity_count test_summary_count binding_count binding_valid_count
  [ "$#" -ge 1 ] || {
    echo "ci-gate: verify requires a status artifact path" >&2
    return 1
  }
  file="$1"
  shift
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --expected-revision)
        [ "$#" -ge 2 ] && [ -n "$2" ] || { echo "ci-gate: --expected-revision requires a value" >&2; return 1; }
        expected_revision="$2"
        shift 2
        ;;
      --expected-job-id)
        [ "$#" -ge 2 ] && [ -n "$2" ] || { echo "ci-gate: --expected-job-id requires a value" >&2; return 1; }
        expected_job_id="$2"
        shift 2
        ;;
      --expected-build-host)
        [ "$#" -ge 2 ] && [ -n "$2" ] || { echo "ci-gate: --expected-build-host requires a value" >&2; return 1; }
        expected_build_host="$2"
        shift 2
        ;;
      --expected-build-slot)
        [ "$#" -ge 2 ] && [ -n "$2" ] || { echo "ci-gate: --expected-build-slot requires a value" >&2; return 1; }
        expected_build_slot="$2"
        shift 2
        ;;
      *)
        echo "ci-gate: unknown verify expectation: $1" >&2
        return 1
        ;;
    esac
  done
  [ -f "$file" ] && [ ! -L "$file" ] || {
    echo "ci-gate: status artifact is not a regular, non-symlink file: $file" >&2
    return 1
  }
  command -v jq >/dev/null 2>&1 || {
    echo "ci-gate: verify requires jq" >&2
    return 1
  }
  sha_prefix="$(jq -r '.sha[0:7]' "$file" 2>/dev/null || true)"
  sha="$(jq -r '.sha' "$file" 2>/dev/null || true)"
  jq -e '
    (type == "object") and
    (keys == ["alert", "build_host", "build_slot", "evidence", "failed_stage", "finished", "job_id", "overall", "sha", "short_sha", "source", "stages", "started", "tests_failed", "tests_passed"]) and
    (.overall == "green") and (.alert == false) and (.failed_stage == "") and
    (.source == "ci-gate") and
    # A promotion revision is a Git object identity, not an arbitrary-length
    # hexadecimal token. Accept the repository SHA-1 and SHA-256 forms only.
    (.sha | type == "string" and test("^([0-9a-fA-F]{40}|[0-9a-fA-F]{64})$")) and
    (.short_sha | type == "string" and length >= 7 and startswith($sha_prefix)) and
    (.job_id | type == "string" and test("^[^[:space:][:cntrl:]]+$")) and
    (.build_host | type == "string" and test("^[^[:space:][:cntrl:]]+$")) and
    (.build_slot | type == "string" and test("^[^[:space:][:cntrl:]]+$")) and
    (.stages | type == "object" and (keys == ["clippy", "coverage", "fmt", "policy", "test"]) and all(.[]; . == "pass")) and
    (.tests_passed | type == "number" and floor == . and . > 0) and
    (.tests_failed == 0) and
    (.evidence | type == "object" and (keys == ["gate_log", "revision"]) and .revision == $sha and
      (.gate_log | type == "object" and (keys == ["path", "sha256"]) and
        (.path | type == "string" and length > 0) and
        (.sha256 | type == "string" and test("^[0-9a-f]{64}$"))))
  ' --arg sha_prefix "$sha_prefix" --arg sha "$sha" "$file" >/dev/null || {
    echo "ci-gate: invalid, incomplete, or non-green status artifact: $file" >&2
    return 1
  }
  if [ -n "$expected_revision" ] && [ "$sha" != "$expected_revision" ]; then
    echo "ci-gate: status revision does not match expected source revision" >&2
    return 1
  fi
  file_dir="$(cd -- "$(dirname -- "$file")" && pwd -P)" || return 1
  log_name="$(jq -r '.evidence.gate_log.path' "$file")"
  case "$log_name" in
    ci-gate.log) ;;
    *)
      echo "ci-gate: evidence log must be the sibling ci-gate.log: $log_name" >&2
      return 1
      ;;
  esac
  log="$file_dir/$log_name"
  expected="$(jq -r '.evidence.gate_log.sha256' "$file")"
  [ -f "$log" ] && [ ! -L "$log" ] || {
    echo "ci-gate: referenced gate log is not a regular, non-symlink file: $log" >&2
    return 1
  }
  actual="$(file_sha256 "$log")" || return 1
  [ "$actual" = "$expected" ] || {
    echo "ci-gate: referenced gate log digest does not match status artifact: $log" >&2
    return 1
  }
  recorded_short_sha="$(jq -r '.short_sha' "$file")"
  recorded_job_id="$(jq -r '.job_id' "$file")"
  recorded_host="$(jq -r '.build_host' "$file")"
  recorded_slot="$(jq -r '.build_slot' "$file")"
  if [ -n "$expected_job_id" ] && [ "$recorded_job_id" != "$expected_job_id" ]; then
    echo "ci-gate: status job identity does not match expected GitHub farm job" >&2
    return 1
  fi
  if [ -n "$expected_build_host" ] && [ "$recorded_host" != "$expected_build_host" ]; then
    echo "ci-gate: status build host does not match expected farm host" >&2
    return 1
  fi
  if [ -n "$expected_build_slot" ] && [ "$recorded_slot" != "$expected_build_slot" ]; then
    echo "ci-gate: status build slot does not match expected farm slot" >&2
    return 1
  fi
  identity_line="  sha=$recorded_short_sha  revision=$sha  job_id=$recorded_job_id  host=$recorded_host (slot=$recorded_slot)"
  identity_count="$(grep -Fxc -- "$identity_line" "$log" || true)"
  [ "$identity_count" -eq 1 ] || {
    echo "ci-gate: gate log must contain exactly one identity line bound to the recorded revision, job, host, and slot" >&2
    return 1
  }
  binding_count="$(grep -Ec '^  release-evidence-binding([[:space:]]|$)' "$log" || true)"
  binding_valid_count="$(grep -Ec '^  release-evidence-binding sha256=[0-9a-f]{64}$' "$log" || true)"
  [ "$binding_count" -eq "$binding_valid_count" ] && [ "$binding_valid_count" -le 1 ] || {
    echo "ci-gate: gate log contains malformed or duplicate release binding input" >&2
    return 1
  }
  [ "$(grep -Ec '^=== CI GATE SUMMARY .+ → green ===$' "$log" || true)" -eq 1 ] || {
    echo "ci-gate: gate log has no completed green summary" >&2
    return 1
  }
  if grep -Eq '^=== CI GATE SUMMARY .+ → RED ===$|^  (policy|fmt|clippy|test|coverage)[[:space:]]+fail([[:space:]]|$)' "$log"; then
    echo "ci-gate: gate log contains a contradictory failed result" >&2
    return 1
  fi
  for stage in policy fmt clippy coverage; do
    [ "$(grep -Ec "^  ${stage}[[:space:]]+pass$" "$log" || true)" -eq 1 ] || {
      echo "ci-gate: gate log is missing the completed pass record for $stage" >&2
      return 1
    }
  done
  test_summary_count="$(grep -Ec '^  test[[:space:]]+pass[[:space:]]+\([0-9]+ passed, [0-9]+ failed\)$' "$log" || true)"
  [ "$test_summary_count" -eq 1 ] || {
    echo "ci-gate: gate log must contain exactly one completed test pass record" >&2
    return 1
  }
  test_summary="$(grep -E '^  test[[:space:]]+pass[[:space:]]+\([0-9]+ passed, [0-9]+ failed\)$' "$log")"
  if [[ "$test_summary" =~ ^[[:space:]]+test[[:space:]]+pass[[:space:]]+\(([0-9]+)[[:space:]]+passed,[[:space:]]+([0-9]+)[[:space:]]+failed\)$ ]]; then
    log_passed="${BASH_REMATCH[1]}"
    log_failed="${BASH_REMATCH[2]}"
  else
    echo "ci-gate: gate log is missing the completed test pass record" >&2
    return 1
  fi
  [ "$log_passed" = "$(jq -r '.tests_passed' "$file")" ] || {
    echo "ci-gate: gate log test count does not match status artifact" >&2
    return 1
  }
  [ "$log_failed" = "$(jq -r '.tests_failed' "$file")" ] || {
    echo "ci-gate: gate log failure count does not match status artifact" >&2
    return 1
  }
  echo "ci-gate: verified green status for $(jq -r '.sha' "$file") on $(jq -r '.build_host' "$file")/$(jq -r '.build_slot' "$file")"
}

# cmd_self_test — prove the policy-stage aggregator returns failure when any
# constituent check fails and success only when all checks pass. Coreutils
# true/false make this deterministic without modifying the checkout.
cmd_self_test() {
  local work status log log_digest rc
  local binding_input expected_binding before_status before_log
  local missing_input malformed_input symlink_input oversized_input duplicate_input
  local unsorted_input incomplete_input malformed_descriptor_input hostile
  local mismatch_revision mismatch_job mismatch_host mismatch_slot
  local -a hostile_inputs=()
  work="$(mktemp -d "${TMPDIR:-/tmp}/ci-gate-self-test.XXXXXX")"
  trap 'rm -rf -- "$work"' RETURN
  LOG="$work/policy.log"
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

  status="$work/status.json"
  log="$work/ci-gate.log"
cat >"$log" <<'EOF'
MCNF CI gate — self-test
  sha=0123456  revision=0123456789abcdef0123456789abcdef01234567  job_id=self-test-job  host=172.20.0.130 (slot=self-test-slot)
=== CI GATE SUMMARY self-test → green ===
  policy   pass
  fmt      pass
  clippy   pass
  test     pass  (1 passed, 0 failed)
  coverage pass
EOF
  log_digest="$(file_sha256 "$log")"
  jq -n \
    --arg sha 0123456789abcdef0123456789abcdef01234567 \
    --arg log "$log" --arg digest "$log_digest" \
    '{overall:"green",alert:false,failed_stage:"",stages:{policy:"pass",fmt:"pass",clippy:"pass",test:"pass",coverage:"pass"},tests_passed:1,tests_failed:0,sha:$sha,short_sha:($sha[0:7]),job_id:"self-test-job",build_host:"172.20.0.130",build_slot:"self-test-slot",evidence:{revision:$sha,gate_log:{path:"ci-gate.log",sha256:$digest}},started:"self-test",finished:"self-test",source:"ci-gate"}' \
    >"$status"
  verify_status "$status" >/dev/null || {
    echo "ci-gate.sh: SELF-TEST FAILED — valid green status was rejected" >&2
    return 1
  }
  set +e
  verify_status "$status" --expected-revision different-source-revision >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || {
    echo "ci-gate.sh: SELF-TEST FAILED — mismatched expected revision was accepted" >&2
    return 1
  }
  verify_status "$status" \
    --expected-revision 0123456789abcdef0123456789abcdef01234567 \
    --expected-job-id self-test-job \
    --expected-build-host 172.20.0.130 \
    --expected-build-slot self-test-slot >/dev/null || {
    echo "ci-gate.sh: SELF-TEST FAILED — matching farm identity was rejected" >&2
    return 1
  }
  "$HERE/ci-gate.sh" verify "$status" \
    --expected-revision 0123456789abcdef0123456789abcdef01234567 \
    --expected-job-id self-test-job \
    --expected-build-host 172.20.0.130 \
    --expected-build-slot self-test-slot >/dev/null || {
    echo "ci-gate.sh: SELF-TEST FAILED — CLI verify rejected matching farm identity" >&2
    return 1
  }
  set +e
  "$HERE/ci-gate.sh" verify "$status" --expected-build-host wrong-host >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || {
    echo "ci-gate.sh: SELF-TEST FAILED — CLI accepted mismatched farm host" >&2
    return 1
  }
  set +e
  verify_status "$status" --expected-job-id wrong-job >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || {
    echo "ci-gate.sh: SELF-TEST FAILED — mismatched expected job was accepted" >&2
    return 1
  }
  set +e
  verify_status "$status" --expected-build-slot wrong-slot >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || {
    echo "ci-gate.sh: SELF-TEST FAILED — mismatched expected farm slot was accepted" >&2
    return 1
  }
  set +e
  verify_status "$status" --expected-build-host wrong-host >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || {
    echo "ci-gate.sh: SELF-TEST FAILED — mismatched expected farm host was accepted" >&2
    return 1
  }
  ln -s -- "$status" "$work/status-symlink.json"
  set +e
  verify_status "$work/status-symlink.json" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || {
    echo "ci-gate.sh: SELF-TEST FAILED — status symlink was accepted" >&2
    return 1
  }
  jq '.evidence.gate_log.path = "../ci-gate.log"' "$status" >"$work/path-status.json"
  set +e
  verify_status "$work/path-status.json" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || {
    echo "ci-gate.sh: SELF-TEST FAILED — escaping evidence path was accepted" >&2
    return 1
  }
  local incomplete incomplete_digest
  incomplete="$work/incomplete"
  mkdir -p -- "$incomplete"
  cp -- "$log" "$incomplete/ci-gate.log"
  sed -i '/^  coverage[[:space:]]\+pass$/d' "$incomplete/ci-gate.log"
  incomplete_digest="$(file_sha256 "$incomplete/ci-gate.log")"
  jq --arg digest "$incomplete_digest" '.evidence.gate_log.sha256 = $digest' \
    "$status" >"$incomplete/status.json"
  set +e
  verify_status "$incomplete/status.json" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || {
    echo "ci-gate.sh: SELF-TEST FAILED — incomplete green log was accepted" >&2
    return 1
  }
  local contradictory contradictory_digest
  contradictory="$work/contradictory"
  mkdir -p -- "$contradictory"
  cp -- "$log" "$contradictory/ci-gate.log"
  printf '  fmt      fail\n' >>"$contradictory/ci-gate.log"
  contradictory_digest="$(file_sha256 "$contradictory/ci-gate.log")"
  jq --arg digest "$contradictory_digest" '.evidence.gate_log.sha256 = $digest' \
    "$status" >"$contradictory/status.json"
  set +e
  verify_status "$contradictory/status.json" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || {
    echo "ci-gate.sh: SELF-TEST FAILED — contradictory failed log was accepted" >&2
    return 1
  }
  jq '.build_slot = "wrong-slot"' "$status" >"$work/identity-status.json"
  set +e
  verify_status "$work/identity-status.json" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || {
    echo "ci-gate.sh: SELF-TEST FAILED — mismatched farm slot was accepted" >&2
    return 1
  }
  local duplicate_identity duplicate_identity_digest
  duplicate_identity="$work/duplicate-identity"
  mkdir -p -- "$duplicate_identity"
  cp -- "$log" "$duplicate_identity/ci-gate.log"
  sed -n '2p' "$log" >>"$duplicate_identity/ci-gate.log"
  duplicate_identity_digest="$(file_sha256 "$duplicate_identity/ci-gate.log")"
  jq --arg digest "$duplicate_identity_digest" \
    '.evidence.gate_log.sha256 = $digest' "$status" >"$duplicate_identity/status.json"
  set +e
  verify_status "$duplicate_identity/status.json" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || {
    echo "ci-gate.sh: SELF-TEST FAILED — duplicated producer identity was accepted" >&2
    return 1
  }

  binding_input="$work/release-binding.json"
  jq -nS \
    --arg revision 0123456789abcdef0123456789abcdef01234567 \
    '{schema_version:1,source_commit:$revision,
      artifacts:[
        {path:"artifacts/a.rpm",size_bytes:17,sha256:("a" * 64)},
        {path:"artifacts/b.raw",size_bytes:31,sha256:("b" * 64)}],
      farm:{job_id:"self-test-job",build_host:"172.20.0.130",build_slot:"self-test-slot"}}' \
    >"$binding_input"
  missing_input="$work/missing-binding.json"
  malformed_input="$work/malformed-binding.json"
  printf '{not-json\n' >"$malformed_input"
  symlink_input="$work/symlink-binding.json"
  ln -s -- "$binding_input" "$symlink_input"
  oversized_input="$work/oversized-binding.json"
  truncate -s $((MAX_RELEASE_BINDING_BYTES + 1)) "$oversized_input"
  duplicate_input="$work/duplicate-binding.json"
  jq '.artifacts += [.artifacts[0]] | .artifacts |= sort_by(.path)' \
    "$binding_input" >"$duplicate_input"
  unsorted_input="$work/unsorted-binding.json"
  jq '.artifacts |= reverse' "$binding_input" >"$unsorted_input"
  incomplete_input="$work/incomplete-binding.json"
  jq 'del(.farm)' "$binding_input" >"$incomplete_input"
  malformed_descriptor_input="$work/malformed-descriptor-binding.json"
  jq '.artifacts[0].sha256 = "not-a-digest"' \
    "$binding_input" >"$malformed_descriptor_input"
  mismatch_revision="$work/mismatch-revision-binding.json"
  jq '.source_commit = "ffffffffffffffffffffffffffffffffffffffff"' \
    "$binding_input" >"$mismatch_revision"
  mismatch_job="$work/mismatch-job-binding.json"
  jq '.farm.job_id = "wrong-job"' "$binding_input" >"$mismatch_job"
  mismatch_host="$work/mismatch-host-binding.json"
  jq '.farm.build_host = "wrong-host"' "$binding_input" >"$mismatch_host"
  mismatch_slot="$work/mismatch-slot-binding.json"
  jq '.farm.build_slot = "wrong-slot"' "$binding_input" >"$mismatch_slot"
  hostile_inputs=(
    "$missing_input" "$malformed_input" "$symlink_input" "$oversized_input"
    "$duplicate_input" "$unsorted_input" "$incomplete_input"
    "$malformed_descriptor_input" "$mismatch_revision" "$mismatch_job"
    "$mismatch_host" "$mismatch_slot"
  )
  before_status="$(file_sha256 "$status")"
  before_log="$(file_sha256 "$log")"
  for hostile in "${hostile_inputs[@]}"; do
    set +e
    cmd_bind_release "$hostile" "$status" >/dev/null 2>&1
    rc=$?
    set -e
    [ "$rc" -ne 0 ] || {
      echo "ci-gate.sh: SELF-TEST FAILED — hostile release binding input was accepted: $hostile" >&2
      return 1
    }
    [ "$(file_sha256 "$status")" = "$before_status" ] \
      && [ "$(file_sha256 "$log")" = "$before_log" ] || {
        echo "ci-gate.sh: SELF-TEST FAILED — rejected release binding changed gate evidence: $hostile" >&2
        return 1
      }
  done
  set +e
  cmd_bind_release >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || {
    echo "ci-gate.sh: SELF-TEST FAILED — missing release binding argument was accepted" >&2
    return 1
  }

  expected_binding="$(jq -cS . "$binding_input" | sha256sum | awk '{print $1}')"
  cmd_bind_release "$binding_input" "$status" >/dev/null || {
    echo "ci-gate.sh: SELF-TEST FAILED — valid final release binding was rejected" >&2
    return 1
  }
  [ "$(grep -Fxc -- "  release-evidence-binding sha256=$expected_binding" "$log" || true)" -eq 1 ] \
    && [ "$(grep -Ec '^  release-evidence-binding sha256=[0-9a-f]{64}$' "$log" || true)" -eq 1 ] || {
      echo "ci-gate.sh: SELF-TEST FAILED — final release binding was not emitted exactly once" >&2
      return 1
    }
  verify_status "$status" >/dev/null || {
    echo "ci-gate.sh: SELF-TEST FAILED — bound gate status did not authenticate its updated log" >&2
    return 1
  }
  before_status="$(file_sha256 "$status")"
  before_log="$(file_sha256 "$log")"
  set +e
  cmd_bind_release "$binding_input" "$status" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] \
    && [ "$(file_sha256 "$status")" = "$before_status" ] \
    && [ "$(file_sha256 "$log")" = "$before_log" ] || {
      echo "ci-gate.sh: SELF-TEST FAILED — duplicate release binding was accepted or changed evidence" >&2
      return 1
    }
  printf 'tampered\n' >>"$log"
  set +e
  verify_status "$status" >/dev/null 2>&1
  rc=$?
  set -e
  [ "$rc" -ne 0 ] || {
    echo "ci-gate.sh: SELF-TEST FAILED — tampered gate log was accepted" >&2
    return 1
  }
  echo "ci-gate.sh: self-test passed — policy failures, authenticated status, and fail-closed release binding propagate"
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
  run)      with_state_lock cmd_run ;;
  policy)   with_state_lock cmd_policy ;;
  --self-test) cmd_self_test ;;
  verify)
    verify_status "${@:2}"
    ;;
  bind-release)
    with_state_lock cmd_bind_release "${@:2}"
    ;;
  poll)     with_state_lock cmd_poll ;;
  liveness) cmd_liveness ;;
  -h | --help | help) usage ;;
  *) usage; exit 1 ;;
esac
