# WL-UX-013 S3 health-history component filtering — r502

Date: 2026-08-13

## Production change

The centered Health modal previously offered only All/Warning/Critical history
filtering. That left no way to isolate a component and did not exercise the
worklist's required filter combinations.

`crates/desktop/mde-shell-egui/src/health_modal.rs` now provides an explicit,
bounded All/System/Mesh/Resources/Devices/Audio/Firmware/Evidence component
filter. Component and severity predicates compose before recurrence
aggregation, the 256-identity bound, and eight-row paging. Changing either
filter dimension invalidates stale page authority and returns the operator to
page one. Active issues remain in their separate section above resolved
history and are not affected by history filters.

## Farm verification

- BigBoy `172.20.0.130`, slot `ux013-component-test-r502`:
  `cargo test -p mde-shell-egui health_modal::tests::history_component_and_severity_filters_compose_before_paging -- --exact --nocapture`
  passed 1/1 with 1,579 filtered tests.
- BigBoy `172.20.0.130`, same warmed slot:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings`
  passed.
- `.196`, slot `ux013-component-fmt-r502`, direct file-scoped farm check:
  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/health_modal.rs`
  passed.

The broader package-format probe was not accepted as slice evidence because it
reported only pre-existing formatting drift in the explicitly excluded
`vdi/resources.rs`; that file was preserved unchanged.

## Remaining acceptance

Within S3, explicit source/provider filtering and a selectable resolved-history
detail view remain to satisfy the full filter/detail outcome; the fixed 24-hour
window, recurrence, paging, severity filtering, component filtering, and stable
selection are implemented. S4 governed recovery and redacted export code is
present with focused evidence, while full release packaging and the operator-
deferred post-release physical-seat/lighthouse transition proof remain open.
