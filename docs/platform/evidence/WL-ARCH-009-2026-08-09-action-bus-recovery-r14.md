# WL-ARCH-009 action Bus recovery r14 — 2026-08-09

`ActionWorker` now treats Bus open and action-tail cursor priming as one
fail-closed activation boundary. Missing Bus availability, `Persist::open`
failure, and `latest_ulid(ACTION_TOPIC)` failure all retain the worker and retry
the full open/prime sequence at a shutdown-aware cadence clamped to 10 ms–2 s.
Polling starts only with a successfully opened `Persist` and successfully read
tail cursor, so a cursor-read fault cannot silently activate at `None` and
replay retained privileged actions.

Production root selection preserves an explicit override, then the shared
`mde_bus::default_data_dir()` resolver, then the documented
`mde_bus::SYSTEM_BUS_ROOT` fallback. The fixed system fallback avoids inventing
or materializing an arbitrary unconfigured root. The same root policy is used
when a typed service-lifecycle action publishes its Workload operation.

The cursor is retained after the first successful open/prime pair and normal
polling does not reopen or reprime. Existing startup history is skipped, while
one signed action published after activation is dispatched and replied to
exactly once.

## Focused farm verification

BigBoy (`172.20.0.130`), slot `action-bus-recovery-r14`:

```text
cargo test -p mackesd --features async-services --lib \
  workers::action::tests::action_bus_root_preserves_override_and_has_system_fallback \
  -- --exact --nocapture
```

Result: **1 passed, 0 failed, 4,423 filtered out**. Explicit override and the
canonical system fallback are both locked.

```text
cargo test -p mackesd --features async-services --lib \
  workers::action::tests::bus_absence_wait_is_alive_and_shutdown_prompt \
  -- --exact --nocapture
```

Result: **1 passed, 0 failed, 4,423 filtered out**. The worker remains alive on
missing Bus availability and a shutdown interrupts the bounded retry wait even
when its configured poll interval is 30 seconds.

```text
cargo test -p mackesd --features async-services --lib \
  workers::action::tests::bus_recovery_skips_history_and_executes_one_forward_action_once \
  -- --exact --nocapture
```

Result: **1 passed, 0 failed, 4,423 filtered out**. One worker survives a
missing-root result, an open failure, and an open-success/cursor-prime failure.
It reopens and primes successfully on the fourth activation attempt, gives the
retained hostile action zero replies and zero dispatches, then gives one newly
published signed Android action exactly one provider launch and one reply.

Exact remote formatting and local scoped diff checks passed:

```text
rustfmt --edition 2021 --check \
  crates/mesh/mackesd/src/workers/action.rs
git diff --check -- crates/mesh/mackesd/src/workers/action.rs
```

Base commit: `c5f4f232d5973c9244ea09d46ef4d3ed13bf0d47`.

Source SHA-256:
`3786311459b4b73b8dcee8a13313611e215a46e5cf4d03f610aa4a71e2edc1d1`.

## Scope

No broad crate tests, package build, or live mount-race proof was run. This
checkpoint is limited to ActionWorker startup activation, no-replay, prompt
shutdown, root selection, and one forward exactly-once action/reply boundary.
