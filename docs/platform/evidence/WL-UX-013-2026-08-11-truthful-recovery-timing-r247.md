# WL-UX-013 truthful recovery timing — 2026-08-11

- Scope: the installed `node_grade` worker's lifecycle merge now retains the
  last positive condition observation when a later sample detects recovery.
  `resolved_at_ms` records that later detection independently, preventing the
  canonical Bus history from claiming the condition persisted throughout an
  unobserved interval.
- Production path: provider observations → `node_grade` lifecycle merge →
  canonical node-health state → Bus snapshot and history projection.
- Farm: `172.20.0.90`, slot `1`.
- Focused gate:
  `workers::node_grade::tests::resolution_preserves_last_positive_observation_before_recovery`:
  PASS, 1 passed, 0 failed, 4,802 filtered out.
- Remaining epic boundary: complete expected-state transition coverage and
  physical-seat recovery proof.
