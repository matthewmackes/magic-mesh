# WL-FUNC-022 / WL-FUNC-021 fallback duck lease — r503

Date: 2026-08-13

## Production gap closed

Clock audio acquired the seat-wide 25-percent duck lease before starting a
Music source, but restored that lease before a governed bundled fallback began.
Both immediate provider/refusal fallback and the three-second inaudible-source
transition therefore played an active alert while Music and other seat streams
were already back at their original levels.

`ClockAudioAuthority` now transfers the existing lease to a successfully
started fallback renderer. The exact pre-alert Music gain and seat stream
levels remain retained until the matching Stop, Snooze, or renderer-loss
transition. If the fallback renderer itself cannot start, the authority still
restores those exact levels immediately and reports provider unavailability.
No queue, history, bookmark, catalog, or playback-owner state is mutated.

Focused regressions cover immediate typed-source failure, the exact 3,000 ms
silent-source deadline, successful terminal restoration, and unavailable
fallback output.

## Farm verification

- BigBoy `.130`, slot `func022-clock-fallback-test`:
  `cargo test -p mde-musicd clock_audio::tests:: -- --nocapture`
  — passed 12/12 Clock-audio library tests; 257 filtered, plus the main target
  with no matching tests.
- `.196`, slot `func022-clock-fallback-clippy-lib`:
  `cargo clippy -p mde-musicd --lib -- -D warnings`
  — passed.
- `.50`, slot `func022-clock-fallback-fmt`:
  `rustfmt --edition 2021 --check crates/services/mde-musicd/src/clock_audio.rs`
  — passed.
- `git diff --check` — passed.

The stronger all-target Clippy invocation reached the crate but was blocked by
eight pre-existing test-target warnings in `bus_responder.rs`, `cache.rs`, and
`queue.rs`, all outside this slice's authorized file scope. The production
library gate above passed strictly.

## Remaining acceptance

The first full release must include the daemon and Clock payloads. Per current
release policy, post-release non-blocking acceptance still needs installed-seat
proof for bundled/local/Music/podcast/NPR/radio sources, audible non-silent
output, network/provider loss and source deletion, daemon restart, simultaneous
Music playback with queue isolation, and exact seat-wide volume restoration.
