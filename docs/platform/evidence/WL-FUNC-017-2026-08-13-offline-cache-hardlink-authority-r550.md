# WL-FUNC-017 offline-cache hard-link authority — r550

Date: 2026-08-13

## Production result

The verified offline tile cache now refuses cache indexes and tile payloads
whose inode has more than one filesystem link. A second path can therefore no
longer retain mutation authority over bytes that Maps treats as verified
offline data.

On lookup, an unsafe cache link is quarantined and removed from the cache index
without following or deleting the externally owned link. Restart admission
also rejects a hard-linked index before deserialization. This is bounded local
provider behavior and introduces no map fixtures, network fallback, or inferred
live proof.

## Hostile regression

`offline_cache::tests::hard_linked_cache_authority_is_rejected_without_touching_external_data`
creates external hard links to an admitted tile and its durable index. It proves
that the tile loses authority, only the cache path is removed, external bytes
remain unchanged, and a hard-linked index fails closed.

## Farm gates

- `.170`, slot 1: `cargo test -p mde-maps-location-egui offline_cache::tests::hard_linked_cache_authority_is_rejected_without_touching_external_data -- --exact --nocapture` — passed; 1 passed, 0 failed, 323 filtered out.
- `.90`, slot 1: `cargo clippy -p mde-maps-location-egui --all-targets --all-features -- -D warnings` — passed.
- `.170`, slot 2: `cargo build -p mde-maps-location-egui --all-targets --all-features` — passed.
- `.90`, slot 2: `cargo fmt -p mde-maps-location-egui -- --check` — found one owned line-wrap; the exact Rustfmt delta was applied without rerunning.
- Scoped `git diff --check` — passed.

## Remaining WL-FUNC-017 acceptance

The first full release still must supply and install the governed offline
catalog, approved basemap payloads, gazetteer, and route data. Post-release,
non-blocking one-node acceptance still covers offline maps/routes, provider
loss and reconnect, restart/sleep/rejoin, MG90 recovery, and direct-DRM Maps
presentation. No live or external-data result is claimed by this slice.
