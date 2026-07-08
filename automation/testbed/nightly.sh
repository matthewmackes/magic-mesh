#!/usr/bin/env bash
# nightly.sh — BUILD-PLATFORM-7: the nightly internal-test run + visibility.
#
# Runs the test pyramid off the critical path: L1 install → L2 feature → L3
# stability → L4 lighthouse-replace (each hermetic on the snapshot-reset pool),
# aggregates a report, and publishes a summary to the Bus (`event/test/nightly`)
# so a rotting safety net is obvious without asking an AI. Driven by
# mcnf-nightly-tests.timer.
#
# Usage:  nightly.sh        (cuts a fresh RPM if none, then runs all tiers)
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
ARTIFACTS="${MCNF_BUILD_ARTIFACTS:-$HOME/mcnf-release-artifacts}"
REPORT="${MCNF_FARM_STATE:-$REPO/automation/.state}/nightly-report.txt"
EAGLE="${MCNF_EAGLE_HOST:-172.20.146.13}"
EAGLE_USER="${MCNF_EAGLE_USER:-mm}"
EAGLE_PASS_FILE="${MCNF_EAGLE_PASS_FILE:-/root/.mcnf-xapi-cred}"
mkdir -p "$(dirname "$REPORT")"

ts() { date -u +%Y-%m-%dT%H:%M:%SZ; }
json_escape() {
  local s="${1//\\/\\\\}"
  s="${s//\"/\\\"}"
  printf '%s' "$s"
}
bus_publish() {
  local topic="$1" body="$2" qbody
  if command -v mde-bus >/dev/null 2>&1; then
    mde-bus publish "$topic" --body-flag "$body" >/dev/null 2>&1 || true
    return 0
  fi
  command -v sshpass >/dev/null 2>&1 || return 0
  [ -f "$EAGLE_PASS_FILE" ] || return 0
  qbody="$(printf '%q' "$body")"
  sshpass -f "$EAGLE_PASS_FILE" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new "$EAGLE_USER@$EAGLE" \
    "command -v mde-bus >/dev/null 2>&1 && mde-bus publish $topic --body-flag $qbody" >/dev/null 2>&1 || true
}
publish_tier() {
  local name="$1" topic="$2" tier="$3" outcome="$4" rpm="$5"
  local safe_rpm
  safe_rpm="$(json_escape "$(basename "$rpm")")"
  bus_publish "$topic" \
    "{\"tier\":\"$tier\",\"outcome\":\"$outcome\",\"rpm\":\"$safe_rpm\",\"source\":\"nightly\",\"stage\":\"$name\"}"
}
echo "MCNF nightly internal tests — $(ts)" | tee "$REPORT"

# Ensure an RPM exists (cut one on the farm if not).
RPM="$(ls -t "$ARTIFACTS"/*.rpm 2>/dev/null | head -1)"
if [ -z "$RPM" ]; then
  echo "no RPM — cutting one on the farm" | tee -a "$REPORT"
  MCNF_BUILD_SHAPE=big "$REPO/install-helpers/xcp-build.sh" rpm >>"$REPORT" 2>&1 || true
  RPM="$(ls -t "$ARTIFACTS"/*.rpm 2>/dev/null | head -1)"
fi
[ -n "$RPM" ] || { echo "FATAL: no RPM to test" | tee -a "$REPORT"; exit 1; }
echo "RPM: $(basename "$RPM")" | tee -a "$REPORT"

declare -A OUT
run_tier() { # <name> <script> <topic> <tier>
  local name="$1" script="$2" topic="$3" tier="$4"
  echo "--- $name ---" | tee -a "$REPORT"
  if "$HERE/$script" "$RPM" >>"$REPORT" 2>&1; then OUT[$name]=pass; else OUT[$name]=fail; fi
  publish_tier "$name" "$topic" "$tier" "${OUT[$name]}" "$RPM"
  echo "$name → ${OUT[$name]}" | tee -a "$REPORT"
}
run_tier "L1-install"    test-install.sh             event/test/install             L1-install
run_tier "L2-feature"    test-feature.sh             event/test/feature/mesh        L2-feature
run_tier "L3-stability"  test-stability.sh           event/test/stability/all       L3-stability
run_tier "L4-lighthouse" test-lighthouse-replace.sh  event/test/lighthouse-replace  L4-lighthouse-replace

# Aggregate + publish (the visibility — readable without an AI).
fails=0; for k in "${!OUT[@]}"; do [ "${OUT[$k]}" = pass ] || fails=$((fails+1)); done
overall="green"; [ "$fails" -eq 0 ] || overall="RED"
{
  echo "=== NIGHTLY SUMMARY $(ts) → $overall ==="
  for k in L1-install L2-feature L3-stability L4-lighthouse; do printf '  %-14s %s\n' "$k" "${OUT[$k]:-skipped}"; done
} | tee -a "$REPORT"
bus_publish event/test/nightly \
  "{\"overall\":\"$overall\",\"l1\":\"${OUT[L1-install]:-skip}\",\"l2\":\"${OUT[L2-feature]:-skip}\",\"l3\":\"${OUT[L3-stability]:-skip}\",\"l4\":\"${OUT[L4-lighthouse]:-skip}\",\"alert\":$([ "$fails" -eq 0 ] && echo false || echo true)}"
[ "$fails" -eq 0 ]
