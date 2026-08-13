# WL-UX-012 status-bar strict Clippy correction — r531

Date: 2026-08-13

## Result

The weather-click isolation test no longer calls `drop` on a closure that has
no destructor. The closure existed only to hold a mutable borrow of
`ConstructChrome` while synthetic pointer events were dispatched; a lexical
scope now expresses that lifetime directly before the test inspects the chrome
state. Product behavior and coverage are unchanged, and no lint suppression or
duplicate test was added.

## Farm gates

- `.130`, slot `ux012-status-clippy-r531`: the pre-change strict
  `cargo clippy -p mde-shell-egui --all-targets -- -D warnings` reproduced
  `clippy::drop_non_drop` at `status_bar.rs:2291`.
- `.130`, the same isolated slot: exact-file Rust 1.94 rustfmt check passed.
- `.130`, the same warm slot: the exact focused test
  `status_bar::tests::one_weather_click_emits_only_the_weather_navigation_action`
  passed 1/1.
- `.130`, the same warm slot: the post-change strict all-target Clippy run no
  longer reports any warning in `status_bar.rs`. The crate remains red only on
  the preserved, out-of-scope warnings `car_keymap.rs:815`
  (`clippy::manual_string_new`) and `system/mesh.rs:254`
  (`clippy::items_after_test_module`).

The gate used a detached clean worktree at the source revision, so unrelated
dirty files in the orchestrator worktree were not included.
