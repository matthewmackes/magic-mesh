# WL-FUNC-021 — seat snapshot startup phase (2026-08-07)

## Scoped mitigation

`crates/desktop/mde-shell-egui/src/seat_pump.rs` now delays the first expensive
seat snapshot by a deterministic host-derived phase bounded to 0–1500 ms. The
existing five-second refresh cadence is unchanged after startup, and the
shutdown receiver remains interruptible while the phase is pending. This keeps
identical seats from issuing their first DDC/PipeWire probe at one shared
boundary without adding unbounded startup latency or process-local randomness.

## Verification

BigBoy farm host `.90`, slot `seat-pump-phase-r1`:

```text
cargo test -p mde-shell-egui seat_pump --locked -- --nocapture
running 7 tests
test result: ok. 7 passed; 0 failed; 1449 filtered out
```

The changed file also passed a file-scoped pinned-toolchain rustfmt check on
farm host `.50`, slot `seat-pump-fmt-r1`, and `git diff --check` passed locally.

This is source/farm evidence only. Installed-seat CPU sampling and live
multi-seat synchronization proof remain open until Dell is reachable.
