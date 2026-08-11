# WL-FUNC-021 Media roster revocation — 2026-08-11

- Scope: missing or malformed Bus roster state revokes retained Jellyfin routes
  and stale UI selection.
- Hostile boundary: the UI cannot continue mesh playback authority from the last
  good roster after current roster admission fails.
- Focused gate: `cargo test -p mde-media-egui app::tests::malformed_roster_revokes_retained_mesh_playback_authority -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 2, admitted with 12,495,568 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 113 filtered out.
- Remaining boundary: live Bus corruption/recovery and installed-UI proof remain.
