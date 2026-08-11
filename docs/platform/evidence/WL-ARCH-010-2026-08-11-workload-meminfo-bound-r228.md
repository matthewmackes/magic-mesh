# WL-ARCH-010 workload capacity probe bound — 2026-08-11

- Scope: workload capacity admission now caps `/proc/meminfo` host text at 64 KiB and fails closed before parsing oversized input.
- Farm: BigBoy `172.20.0.130`; focused agent farm lane passed 1 test.
- Test: `workers::workload_compute::tests::oversized_host_text_input_is_rejected_before_capacity_parse`.
- Result: PASS, 1 passed, 0 failed.
