#!/bin/sh
# lint-shared-substrate.sh — OB6-FIX-2: guard against the ONBOARD-6 audit gap.
#
# The gap: a feature whose code is 100% reachable + unit-tested can still
# silently no-op in the mesh because its INFRASTRUCTURE PRECONDITION (QNM-Shared
# being a real replicated mount, not a local dir) was never provisioned. §7 +
# the audit verify code, not deployed substrate; tests bind QNM-Shared to a
# tempdir, identical to a mount — so "works locally" masqueraded as "works in
# the mesh" (NO LEADER / node_count 0).
#
# This lint can't prove a substrate is deployed, but it keeps the GUARDRAILS
# that catch the class from being silently removed:
#   1. the mesh-health watchdog must still fail-loud if QNM-Shared isn't a mount
#      (OB6-FIX-3);
#   2. the multi-node shared-state test must still exist + assert the cross-node
#      invariants (OB6-FIX-1);
# and it surfaces (advisory) new non-test readers of the shared root so a
# reviewer asks "is the substrate asserted / multi-node-tested?".
#
# Exit 0 = guardrails intact; 1 = a guardrail was removed.
set -eu
REPO="$(cd "$(dirname "$0")/.." && pwd)"
fail=0

# 1. The fail-loud mount assertion in the watchdog.
hc="$REPO/install-helpers/mesh-health-check.sh"
if ! grep -q 'QNM-Shared not mounted' "$hc" 2>/dev/null; then
    echo "lint-shared-substrate: FAIL — mesh-health-check.sh lost its QNM-Shared mount assertion (OB6-FIX-3)" >&2
    fail=1
fi

# 2. The multi-node shared-state gate.
t="$REPO/crates/mesh/mackesd/tests/mesh_shared_state.rs"
if ! grep -q 'exactly_one_leader_is_elected_across_nodes_on_a_shared_volume' "$t" 2>/dev/null \
   || ! grep -q 'healthz_counts_reflect_the_shared_mesh' "$t" 2>/dev/null; then
    echo "lint-shared-substrate: FAIL — the multi-node shared-state test (OB6-FIX-1) is missing/gutted" >&2
    fail=1
fi

# 3. Advisory: new non-test readers of the shared root. Not an error (many are
#    legitimate), but each should be covered by a mount assertion or the
#    multi-node test — a reviewer prompt, not a gate.
readers=$(grep -rln 'default_qnm_shared_root\|QNM_SHARED_ROOT' "$REPO"/crates --include='*.rs' 2>/dev/null \
    | grep -v '/tests/' | grep -vc 'lib.rs' || true)
echo "lint-shared-substrate: ${readers} shared-root reader file(s) — confirm cross-node behavior is asserted (multi-node test) not just tempdir-tested"

if [ "$fail" -eq 0 ]; then
    echo "lint-shared-substrate.sh: clean — substrate guardrails intact (OB6-FIX-1/3)"
fi
exit "$fail"
