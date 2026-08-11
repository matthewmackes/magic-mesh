# WL-FUNC-021 player replacement generation — 2026-08-11

- Scope: a successful media replacement revokes queued observations belonging
  to the superseded identity before the new item becomes active.
- Hostile boundary: stale EOF cannot complete the replacement or suppress its
  readiness; a failed replacement preserves incumbent observations.
- Focused gate: `cargo test -p mde-media-core player::tests::replacement_load_revokes_queued_eof_before_new_identity_can_be_completed -- --exact --nocapture`.
- Farm: BigBoy `172.20.0.130`, slot 2, admitted with 9,803,276 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 270 filtered out.
- Remaining boundary: live replacement playback and installed-player proof remain.
