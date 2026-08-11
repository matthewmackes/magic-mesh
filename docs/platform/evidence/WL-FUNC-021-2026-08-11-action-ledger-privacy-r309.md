# WL-FUNC-021 action-ledger privacy epoch — 2026-08-11

- Scope: the durable Music workspace action ledger records admission time and
  retains request/result history for at most the fleet-wide six-hour epoch.
- Restart boundary: entries exactly six hours old remain; older entries are
  pruned and the file is rewritten immediately. Legacy rows without trustworthy
  time expire during schema-v3 upgrade, and future timestamps fail closed.
- Focused gate: `cargo test -p mde-musicd bus_responder::tests::workspace_ledger_restart_enforces_six_hour_privacy_epoch -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 2.
- Result: **PASS**, 1 passed, 0 failed, 249 filtered out.
- Remaining boundary: live playback/cast/handoff and package/seat acceptance remain.
