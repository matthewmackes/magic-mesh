# WL-REL-006 Maps leftover: production dest path — r1

Date: 2026-08-22 UTC  
Classification: BigBoy canonical dest copy of dest-root OSM-derived raster; **not** production Maps admission  
Source revision: `448bcd220`  
Worktree: `/tmp/mcnf-drain-worktrees/cursor-WL-REL-006-qu0012ds`  
Unit: `qu0012ds`  
`production_admitted: false`

## Act

Dest-root already held `buffalo-niagara.pbf-raster.mbtiles`. This helper
never fetches. It copies that relative leaf onto the canonical install
path as exactly `buffalo-niagara.mbtiles` under a real
`buffalo-niagara/` parent. Publication is no-replace, mode `0400`. The
known 12 KiB fixture digest/size is refused. Sidecar kind is
`mcnf-maps-dest-install`, not `mcnf-maps-mbtiles-receipt`.
`bind_receipt` was not edited and still writes `production_admitted:
false`.

- Host: BigBoy `172.20.0.130` (`mcnf-build-52`)
- Slot: `2` (exclusive; `MCNF_BUILD_SLOT=2` → `~/magic-mesh-farm-2`)
- Dest-root: `/home/mm/mcnf-maps-sources` (real directory, inode
  `103748150`, mode `0755`, not a symlink)
- Install parent: `/var/lib/mde/maps/buffalo-niagara` created with
  `sudo -n mkdir -p` (real directory, inode `5552805`, mode `0755`,
  not a symlink)
- Sync: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=2
  ./install-helpers/xcp-build.sh sync` from worktree `448bcd220` —
  admitted (`103608844` KiB free; required `8388608` KiB)
- Public OSM tile CDNs were not fetched. PBF/zip/raster were not
  re-fetched.
- Bytes stay on BigBoy. PBF, zip, GeoJSON, and MBTiles were not copied
  into Git.
- Dest-root fixture `buffalo-niagara.mbtiles` was not overwritten.

## Local / farm tests

Local no-network helper test:

```text
python3 install-helpers/test-maps-install-mbtiles-dest.py
maps dest-install mbtiles hostile suite passed
```

Fixture digest/size refuses. Dest-exists refuses (bytes unchanged).
Happy path writes dest + sidecar with `production_admitted: false` and
kind `mcnf-maps-dest-install`. Dest filename other than
`buffalo-niagara.mbtiles`, path escape, tile-CDN prefix, and symlink
dest refuse. Mode `0400`. No network.

Same suite on BigBoy slot 2 after sync: passed. Python `3.13.13`. No
network.

## Canonical dest install

```text
sudo -n mkdir -p /var/lib/mde/maps/buffalo-niagara
sudo -n python3 ~/magic-mesh-farm-2/install-helpers/maps-install-mbtiles-dest.py \
  --dest-root /home/mm/mcnf-maps-sources \
  --source buffalo-niagara.pbf-raster.mbtiles \
  --destination /var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles \
  --sidecar /var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles.sha256.json
```

Independent `sha256sum` of the installed file matches the dest-root
raster. Sidecar keeps `production_admitted: false`. Kind is not
`mcnf-maps-mbtiles-receipt`.

| object | bytes | sha256 | mode | note |
|---|---|---|---|---|
| dest-root `buffalo-niagara.pbf-raster.mbtiles` | 167936 | `6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895` | 0400 | source; unchanged |
| dest-root `buffalo-niagara.mbtiles` | 12288 | `dd7cde7e116cb52f114fc1c886fec32618bdfcb8c82a16e3e45dae601c87046e` | 0400 | fixture; untouched |
| `/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles` | 167936 | `6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895` | 0400 | BigBoy dest only |
| dest sidecar `.mbtiles.sha256.json` | 544 | `2118c12d2a5844b955847a1f2af9e02e9bb85d34e083d1dcd61a8d26c3adc4e7` | 0400 | beside dest |

Sidecar fields: `kind=mcnf-maps-dest-install`,
`source=buffalo-niagara.pbf-raster.mbtiles`,
`destination=/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles`,
`provider=openstreetmap-derived`, `license=ODbL-1.0`,
`region_id=buffalo-niagara`, `mbtiles_bytes=167936`,
`mbtiles_sha256=6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895`,
`production_admitted=false`. Kind is not `mcnf-maps-mbtiles-receipt`.

The canonical path now has the OSM-derived raster **on BigBoy only**.
This is not Dell / Seat 15 / Surface admission and does not flip
`production_admitted`. `verify_receipt` still refuses any receipt that
is not `production_admitted: false`. This dest copy does not satisfy
preflight Maps admission.

## Leftover / blocker

Leftover is candidate-bound production receipt / `production_admitted`
(`bind_receipt` still false) and live-seat dest (WL-TEST-002). Dest-root
raster, clipped PBF, and fixture PNG raster are not production
admission. The BigBoy dest path is not a live-seat dest. This does not
close the Maps gate and does not mark `production_admitted`.

PBF, zip, GeoJSON, clipped PBF, dest-root raster, dest-root fixture, and
the BigBoy install dest stay on BigBoy.
