# WL-ARCH-010 / WL-ARCH-009 — Scheduler Bus transaction recovery r75

Date: 2026-08-09

Scope was limited to `crates/mesh/mackesd/src/workers/scheduler.rs` and this evidence record. `WORKLIST.md` was not edited, and no commit or push was made.

## Semantic result

- Production resolves the explicit/current Bus root on every pass with `SYSTEM_BUS_ROOT` fallback, fresh-opens it, and accepts a connection only when device/inode identity is stable across identity-before/open/identity-after. Late and unopenable storage retries in the same shutdown-aware worker; same-path replacement starts a new activation.
- Activation stages the action tail and all pending-outbox/history reads before publication. Only a complete activation commits the Bus identity and tail, so retained transient placement requests are skipped on startup and replacement while the first forward request is admitted.
- Runtime reads use bounded 64-row pages and complete action, capacity, desired-state, and pending-output history reads before publication. Any read or final identity failure leaves publications and cursor unchanged.
- A bounded mode-0700 host-local outbox durably stages every desired-state row, audit proposal, and correlated reply before publication. Required writes are error-visible. Recovery scans for each exact staged body before retry, so a failed reply or partial proposal publication completes corrected-forward after worker restart without duplicating an already-visible output. Cursor advancement occurs only after all required outputs and outbox cleanup succeed.
- Malformed, non-leader-gated, and no-candidate actions receive retryable correlated replies rather than being silently dropped. Failover proposals use the same durable output path. The scheduler still publishes only `event/schedule/*` and `reply/*`; it never emits a privileged lifecycle action.
- The public two-argument `Publisher` method remains source-compatible for onboarding siblings. Scheduler production alone uses the added transaction-aware fallible method bound to its already-open Bus generation.

## Focused farm proof

Farm host: machine193, `172.20.0.90`

Slot: `scheduler-bus-r75`

Initial final-source compile/open-race gate:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=scheduler-bus-r75 \
  install-helpers/xcp-build.sh cargo test -p mackesd \
  workers::scheduler::tests::replacement_during_open_is_rejected_before_current_reopen \
  -- --exact --nocapture
```

Result: exit 0; `1 passed; 0 failed`; the final-source focused body completed in 0.01s. Compiling the `mackesd` library also proved the preserved `Publisher` API remains compatible with `service_onboard.rs`, `spawn_lighthouse_onboard.rs`, and `onboard_apply.rs` without editing them.

Final exact hostile wave in the helper-synced warm slot:

```text
ssh -i /root/.ssh/mackes_mesh_ed25519 mm@172.20.0.90 \
  'set -euo pipefail; cd ~/magic-mesh-farm-scheduler-bus-r75; \
   for test_name in \
     worker_recovers_late_and_replaced_bus_and_skips_retained_actions \
     complete_reads_and_durable_reply_recovery_do_not_repeat_outputs \
     failover_publication_recovers_without_duplicate_proposals \
     malformed_and_gated_replies_retry_without_cursor_loss \
     replacement_during_open_is_rejected_before_current_reopen; do \
       cargo test -p mackesd workers::scheduler::tests::$test_name \
         -- --exact --nocapture; \
   done'
```

Result: exit 0. Every exact test reported `1 passed; 0 failed` with 4593 library tests filtered out. Focused durations were 0.24s, 0.01s, 0.06s, 0.02s, and 0.02s respectively. The tests prove same-worker late/replaced recovery and shutdown, retained-skip/first-forward semantics, final-lane read failure with zero output, reply-failure restart recovery with one desired/audit/reply row, partial failover recovery without duplicate proposals, malformed/gated corrected-forward replies, and rejection of a replacement occurring after SQLite open but before the post-open identity sample.

Formatting and scoped diff checks:

```text
ssh -i /root/.ssh/mackes_mesh_ed25519 mm@172.20.0.90 \
  'cd ~/magic-mesh-farm-scheduler-bus-r75 && \
   rustup run 1.94.0 rustfmt --edition 2021 \
     crates/mesh/mackesd/src/workers/scheduler.rs && \
   rustup run 1.94.0 rustfmt --edition 2021 --check \
     crates/mesh/mackesd/src/workers/scheduler.rs && \
   sha256sum crates/mesh/mackesd/src/workers/scheduler.rs'
git diff --check -- crates/mesh/mackesd/src/workers/scheduler.rs \
  docs/platform/evidence/WL-ARCH-010-WL-ARCH-009-2026-08-09-scheduler-bus-transaction-recovery-r75.md
```

Result: exit 0. Farm and workspace source hashes matched.

## Hash

```text
2bb02ab509ee88c5f5586cab76d8ab9e6990b78c63170e33a76089f7b68993f0  crates/mesh/mackesd/src/workers/scheduler.rs
```

The evidence-file hash is reported in the final handoff because inserting its own hash would change it.

## Residual crash boundary

The outbox is durable host-local state, not a kernel-atomic transaction with SQLite or an external index installer. Loss of the scheduler state root loses the recovery barrier. Exact-body history checks recover a crash after a Bus output but before outbox cleanup, and startup tail priming prevents an action from replaying after cleanup but before an in-memory cursor assignment. Device/inode checks close the reviewed open-generation race and detect replacement around writes/cleanup, but remain point-in-time checks rather than a rename lock shared with external installers; a replacement immediately after the final verification can retire a just-delivered output. The scheduler emits proposals only, so there is no libvirt, container, or other privileged lifecycle effect inside this residual window.
