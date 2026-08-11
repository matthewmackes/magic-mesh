# Persisted Left placement escape evidence — 2026-08-11

- Scope: Left-mode geometry reserves and bottom-anchors the placement control
  before admitting taskbar catalog, session, or pin controls.
- Failure behavior: on a hostile 320×160 restart with maximum pins and a live
  session, lower-priority controls shed before overlap; the placement target
  remains inside the owned rail and still dispatches `ToggleDock`.
- Farm gate: BigBoy `.130`, slot 1: **1 passed, 0 failed, 1,553 filtered**.
- File-scoped rustfmt and scoped `git diff --check`: passed.
- Remaining proof: responsive Dark/Light/largest-text and physical-seat capture
  remain part of the epic's broader acceptance.
