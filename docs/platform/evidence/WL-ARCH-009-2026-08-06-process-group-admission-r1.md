# WL-ARCH-009 process-group admission — 2026-08-06

## Delivered

The mackesd worker boundary now has an exact `WorkerGroup` parser for the six
canonical group tokens. `Supervisor` can be pinned to one group and admits
only workers whose canonical registry entry belongs to that group; unknown
worker names fail closed. `mackesd serve --group <group>` exposes the boundary
without accepting arbitrary service-token input. Tiered worker construction
also refuses filtered workers before construction.

## Verification

- BigBoy `.130`, slot `arch009-process-group-check-20260806-r2`: focused
  `cargo test -p mackesd process_group -- --nocapture` passed **3/3**.
- BigBoy `.130`, slot `arch009-process-group-check-20260806-r1`:
  `cargo check -p mackesd --features async-services --bin mackesd` passed.
- The built CLI help on `.130` exposes `serve --group <GROUP>` and documents
  the transitional monolithic fallback when the option is omitted.
- `.50`, slot `arch009-group-admission-20260806-r2`, could not compile the
  focused test because the farm fixture hit `ENOSPC`; this is capacity evidence,
  not a code failure.
- `git diff --check` passed. Source hashes:
  `worker_role.rs` `1cc639b48e0cb37e82ff95b7316354bf5a6c35b7fec279d73c94befa8149e720`;
  `workers/mod.rs` `403a3302d3b6e30b9a9988b11c8396ffaa6d901cd81173e73bd12dddb8a9133c`;
  `mackesd.rs` `6f6f19842e3805459e7bc80ddb07cf6cc31f072399de08cf88a1fe2a5892a469`;
  `spawn.rs` `45ef66a59b7f7a68bb5193c089e217e9b1055615715751fc18f898a7067ff402`.

## Remaining gap

This is a real admission boundary, not the ARCH-009 S4 completion proof. The
six systemd units/target, responder partitioning, single SQLite writer,
cgroup/resource limits, shutdown/retry behavior, and live package/process
isolation remain open. The old monolithic service remains, and Dell runtime was
not changed.
