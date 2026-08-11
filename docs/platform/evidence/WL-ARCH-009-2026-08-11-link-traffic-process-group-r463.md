# WL-ARCH-009 link-traffic process-group authority — 2026-08-11

- Scope: link-traffic provider execution is bounded to the worker-owned process group.
- Hostile boundary: a daemonized nft descendant cannot outlive or pin worker authority.
- Focused gate: `cargo test -p mackesd workers::link_traffic::tests::hostile_nft_descendant_cannot_outlive_link_traffic_worker_authority -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 2.
- Result: **PASS**, 1 passed, 0 failed.
- Related exact passes: `WL-ARCH-009-2026-08-11-datacenter-process-group-r466.md` and `WL-CRIT-006-WL-ARCH-009-2026-08-11-worker-executable-generation-r467.md`.
- Remaining boundary: live provider timeout/recovery proof.
