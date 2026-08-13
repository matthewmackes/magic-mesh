# WL-UX-009 — Bluetooth stale mutation authority (r514)

## Implementation

The System Bluetooth panel now binds every BlueZ mutation to the monotonic
receipt time of the latest off-thread seat snapshot. After three missed
five-second publications, the retained adapter/device topology remains visible
for diagnosis but is explicitly marked stale and all controls are disabled.
The action-application seam independently rejects already-queued Bluetooth
mutations, so a same-frame or synthetic stale action cannot bypass the render
gate. A fresh seat publication restores authority.

Changed production path:

- `crates/desktop/mde-shell-egui/src/system/mod.rs`

## Farm evidence

- `172.20.0.90`, slot `ux009-bt-test`:
  `cargo test -p mde-shell-egui stale_bluetooth_snapshot_revokes_queued_mutation_authority -- --nocapture`
  passed 1/1 with 1,592 tests filtered after a cold 13m01s build.
- `172.20.0.170`, slot `ux009-bt-clippy`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings`
  passed in 5m19s.
- `172.20.0.196`, slot `ux009-bt-fmt`:
  `cargo fmt -p mde-shell-egui -- --check` passed.
- BigBoy `172.20.0.130` was not used.
- `git diff --check` passed before commit.

## Remaining acceptance

WL-UX-009 still requires complete shared Style/Visuals migration, first full
release payload verification, and the deferred non-blocking post-release
Dark/Light, narrow/largest-text, direct-DRM, hardware-provider, and human visual
review. This slice makes no live-seat or release claim.
