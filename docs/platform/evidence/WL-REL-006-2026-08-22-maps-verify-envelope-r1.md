# WL-REL-006 Maps verify envelope — r1

Date: 2026-08-22 UTC  
Classification: verifier envelope widening; **not** production Maps admission  
Source revision: `f582429f6`  
Worktree: `/tmp/mcnf-drain-worktrees/cursor-WL-REL-006-qu0011en`  
`production_admitted: false`

## Act

`maps-verify-mbtiles.py` `BOUNDS_ENVELOPE` was
`west -79.30, south 42.35, east -78.35, north 43.45`. Official Erie /
Niagara TIGER bbox
`[-79.312136, 42.437997, -78.460416, 43.634799]` escaped on west
(`-79.312136` < `-79.30`) and north (`43.634799` > `43.45`). Dest-root
raster sidecar already recorded `bounds_envelope_compatible: false`.
Official county geometry was not shrunk.

The envelope was widened just enough to contain that official clip plus
a small county-scale margin:

`west -79.35, south 42.35, east -78.35, north 43.70`

This is not the New York state envelope. `parse_bounds` now accepts
`-79.312136,42.437997,-78.460416,43.634799`. Hostile refusals remain
for reversed bounds, non-numeric metadata, public tile-CDN providers,
and bounds that still escape the widened envelope (west `-80`, north
`45`). `bind_receipt` still writes `production_admitted: false`.

No fetch. No dest-root rewrite. No MBTiles copied into Git.

## Local tests

No-network helper test:

```text
python3 install-helpers/test-maps-verify-mbtiles.py
maps verify mbtiles envelope suite passed
```

Also green:

```text
python3 install-helpers/test-maps-produce-mbtiles-receipt.py
maps mbtiles receipt hostile suite passed
```

Python `3.9` stdlib only. No network. Official bbox admits.
`production_admitted` stays false on the bound receipt.

## Dest-root (unchanged)

BigBoy `172.20.0.130` `/home/mm/mcnf-maps-sources` objects were not
rewritten. The dest-root raster sidecar still records the historical
`bounds_envelope_compatible: false` written against the old envelope.
That sidecar is not a `mcnf-maps-mbtiles-receipt` and is not
re-admitted here.

| object | bytes | sha256 | note |
|---|---|---|---|
| `buffalo-niagara.pbf-raster.mbtiles` | 167936 | `6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895` | dest-root raster; not production |
| `buffalo-niagara.mbtiles` | 12288 | `dd7cde7e116cb52f114fc1c886fec32618bdfcb8c82a16e3e45dae601c87046e` | fixture PNG raster; not production |

`/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles` remains the
production dest and is not claimed admitted.

## Leftover / blocker

Envelope leftover is closed in the verifier. Leftover is production dest
`/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles` and
`production_admitted` (`bind_receipt` still false). Dest-root raster,
clipped PBF, and fixture PNG raster are not production admission. This
does not close the Maps gate.
