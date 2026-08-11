# WL-FUNC-021 reconnect backoff overflow — 2026-08-11

- Scope: exponential retry delay saturates at the configured cap without
  arithmetic wraparound.
- Hostile boundary: a large custom base and attempt count cannot collapse a
  nonzero retry delay to zero and create a hot retry loop.
- Focused gate: `cargo test -p mde-musicd reconnect::tests::large_custom_base_cannot_wrap_a_retry_delay_to_zero -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 2, admitted with 8,428,504 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 251 filtered out.
- Remaining boundary: live provider-loss audible continuation remains.
