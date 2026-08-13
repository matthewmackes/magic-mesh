# WL-UX-013 support-export authority boundary (r516)

Date: 2026-08-13

The Health support export now captures and carries the exact admitted snapshot
generation and timestamp, selected node/mesh scope, severity/component/source/
provider filters, and selected resolved-incident identity. The durable export
seam compares that captured authority with the current Health authority and
fails with `PermissionDenied` before creating the destination directory when
any member has changed. The same authority tuple is recorded in the redacted
bundle, and a selected incident must still resolve to the exact retained
same-node identity in the captured snapshot.

## Farm gates

- `172.20.0.130`, slot `ux013-export-authority-test-r516`:
  `cargo test -p mde-shell-egui support_export_rejects_replaced_generation_scope_filters_and_incident -- --nocapture`
  passed 1/1 (1,597 filtered).
- `172.20.0.50`, slot `ux013-export-authority-clippy-r516`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings` passed.
  The first synced attempt exposed an unrelated in-flight `timers.rs` import;
  the passing rerun retained the warmed target and restored only unrelated shell
  files to `HEAD` inside the disposable farm workspace.
- `172.20.0.90`, slot `ux013-export-authority-fmt-r516b`:
  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/health_modal.rs`
  passed.

The hostile regression independently replaces generation, node scope, each
filter dimension, and selected incident identity. Every replacement is refused
before a durable export path exists. Existing no-follow, byte/redaction,
resolved-history identity, paging/filter, and secret tests were not duplicated.
