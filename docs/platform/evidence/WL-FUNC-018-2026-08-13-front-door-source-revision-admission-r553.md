# WL-FUNC-018 — Front Door source-revision admission (r553)

Date: 2026-08-13

## Production result

Front Door now retains the exact `source_revision` from each admitted Flatpak
catalog row. A row without that bounded catalog metadata is not considered an
admitted Flatpak launch candidate. The source revision also participates in
same-peer/same-app declaration equivalence, so two declarations that keep the
same application ID and catalog revision while substituting the guest package
revision are treated as equivocation and withheld from launch projection.

This closes one executable S1 catalog-admission gap without changing Mackes
worker ownership or claiming that the first App-VM image has shipped.

## Changed production files

- `crates/desktop/mde-shell-egui/src/front_door_peer_apps.rs`
- `crates/desktop/mde-shell-egui/src/front_door.rs`

## Farm evidence

Host/slot: `172.20.0.196/1`

```text
cargo test -p mde-shell-egui source_revision -- --nocapture
running 2 tests
test front_door_peer_apps::tests::validated_catalog_source_revision_projects_into_front_door_row ... ok
test front_door::tests::source_revision_substitution_never_becomes_a_launch_target ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 1619 filtered out
```

The command compiled `mde-shell-egui` before executing the two exact hostile
regressions. A prior attempt on `.170/1` was stopped before compilation because
that slot was waiting on an artifact lock; it was not duplicated.

Scoped `git diff --check` passed.

## Explicit gate debt

- No separate all-target build or strict Clippy gate was started after the
  unique focused gate completed, per the operator's instruction to finish and
  push rather than launch duplicate broad gates.
- `cargo fmt -p mde-shell-egui -- --check` on `.170/1` reported pre-existing
  formatting drift in `front_door.rs` and concurrent dirty `main.rs`. Only the
  newly authored lines were reconciled; unrelated formatting was not changed.
- The farm emitted the pre-existing `mde-vdi-rdp::begin_connection_generation`
  dead-code warning while compiling dependencies.

## Remaining WL-FUNC-018 acceptance

- Cut and install the first governed App-VM image and catalog payload.
- Complete the remaining typed launch/stop cleanup and guest package authority
  binding beyond this Front Door admission boundary.
- Run the deferred post-release one-node Wayland-app VDI, GPU/audio, restart,
  failure, and cleanup acceptance.
