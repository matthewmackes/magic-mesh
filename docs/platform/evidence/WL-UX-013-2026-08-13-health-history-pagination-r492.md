# WL-UX-013 health history pagination — r492

Date: 2026-08-13

## Scope

The centered System and Mesh Health modal previously retained and rendered only
the strongest eight resolved recurrence identities. Although the worklist calls
for paged 24-hour history, no operator action could reach later valid rows.

`crates/desktop/mde-shell-egui/src/health_modal.rs` now:

- retains a fixed maximum of 256 filtered recurrence identities and paints only
  the requested eight-row page;
- exposes Previous/Next controls and an exact page count when more than one page
  exists;
- resets page state when the selected node or severity filter changes;
- clamps a stale page after a live snapshot shrinks, avoiding an empty stranded
  detail view;
- preserves the existing node, severity, resolved-lifecycle, 24-hour-window,
  deterministic-order, and recurrence-count rules.

## Farm verification

- `172.20.0.170`, slot `ux013-history-paging-test-r492`: focused regression
  `health_modal::tests::history_pages_expose_later_rows_and_clamp_live_shrink`
  passed 1/1 (1,587 filtered out). The initial unqualified exact filter ran zero
  tests and was rejected as evidence before this corrected rerun.
- `172.20.0.170`, slot `ux013-history-paging-clippy-r492b`: strict
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings` passed.
- `172.20.0.196`, slot `ux013-history-paging-fmt-r492`: file-scoped
  `rustfmt --edition 2021 --check` — passed.

## Remaining epic acceptance

This closes the executable modal-history pagination gap. Post-release physical
seat/lighthouse transition captures remain deferred under the operator's release
sequencing direction, including planned absence, outage, rejoin, no false
emergency, and no duplicate Health authority.
