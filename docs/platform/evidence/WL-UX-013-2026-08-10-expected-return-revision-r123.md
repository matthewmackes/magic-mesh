# WL-UX-013 — revised expected-return publication (r123)

Date: 2026-08-10

Base revision: `45859dd3`

## Defect and correction

Runtime availability idempotency compared lifecycle state and reason but not
the requested expected-return duration. Revising a sleep or maintenance return
window could therefore reuse stale durable truth instead of publishing a new
generation. Idempotency now binds the duration represented by the retained
observed and expected timestamps. Changed durations advance generation; exact
retries remain stable, including saturating `u64::MAX` timestamps.

## Focused farm proof

Build VM `.90` (`172.20.0.90`), warmed slot
`ux013-revised-return-r2`, passed:

```text
cargo test -p mackesd --lib workers::node_availability::tests::runtime_publisher_publishes_revised_expected_return_deadline -- --exact --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 4671 filtered out
```

Targeted `rustfmt --check` and `git diff --check` passed.

Source SHA-256:

- `5ba4ff108b009212db573c78541b2f0ad95a6b29b53c83f209139a1dfc72b8bd`
  — `crates/mesh/mackesd/src/workers/node_availability.rs`

Installed-seat sleep/maintenance deadline transitions and integrated recovery
proof remain open.
