# WL-UX-009 disabled status tone — 2026-08-11

- Scope: disabled Quazar workspaces override retained live status tones at the shared paint boundary across Dark, Light, and Car appearances.
- Hostile boundary: unavailable chrome cannot continue presenting success, warning, or error semantics from cached state.
- Focused gate: `cargo test -p mde-egui menubar::tests::disabled_workspace_cannot_retain_a_live_status_tone_across_appearances -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on BigBoy `172.20.0.130`, slot 1.
- Result: **PASS**, exact hostile regression passed.
- Remaining boundary: capture disabled status chips in all three appearances on representative live surfaces.
