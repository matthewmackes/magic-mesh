# WL-UX-013 device-inventory provenance — 2026-08-11

- Scope: A-F node grades must consume only current device inventory issued for the graded node.
- Hostile boundary: a future-dated or foreign-host inventory cannot contribute healthy device capability and instead creates an evidence warning that prevents an unsupported A grade.
- Focused gate: `cargo test -p mackesd workers::node_grade::tests::future_device_inventory_cannot_publish_an_a_grade -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1, admitted with 12,735,724 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,864 filtered out.
- Remaining boundary: physical-node inventory publication and live grade-transition proof remain.
