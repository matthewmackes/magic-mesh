# WL-FUNC-011 / WL-ARCH-009 — Collab Bus startup recovery (r22)

Date: 2026-08-09

Base commit: `981d2ddef7a3f248c83f74628f453e073e0019c0`

Production source: `crates/mesh/mackesd/src/workers/collab.rs`

Source SHA-256:
`7161a1185cc8c8da026053cfef28316ab8b7d8eb00c0fee7207974c29f99e10e`

## Correction

`CollabWorker` no longer returns permanent success when its Bus root cannot be
resolved/opened. It resolves an explicit override first, then the shared mde-bus
default, with documented `mde_bus::SYSTEM_BUS_ROOT` fallback for service context.
Startup retries at the poll cadence clamped to 10 ms–2 s, and shutdown interrupts
every wait.

Activation now atomically primes every fixed `action/collab/<verb>` command lane,
the transient clipboard lane, and every discovered transient alert lane. A Bus
open, topic-list, or tail-read failure cannot activate a partial cursor set. The
same worker retries and later activates without a daemon restart. Retained
transient commands remain forward-only and are not replayed; a fresh authorized
command after recovery is projected exactly once. Runtime command reads use a
temporary cursor map and commit cursor movement only after every fixed lane read
succeeds, so a read failure neither advances a partial command sweep nor applies
partial command effects.

Durable signed `collab/event/*` lanes remain intentionally outside startup cursor
priming and continue to drain retained Bus history. Replicated actor-log backfill
also remains active and bounded. No provider, synthetic state, or alternate
collaboration authority was added.

## Focused farm proof

Host: machine 193 (`172.20.0.90`)

Slot: `collab-bus-recovery-r22`

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=collab-bus-recovery-r22 \
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib \
workers::collab::tests::late_bus_and_cursor_prime_recover_without_replay_or_restart \
-- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,431 filtered out`.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=collab-bus-recovery-r22 \
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib \
workers::collab::tests::service_bus_root_falls_back_to_the_shared_system_spool \
-- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,431 filtered out`.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=collab-bus-recovery-r22 \
install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib \
workers::collab::tests::backfill_logs_streams_retained_actor_log_in_chunks \
-- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,431 filtered out`.

```text
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/collab.rs
```

Result: passed on machine 193 after syncing the exact final source. The scoped
`git diff --check -- crates/mesh/mackesd/src/workers/collab.rs` passed in the
authoritative local checkout; farm rsync slots intentionally contain no `.git`
metadata. No broad test, package build, installed-seat proof, or unrelated gate
was run.
