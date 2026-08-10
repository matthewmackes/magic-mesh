# WL-UX-013 — bounded health-history severity filtering (r140)

Date: 2026-08-10

Base revision: `d0fc3c10`

## Result

The centered System and Mesh Health detail view now provides persistent
All/Warning/Critical history filtering. The selected severity is applied before
same-condition recurrence aggregation and before the eight-row history page
cap, so excluded records cannot displace matching history. Node scope and the
existing inclusive 24-hour window remain mandatory. A valid filter with no
matches renders an explicit empty state instead of a blank region.

The filter is temporary UI state only. It does not change signed health input,
expected-state evaluation, grades, recovery authority, or persisted history.

## Focused farm proof

BigBoy `.130` (`172.20.0.130`), isolated slot
`wl-ux013-history-filter-r1`:

```text
cargo test -p mde-shell-egui \
  history_severity_filters_apply_before_the_bounded_recurrence_page \
  --features live-vdi
```

Result: 1 passed, 0 failed, 1,573 filtered. The related history filter suite
passed 4/4 with 1,570 filtered. Target-file rustfmt and `git diff --check`
passed.

## Remaining boundary

Component, source, and date filters; broader filter combinations;
expected-state publishers/transition evaluation; and physical-seat visual
proof remain. This checkpoint does not close WL-UX-013.
