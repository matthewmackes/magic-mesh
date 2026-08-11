# WL-ARCH-010 storage probe timeout — 2026-08-11

- Scope: Workload storage-capacity probing now uses the shared bounded subprocess path and fails closed when `df` hangs.
- Farm: BigBoy `172.20.0.130`, focused regression `storage_probe_fails_closed_when_df_hangs`.
- Result: PASS, 1 passed, 0 failed.
