# WL-FUNC-011 collaboration projection atomicity — 2026-08-09

## Correction

`CollabEngine` previously advanced its HLC and mutated its retained event,
domain-state, and purge-ack sets before SQLite projection committed. A projection
failure could therefore expose live collaboration state that durable offline
replay had not accepted. The projection is now the commit point for local
commands, worker-authored facts, and replicated merges; all live state advances
only after it succeeds.

The hostile regression removes a required projection table, then proves both a
local space command and a signed peer replay fail without changing the engine's
clock, retained events, or folded spaces.

## Farm proof

- Host: `172.20.0.50`
- Slot: `func011-collab-projection-atomic-r1-20260809`
- Focused regression: 1 passed, 0 failed.
- Complete `mde-collab-core` suite: 98 passed, 0 failed.
- Exact-file `rustfmt --check`: passed.
- `engine.rs` SHA-256: `29a3e483ca739c8c542bbd3bf20ac89e00d538a55b58c5069718c924926a78b4`
- `tests.rs` SHA-256: `22e38ea179cc1c90f5da33f1449441274486da0e14c6adfbb2bdacfda1228f19`

This is a bounded S3 offline/durability correction, not closure of WL-FUNC-011
or a live multi-seat release claim.
