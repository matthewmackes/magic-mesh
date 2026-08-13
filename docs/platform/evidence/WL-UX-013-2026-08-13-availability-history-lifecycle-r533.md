# WL-UX-013 availability history lifecycle — r533

Date: 2026-08-13

## Production change

The System and Mesh Health authority now carries synthetic expected-absence and
missed-return conditions through the previous canonical fleet snapshot. A
refresh or severity escalation preserves the exact incident onset,
acknowledgement, and snooze state. When the publisher returns, the authority
moves that condition into bounded resolved history while preserving its last
positive observation rather than silently deleting it.

Restart recovery reads only the observer-bound, roster-bound, regular,
byte-bounded canonical snapshot and admits it only when it was valid at its own
generation time. Resolved availability history is pruned to the six-hour
privacy epoch and the shared condition-capacity bound.

Owned production file:

- `crates/mesh/mackesd/src/workers/node_grade.rs`

## Farm gates

- `172.20.0.90`, slot `ux013-history-lib-clippy-r2`:
  `cargo test -p mackesd --lib --features async-services availability_lifecycle_survives_refresh_and_records_return_history -- --nocapture`
  passed 1/1 (4,971 filtered out).
- `172.20.0.90`, slot `ux013-history-lib-clippy-r2`:
  `cargo clippy -p mackesd --features async-services --lib -- -D warnings`
  passed.
- `172.20.0.50`, slot `ux013-history-file-fmt-r3`:
  exact-file Rust 2021 `rustfmt --check` passed after applying the formatter to
  the owned health module.
- Local `git diff --check` passed.

The attempted all-target mackesd Clippy and package test did not reach this
slice because concurrent, unowned App-front-door work in
`workers/peer_app_launch.rs:922` referenced a missing `wall_now_ms` in its test
configuration. The production-library Clippy gate above is green. No excluded
file was modified for this slice.

## Remaining WL-UX-013 acceptance

- Complete any remaining production publisher/transition integrations not yet
  represented by the canonical expected-state and health contracts.
- Run the deferred post-release one-node physical transition proof for
  boot/sleep/network/maintenance/outage/rejoin, including modal rendering and
  absence of duplicate Health authority.
- Retain the already-required post-release package, direct-transition capture,
  and live-seat evidence; these proofs are non-blocking before the first full
  release under the operator directive.
