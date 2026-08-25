# WL-FUNC-026 leftover honesty — Surface folder-prefs persist — r1

Date: 2026-08-25  
Observed: `2026-08-25T10:51:07Z`–`2026-08-25T10:53:59Z`  
Classification: leftover-honesty / live-seat observe; **not** operator
GUI view/sort/hidden mutation, **not** dest hydrate, **not**
`production_admitted`  
Source worktree: `agent/drain-worklist-20260725` at `4071ed295e18a8bd117cea5ee639eb5cafab3485`  
Installed seat: unpublished `magic-mesh-13.0.0-35` /
`4071ed295e18a8bd117cea5ee639eb5cafab3485`  
Control host: `rocky9-kvm2`  
Seat: Surface `172.20.146.79` (`SURFACE`, overlay `10.42.0.7`)  
Live topology mesh-id (governed, not minted here): `mcnf-clean-20260728`  
`production_admitted: false`

Read-only SSH plus non-mutating kmsgrab acquire. No `seat-update-warning`.
No dest JSON. No Construct restart. No GUI click. No Sunshine. No
uinput. Did not SSH Seat 15 or Dell.

## Authority

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-026`.
- Leftover: operator Files view/sort/hidden then restart.
- Prior Seat 15 dest hydrate (not operator use):
  `WL-FUNC-026-2026-08-24-seat15-folder-prefs-r1.md`.
- Packaged unit pin (source for this dest-cut):
  `WL-FUNC-026-2026-08-24-xdg-config-home-unit-r1.md`.

## Path used

`default_config_file("files-folder-prefs.json")` is
`$XDG_CONFIG_HOME/mcnf/files-folder-prefs.json`, else
`$HOME/.config/mcnf/files-folder-prefs.json` (`None` if both unset).

Unlike Seat 15's `7e3474eeb` leftover, dest-cut `4071ed295` ships the pin
in the packaged unit (not a local drop-in):

| Item | Value |
|---|---|
| Unit | `/usr/lib/systemd/system/mde-shell-egui.service` |
| Drop-ins | cloud-arm / music-action / resource-publisher HMAC only — **no** `60-xdg-config.conf` |
| Live environ | `XDG_CONFIG_HOME=/root/.config`; `HOME` unset; `USER=root` |
| Expected store | `/root/.config/mcnf/files-folder-prefs.json` |
| Observed | `/root/.config/mcnf` **absent**; store **absent** |

`/root/.config` exists (`gtk-3.0`, `pulse`, `sublime-music` only). Packed
shell contains ASCII `files-folder-prefs.json` (count 1). Resolver can
write after a GUI mutation; nothing has written yet.

## GUI click vs dest hydrate

kmsgrab acquired the live eDP-1 scanout (`2736x1824` XR30 X-tiled,
`-f null` exit 0). PNG/VAAPI conversion failed. No Files toolbar/menu
was visible, so view/sort/hidden was not clicked. Dest JSON was **not**
planted (Seat 15 dest hydrate is not repeated here). No restart, so
OPEN-hydrate of a store file was not observed.

Construct pid `1290917` has been up since `2026-08-24 20:25:57 EDT`
(`NRestarts=0`) after the dest-cut install. Persist files would have
survived that boot if a prior GUI mutation or dest had created them.
They did not exist.

## What this does not prove

- Operator Files toolbar/menu changed view, sort, or show-hidden.
- A visited-folder mutation then restart.
- GUI paint of Grid/Details after hydrate.
- Debounce writer creating the store.

## Leftover

Still open. Closing the production leftover needs operator Files use (or
a DRM/input harness on a **visible** Files control) on this
current-revision seat so a real view/sort/hidden change is what survives
restart. Packaged dest-cut pin is present; dest hydrate is not operator
use. Do not invent a dest. Do not flip `production_admitted`.
