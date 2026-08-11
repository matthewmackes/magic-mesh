# WL-ARCH-010 completed workload reservation — 2026-08-11

- Scope: completed operations retain CPU, memory, and storage reservations while
  their workload remains running, paused, or stopping.
- Hostile boundary: a successful start cannot immediately release capacity and
  admit a new placement that overcommits the node.
- Focused gate: `cargo test -p mackesd workers::workload_compute::tests::completed_running_workload_remains_reserved_against_new_placement -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 1, admitted with 12,212,312 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,850 filtered out.
- Remaining boundary: live placement/capacity and installed-runtime proof remain.
