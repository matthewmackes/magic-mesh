# WL-CRIT-007 node-availability device-class binding — 2026-08-11

- Scope: durable node availability binds node ID, device ID, and device class across restart.
- Hostile boundary: same-identity recovery with substituted class thresholds cannot inherit and republish prior availability.
- Focused gate: `cargo test -p mackesd workers::node_availability::tests::restart_cannot_retain_device_class_substitution_as_node_availability -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 2, admitted with 10,324,180 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,857 filtered out.
- Remaining boundary: installed physical suspend/resume and fleet peer-return convergence proof remain.
