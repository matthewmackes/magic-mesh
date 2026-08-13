# WL-FUNC-017 navigation checkpoint inode integrity — r494

Date: 2026-08-13

## Acceptance gap

The daemon-owned navigation worker durably checkpoints route generation, phase,
action cursors, and replay reservations before publishing state. The route
provider authority was already opened through a no-follow, same-inode boundary,
but restart recovery used an ordinary `File::open` for that checkpoint. A
symlinked or hard-linked checkpoint could therefore redirect or alias the state
used to recover navigation authority, contrary to S6's no-stale-route and
deterministic restart requirements.

## Implementation

`load_record` now fails closed unless the checkpoint is a bounded regular file
with exactly one link. It opens with Linux `O_NOFOLLOW | O_CLOEXEC` and verifies
that the opened descriptor is still the same device and inode inspected before
open. Existing size, schema, host, phase, and generation validation remains in
force after the secure read.

The focused restart regression creates both a symlink and a hard-link alias for
a valid durable checkpoint and proves neither can populate in-memory navigation
authority.

## Farm gates

- `.170`, slot `func017-navigation-checkpoint-test-r494b`:
  `cargo test -p mackesd --features async-services workers::navigation::tests::restart_rejects_aliased_navigation_checkpoint -- --exact --nocapture`
  passed 1/1 with 4,936 filtered out.
- `.50`, slot `func017-navigation-checkpoint-clippy-r494`:
  `cargo clippy -p mackesd --lib --features async-services -- -D warnings`
  passed.
- `.50`, slot `func017-navigation-checkpoint-fmt-r494`:
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/navigation.rs`
  passed in the isolated farm workspace.

An earlier focused invocation on BigBoy `.130` was stopped before verdict when
the operator directed outstanding work to the newly free `.170` lane. It is not
counted as evidence and did not run concurrently with the gate of record.

## Remaining epic acceptance

This slice closes aliased durable-state admission during daemon restart. The
epic still requires provisioned offline map/provider data, configured MG90
manager and hardware recovery proof, live NWS/atmospheric publication,
direct-DRM Maps/weather/navigation review, and deferred post-release seat proof
for the weather deep link and Car route/radio health.
