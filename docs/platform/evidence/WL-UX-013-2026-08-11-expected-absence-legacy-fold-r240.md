# WL-UX-013 expected-absence legacy health fold — 2026-08-11

- Scope: the production peer-health reconciler reads the bounded durable node
  availability intent before classifying a missing heartbeat. A valid planned
  absence no longer appears as an outage in the legacy projection; a missed
  expected return escalates through the shared device-class policy. Invalid,
  mismatched, future, and oversized evidence remains excluded.
- Farm: `172.20.0.90`, slot `2`.
- Focused gates:
  - `workers::health_reconciler::tests::expected_absence_prevents_missing_heartbeat_from_becoming_outage`: PASS, 1 passed, 0 failed, 4,791 filtered out.
  - `workers::health_reconciler::tests::missed_expected_return_escalates_instead_of_suppressing_outage`: PASS, 1 passed, 0 failed, 4,795 filtered out.
