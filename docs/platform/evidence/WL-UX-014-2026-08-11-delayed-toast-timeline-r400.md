# WL-UX-014 delayed toast timeline — 2026-08-11

- Scope: elapsed wall time must advance the KIRON queue truthfully after a delayed shell frame.
- Hostile boundary: one delayed tick drains every expired timed alert but cannot cross a grade-F `UntilAck` hold.
- Focused gate: `cargo test -p mde-egui toast::tests::delayed_tick_consumes_elapsed_across_timed_queue_but_cannot_cross_ack_hold -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 2, admitted with 11,852,628 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 295 filtered out.
- Remaining boundary: live shell frame stalls, suspend/resume, hover pause, and grade-transition render timing remain.
