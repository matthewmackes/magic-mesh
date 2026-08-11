# WL-ARCH-009 readiness publication recovery evidence — 2026-08-11

- Scope: failed Boot Readiness Bus publication invalidates every cached healthy
  fabric, service, and peer probe and resets their schedules.
- Hostile boundary: after Bus recovery the worker cannot republish `ready: true`
  from pre-failure cache; fresh observations from all readiness planes are
  required.
- Focused gate: `cargo test -p mackesd --lib workers::boot_readiness::tests::failed_publication_discards_healthy_caches_before_bus_recovery -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 2.
- Result: **PASS**, 1 passed, 0 failed, 4,836 filtered out.
- Remaining boundary: installed grouped-daemon boot/restart and fleet acceptance remain.
