# WL-ARCH-009 — supervisor runtime sampler and aggregate publisher (2026-08-05)

`mackesd` now samples its explicit supervisor lifecycle map into the shared
bounded worker-runtime contract. It maps live, clean-exit, failed-exit, breaker,
restart, and starting observations without PID probes or fabricated health,
retains monotonic generations and bounded transition history, and rejects an
unknown supervisor row as registry drift. Generation overflow fails closed.

The daemon publishes deterministic per-worker rows followed by one aggregate
`state/mackesd/<node>` record as the complete-sample commit marker. It also
atomically replaces `/run/mde/mackesd-status.json` with the same bounded,
credential-free aggregate. The publisher starts after supervised worker
registration, retries Bus opening/publication, and stops with daemon shutdown.

## Verification

- BigBoy `.130`, warmed slot `wl-arch009-runtime-sampler-r2`:
  `cargo test -p mackesd --lib worker_runtime_status -- --nocapture`.
- Result: `12 passed; 0 failed; 4452 filtered out`.
- The command compiled the complete `mackesd` library target with the current
  Transfer V2 and vehicle scheduler integration. File-scoped Rust formatting on
  farm `.170`, slot `wl-arch009-runtime-fmt-r2`, passed.

Two earlier defect-driven runs found and corrected a required state/reason
constructor mismatch and a hostile duplicate fixture whose timestamp masked the
intended duplicate check. The final result above is after those fixes and after
the aggregate node wire-type and generation-overflow changes.

## Remaining acceptance edge

This advances the canonical runtime truth/file/Bus path; it does not implement
the six independent process services, systemd resource policies, Workers UI,
Action Console, legacy shell removal, package cutover, or live fleet chaos proof.
