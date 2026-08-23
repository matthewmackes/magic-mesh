# Maps dest + Weather NWS on the five seats — r1

Date: 2026-08-23  
Classification: live-seat dest install and weather workers; **not** freeze,
publication, `production_admitted: true`, or FUNC-023 enroll  
`production_admitted: false`

Operator 2026-08-23: solve the empty Maps and Weather surfaces. Data
sources already discussed (Geofabrik NY clip + TIGER Erie/Niagara; keyless
NWS).

## Maps

BigBoy dest
`/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles`
sha256 `6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895`
(167936 B, z8–z10, 30 tiles) was copied with
`install-offline-map-region.sh` after red `AI-GENERATED-ALERT` + 5s on each
seat. The 12 KiB fixture digest was not used. Public OSM tile CDNs were
not fetched.

Gazetteer: 292 `place=city|town|village|hamlet|suburb` nodes from the
already-local `erie-niagara.osm.pbf` on BigBoy
(`/home/mm/mcnf-maps-sources`). sha256
`d78ed522537302c7b6a520136b5a687fbafb68a6739a8c1e9aadccd7e54c169b`
(81920 B). Installed beside the MBTiles as
`/var/lib/mde/maps/buffalo-niagara/gazetteer.sqlite`.

Independent reread: all five seats have both files at those digests.

## Weather

Official `mackesd-*.service` units still fail on missing
`/etc/mcnf/release-inputs/collaboration/collaboration-identity-receipt.json`.
That receipt was not invented. Weather workers were started with transient
`mcnf-wx-data` / `mcnf-wx-integrations` (`mackesd serve --group data|integrations`)
after alert + 5s. Manual location Buffalo
(`42.8864,-78.8784`, `America/New_York`, NWS US) was published on
`action/weather/set-location`. NWS current on every seat:
`fresh` · Mostly Cloudy · 17 °C (KBUF). Forecast also published.

These transient units do not survive reboot and do not replace FUNC-023
enroll or the collaboration-identity receipt.

## Seats

| Seat | Maps dest | Gazetteer | Weather location | NWS current |
|---|---|---|---|---|
| Dell | dest digest | gaz digest | Buffalo available | fresh 17 °C |
| Seat 15 | dest digest | gaz digest | Buffalo available | fresh 17 °C |
| Surface | dest digest | gaz digest | Buffalo available | fresh 17 °C |
| Eagle | dest digest | gaz digest | Buffalo available | fresh 17 °C |
| T480 | dest digest | gaz digest | Buffalo available | fresh 17 °C |
