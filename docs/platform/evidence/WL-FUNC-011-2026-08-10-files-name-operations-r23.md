# WL-FUNC-011 — reachable Files name operations (r23)

Date: 2026-08-10

Base revision: `7f757e23`

## Defect and correction

Files already had one injected `FileOps` authority, but New Folder and Rename
were absent from the interface. Both actions are now reachable from the normal
menus and share one bounded single-component dialog. New Folder targets the
resolved active directory; Rename requires exactly one selected local item.
Validation rejects empty and whitespace-only names, dot aliases, paths, NUL,
unchanged names, and components longer than 255 bytes.

Review found that an existence preflight followed by overwrite-capable
`rename(2)` left a race window. The existing `FileOps` seam now exposes a safe
atomic `renameat2(RENAME_NOREPLACE)` operation, and both its live and fake
implementations preserve an existing destination.

## Focused farm proof

- Machine 193 (`172.20.0.90`) passed all six name-dialog, menu, model, and
  rendering tests: 6 passed, 0 failed, 167 filtered out.
- Machine 9 (`172.20.0.50`) passed the exact live atomic no-replace filesystem
  test: 1 passed, 0 failed, 155 filtered out.
- Farm `cargo fmt -p mde-files -p mde-files-egui -- --check` and local
  `git diff --check` passed.

The slice adds no second filesystem authority. Broader collaboration transfer,
live-seat, and three-seat-maximum release acceptance remain open.
