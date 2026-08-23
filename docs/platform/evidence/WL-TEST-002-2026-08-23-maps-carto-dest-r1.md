# Carto-styled Maps dest (buildings, landuse, labels) — r1

Date: 2026-08-23  
Classification: live-seat dest replace; **not** freeze, publication, or
`production_admitted: true`  
`production_admitted: false`

Operator on Dell: previous dest looked basic. That dest was a Pillow
highway/water/park sketch. Gazetteer is search-only and is not painted
on the basemap. Public OSM Carto tiles stay forbidden.

## Dest

Same local Erie/Niagara PBF. Helper now also exports buildings, residential
/ commercial / industrial landuse, railway, and place nodes, and burns
DejaVu labels onto the tiles. Dest-root fixture, z8–z10 line-raster, and
the earlier styled-raster were not overwritten.

`/home/mm/mcnf-maps-sources/buffalo-niagara.carto-raster.mbtiles`

- sha256 `7afcab0ab3ccc53cc5885ae9e9b796f1db45ff6043233497cac56ae26f7ec3a3`
- 6594560 B, PNG, z8–z13, 1059 tiles
- sidecar kind `mcnf-maps-styled-raster`, `production_admitted: false`
- Buffalo z13 tile: buildings, residential fabric, railways, and labels
  present (940 unique colors vs 11 on the prior styled dest)

Installed as `/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles`
after red `AI-GENERATED-ALERT` + 5s. Gazetteer unchanged.

Independent reread: Dell, Seat 15, Surface, Eagle, T480 all `7afcab0a…`.
This dest does not satisfy preflight Maps admission.
