# WL-ARCH-010 Workload journal hard-link authority — 2026-08-11

- Scope: reconciler restart state is admitted from one bounded, owned journal inode.
- Hostile boundary: a hard-linked workload journal cannot be adopted as lifecycle authority after restart.
- Focused gate: `cargo test -p mackesd workload_reconciler::tests::restarted_reconciler_cannot_adopt_hardlinked_workload_journal_authority -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 2.
- Result: **PASS**, 1 passed, 0 failed.
- Remaining boundary: live corrected-forward reconciliation proof.
