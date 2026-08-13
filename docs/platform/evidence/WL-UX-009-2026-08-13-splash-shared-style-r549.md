# WL-UX-009 — Construct splash shared-style roster (r549)

Date: 2026-08-13

## Implemented slice

- Replaced the boot-service roster's local responsive geometry values with one
  deterministic `BootServiceLayout` derived from the shared Quazar `Style`
  spacing and typography ladders while preserving the established 560 pt
  collapse boundary and 21/24 pt row, 3/4 pt gap, and 42/76 pt meter geometry.
- Resolved roster surface, background, text, muted text, and semantic status
  colors through the active `Style` color scheme. Light appearance no longer
  receives pinned dark-scheme paint constants.
- Added a discoverable deterministic compact/wide and Dark/Light regression.

Owned production/test file:

- `crates/desktop/mde-shell-egui/src/splash.rs`

## Farm evidence

- `.90`, slot 1 — exact focused regression:
  `cargo test -p mde-shell-egui splash::tests::splash_boot_service_layout_is_shared_style_responsive_and_appearance_aware -- --exact --nocapture`
  passed **1/1**, with 1,615 tests filtered out.
- `.196`, slot 1 — `cargo build -p mde-shell-egui --all-targets --all-features`
  passed.
- BigBoy `.130`, slot 2 — strict relevant
  `cargo clippy -p mde-shell-egui --all-targets --all-features --no-deps -- -D warnings`
  compiled the owned splash slice and stopped on the pre-existing concurrent
  `communications/mod.rs:608` `clippy::while_let_loop`; it emitted no
  `splash.rs` diagnostic. Per cadence, it was not rerun.
- `.50`, slot 1 — `cargo fmt -p mde-shell-egui -- --check` found unrelated
  concurrent drift and one owned `boot_service_style` line-wrap. The exact
  owned Rustfmt correction was applied; the broad gate was not rerun.
- Scoped `git diff --check -- crates/desktop/mde-shell-egui/src/splash.rs`
  passed before that exact whitespace-only Rustfmt correction.

No live acceptance, provider/release file, active worklist entry, or concurrent
dirty surface was changed.

## Residual WL-UX-009 acceptance

- Inventory and migrate the remaining clean Construct-owned surfaces that
  still bypass shared Style/Visuals.
- Complete deterministic largest-text and appearance fixtures across the
  shipped surface set.
- Package the frozen font/icon/style registry in the first full release.
- Perform the deferred post-release direct-DRM visual and human consistency
  review.
