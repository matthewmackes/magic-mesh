# WL-FUNC-021 evidence — bounded standalone Music worker lanes (2026-08-06)

Working-tree base revision: `e52322ec` (changes are intentionally uncommitted).

## Implemented invariant

The standalone/embedded `mde-music-egui` worker handoff no longer uses
unbounded standard-library channels. UI commands are admitted through a fixed
64-item synchronous queue; worker updates use a fixed 256-item queue and apply
backpressure at the worker boundary. The UI uses non-blocking `try_send` for
commands and surfaces queue saturation as an honest error instead of freezing
the render thread or silently growing memory. Provider credentials, catalog
access, playback engine ownership, and mutations remain in the existing Music
worker/daemon contracts; no second provider authority was introduced.

Changed files:

- `crates/desktop/mde-music-egui/src/app.rs`
- `crates/desktop/mde-music-egui/src/worker.rs`

## Farm verification

All heavy verification ran on explicit farm host `.50` in isolated slot
`music-worker-r1`:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-worker-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-music-egui -- --nocapture
result: 37 passed, 0 failed; 0 binary tests; 0 doctests

ssh mm@172.20.0.50 \
  'rustfmt --edition 2021 --check \
   crates/desktop/mde-music-egui/src/app.rs \
   crates/desktop/mde-music-egui/src/worker.rs'
result: pass
```

The hostile regressions are `music_worker_command_queue_is_bounded` and
`music_worker_update_queue_is_bounded`. A post-change search of the Music UI
source finds only `sync_channel` for the worker/UI lanes; no unbounded
`mpsc::channel` remains in the crate. Local `git diff --check` passed.

## Remaining migration and runtime proof

This is a bounded handoff step, not completion of the GUI-worker migration.
The direct Airsonic/engine worker remains to be retired after complete Bus
parity, with standalone/shell migration, live two-catalog playback, network
loss, seat handoff, DLNA, and direct-DRM proof still open. Dell runtime was not
mutated or rebooted.

## Source hashes at capture

```text
2a77ed229d0bf22bc0663f463c2152bcf4d920e3f782e56eb9bc43b749ee42c7  crates/desktop/mde-music-egui/src/app.rs
71e73598653e5df9ad7f225eec7079a9e9c245df3a356ab8464a6c5abfd6f8fd  crates/desktop/mde-music-egui/src/worker.rs
```
