# WL-UX-014 device-loss tier transition — 2026-08-13 r497

## Implemented boundary

The canonical `mde-egui::ToastHost` now owns a backend-neutral KIRON scene-tier
state machine. A complete readiness snapshot deterministically selects verified
live 3D, verified pre-rendered, or the always-available egui static tier, in
that order. Device loss immediately revokes both richer tiers even if stale
asset-ready bits remain set. Corrected-forward device readiness recovers only
to a tier whose assets are explicitly ready; missing assets can never be
selected by device recovery alone.

Tier transitions are independent from the alert queue. The hostile regression
proves fallback and recovery do not reset a timed scene's remaining dwell,
replace the visible operator interruption, alter the bounded interrupted-scene
slot, or clear a grade-F acknowledgement hold. No unavailable live or
pre-rendered asset is represented as rendered by this slice.

## Farm gates

- `.50`, slot `ux014-device-tier-hostile-r497`: focused hostile regression
  `cargo test -p mde-egui toast::tests::device_loss_falls_back_and_recovers_without_resetting_scene_lifecycle -- --exact --nocapture`
  passed **1/1** with 307 filtered out.
- `.50`, the same warm slot: full module gate
  `cargo test -p mde-egui toast::tests -- --nocapture` passed **42/42** with 266
  filtered out.
- `.90`, slot `ux014-device-tier-clippy-r497b`:
  `cargo clippy -p mde-egui --locked --all-targets -- -D warnings` passed.
- `.170`, slot `ux014-device-tier-fmt-r497c`:
  `cargo fmt -p mde-egui -- --check` passed.

The first clippy run correctly rejected a manually implemented derivable
`Default`; the implementation was corrected and the final strict run passed.
An initial `.196` full-suite lane stalled during final linking and became
unreachable over SSH, so it was stopped and no evidence is claimed from it.
The full suite was rerouted to the completed warm `.50` workspace.

## Remaining acceptance

WL-UX-014 still requires governed live-3D and pre-rendered renderer/assets,
recovery/morph timeline integration, governed scene/audio packaging, matching
semantic captures, and the deferred post-release direct-DRM visual, audio,
device-loss, upgrade, and performance proof.
