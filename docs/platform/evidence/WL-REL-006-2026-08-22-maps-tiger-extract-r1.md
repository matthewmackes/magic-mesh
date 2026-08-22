# WL-REL-006 Maps TIGER clip extract — r1

Date: 2026-08-22 UTC  
Classification: official-county clip extract evidence; **not** production Maps admission  
Source revision: `57db746db`  
Worktree: `/tmp/mcnf-drain-worktrees/cursor-WL-REL-006-qu0007ex`  
`production_admitted: false`

## Act

Clip-detect already admitted packed GEOID tokens `36029` / `36063` inside the
official TIGER zip. It did not extract county polygons. This helper opens that
zip with stdlib `zipfile` + `struct` only, reads the shapefile `.dbf` + `.shp`
(and `.shx` when present), selects records whose GEOID is exactly Erie
`36029` or Niagara `36063`, and writes a bounded GeoJSON FeatureCollection
with those two features only.

- Host: BigBoy `172.20.0.130` (`mcnf-build-52`)
- Slot: `1` (exclusive; `MCNF_BUILD_SLOT=1` → `~/magic-mesh-farm-1`)
- Dest-root: `/home/mm/mcnf-maps-sources` (real directory, inode `103748150`,
  mode `0755`, not a symlink)
- Sync: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=1
  ./install-helpers/xcp-build.sh sync` from worktree `57db746db` — admitted
  (`103729684` KiB free; required `8388608` KiB)
- Public OSM tile CDNs were not fetched. PBF/zip were not re-fetched.
- Bytes stay on BigBoy. PBF, zip, GeoJSON, and MBTiles were not copied into Git.
- Existing fixture MBTiles `buffalo-niagara.mbtiles` was not overwritten.

## Local / farm tests

Local no-network helper test:

```text
python3 install-helpers/test-maps-extract-tiger-clip.py
maps extract tiger clip hostile suite passed
```

Tiny shapefile zip with Erie `36029`, Niagara `36063`, and distractor `36001`
extracts exactly those two GEOIDs. Zip missing Niagara refuses. Symlink dest,
path substitution, existing dest, and public tile CDN URLs refuse. Sidecar
kind is `mcnf-maps-tiger-clip`, not `mcnf-maps-mbtiles-receipt`.
`production_admitted` is false. Mode `0400`. No network.

Same suite on BigBoy slot 1 after sync: passed. Python `3.13.13`. No network.

## Dest-root extract

Extract against the existing official zip (no re-fetch):

```text
python3 install-helpers/maps-extract-tiger-clip.py \
  --source-root /home/mm/mcnf-maps-sources \
  --geometry tl_2024_us_county.zip \
  --dest-root /home/mm/mcnf-maps-sources \
  --destination erie-niagara.geojson \
  --sidecar erie-niagara.geojson.sha256.json
```

Shapefile parse succeeded. Output is a bounded FeatureCollection with exactly
two features (Erie `36029`, Niagara `36063`). Sidecar keeps
`production_admitted: false`. Kind is not `mcnf-maps-mbtiles-receipt`.

| object | bytes | sha256 | mode |
|---|---|---|---|
| `new-york-latest.osm.pbf` | 495288424 | `8d7b60bff5d5fafc16d39f4a17f87c9f11014f56b1f4191c4ec64fb43684fd64` | 0400 |
| `tl_2024_us_county.zip` | 83913260 | `04e668d3502757c837c13444730547cd967f28a2c49aeffb873d1792ab2cb97b` | 0400 |
| `erie-niagara.geojson` | 150805 | `aa994c9a83cf355a6550716884f16f41e7c7a5c43741c1b360c6efe8943ac1f1` | 0400 |
| `erie-niagara.geojson.sha256.json` | 702 | `1e5f8e01323e793dcfa29d4ea1eedfb8ca72c9add8083381240e322c43365e49` | 0400 |
| `buffalo-niagara.mbtiles` | 12288 | `dd7cde7e116cb52f114fc1c886fec32618bdfcb8c82a16e3e45dae601c87046e` | 0400 |

Sidecar fields: `kind=mcnf-maps-tiger-clip`,
`clip_geoids=["36029","36063"]`, `clip_names=["Erie County","Niagara County"]`,
`feature_count=2`, `bbox=[-79.312136,42.437997,-78.460416,43.634799]`,
`provider=openstreetmap-derived`, `license=ODbL-1.0`,
`production_admitted=false`. Kind is not `mcnf-maps-mbtiles-receipt`.

The extracted clip is an official-county boundary artifact for a later
production renderer. It does not satisfy preflight Maps admission. The
12288-byte fixture PNG raster remains untouched and is not a production
Maps object.

## Leftover / blocker

Leftover is still production dest
`/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles` (absent on BigBoy)
and a production renderer. Fixture PNG raster is not production admission.
Extracted GeoJSON is not production admission. This extract does not close
the Maps gate and does not mark `production_admitted`.

PBF, zip, GeoJSON, and MBTiles stay on BigBoy dest-root.
