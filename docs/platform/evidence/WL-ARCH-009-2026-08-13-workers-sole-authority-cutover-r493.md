# WL-ARCH-009 Workers sole-authority cutover — r493

Date: 2026-08-13

## Outcome

`Surface::Workers` is now the only reachable navigation authority for the
former Workbench provider views. Historical Fleet Mesh, Workbench, and This
Node surface aliases normalize directly to the governed This Node overview
leaf instead of the retired aggregate `WorkersDestination::ThisNode` route.

The unreachable `show_fleet_mesh` renderer and its duplicate Workbench chrome
were removed. This deletes the second View menu, plane rail, plane guide,
status-chip bar, and embedded Action Console authority. The Workers catalog
continues to mount each provider through `show_catalog_plane`, and Action
Console remains one first-class Workers leaf.

The stable `Plane::label` mapping remains because Front Door typed workflow
actions use it for operator and audit text; it does not render navigation
chrome. Persisted historical `Surface` variants remain accepted only at the
single normalization seam.

## Changed paths

- `crates/desktop/mde-shell-egui/src/main.rs`
- `crates/desktop/mde-shell-egui/src/workbench.rs`

The existing alias regression now proves both canonical `Surface::Workers`
ownership and the exact leaf destination selected for every historical route.

## Farm gates

- `.90`, slot `arch009-workers-sole-authority-test-r493b`: `cargo test -p
  mde-shell-egui tests::legacy_node_surfaces_normalize_into_workers_tabs --
  --exact --nocapture` passed 1/1 with 1,577 filtered tests in 21.16 seconds.
- `.170`, slot `arch009-workers-sole-authority-clippy-r493b`: `cargo clippy -p
  mde-shell-egui --bin mde-shell-egui --no-deps -- -D warnings` passed.
- `.196`, slot `arch009-workers-sole-authority-fmt-r493e`: file-scoped
  `rustfmt --edition 2021 --check` passed for both changed Rust files.

An initial strict clippy run correctly exposed that `Plane::label` remains a
Front Door data-contract dependency; only that non-navigation compatibility
mapping was restored before the green rerun. An initial focused-test command
selected zero tests because its `--exact` filter omitted the module path; it was
not counted, and the qualified 1/1 rerun above is the acceptance gate.

## Remaining epic acceptance

ARCH-009 still requires the broader package/process gate set and deferred
post-release fleet/live convergence proof. No fleet or installed-seat claim is
made by this source-level authority cutover.
