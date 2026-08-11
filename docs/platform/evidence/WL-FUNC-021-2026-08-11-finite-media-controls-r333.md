# WL-FUNC-021 finite media controls — 2026-08-11

- Scope: active-player speed, audio-delay, and A-B loop commands receive only
  finite, ordered numeric values.
- Hostile boundary: invalid speed normalizes to `1.0`, nonfinite delay clears to
  zero, and nonfinite, negative, or reversed loops turn off.
- Focused gate: `cargo test -p mde-media-core controls::tests::malformed_numeric_controls_cannot_reach_the_active_player -- --exact --nocapture`.
- Farm: BigBoy `172.20.0.130`, slot 2, admitted with 9,430,100 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 271 filtered out.
- Remaining boundary: live control interaction and installed-player proof remain.
