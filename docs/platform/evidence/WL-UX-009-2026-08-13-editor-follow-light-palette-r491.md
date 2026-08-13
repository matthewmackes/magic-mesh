# WL-UX-009 Editor follow control Light palette — 2026-08-13

- Scope: the runtime-reachable Editor collaboration follow affordance in
  `crates/desktop/mde-editor-egui/src/follow.rs`.
- Acceptance link: WL-UX-009 S2/S3 require every Construct surface to use the
  shared Quazar language and remain legible in Light appearance.
- Defect: the custom follow button supplied canonical Dark token constants
  directly to egui, bypassing the active appearance for its raised surface,
  accent outline, and high-accent label.
- Resolution: all three semantic tokens are resolved through the installed
  `StyleColorScheme` at the render boundary. The live follow/unfollow dispatch
  and interaction behavior are unchanged.
- Focused farm gate: BigBoy `.130`, slot
  `ux009-editor-follow-light-test-r491`; exact test
  `follow::tests::the_banner_resolves_every_custom_color_into_quazar_light`
  passed 1/1 (414 filtered). The render assertion inspects emitted egui shapes,
  requires the Quazar Light surface/accent/high-accent colors, and rejects
  retained Dark surface/accent tokens.
- Static farm gate: BigBoy `.130`, slot
  `ux009-editor-follow-light-clippy-r491`; `cargo clippy -p mde-editor-egui
  --all-targets -- -D warnings` passed.
- Format farm gate: `.196`, slot `ux009-editor-follow-light-fmt-r491`;
  `rustfmt --edition 2021 --check` passed for the touched source file.
- Remaining epic acceptance: inventory and migrate remaining Construct-owned
  style bypasses; complete Dark/Light/responsive/largest-text/stale/unavailable
  captures; then prove motion, focus, repaint, packaging, and human review for
  the first full release.
