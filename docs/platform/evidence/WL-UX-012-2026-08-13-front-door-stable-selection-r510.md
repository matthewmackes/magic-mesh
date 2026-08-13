# WL-UX-012 S2 — Front Door stable typed selection

Date: 2026-08-13

## Result

Front Door now retains the selected typed `FrontDoorTarget`, rather than
treating a live result-list index as activation authority. Provider reorder
keeps the same target selected. If that target disappears, Front Door paints a
deterministic replacement but refuses same-frame Enter activation, preventing a
stale input event from opening a different app, workload, service, file, peer,
Browser result, or command. The replacement becomes authoritative only on a
subsequent frame or explicit navigation/click.

Query, filter, open/close, command-mode, keyboard movement, pointer selection,
and selected-peer projection all reset or reconcile the typed identity. No raw
command route, second launcher, persistence model, or render-path I/O was added.

## Farm verification

- BigBoy `172.20.0.130`, slot `ux012-front-selection`:
  `cargo test -p mde-shell-egui front_door_selection_follows_typed_target_and_refuses_stale_substitution -- --nocapture`
  passed 1/1 with 1,587 filtered.
- `172.20.0.170`, slot `ux012-front-clippy`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings`
  passed.
- `172.20.0.50`, slot `ux012-front-fmt`: file-scoped
  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/front_door.rs`
  passed. Package-wide formatting also exposed unrelated existing drift in
  `src/vdi/resources.rs`; that concurrent file was not changed by this slice.
- `git diff --check` passed.

## Remaining acceptance

WL-UX-012 still requires first-full-release payload integration and the
operator-deferred, non-blocking post-release proof of Bottom/Left, Dark/Light,
largest-text, lock, multi-display, session switching, upgrade, and physical-seat
behavior. This slice closes the live-result selection-substitution coding gap;
it does not claim those deferred acceptance results.
