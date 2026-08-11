# WL-FUNC-021 audio sink revocation — 2026-08-11

- Scope: clearing an explicit sink emits mpv's neutral `audio-device=auto`
  configuration.
- Hostile boundary: a device retained globally by mpv from the prior generation
  cannot remain the replacement configuration's sink.
- Focused gate: `cargo test -p mde-media-core audio::tests::clearing_explicit_device_revokes_the_stale_sink -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1.
- Result: **PASS**, 1 passed, 0 failed, 276 filtered out.
- Remaining boundary: live sink transition and installed-player proof remain.
