# WL-UX-009 shared Quazar tooltip gate

- Mechanical gate: `install-helpers/lint-style-leaks.sh` — PASS; Carbon
  registry 44/44 and zero raw color, motion, or hover-text leaks.
- The 23 raw `on_hover_text` calls across Music, status chrome, resource
  catalog, Android apps, and Remote Sessions now route through the shared
  `mde_egui::hover_text` / `disabled_hover_text` overlay helpers.
- BigBoy `.130`: `cargo check -p mde-shell-egui --all-targets` — PASS.
- The check emitted only the pre-existing unused
  `begin_connection_generation` warning in `mde-vdi-rdp`.
