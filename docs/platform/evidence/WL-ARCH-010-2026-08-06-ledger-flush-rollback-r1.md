# WL-ARCH-010 durable journal flush rollback — 2026-08-06

`WorkloadOperationLedger::advance` now restores the prior in-memory status when
the atomic journal replacement fails. A daemon cannot continue from a phase
that was not persisted; the next reconciliation pass retries from the last
durable state.

Verification:

- BigBoy `.130`, slot `arch010-ledger-flush-rollback-20260806-r1`:
  `cargo test -p mackesd workload_reconciler::tests -- --nocapture` passed
  **9/9**, including the forced rename-failure rollback regression.
- `git diff --check` passed.
- Source SHA-256:
  `355ed88ad331c8fa7804f7cfec125aa320c976a979f1a577bee39e0fbc645994`.

This hardens the journal boundary but does not prove live crash recovery,
real adapter idempotence, native attachment, packaging, or Dell/seat proof.
Dell runtime was not modified.
