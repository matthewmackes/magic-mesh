# WL-CRIT-007 fleet reconcile corrected-forward retry — 2026-08-11

- Scope: failed fleet reconciliation must remain immediately due after its source state is corrected.
- Hostile boundary: a failed attempt cannot advance the 15-minute success cadence and suppress the next poll's repair.
- Focused gate: `cargo test -p mackesd workers::fleet_reconcile::tests::failed_reconcile_remains_due_for_corrected_forward_retry -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 14,800,796 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,863 filtered out.
- Remaining boundary: installed-peer restart/return after an initial failure and real replicated convergence proof remain.
