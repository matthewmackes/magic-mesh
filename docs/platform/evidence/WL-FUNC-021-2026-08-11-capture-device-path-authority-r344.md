# WL-FUNC-021 capture device path authority — 2026-08-11

- Scope: capture authority admits only canonical direct Linux device paths for
  video, VBI, radio, media, and V4L subdevices.
- Hostile boundary: traversal, nested paths, leading-zero aliases, and
  overflowing indices cannot manufacture device authority.
- Focused gate: `cargo test -p mde-media-core capture::tests::path_aliases_cannot_manufacture_capture_authority -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 1, admitted with 11,239,452 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 277 filtered out.
- Remaining boundary: live capture hardware and installed-player proof remain.
