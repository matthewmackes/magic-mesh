# WL-FUNC-016 / WL-FUNC-019 / WL-ARCH-009 transfer Bus transaction recovery r69

Date: 2026-08-09

## Scope and result

Owned production path: `crates/mesh/mackesd/src/workers/transfers/mod.rs`.

The transfer worker now resolves the Bus for every registry, collaboration-command, projection-confirmation, and notification transaction. Production resolution uses the current configured/user Bus and then the concrete `SYSTEM_BUS_ROOT`; explicit test roots remain explicit, and `with_bus_root(None)` remains an intentional disable. Every open refreshes a replaced index before the transaction. A late, unopenable, or replaced Bus therefore defers publication without terminating the worker, while its existing shutdown-select bounds retry cadence.

Every accepted transaction now brackets `Persist::open` plus
`reopen_if_index_changed()` with path identity observations and also requires the
opened handle's inode to equal the accepted post-open identity. Replacement
after SQLite open/reopen but before metadata therefore cannot bind a retired
connection to the new path identity. The same accepted identity is checked
after a complete Files-registry snapshot, after collaboration-command append,
and after each projection-confirmation read. A retired registry view cannot
return filesystem authority; a command stranded on a retired index is not
acknowledged; and a success projection from a retired connection cannot admit a
filesystem commit.

Files registry acquisition is a complete bounded snapshot: topic discovery and every admitted `state/collab/file-references/<space>` read/decode must succeed before an endpoint is returned and before a Files copy can begin. A malformed/unreadable admitted projection cannot masquerade as an absent object or permit destination mutation.

Terminal notifications now use a durable content-addressed outbox plus durable terminal-receipt identities. Completion is persisted before notification; failed Bus publication leaves one pending result and never reruns the transfer lane. Reconciliation folds crash/outage terminal history, deduplicates repeated failed-publication staging by receipt identity, and advances delivery/cleanup only after a successful Bus append. The append transaction captures the concrete root plus `index.sqlite` device/inode before writing and verifies that identity again immediately before receipt commit. Replacement during the write therefore returns a retryable error and retains the outbox instead of acknowledging a row stranded on the retired index. Each semantic terminal result has one directly addressed receipt file whose bounded content is the delivered Bus-generation digest: replacement after validation records only the retired identity, which cannot suppress corrected-forward publication on the current index, and corrected delivery atomically replaces it with the current identity. This avoids a whole-directory receipt scan and bounds replacement history to one identity per result. Receipt creation and pending-row removal both fsync their containing directories. A complete bounded outbox read is required before inbox consumption or lane effects. Existing terminal rows use separate activation-baseline markers, atomically excluding crash-surviving pending rows. Activation priming directly inspects each bounded delivery receipt and creates a baseline only for a terminal row with neither pending work nor any generation-bound delivery. Consequently, a same-index restart is suppressed by its matching receipt, while a restart after replacement preserves the old receipt and republishes to the new identity instead of converting the row into an identity-agnostic baseline.

## Semantic lane audit

- The module has no Bus action/request/reply ingress cursor. Submit/control verbs are a durable local filesystem inbox and are claimed into the durable transfer ledgers; inventing a second Bus ingress here would create competing authority.
- `action/collab/command` is an output mutation lane. Its owning collaboration worker performs transient action activation/tail semantics; transfers publishes a newly authorized command and then confirms the durable Files projection.
- `state/collab/file-references/<space>` is durable registry state. It is folded completely, never tail-skipped.
- `state/notify/transfers` is a forward output. Durable outbox/receipt state, rather than a Bus cursor, gates replay and corrected-forward delivery.

## Focused verification

Farm host: machine9 `172.20.0.50`

Farm slot: `MCNF_BUILD_SLOT=transfer-bus-r69`

The helper initially refused sync at 6.59 GiB free. The completed prior `wildfire-overlay-bus-r64` slot was the only farm slot present (13 GiB); removing that exact completed workspace restored 15 GiB free. No source workspace or active agent slot was removed.

Clean-tree sync/build command:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=transfer-bus-r69 install-helpers/xcp-build.sh cargo test -p mackesd workers::transfers::tests::incomplete_files_registry_view_defers_before_filesystem_effects -- --exact
```

Result: PASS, 1 passed, 0 failed, 4561 filtered out. Cold test-profile build completed in 5m29s. An initial compile found three local `PersistError`/`io::Error` conversions; those were corrected before the passing run.

Farm formatting and identity check:

```text
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/transfers/mod.rs
sha256sum crates/mesh/mackesd/src/workers/transfers/mod.rs
```

Result: PASS; farm and local source hashes matched.

The corrected linked farm binary was run with each fully qualified test name and `--exact`:

```text
workers::transfers::tests::late_and_replaced_bus_recovers_identity_bound_forward_notifications
workers::transfers::tests::unreadable_outbox_defers_inbox_and_lane_effects
workers::transfers::tests::incomplete_files_registry_view_defers_before_filesystem_effects
workers::transfers::tests::failed_result_publication_corrects_forward_without_repeating_lane
workers::transfers::tests::failed_transfer_emits_one_notify_alert
workers::transfers::tests::same_tick_terminal_batch_emits_one_coalesced_notify_alert
workers::transfers::tests::worker_exits_promptly_on_shutdown
workers::transfers::tests::v2_collab_files_destination_without_commit_authority_is_safely_gated
workers::transfers::tests::replacement_after_validation_restart_corrects_to_current_index
workers::transfers::tests::notification_replacement_during_open_rejects_retired_connection_identity
workers::transfers::tests::files_registry_replacement_after_complete_read_defers_before_filesystem_effects
workers::transfers::tests::files_command_and_projection_transactions_reject_retired_index
```

Result: PASS for all twelve exact tests, each 1 passed / 0 failed (4,580
filtered out). The reviewer-correction rebuild used the existing machine9 slot
and a clean staged tree containing clean `HEAD` plus only the owned transfer
source. Its first exact command was:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=transfer-bus-r69 \
  ./install-helpers/xcp-build.sh cargo test -p mackesd \
  workers::transfers::tests::notification_replacement_during_open_rejects_retired_connection_identity \
  -- --exact --nocapture
```

Result: PASS, 1 passed / 0 failed / 4,580 filtered out. The package rebuild
finished in 7m08s; the remaining exact tests ran directly through that linked
farm library-test binary. The three new tests replace the index during accepted
open, after complete registry reads, after command append, and after a matching
projection read. Each stale transaction is rejected, the replacement receives
no falsely acknowledged command/notification, and registry replacement occurs
before destination filesystem effects. The failed-publication test first
exposed timestamp-distinct duplicate pending rows during repeated outage
reconciliation; receipt-identity deduplication corrected that production
defect. The restart hostile test still performs replacement after append and
after validation, drops the engine at the receipt boundary, republishes exactly
once to the current replacement on restart, never repeats the transfer lane,
and suppresses a second same-index restart.

Scoped local check:

```text
git diff --check -- crates/mesh/mackesd/src/workers/transfers/mod.rs
```

Result: PASS.

## Hash

```text
5107aea4b483cfe45d115de5bbfd4ea1b7fdbbbd10d346285e103b6ee4d95d0d  crates/mesh/mackesd/src/workers/transfers/mod.rs
```

## Residual non-atomic caveats

- Bus append and the filesystem delivery receipt are separate durable stores. A process/power loss after a verified append but before receipt fsync can duplicate the notification on recovery; it cannot repeat the already-terminal transfer/copy/delete lane effect.
- Legacy in-flight process-crash recovery remains governed by `transfers/queue.rs`, outside this slice's ownership. This change specifically prevents notification/reply failure from rescheduling a terminal effect.
- The local verb, queue, V2 ledger, and sync-pair storage implementations live in sibling owned-out-of-scope modules. This slice does not alter their existing record parsing or crash contracts; it makes every Bus-backed registry/output transaction and the new notification outbox fail closed.

No WORKLIST edit, commit, or push was performed.
