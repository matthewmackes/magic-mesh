# WL-REL-006 — Flathub LibreOffice is the chosen App catalog path — r1

Date: 2026-08-23  
Classification: open-source catalog choice recorded in inventory; **not**
`production_admitted`, freeze, or S7 preflight close  
Source dest-cut: `7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac` epoch
`1787450205`  
`production_admitted: false`

## Authority

Operator 2026-08-23: do not park open-source choices; take the best
open-source path. Earlier the same day: acquire REL-006 inputs from
open-source providers.

## Choice

The App VM curated catalog is dest-backed Flathub
`org.libreoffice.LibreOffice` (office guest; Construct host has no office
app). Fixture ids stay refused. Other Flatpak refs are not invented.

`produce-open-source-input-inventory.py` now loads

`/root/mcnf-private/app-catalog-curated.json`

and records `catalog_refs` / `catalog_sha256` when that dest exists.
`catalog_sha256=de95022649b2a444791cdca3c88211c8bb06b8ed1a3a64f44f8b0034e6dd3e37`.

Local: `python3 install-helpers/test-produce-open-source-input-inventory.py`
→ PASS.

## Still leftover (not open-source choices)

| Leftover | Why it stays fail-closed |
|---|---|
| Maps `production_admitted` | OSM/ODbL dest already selected; admit waits on freeze bind |
| RPM signer receipt | Governed secret, not an open-source fetch |
| Surface `bootc_base` | Blocked stack must not guess a digest |
| S7 Maps/RPM `REPLACE_*` | Cross-revision combine of dests still refused |

S7 template and `release-preflight.bootc-bound.json` were not overwritten.
