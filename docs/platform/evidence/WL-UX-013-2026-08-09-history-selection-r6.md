# WL-UX-013 S3 — bounded history and stable selection (r6)

Date: 2026-08-09  
Farm lane: machine 9 (`172.20.0.50`), slot `ux013-history-r6`

## Correction

- The production Health modal latches its initial node selection into modal state. Live snapshot reorder or temporary node removal no longer silently moves the detail pane.
- Resolved-history scanning maintains an ordered top-K buffer whose capacity never exceeds eight borrowed rows. It preserves the existing severity, duration, then ID order and excludes other-node rows while scanning.
- The reachable detail path remains ordered as Active Issues, Information, then Recent History; the correction does not place history above active conditions.

## Focused verification

```text
cargo test -p mde-shell-egui health_modal::tests::history_materialization_is_node_scoped_and_hard_bounded -- --exact --nocapture
1 passed; 0 failed; 1495 filtered out

cargo test -p mde-shell-egui health_modal::tests::live_snapshot_reorder_or_removal_never_moves_selection -- --exact --nocapture
1 passed; 0 failed; 1495 filtered out
```

The hostile history input contained 64 matching warnings, two matching critical rows, and one wrong-node critical row. It checks the exact eight-row severity/duration/ID order and node scope. The selection test checks initial latching, live reorder, and temporary removal without target movement. Scoped `git diff --check` passed. Source SHA-256: `a462394a4eec23e9825dc7333c2c51a0ff99e553e3fd4328868e4478c45effd2`.

## Remaining S3 and live limits

Durable 24-hour storage, explicit paging/filter controls, recurrence aggregation, redacted export/detail, restart behavior, packaging, and five-seat live proof remain open.
