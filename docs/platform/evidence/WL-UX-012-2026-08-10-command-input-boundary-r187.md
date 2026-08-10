# WL-UX-012 — Front Door command-input boundary (r187)

Date: 2026-08-10

## Production behavior

The explicit Front Door > command route now rejects control characters
(including CR, LF, and tab) before producing a terminal activation target. It
also enforces the existing 256-character omnibox budget at the command parser
boundary, so pasted or directly constructed input cannot bypass the UI editor's
bound.

## Focused farm proof

BigBoy 172.20.0.130, slot ux012-front-door-command-input-r187c, passed:

~~~text
cargo test -p mde-shell-egui --bin mde-shell-egui front_door_run_command -- --nocapture
~~~

Result: 4 passed; 0 failed; 0 ignored; 0 measured; 1541 filtered out.

The gate included the new control-separator/oversize regression plus the
existing explicit command-mode accessibility and terminal-route tests.
git diff --check passed before evidence was recorded.

## Live limits

No live seat, terminal process, or physical taskbar proof was run. This
checkpoint proves the pure input/model boundary and the shell's focused
headless route tests only.
