# WL-ARCH-009 app-sync provider quiescence — 2026-08-09

## Outcome

The optional app-sync media provider now treats the shared probe-inventory
directory as its enablement anchor. When that anchor is absent, the worker
waits solely for shutdown: it does not create empty Sublime Music or Delfin
configuration, a `Mackes Media` launcher directory, or a GTK bookmark, and it
does not enter its 60-second polling loop. An existing but empty inventory root
still performs normal reconciliation so stale client state can be removed.

The optional-provider audit also inspected credential-gated backup, catalog,
alert, media-service, and app-sync startup paths. App-sync was the concrete
production gap selected because its absent anchor caused both periodic wakeups
and user-home state creation; the already-quiesced Android and Flatpak catalogs
were not retested or changed.

## Farm verification

- Machine `.50` (`172.20.0.50`), unique slot
  `arch009-app-sync-quiescence-r7-20260809`:
  `cargo test -p mackesd --lib --features async-services
  workers::app_sync::tests --locked -- --nocapture` passed 9/9, including the
  missing-anchor no-state and prompt-shutdown regression. The cold build emitted
  existing crate-wide warnings unrelated to this file and completed successfully.

## Source hash

- `7971df1b9bf3dd5e8705ad81cbb9ffa6376127a47334e8ebdb122a726e75b60b` —
  `crates/mesh/mackesd/src/workers/app_sync.rs`

This checkpoint advances optional-provider quiescence; it does not close
WL-ARCH-009.
