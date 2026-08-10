# WL-ARCH-010 — terminal attachment revocation after restart (r119)

Date: 2026-08-10

## Correction

A persisted in-flight StartAndAttach operation could retain its Display1 lease
when restart recovery reached a permanent observation failure. Terminal failure
now revokes the exact attachment capability and clears it before the failed
state is journaled, preventing durable or projected stale session endpoints.

## Focused farm proof

Machine 9 (`172.20.0.50`) passed the exact library regression:

```text
cargo test -p mackesd --lib \
  workers::workload_compute::tests::permanent_observation_failure_after_restart_revokes_persisted_attachment \
  -- --exact --nocapture

test result: ok. 1 passed; 0 failed; 4666 filtered out
```

The regression reconstructs the durable restart boundary, forces a hostile
permanent observation error, and proves exact lease revocation plus an
attachment-free terminal record. The abandoned broad Cargo invocation was
stopped and is not counted.

## Remaining boundary

Live native attachment, restart recovery, and physical-seat proof remain; this
checkpoint does not close WL-ARCH-010.
