# WL-FUNC-019 / WL-CRIT-007 — seat remote-input Bus recovery (r23)

Date: 2026-08-09

Production source: `crates/mesh/mackesd/src/workers/seat_remote_input.rs`

Source SHA-256:
`2a96d81a10a6426533b810b4eb5f437ae8ccf29e3bcb73381cf69e59cdb80ebf`

## Correction

`SeatRemoteInputWorker` no longer terminates successfully when its shared Bus
is absent or unopenable during daemon startup. Explicit roots remain exact;
otherwise normal mde-bus resolution is used with the documented
`mde_bus::SYSTEM_BUS_ROOT` service fallback. Startup retries at the configured
tick clamped to 10 ms–2 s, and shutdown interrupts every retry wait.

Bus open and successful tail reads for both the input and arm-control topics
now form one fail-closed activation boundary. The worker starts explicitly
disarmed at those tails. Retained arm grants and retained input commands can
therefore neither re-arm a seat nor inject input after daemon restart. Once the
same worker activates, a fresh signed arm followed by one fresh signed input is
consumed and injected exactly once.

## Focused farm proof

Host: machine 196 (`172.20.0.196`)

Slot: `seat-remote-input-bus-r23`

```text
cargo test -p mackesd --features async-services --lib \
  workers::seat_remote_input::tests::remote_input_bus_root_preserves_override_and_has_system_fallback \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,431 filtered out`.

```text
cargo test -p mackesd --features async-services --lib \
  workers::seat_remote_input::tests::late_bus_recovery_skips_retained_controls_and_applies_forward_input_once \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,431 filtered out`. The same worker remained
alive behind an unopenable root, activated when the retained Bus returned,
gave retained signed arm/input controls zero injection effects, and injected
one forward signed input exactly once after fresh consent.

The exact source passed remote `rustfmt --edition 2021 --check` and local scoped
`git diff --check`. No broad suite, package build, live-seat injection, or
unrelated test was run.
