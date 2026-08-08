# WL-ARCH-010 durable Workload journal duplicate-key replay guard — 2026-08-06

## Boundary implemented

`mackesd` now rejects duplicate JSON object keys in the persisted
`workload-operations.json` document before deserializing or replaying it. The
recursive guard is shared with the bounded Workload wire contract, so hostile
keys nested in records, statuses, resources, or sequences fail closed rather
than relying on last-key-wins JSON decoding.

## Verification

- BigBoy `.130`, slot `arch010-ledger-duplicate-replay-20260806-r2`:
  `cargo test -p mackesd workload_reconciler::tests -- --nocapture` passed
  **8/8**; the duplicate persisted-key test and the existing durability,
  transition, CAS, capacity, and bounded-history tests all passed.
- The first `.50` attempt reached final linking but failed with `ENOSPC`; no
  passing result is claimed for that host.
- `git diff --check` passed.
- Source SHA-256:
  `bf88a6ca3a182eaa2a2f4f81447c495edc57be4ed05894f45ab4c936890eed23`
  (`mackes-mesh-types/src/workloads.rs`),
  `189d6b0ea79be09a4b50cf51c60943b5bf19db7ac0f9bae743c2242b63ab6798`
  (`mackesd/src/workload_reconciler.rs`).

## Remaining gap

This proves the journal replay boundary, not the complete live reconciler
actuator path. HostCapacity admission, real libvirt/Quadlet adapters, native
Display1/KMS attachment, restart on a live node, packaging, and Dell/seat
acceptance remain open. Dell runtime was not modified.
