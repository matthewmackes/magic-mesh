# WL-FUNC-021 — compatibility worker transport authority (2026-08-06)

## Correction

The standalone Airsonic compatibility worker now emits `Update::Playing` only
when it has both an allocated audio engine and an actively loaded track. A
queued `Pause`/`Resume` before first playback, after `Stop`, or after natural
end no longer invents playback state in the worker-owned projection.

This is a worker-side guard; the embedded shell remains workerless and drains
stale compatibility updates without applying them.

## Focused verification

Farm host `.50`, slot `music-worker-authority-r3`:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-worker-authority-r3 \
  install-helpers/xcp-build.sh cargo test --locked -p mde-music-egui \
  worker::tests:: --lib -- --nocapture
result: 3 passed, 0 failed; 52 filtered out
```

Farm host `.130`, slot `music-worker-authority-fmt-r5`:

```text
rustfmt --edition 2021 --check \
  crates/desktop/mde-music-egui/src/worker.rs
result: pass
```

The crate-wide formatter still reports an unrelated pre-existing formatting
difference in `src/model.rs`; that file was not changed.

## Scope

Changed files are limited to `crates/desktop/mde-music-egui/src/worker.rs` and
this evidence record. `docs/platform/WORKLIST.md`, the daemon, media core, and
shared governance files were not changed.
