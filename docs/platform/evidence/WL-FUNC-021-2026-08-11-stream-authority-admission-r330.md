# WL-FUNC-021 stream authority admission — 2026-08-11

- Scope: network playback targets reject malformed, credential-bearing,
  ambiguous, and control-bearing authorities before player contact.
- Hostile boundary: userinfo-like substitution such as
  `trusted.mesh@attacker.example` cannot select the playback source.
- Focused gate: `cargo test -p mde-media-core stream::tests::ambiguous_network_authority_cannot_substitute_the_playback_source -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1, admitted with 9,295,020 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 268 filtered out.
- Remaining boundary: live network-stream and installed-player proof remain.
