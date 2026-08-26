# WL-FUNC-032 leftover — dest-cut catalog + live Keychron chords; no frame — r1

Date: 2026-08-25
Classification: leftover-honesty / live-seat; **not** readable Transfers
paint, **not** S1 close, **not** `production_admitted`
Dest-cut identity: `bc14a22d79f9d7523e6fbf9ceae5b6a70c198e4c`
Seat: Seat 15 `172.20.0.15` (`Basement-Test-Workstation`)
`production_admitted: false`

No new uinput node. No Sunshine start (`/usr/bin/sunshine` is present and
was left stopped). No invented dest. Foreign dirty `mackesd` files were
not folded.

## Dest-cut catalog

`git merge-base --is-ancestor 617e7a5eb bc14a22d7`. Installed
`/usr/bin/mde-shell-egui` (mtime 2026-08-25 11:33, dest-cut) contains
`Ctrl+J`, `Ctrl+N`, `Open Transfers`, and `New transfer`. Construct pid
`1909020` holds `/dev/dri/card1` and Keychron K6 `/dev/input/event4`
(KEY_LEFTCTRL+KEY_J+KEY_N).

## Live chords (existing keyboard, not uinput)

Ctrl+J then Ctrl+N were written to `/dev/input/event4`, which Construct
already had open. `mde-shell-egui.service` journal gained no
`OpenTransfers` / `open_transfers` lines. `settings-nav-bar.json` did
not change. Transfers worker stayed `running` with no UI latch
publication.

`ffmpeg -f kmsgrab -device /dev/dri/card1` acquired `drm_prime` and
could not convert to PPM (`Function not implemented`). grim/Moonlight
absent.

## Source follow-up (this change)

`apply_hotkey` now journals `open_transfers` / `new_transfer` with the
current surface, and the Documents/Terminal/Desktop/Browser refuse path
journals `transfer_hotkey_refused`. Farm: `cargo test -p mde-shell-egui`
1591 passed / 0 failed on `.90` slot 1. Those lines are **not** on the
dest-cut seat until the next unpublished workstation replace.

## Non-claims

- Communications Transfers paint was not read.
- Refuse on Documents / Terminal / Desktop / Browser was not observed live.
- S1 is not closed. Catalog + dest-cut + held-keyboard chords do not
  close the leftover.
