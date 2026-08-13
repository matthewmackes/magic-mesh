# WL-UX-009 control-center responsive grid — r506

Date: 2026-08-13

## Production correction

`crates/desktop/mde-shell-egui/src/control_center.rs` no longer forces the
Control Center's state and route tiles into two columns after the card is
clamped to a narrow viewport. The grid now uses one column when two readable
Quazar tile widths do not fit, and the same column decision drives both content
height and painting. All tiles therefore remain reachable through the bounded
scroll region instead of captions painting into neighbouring cells.

The hostile regression exercises the six-tile all-providers-absent state at a
320 by 240 point viewport, proves desktop geometry remains two-column, proves
narrow geometry reserves six one-column rows, and renders the settled panel.

## Farm gates

- `.196`, slot `ux009-cc-test-reroute`:
  `cargo test -p mde-shell-egui a_narrow_control_center_collapses_tiles_without_hiding_rows -- --nocapture`
  passed 1/1 with 1,583 filtered tests. This was the single reroute from
  unreachable BigBoy `.130`; the first `.196` run exposed and corrected an
  initial-spring fixture assertion before the warmed rerun passed.
- `.170`, slot `ux009-cc-clippy`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings` passed.
- `.50`, slot `ux009-cc-fmt-file`: file-scoped
  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/control_center.rs`
  passed. The earlier crate-wide formatter probe also reported unrelated
  existing drift in `src/vdi/resources.rs`; that file was not changed.
- `git diff --check` passed.

## Remaining epic acceptance

WL-UX-009 still requires remaining Construct surface Style/Visuals adoption,
supported appearance/responsive captures, release payload verification, and
the deferred non-blocking post-release direct-DRM motion/focus/human review.
