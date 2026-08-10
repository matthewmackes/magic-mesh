# WL-UX-013 — stale health status-cell truth (r12)

Date: 2026-08-10

Base revision: `4ba2428d`

## Defect and correction

The centered health matrix treated any present snapshot with no matching
required condition as green `OK`, even after the snapshot's bounded provenance
expired. That allowed yesterday's absence of an outage condition to look like
current healthy evidence.

Status-cell evaluation is now a pure bounded state fold:

- a missing projection renders unavailable;
- an expired projection renders `Stale` in dim semantic ink;
- only active, required, matching-scope conditions affect warning/critical;
- a fresh informational expected absence remains non-outage; and
- only a fresh projection with no active required condition renders `OK`.

Resolved conditions no longer leak into current grading. The renderer consumes
the same pure state enum covered by the regression test.

## Focused farm proof

Machine 193 slot `ux013-stale-provenance-r1` passed the exact
`status_cells_keep_expected_absence_non_outage_but_refuse_stale_ok` test (1/1),
exact-file Rust formatting, and `git diff --check`. No broad shell suite was run.
