# WL-REL-006 — Flathub LibreOffice curated catalog dest — r1

Date: 2026-08-23  
Classification: acquired open-source catalog dest; **not**
`production_admitted`  
Source revision bound on the receipt: `7e3474eeb16cb8c4b8c9a378bfcd1f9c45f5e4ac`
epoch `1787450205`  
`production_admitted: false`

## Authority

Operator 2026-08-23: acquire REL-006 inputs from open-source providers.

## Acquisition

`acquire-flathub-curated-catalog.py` fetched the live ostree commit from

`https://dl.flathub.org/repo/refs/heads/app/org.libreoffice.LibreOffice/x86_64/stable`

and wrote dest catalog + sidecar. Fixture ids (`org.example.*`,
`org.mcnf.test.*`) are refused.

| Dest | Mode |
|---|---|
| `/root/mcnf-private/app-catalog-curated.json` | `0444` |
| `/root/mcnf-private/app-catalog-curated.sidecar.json` | `0400` |
| `/root/mcnf-private/app-catalog-receipt.json` | `0400` |

`catalog_sha256=de95022649b2a444791cdca3c88211c8bb06b8ed1a3a64f44f8b0034e6dd3e37`  
App id: `org.libreoffice.LibreOffice` (office guest; Construct host has
no office app). License on Flathub appstream: MPL-2.0 family.

## Still leftover

Maps `production_admitted` stays false (bind_receipt lock). RPM signer
secret is not an open-source fetch. Surface `bootc_base` stays null.
S7 `REPLACE_*` was not overwritten across dest-cut revisions.
