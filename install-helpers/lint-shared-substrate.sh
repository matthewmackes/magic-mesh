#!/bin/sh
# lint-shared-substrate.sh — OB6-FIX-2: guard against the ONBOARD-6 audit gap.
#
# The gap: a feature whose code is 100% reachable + unit-tested can still
# silently no-op in the mesh because its INFRASTRUCTURE PRECONDITION (the shared
# root being a real replicated dir, not a bare local dir) was never provisioned.
# Tests bind the shared root to a tempdir, identical to a real share — so "works
# locally" masqueraded as "works in the mesh" (NO LEADER / node_count 0).
#
# This lint can't prove a substrate is deployed, but it keeps the GUARDRAILS
# that catch the class from being silently removed:
#   1. the mesh-health watchdog must still fail-loud on etcd quorum loss
#      (SUBSTRATE-11) AND carry NO LizardFS references (SUBSTRATE-6 retired it);
#   2. the multi-node shared-state test must still exist + assert the cross-node
#      invariants (OB6-FIX-1);
# and it surfaces (advisory) new non-test readers of the shared root so a
# reviewer asks "is the substrate asserted / multi-node-tested?".
#
# Exit 0 = guardrails intact; 1 = a guardrail was removed.
set -eu
REPO="$(cd "$(dirname "$0")/.." && pwd)"
fail=0

# 1. The fail-loud shared-state assertion in the watchdog. SUBSTRATE-V2: the
#    plane is etcd (coordination) + Syncthing (files), so the watchdog must
#    assert etcd quorum health and carry NO LizardFS/qnm-shared paths.
hc="$REPO/install-helpers/mesh-health-check.sh"
if ! grep -q 'etcd unreachable' "$hc" 2>/dev/null; then
    echo "lint-shared-substrate: FAIL — mesh-health-check.sh lost its etcd-quorum readiness guard (SUBSTRATE-11)" >&2
    fail=1
fi
if grep -qiE 'lizardfs|mfsmount|qnm-shared\.service' "$hc" 2>/dev/null; then
    echo "lint-shared-substrate: FAIL — mesh-health-check.sh still references retired LizardFS (SUBSTRATE-6 removed it)" >&2
    fail=1
fi

# 2. The etcd multi-node gate — exactly-one-leader + the peer directory exercised
#    against a REAL etcd container (SUBSTRATE-11, replaces the tempdir gate).
t="$REPO/crates/mesh/mackesd/tests/substrate_etcd.rs"
if ! grep -q 'etcd_leader_election_elects_one_renews_and_force_takes' "$t" 2>/dev/null \
   || ! grep -q 'etcd_peer_directory_round_trips_and_deletes' "$t" 2>/dev/null; then
    echo "lint-shared-substrate: FAIL — the etcd multi-node gate (SUBSTRATE-2/3) is missing/gutted" >&2
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
