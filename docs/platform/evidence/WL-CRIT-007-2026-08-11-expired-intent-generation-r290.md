# WL-CRIT-007 expired-intent replacement generation evidence — 2026-08-11

- Scope: Node Availability reconciles expired durable intent against the
  current Bus generation before accepting new work.
- Hostile boundary: expiry prevents stale republication but cannot erase
  generation authority. A newer replacement-Bus generation therefore makes an
  equivocal duplicate fail closed instead of overwriting durable truth.
- Focused gate: `cargo test -p mackesd --lib --features async-services workers::node_availability::tests::expired_durable_truth_cannot_ignore_newer_replacement_generation -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 1.
- Result: **PASS**, 1 passed, 0 failed, 4,831 filtered out.
- Remaining boundary: live sleep/resume and fleet convergence acceptance remain.
