# WL-ARCH-009 service-catalog canonical-file recovery — 2026-08-11

- Scope: service configuration authority loads only canonical
  `<service_kind>.json` files.
- Hostile boundary: crash-left `.kind.<pid>.tmp` staging files are ignored after
  restart and cannot enable a service whose configuration never committed.
- Focused gate: `cargo test -p mackesd --features async-services --lib workers::service_catalog::tests::restart_ignores_uncommitted_service_configuration_staging_inode -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 1.
- Result: **PASS**, 1 passed, 0 failed, 4,844 filtered out.
- Remaining boundary: grouped-process installed recovery and live service acceptance remain.
