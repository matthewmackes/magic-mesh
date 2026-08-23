#!/usr/bin/env bash
# tick-fill.sh — start the existing @farm fill oneshot without waiting.
#
# Shared by every agent runtime (Cursor / Codex / Claude) after a commit or
# push. This is not a second scheduler: it only `systemctl start --no-block`
# mcnf-farm-reconcile.service. The oneshot is automation/reconciler/farm-reconcile.sh.
#
# Usage: tick-fill.sh [--self-test]
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

self_test() {
  echo "tick-fill --self-test:"
  if [ -x "$HERE/tick-fill.sh" ]; then
    echo "  ok: script is executable"
  else
    echo "  FAIL: $HERE/tick-fill.sh is not executable" >&2
    return 1
  fi
  if grep -q 'systemctl start --no-block mcnf-farm-reconcile.service' "$HERE/tick-fill.sh"; then
    echo "  ok: starts the existing oneshot"
  else
    echo "  FAIL: does not start mcnf-farm-reconcile.service" >&2
    return 1
  fi
  echo "tick-fill: self-test passed"
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit $?
fi

if ! command -v systemctl >/dev/null 2>&1; then
  echo "tick-fill: systemctl absent — run $HERE/farm-reconcile.sh" >&2
  exit 2
fi
if ! systemctl cat mcnf-farm-reconcile.service >/dev/null 2>&1; then
  echo "tick-fill: mcnf-farm-reconcile.service is not installed" >&2
  exit 2
fi

systemctl start --no-block mcnf-farm-reconcile.service
echo "tick-fill: started mcnf-farm-reconcile.service (no-block)"
