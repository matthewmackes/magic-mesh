# WL-FUNC-021 transcode generation binding — 2026-08-11

- Scope: Jellyfin transcode URLs retain the selected media-source and play-session generations.
- Hostile boundary: duplicate or mismatched `MediaSourceId`/`PlaySessionId` values fail closed after restart.
- Focused gate: `cargo test -p mde-jellyfin playback::tests::stale_transcode_generation_cannot_relabel_media_source_or_play_session -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1, isolated.
- Result: **PASS**, 1 passed, 0 failed, 100 filtered out.
- Remaining boundary: restart live playback and reject a server response that substitutes either transcode generation.
