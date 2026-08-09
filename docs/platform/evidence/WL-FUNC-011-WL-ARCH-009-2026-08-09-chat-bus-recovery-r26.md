# WL-FUNC-011 / WL-ARCH-009 — Chat Bus startup recovery (r26)

Date: 2026-08-09

Base commit: `c6291c33ed2e6d0c200f9223c34104f31c3fd955`

Production source: `crates/mesh/mackesd/src/workers/chat.rs`

Source SHA-256:
`a75824873b7aee02d6737145d66d28632e98ec1aac089bce09932b65d333590c`

## Correction

`ChatWorker` no longer returns permanent success when its Bus is unresolved or
unopenable. Explicit roots and normal account data roots remain authoritative,
with `mde_bus::SYSTEM_BUS_ROOT` as the canonical service-context fallback.
Startup retries at the poll cadence clamped to 10 ms–2 s, and shutdown interrupts
the retry wait.

Activation is one transaction assembled outside live worker state. It must
successfully list topics, read the current tail of all six mutable
`action/chat/*` lanes, and read retained durable lanes before any cursor set is
installed. An open, topic-list, tail, or retained-read failure therefore cannot
activate a partial cursor set. Runtime lane-read failures also retain the prior
cursor instead of presenting an empty successful read.

The lane audit keeps mutable sends, room lifecycle, presence, mute preferences,
notification preferences, and inline alert actions fail-closed and
forward-only: retained commands are tail-primed and never replay effects. Signed
`event/chat/message` envelopes and deterministic folded alert history instead
replay from retained Bus storage. Syncthing conversation logs still bootstrap
the durable union. Retained history rebuilds read models without replaying its
transient toast, while newly discovered durable lanes drain from their beginning.

The same worker survives an absent root, an open failure, and a failed atomic
activation, then processes one fresh authorized send exactly once without a
restart. No fake state, provider, or alternate collaboration authority was
added.

## Focused farm proof

Host: machine 9 (`172.20.0.50`)

Slot: `chat-bus-recovery-r26`

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=chat-bus-recovery-r26 \
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib \
workers::chat::tests::default_bus_root_resolution_honors_mde_bus_root \
-- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,443 filtered out`.

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=chat-bus-recovery-r26 \
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib \
workers::chat::tests::activation_replays_durable_history_and_primes_every_transient_lane \
-- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,443 filtered out`.

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=chat-bus-recovery-r26 \
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib \
workers::chat::tests::late_bus_and_failed_activation_recover_without_replay_or_restart \
-- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,443 filtered out`.

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=chat-bus-recovery-r26 \
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib \
workers::chat::tests::shutdown_interrupts_the_unavailable_bus_retry_wait \
-- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,443 filtered out`.

```text
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/chat.rs
```

Result: passed on machine 9 after syncing the exact final source. The scoped
`git diff --check -- crates/mesh/mackesd/src/workers/chat.rs` passed in the
authoritative local checkout; farm rsync slots intentionally contain no `.git`
metadata. No broad test, package build, installed-seat proof, or unrelated gate
was run.
