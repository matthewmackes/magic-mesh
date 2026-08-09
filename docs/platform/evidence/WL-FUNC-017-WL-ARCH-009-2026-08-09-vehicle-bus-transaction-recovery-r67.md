# WL-FUNC-017 / WL-ARCH-009 — vehicle Bus transaction recovery r67

Date: 2026-08-09

## Transaction semantics

- Production no longer captures `default_bus_root()` at construction. Every read, action, reply, and state publication resolves the current mde-bus root and then the canonical system spool, while an explicit root remains exact. `with_bus_root(None)` remains an intentional test/offline disable whose transactions succeed without Bus effects.
- Bus transactions open a fresh `Persist`, bind to the current `index.sqlite` device/inode, and verify that same identity after reads or writes. The same worker therefore retries a late/unopenable Bus and observes same-path index replacement.
- `action/vehicle/*` is a transient mutation/request family. On each newly observed index, all existing action topics and tails are staged before activation; retained commands are skipped. Topics and messages appearing after activation remain forward work. A runtime sweep completes every action-lane read before the first effect.
- `reboot` now has a bounded host-local transaction/result journal beside the daemon DB. A claim is atomically persisted and fsynced before authorization reaches the gateway effect; the exact serialized typed reply is atomically persisted as `completed` before Bus publication; `delivered` is persisted only after the reply write and index-stability boundary, and cleanup follows that boundary. A completed result survives worker restart and is corrected-forward into a replacement Bus without repeating reboot or audit. A recovered claim without a completed result is never executed again: it becomes an honest typed `ok=false` indeterminate result, is durably completed, and is then delivered.
- Journal admission is fail-closed: the immediate parent must be a non-symlink directory with no group/world write access; the journal must be a bounded regular file owned by the same account as that trusted parent, exact mode `0600`, and opened with `O_NOFOLLOW`. Atomic replacements use a same-directory `create_new` mode-`0600` temporary file plus file/directory fsync. The wire file is capped at 256 KiB, 32 transactions, 64 KiB per serialized reply, and bounded request/topic identities; schema, host authority, duplicate transaction IDs, phases, and typed replies are validated before use. A hostile journal prevents the privileged effect.
- Non-privileged actions retain the same-worker pending reply behavior. Cursor acknowledgement for every action still happens only after reply publication and index verification. For journaled reboot, same-worker publication failure retains the exact result both in memory and durably; recovery checks for the exact reply row before corrected-forward publication so a crash between Bus write and journal delivery marking does not duplicate the result on the same index.
- Remote `state/vehicle/<manager>/<MG90>` lanes are durable state. On replacement, their cursors reset and queued history folds into a cloned roster. Every configured manager lane must read successfully before any cursor, accepted snapshot, manager selection, or replacement-state change commits. Malformed/identity-invalid records remain explicit rejections and cannot impersonate a manager.
- Local current/enrichment results, rendered legacy state, roster acceptance/publication clocks, sequence, failure cadence, and enrichment scheduling are staged. Required v2/legacy writes must all succeed before those values commit. A failed publication retains the exact pending state and retries it before another poll result can overwrite it. Heartbeat clocks likewise advance only after required publication succeeds.
- Existing MG90 source bounds, manager enrollment/identity validation, typed reboot arming, authorization, and audit truth are unchanged.

## Focused BigBoy verification

Host: BigBoy `172.20.0.130`

Slot: `vehicle-bus-r67`

The requested farm helper performed the correction sync and initial exact compile/test invocation:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=vehicle-bus-r67 \
install-helpers/xcp-build.sh cargo test -p mackesd \
  --features async-services \
  workers::vehicle::tests::completed_reboot_journal_survives_worker_restart_without_repeating_effect_or_audit \
  -- --exact --nocapture
```

The first correction compile was blocked only by unrelated concurrent `transfers/mod.rs` diagnostics. Local concurrent files were preserved; committed `HEAD` forms of `transfers/mod.rs` and `node_grade.rs` were overlaid only in the disposable r67 farm workspace for scoped vehicle verification.

Final exact commands in the warmed slot used this shape for each named test:

```text
cargo test -p mackesd --features async-services \
  workers::vehicle::tests::<name> \
  -- --exact --nocapture
```

Results:

- `completed_reboot_journal_survives_worker_restart_without_repeating_effect_or_audit`: PASS — 1 passed, 0 failed, 4570 filtered out. The final version replaces `index.sqlite` between the injected reply failure and worker restart, then proves one reboot, one audit, exact successful reply on the replacement Bus, and journal cleanup.
- `claimed_reboot_journal_recovers_indeterminate_without_effect_or_audit`: PASS — 1 passed, 0 failed, 4568 filtered out. Recovery emits one typed indeterminate failure with zero ESN probes, reboots, or audit DB creation.
- `hostile_privileged_journal_is_rejected_before_reboot`: PASS — 1 passed, 0 failed, 4568 filtered out. Symlink, mode `0644`, and over-256-KiB journals are each rejected with zero privileged effects.
- `reboot_reply_failure_retries_result_without_repeating_effect_or_audit`: PASS — 1 passed, 0 failed, 4570 filtered out. Same-worker result retry remains one reboot and one committed audit.

Final formatting and scoped checks:

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/vehicle.rs
Result: PASS — no formatting diff.

git diff --check -- crates/mesh/mackesd/src/workers/vehicle.rs
Result: PASS — no whitespace errors.

sha256sum crates/mesh/mackesd/src/workers/vehicle.rs
ac6b328289276e3df8da43015b04742147a256e7125bb3129eb9c1e8fcbe23af
```

The final local and farm source hashes matched exactly.

## Residual caveats

- The journal and Bus are separate durability domains. The ordering is deliberately safety-first: a crash after durable claim but before durable completion reports an indeterminate failure and never repeats reboot, because software cannot prove whether the gateway accepted the effect. An operator must inspect gateway and audit state before authorizing a new request.
- v2 and legacy mirrors are separate SQLite rows, not one cross-topic atomic write. If a later row fails, internal state, sequence, and publication clocks remain uncommitted and the complete projection retries; an earlier row may therefore be duplicated during corrected-forward recovery.
- An unavailable Bus can leave an older retained projection externally visible until recovery. It never masquerades as empty state or advances the worker's accepted state/cursors.
