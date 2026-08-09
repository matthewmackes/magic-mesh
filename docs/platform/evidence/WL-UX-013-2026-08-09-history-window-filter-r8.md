# WL-UX-013 S3 — 24-hour history filter boundary (R8)

Date: 2026-08-09

## Gap and correction

The Health modal's bounded eight-row recurrence page filtered by node but did
not enforce its promised 24-hour history window. A matching record of any age
could displace a recent row, and hostile unresolved or future-dated records
could be rendered as resolved history.

`health_modal.rs` now derives an inclusive 24-hour window from the signed
snapshot's `generated_at_ms`. Filtering happens before bounded ranking and
recurrence counting. A row participates only when it belongs to the selected
node and has `resolved_at_ms` in
`generated_at_ms - 24 hours ..= generated_at_ms`; saturating subtraction keeps
the lower boundary valid near epoch zero. Using snapshot time rather than wall
clock time also makes a single snapshot's page stable across repaints.

The hostile regression admits records exactly at both window boundaries while
rejecting a critical record one millisecond too old, a critical record one
millisecond in the future, and a critical record with no resolution timestamp.
The excluded high-severity records therefore cannot displace valid recent
history.

## Focused verification

BigBoy (`172.20.0.130`), slot `ux013-history-filter-r8`:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=ux013-history-filter-r8 \
  install-helpers/xcp-build.sh cargo test -p mde-shell-egui \
  health_modal::tests::history_filter_rejects_out_of_window_future_and_unresolved_records \
  -- --exact

test health_modal::tests::history_filter_rejects_out_of_window_future_and_unresolved_records ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1506 filtered out
```

The same synced BigBoy slot passed a direct `rustfmt --edition 2021 --check`
for `crates/desktop/mde-shell-egui/src/health_modal.rs`. Scoped
`git diff --check` also passed. The earlier machine 9 attempt was deliberately
stopped for host-capacity enforcement and is not counted as verification.

## Remaining S3 scope

This closes one concrete modal filter-correctness gap. Durable 24-hour history
storage, explicit operator filter combinations and paging controls, restart
behavior, export/detail redaction, packaging, and five-seat proof remain open.
