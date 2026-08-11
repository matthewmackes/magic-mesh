# WL-UX-013 history privacy epoch evidence — 2026-08-11

- Scope: resolved Health incidents are retained across restart only inside the
  fleet-wide six-hour privacy epoch; the exact six-hour boundary remains valid.
- Hostile boundary: older resolved rows, future-dated rows, and unresolved
  history cannot survive restart carry-forward as retained incident history.
- Focused gate: `cargo test -p mackesd --lib workers::node_grade::tests::restart_prunes_resolved_history_outside_the_six_hour_privacy_epoch -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 2.
- Result: **PASS**, 1 passed, 0 failed, 4,838 filtered out.
- Remaining boundary: full recurrence/history UI, export, live transitions, and
  three-seat acceptance remain open.
