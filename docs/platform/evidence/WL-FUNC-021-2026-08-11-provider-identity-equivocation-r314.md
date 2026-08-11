# WL-FUNC-021 provider identity equivocation — 2026-08-11

- Scope: canonical media-provider endpoints admit only one consistent provider
  identity while exact duplicate declarations collapse harmlessly.
- Hostile boundary: conflicting identities for one endpoint publish an empty
  roster, revoking retained authority without fabricating legacy-local fallback;
  unrelated valid providers remain available.
- Focused gate: `cargo test -p mackesd --features async-services --lib workers::media_registry::tests::equivocated_provider_identity_is_revoked_without_legacy_fallback -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 2, admitted with 12.1 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,848 filtered out.
- Remaining boundary: live provider discovery/playback and package/seat proof remain.
