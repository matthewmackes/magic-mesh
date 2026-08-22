# WL-REL-006 Maps leftover: candidate-bound dest receipt — r1

Date: 2026-08-22 UTC  
Classification: BigBoy canonical dest candidate-bound receipt via
`bind_receipt` / `verify_receipt`; **not** production Maps admission  
Source revision: `ab4a9d554` (`ab4a9d5546fe05da65338ff4d3355e70e7e2231a`)  
Source epoch: `1787438581` (`git show -s --format=%ct`)  
Worktree: `/tmp/mcnf-drain-worktrees/cursor-WL-REL-006-qu0015rc`  
Unit: `qu0015rc`  
`production_admitted: false`

## Act

Canonical dest already held dest-root OSM-derived raster
`buffalo-niagara.mbtiles` and already inspected. This helper never
fetches. It binds an existing dest that is exactly
`.../buffalo-niagara/buffalo-niagara.mbtiles` (real file, no symlink)
through `verify.load_approval`, `verify.inspect_mbtiles`,
`verify.bind_receipt`, and `verify.verify_receipt`. Default quota
`65536` refuses dest `167936` B. This bind used quota `262144`
(`DEST_ADMIT_QUOTA_BYTES`). Receipt kind is
`mcnf-maps-mbtiles-receipt`. Receipt publication is no-replace beside
dest as `buffalo-niagara.mbtiles.receipt.json`. Dest-install sidecar
`buffalo-niagara.mbtiles.sha256.json` and dest-inspect sidecar
`buffalo-niagara.mbtiles.inspect.json` were not overwritten.
`bind_receipt` was not edited and still writes `production_admitted:
false`. `verify_receipt` still refuses any receipt that is not
`production_admitted: false`.

- Host: BigBoy `172.20.0.130` (`mcnf-build-52`)
- Slot: `0` (exclusive; `MCNF_BUILD_SLOT=0` → `~/magic-mesh-farm-0`)
- Dest: `/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles`
- Dest-root: `/home/mm/mcnf-maps-sources`
- Sync: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=0
  ./install-helpers/xcp-build.sh sync` from worktree `ab4a9d554` —
  admitted (`103650976` KiB free; required `8388608` KiB)
- Public OSM tile CDNs were not fetched. PBF/zip/raster were not
  re-fetched.
- Bytes stay on BigBoy. PBF, zip, GeoJSON, and MBTiles were not copied
  into Git.
- Dest-root fixture `buffalo-niagara.mbtiles` was not overwritten.
- Dest bytes, dest-root raster, dest-install sidecar, and dest-inspect
  sidecar were not overwritten (no-replace).

## Local / farm tests

Local no-network helper test:

```text
python3 install-helpers/test-maps-bind-dest-receipt.py
maps dest-receipt mbtiles hostile suite passed
```

Fixture digest/size refuses. Default quota `65536` refuses a `167936` B
dest. Happy path writes receipt sidecar with `production_admitted:
false` and kind `mcnf-maps-mbtiles-receipt`, then `verify_receipt`
passes. Dest filename other than `buffalo-niagara.mbtiles`, path
escape, tile-CDN prefix, symlink dest, dest-install sidecar name,
dest-inspect sidecar name, and a bind that would mark
`production_admitted` true refuse. Sidecar no-replace. Mode `0400`. No
network.

Same suite on BigBoy slot 0 after sync: passed. Python `3.13.13`. No
network.

## Canonical dest receipt

```text
sudo -n python3 ~/magic-mesh-farm-0/install-helpers/maps-bind-dest-receipt.py \
  --destination /var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles \
  --quota-bytes 262144 \
  --source-revision ab4a9d5546fe05da65338ff4d3355e70e7e2231a \
  --source-epoch 1787438581
```

Independent `sha256sum` of the dest still matches dest-root raster
`6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895`.
`verify_receipt` re-ran against dest + receipt and passed with
`production_admitted: false`. Approval sidecar binds HEAD
`ab4a9d5546fe05da65338ff4d3355e70e7e2231a` and epoch `1787438581`,
quota `262144`, region `buffalo-niagara`, install path
`/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles`, provider
`openstreetmap-derived`, license `ODbL-1.0`, attribution from inspect.

| object | bytes | sha256 | mode | note |
|---|---|---|---|---|
| dest-root `buffalo-niagara.pbf-raster.mbtiles` | 167936 | `6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895` | 0400 | source; unchanged |
| dest-root `buffalo-niagara.mbtiles` | 12288 | `dd7cde7e116cb52f114fc1c886fec32618bdfcb8c82a16e3e45dae601c87046e` | 0400 | fixture; untouched |
| `/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles` | 167936 | `6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895` | 0400 | BigBoy dest only; not rewritten |
| dest-install sidecar `.mbtiles.sha256.json` | 544 | `2118c12d2a5844b955847a1f2af9e02e9bb85d34e083d1dcd61a8d26c3adc4e7` | 0400 | unchanged |
| dest-inspect sidecar `.mbtiles.inspect.json` | 610 | `c7a3a82cc4e4fc98ac3477a5ff3648515c3eafa9cce3799f8e1cc1c9ce20ce19` | 0400 | unchanged |
| dest approval `.mbtiles.approval.json` | 332 | `c7111530296fdbf3e0f59b9a39144140701bcfae5aada3513cdc524971e878a1` | 0400 | beside dest; no-replace |
| dest receipt `.mbtiles.receipt.json` | 648 | `04fc5e984c8e08dbb7b2889897ad908180f362513701ed91b3d097e8ef48ed28` | 0400 | beside dest; no-replace |

Receipt fields: `kind=mcnf-maps-mbtiles-receipt`,
`install_path=/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles`,
`provider=openstreetmap-derived`, `license=ODbL-1.0`,
`region_id=buffalo-niagara`, `attribution=© OpenStreetMap contributors`,
`payload_bytes=167936`,
`mbtiles_sha256=6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895`,
`tile_bytes=128928`, `tile_count=30`, `min_zoom=8`, `max_zoom=10`,
`bounds={west:-79.312136,south:42.437997,east:-78.460416,north:43.634799}`,
`quota_bytes=262144`,
`source_revision=ab4a9d5546fe05da65338ff4d3355e70e7e2231a`,
`source_epoch=1787438581`, `production_admitted=false`.

Independent `maps-verify-mbtiles.py --receipt` against dest + receipt
reproduced the same object. `production_admitted` stayed false.

This is not Dell / Seat 15 / Surface admission and does not flip
`production_admitted`. `verify_receipt` still refuses any receipt that
is not `production_admitted: false`. This dest receipt does not satisfy
preflight Maps admission.

## Leftover / blocker

Leftover is `production_admitted` (needs the real candidate-bound
provider object / freeze) and live-seat dest (WL-TEST-002). Dest-root
raster, clipped PBF, fixture PNG raster, dest-install copy, dest
inspect sidecar, and this candidate-bound dest receipt are not
production admission. The BigBoy dest path is not a live-seat dest.
This does not close the Maps gate and does not mark
`production_admitted`.

PBF, zip, GeoJSON, clipped PBF, dest-root raster, dest-root fixture, and
the BigBoy install dest stay on BigBoy.
