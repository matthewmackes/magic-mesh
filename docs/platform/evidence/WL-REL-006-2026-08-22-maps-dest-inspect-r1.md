# WL-REL-006 Maps leftover: production inspect of the dest-root dest — r1

Date: 2026-08-22 UTC  
Classification: BigBoy canonical dest inspect of dest-root OSM-derived raster; **not** production Maps admission  
Source revision: `37fd8fef4`  
Worktree: `/tmp/mcnf-drain-worktrees/cursor-WL-REL-006-qu0013in`  
Unit: `qu0013in`  
`production_admitted: false`

## Act

Canonical dest already held dest-root OSM-derived raster
`buffalo-niagara.mbtiles`. This helper never fetches. It runs
`inspect_mbtiles` from `maps-verify-mbtiles.py` on an absolute dest that
is exactly `.../buffalo-niagara/buffalo-niagara.mbtiles` (real file, no
symlink). Default quota `65536` refuses dest `167936` B. This inspect
used quota `262144` (`DEST_ADMIT_QUOTA_BYTES`). Sidecar kind is
`mcnf-maps-dest-inspect`, not `mcnf-maps-mbtiles-receipt`. Sidecar
publication is no-replace beside dest as
`buffalo-niagara.mbtiles.inspect.json`. Dest-install sidecar
`buffalo-niagara.mbtiles.sha256.json` was not overwritten.
`bind_receipt` was not edited and still writes `production_admitted:
false`. `verify_receipt` still refuses any receipt that is not
`production_admitted: false`.

- Host: BigBoy `172.20.0.130` (`mcnf-build-52`)
- Slot: `1` (exclusive; `MCNF_BUILD_SLOT=1` → `~/magic-mesh-farm-1`)
- Dest: `/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles`
- Dest-root: `/home/mm/mcnf-maps-sources`
- Sync: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=1
  ./install-helpers/xcp-build.sh sync` from worktree `37fd8fef4` —
  admitted (`103608580` KiB free; required `8388608` KiB)
- Public OSM tile CDNs were not fetched. PBF/zip/raster were not
  re-fetched.
- Bytes stay on BigBoy. PBF, zip, GeoJSON, and MBTiles were not copied
  into Git.
- Dest-root fixture `buffalo-niagara.mbtiles` was not overwritten.
- Dest bytes and dest-root raster were not overwritten (no-replace).

## Local / farm tests

Local no-network helper test:

```text
python3 install-helpers/test-maps-inspect-dest-mbtiles.py
maps dest-inspect mbtiles hostile suite passed
```

Fixture digest/size refuses. Default quota `65536` refuses a `167936` B
dest. Happy path writes inspect sidecar with `production_admitted:
false` and kind `mcnf-maps-dest-inspect`. Dest filename other than
`buffalo-niagara.mbtiles`, path escape, tile-CDN prefix, symlink dest,
dest-install sidecar name, and verify inspect refusals refuse.
Sidecar no-replace. Mode `0400`. No network.

Same suite on BigBoy slot 1 after sync: passed. Python `3.13.13`. No
network.

## Canonical dest inspect

```text
sudo -n python3 ~/magic-mesh-farm-1/install-helpers/maps-inspect-dest-mbtiles.py \
  --destination /var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles \
  --quota-bytes 262144
```

Independent `sha256sum` of the dest still matches dest-root raster
`6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895`.
Inspect admitted PNG tiles, provider/attribution/license, official
TIGER envelope, mode `0400`, and quota `262144`. Sidecar keeps
`production_admitted: false`. Kind is not `mcnf-maps-mbtiles-receipt`.

| object | bytes | sha256 | mode | note |
|---|---|---|---|---|
| dest-root `buffalo-niagara.pbf-raster.mbtiles` | 167936 | `6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895` | 0400 | source; unchanged |
| dest-root `buffalo-niagara.mbtiles` | 12288 | `dd7cde7e116cb52f114fc1c886fec32618bdfcb8c82a16e3e45dae601c87046e` | 0400 | fixture; untouched |
| `/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles` | 167936 | `6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895` | 0400 | BigBoy dest only; not rewritten |
| dest-install sidecar `.mbtiles.sha256.json` | 544 | `2118c12d2a5844b955847a1f2af9e02e9bb85d34e083d1dcd61a8d26c3adc4e7` | 0400 | unchanged |
| dest-inspect sidecar `.mbtiles.inspect.json` | 610 | `c7a3a82cc4e4fc98ac3477a5ff3648515c3eafa9cce3799f8e1cc1c9ce20ce19` | 0400 | beside dest |

Inspect sidecar fields: `kind=mcnf-maps-dest-inspect`,
`destination=/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles`,
`provider=openstreetmap-derived`, `license=ODbL-1.0`,
`region_id=buffalo-niagara`, `attribution=© OpenStreetMap contributors`,
`mbtiles_bytes=167936`,
`mbtiles_sha256=6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895`,
`tile_bytes=128928`, `tile_count=30`, `min_zoom=8`, `max_zoom=10`,
`bounds={west:-79.312136,south:42.437997,east:-78.460416,north:43.634799}`,
`quota_bytes=262144`, `production_admitted=false`. Kind is not
`mcnf-maps-mbtiles-receipt`.

Optional dest-root raster cross-check via `inspect_mbtiles` (quota
`262144`) matched dest digest, tile count, bounds, provider, and
license. Dest-root raster filename is not a dest inspect target.

This is not Dell / Seat 15 / Surface admission and does not flip
`production_admitted`. `verify_receipt` still refuses any receipt that
is not `production_admitted: false`. This dest inspect does not satisfy
preflight Maps admission.

## Leftover / blocker

Leftover is candidate-bound production receipt / `production_admitted`
(`bind_receipt` still false) and live-seat dest (WL-TEST-002). Dest-root
raster, clipped PBF, fixture PNG raster, dest-install copy, and dest
inspect sidecar are not production admission. The BigBoy dest path is
not a live-seat dest. This does not close the Maps gate and does not
mark `production_admitted`.

PBF, zip, GeoJSON, clipped PBF, dest-root raster, dest-root fixture, and
the BigBoy install dest stay on BigBoy.
