# WL-FUNC-021 frame layout authority — 2026-08-11

- Scope: a decoded RGBA frame is nonblank only when positive dimensions map
  exactly to its bounded byte buffer.
- Hostile boundary: zero-area, overflowing geometry, and truncated or overlong
  buffers fail closed as blank instead of proving playback success.
- Focused gate: `cargo test -p mde-media-core engine::tests::malformed_rgba_layout_cannot_report_nonblank_decode_success -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1, admitted with 8,401,592 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 273 filtered out.
- Remaining boundary: live decoded-frame and installed-player proof remain.
