# WL-FUNC-032 leftover honesty — source complete; leftover is live keystroke after Health Fix — r1

Date: 2026-08-25  
Classification: leftover-honesty / source-complete; **not** live-surface
Ctrl+J / Ctrl+N keystroke proof, **not** keystroke injection, **not**
`production_admitted`  
Source revision (control tree / dest-cut):
`4071ed295e18a8bd117cea5ee639eb5cafab3485`  
Control host: `rocky9-kvm2`  
`production_admitted: false`

This agent did **not** SSH Seat 15 or Dell (used DRM). Parent's Construct
Health Fix click is recorded in
`WL-FUNC-023-2026-08-25-construct-health-fix-click-r1.md` (no frame;
overlay-ip still empty; `Restart mackesd` not confirmed). No package
install, no enroll, no dest invented, no Sunshine start, no Ctrl+J /
Ctrl+N uinput.

Prior catalog records (installed `13.0.0-35` literals, not keystroke):
`WL-FUNC-032-2026-08-23-installed-hotkeys-catalog-r1.md` (Dell),
`WL-FUNC-032-2026-08-24-live-hotkeys-r1.md` (Seat 15). Dest-cut land:
`WL-FUNC-023-2026-08-24-destcut-4071ed295-upgrade-wipe-r1.md` (same NEVRA
`13.0.0-35`, identity `4071ed295`).

## Source complete

`git diff 4071ed295 HEAD --` is empty for:

- `crates/desktop/mde-shell-egui/src/hotkeys.rs`
- `crates/desktop/mde-collab-egui/src/transfers.rs`

In-tree `hotkeys.rs` already:

- catalogs `Ctrl+J` → `HotkeyAction::OpenTransfers` and `Ctrl+N` →
  `HotkeyAction::NewTransfer` without a Super leader
- refuses those chords while Communications (Documents), Terminal,
  Desktop, or Browser have text / PTY / guest focus
- fires them on other surfaces and on those surfaces without text focus

In-tree `transfers.rs` already latches `request_open_transfers` /
`request_new_transfer` and consumes in-mode Ctrl+N into the New Transfer
editor. There is no remaining binding gap in this write scope.

## Health Fix does not close S1

Construct Health Fix is the overlay / identity heal path (FUNC-023). The
2026-08-25 click did not press Ctrl+J or Ctrl+N, did not open
Communications Transfers, and had no DRM frame. After that click on a
used dest-cut seat the leftover is unchanged.

## Non-claims

- Ctrl+J / Ctrl+N were not pressed on a live Construct surface.
- Transfers mode was not observed opening from any surface.
- In-mode New Transfer was not observed.
- Documents / Terminal / Desktop / Browser refuse was not observed live.
- `production_admitted` was not flipped.
- No dest was invented. uinput without a frame record is forbidden.

## Leftover / blocker

Source and dest-cut catalog are complete. `@leftover:{live-seat}` remains
a real Construct key press on a used seat after Health Fix: Ctrl+J opening
Communications Transfers from every surface, in-mode Ctrl+N starting a new
transfer, and the catalog refuse holding on Documents / Terminal / Desktop
/ Browser text-or-guest focus. Catalog presence, dock-journal, dest-cut
`4071ed295`, and Health Fix do not close S1.
