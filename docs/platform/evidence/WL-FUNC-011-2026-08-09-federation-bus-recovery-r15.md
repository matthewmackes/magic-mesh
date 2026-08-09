# WL-FUNC-011 / WL-ARCH-009 — federation Bus startup recovery (r15)

Date: 2026-08-09

Base commit: `c5f4f232d5973c9244ea09d46ef4d3ed13bf0d47`

Production source: `crates/mesh/mackesd/src/workers/federation_enforcer.rs`

Source SHA-256: `8fd81a9bf3ae07af3d4af352384a28f8033c9375b26ff3fd1b936dfd7272f769`

## Correction

`FederationEnforcerWorker` no longer exits successfully when Bus resolution or
`Persist::open` is temporarily unavailable. Startup retries at the configured
poll cadence clamped to 10 ms–2 s, and every retry wait is interrupted by
shutdown. Explicit roots remain authoritative; otherwise the normal mde-bus
data-root resolver is honored, with `mde_bus::SYSTEM_BUS_ROOT` as the daemon
fallback when no user root resolves.

No trust directory or enforcement tick is initialized before Bus open. After
the first successful open, trust resolution occurs once and the unchanged tick
order remains action drain, grant reload, ingress enforcement, then status
publication. Action cursors intentionally retain their historical `None`
startup semantics: a valid accept/revoke/refuse intent queued during the Bus
startup race is processed instead of dropped. The existing durable
`ActionAuthorizer` ledger and token lifetime remain the replay/staleness guard.

## Focused farm proof

Host: machine 194 (`172.20.0.170`)

Slot: `federation-bus-recovery-r15`

```text
cargo test -p mackesd --features async-services --lib \
  workers::federation_enforcer::tests::federation_bus_root_preserves_override_and_has_system_fallback \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,419 filtered out`. Explicit-root preservation
and the system-daemon fallback are exact assertions.

```text
cargo test -p mackesd --features async-services --lib \
  workers::federation_enforcer::tests::bus_absence_wait_is_alive_and_shutdown_prompt \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,419 filtered out`. The worker remains alive
without a resolved Bus and shutdown interrupts the bounded retry wait.

After critical review removed an incorrect action-tail priming attempt, only
the affected recovery test was rerun:

```text
cargo test -p mackesd --features async-services --lib \
  workers::federation_enforcer::tests::bus_open_retry_recovers_and_processes_queued_action_once \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,419 filtered out`. One worker survives an
unresolved-root result and an injected open failure, then opens once, processes
a valid refuse-mint action that was already queued, emits exactly one success
audit, remains active, and does not re-enter Bus opening.

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/federation_enforcer.rs
```

Result: passed on machine 194. Local scoped `git diff --check` also passed.

The synced dirty tree contained unrelated in-progress `action.rs` work that had
renamed its root helper while leaving one old call site. It prevented the crate
test binary from compiling. A disposable farm-slot-only substitution restored
that call to `mde_bus::default_data_dir`; no local unrelated source was edited,
and the shim does not touch federation code or its tests.

## Scope

No broad suite, package build, installed-node mount-race proof, or unrelated
test was run. This checkpoint is limited to startup recovery, shutdown, root
selection, queued-action delivery, and exact-once enforcement boundaries.
