# WL-CRIT-007 availability device-class chain — 2026-08-11

- Scope: corrected-forward availability reconciliation remains bound to the node's configured device class.
- Hostile boundary: an older same-node/device Bus row with a substituted class cannot join the durable generation chain.
- Focused gate: `cargo test -p mackesd workers::node_availability::tests::older_bus_device_class_substitution_cannot_join_corrected_forward_chain -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 25,612,508 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,876 filtered out.
- Remaining boundary: physical sleep/resume against an older wrong-class Bus row, followed by valid corrected-forward recovery.
