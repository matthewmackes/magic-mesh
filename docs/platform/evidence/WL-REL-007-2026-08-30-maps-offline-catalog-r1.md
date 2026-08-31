# WL-REL-007 / WL-REL-006 dest-derived Maps offline catalog — 2026-08-30

Classification: dest consumption of already-selected MBTiles tiles. Not a
preflight pass. Not freeze. Not `production_admitted`. No dest invented.
Surface `bootc_base` stays null.

Tree: `f8dce4e0c` epoch `1788139522`. Farm cargo units were already
fresh; this increment is dest-operator leftover, not a filler workspace
grind.

## Act

Extracted the 30 PNG tiles already inside BigBoy dest
`/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles`
(`6d01a543…`, 128928 tile bytes, TMS rows, zooms 8–10, official TIGER
bounds). Wrote them as a named tile source-root
`/home/mm/mcnf-maps-catalog-sources` (mode 0444 files). Wrote a private
offline-catalog approval from those dest hashes and produced
`/home/mm/mcnf-maps-offline-bundle-f8dce4e0c` via
`produce-offline-catalog.py`. Public OSM tiles were not fetched. Dest
MBTiles was not replaced.

| Dest | Path | Notes |
|---|---|---|
| Tile source-root | BigBoy `/home/mm/mcnf-maps-catalog-sources` | 30 dest PNGs; TMS `z/x/y` as stored |
| Approval | `/root/mcnf-private/maps-offline-approval-f8dce4e0c.json` | mode 0400; bound to `f8dce4e0c` / `1788139522`; quota 262144 |
| Bundle | BigBoy `/home/mm/mcnf-maps-offline-bundle-f8dce4e0c` | `payload_bytes` 128928; catalog sha `7ed46e8a…`; cache index sha `7d34ae81…` |

## Still leftover

| Leftover | Probe |
|---|---|
| `verify-offline-map-catalog` | Isolated crate; no dest binary on this host or farm `target/` |
| bootc dest `3a5e74e6…` | Still `manifest unknown` on quay; not cached |
| Surface `bootc_base` | `packaging/surface/surface-stack.f44.json` remains null |

S7 argv was not written. Do not grind `cargo test --workspace`.
