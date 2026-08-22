# WL-REL-006 Maps PBF clip — r1

Date: 2026-08-22 UTC  
Classification: official-county PBF clip evidence; **not** production Maps admission  
Source revision: `e169158f1`  
Worktree: `/tmp/mcnf-drain-worktrees/cursor-WL-REL-006-qu0009pb`  
`production_admitted: false`

## Act

GeoJSON extract already wrote official Erie `36029` / Niagara `36063`
polygons and recorded bbox
`[-79.312136,42.437997,-78.460416,43.634799]`. This helper reads that
bbox from the GeoJSON / its sidecar and invokes a fixed osmium argv
list only (never a shell string):

`osmium extract --strategy=smart --bbox=W,S,E,N --overwrite -o DEST SRC`

- Host: BigBoy `172.20.0.130` (`mcnf-build-52`)
- Slot: `3` (exclusive; `MCNF_BUILD_SLOT=3` → `~/magic-mesh-farm-3`)
- Dest-root: `/home/mm/mcnf-maps-sources` (real directory, inode
  `103748150`, mode `0755`, not a symlink)
- Sync: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=3
  ./install-helpers/xcp-build.sh sync` from worktree `e169158f1` —
  admitted (`103646940` KiB free; required `8388608` KiB)
- Public OSM tile CDNs were not fetched. PBF/zip were not re-fetched.
- Bytes stay on BigBoy. PBF, zip, GeoJSON, and MBTiles were not copied
  into Git.
- Existing fixture MBTiles `buffalo-niagara.mbtiles` was not overwritten.
- Existing GeoJSON `erie-niagara.geojson` was not overwritten.

## dnf / osmium

`osmium` was absent on BigBoy. Installed with
`sudo -n dnf install -y osmium-tool`. Mapnik was not installed.
Pillow/GDAL were not required and were not installed.

| package | NEVRA | notes |
|---|---|---|
| `osmium-tool` | `osmium-tool-1.18.0-1.fc42.x86_64` | `/usr/bin/osmium` 1.18.0; libosmium 2.22.0; RPM header sha256 `4e182cc96a5fe2fa9ad474116a5ab37ca826be310d3b337cc1c8617f23fbdf71` |
| `boost-program-options` | `boost-program-options-1.83.0-12.fc42.x86_64` | dependency |
| `mapnik` | absent | not installed |
| `python3-mapnik` | absent | not installed |
| `python3-pillow` | absent | not installed |
| `gdal` | absent | not installed |

## Local / farm tests

Local no-network helper test:

```text
python3 install-helpers/test-maps-extract-pbf-clip.py
maps extract pbf clip hostile suite passed
```

Injected osmium records the exact argv
`["osmium","extract","--strategy=smart","--bbox=W,S,E,N","--overwrite","-o",DEST,SRC]`
and writes dummy bytes. Missing binary refuses. Dest exists refuses.
Bbox is read from fixture GeoJSON. Sidecar kind is `mcnf-maps-pbf-clip`,
not `mcnf-maps-mbtiles-receipt`. `production_admitted` is false. Mode
`0400`. No network.

Same suite on BigBoy slot 3 after sync: passed. Python `3.13.13`. No
network.

## Dest-root extract

Extract against the existing NY PBF and GeoJSON (no re-fetch):

```text
python3 install-helpers/maps-extract-pbf-clip.py \
  --source-root /home/mm/mcnf-maps-sources \
  --pbf new-york-latest.osm.pbf \
  --geometry erie-niagara.geojson \
  --geometry-sidecar erie-niagara.geojson.sha256.json \
  --dest-root /home/mm/mcnf-maps-sources \
  --destination erie-niagara.osm.pbf \
  --sidecar erie-niagara.osm.pbf.sha256.json
```

Started `2026-08-22T22:18:50Z`, finished `2026-08-22T22:18:58Z`.
Osmium wrote clipped PBF `erie-niagara.osm.pbf`. Sidecar keeps
`production_admitted: false`. Kind is not `mcnf-maps-mbtiles-receipt`.

Official county bbox west `-79.312136` is west of
`maps-verify-mbtiles.py` `BOUNDS_ENVELOPE` west `-79.30`. Sidecar
records `bounds_envelope_compatible: false`. The clip used the official
county bbox and was not shrunk to cheat MBTiles admission.

| object | bytes | sha256 | mode |
|---|---|---|---|
| `new-york-latest.osm.pbf` | 495288424 | `8d7b60bff5d5fafc16d39f4a17f87c9f11014f56b1f4191c4ec64fb43684fd64` | 0400 |
| `tl_2024_us_county.zip` | 83913260 | `04e668d3502757c837c13444730547cd967f28a2c49aeffb873d1792ab2cb97b` | 0400 |
| `erie-niagara.geojson` | 150805 | `aa994c9a83cf355a6550716884f16f41e7c7a5c43741c1b360c6efe8943ac1f1` | 0400 |
| `erie-niagara.geojson.sha256.json` | 702 | `1e5f8e01323e793dcfa29d4ea1eedfb8ca72c9add8083381240e322c43365e49` | 0400 |
| `erie-niagara.osm.pbf` | 34073493 | `c5fd765d68e0051b7a4fb4ae896653bf0a427495497ff13294cb37d4716d481c` | 0400 |
| `erie-niagara.osm.pbf.sha256.json` | 904 | `a635544333a54380508fd3894d9d53b6e637b07eeb781ca13e42cebb57f5a5ae` | 0400 |
| `buffalo-niagara.mbtiles` | 12288 | `dd7cde7e116cb52f114fc1c886fec32618bdfcb8c82a16e3e45dae601c87046e` | 0400 |

Sidecar fields: `kind=mcnf-maps-pbf-clip`,
`clip_geoids=["36029","36063"]`, `clip_names=["Erie County","Niagara County"]`,
`bbox=[-79.312136,42.437997,-78.460416,43.634799]`,
`bounds_envelope_compatible=false`, `provider=openstreetmap-derived`,
`license=ODbL-1.0`, `production_admitted=false`. Kind is not
`mcnf-maps-mbtiles-receipt`.

The clipped PBF is an official-county OSM extract for a later production
rasterizer. It does not satisfy preflight Maps admission. The 12288-byte
fixture PNG raster remains untouched and is not a production Maps object.

## Leftover / blocker

Leftover is still a production **raster** renderer and dest
`/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles` (absent on
BigBoy). Fixture PNG raster is not production admission. Extracted
GeoJSON is not production admission. Clipped PBF is not MBTiles
admission. This extract does not close the Maps gate and does not mark
`production_admitted`.

PBF, zip, GeoJSON, clipped PBF, and MBTiles stay on BigBoy dest-root.
