# WL-FUNC-022 Clock action payload — 2026-08-11

- Scope: retained Clock controls require the complete daemon-admitted action payload, not only display identity.
- Hostile boundary: a restarted Notification Center cannot authorize a replaced action payload through a retained row.
- Focused gate: `cargo test -p mde-shell-egui notification_center::tests::restarted_notification_center_cannot_reauthorize_replaced_clock_action_payload -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on BigBoy `172.20.0.130`, slot 1.
- Result: **PASS**, exact hostile regression passed.
- Remaining boundary: exercise retained notification actions against a live restarted Clock daemon.
