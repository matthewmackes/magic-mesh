# WL-FUNC-021 Jellyfin cache digest — 2026-08-11

- Scope: cached titles persist a SHA-256 digest and verify exact content through
  bounded streaming reads before offline playback.
- Hostile boundary: legacy digest-less entries and same-sized substituted files
  fail closed after restart.
- Focused gate: `cargo test -p mde-jellyfin cache::tests::same_sized_substituted_media_is_rejected_after_restart -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1.
- Result: **PASS**, 1 passed, 0 failed, 93 filtered out.
- Remaining boundary: live offline playback and package proof remain.
