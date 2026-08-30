# Operator dest-operator admission — 2026-08-30

Classification: operator lock + dest inventory. Not a preflight pass.
No dest invented. No mesh-id or bearer invented. Surface `bootc_base`
stays null. Helper sidecars still refuse to self-mark
`production_admitted`.

Tree: `0118b40d1`. Operator: “Authorized for all operator admitted.”

## Already-selected dests (usable)

| Dest | Where | Notes |
|---|---|---|
| Maps MBTiles | BigBoy `/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles` | mode 0400; approval/inspect/receipt sidecars present |
| Maps approval | same dir `.approval.json` | quota 262144; ODbL; dest install_path bound |
| App catalog | `/root/mcnf-private/app-catalog-curated.json` | Flathub LibreOffice digest pin |
| App catalog receipt | `/root/mcnf-private/app-catalog-receipt.json` | bound to `7e3474eeb` |
| App VM base receipt | `/root/mcnf-private/app-vm-base-digest.json` | bound to `aca7573bc` |
| Bootc all-roles receipt | `/root/mcnf-private/bootc-all-roles-digest.json` | pin `3a5e74e6…` |
| Browser VM base receipt | `/root/mcnf-private/browser-vm-base-digest.json` | pin `3a5e74e6…` |

## Still missing (not invented)

| Leftover | Probe |
|---|---|
| Maps source-root | `/home/mm/mcnf-maps-sources` absent on BigBoy |
| RPM signer secret | `gpg --list-secret-keys 06B1C27EA0E08A225155EB3314018AA1497DDC7C` → no secret key |
| Surface `bootc_base` | `packaging/surface/surface-stack.f44.json` remains blocked/null |

S7 private argv was not written: `REPLACE_*` refuses, and a complete
object would need the missing source-root directory and a current-revision
RPM identity receipt. Preflight was not claimed.

Do not grind `cargo test --workspace`.
