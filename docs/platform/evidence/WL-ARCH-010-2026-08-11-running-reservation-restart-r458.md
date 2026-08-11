# WL-ARCH-010 running-reservation restart authority — 2026-08-11

- Scope: Workloads capacity accounting retains the last running reservation across restart.
- Hostile boundary: a same-generation stopped substitution cannot release capacity owned by the retained running workload.
- Focused gate: `cargo test -p mackesd workers::workload_compute::tests::same_generation_stopped_substitution_cannot_release_running_reservation_after_restart -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 2.
- Result: **PASS**, 1 passed, 0 failed.
- Related exact passes: `WL-ARCH-010-2026-08-11-display1-cleanup-generation-r459.md` and `WL-ARCH-010-2026-08-11-workload-journal-hardlink-r462.md`.
- Remaining boundary: live multi-node placement and corrected-forward recovery proof.
