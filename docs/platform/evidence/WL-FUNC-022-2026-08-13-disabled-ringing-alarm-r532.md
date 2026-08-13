# WL-FUNC-022 disabled ringing-alarm authority — r532

Date: 2026-08-13

## Production gap closed

`SetScheduleEnabled { enabled: false }` previously removed scheduled snooze
children but left an already-ringing alarm and its Music effect active. The
daemon now terminally acknowledges every exact ringing occurrence in the same
durable Clock commit. The existing transition/outbox authority consequently
publishes one generation-bound Music `Stop`; one-time alarms, which are already
disabled when they ring, are also silenced by an explicit disable command.
Duplicate signed command delivery remains replay-closed.

Owned implementation scope:

- `crates/mesh/mackesd/src/workers/clock.rs`
- this evidence record

## Farm evidence

- BigBoy `172.20.0.130`, slot `func022-disable-ring-test-r1`:
  `cargo test -p mackesd --lib --features async-services workers::clock::tests::disabling_a_ringing_alarm_atomically_stops_its_exact_audio_generation -- --exact --nocapture`
  passed **1/1** (`4968` filtered), including durable terminal state, exact
  occurrence/global-event/generation binding, one Music Stop, and replay closure.
- `172.20.0.170`, slot `func022-disable-ring-clippy-r1`:
  `cargo clippy -p mackesd --lib --features async-services -- -D warnings`
  passed.
- BigBoy `172.20.0.130`, slot `func022-disable-ring-fmt-r2`:
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/clock.rs`
  passed. The workspace-wide format probe was not used as evidence because it
  reported unrelated concurrent edits outside this slice.
- Dev-host non-build checks: `git diff --check` passed. The dev host has no
  `cargo`/`rustfmt`; all compilation and formatting authority above is farm-run.

## Remaining WL-FUNC-022 acceptance

The epic still requires the deferred post-first-release UI/package/live proof:
deterministic four-section Clock captures and complete action traces; Bottom and
Left direct-DRM clock/bell/banner/curtain proof; physical audio source,
duck/restore, audibility, and fallback metrics; fresh-install/non-importing
upgrade RPM payload proof; and the reduced one-node restart/rejoin/recovery
acceptance. This slice closes only the disabled-ringing authority hole.
