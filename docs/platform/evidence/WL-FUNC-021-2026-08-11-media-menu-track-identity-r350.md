# WL-FUNC-021 Media menu track identity — 2026-08-11

- Scope: track actions bind the exact media identity, track kind, and track ID,
  then revalidate against the current enumeration on activation.
- Hostile boundary: a stale action cannot select the same numeric track ID on
  replacement media and instead fails with an explicit status.
- Focused gate: `cargo test -p mde-media-egui menubar::tests::stale_track_menu_action_cannot_select_same_numeric_id_on_replacement_media -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 1, admitted with 11,137,928 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 113 filtered out.
- Remaining boundary: live replacement-media menu interaction remains.
