# WL-FUNC-021 Jellyfin outage-boundary evidence — 2026-08-06

## Scope

The Jellyfin client and offline-cache boundary now fails closed across the
provider failure modes covered by the fixture. `JellyfinClient::download`
rejects non-2xx responses, transport failures, and successful empty media
responses. `OfflineCache::store` independently rejects zero-byte media so a
future caller cannot bypass that client guard. A previously complete offline
copy remains available through provider outage, while a cache file whose size
no longer matches its manifest is refused as unplayable.

## Focused fixture

`crates/desktop/mde-jellyfin/tests/outage.rs` drives the public client and
cache APIs with a bounded transport fixture:

- HTTP 503 with a partial-looking body is returned as `JellyfinError::Http`;
- a provider connection reset is returned as `JellyfinError::Transport`;
- HTTP 200 with no bytes is returned as `JellyfinError::EmptyMedia`;
- all three leave the known-good offline copy unchanged;
- a manually truncated cached file is rejected by `contains` and `local_path`;
- an empty cache replacement is rejected as `CacheError::EmptyMedia` without
  replacing the existing manifest row.

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=jellyfin-outage-r1 \
  ./install-helpers/xcp-build.sh cargo test -p mde-jellyfin -- --nocapture
```

Result: passed — 90 unit tests, 12 browse integration tests, 1 outage
integration test, 9 playback integration tests, and 1 doctest; 0 failed.

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=jellyfin-outage-fmt-r2 \
  ./install-helpers/xcp-build.sh cargo fmt -p mde-jellyfin -- --check
```

Result: passed with no formatting diff. Local `git diff --check` also passed.

## Changed files

- `crates/desktop/mde-jellyfin/src/cache.rs`
- `crates/desktop/mde-jellyfin/tests/outage.rs`
- `docs/platform/evidence/WL-FUNC-021-2026-08-06-jellyfin-outage-r1.md`

## Review hashes

- `crates/desktop/mde-jellyfin/src/cache.rs` — `ea79fb4a0f6619fdec2ced8063e573ff33eb291c741c1006df91b33be14b5d5d`
- `crates/desktop/mde-jellyfin/tests/outage.rs` — `7f4408ea644c38997fbd7abc45182a376ba880594587aec3ef589ec83ce0876e`

## Proof boundary

This is bounded fixture evidence, not live-provider acceptance. It does not
prove a physical Jellyfin network drop, server reconnect, expected-byte or
container-level validation for a non-empty but semantically damaged stream,
mpv/PipeWire/DRM output, package/RPM promotion, or Dell/seat-15 acceptance.
Those live provider, hardware, and release gates remain open.
