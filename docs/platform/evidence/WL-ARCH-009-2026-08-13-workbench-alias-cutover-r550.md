# WL-ARCH-009 — retired Workbench alias cutover (r550)

Date: 2026-08-13

## Implementation

The retired `Surface::Workbench` value no longer selects a Workbench plane or
replaces the current Workers destination with the local-node overview. Stale
persisted values collapse into the canonical `Surface::Workers` shell while the
destination already selected through `WorkersDestination` remains authoritative.

The hostile regression
`retired_workbench_surface_cannot_override_typed_action_console_destination`
selects `WorkersDestination::ActionConsole` through the typed entry point,
injects the retired surface value, and verifies that the alias cannot replace
the destination, tab, or authority.

## Gates

- BigBoy `.130`, slot 3: the module-qualified hostile selector discovered
  exactly one test after a successful test-profile compile. The test entered
  execution but did not complete within the finishing window and was stopped;
  it is recorded as non-passing debt, not green evidence.
- `.196`, slot 1: strict all-target/all-feature Clippy reached
  `mde-shell-egui` and stopped solely on the pre-existing
  `communications/mod.rs:608` `clippy::while_let_loop` diagnostic. No owned
  diagnostic was emitted.
- `.196`, slot 1: the all-target/all-feature build was started once, then
  stopped during dependency compilation to honor the requested immediate
  finish. It is not claimed passing.
- `.90`, slot 1: package formatting ran once. It reported pre-existing drift
  in `front_door.rs` and unrelated regions of `main.rs`; no delta intersects
  the Workbench alias arm or its hostile regression.
- Owned-scope `git diff --check`: passed.

## Residual ARCH-009 acceptance

- Cut over the remaining Fleet and This Node legacy aliases independently.
- Finish Network Operations projections and the provider/action ownership
  inventory.
- Complete responsive and largest-text evidence.
- Run deferred non-blocking one-node acceptance after the first full release.
