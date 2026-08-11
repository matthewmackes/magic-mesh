# WL-UX-012 future preference schema — 2026-08-11

- Scope: taskbar preferences from an unsupported future schema do not become trusted local state.
- Hostile boundary: future-schema Left placement, pins, favorites, and new-profile state fail closed to an empty Bottom taskbar.
- Focused gate: `cargo test -p mde-shell-egui nav_bar::tests::future_preference_schema_cannot_restore_untrusted_placement_or_pins -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1; its verified-inactive target cache was removed before admission.
- Result: **PASS**, 1 passed, 0 failed, 1,560 filtered out.
- Remaining boundary: install a future-schema preference on a live seat and verify safe restart defaults before reconfiguration.
