#!/usr/bin/env bash
# nightly.sh — BUILD-PLATFORM-7: the nightly internal-test run + visibility.
#
# Runs the test pyramid off the critical path: L1 install → L2 feature → L3
# stability (each hermetic on the snapshot-reset pool), aggregates a report, and
# publishes a summary to the Bus (`event/test/nightly`) so a rotting safety net is
# obvious without asking an AI. Driven by mcnf-nightly-tests.timer.
#
# Usage:  nightly.sh        (cuts a fresh RPM if none, then runs all tiers)
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
ARTIFACTS="${MCNF_BUILD_ARTIFACTS:-$HOME/mcnf-release-artifacts}"
REPORT="${MCNF_FARM_STATE:-$REPO/automation/.state}/nightly-report.txt"
mkdir -p "$(dirname "$REPORT")"

ts() { date -u +%Y-%m-%dT%H:%M:%SZ; }
echo "MCNF nightly internal tests — $(ts)" | tee "$REPORT"

# Ensure an RPM exists (cut one on the farm if not).
RPM="$(ls -t "$ARTIFACTS"/*.rpm 2>/dev/null | head -1)"
if [ -z "$RPM" ]; then
  echo "no RPM — cutting one on the farm" | tee -a "$REPORT"
  MCNF_BUILD_HOST=172.20.0.52 "$REPO/install-helpers/xcp-build.sh" rpm >>"$REPORT" 2>&1 || true
  RPM="$(ls -t "$ARTIFACTS"/*.rpm 2>/dev/null | head -1)"
fi
[ -n "$RPM" ] || { echo "FATAL: no RPM to test" | tee -a "$REPORT"; exit 1; }
echo "RPM: $(basename "$RPM")" | tee -a "$REPORT"

declare -A OUT
run_tier() { # <name> <script>
  local name="$1" script="$2"
  echo "--- $name ---" | tee -a "$REPORT"
  if "$HERE/$script" "$RPM" >>"$REPORT" 2>&1; then OUT[$name]=pass; else OUT[$name]=fail; fi
  echo "$name → ${OUT[$name]}" | tee -a "$REPORT"
}
run_tier "L1-install"   test-install.sh
run_tier "L2-feature"   test-feature.sh
run_tier "L3-stability" test-stability.sh

# Aggregate + publish (the visibility — readable without an AI).
fails=0; for k in "${!OUT[@]}"; do [ "${OUT[$k]}" = pass ] || fails=$((fails+1)); done
overall="green"; [ "$fails" -eq 0 ] || overall="RED"
{
  echo "=== NIGHTLY SUMMARY $(ts) → $overall ==="
  for k in L1-install L2-feature L3-stability; do printf '  %-14s %s\n' "$k" "${OUT[$k]:-skipped}"; done
} | tee -a "$REPORT"
command -v mde-bus >/dev/null 2>&1 && mde-bus publish "event/test/nightly" --body-flag \
  "{\"overall\":\"$overall\",\"l1\":\"${OUT[L1-install]:-skip}\",\"l2\":\"${OUT[L2-feature]:-skip}\",\"l3\":\"${OUT[L3-stability]:-skip}\",\"alert\":$([ "$fails" -eq 0 ] && echo false || echo true)}" 2>/dev/null || true
[ "$fails" -eq 0 ]
