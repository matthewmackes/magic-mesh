# WL-UX-014 status-grade watermark — 2026-08-11

- Scope: status-bar health chrome binds to the local observer and a monotonic validated A–F snapshot watermark.
- Hostile boundary: foreign, rolled-back, or equal-generation conflicting health cannot relabel the retained grade; corrected-forward state recovers.
- Focused gate: `cargo test -p mde-shell-egui status_bar::tests::restarted_status_bar_cannot_relabel_health_grade_from_foreign_or_rolled_back_generation -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on BigBoy `172.20.0.130`, slot 1.
- Result: **PASS**, 1 passed, 0 failed, 1,566 filtered out.
- Remaining boundary: capture foreign and corrected-forward grade transitions on a live direct-DRM status bar.
