# WL-FUNC-017 — transactional offline-cache provisioning (r500)

Date: 2026-08-13

Branch: `agent/drain-worklist-20260725`

Source scope: `crates/desktop/mde-maps-location-egui/src/offline_cache.rs`

## Gap closed

`OfflineTileCache::store_verified` previously removed and durably de-indexed
expired, replacement, and LRU entries before the incoming tile payload and
replacement index were durable. A later filesystem or index-publication failure
could therefore destroy valid provisioned offline coverage even though the new
tile was rejected.

Provisioning now computes the complete bounded replacement index in memory,
writes the catalog-approved payload, atomically publishes that index once, and
only then removes payloads no longer referenced by the durable index. A failure
before publication leaves the prior in-memory and on-disk index and every
referenced payload intact. Expiry, replacement, deterministic LRU selection,
catalog digest provenance, and the configured quota remain part of the one
candidate transaction.

The hostile regression fills the cache to quota, causes the next tile to select
the existing tile for eviction, and then blocks the incoming tile with a
non-directory parent. It proves the previously verified tile remains available
both immediately and after reopening the cache.

## Farm evidence

- BigBoy `.130`, slot `func017-offline-atomic-test-r500`:
  `cargo test -p mde-maps-location-egui offline_cache::tests::failed_provisioning_preserves_tiles_selected_for_eviction -- --exact --nocapture`
  passed **1/1** (`318` filtered). An earlier short-name invocation selected
  zero tests and was explicitly rejected as evidence.
- `.170`, slot `func017-offline-atomic-clippy-r500`:
  `cargo clippy -p mde-maps-location-egui --all-targets -- -D warnings` passed.
- `.196`, slot `func017-offline-atomic-fmt-r500b`:
  `cargo fmt -p mde-maps-location-egui -- --check` passed after correcting the
  touched region and two pre-existing rustfmt drifts in the authorized file.
- `.90`, slot `func017-offline-cache-module-r500`:
  `cargo test -p mde-maps-location-egui offline_cache::tests:: -- --nocapture`
  passed **11/11** (`308` filtered).

## Remaining epic acceptance

This closes the provisioned cache transaction-loss gap in S5. WL-FUNC-017 still
requires the remaining Maps/weather integration and packaging gates. Installed
offline map/route, provider-loss, restart, sleep/rejoin, MG90, and weather proof
remains deferred and non-blocking until after the first full release under the
operator's current acceptance policy.
