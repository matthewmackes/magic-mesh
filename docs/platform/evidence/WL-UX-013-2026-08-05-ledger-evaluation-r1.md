# WL-UX-013 — availability ledger evaluation (2026-08-05)

The daemon availability ledger now evaluates admitted intent against explicit
caller-supplied device-class and last-seen evidence through the shared
device-aware policy. Missing, wrong-node, or contradictory class evidence stays
`Unknown`; expected absence, missed return, and unannounced outage retain their
distinct assessments. Batch evaluation is capacity-bounded, rejects duplicate
evidence, and returns deterministic node order.

## Verification

- Farm `.50`, slot `wl-ux013-ledger-eval-r1`:
  `cargo test -p mackesd node_availability -- --nocapture`.
- Result: `9 passed; 0 failed; 4417 filtered out`.
- File-scoped Rust formatting passed. Actual lifecycle publishers and the live
  node-grade/health worker invocation remain open.
