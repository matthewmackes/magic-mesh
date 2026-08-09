# Service-onboard late-Bus recovery checkpoint (R34)

Date: 2026-08-09

Worklist: `WL-ARCH-009`
Base commit: `020d798ef0f1eddbf453221077df31ce4661db85`

## Runtime semantics

The `service_onboard` worker no longer exits successfully and permanently when
its Bus root is unresolved or temporarily unopenable. An explicit or resolved
user Bus wins, with `mde_bus::SYSTEM_BUS_ROOT` as the canonical daemon fallback.
The same worker retries unavailable roots, open errors, and unsafe activation
with shutdown-aware exponential backoff bounded from 10 ms to 2 s.

`action/onboard/service-add` is the worker's only input Bus topic. It is one
fixed transient one-shot command lane, not a wildcard family and not durable
state. Activation atomically reads and installs that lane's current tail; a
tail-read failure installs no cursor and retries activation. Retained startup
commands therefore do not replay, while every message published after the
activation boundary drains from the installed cursor and executes once.

Steady-state reads now return an unavailable-state error rather than an empty
action vector. The worker obtains the complete `list_since` result before it
moves the cursor, checks leadership, consumes authorization, gathers facts, or
reaches the apply/publish seams. A failed read therefore defers all effects and
the same forward command remains available on recovery.

There is no durable inbound Bus lane to tail-prime or skip. Durable onboarding
facts are the replicated workgroup peer roster, which is gathered only after a
successful forward command read; roster records accrued during a Bus outage are
therefore folded into that command's current facts rather than being replaced
by an empty Bus view. `event/onboard/service-add` remains append-only result
history and is output, not an effect-driving input.

## Focused farm verification

Host: machine9 (`172.20.0.50`)
Slot: `service-onboard-bus-r34`

The following exact affected tests ran in that explicit slot:

```text
cargo test -q -p mackesd --features async-services --lib \
  workers::service_onboard::tests::service_bus_root_falls_back_to_the_canonical_system_spool \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::service_onboard::tests::bus_read_failure_defers_effects_and_retains_the_command_cursor \
  -- --exact --nocapture

cargo test -q -p mackesd --features async-services --lib \
  workers::service_onboard::tests::late_bus_retries_activation_skips_history_and_executes_forward_messages \
  -- --exact --nocapture
```

Each command passed: `1 passed; 0 failed; 4,461 filtered out`. The late-Bus
proof keeps one worker alive through an unresolved-root result, an open error,
and a command-tail read failure; suppresses one retained startup command;
publishes exactly one result for each of two forward commands; and exits
promptly on shutdown. The read-failure proof observes no cursor movement and no
event during failure, then one result for the retained forward command after
the read recovers.

Exact formatting and scoped diff checks passed:

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/service_onboard.rs
git diff --check -- \
  crates/mesh/mackesd/src/workers/service_onboard.rs
```

The initial helper-driven farm compile was blocked before this module by an
unrelated concurrent `voice_provision.rs` change that referenced a missing
helper. The local file was not touched. Only the ephemeral machine9 slot copy of
`voice_provision.rs` was restored to `HEAD`; all three service-onboard commands
then compiled and passed. This is not a service-onboard blocker.

Source SHA-256:
`b38d02796eaeef78cf6194c3be3958c6a274b151cfb4c9b40da6babc937415df`.

## Scope

No broad suite, package build, live seat proof, WORKLIST edit, or unrelated test
was run. This checkpoint is limited to service-onboard Bus-root resolution,
startup retry and activation, transient command replay boundaries, durable-fact
timing, and read-failure deferral.
