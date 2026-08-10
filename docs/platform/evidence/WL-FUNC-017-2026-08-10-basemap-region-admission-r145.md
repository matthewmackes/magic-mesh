# WL-FUNC-017 — offline basemap region admission (r145)

Date: 2026-08-10

Source revision: `89f6ec75`

## Result

Offline map discovery now fails closed when an authoritative root contains
multiple region directories, multiple MBTiles candidates, symlinked or
non-regular database entries, unreadable candidates, or unsafe region/database
slugs. A valid single region remains admitted, and no-data roots remain an
honest unavailable state. The admitted identity is validated with the same
bounded `RegionId` rules used by the offline catalog.

## Focused farm proof

Machine 193 build VM `.90`, slot `func017-region-admission-r144`:

```text
cargo test -p mde-maps-location-egui \
  'basemap::tests::region_admission_' -- --nocapture
```

Result: 6 passed, 0 failed, 303 filtered. Focused rustfmt for
`basemap.rs` passed. The crate-wide format check reported only an existing
`offline_cache.rs` hunk outside this slice. No physical seat was used.

## Remaining boundary

Production map payload installation, catalog-to-bundle binding, navigation
routing, renderer integration, packaging, and installed-seat live proof remain.

