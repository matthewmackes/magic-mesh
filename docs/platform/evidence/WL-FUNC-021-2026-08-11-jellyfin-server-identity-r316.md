# WL-FUNC-021 Jellyfin server identity — 2026-08-11

- Scope: durable Jellyfin configuration must contain one declaration per exact
  server identity before selecting credentials or offline-cache authority.
- Hostile boundary: duplicate server IDs fail closed after restart instead of
  selecting an order-dependent provider declaration.
- Focused gate: `cargo test -p mde-jellyfin store::tests::load_rejects_duplicate_server_identity_before_selecting_cache_authority -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1, admitted with 10,628,736 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 91 filtered out.
- Remaining boundary: live Jellyfin outage/playback and package proof remain.
