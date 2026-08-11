# WL-FUNC-021 Jellyfin redirect authority — 2026-08-11

- Scope: production Jellyfin transport does not follow provider-controlled HTTP
  redirects to another authority.
- Hostile boundary: a redirect target is never contacted and the original `302`
  response fails through unchanged.
- Focused gate: `cargo test -p mde-jellyfin net::tests::provider_redirect_cannot_contact_a_different_authority -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 2, admitted with 11,835,980 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 98 filtered out.
- Remaining boundary: live provider redirect/outage and package proof remain.
