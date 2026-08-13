# WL-UX-009 Front Door shared-style migration — r548

Date: 2026-08-13

## Production result

The Construct-owned Front Door no longer maintains a second set of literal
panel, row, input, chip, action, tooltip, stroke, and clipping measurements.
Its established geometry is now derived from the shared Quazar `Style` spacing,
control-height, radius, icon, and hairline tokens. The migration preserves the
current dimensions while ensuring future shared-style changes propagate as one
coherent system.

A deterministic regression binds the geometry to those shared tokens and checks
that compact 220x480, portrait 430x900, and desktop 1280x800 panel variants stay
inside their viewport in both panel and expanded modes.

## Scope

- `crates/desktop/mde-shell-egui/src/front_door.rs`
- This evidence file

No navigation bar, workbench, health modal, motion, worklist, provider, release,
or concurrently dirty file was edited by this slice.

## Gate evidence

- `.196`, slot 1 —
  `cargo test -p mde-shell-egui front_door_geometry_is_shared_style_derived_and_responsive -- --exact --nocapture`
  completed compilation successfully, but the selector discovered **0 tests**
  (`1613 filtered out`). This is recorded as insufficient evidence, not a pass;
  it was not rerun under the stop-after-current-gate directive.
- `.90`, slot 2 —
  `cargo clippy -p mde-shell-egui --all-targets --all-features -- -D warnings`
  compiled the changed shell source, then stopped on the pre-existing,
  out-of-scope `clippy::while_let_loop` at
  `crates/desktop/mde-shell-egui/src/communications/mod.rs:608`.
- `.170`, slot 2 —
  `cargo build -p mde-shell-egui --all-targets --all-features` compiled the
  changed shell and reached final link, then failed because that farm fixture
  lacks the external `mpv` library (`mold: fatal: library not found: mpv`).
- Scoped `git diff --check` passed.
- No replacement, rerun, broader gate, or live acceptance was started after the
  explicit cadence instruction.

## Residual WL-UX-009 acceptance

This closes one Front Door S2/S3 migration slice, not the epic. Remaining work
includes inventorying and migrating other active Construct surfaces, completing
the deterministic appearance/responsive fixture matrix, packaging frozen
fonts/icons/styles in the first release, and deferred post-release direct-DRM
motion and visual-consistency acceptance.
