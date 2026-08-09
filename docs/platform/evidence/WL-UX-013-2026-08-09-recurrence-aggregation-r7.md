# WL-UX-013 S3 — deterministic bounded recurrence aggregation (r7)

Date: 2026-08-09
Farm lane: BigBoy (`172.20.0.130`), slot `ux013-recurrence`

## Implementation

- Recent History now aggregates resolved records by the health contract's stable
  lifecycle identity within the selected node. One identity occupies one row and
  the row reports whether it occurred once or the exact recurrence count.
- Paint-time storage remains fixed at eight borrowed rows. A first pass retains
  only the strongest eight identities; a second pass counts occurrences only for
  those retained identities, so a hostile oversized input cannot make modal
  allocation grow with history size.
- Representative selection is deterministic: severity, longest resolved
  duration, stable ID, latest resolution, latest observation, and stable textual
  evidence tie-breakers. Reversing input order therefore cannot change the page.
- The modal continues to consume the existing signed health snapshot and does
  not create a second history or health authority.

## Focused verification

```text
cargo test --quiet -p mde-shell-egui \
  health_modal::tests::recurrence_aggregation_is_bounded_complete_and_order_independent \
  -- --exact
1 passed; 0 failed; 1503 filtered out

rustfmt --edition 2021 --check \
  crates/desktop/mde-shell-egui/src/health_modal.rs
exit 0
```

The hostile fixture contains 32 distinct matching warnings, four occurrences of
one critical identity, an equal-duration/latest-resolution tie, a wrong-node
record with the same identity, and reversed input order. It proves the exact
eight-row cap, complete same-node count, cross-node exclusion, deterministic
representative, and one-row-per-identity result. Scoped `git diff --check`
passed. Source SHA-256:
`54b0119604479591786d456e2f665e2154854ff4456a8c22694aa1a6eaffbcae`.

## Remaining S3 limits

Durable 24-hour storage, explicit paging/filter controls, restart behavior,
packaging, and five-seat live proof remain open.
