# WL-ARCH-010 distributed migration recovery ledger — 2026-08-08

`compute_migrate` now opens one root-only, bounded, atomic JSON authority before
it drains any migration topic. The ledger owns all four Bus cursors, prepared
and authorized source/target jobs, authenticated acknowledgement jobs, retained
source definitions, wall-clock commit deadlines, and explicit publish,
relinquish, and rollback retry phases. A cursor cannot advance past a relevant
action without the corresponding job being persisted first.

Source disk shipment is checkpointed before `migrate-ready` publication; target
define/start is checkpointed before its committed/failed receipt; and terminal
source cleanup is removed only after an idempotent Workload adapter call
succeeds. Publish failures and failed relinquish/rollback effects stay durable
with bounded retry pacing. Restart recovery re-admits a prepared capability,
including the narrow spent-nonce crash seam, without advancing to an external
effect from an unverified body. Failed shutdown, observation, timeout, or rsync
also redefines and restarts the retained source definition instead of leaving a
stopped VM behind.

Ledger admission rejects symlink/non-regular state, duplicate JSON keys,
oversized state, invalid/oversized fields, duplicate jobs, and over-capacity
queues. File replacement is write-fsync/rename/directory-fsync and the worker
fails closed when recovery state is unavailable.

## Verification

- BigBoy `.130`, slot `arch010-distributed-migration-r1`:
  `cargo test --locked -p mackesd --lib compute_migrate::tests -- --nocapture`
  passed 53/53, with 4,336 filtered out.
- The focused suite includes durable cursor/terminal-phase restart recovery,
  symlink rejection, recursive duplicate-key refusal, oversize refusal,
  managed-disk traversal refusal, authenticated replay behavior, atomic source
  admission/commit recovery, adapter-only lifecycle routing, and source recovery
  after a failed stop request.
- Focused remote `rustfmt --edition 2021 --check` and `git diff --check` passed.

## Remaining acceptance gap

This closes the known memory-backed distributed migration state and dropped
terminal-retry gaps. ARCH-010 remains `Remaining`: live libvirt crash injection,
native KMS/EGL attachment, package/upgrade proof, and Dell/seat-15 lifecycle
evidence are still required.
