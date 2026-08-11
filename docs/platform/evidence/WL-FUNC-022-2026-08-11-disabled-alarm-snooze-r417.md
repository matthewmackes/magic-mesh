# WL-FUNC-022 disabled alarm snooze cancellation — 2026-08-11

- Scope: disabling an alarm durably cancels every scheduled snooze generation.
- Hostile boundary: an already auto-disabled one-time alarm cannot retain a snooze child that rings later.
- Focused gate: `cargo test -p mackesd workers::clock::tests::disabled_alarm_cannot_ring_a_preexisting_snooze_generation -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 1, admitted with 10,642,724 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,875 filtered out.
- Remaining boundary: disable a snoozed live alarm before its deadline and verify silence across daemon restart and selected peers.
