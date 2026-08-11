# WL-UX-013 health-history capacity authority — 2026-08-11

- Scope: scheduler capacity decisions consume current, monotonic health history.
- Hostile boundary: replayed, rolled-back, or same-generation conflicting health cannot authorize capacity after restart.
- Focused gate: `cargo test -p mackesd workers::scheduler::tests::replayed_or_equivocated_health_history_cannot_authorize_capacity_after_restart -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 2.
- Result: **PASS**, 1 passed, 0 failed.
- Remaining boundary: live expected-state/history capture under node return.
