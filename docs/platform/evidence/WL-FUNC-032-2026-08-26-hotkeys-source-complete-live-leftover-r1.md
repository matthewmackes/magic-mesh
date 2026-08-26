# WL-FUNC-032 leftover honesty — hotkeys.rs source complete; live leftover remains — r1

Date: 2026-08-26
Classification: leftover-honesty / source-complete; **not** live-surface
Ctrl+J / Ctrl+N keystroke proof, **not** keystroke injection, **not**
`production_admitted`
Source revision (control tree, before this catalog sweep):
`b6fd8aeabcb850a11396a4a412b2ddd5c79b21ce`
Dest-cut identity: `bc14a22d79f9d7523e6fbf9ceae5b6a70c198e4c`
Control host: `rocky9-kvm2`
`production_admitted: false`

This worker did **not** occupy Seat 15 Construct (FUNC-023 live worker),
did **not** click Surface or Dell, did **not** SSH those seats, and did
**not** inject uinput. No Sunshine start. No dest invented. Foreign dirty
`main.rs` / `mackesd` / peer crate files were not folded.

## Catalog (`hotkeys.rs`)

Bindings were already complete before this change:

- `Ctrl+J` → `HotkeyAction::OpenTransfers` without a Super leader
- `Ctrl+N` → `HotkeyAction::NewTransfer` (catalog registration; in-mode
  consume stays in apply-site / `transfers.rs`)
- refuse while Communications (Documents), Terminal, Desktop, or Browser
  have text / PTY / guest focus
- fire on every other Springboard surface, and on those four without
  text/guest focus
- not host-first, so the apply site can leave the keystroke with the
  editor / guest

The prior named test only asserted the fire path on Files. This change
names `is_transfer_ctrl_refuse_surface` and sweeps `Surface::ALL` so S1
"from every surface" is a catalog assertion. No chord, action, or refuse
set changed.

## Journal (apply site; not this write scope)

In-tree `main.rs` at `b6fd8aeab` journals:

- `open_transfers` / `new_transfer` on apply (`mde_shell_egui::hotkeys`)
- `transfer_hotkey_refused` when Documents/Terminal still shadow at the
  apply site. Desktop/Browser refuse in the catalog, so they never emit
  an apply-site action.

Farm: `cargo test -p mde-shell-egui` 1591 passed / 0 failed (recorded in
`WL-FUNC-032-2026-08-25-destcut-keystroke-no-frame-r1.md`). Those journal
lines are **not** on dest-cut `bc14a22d7` (`b6fd8aeab` is a descendant).
Seat 15 Keychron chords on dest-cut produced no `open_transfers` journal
and no readable kmsgrab frame.

## Farm this unit

Skipped. Parent HDMI Control Panel owns dirty
`crates/desktop/mde-shell-egui/src/main.rs` (~318-line uncommitted
delta). `cargo test -p mde-shell-egui` would compile and race that tree.
Local heavy cargo stays blocked (exit 97). Do not revert parent edits.

## Non-claims

- Ctrl+J / Ctrl+N were not pressed on a live Construct surface.
- Transfers mode was not observed opening from any surface.
- In-mode New Transfer was not observed.
- Documents / Terminal / Desktop / Browser refuse was not observed live.
- Dest-cut was not replaced. `production_admitted` was not flipped.
- No dest was invented. uinput without a frame record is forbidden.

## Leftover / blocker

`@leftover:{live-seat}` remains. Catalog + ALL-sweep + dest-cut packed
literals + dock-journal do **not** close S1. Closing needs a used
acceptance seat on a dest-cut that carries the apply/refuse journal,
then a real Construct key press: Ctrl+J from every surface opening
Communications Transfers, in-mode Ctrl+N starting a new transfer, and
the refuse holding on Documents / Terminal / Desktop / Browser. Injecting
uinput invents a dest and still could not record the frame.
