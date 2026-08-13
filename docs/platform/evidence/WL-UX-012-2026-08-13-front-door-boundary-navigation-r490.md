# WL-UX-012 Front Door boundary navigation — r490

## Implemented slice

The shell-owned Front Door now consumes `Home` and `End` in its production
keyboard path. They move the active result directly to the first or final
visible ranked row; `Enter` then activates that exact row. Either boundary move
also clears armed workload/service lifecycle confirmation so stale destructive
intent cannot follow selection movement.

This closes a reachable search-first Home gap for the full launcher catalog. It
does not create a Start menu, second launcher, or alternate result model.

## Farm evidence

- `.50`, slot `ux012-frontdoor-home-end-test-r490`:
  `cargo test -p mde-shell-egui --locked --bin mde-shell-egui front_door::tests::front_door_keyboard_home_and_end_activate_result_boundaries -- --exact --nocapture`
  passed 1/1 (1,585 filtered).
- `.50`, slot `ux012-frontdoor-home-end-fmt-r490b`:
  `cargo fmt -p mde-shell-egui -- --check` passed.
- `.170`, slot `ux012-frontdoor-home-end-nodeps-clippy-r490`:
  `cargo clippy -p mde-shell-egui --locked --bin mde-shell-egui --no-deps -- -D warnings`
  passed. `--no-deps` keeps warnings strict for the owned production shell while
  excluding unrelated dependency lint.

The broader all-target clippy attempt reached unrelated pre-existing warnings in
`car_keymap.rs`, `status_bar.rs`, and `system/mesh.rs`; the first production-bin
attempt then reached an unrelated `mde-collab-egui/src/activity.rs` dependency
warning. Those paths are outside this slice and were preserved.

## Remaining epic acceptance

WL-UX-012 still requires the deferred release-phase responsive/package/live-seat
matrix for Bottom/Left, Dark/Light, large text, lock, multi-display, and session
switching, with no clipping, focus loss, or duplicate launcher.
