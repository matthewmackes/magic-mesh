# WL-CRIT-007 fleet-reconcile process budget — 2026-08-11

- Scope: fleet reconciliation has a bounded execution budget and corrected-forward retry.
- Hostile boundary: a wedged reconciler cannot pin restart or suppress the next generation.
- Focused gate: `cargo test -p mackesd workers::fleet_reconcile::tests::wedged_reconcile_cannot_block_restart_or_corrected_forward_generation -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 1.
- Result: **PASS**, 1 passed, 0 failed.
- Remaining boundary: live six-node return drill.
