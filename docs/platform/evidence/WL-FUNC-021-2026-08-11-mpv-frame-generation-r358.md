# WL-FUNC-021 mpv frame generation — 2026-08-11

- Scope: replacement load immediately revokes prior-frame authority and restores
  capture only after ordered `StartFile` then `FileLoaded` for the current load.
- Hostile boundary: stale `FileLoaded`, terminal events, and explicit stops
  cannot reuse a prior-generation frame as playback proof.
- Focused gate: `cargo test -p mde-media-core --features mpv mpv::tests::replacement_load_cannot_reuse_prior_generation_frame_as_playback_proof -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 12,383,240 KiB free and real `libmpv`.
- Result: **PASS**, 1 passed, 0 failed, 280 filtered out.
- Remaining boundary: real installed-player replacement-frame proof remains.
