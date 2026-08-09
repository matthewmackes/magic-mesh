# WL-ARCH-010 / WL-CRIT-007 — session Bus-loss safety (r19)

Date: 2026-08-09

Source baseline: `f287c04f`

Source SHA-256:
`7d3eb8ee642445822628a454e927a5397d5d91e7756b9d563a20f86ebd1cd0a6`

## Correction

`SessionBrokerWorker` now preserves an explicit/default Bus root and selects
the documented shared `/run/mde-bus` spool when a system service has no user
data root. Its action-log read returns a typed failure instead of converting
`Persist::open` or `list_since` failure into an empty action set.

This distinction closes a destructive convergence path: an unreadable Bus can
no longer appear to be an empty desired roster and remove otherwise live
sessions from the shared session store. The worker remains active, leaves the
existing roster/store untouched, and retries on its normal bounded cadence.
Once the Bus returns, its unchanged `None` cursor folds the complete authorized
session log before convergence, preserving restart recovery rather than
dropping queued lifecycle state.

## Focused farm verification

Machine 193 (`172.20.0.90`), slot `session-bus-recovery-r18`:

```text
cargo test -p mackesd --features async-services --lib \
  workers::session_broker::tests::unavailable_bus_defers_convergence_without_removing_live_sessions \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,423 filtered out`.

```text
cargo test -p mackesd --features async-services --lib \
  workers::session_broker::tests::default_bus_root_uses_the_shared_mde_bus_resolver \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,423 filtered out`.

The exact single-file `rustfmt --check` and scoped `git diff --check` passed.
No broad suite, package build, or unrelated test was run.
