# WL-CRIT-006 / WL-ARCH-009 worker executable generation — 2026-08-11

- Scope: restarted worker groups bind admission to the installed executable inode.
- Hostile boundary: replacement of the admitted executable cannot repopulate a worker group after restart.
- Focused gate: `cargo test -p mackesd --bin mackesd spawn::process_group_thread_admission_tests::replaced_installed_executable_cannot_repopulate_worker_group_after_restart -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 1.
- Result: **PASS**, 1 passed, 0 failed.
- Remaining boundary: signed release and live six-group restart proof.
