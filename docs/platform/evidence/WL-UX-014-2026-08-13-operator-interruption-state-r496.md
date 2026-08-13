# WL-UX-014 operator interruption state — 2026-08-13 r496

## Implemented boundary

The canonical `mde-egui::ToastHost` now suspends the exact active lower-third
state while an operator notice interrupts it. The one bounded suspended-current
slot is separate from the 64-entry waiting queue, so a mandatory operator notice
cannot be dropped when an acknowledgement-held grade-F backlog is saturated.
When the interruption ends, the displaced scene resumes ahead of ordinary
waiters with its original acknowledgement requirement or remaining timed dwell.
Excess elapsed time from the interrupting notice cannot drain a scene that was
not visible.

Forward health generations continue to coalesce against the suspended scene,
and the visible backlog count includes that scene. This closes a deterministic
S4 interruption-policy gap without introducing another renderer, queue, history
store, asset tier, or sound owner.

## Farm gates

- `.50`, slot `ux014-interruption-state-test-r496`: focused hostile regression
  `cargo test -p mde-egui toast::tests::operator_interruption_preserves_saturated_f_grade_and_timed_scene_state -- --exact --nocapture`
  passed **1/1** with 306 filtered out.
- `.90`, slot `ux014-interruption-state-clippy-r496`:
  `cargo clippy -p mde-egui --locked --all-targets -- -D warnings` passed.
- `.170`, slot `ux014-interruption-state-fmt-r496`:
  `cargo fmt -p mde-egui -- --check` passed.
- `.196`, slot `ux014-interruption-module-r496`: the distinct broader
  `cargo test -p mde-egui toast::tests -- --nocapture` gate passed **41/41**
  with 266 filtered out.

## Remaining acceptance

WL-UX-014 still requires governed source scene/audio assets, implemented
live-3D and pre-rendered tiers, device-loss transitions between those richer
tiers and the existing static tier, matching semantic captures, package/upgrade
proof, and deferred post-release direct-DRM visual/audio/performance proof.
