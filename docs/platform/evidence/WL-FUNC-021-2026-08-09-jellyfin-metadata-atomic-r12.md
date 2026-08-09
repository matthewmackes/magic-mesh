# WL-FUNC-021 Jellyfin metadata atomic fallback — r12

Date: 2026-08-09

Base revision: `f995404af114326bef808d66dd0e6693906e18b6`

## Production gap and correction

The reachable Media browse path calls `MetadataCache::store_snapshot` after a
successful mesh Jellyfin gateway response and restores that snapshot during a
later provider outage. Unlike the playable-byte cache, this metadata manifest
was written directly to its final path. A crash could therefore truncate the
only fallback, while an ordinary persistence failure left the new unpersisted
projection active in memory.

`crates/desktop/mde-jellyfin/src/cache.rs` now serializes the candidate snapshot
set to a synced temporary file, atomically renames it, syncs the parent
directory, and only then commits the candidate to live memory. Failure removes
the temporary file and retains the last complete in-memory and on-disk
projection. This remains display metadata only; it does not make unavailable
media playable or retain stream credentials.

Source SHA-256 after the change:

- `cache.rs`: `87206fa2b5f79d536210ee4e22a634930a3ad5c92a4b4e8857d39608e9428629`

## BigBoy verification

Host: XEN-BIGBOY build VM `172.20.0.130`

Focused hostile regression:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=func021-jellyfin-metadata-r12 \
install-helpers/xcp-build.sh cargo test -p mde-jellyfin \
  cache::tests::failed_metadata_snapshot_replacement_keeps_the_last_complete_projection \
  -- --exact --nocapture
```

Result: PASS — 1 passed, 0 failed, 90 unit tests filtered; all integration
targets ran with zero selected tests. The fixture forces manifest replacement
to fail, verifies that the active projection remains at the prior generation,
restores and reloads the last complete manifest, and finds no abandoned
temporary file.

Focused format gate:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=func021-jellyfin-metadata-fmt2-r12 \
install-helpers/xcp-build.sh cargo fmt -p mde-jellyfin -- --check
```

Result: PASS. Local scoped `git diff --check` also passed. No external Jellyfin
server was required because this correction is the post-response persistence
and outage-fallback boundary; no live-provider behavior is claimed.
