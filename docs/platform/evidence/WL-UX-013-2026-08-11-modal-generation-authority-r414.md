# WL-UX-013 modal generation authority — 2026-08-11

- Scope: the open Health modal must retain the newest admitted snapshot authority across live updates.
- Hostile boundary: a fresh-timestamp lower-generation snapshot cannot erase an active outage; replacement requires the same observer plus advancing generation and publication time.
- Focused gate: `cargo test -p mde-shell-egui health_modal::tests::lower_generation_live_update_cannot_replace_admitted_health_authority -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1, admitted with 9,452,704 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 1,558 filtered out.
- Remaining boundary: live production projection rollback while the modal is open and corrected-forward recovery proof remain.
