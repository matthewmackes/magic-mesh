# WL-UX-009 finite motion restart — 2026-08-11

- Scope: restored DRM motion timelines settle to finite endpoint geometry.
- Hostile boundary: corrupt restart state, non-finite timing, or non-finite targets cannot force continuous repaint.
- Focused gate: `cargo test -p mde-egui motion::tests::restarted_or_non_finite_timeline_cannot_keep_drm_repainting -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1, admitted with 9,572,648 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 296 filtered out.
- Remaining boundary: interrupt active motion with a live DRM/EGL or VT restart and observe the page-flip scheduler settle idle.
