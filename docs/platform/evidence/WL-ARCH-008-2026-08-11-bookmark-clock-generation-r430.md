# WL-ARCH-008 bookmark clock generation — 2026-08-11

- Scope: Browser-VM bookmark mutation resumes above every durable snapshot/tail generation.
- Hostile boundary: a transplanted retired-node clock cannot roll back snapshot-only history after restart.
- Focused gate: `cargo test -p mackesd workers::bookmarks::tests::transplanted_clock_cannot_rollback_snapshot_generation_after_browser_vm_restart -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 2, admitted with 9,572,648 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,880 filtered out.
- Remaining boundary: replace the live Browser-VM clock during restart and prove the first corrected mutation dominates replicated history.
