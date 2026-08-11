# WL-FUNC-021 MPRIS audible generation — 2026-08-11

- Scope: `SetPosition` validates against the engine's audible generation
  (`play_base + current_track_index`) rather than an advanced durable cursor.
- Hostile boundary: stopped, out-of-range, overflowed, and stale queue generations
  cannot seek a different audible track.
- Focused gate: `cargo test -p mde-musicd mpris::tests::stale_queue_cursor_cannot_authorize_seek_on_a_different_audible_track -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 2, admitted with 9,296,428 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 258 filtered out.
- Remaining boundary: live MPRIS seek and installed-daemon proof remain.
