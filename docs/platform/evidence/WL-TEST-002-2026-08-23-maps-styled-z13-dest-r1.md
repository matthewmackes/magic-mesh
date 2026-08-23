# Styled z8–z13 Maps dest on the five seats — r1

Date: 2026-08-23  
Classification: live-seat dest replace; **not** freeze, publication,
`production_admitted: true`, or FUNC-023 enroll  
`production_admitted: false`

Operator 2026-08-23 on Dell: one black-and-white map image. Construct
opens Maps at view zoom 13. The previous dest was the dest-root z8–z10
line sketch (`6d01a543…`, 30 tiles). The renderer clamped tile zoom to
10 and stretched that one tile ~8×.

## Dest

Local Erie/Niagara PBF + official TIGER GeoJSON on BigBoy dest-root
were rasterized with `maps-raster-styled-pbf-mbtiles.py` (osmium
`tags-filter` + `export` polygon/linestring, Pillow dark style). Public
OSM tile CDNs were not fetched. Dest-root fixture
`buffalo-niagara.mbtiles` and z8–z10 `buffalo-niagara.pbf-raster.mbtiles`
were not overwritten.

`/home/mm/mcnf-maps-sources/buffalo-niagara.styled-raster.mbtiles`

- sha256 `b27742e5bb438eb254341eed145005e155d5d293ec43d1787c230623e064f150`
- 4292608 B, PNG, z8–z13, 1059 tiles (4+8+18+60+209+760)
- sidecar kind `mcnf-maps-styled-raster`, `production_admitted: false`
- Buffalo z13 tile is dark-styled (motorway gold, streets gray, park
  green), not the previous pale-on-dark 1 px sketch

Installed as `/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles`
with `install-offline-map-region.sh --replace` after red
`AI-GENERATED-ALERT` + 5s. Gazetteer unchanged
(`d78ed522537302c7b6a520136b5a687fbafb68a6739a8c1e9aadccd7e54c169b`).

## Seats

Independent reread: all five have dest digest `b27742e5…`, metadata
`minzoom=8` `maxzoom=13`, 1059 tiles.

| Seat | Address | dest digest |
|---|---|---|
| Dell | `172.20.146.225` | `b27742e5…` |
| Seat 15 | `172.20.0.15` | `b27742e5…` |
| Surface | `172.20.146.79` | `b27742e5…` |
| Eagle | `172.20.146.88` | `b27742e5…` |
| T480 | `172.20.146.68` | `b27742e5…` |

## Renderer leftover

`basemap.rs` now clamps view zoom to the dest range so a future coarse
dest cannot 8×-stretch again. Farm: `MCNF_BUILD_HOST=172.20.0.50`
`MCNF_BUILD_SLOT=1` `xcp-build.sh cargo test -p mde-maps-location-egui
projection_clamps_view_zoom` — 1 passed. That clamp is **not** on the
installed 13.0.0-35 RPM. Today's seats see native z13 tiles because the
new dest max is 13, matching Construct's default viewport.

This dest does not satisfy preflight Maps admission.
