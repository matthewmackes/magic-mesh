# WL-FUNC-021 media manifest item identity — 2026-08-11

- Scope: complete media manifests derive each item identity from its canonical
  declaration and admit that identity only once.
- Hostile boundary: forged deterministic IDs and duplicate item declarations
  cannot enter the complete retained fold.
- Focused gate: `cargo test -p mackesd --features async-services --lib workers::media_server::tests::forged_or_duplicate_item_identity_cannot_enter_the_complete_fold -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1, admitted with 11,386,596 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,847 filtered out.
- Remaining boundary: live provider discovery/playback and package proof remain.
