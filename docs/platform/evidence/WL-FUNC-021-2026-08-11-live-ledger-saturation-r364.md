# WL-FUNC-021 live ledger saturation — 2026-08-11

- Scope: the six-hour workspace replay ledger prunes expired rows but never
  evicts a still-live request identity to admit another mutation.
- Hostile boundary: a full live ledger returns `ledger_full`, so saturation
  cannot re-enable a consumed signed request inside the privacy epoch.
- Focused gate: `cargo test -p mde-musicd bus_responder::tests::saturated_workspace_ledger_cannot_reenable_consumed_request_inside_privacy_epoch -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 10,270,072 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 261 filtered out.
- Remaining boundary: live sustained-action saturation and installed-daemon proof remain.
