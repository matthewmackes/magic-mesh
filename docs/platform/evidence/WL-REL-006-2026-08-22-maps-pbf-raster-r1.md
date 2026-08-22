# WL-REL-006 Maps PBF raster — r1

Date: 2026-08-22 UTC  
Classification: dest-root OSM-derived raster MBTiles; **not** production Maps admission  
Source revision: `9dd919f87`  
Worktree: `/tmp/mcnf-drain-worktrees/cursor-WL-REL-006-qu0010rs`  
`production_admitted: false`

## Act

Clipped PBF `erie-niagara.osm.pbf` and official Erie `36029` / Niagara
`36063` GeoJSON were already on dest-root. This helper never fetches.
It invokes a fixed osmium argv list only (never a shell string):

`osmium export --geometry-types=linestring --output-format=geojsonseq --overwrite -o DEST SRC`

Stdlib + Pillow rasterized those ways into TMS PNG tiles over the
official clip bbox at z8–z10 (30 tiles). Output leaf is
`buffalo-niagara.pbf-raster.mbtiles`. The 12 KiB fixture
`buffalo-niagara.mbtiles` was not replaced.

- Host: BigBoy `172.20.0.130` (`mcnf-build-52`)
- Slot: `1` (exclusive; `MCNF_BUILD_SLOT=1` → `~/magic-mesh-farm-1`)
- Dest-root: `/home/mm/mcnf-maps-sources` (real directory, inode
  `103748150`, mode `0755`, not a symlink)
- Sync: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=1
  ./install-helpers/xcp-build.sh sync` from worktree `9dd919f87` —
  admitted (`103612348` KiB free; required `8388608` KiB)
- Public OSM tile CDNs were not fetched. PBF/zip were not re-fetched.
- Bytes stay on BigBoy. PBF, zip, GeoJSON, and MBTiles were not copied
  into Git.
- Existing fixture MBTiles `buffalo-niagara.mbtiles` was not overwritten.
- Existing clipped PBF `erie-niagara.osm.pbf` was not overwritten.

## dnf / pillow

`python3-pillow` was absent on BigBoy. Installed with
`sudo -n dnf install -y python3-pillow`. Mapnik was not installed.
PostGIS/osm2pgsql were not installed.

| package | NEVRA | notes |
|---|---|---|
| `python3-pillow` | `python3-pillow-11.1.0-3.fc42.x86_64` | Pillow 11.1.0; RPM header sha256 `2b7e0707a61eb9a237396af47457c085657c7fa57bf0d4236bd87450b7f14917` |
| `osmium-tool` | `osmium-tool-1.18.0-1.fc42.x86_64` | already present; `/usr/bin/osmium` 1.18.0 |
| `mapnik` | absent | not installed |
| `python3-mapnik` | absent | not installed |
| `postgis` | absent | not installed |
| `osm2pgsql` | absent | not installed |

## Local / farm tests

Local no-network helper test:

```text
python3 install-helpers/test-maps-raster-pbf-mbtiles.py
maps raster pbf mbtiles hostile suite passed
```

Injected osmium records the exact argv
`["osmium","export","--geometry-types=linestring","--output-format=geojsonseq","--overwrite","-o",DEST,SRC]`
and writes dummy GeoJSON. Injected raster returns fixture PNG tiles.
Missing binary refuses. Dest exists refuses. Destination
`buffalo-niagara.mbtiles` refuses (fixture no-replace). Sidecar kind is
`mcnf-maps-pbf-raster`, not `mcnf-maps-mbtiles-receipt`.
`production_admitted` is false. Mode `0400`. No network.

Same suite on BigBoy slot 1 after sync: passed. Python `3.13.13`. No
network.

## Dest-root raster

Raster against the existing clipped PBF and official GeoJSON (no
re-fetch):

```text
python3 install-helpers/maps-raster-pbf-mbtiles.py \
  --source-root /home/mm/mcnf-maps-sources \
  --pbf erie-niagara.osm.pbf \
  --geometry erie-niagara.geojson \
  --geometry-sidecar erie-niagara.geojson.sha256.json \
  --dest-root /home/mm/mcnf-maps-sources \
  --destination buffalo-niagara.pbf-raster.mbtiles \
  --sidecar buffalo-niagara.pbf-raster.mbtiles.sha256.json
```

Started `2026-08-22T22:24:45Z`, finished `2026-08-22T22:27:09Z`.
Osmium exported ways as GeoJSON text sequence. Pillow wrote 30 PNG
tiles (z8–z10). Sidecar keeps `production_admitted: false`. Kind is
not `mcnf-maps-mbtiles-receipt`.

Official county bbox west `-79.312136` is west of
`maps-verify-mbtiles.py` `BOUNDS_ENVELOPE` west `-79.30`. Sidecar
records `bounds_envelope_compatible: false`. Metadata bounds are the
official clip bbox and were not shrunk to cheat MBTiles admission.

| object | bytes | sha256 | mode |
|---|---|---|---|
| `new-york-latest.osm.pbf` | 495288424 | `8d7b60bff5d5fafc16d39f4a17f87c9f11014f56b1f4191c4ec64fb43684fd64` | 0400 |
| `tl_2024_us_county.zip` | 83913260 | `04e668d3502757c837c13444730547cd967f28a2c49aeffb873d1792ab2cb97b` | 0400 |
| `erie-niagara.geojson` | 150805 | `aa994c9a83cf355a6550716884f16f41e7c7a5c43741c1b360c6efe8943ac1f1` | 0400 |
| `erie-niagara.geojson.sha256.json` | 702 | `1e5f8e01323e793dcfa29d4ea1eedfb8ca72c9add8083381240e322c43365e49` | 0400 |
| `erie-niagara.osm.pbf` | 34073493 | `c5fd765d68e0051b7a4fb4ae896653bf0a427495497ff13294cb37d4716d481c` | 0400 |
| `erie-niagara.osm.pbf.sha256.json` | 904 | `a635544333a54380508fd3894d9d53b6e637b07eeb781ca13e42cebb57f5a5ae` | 0400 |
| `buffalo-niagara.mbtiles` | 12288 | `dd7cde7e116cb52f114fc1c886fec32618bdfcb8c82a16e3e45dae601c87046e` | 0400 |
| `buffalo-niagara.pbf-raster.mbtiles` | 167936 | `6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895` | 0400 |
| `buffalo-niagara.pbf-raster.mbtiles.sha256.json` | 1084 | `f5ed1baddd06588b50e48cb31862591f3990604b6e40a684c2cca0fb1efef13c` | 0400 |

Sidecar fields: `kind=mcnf-maps-pbf-raster`,
`clip_geoids=["36029","36063"]`, `clip_names=["Erie County","Niagara County"]`,
`bbox=[-79.312136,42.437997,-78.460416,43.634799]`,
`bounds_envelope_compatible=false`, `provider=openstreetmap-derived`,
`license=ODbL-1.0`, `format=png`, `min_zoom=8`, `max_zoom=10`,
`tile_count=30`, `production_admitted=false`. Kind is not
`mcnf-maps-mbtiles-receipt`.

MBTiles metadata: `name=buffalo-niagara`, `format=png`,
`provider=openstreetmap-derived`, `license=ODbL-1.0`, attribution
`© OpenStreetMap contributors`, bounds
`-79.312136,42.437997,-78.460416,43.634799`.

The dest-root raster is an OSM-derived PNG MBTiles object for a later
production install path. It does not satisfy preflight Maps admission.
`/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles` is absent
on BigBoy and is not claimed admitted. The 12288-byte fixture PNG
raster remains untouched and is not a production Maps object.

## Leftover / blocker

Leftover is production dest
`/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles` (absent on
BigBoy) and the verify-envelope / `production_admitted` gate. Official
bbox west `-79.312136` still escapes envelope west `-79.30`. Fixture
PNG raster is not production admission. Clipped PBF is not MBTiles
admission. Dest-root raster is not production admission. This raster
does not close the Maps gate and does not mark `production_admitted`.

PBF, zip, GeoJSON, clipped PBF, dest-root raster, and fixture MBTiles
stay on BigBoy dest-root.
