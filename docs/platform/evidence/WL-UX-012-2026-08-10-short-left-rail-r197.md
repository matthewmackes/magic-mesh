# WL-UX-012 short Left rail geometry — r197

- Scope: short Bottom/Left taskbar geometry must admit only controls whose hit
  targets remain inside the owned display rectangle; fixed controls beyond the
  available height are deferred to the bounded More path.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=ux012-short-left-rail-r197-final-bigboy install-helpers/xcp-build.sh cargo test -p mde-shell-egui --bin mde-shell-egui nav_bar::tests::short_left_rail_admits_only_controls_inside_its_display_rect -- --exact --nocapture`.
- Result: BigBoy `.130` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 1547 filtered out`.
