# WL-ARCH-010 terminal detach ordering evidence — 2026-08-11

- Scope: terminal Workload failure durably removes attachment authority before
  externally revoking the exact presentation lease.
- Hostile boundary: if journal persistence fails, lease revocation is withheld
  and the prior durable/runtime authority remains aligned for corrected-forward
  recovery. A successful flush then permits exact revocation.
- Focused gate: `cargo test -p mackesd --lib workers::workload_compute::tests::terminal_failure_detaches_durably_before_revocation_and_preserves_on_flush_failure -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 2.
- Result: **PASS**, 1 passed, 0 failed, 4,837 filtered out.
- Remaining boundary: live attachment/presentation cleanup and release acceptance remain.
