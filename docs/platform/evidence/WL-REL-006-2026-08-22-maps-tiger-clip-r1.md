# WL-REL-006 Maps TIGER zip clip — r1

Date: 2026-08-22 UTC  
Classification: local-render clip evidence; **not** production Maps admission  
Source revision: `c3277f862`  
Worktree: `/tmp/mcnf-drain-worktrees/cursor-WL-REL-006-qu0006cl`  
`production_admitted: false`

## Act

The leftover from the 2026-08-22 authorized fetch was an honest clip refusal:
the official TIGER object is a compressed zip, and GEOID strings live inside
members (typically `.dbf`). `extract_clip_geoids` now opens that zip with
stdlib `zipfile` only, scans member bytes (`.dbf` first), and if both locked
GEOIDs are present returns exactly `["36029", "36063"]`. It does not invent
counties and does not return every 36xxx GEOID in the national archive.
Non-zip bytes keep the existing JSON / text paths.

Official TIGER `.dbf` rows pack STATEFP/COUNTYFP/COUNTYNS/GEOID as adjacent
ASCII digits (`360290097411336029050000`). A UTF-8 word-boundary scan misses
Erie/Niagara; the zip path therefore tests for the locked GEOID byte tokens
inside member bytes.

- Host: BigBoy `172.20.0.130` (`mcnf-build-52`)
- Slot: `2` (exclusive; `MCNF_BUILD_SLOT=2` → `~/magic-mesh-farm-2`)
- Dest-root: `/home/mm/mcnf-maps-sources` (real directory, not a symlink)
- Sync: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=2
  ./install-helpers/xcp-build.sh sync` from worktree `c3277f862` — admitted
  (`103729732` KiB free; required `8388608` KiB)
- Public OSM tile CDNs were not fetched. PBF/zip were not re-fetched.
- Bytes stay on BigBoy. PBF, zip, and MBTiles were not copied into Git.

## Local / farm tests

Local no-network helper test:

```text
python3 install-helpers/test-maps-render-local-mbtiles.py
maps local-render mbtiles hostile suite passed
```

Packed in-memory `.dbf` zip with Erie `36029`, Niagara `36063`, and distractor
`36001` admits exactly `["36029", "36063"]`. Zip missing Niagara refuses.
Word-boundary regex finds zero tokens in the packed `.dbf` member.

Same suite on BigBoy slot 2 after sync: passed. No network.

## Dest-root local render

Re-run against the existing dest-root (no re-fetch):

```text
python3 install-helpers/maps-render-local-mbtiles.py \
  --source-root /home/mm/mcnf-maps-sources \
  --pbf new-york-latest.osm.pbf \
  --geometry tl_2024_us_county.zip \
  --dest-root /home/mm/mcnf-maps-sources \
  --destination buffalo-niagara.mbtiles \
  --sidecar buffalo-niagara.mbtiles.sha256.json
```

Clip admitted Erie/Niagara from the official zip. The helper wrote a
contract-valid fixture PNG MBTiles. This helper is local-render, not the
production Maps gate. Sidecar keeps `production_admitted: false`.

| object | bytes | sha256 | mode |
|---|---|---|---|
| `new-york-latest.osm.pbf` | 495288424 | `8d7b60bff5d5fafc16d39f4a17f87c9f11014f56b1f4191c4ec64fb43684fd64` | 0400 |
| `tl_2024_us_county.zip` | 83913260 | `04e668d3502757c837c13444730547cd967f28a2c49aeffb873d1792ab2cb97b` | 0400 |
| `buffalo-niagara.mbtiles` | 12288 | `dd7cde7e116cb52f114fc1c886fec32618bdfcb8c82a16e3e45dae601c87046e` | 0400 |
| `buffalo-niagara.mbtiles.sha256.json` | 943 | (sidecar JSON) | 0400 |

Sidecar fields: `kind=mcnf-maps-local-render`, `clip_geoids=["36029","36063"]`,
`clip_names=["Erie County","Niagara County"]`, `format=png`, `tile_count=1`,
`provider=openstreetmap-derived`, `license=ODbL-1.0`,
`production_admitted=false`. Kind is not `mcnf-maps-mbtiles-receipt`.

The 12288-byte fixture PNG raster is not a production Maps object. It proves
clip + local-render publication against the authorized sources. It does not
satisfy preflight Maps admission.

## Leftover / blocker

Leftover is still production dest
`/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles` (absent on BigBoy)
and a production renderer. Fixture PNG raster is not production admission.
This clip/render does not close the Maps gate and does not mark
`production_admitted`.

PBF, zip, and MBTiles stay on BigBoy dest-root. `own_nebula_ip` / `voip_rtt.rs`
were out of scope.
