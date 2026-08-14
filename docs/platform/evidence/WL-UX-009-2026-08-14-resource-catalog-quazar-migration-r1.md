# WL-UX-009 resource catalog Quazar migration

- BigBoy `.130` farm slot: `ux-009-resource-catalog`.
- `crates/desktop/mde-shell-egui/src/chooser/resources.rs` migrated catalog
  headers, hero lanes, service cards, typography, radii, and spacing to shared
  Quazar `Style`/`TypographyRole` tokens; lifecycle colors remain domain-owned.
- Gate: `cargo check -p mde-shell-egui --all-targets` — PASS.
- `git diff --check` — PASS.
