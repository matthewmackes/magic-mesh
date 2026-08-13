# WL-UX-009 — Construct switcher shared-style cards (r550)

Date: 2026-08-13

## Implemented slice

- Re-anchored the app switcher's card width, 16:10 preview, header, glyphs,
  and selected underline to the shared Quazar `Style` spacing, control, icon,
  and focus ladders while preserving the established 256 x 196 point card.
- Kept card, preview, text, border, focus, scrim, and accent paint resolved
  through the active shared color scheme rather than introducing a local
  palette.
- Added a discoverable rendered regression covering Dark and Light paint plus
  the effective compact viewport produced by the largest 2x text/zoom setting.

Owned production/test file:

- `crates/desktop/mde-shell-egui/src/switcher.rs`

## Farm evidence

- BigBoy `.130`, slot 1 — exact focused regression
  `cargo test -p mde-shell-egui switcher::tests::shared_style_palette_and_zoomed_compact_layout_remain_legible -- --exact --nocapture`
  compiled the full dependency graph and reached final shell linkage. It was
  stopped at operator direction before test execution, so discovery and pass
  count remain unproven.
- BigBoy `.130`, slot 2 —
  `cargo build -p mde-shell-egui --all-targets --all-features` compiled the
  full dependency graph and reached final shell linkage. It was stopped at
  operator direction before completion, so this is not a passing build claim.
- `.90`, slot 1 — strict relevant
  `cargo clippy -p mde-shell-egui --all-targets --all-features --no-deps -- -D warnings`
  reached `mde-shell-egui` and stopped solely on the pre-existing concurrent
  `communications/mod.rs:608` `clippy::while_let_loop`; no `switcher.rs`
  diagnostic was emitted. It was not rerun.
- `.196`, slot 1 — `cargo fmt -p mde-shell-egui -- --check` ran once and
  remained red from unrelated existing `front_door.rs` and `main.rs` drift plus
  three owned switcher line wraps. Those three wraps were corrected exactly;
  the broad formatter was not rerun.
- Scoped `git diff --check` over the owned switcher and evidence files passed.

Gate debt: one future permitted wave must obtain nonzero execution of the exact
regression and a completed all-target build. Strict Clippy remains blocked by
the separately owned Communications diagnostic; package formatting remains
blocked by separately owned Front Door/Main drift.

No live acceptance, provider, Workers, release, active worklist, or concurrent
dirty file was changed.

## Residual WL-UX-009 acceptance

- Inventory and migrate the remaining clean Construct-owned surfaces that
  still bypass shared Style/Visuals.
- Complete deterministic largest-text and appearance fixtures across the
  shipped surface set.
- Package the frozen font/icon/style registry in the first full release.
- Perform the deferred post-release direct-DRM visual and human consistency
  review.
