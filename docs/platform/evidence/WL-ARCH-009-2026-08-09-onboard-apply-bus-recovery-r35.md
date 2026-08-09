# WL-ARCH-009 — onboard-apply Bus startup recovery (r35)

Date: 2026-08-09

## Scope

The `onboard_apply` worker no longer exits permanently when its Bus root is
unresolved or cannot be opened. An explicit root still wins; otherwise root
resolution uses `mde_bus::default_data_dir()` and then the canonical
`mde_bus::SYSTEM_BUS_ROOT`. The same worker retries open and activation with
shutdown-interruptible exponential backoff bounded from 10 ms through 2 s.

The worker consumes one transient privileged command lane,
`action/onboard/apply`. Activation reads that lane's tail before installing its
cursor, so retained role, secret, and session effects never replay. A command
written after successful activation remains forward work and executes once.
Runtime reads stage the next cursor and all parsed commands before applying any
bundle; a read failure preserves the prior cursor and defers the complete apply
and event-history sweep.

`event/onboard/apply` is append-only durable output history, not an input cursor.
It is therefore not tail-skipped or folded as command state. Production now
writes each recovered command's result through the same recovered `Persist`
handle, so an outage-delayed forward command appends its result history when the
command read succeeds. There are no other durable input/state lanes in this
worker.

## Focused farm proof

Host: machine194, `172.20.0.170`

Slot: `onboard-apply-bus-r35`

The initial routed sync encountered an unrelated concurrent compile error in
`voice_provision.rs`. The owned slot was rebuilt from clean `HEAD`, then only
`onboard_apply.rs` was overlaid. No unrelated source was changed or included in
the successful proof.

Primary exact command in the clean machine194 slot:

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::onboard_apply::tests::late_bus_recovers_without_replay_and_defers_failed_reads \
  -- --exact --nocapture
```

Final result: `1 passed; 0 failed; 4,458 filtered out`. The same worker survived
an unresolved root, an open failure, and an activation-tail failure. The retained
startup command caused zero apply effects. A runtime read failure deferred a
post-activation command and its history event; after read recovery, that command's
two typed actions applied once and exactly one correlated history event appeared.
Shutdown completed cleanly.

Two warmed-slot exact checks:

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::onboard_apply::tests::service_bus_root_honors_override_and_falls_back_to_system_spool \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::onboard_apply::tests::worker_drains_an_apply_and_publishes_the_observed_state \
  -- --exact --nocapture
```

Results: each `1 passed; 0 failed; 4,458 filtered out`.

Final farm formatting and scoped diff checks:

```text
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/onboard_apply.rs
git diff --no-index --check <clean-HEAD-onboard_apply.rs> \
  crates/mesh/mackesd/src/workers/onboard_apply.rs
```

Results: passed. The clean baseline was temporary verification input and was
deleted after the check.

Scoped local integrity gate:

```text
git diff --check -- crates/mesh/mackesd/src/workers/onboard_apply.rs \
  docs/platform/evidence/WL-ARCH-009-2026-08-09-onboard-apply-bus-recovery-r35.md
```

Result: passed.

## Artifact identity

```text
1b2edd0604e1446fda0888d4775507ee92771748634a924fdadcc384608040b0  crates/mesh/mackesd/src/workers/onboard_apply.rs
```

No WORKLIST edit or commit was made.
