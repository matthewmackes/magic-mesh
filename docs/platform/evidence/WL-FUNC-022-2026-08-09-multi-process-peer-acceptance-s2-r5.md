# WL-FUNC-022 S2 multi-process Clock peer acceptance r5 — 2026-08-09

The focused acceptance fixture now runs every Clock node tick in a new OS
process. Each process independently opens the retained `mde-bus` index and its
node-specific SQLite Clock authority; the parent exchanges only persisted Bus
messages and reads durable snapshots. No in-memory transport or GUI scheduler
participates.

The trace proved signed A-to-B delivery, C target loss and retained-command
rejoin, B-only local opt-out while A and C rang, and concurrent B Snooze/C Stop
at actor clock 7. Stop won the exact tie and all three independently reopened
durable authorities converged to C's global Stop.

## Exact verification

- Machine 9 build VM `.50`, slot `func022-peer-r5`.
- Command: `cargo test -p mackesd --lib --features async-services
  persisted_bus_multi_process_peer_rejoin_opt_out_and_global_ack_converge --
  --nocapture`.
- Result: parent 1 passed, 0 failed, 4,371 filtered; 14 child-process ticks each
  passed their single exact helper test. Focused execution took 0.41 seconds
  after compilation.
- Exact-file `rustfmt --edition 2021 --check` and scoped `git diff --check`
  passed.
- Clock source SHA-256:
  `0a2cac50e237c22c670556e5777290ab1d76324fbe8f8672c7bd6ba09c19ed84`.

The first compile attempt stopped before Clock on a concurrent ARCH-009
`worker_role.rs` edit that formatted an unfinalized SHA-256 state. Its owner
corrected that unrelated file; the warmed, unchanged Clock filter then passed.
The persisted boundary exposed no Clock persistence, publication, or replay
failure, so no unsupported production behavior was changed.

## Remaining acceptance gap

This is deterministic multi-process farm proof, not a physical three-seat alarm
or shipped-package result. UI, package, and live-seat acceptance remain open.
