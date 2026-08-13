# WL-UX-012 taskbar keyboard action map — 2026-08-13

- Scope: focused taskbar controls now activate through Enter/Space as well as a pointer click.
- Implementation: `crates/desktop/mde-shell-egui/src/nav_bar.rs` centralizes activation semantics in `control_activated`; Start, Search, Back, Home, pin, app, session, and overflow controls use the same predicate.
- Boundary: an Enter/Space key is ignored when the control is not focused; unrelated key presses never activate a control.
- Focused test: `nav_bar::tests::focused_taskbar_controls_activate_only_on_enter_or_space`.
- Farm gates:
  - `.90` `cargo fmt -p mde-shell-egui -- --check` — **PASS**.
  - `.50` `cargo test -p mde-shell-egui --locked --bin mde-shell-egui nav_bar::tests::focused_taskbar_controls_activate_only_on_enter_or_space -- --exact --nocapture` — **PASS**, 1/1 (1,581 filtered).
- Remaining WL-UX-012 acceptance: full responsive/render proof, persistence/deep-link integration, and post-release seat proof.
