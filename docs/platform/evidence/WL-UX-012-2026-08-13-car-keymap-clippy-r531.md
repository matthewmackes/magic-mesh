# WL-UX-012 — Car keymap strict-Clippy correction (r531)

The strict all-target `mde-shell-egui` gate reported
`clippy::manual_string_new` at `car_keymap.rs:815`. The empty persisted key is
an intentional hostile input in the existing sanitization test, so the
coverage remains necessary. Only its construction changed from
`"".to_string()` to `String::new()`; no lint suppression or duplicate test was
added.

Farm verification:

- `.130`, `ux012-car-keymap-repro-r531`: reproduced the exact warning from a
  clean committed `vdi/resources.rs` after the initial sync included unrelated
  concurrent worktree edits.
- `.196`, `ux012-car-keymap-fmt-r531`: exact-file `rustfmt --edition 2021
  --check` passed.
- `.50`, `ux012-car-keymap-test-r531`: the existing
  `persisted_bindings_sanitize_keys_actions_and_map_size` test passed, 1/1.
- `.130`, `ux012-car-keymap-repro-r531`: strict `cargo clippy -p
  mde-shell-egui --all-targets -- -D warnings` cleared `car_keymap.rs` and then
  stopped on unrelated `status_bar.rs:2291` (`drop_non_drop`) and
  `system/mesh.rs:254` (`items_after_test_module`). Those files were outside
  this slice and were not edited.
