# WL-UX-012 responsive taskbar and largest-text evidence (r549)

Date: 2026-08-13

Scope:

- `crates/desktop/mde-shell-egui/src/nav_bar.rs`
- deterministic Bottom/Left geometry and largest-text overflow behavior only

Production result:

- Bottom remains the locked full-width 48px icon-only taskbar; Left remains a
  display-owning 56px rail.
- Both orientations derive their fixed controls from one typed profile. Compact
  Left ordering preserves Start, Workloads, and Back first without minting a
  second action authority.
- The textual More surface grows each row from the configured label size and
  zoom while preserving the 40px minimum target. Invalid/non-finite label sizes
  fail to the normal bounded target.
- The discoverable regression
  `nav_bar::tests::responsive_bottom_left_and_largest_text_keep_one_of_every_required_action`
  covers compact Bottom, portrait Left, desktop Bottom, and a 2x-text reduced
  logical Left viewport. It asserts exactly one Start, Search, Workloads, Back,
  Home, and placement control, bounded/disjoint hit geometry, and unclipped
  largest-text overflow rows.

Gate record (one invocation per requested class; no reruns under stop cadence):

- `.50`, slot 1, focused test: **not executed**. Remote synchronization failed
  with rsync code 23 while a volatile ignored OpenTofu provider cache entry was
  concurrently renamed. Therefore no test-discovery or pass claim is made.
- `.196`, slot 1, `cargo build -p mde-shell-egui --all-targets`: **passed** in
  8m48s. One unrelated existing `mde-vdi-rdp` dead-code warning was emitted.
- BigBoy `.130`, slot 3,
  `cargo clippy -p mde-shell-egui --all-targets --all-features -- -D warnings`:
  compiled the changed shell and stopped on the pre-existing out-of-scope
  `communications/mod.rs:608` `clippy::while_let_loop` diagnostic. No
  `nav_bar.rs` diagnostic was emitted.
- `.50`, slot 2, `cargo fmt -p mde-shell-egui -- --check`: found two owned
  `nav_bar.rs` line-shape deltas plus unrelated concurrent drift. The two owned
  deltas were applied exactly; the gate was not rerun.
- Scoped working-tree `git diff --check`: **passed** before the exact Rustfmt
  line-shape corrections; those corrections introduce no whitespace errors.

Residual acceptance:

- Execute the named focused regression with nonzero discovery on a stable farm
  sync lane.
- Complete lock, multi-display, session-switching, and release-package audit.
- Perform deferred post-release direct-DRM Bottom/Left and largest-text captures.

