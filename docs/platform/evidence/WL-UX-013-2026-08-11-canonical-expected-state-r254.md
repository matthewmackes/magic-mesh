# WL-UX-013 canonical expected-state transition — 2026-08-11

- Scope: the installed `node_grade` worker now reads each canonical node's
  no-symlink, bounded runtime availability intent and supplies the shared policy
  assessment to the canonical health fold. Declared absence is informational
  and grade-neutral; missed return escalates through warning and critical
  thresholds. The fold emits a scoped condition but never fabricates the absent
  node as a fresh publication. Invalid, oversized, identity-switched, or
  symlink-traversing records cannot explain missing health.
- Production path: host/network lifecycle intent → secure availability record →
  `node_grade` canonical fold → snapshot file and Bus publication.
- Farm: `172.20.0.90`, slot `ux013-expected-r248`, small shape.
- Focused gate:
  `workers::node_grade::tests::declared_seat_absence_escalates_only_after_its_return_deadline`:
  PASS, 1 passed, 0 failed, 4,806 filtered out (`--features async-services
  --locked --exact`).
- Remaining epic boundary: physical-seat suspend/loss/return proof, governed
  recovery, and remaining history/export coverage.
