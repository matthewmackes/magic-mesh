# WL-UX-013 S3 — bounded Health history source/provider filtering

Date: 2026-08-13
Branch: `agent/drain-worklist-20260725`

## Implemented behavior

`mde-shell-egui` now derives independently bounded source and evidence-provider
choices from truthful, node-scoped resolved conditions in the inclusive 24-hour
history window. Both selectors compose with severity and component before the
fixed 256-identity recurrence aggregation and eight-row page are materialized.
Changing either origin dimension invalidates stale page authority, and a choice
that disappears from the current bounded snapshot resets to `All`.

The selectors exist only inside `Recent History`; active required conditions
continue to render from `active_conditions` above history and cannot be hidden,
absorbed, or duplicated by a history-origin choice. Provider/source labels pass
through the existing support-text redactor before display.

## Farm evidence

- `.130`, slot `ux013-source-test`: `cargo test -p mde-shell-egui
  history_source_and_provider_filters_compose_before_recurrence_and_paging --
  --nocapture` — **passed 1/1**.
- `.90`, slot `ux013-source-clippy`: `cargo clippy -p mde-shell-egui --bin
  mde-shell-egui -- -D warnings` — **passed**. The stronger all-target attempt
  reached the crate but was blocked exclusively by concurrent test-target
  warnings in `car_keymap.rs`, `status_bar.rs`, and `system/mesh.rs`; none is in
  this slice's authorized file.
- `.50`, slot `ux013-source-fmt2`: file-scoped Rustfmt check for
  `health_modal.rs` — **passed**.
- `git diff --check` — **passed**.

## Remaining acceptance

- Add selectable resolved-history detail without weakening bounded/redacted
  rendering.
- Include the completed Health UI in the first full release package.
- After that release, perform the deferred non-blocking physical-seat and
  lighthouse transition proof against the installed package.
