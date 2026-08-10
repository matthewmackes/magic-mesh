# WL-FUNC-017 — offline basemap metadata admission (r141)

Date: 2026-08-10

Source revision: `7cefa1af`

## Result

The offline MBTiles reader now fails closed when recognized metadata is
malformed, duplicated, mistyped, non-finite, inverted, or outside the Web
Mercator domain. It also rejects centers outside declared bounds and zooms
above the supported tile pyramid.

Tile lookup now bounds `x` and `y` before coordinate conversion and uses a
checked zoom shift, preventing invalid requests from wrapping into a different
tile or overflowing the pyramid calculation.

## Focused farm proof

Machine 193 build VM `.90`:

```text
cargo test -p mde-maps-location-egui basemap::tests -- --nocapture
```

Result: 12 passed, 0 failed, 291 filtered. Machine 193 build VM `.170` also
passed focused rustfmt and `git diff --check`.

The package-wide clippy gate remains blocked by pre-existing unwrap findings
in `model.rs:3513` and `offline_catalog.rs:228`; neither finding is in this
basemap slice.

## Remaining boundary

This checkpoint validates metadata and coordinate admission only. Production
offline map data and its governed manifest, complete renderer integration,
Valhalla-backed offline routing, packaging, and live installed-seat proof
remain before WL-FUNC-017 can close.
