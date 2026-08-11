# WL-FUNC-021 production Navidrome withdrawal — 2026-08-11

- Scope: the production spawn path constructs the tested two-unit Navidrome
  supervisor; the obsolete parallel supervisor is removed.
- Hostile boundary: store recovery first withdraws Navidrome and a failed repair
  cannot re-enable it; successful repair may restore service.
- Focused gate: `cargo test -p mackesd workers::media_navidrome::tests::failed_store_recovery_leaves_navidrome_withdrawn -- --exact --nocapture`.
- Farm: BigBoy `172.20.0.130`, slot 1.
- Result: **PASS**, 1 passed, 0 failed, 4,847 filtered out.
- Remaining boundary: live store-loss/recovery and installed-service proof remain.
