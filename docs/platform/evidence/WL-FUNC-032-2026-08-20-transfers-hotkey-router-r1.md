# WL-FUNC-032 evidence — Transfers hotkey router slice

Date: 2026-08-20
Status: implementation evidence; worklist remains `Remaining` until the
authoritative worklist is reconciled by the operator.

## Scope

The existing shell/Communications path now covers the epic's two typed chords:

- `Ctrl+J` is catalogued as `HotkeyAction::OpenTransfers`, dispatched without
  the Super leader, routed by the shell to Communications, and consumed by the
  Communications surface as a Transfers-mode intent.
- `Ctrl+N` is catalogued as `HotkeyAction::NewTransfer`, accepted only while
  Communications is on Transfers, and opens the in-mode New Transfer editor.

The repair in this slice makes the process-local intent ownership symmetric:
`Ctrl+N` requests now record the requesting render thread, and the consumer
refuses to steal either `Open` or `New` intents from another render thread.
This preserves the existing multi-test/multi-surface isolation behavior for both
hotkeys.

## Source and test proof

- `crates/desktop/mde-shell-egui/src/hotkeys.rs`
  - `ctrl_j_opens_transfers_without_a_leader`
  - `ctrl_n_is_the_in_mode_new_transfer_accelerator`
  - `transfer_chords_require_the_exact_ctrl_modifier`
  - `transfer_ctrl_chords_are_listed_and_unique_in_the_catalog`
- `crates/desktop/mde-collab-egui/src/transfers.rs`
  - `request_open_transfers` / `request_new_transfer`
  - `take_transfers_hotkey_intent`
- `crates/desktop/mde-collab-egui/src/tests.rs`
  - `ctrl_j_opens_transfers_mode_from_any_communications_surface`
  - `ctrl_n_in_transfers_opens_the_new_transfer_editor`
  - `request_open_transfers_latch_lands_on_transfers_mode`

## Verification

Formatting passed locally with:

    cargo fmt -p mde-collab-egui -- --check

Focused farm verification passed on `172.20.0.170`:

    cargo test -p mde-collab-egui ctrl_ -- --nocapture
    # 5 passed, 0 failed

    cargo test -p mde-collab-egui request_open_transfers_latch -- --nocapture
    # 1 passed, 0 failed

The first attempted BigBoy route was refused because its `/home` free space
was below the farm sync headroom requirement; the gate was rerouted to `.170`.
