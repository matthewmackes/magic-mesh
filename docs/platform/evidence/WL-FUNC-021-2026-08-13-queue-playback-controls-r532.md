# WL-FUNC-021 queue playback controls — r532

Date: 2026-08-13

## Production slice

The Music workspace bottom player now routes Previous, Next, Shuffle, and
Repeat through the authenticated daemon action publisher. Each mutation carries
the exact `queue_revision` from the retained workspace projection, refuses an
absent/currentless queue, and never mutates UI-local playback policy. Under
daemon authority, Previous no longer masquerades as a seek-to-zero action.

Owned implementation:

- `crates/desktop/mde-music-egui/src/app.rs`

## Farm evidence

- BigBoy `172.20.0.130`, slot `func021-queue-suite`:
  `cargo test -p mde-music-egui --lib` — PASS, 77 passed, 0 failed. This includes
  `queue_controls_publish_exact_rendered_generation_and_policy`.
- `172.20.0.170`, slot `func021-queue-clippy`:
  `cargo clippy -p mde-music-egui --all-targets -- -D warnings` — PASS.
- `172.20.0.90`, slot `func021-queue-fmt-final`:
  `cargo fmt -p mde-music-egui -- --check` — the owned `app.rs` is clean; the
  package gate reports only pre-existing formatting drift in unowned
  `crates/desktop/mde-music-egui/src/main.rs` lines 32 and 76. That file was not
  changed under this slice's ownership.
- `172.20.0.50`, slot `func021-queue-test`: the focused test compile was stopped
  after the stronger BigBoy full-library suite passed the same test, avoiding a
  duplicate gate.

## Remaining WL-FUNC-021 acceptance

This slice closes the workspace wiring gap for daemon-owned queue traversal and
playback policy. The epic still requires the deferred shipped-release proof for
audible/network-loss recovery, physical cast, peer handoff, package/runtime
identity, and the bounded live-seat evidence named by the active worklist.
