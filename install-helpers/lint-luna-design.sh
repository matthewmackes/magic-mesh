#!/usr/bin/env bash
# lint-luna-design.sh — verify the Luna build-manager design stays implementable
# and does not drift into a second scheduler or a stale lifecycle model.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
DESIGN="$REPO/docs/design/luna-build-manager.md"
DISPATCH="$REPO/automation/lib/farm-dispatch.sh"
RECONCILE="$REPO/automation/reconciler/farm-reconcile.sh"
AGENT="$REPO/automation/drain/agent-dispatch.sh"
NATIVE="$REPO/automation/drain/native-agent-dispatch.sh"
TOPOLOGY="$REPO/install-helpers/farm-topology.sh"

fail() { echo "lint-luna-design: FAIL: $*" >&2; exit 1; }

[ -f "$DESIGN" ] || fail "missing $DESIGN"
[ -f "$DISPATCH" ] || fail "missing $DISPATCH"
[ -f "$RECONCILE" ] || fail "missing $RECONCILE"
[ -f "$AGENT" ] || fail "missing $AGENT"
[ -f "$NATIVE" ] || fail "missing $NATIVE"
[ -f "$TOPOLOGY" ] || fail "missing $TOPOLOGY"

grep -q 'Luna — dependable 24-hour build manager' "$DESIGN" || fail "design title missing"
grep -q 'No Jenkins/Buildkite/GitLab/Nomad/Kubernetes control plane' "$DESIGN" || fail "design must forbid a new central scheduler"
grep -q 'slot-aware dispatcher' "$DESIGN" || fail "design must preserve the slot-aware dispatcher"
grep -q 'Forgejo Actions' "$DESIGN" || fail "design must preserve the Forgejo lane"
grep -q 'stale' "$DESIGN" || fail "design must define stale lifecycle handling"
grep -q 'salvaged' "$DESIGN" || fail "design must define salvage handling"
grep -q 'heartbeat' "$DESIGN" || fail "design must define heartbeat handling"
grep -q 'duplicate prevention' "$DESIGN" || fail "design must define duplicate prevention"
grep -q 'disk-shaped admission' "$DESIGN" || fail "design must define disk-shaped admission"
grep -q '24-hour' "$DESIGN" || fail "design must define 24-hour operation"

# The implementation must still expose the management surfaces the design relies on.
grep -q 'slots)' "$DISPATCH" || fail "farm-dispatch.sh must expose slots"
grep -q 'nodes)' "$DISPATCH" || fail "farm-dispatch.sh must expose nodes"
grep -q 'result)' "$DISPATCH" || fail "farm-dispatch.sh must expose result"
grep -q -- '--self-test' "$DISPATCH" || fail "farm-dispatch.sh must expose --self-test"
grep -q -- '--self-test' "$RECONCILE" || fail "farm-reconcile.sh must expose --self-test"
grep -q -- '--self-test' "$AGENT" || fail "agent-dispatch.sh must expose --self-test"
grep -q -- '--self-test' "$NATIVE" || fail "native-agent-dispatch.sh must expose --self-test"
grep -q 'table)' "$TOPOLOGY" || fail "farm-topology.sh must expose table"

# Guard against reintroducing a stale liveness model.
if grep -n 'job already dispatched' "$NATIVE" >/dev/null 2>&1; then
  fail "native-agent-dispatch.sh still treats worktree existence as liveness"
fi

echo "lint-luna-design: clean"
