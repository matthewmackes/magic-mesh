# WL-FUNC-022 weekday alarm DST scheduling — 2026-08-09 r2

## Correction

`mackesd` previously considered only one-time alarms and timers in its deadline queue, so every valid `Weekdays` alarm remained armed but could never ring. The Clock worker now resolves the most recent selected civil day through Jiff 0.2.21 and system IANA zoneinfo, applies the contract's earlier/later fold policy and next-valid gap policy, and uses the durable snapshot timestamp as the recurrence evaluation watermark.

Recurring alarms remain enabled after an occurrence. An occurrence identity is checked before mutation, so repeated ticks at the same instant are no-ops while a later selected civil day creates one distinct occurrence. New recurring schedules begin with their next selected day instead of fabricating a prior-week missed event.

## Files and identity

- `crates/mesh/mackesd/Cargo.toml` — workspace-pinned Jiff runtime dependency; SHA-256 `44d7fd13ca5c87d6ce4db68ae859d840c0337877155ae2b881023568bc1e4d8a`.
- `crates/mesh/mackesd/src/workers/clock.rs` — weekday deadline resolution, recurrence preservation, duplicate guard, and focused regression; SHA-256 `9f57851455b55cf33c79c2f2c8f2c924c7a3ef5b97e1699a6fdc5cd1571999fe`.

## BigBoy verification

Host `172.20.0.130`, slot `func022-clock-weekday-dst-r1-20260809`:

```text
cargo test -p mackesd --lib --features async-services weekday_alarm_resolves_dst_and_advances_once_per_selected_civil_day -- --nocapture
test workers::clock::tests::weekday_alarm_resolves_dst_and_advances_once_per_selected_civil_day ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 4354 filtered out
```

The broader shared-deadline gate used the same synced slot:

```text
cargo test -p mackesd --lib --features async-services workers::clock::tests -- --nocapture
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 4350 filtered out; finished in 0.30s
```

All six Clock worker tests passed: weekday DST scheduling, one-time/timer restart and durable audio replay, alarm snooze/stop, exact timer extension, first-received-late idempotency, and three-node delivery/loss/rejoin convergence. The focused regression proves the 2024 New York spring gap resolves to `03:30 EDT`, the fall fold selects `01:30 EDT` or `01:30 EST` according to policy, a repeated deadline pass is idempotent, the recurring schedule stays armed, and the following Sunday creates exactly one new occurrence. Exact-file `rustfmt --check` and scoped `git diff --check` passed on the same synced slot.
