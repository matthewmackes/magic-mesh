# WL-FUNC-022 Timer/alarm snooze boundary — 2026-08-11

- Scope: shell ringing projections and commands preserve the distinction
  between timer occurrences and snoozable alarm occurrences.
- Hostile boundary: a ringing timer cannot enter the alarm-only ringing UI or
  construct a Snooze command.
- Focused gate: `cargo test -p mde-shell-egui timers::tests::ringing_timer_cannot_cross_the_alarm_snooze_authority_boundary -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 2, admitted with 10,550,504 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 1,556 filtered out.
- Remaining boundary: installed-seat interaction and daemon/UI process proof remain.
