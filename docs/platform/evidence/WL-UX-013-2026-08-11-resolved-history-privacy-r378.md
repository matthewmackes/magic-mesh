# WL-UX-013 resolved-history privacy admission — 2026-08-11

- Scope: every fresh node-health publication enforces the six-hour resolved-history privacy epoch.
- Hostile boundary: a current outer publication cannot restore an embedded resolved incident older than the retention epoch.
- Focused gate: `cargo test -p mackes-mesh-types health::tests::node_health_publication_rejects_resolved_history_outside_privacy_epoch -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 1, admitted with 8,563,164 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 520 filtered out.
- Remaining boundary: installed multi-node health-history recovery proof remains.
