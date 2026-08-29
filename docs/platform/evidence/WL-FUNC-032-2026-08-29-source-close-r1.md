# WL-FUNC-032 source close — 2026-08-29

Classification: source/cargo close. Live-surface keystroke leftover remains
`WL-TEST-003` after a testing Beta.

Tree: `5f9685408` plus the voice-admin persist compile fix (dirty).
`production_admitted: false`. No dest invented. No uinput.

## Why this closes

S1 is in-tree: `hotkeys.rs` catalogs Ctrl+J (Communications Transfers)
and the in-mode New Transfer accelerator, refuses shadowing in
Documents/Terminal, and journals `open_transfers` /
`transfer_hotkey_refused`.

## Farm (one crate, dirty tree)

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=1
./install-helpers/xcp-build.sh cargo test -p mde-shell-egui
```

Admission: 13,487,788 KiB free on `.50` (required 8,388,608 KiB).
Result: **1652 passed, 0 failed**, exit 0, ended 2026-08-29T11:57:06Z.

Do not grind `mde-collab-egui` again for this close.

Live leftover: `WL-FUNC-032-2026-08-25-destcut-keystroke-no-frame-r1.md`.
