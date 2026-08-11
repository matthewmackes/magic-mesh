# WL-FUNC-021 video adjustment revocation — 2026-08-11

- Scope: every replacement configuration explicitly emits zoom, pan, and crop,
  including their neutral values.
- Hostile boundary: mpv-global frame adjustments retained from a prior media
  generation cannot alter the replacement item.
- Focused gate: `cargo test -p mde-media-core video::tests::replacement_config_revokes_retained_frame_adjustments -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 1, admitted with 12,241,288 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 276 filtered out.
- Remaining boundary: live replacement-video and installed-player proof remain.
