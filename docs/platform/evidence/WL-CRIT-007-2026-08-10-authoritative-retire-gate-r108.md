# WL-CRIT-007 — authoritative lighthouse retirement gate (r108)

Date: 2026-08-10

Base revision: `4ba2428d`

## Defect

`mackesd lighthouse retire` decided whether a destructive retirement preserved
the HA floor by counting lighthouse rows in the replicated peer directory.
Those rows are desired/discovery state, not authoritative etcd membership, and
can outlive an unreachable voter. A stale row could therefore satisfy the
floor before certificate revocation, membership removal, and droplet deletion.

## Correction

Retirement now fails closed before any local decommission or revoke unless it
can:

1. connect to the configured etcd cluster and read its authoritative member
   list;
2. derive each member's client endpoint from its advertised raft endpoint;
3. complete a bounded linearizable read through each exact member; and
4. prove that removing the selected reachable voter leaves at least
   `HA_MIN_LIGHTHOUSES` directly reachable, started, non-learner voters.

Unstarted members, learners, and unreachable members never count as survivors.
An already-absent target remains an idempotent retry only when the surviving
live voters independently satisfy the floor. `--force` still overrides the HA
floor, but cannot bypass the authoritative preflight or configured-endpoint
requirement. The unused directory-count authority was deleted.

## Current live boundary

A read-only probe through lighthouse `.1` found all three current members
started and directly reachable at `10.42.0.1` through `.3`. All held term 5534
with raft/applied index 456477 and no endpoint errors. This means the corrected
gate refuses to retire any current member from the three-voter fleet; a fourth
replacement must first join and converge.

No membership, certificate, directory, package, or droplet mutation was made
for this checkpoint. Lighthouses `.2` and `.3` therefore remain on release 5;
their corrected-forward package rollout still requires the supported
add/converge/retire sequence.

## Focused farm proof

- machine 196, slot `lh-retire-policy-r108`: the two authoritative policy
  tests passed, covering stale/unstarted/learner exclusion, safe four-to-three,
  idempotent retry, floor refusal, and explicit force behavior;
- machine 193, slot `lh-retire-probe-r108`: the exact raft-to-client endpoint
  mapping test passed; and
- machine 193, slot `lh-retire-live-gate`: exact Rust formatting passed for the
  three changed lifecycle files.

The tests compile the production `mackesd` library path. No broad suite or
destructive live retirement was run.
