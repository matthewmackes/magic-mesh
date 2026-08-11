# WL-UX-013 action-result replay conflict evidence — 2026-08-11

- Scope: durable health-action restart recovery now treats an `audit_id` as an
  exact typed-result identity, not merely a presence marker.
- Boundary: result-lane rows are validated before comparison. A row acknowledges
  publication only when the complete `HealthActionResult` equals the durable
  journal result. A conflicting request identity, conflicting body, or multiple
  unequal bodies for one audit ID fails closed without advancing the action
  cursor, re-executing the action, or deleting the genuine journal.
- Focused farm gate:
  - Host: BigBoy (`172.20.0.130`), slot 3.
  - Command: `cargo test -p mackesd --features async-services workers::node_grade::tests::conflicting_same_audit_result_cannot_acknowledge_restart_replay -- --exact --nocapture`
  - Result: PASS — 1 passed, 0 failed, 4,816 filtered.
- `git diff --check` passed.
- Remaining proof: physical-seat remediation and three-seat corrected-forward
  recovery remain part of the epic's live acceptance boundary.
