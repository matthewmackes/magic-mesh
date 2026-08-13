# WL-ARCH-009 Workers projection cutover — r529

Date: 2026-08-13

## Result

`Surface::Workers` now uses one selected canonical worker snapshot for its
grouped tree, typed relation graph, inspector, bounded history, and Action
Console. The former action-only target selector was removed. Observation-only
workers remain inspectable, while Preview stays disabled unless the selected
worker contract advertises an admitted typed action. Changing the shared
selection invalidates staged/result state before another worker can inherit it.

Owned implementation: `crates/desktop/mde-shell-egui/src/workbench.rs`.

## Farm evidence

- `.50`, slot `arch009-workers-focused-r529`:
  `cargo test -p mde-shell-egui action_console::tests -- --nocapture` passed
  4/4 (1,596 filtered out). This covers observation-only selection, authenticated
  generation-bound preview, generation invalidation, and partial-result audit.
- `.170`, slot `arch009-workers-fmt-r529b`:
  `cargo fmt -p mde-shell-egui -- --check` passed.
- `.130`, slot `arch009-workers-clippy-r529`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings` passed
  from clean `HEAD` plus this owned patch.
- The stronger `--all-targets` Clippy invocation reached the crate but remains
  red on pre-existing test-only warnings outside this ownership scope in
  `car_keymap.rs`, `status_bar.rs`, and `system/mesh.rs`. No warning originates
  from `workbench.rs`, and those concurrent files were not modified.

No live proof was performed; release and live acceptance remain separately
deferred by operator direction.
