# WL-UX-013 — canonical health Bus-twin cursor recovery (r122)

Date: 2026-08-10

Base revision: `45859dd3`

## Defect and correction

Health ingress stages the canonical signed file before consuming its durable
Bus lane. An exact Bus twin therefore met an equal retained generation and was
incorrectly logged as a replay on every cycle. Exact equality now advances the
Bus cursor without a second admission or rejection. A non-identical candidate
at an equal or older generation remains fail-closed.

This directly corrects the repeated live seat-15
`persisted health publication rejected` condition where retained and candidate
generations were identical.

## Focused farm proof

Build VM `.90` (`172.20.0.90`), slot `ux013-health-bus-twin-r120`, passed:

```text
cargo test -p mackesd --lib workers::health_reconciler::tests::health_ingress_advances_exact_bus_twin_of_canonical_state -- --exact --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 4671 filtered out
```

Source SHA-256:

- `8693f3e0692ea5e39929606432f7f5e1cc2356c16166984a9d729eb4d90fec2b`
  — `crates/mesh/mackesd/src/workers/health_reconciler.rs`

Installed-seat log closure and broader transition/recovery proof remain open.
