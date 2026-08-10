# WL-UX-013 — availability duplicate precedence (r13)

Date: 2026-08-10

Base revision: `7f757e23`

## Defect and correction

At the availability ledger's node limit, duplicate evidence could be reported
as capacity exhaustion before duplicate identity was checked. The fold now
checks the incoming identity first, so both forward and reversed duplicate-at-
capacity orderings return `DuplicateEvidence`; a genuinely distinct extra node
still returns `CapacityExceeded`.

## Focused farm proof

Machine 194 (`172.20.0.170`) passed the exact
`evaluation_rejects_duplicate_at_capacity_before_distinct_overflow` regression:
1 passed, 0 failed, 4,663 filtered out. `git diff --check` passed.

Source SHA-256:

- `b07f5feecf405235ec2830293c4df2d8c90bbfd35740e2a6c472d4e1d0bf5152`
  — `crates/mesh/mackesd/src/workers/node_availability.rs`

This closes the duplicate-at-capacity classification defect. Integrated health
history, recovery, and three-seat proof remain open.
