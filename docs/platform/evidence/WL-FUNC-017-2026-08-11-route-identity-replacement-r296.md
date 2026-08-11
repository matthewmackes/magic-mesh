# WL-FUNC-017 replacement route identity evidence — 2026-08-11

- Scope: provider output replacing the active route must mint a distinct
  `route_id`; replacement geometry cannot inherit the old result identity.
- Hostile boundary: a provider response declaring the current active route ID
  is rejected before publication, so downstream consumers cannot conflate new
  geometry with previously admitted authority.
- Focused gate: `cargo test -p mackesd workers::navigation::tests::replacement_route_cannot_reuse_active_route_identity -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 2.
- Result: **PASS**, 1 passed, 0 failed, 4,835 filtered out.
- Remaining boundary: the provider-revocation exact gate and live route/provider
  acceptance remain open.
