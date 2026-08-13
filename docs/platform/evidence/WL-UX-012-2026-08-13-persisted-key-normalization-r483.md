# WL-UX-012 persisted taskbar-key normalization gate

Date: 2026-08-13

The taskbar preference decoder now tolerates surrounding whitespace, ASCII case
drift, and underscore-vs-hyphen formatting from older or hand-edited profiles,
while still resolving only the finite canonical surface table. Tool-tray-only
historical destinations remain excluded from persisted center-taskbar pins.

## Farm gates

- BigBoy `.130`, slot `ux012-nav-prefs-canonicalization-20260813`:
  `cargo test -p mde-shell-egui --locked --bin mde-shell-egui taskbar_surface_preferences_are_versioned_and_fail_closed -- --nocapture` — PASS (1/1; 1,581 filtered).
- `.90`, slot `ux012-nav-prefs-clippy-20260813`:
  `cargo clippy -p mde-shell-egui --locked --bin mde-shell-egui` — PASS.
- `.50`: `cargo fmt --check` — PASS (from implementation lane).

## Remaining acceptance

UX-012 remains `Remaining`: responsive/render proof, broader persistence and
deep-link integration, and post-release seat proof remain.
