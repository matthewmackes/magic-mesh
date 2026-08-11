# WL-FUNC-016 native offer revocation — 2026-08-11

- Scope: invalid rich-offer replacement revokes the prior DRM clipboard generation; only a corrected-forward generation restores selection authority.
- Hostile boundary: stale selections cannot survive a restarted or malicious native provider substitution.
- Focused gate: `cargo test -p mde-egui clipboard::tests::restarted_native_provider_invalid_replacement_cannot_retain_prior_offer_authority -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on BigBoy `172.20.0.130`, slot 1.
- Result: **PASS**, exact hostile regression passed.
- Remaining boundary: prove revocation and recovery against the live DRM clipboard provider.
