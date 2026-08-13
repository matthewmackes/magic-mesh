# WL-UX-013 — stable resolved-history detail (r504)

Date: 2026-08-13

## Result

The System and Mesh Health modal now exposes selectable resolved-history rows
and a bounded, redacted detail projection. Selection is retained by the exact
`(node, incident_id)` lifecycle identity rather than by row position. Live
reordering, severity/component/source/provider filtering, and page changes
therefore cannot replace the selected incident with another row. If retention
removes the selected incident, the modal reports that it is no longer retained
and does not substitute a different incident. Active issues remain rendered
above the history authority.

Detail is resolved directly from the admitted snapshot's bounded 24-hour
history. Evidence facts use the existing credential/path redaction and fixed
fact-count bounds; no render-path I/O was introduced.

## Farm gates

- `172.20.0.50`, slot `ux013detail-test`:
  `cargo test -p mde-shell-egui resolved_history_selection_keeps_exact_identity_across_reorder_filter_and_page_changes -- --nocapture`
  passed 1/1 with 1,583 filtered tests.
- `172.20.0.90`, slot `ux013detail-clippy`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings`
  passed.
- `172.20.0.50`, owned-file format gate:
  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/health_modal.rs`
  passed. Crate-wide formatting was not claimed because an unrelated committed
  drift remains in `src/vdi/resources.rs`.
- `git diff --check` passed.

The original BigBoy `.130` focused-test attempt became unreachable during its
cold compile and is not counted as evidence. The exact test was rerouted once
to `.50`, as recorded above.

## Remaining acceptance

The named S3 coding acceptance for Health history detail/filter/selection is
complete. WL-UX-013 still requires first-full-release packaging followed by the
operator-deferred, non-blocking installed physical-seat/lighthouse transition
proof (boot, sleep, network loss, maintenance, outage, and rejoin), including
confirmation that Health remains the sole authority.
