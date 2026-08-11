# WL-UX-014 node-grade observer generation — 2026-08-11

- Scope: node-grade recovery binds retained health to the local observer and monotonic lifecycle.
- Hostile boundary: a foreign restart row cannot seed local grade generation or lifecycle.
- Focused gate: `cargo test -p mackesd workers::node_grade::tests::foreign_restart_row_cannot_seed_local_health_generation_or_lifecycle -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 1.
- Result: **PASS**, 1 passed, 0 failed.
- Remaining boundary: live grade transition and cinematic capture.
