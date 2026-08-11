# WL-FUNC-021 library playback path identity — 2026-08-11

- Scope: each durable library map key must equal the canonical playback path
  stored in its item declaration.
- Hostile boundary: a trusted lookup identity cannot substitute a different
  attacker-controlled playback path after restart.
- Focused gate: `cargo test -p mde-media-core library::tests::persisted_item_cannot_substitute_a_different_playback_path_after_restart -- --exact --nocapture`.
- Farm: BigBoy `172.20.0.130`, slot 1, admitted with 9,430,100 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 271 filtered out.
- Remaining boundary: live library playback and installed-player proof remain.
