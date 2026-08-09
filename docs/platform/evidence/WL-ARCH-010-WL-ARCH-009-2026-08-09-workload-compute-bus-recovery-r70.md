# WL-ARCH-010 / WL-ARCH-009 — Workload compute Bus recovery r70

Date: 2026-08-09

Scope was limited to `crates/mesh/mackesd/src/workers/workload_compute.rs` and this evidence record. `WORKLIST.md` was not edited and no commit or push was made.

## Semantic result

- Production resolves the explicit override or current `mde-bus` data directory on every transaction, with `SYSTEM_BUS_ROOT` as the concrete fallback. `with_bus_root(None)` remains an explicit test/offline disable.
- The worker fresh-opens the Bus each pass and identifies `index.sqlite` by device/inode. An accepted connection is bracketed by identity-before/open/identity-after checks; a generation change anywhere across `Persist::open` rejects the transaction. A genuinely absent Bus is initialized through a discarded connection and then reopened under the same proof. A missing or unopenable Bus no longer terminates or permanently disables the worker, and same-path index replacement causes a new activation without daemon restart. A transient open failure preserves the prior identity/cursor so commands on the returning same index are not tail-skipped.
- Activation stages the action-topic tail and every durable reply-outbox/reply read before committing anything. Only a complete activation tail-primes retained `action/workload/operation` rows. The first row written after activation remains forward work and executes.
- Runtime action pages and any existing reply rows are completely staged before migration draining, reconciliation, reaping, or a new backend effect. Read failure leaves cursor and effects unchanged.
- A bounded, host-local durable reply outbox is written before a local lifecycle request enters the existing durable Workload ledger/actuator path. Completion is persisted before reply delivery. Reply failure therefore leaves the cursor unchanged and restart recovery publishes the reply from the outbox/ledger without repeating the lifecycle effect.
- Every active transaction carries the staged Bus root plus `index.sqlite` device/inode identity. Action reads, state writes, reply writes, outbox cleanup, cursor movement, and activation commit check that the path still names that index. If replacement is detected after a reply write, the completed outbox record remains (or is restored if cleanup raced the replacement), the cursor/activation do not commit, and the next activation delivers the reply to the current index without another actuator effect.
- Action cursors advance only after the required typed reply succeeds. State projection errors are returned, do not update the publication cache, and retry corrected-forward without repeating an already replied action.

## Focused hostile proof

Farm: machine193, `172.20.0.90`

Slot: `workload-compute-bus-r70`

Initial sync and final late/replacement/shutdown regression:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=workload-compute-bus-r70 \
  install-helpers/xcp-build.sh cargo test -p mackesd \
  workers::workload_compute::tests::worker_recovers_late_and_replaced_bus_without_replaying_retained_actions \
  -- --exact --nocapture
```

Result: exit 0; `1 passed; 0 failed`; 4562 library tests filtered out; focused test completed in 0.09s (final helper invocation finished in 2m50s after the source resync). It proved the same worker stayed alive while the Bus path was unopenable, activated a late Bus without replaying its retained action, executed the first forward action once, detected a same-path replacement index, skipped that index's retained action, executed its first forward action once, used the system fallback helper, preserved explicit disable, and exited on shutdown.

Second exact regression in the helper-synced warm slot:

```text
ssh -i /root/.ssh/mackes_mesh_ed25519 mm@172.20.0.90 \
  'set -o pipefail; cd ~/magic-mesh-farm-workload-compute-bus-r70 && \
   cargo test -p mackesd \
   workers::workload_compute::tests::atomic_activation_and_durable_reply_recovery_never_repeat_the_effect \
   -- --exact --nocapture'
```

Result: exit 0; `1 passed; 0 failed`; 4562 library tests filtered out; finished in 3.81s. It proved activation-tail failure commits no cursor/identity, retained action is skipped after the complete retry, runtime page-read failure has zero effects, reply failure leaves one effect plus a durable pending result and no cursor advance, a new worker publishes that result without another effect, and a state-write failure retries publication without another action effect.

Open-race correction initial helper gate:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=workload-compute-bus-r70 \
  install-helpers/xcp-build.sh cargo test -p mackesd \
  workers::workload_compute::tests::replacement_during_open_is_rejected_and_reopens_current_index \
  -- --exact --nocapture
```

Result: exit 0; `1 passed; 0 failed`; 4576 library tests filtered out; focused body finished in 0.01s after a 6m22s cold farm build. The fault seam replaces `index.sqlite` after `Persist::open` has accepted the retired file but before the post-open identity sample. The transaction is rejected, and a clean reopen reads the replacement generation's marker rather than the retired generation's marker.

Final replacement corrections and four-test rerun:

```text
ssh -i /root/.ssh/mackes_mesh_ed25519 mm@172.20.0.90 \
  'set -euo pipefail; cd ~/magic-mesh-farm-workload-compute-bus-r70; \
   for test_name in \
     worker_recovers_late_and_replaced_bus_without_replaying_retained_actions \
     atomic_activation_and_durable_reply_recovery_never_repeat_the_effect \
     replacement_during_reply_keeps_outbox_and_recovers_into_current_index \
     replacement_during_open_is_rejected_and_reopens_current_index; do \
       cargo test -p mackesd workers::workload_compute::tests::$test_name \
         -- --exact --nocapture; \
   done'
```

Result: exit 0. Each exact test reported `1 passed; 0 failed` with 4576 library tests filtered out. Final focused body durations were 0.09s, 0.01s, 0.01s, and 0.01s respectively. The reply-race regression replaces the current index immediately after the stale connection accepts the reply write and proves one actuator effect, no cursor movement, retained completed outbox, and exactly one recovered reply in the current index. The open-race regression proves a connection spanning retired/current generations is rejected and the next open binds the current generation.

Formatting and scoped diff checks:

```text
ssh -i /root/.ssh/mackes_mesh_ed25519 mm@172.20.0.90 \
  'cd ~/magic-mesh-farm-workload-compute-bus-r70 && \
   rustup run 1.94.0 rustfmt --edition 2021 --check \
   crates/mesh/mackesd/src/workers/workload_compute.rs && \
   sha256sum crates/mesh/mackesd/src/workers/workload_compute.rs'
git diff --check -- crates/mesh/mackesd/src/workers/workload_compute.rs
```

Result: both exit 0. The farm and workspace source hashes matched.

## Hashes

```text
81aed97c78869befe6e034e8b2b96ddc6fd753cabf5de771eb3d3d19539880d7  crates/mesh/mackesd/src/workers/workload_compute.rs
```

The evidence-file hash is recorded in the final handoff because hashing this file changes when the value is inserted into itself.

## Residual boundary

The reply outbox is durable host-local state, not a cross-device transaction with the Bus or backend. Loss of the Workload state root loses the barrier. Identity-before/open/identity-after closes the reviewed open-generation binding race, but device/inode verification remains a point-in-time replacement detector rather than a filesystem lock shared with external index installers. The code verifies before cleanup and after a cleanup race (restoring the record on mismatch), but cannot make arbitrary external rename, outbox unlink, and cursor assignment one kernel-atomic operation. The existing ledger deliberately persists the `Defining` boundary before invoking the actuator; a process/power crash inside the backend effect and before its post-effect ledger transition is not made atomic by this slice and continues to rely on the supported libvirt/Quadlet actuator's idempotent recovery. The focused tests use the real Bus and durable ledger/outbox with an injected actuator seam; they do not claim live libvirt or systemd hardware execution.
