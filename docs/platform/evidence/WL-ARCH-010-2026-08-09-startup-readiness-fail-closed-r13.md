# WL-ARCH-010 startup readiness fail-closed correction — 2026-08-09

## Outcome

The sole production Workload actuator no longer converts a transient
not-running observation during `WaitingForGuest` into terminal
`Completed/Stopped`. That result falsely reported a successful Start even
though guest readiness had never been established.

Startup absence is now a retryable actuator observation and therefore remains
under the reconciler's durable backoff, retry budget, and operation deadline.
Only an operation already in `Stopping` may use a not-running observation to
complete as stopped. The focused regression exercises both decisions through
the production `SystemWorkloadActuator` seam without invoking a backend.

## BigBoy verification

Host `172.20.0.130`, slot
`arch010-start-readiness-r13-20260809`:

- Focused library regression: 1 passed, 0 failed; 4,356 filtered out.
- Complete `workers::workload_compute::tests` suite: 37 passed, 0 failed;
  4,320 filtered out.
- Exact-file `rustfmt --edition 2021 --check`: passed.
- Scoped local `git diff --check`: passed. The farm sync intentionally omits
  `.git`, so the scoped diff check was run in the authoritative workspace.
- The initial non-library command was not claimed: it hit 25 unrelated
  `workers::cloud` export/visibility errors. The requested library target
  compiled and executed successfully.

## Source hash

- `c0cf6252325572725c892ce09ca33d301fa0909bdf7d8f7896d9218a6f7ff744`
  — `crates/mesh/mackesd/src/workers/workload_compute.rs`

## Remaining boundary

This closes one startup-readiness false-success path. It does not constitute
live libvirt crash injection, native Display1/KMS proof, package lifecycle
proof, or closure of WL-ARCH-010.
