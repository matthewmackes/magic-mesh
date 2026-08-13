# WL-UX-009 — System Mouse & Touch responsive cards (r510)

Date: 2026-08-13

## Implementation

The reachable **System → Devices → Mouse & Touch** surface no longer forces its
Pointer/Scroll and Touchpad/Surface card pairs into two columns. Each pair now
uses the shared Quazar `TILE_MIN_W`/`fit_columns` layout authority already used
by the Bluetooth and Power provider panels: wide layouts retain two equal
cards, while narrow and largest-text layouts stack full-width cards before
either column can be crushed. Control behavior and the typed seat-provider
boundary are unchanged.

Changed production scope:

- `crates/desktop/mde-shell-egui/src/system/mod.rs`

## Farm evidence

- BigBoy `172.20.0.130`, slot `ux009-system-responsive`:
  `cargo test -p mde-shell-egui mouse_touch_cards_collapse_before_either_column_is_crushed -- --nocapture`
  passed 1/1 with 1,586 tests filtered.
- `172.20.0.170`, slot `ux009-system-clippy`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings` passed.
- `172.20.0.50`, slot `ux009-system-rustfmt`:
  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/system/mod.rs`
  passed.
- `git diff --check` passed locally.

An initial workspace-wide Cargo formatting probe exposed unrelated existing
format drift and was discarded; it changed no files and is not acceptance
evidence for this slice.

## Remaining acceptance

WL-UX-009 remains open for complete Style/Visuals migration across all shipped
Construct surfaces, first-release payload verification, and the deferred
post-release Dark/Light, narrow, largest-text, disabled/stale/unavailable
direct-DRM capture and human review matrix.
