# WL-FUNC-021 Jellyfin transport redirect authority — 2026-08-11

- Scope: the production Jellyfin HTTP transport disables redirects after applying caller-provided client-builder configuration.
- Hostile boundary: even a caller that enables redirect following cannot send a server-bound request, API query, or authorization header to a provider-selected authority.
- Focused gate: `cargo test -p mde-jellyfin net::tests::provider_redirect_cannot_contact_a_different_authority -- --exact --nocapture`.
- Farm: clean coordinator-only run on `172.20.0.90`, slot 1.
- Result: **PASS**, 1 passed, 0 failed, 100 filtered out.
- Remaining boundary: capture a live Jellyfin provider redirect and prove the operator receives a bounded refusal without credential disclosure.
