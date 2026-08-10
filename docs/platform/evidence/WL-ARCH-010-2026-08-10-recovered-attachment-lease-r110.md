# WL-ARCH-010 — recovered attachment lease truth (r110)

Date: 2026-08-10

Base revision: `b2895c9f`

## Defect and correction

Restart recovery accepted an actuator outcome that reported a completed,
`Ready` StartAndAttach operation without returning the authoritative Display1
lease. That could revoke the old lease while still projecting a usable session.

Recovery now refuses that contradictory outcome, revokes the stale descriptor,
clears the attachment, and persists `Unavailable` with a bounded reason. Only an
exact validated lease can restore a ready attachment after restart.

## Focused farm proof

Machine 193 (`172.20.0.90`) passed the exact
`recovered_ready_without_authoritative_lease_is_refused_and_unpublished`
regression: 1 passed, 0 failed, 4,662 filtered out. `git diff --check` passed.

Source SHA-256:

- `259418b11e095e5a09c5fc7f7ff2e16b9a27386f94695d16d835a5a99fe0754b`
  — `crates/mesh/mackesd/src/workers/workload_compute.rs`

This closes one restart false-ready path. Live Display1/KMS recovery and the
remaining integrated WL-ARCH-010 acceptance stay open.
