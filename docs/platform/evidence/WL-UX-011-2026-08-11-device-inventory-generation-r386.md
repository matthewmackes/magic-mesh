# WL-UX-011 device-inventory generation arbitration — 2026-08-11

- Scope: hardware inventory generations bind to probe start time and publish under a no-follow per-host lock.
- Hostile boundary: a delayed pre-restart or conflicting same-generation snapshot cannot replace newer hardware truth.
- Focused gate: `cargo test -p mackesd workers::device_inventory::tests::delayed_pre_restart_inventory_cannot_replace_a_newer_hardware_generation -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 2, admitted with 10,782,820 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,857 filtered out.
- Remaining boundary: live concurrent restart/probe against real sysfs and replicated state remains.
