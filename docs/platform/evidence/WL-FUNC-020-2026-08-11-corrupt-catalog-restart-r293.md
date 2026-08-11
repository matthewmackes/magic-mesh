# WL-FUNC-020 corrupt catalog restart evidence — 2026-08-11

- Scope: corrupt or invalid durable Android catalog state is a retryable
  authority failure, never an empty catalog.
- Hostile boundary: signed Bus history cannot use a corrupt restart cache to
  switch catalog identity. The worker remains fail closed, retries the durable
  cache, and resumes after corrected-forward repair.
- Focused gate: `cargo test -p mackesd --lib --features async-services workers::android_catalog::tests::corrupt_restart_cache_cannot_erase_catalog_identity_authority -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 2.
- Result: **PASS**, 1 passed, 0 failed, 4,832 filtered out.
- Remaining boundary: governed image boot, nested-KVM lifecycle, VDI, and live
  release acceptance remain open.
