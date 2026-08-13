# WL-UX-013 filtered support-export binding — 2026-08-13 r545

## Production result

The System and Mesh Health support export now materializes the exact captured
Health view instead of merely labeling an unfiltered bundle with the selected
scope and history filters.

- A node-scoped export includes only that node's grade and active conditions.
- Resolved history applies the captured node, severity, component, source, and
  provider filters before the fixed top-N bound.
- Mesh-wide exports retain their existing all-node scope.
- Contradictory resolved rows whose resolution predates their final observation
  are excluded from export, matching the modal's truthful history admission.
- A hostile regression supplies 64 higher-severity foreign-node rows and proves
  they neither leak into the bundle nor displace the one matching incident.

Changed production and regression surface:

- `crates/desktop/mde-shell-egui/src/health_modal.rs`

## Farm gates

- `.90`, slot 1 — `cargo test -p mde-shell-egui
  support_bundle_materializes_only_the_captured_health_view -- --exact
  --nocapture`: the test crate compiled, but the selector discovered 0 tests
  because the harness name is module-qualified. This is not counted as a passed
  regression.
- `.90`, slot 2 — strict all-target/all-feature shell Clippy with `--no-deps`:
  farm artifact corruption stopped compilation after dependency metadata and a
  target temporary directory disappeared. No source diagnostic was emitted for
  this slice.
- `.170`, slot 2 — all-target/all-feature shell build: production and test
  sources compiled through `mde-shell-egui`; final binary/test linkage failed
  because that farm image has no `libmpv` (`mold: library not found: mpv`).
- BigBoy `.130`, slot 1 — shell Rustfmt check ran once. It identified three
  owned layout deltas, which were applied exactly, plus pre-existing unrelated
  drift in `nav_bar.rs`. Per the single-run instruction it was not rerun.
- Scoped `git diff --check`: passed.

The failed or insufficient farm invocations are retained here rather than
misrepresented as passing evidence.

## Residual WL-UX-013 acceptance

- Permit the new hostile regression to execute under its module-qualified name
  on a farm lane with the required shell link dependencies.
- Complete first-release package integration.
- After that release, perform the deferred non-blocking one-node/lighthouse
  transition, recovery, export, and direct-render acceptance.
