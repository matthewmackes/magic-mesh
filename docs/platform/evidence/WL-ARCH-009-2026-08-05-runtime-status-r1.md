# WL-ARCH-009 — worker runtime status projection (2026-08-05)

The daemon now has a pure runtime-status seam over the admitted WorkerContract
and explicit WorkerRuntimeSnapshot. It validates state, generation, freshness,
relations, timeline, and change-set bounds; binds change sets to the current
node/worker generation; emits deterministic worker/node topic names; and rejects
hostile JSON or contract/snapshot mismatches. It does not sample processes,
infer state, or publish Bus data by itself.

## Verification

- BigBoy `.130`, slot `wl-arch009-runtime-status-r3`:
  `cargo test -p mackesd workers::worker_runtime_status -- --nocapture`.
- Result: `4 passed; 0 failed; 4413 filtered out`.
- Farm Rust formatting checks passed.
- Process entrypoint wiring, live publisher invocation, and fleet proof remain
  open for WL-ARCH-009.
