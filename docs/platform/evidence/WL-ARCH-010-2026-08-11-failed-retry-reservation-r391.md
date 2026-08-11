# WL-ARCH-010 failed-retry reservation authority — 2026-08-11

- Scope: reservation accounting retains the last running workload generation when a later retry fails without effects.
- Hostile boundary: a failed retry cannot hide the prior running generation and release its CPU, memory, or storage capacity into overcommit.
- Focused gate: `cargo test -p mackesd workers::workload_compute::tests::failed_retry_cannot_release_prior_running_workload_reservation -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 15,789,736 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,862 filtered out.
- Remaining boundary: installed managed-storage admission, native attachment, and physical lifecycle/restart proof remain.
