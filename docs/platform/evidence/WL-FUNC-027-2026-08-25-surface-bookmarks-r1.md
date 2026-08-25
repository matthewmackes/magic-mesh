# WL-FUNC-027 leftover honesty — Surface bookmarks persist — r1

Date: 2026-08-25  
Observed: `2026-08-25T10:51:07Z`–`2026-08-25T10:53:59Z`  
Classification: leftover-honesty / live-seat observe; **not** operator
pin/rename/reorder/remove/navigate, **not** dest hydrate, **not**
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

- Worklist: `docs/platform/WORKLIST.md` `WL-FUNC-027`.
- Leftover: operator pin then restart/navigate.
- Prior Seat 15 dest hydrate (not operator use):
  `WL-FUNC-027-2026-08-24-seat15-bookmarks-r1.md`.
- Same packaged `XDG_CONFIG_HOME` pin as WL-FUNC-026:
  `WL-FUNC-026-2026-08-24-xdg-config-home-unit-r1.md`.

## Path used

`default_config_file("files-bookmarks.json")` is
`$XDG_CONFIG_HOME/mcnf/files-bookmarks.json`. Dest-cut `4071ed295`
Construct pins `XDG_CONFIG_HOME=/root/.config` in the packaged DRM unit.
`HOME` is unset on pid `1290917`.

| Item | Value |
|---|---|
| Expected store | `/root/.config/mcnf/files-bookmarks.json` |
| Observed | `/root/.config/mcnf` **absent**; store **absent** |
| Packed literal | `files-bookmarks.json` count 1 in `/usr/bin/mde-shell-egui` |

No user Places pins on disk. Same XDG pin as folder-prefs; no
Seat-15-style local drop-in on this seat.

## GUI click vs dest hydrate

kmsgrab acquired tiled XR30 scanout; no PNG; no visible Places
pin/navigate control. Dest JSON was **not** planted. No restart, so
OPEN-hydrate of a bookmark store was not observed. Persist files would
have survived the dest-cut boot (`ActiveEnterTimestamp` `2026-08-24
20:25:57 EDT`, `NRestarts=0`) if a prior pin or dest had created them.

## What this does not prove

- Operator pin / rename / reorder / remove in Places.
- Navigate-on-activation of a hydrated pin.
- GUI paint of the user Places section.

## Leftover

Still open. Closing the production leftover needs operator pin then
restart and activate on this current-revision seat. Packaged dest-cut
pin is present; dest hydrate is not operator use. Do not invent a dest.
Do not flip `production_admitted`.
