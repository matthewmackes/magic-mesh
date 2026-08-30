# WL-REL-007 / WL-REL-006 dest restore — Maps source-root + RPM identity

Classification: dest restore + non-secret identity receipt. Not a preflight
pass. Not freeze. Not `production_admitted`. No dest invented. Surface
`bootc_base` stays null.

Tree: `ba1726ffa` epoch `1788132281`. Operator dest-operator admission of
already-selected dests remains in force
(`WL-REL-007-2026-08-30-operator-admit-r1.md`).

## Maps source-root dest restored

Named dest `/home/mm/mcnf-maps-sources` was absent on BigBoy. Restored via
`maps-fetch-authorized-sources.py` against the 2026-08-22 locked URLs only
(no public OSM tiles). Sidecars keep `production_admitted: false`.

| Source | Destination | Bytes | sha256 |
|---|---|---|---|
| geometry | `tl_2024_us_county.zip` | 83913260 | `04e668d3502757c837c13444730547cd967f28a2c49aeffb873d1792ab2cb97b` |
| pbf | `new-york-latest.osm.pbf` | 495742600 | `4bb4b4f5472317e4c32fe8ceb0a92c6e17fc6ca99fda75726544ffbc5f7e5b8d` |

TIGER zip matches the 2026-08-22 fetch. Geofabrik `new-york-latest` moved
since that fetch (`8d7b60bf…` / 495288424). The dest-root is the locked
URL dest, not a new path. Existing Buffalo-Niagara MBTiles dest on BigBoy
was not replaced.

## RPM signing identity receipt

Control-host `gpg --list-secret-keys 06B1C27EA0E08A225155EB3314018AA1497DDC7C`
still has no secret. The governed secret is present on BigBoy (`mm@172.20.0.130`).
`produce-rpm-signing-identity-receipt.py produce` ran there against
`packaging/repo/RPM-GPG-KEY-magic-mesh`, then `inspect` matched.

Private non-secret dest (mode 0400, not in Git):
`/root/mcnf-private/rpm-signing-identity.json`

| Field | Value |
|---|---|
| kind | `mcnf-rpm-signing-identity` |
| primary_fingerprint | `06B1C27EA0E08A225155EB3314018AA1497DDC7C` |
| public_key_sha256 | `39c4f65d7c7a44a8ab64e234dfa9989d1fb3f335f7e5221f619679aeb59183c9` |
| source_revision | `ba1726ffae57a5413960d4081efeaae26177a38b` |
| release_epoch | `1788132281` |

## Still not written / not claimed

S7 private argv was not written. `REPLACE_*` still refuses. App VM / catalog /
bootc receipts remain bound to earlier dest-cut revisions. Surface
`bootc_base` stays null on the blocked stack. Do not invent dests. Do not
grind `cargo test --workspace`.
