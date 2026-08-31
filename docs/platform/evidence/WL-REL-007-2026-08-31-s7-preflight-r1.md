# WL-REL-007 / WL-REL-006 S7 preflight at `73828796f` — 2026-08-31

Classification: first-release input preflight pass at one clean pushed
revision. Not freeze. Not `production_admitted`. No dest invented.
Surface `bootc_base` stays null.

Tree: `73828796f` epoch `1788150284`. Farm cargo was already fresh.

## Rebind

Already-selected dest bytes were rebound to this revision:

| Dest | Path |
|---|---|
| bootc | `/root/mcnf-private/bootc-all-roles-digest-73828796f.json` — same `3a5e74e6…` dest; `rebind` did not call quay |
| App VM | `/root/mcnf-private/app-vm-base-digest-73828796f.json` — digest pin `e78cd1a6…` |
| App catalog | curated LibreOffice dest + receipt `…-73828796f.json` |
| RPM identity | BigBoy produce/inspect of `06B1C27EA0E08A225155EB3314018AA1497DDC7C` |
| Maps catalog | dest MBTiles tiles; bundle `/home/mm/mcnf-maps-offline-bundle-73828796f` |

`produce-bootc-digest-receipt.py rebind` copies dest identity onto a new
revision without registry inspect. Hostile self-test PASS.

## Preflight

Host: BigBoy `172.20.0.130` as `mm`,
`REPO_ROOT=/tmp/mcnf-rpm-receipt-repo`,
`MAGIC_MESH_SIGN_KEY=06B1C27EA0E08A225155EB3314018AA1497DDC7C`.

Private argv (mode 0400):
`/home/mm/mcnf-s7-inputs/release-preflight-73828796f.json`
and control-host copy
`/root/mcnf-private/release-preflight-73828796f.json`.

MBTiles dest `6d01a543…` was copied to a readable dest path for the
preflight uid; canonical dest
`/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles` was not
replaced.

```
release-input-preflight: PASS: all mandatory first-release inputs admitted for 73828796f783affa3232c0c9323b18006740d3f0
```

Final freeze still requires dest-cut reconfirmation after any later
source change. Do not grind `cargo test --workspace`.
