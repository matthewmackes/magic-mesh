# WL-FUNC-021 — media-egui real-mpv feature gate (2026-08-07)

## Verification

Farm host `.90`, slot `media-egui-mpv-continue-r2`:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=media-egui-mpv-continue-r2 \
  install-helpers/xcp-build.sh cargo test -p mde-media-egui \
  --features mpv --locked -- --nocapture
result: 107 passed, 0 failed; 0 ignored; doc-tests 0 passed
```

The lane includes the real `mpv` feature and covers the Jellyfin source
projection, playback, cache, cast, capture, queue, and rendered UI seams. The
ambiguous-identity regression was corrected by isolating the safe fixture's
upstream identity; the fail-closed projector continues to reject duplicate
identity groups and unsafe endpoints.

## Scope

This is farm verification only. Live provider-loss recovery, physical renderer
acceptance, cross-seat owner-yield/resume, and second-seat package proof remain
open under `WL-FUNC-021`.
