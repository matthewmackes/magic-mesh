# WL-FUNC-017 atmosphere source identity — 2026-08-11

- Scope: weather restart cache remains bound to its atmospheric source identity.
- Hostile boundary: retained cache cannot relabel one provider/source as another after restart.
- Focused gate: `cargo test -p mackesd workers::weather_atmosphere::tests::restart_cache_cannot_relabel_atmospheric_source_identity -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 1.
- Result: **PASS**, 1 passed, 0 failed.
- Remaining boundary: live provider outage/corrected-forward capture.
