# WL-FUNC-016 — expired consent capacity cleanup (r22)

Date: 2026-08-10

Base revision: `b2895c9f`

## Defect and correction

Expired clipboard consent records remained in the bounded 256-session ledger.
An attacker or long-running fleet could therefore exhaust every slot with dead
sessions and deny a fresh authenticated session even though none retained
authority.

Every consent-lane sweep now removes expired records before admission,
including an empty sweep. Fresh consent can reuse released capacity; current
unexpired consent and all existing identity/sequence validation remain intact.

## Focused farm proof

Machine 193 (`172.20.0.90`) passed the exact
`consent_sweep_releases_expired_capacity_before_fresh_admission` regression:
1 passed, 0 failed, 4,661 filtered out. `git diff --check` passed.

Source SHA-256:

- `62b5cc6e23250f5f0a72f8290db400f740ae173df2ff4fd59f8a25eda5190e94`
  — `crates/mesh/mackesd/src/workers/clipboard_sync.rs`

This closes the dead-consent capacity denial. Local/mesh/VDI proof on no more
than three physical test seats remains part of WL-FUNC-016.
