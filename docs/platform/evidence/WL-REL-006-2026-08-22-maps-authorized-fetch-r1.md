# WL-REL-006 Maps authorized source fetch — r1

Date: 2026-08-22 UTC  
Classification: operator-authorized source fetch evidence; **not** production Maps admission  
Source revision: `5de12c56b`  
`production_admitted: false`

## Act

Operator lock 2026-08-22 authorized a Geofabrik New York PBF fetch plus official
TIGER Erie `36029` / Niagara `36063` geometry, with local render only. This
record is the fetch receipt. It is not a production Maps MBTiles receipt and
does not close the Maps gate.

- Host: BigBoy `172.20.0.130` (`mcnf-build-52`)
- Slot: `1` (exclusive; `MCNF_BUILD_SLOT=1` → `~/magic-mesh-farm-1`)
- Dest-root: `/home/mm/mcnf-maps-sources` (real directory, not a symlink;
  inode `103748150`, mode `0755`)
- Sync: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=1
  ./install-helpers/xcp-build.sh sync` from worktree `5de12c56b` — admitted
  (`104392272` KiB free; required `11534336` KiB)
- Public OSM tile CDNs were not fetched. Locked URLs only:
  - `https://download.geofabrik.de/north-america/us/new-york-latest.osm.pbf`
  - `https://www2.census.gov/geo/tiger/TIGER2024/COUNTY/tl_2024_us_county.zip`
- Bytes stay on BigBoy. The PBF and zip were not copied into Git.

## Dest-root sidecars

Both objects were published no-replace, mode `0400`, singly-linked regular
files. Sidecar kind is `mcnf-maps-authorized-source-fetch`. Every sidecar
records `production_admitted: false`.

| source_id | destination | bytes | sha256 | sidecar |
|---|---|---|---|---|
| pbf | `new-york-latest.osm.pbf` | 495288424 | `8d7b60bff5d5fafc16d39f4a17f87c9f11014f56b1f4191c4ec64fb43684fd64` | `new-york-latest.osm.pbf.sha256.json` |
| geometry | `tl_2024_us_county.zip` | 83913260 | `04e668d3502757c837c13444730547cd967f28a2c49aeffb873d1792ab2cb97b` | `tl_2024_us_county.zip.sha256.json` |

PBF sidecar fields: `kind=mcnf-maps-authorized-source-fetch`,
`upstream=geofabrik`, `license=ODbL-1.0`,
`operator_authorization=2026-08-22-survey`, `region_id=buffalo-niagara`,
`production_admitted=false`.

Geometry sidecar fields: `kind=mcnf-maps-authorized-source-fetch`,
`upstream=census-tiger`, `license=ODbL-1.0`,
`operator_authorization=2026-08-22-survey`, `region_id=buffalo-niagara`,
`production_admitted=false`.

## Local render

Both sources landed, so local render was invoked on the same dest-root:

```text
python3 install-helpers/maps-render-local-mbtiles.py \
  --source-root /home/mm/mcnf-maps-sources \
  --pbf new-york-latest.osm.pbf \
  --geometry tl_2024_us_county.zip \
  --dest-root /home/mm/mcnf-maps-sources \
  --destination buffalo-niagara.mbtiles \
  --sidecar buffalo-niagara.mbtiles.sha256.json
```

Honest refusal (exit 2):

```text
maps-render-local-mbtiles: refusal: geometry clip is not Erie 36029 / Niagara 36063
```

The official TIGER zip is the national county archive. The default clip
extractor does not select Erie `36029` / Niagara `36063` from those zip bytes,
so no `buffalo-niagara.mbtiles` was written. No fixture MBTiles was substituted.
`production_admitted` remains false.

## Leftover / blocker

Leftover is production `buffalo-niagara.mbtiles` admission and the operator
dest `/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles`. Required
next: clip Erie/Niagara from the already-fetched TIGER zip, render locally,
and admit the production dest. This fetch does not satisfy preflight Maps
admission.

Local no-network helper test
`python3 install-helpers/test-maps-fetch-authorized-sources.py` passed.
`own_nebula_ip` was out of scope.
