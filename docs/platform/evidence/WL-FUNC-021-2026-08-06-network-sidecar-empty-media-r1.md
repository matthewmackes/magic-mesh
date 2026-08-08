# WL-FUNC-021 Jellyfin network-loss sidecar evidence — 2026-08-06

## Focused correction

`JellyfinClient::download` now rejects a successful HTTP response with an empty
body as `JellyfinError::EmptyMedia`. A zero-byte response is not a playable
media title; failing before the media controller calls `OfflineCache::store`
keeps an interrupted or invalid provider response from becoming an apparently
available offline item.

The correction is in
`crates/desktop/mde-jellyfin/src/client.rs`, with the hostile regression test
`download_rejects_an_empty_success_before_offline_cache_admission`. The
existing transport-error path remains fail-closed for connect/TLS/timeout/read
failures.

## Farm verification

All commands were run with an explicit farm host and isolated slot:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=network-sidecar-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-jellyfin
```

Result: passed — 89 unit tests, 12 browse integration tests, 9 playback
integration tests, and 1 doctest; 0 failed.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=network-sidecar-r1 \
  install-helpers/xcp-build.sh cargo fmt -p mde-jellyfin -- --check
```

Result: passed with no formatting diff.

## Proof boundary

This is fixture-backed fail-closed behavior, not live-provider evidence. It does
not claim a physical network drop, live Jellyfin recovery, media decode,
PipeWire/DRM output, package acceptance, or UI acceptance. Those gates remain
open until their respective live evidence exists.
