# WL-UX-013 — expired expected-absence evidence

- Date: 2026-08-10
- Farm host: `172.20.0.130` (BigBoy)
- Farm slot: `ux013-expected-absence-r153`
- Gate: `cargo test -p mackesd --lib workers::node_availability::tests::runtime_publisher_does_not_republish_expired_durable_expected_absence -- --nocapture`
- Result: 1 passed, 0 failed

Expired durable expected-state intents remain available for audit but are not
republished as current health truth; a newer returned transition is the only
projected Bus row.
