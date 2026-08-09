# WL-UX-013 S4 health action publication — r9

- Date: 2026-08-09
- Base commit: `fcc4c013292ac57b31420e986f8878821991f30a`
- Farm host: machine 196 (`172.20.0.196`)
- Farm slot: `health-action-publish-r9`
- Source SHA-256: `159181d14937ea8e2ba09283c64291a5d1bfa174cb3715ab423e0104982d1732`

## Production correction

`crates/desktop/mde-shell-egui/src/health_modal.rs` now returns an exhaustive,
bounded `ActionPublishOutcome` for missing Bus root, Persist open,
serialization, Persist write, and successful publication. Failures are rendered
as bounded local modal errors. A confirmed action remains pending after every
publication failure and clears only after a successful Persist write. The wire
request continues to bind the selected target, condition identity, snapshot
generation, action, and explicit confirmation.

## Hostile proof

The exact test covers three paths in one bounded fixture:

1. no Bus root returns `BusRootUnavailable`, presents the failure, and retains
   confirmation;
2. a regular file blocking the `action/` topic hierarchy returns
   `PersistWrite`, presents the failure, and retains confirmation;
3. a writable Bus publishes one typed request, preserves target and generation
   binding, and only then clears confirmation and the prior error.

Exact command run in `/home/mm/magic-mesh-farm-health-action-publish-r9`:

```text
cargo test -p mde-shell-egui health_modal::tests::action_publication_reports_hostile_failures_and_preserves_bound_success -- --exact --nocapture
```

Result:

```text
running 1 test
test health_modal::tests::action_publication_reports_hostile_failures_and_preserves_bound_success ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1507 filtered out
```

The exact source-file format gate also passed on machine 196:

```text
rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/health_modal.rs
```

During iteration, compilation first rejected direct removal of a non-`Default`
egui temporary value and then a test-only struct update across private fields.
The correction stores the failure in a defaultable `Option` container and
initializes `ConstructChrome` through its public default plus field assignment;
the final exact test above compiled and passed after both corrections.

No broad or full tests were run. Package-wide `cargo fmt --check` was not used
as evidence because it reports pre-existing formatting drift in unrelated shell
files; the requested source file passed its exact format check.
