# WL-REL-007 / WL-REL-006 receipt rebind at `54ee58acf` — 2026-08-30

Classification: dest rebind of already-selected bytes. Not a preflight
pass. Not freeze. Not `production_admitted`. No dest invented. Surface
`bootc_base` stays null.

Tree: `54ee58acf` epoch `1788132485`. Farm cargo units are already fresh
at this revision; this increment is dest-operator leftover, not a filler
workspace grind.

## Rebound (same dest bytes, new revision)

| Dest | Private path | Notes |
|---|---|---|
| RPM identity | `/root/mcnf-private/rpm-signing-identity-54ee58acf.json` | Produced on BigBoy; inspect matched fingerprint `06B1C27EA0E08A225155EB3314018AA1497DDC7C` |
| App catalog | `/root/mcnf-private/app-catalog-receipt-54ee58acf.json` | Same LibreOffice pin `ff822a56…`; catalog sha `de950226…` |
| App VM base | `/root/mcnf-private/app-vm-base-digest-54ee58acf.json` | Digest-only rebind of selected `e78cd1a6…` / platform `63773f45…` |

Kiron `verify-package.sh --source --expected-source-revision 54ee58acf`
passed.

## Refused (not invented)

| Leftover | Probe |
|---|---|
| bootc dest `3a5e74e6…` | `quay.io/fedora/fedora-bootc@sha256:3a5e74e6…` → manifest unknown. Live `:44` tag now resolves `e8f93cc9…`. That new digest was not kept. Existing dest `/root/mcnf-private/bootc-all-roles-digest.json` stays bound to `479ec2b8c`. |
| Maps S7 approval | Dest sidecar `/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles.approval.json` is an MBTiles dest-approval bound to `ab4a9d554`, not the offline-catalog `{regions,tiles}` object `produce-offline-catalog.py` requires. Restored source-root is PBF+TIGER, not approved tile files. Geofabrik latest moved. MBTiles dest `6d01a543…` was not replaced. |
| Surface `bootc_base` | `packaging/surface/surface-stack.f44.json` remains null |

S7 private argv was not written. `REPLACE_*` still refuses a complete
object. Do not grind `cargo test --workspace`.
