# WL-FUNC-021 — bounded GUI authority audit (2026-08-06)

## Scope

This slice audits `mde-music-egui` S3 only. The active worklist was not edited;
the change is intentionally limited to the Music UI authority boundary and this
evidence record.

## Implemented invariant

`MusicApp::daemon_authority_active` treats embedded mode, an accepted retained
daemon snapshot, or a shell-owned action/browse writer as daemon authority. In
that mode:

- Home, Library, Album, and Search wait honestly for daemon state instead of
  rendering legacy Airsonic worker state while the snapshot is absent.
- A transport action with no authenticated daemon writer reports unavailable and
  does not queue the same intent to the local worker.
- Embedded `music_pump` drains stale compatibility updates without applying them
  to playback or store state; the embedded surface also does not schedule the
  standalone credential retry loop.

The hostile regression
`app::tests::embedded_surface_waits_for_daemon_instead_of_revealing_legacy_state`
covers legacy catalog leakage and a stale worker `Started` update. The regression
`app::tests::daemon_transport_without_writer_does_not_queue_a_local_worker_command`
covers the transport fallback boundary.

## Explicit fallback that remains

The standalone `MusicApp::new_with_ctx` compatibility client still enables the
Airsonic worker. When it has no daemon snapshot or shell-owned writer,
`try_publish_transport_action` returns `false` so the existing local worker
transport remains usable; standalone library, album, search, and playback
commands likewise remain for compatibility. Removing that fallback requires a
separate standalone migration and is outside this bounded S3 audit. Embedded
shell Music does not start that worker.

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=ui-authority-r1 \
  ./install-helpers/xcp-build.sh cargo test -p mde-music-egui --lib -- --nocapture
result: 52 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=ui-authority-fmt-r1 \
  ./install-helpers/xcp-build.sh cargo fmt -p mde-music-egui -- --check
result: pass
```

No `mde-shell-egui`, daemon, media-core, Jellyfin, or active-worklist files were
changed by this slice.
