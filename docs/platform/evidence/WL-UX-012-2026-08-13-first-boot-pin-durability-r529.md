# WL-UX-012 first-boot pin durability — r529

Date: 2026-08-13

Source commits: `ec1e0244` (behavior and test), `3dcb8f18` (rustfmt)

## Result

The first-boot taskbar selector now treats the configured profile and its exact
ordered pins as one durable mutation. A failed preference write leaves the
profile in `New`, retains the user's pending selection, and publishes no live
pins. Only a successful atomic preference write changes the running projection,
dismisses personalization, and allows restart to restore `Configured` with the
same pin order. Empty selections remain an explicit durable user choice.

## Farm gates

- `.50`, slot `ux012-pin-durable-r529`: the module-qualified focused test
  `nav_bar::tests::new_profile_selection_publishes_only_after_configured_pins_are_durable`
  passed 1/1, including simulated write failure, successful atomic persistence,
  and restart reconstruction.
- `.170`, slot `ux012-fmt-r529b`: `cargo fmt -p mde-shell-egui -- --check`
  passed against `3dcb8f18`.
- `.130`, slot `ux012-clippy-r529`: strict
  `cargo clippy -p mde-shell-egui --all-targets -- -D warnings` reached the
  affected crate but was blocked by three unrelated existing warnings:
  `car_keymap.rs:815` (`manual_string_new`), `status_bar.rs:2291`
  (`drop_non_drop`), and `system/mesh.rs:254` (`items_after_test_module`). No
  warning points at `nav_bar.rs`; expanding this bounded slice to repair those
  concurrent scopes was intentionally refused.

Local `git diff --check` passed for the owned file. No live proof was run.
