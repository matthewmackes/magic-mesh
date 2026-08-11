# WL-FUNC-021 queue entry identity — 2026-08-11

- Scope: queue-entry IDs are nonblank, bounded, control-free, and unique before
  entering daemon-owned state.
- Hostile boundary: conflicting tracks cannot share an order-dependent queue
  identity.
- Focused gate: `cargo test -p mde-musicd domain::tests::equivocated_queue_entry_identity_cannot_select_track_by_order -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 2, admitted with 10.6 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 254 filtered out.
- Remaining boundary: live queue mutation and installed-daemon proof remain.
