# WL-FUNC-017 MG90 retained-radio stale evidence — 2026-08-11

- Scope: failed MG90 radio refresh cannot republish retained Cellular or Wi-Fi
  rows as live authority. The production v2 path converts every retained radio
  row to `Stale` and clears active-path/age claims.
- Hostile boundary: both ordinary and active-path-only Wi-Fi B rows are covered;
  neither can escape stale conversion when the fresh WAN probe is absent.
- Focused gate: `cargo test -p mackesd workers::vehicle::tests::failed_radio_refresh_cannot_republish_retained_link_as_live -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1.
- Result: **PASS**, 1 passed, 0 failed, 4,836 filtered out.
- Remaining boundary: live MG90 failover/connectivity and release proof remain.
