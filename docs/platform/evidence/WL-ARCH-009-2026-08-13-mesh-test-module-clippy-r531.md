# WL-ARCH-009 — mesh test-module Clippy correction (r531)

Date: 2026-08-13  
Source commit under test: `9cbf44cf`  
Owned production file: `crates/desktop/mde-shell-egui/src/system/mesh.rs`

## Result

Strict all-target Clippy diagnosed `clippy::items_after_test_module`: the existing
`system::mesh` tests were located before later production section functions. The
tests already covered the behavior, so the correction moves the unchanged test
module to the end of the file. No behavior, test case, or lint policy changed.

## Farm gates

- `.50`, slot `arch009-mesh-fmt-r531`: `rustfmt --edition 2021 --check
  crates/desktop/mde-shell-egui/src/system/mesh.rs` passed.
- `.90`, slot `arch009-mesh-test-r531`: `cargo test -p mde-shell-egui
  mesh_summary -- --nocapture` passed: 4 passed, 0 failed.
- BigBoy `.130`, slot `arch009-mesh-clippy-clean-r531`: `cargo clippy -p
  mde-shell-egui --all-targets -- -D warnings` passed the owned file and then
  failed on exactly two unrelated test warnings:
  - `crates/desktop/mde-shell-egui/src/car_keymap.rs:815`,
    `clippy::manual_string_new`.
  - `crates/desktop/mde-shell-egui/src/status_bar.rs:2291`,
    `clippy::drop_non_drop`.
- The same warmed BigBoy slot with only those two known, out-of-scope lints
  demoted (`-A clippy::manual-string-new -A clippy::drop-non-drop`) passed all
  targets, proving no remaining warning in `system/mesh.rs`.
- Local `git diff --check` passed for the owned files.

The unrelated warnings and concurrent dirty files were not edited.
