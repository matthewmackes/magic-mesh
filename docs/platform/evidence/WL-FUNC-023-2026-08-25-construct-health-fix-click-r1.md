# WL-FUNC-023 — Construct Health Fix click on dest-cut seats (2026-08-25)

Operator authorized live Construct Health Fix on Seat 15 and Dell after dest-cut
`4071ed295` / unpublished `13.0.0-35`. `production_admitted` unchanged. No REL
freeze. No invented dest, token, mesh-id, or Sunshine start. Foreign dirty
`mackesd` files were not folded.

## Seats (dest-cut identity)

Both run `mackesd 13.0.0 "Construct" · 4071ed295e18a8bd117cea5ee639eb5cafab3485 · 2026-08-24 · dev`.
Construct (`/usr/bin/mde-shell-egui`) is active as root and holds `/dev/dri/card1`.

| Seat | Overlay-ip | CA | Host cert | After click |
|---|---|---|---|---|
| Seat 15 `Basement-Test-Workstation` | empty | present | absent | still empty |
| Dell `DELL-LAPTOP` | empty | present | absent | still empty |

## What Health actually offers (not Publish overlay IP)

Latest `state/health/node/<host>` snapshots do **not** carry
`overlay-ip-unpublished` / `PublishOverlayIp`. That nag requires
`live_overlay_ip && !overlay_ip_published`. Host cert is absent, so the overlay
iface is not live and the typed overlay Fix is not offered.

Seat 15 (9 conditions): `restart_mackesd` (confirm, critical; grouped plane is
already active), `restart_dns`, `restart_kdc`, `refresh_provider` (no confirm),
`run_lifecycle_firstboot`, `recover_xdg_binds`, `open_onboarding` (arming dest),
`restore_workstation_audio`, `refresh_firmware_metadata`.

Dell (8): same minus firstboot/xdg; plus `open_onboarding` for missing Browser VM
image dest.

`restart_mackesd` was **not** confirmed. It would target monolithic
`mackesd.service` while `mackesd-control.service` is already active (solutions
note F1).

## Click path

1. Packaged `seat-remote-input` `absolute_tap` on the computed System and Mesh
   Health hit target (24 px top rail; Seat 15 `(1015,12)` @ 1920×1080, Dell
   `(739,12)` @ 1366×768). Helper exit 0. Kernel logged
   `input: mackesd-seat-remote-input`. Construct's libinput fds were opened at
   yesterday's shell start and did **not** add the short-lived uinput node.
2. Pointer events were then written to devices Construct already holds: Seat 15
   Keychron/MS116 event-mouse; Dell `DELL081A` event5 Mouse. Park top-left, move
   to the health target, left click. Overlay-ip unchanged.
3. DRM capture: `ffmpeg kmsgrab` cannot steal card1 from Construct. grim/Moonlight
   absent. No frame readback, so the modal Fix for `Refresh provider` was not
   guessed (first row is the unsafe `Restart mackesd` confirm).

## Close condition

Live Construct Fix proof for overlay-ip / join leftovers is still open. The
offered heal for missing host cert / arming is `Open Onboarding`, which needs an
operator dest, not an invented enroll.

## Parallel leftover fan (this tick)

Disjoint Cursor workers (no Seat 15/Dell DRM, no dirty `mackesd` paths):

- FUNC-025/026/027 Files on Surface
- FUNC-028/029/030 Transfers/Activity on Surface (no Vitelity dest)
- FUNC-024/031 Calls/Documents source + Surface-only
- FUNC-032 hotkeys.rs only
