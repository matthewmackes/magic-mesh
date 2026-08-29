# WL-FUNC-023 S4 — Lifecycle menu uses the shared session screen — r1

Source heal only. Does not close `WL-FUNC-023`. Live leftover stays
`WL-TEST-003`. `production_admitted: false`. No dest invented.

## Change

`MenuItem::Lifecycle` opened `Screen::Status`, so the TUI never showed
the shared [`LifecycleSessionView`]. It now opens `Screen::Lifecycle`.
`Wizard::lifecycle_lines()` is the GUI/TUI copy: honest
`no lifecycle session published`, or the typed status/capability lines.
The screen is read-only (no found/join/status verbs).

## Farm

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=1
./install-helpers/xcp-build.sh cargo test -p mde-enroll
```

Admission: 9,263,392 KiB free on `.50`. Exit 0.
Lib: **53 passed, 0 failed** (includes
`lifecycle_menu_opens_the_shared_session_screen_not_status`).
Bins: **3 passed**. Ended 2026-08-29T11:59–12:00Z.

No workspace grind. No `mackesd` re-run.
