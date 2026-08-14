# WL-ARCH-010 — full shell farm gate

- Date: 2026-08-14
- Revision: `e749e2ad`
- Farm: BigBoy `172.20.0.130`, slot `shell-full-audit`
- Command: `cargo test -p mde-shell-egui --bin mde-shell-egui`
- Result: 1,621 passed, 0 failed, 0 ignored

The full shell suite passed across front door, Workloads/VDI, Clock, Music,
communications, clipboard, device management, lock curtain, taskbar, health,
accessibility, and render-model paths. The gate also verified the VNC/SPICE
input regression: Escape is host-owned before focus acquisition, and keyboard
input is admitted only over the guest framebuffer or held focus.

The existing `mde-vdi-rdp` dead-code warning and SVG parser warnings are
non-failing dependency/runtime warnings; no shell test failed.
