# WL-FUNC-022 — local stopwatch monotonic authority (r540)

## Production gap and result

The pre-release audit covered the explicit Clock surface, persisted alarm/timer
scheduler, selected-peer convergence, governed Clock audio, and stopwatch
criteria. Existing evidence already covers the alarm/timer deadline, replay,
audio fallback, payload binding, peer convergence, and retained-action
boundaries. The strongest remaining executable production gap was local
stopwatch progression: the shell persisted `started_monotonic_ms`, but Pause,
Lap, and the live display used wall time alone. An NTP or operator wall-clock
rollback could therefore erase elapsed time.

`crates/desktop/mde-shell-egui/src/timers.rs` now computes a local running
stopwatch's elapsed interval from its monotonic start. It uses wall time only
for a foreign mirror, whose monotonic value belongs to another host, or after a
reboot replaces the local monotonic epoch. Pause and Lap preserve the resulting
elapsed value before clearing/restarting their clock anchors.

The hostile regression moves wall time backward by 30 seconds while monotonic
time advances by five seconds. Both Pause and Lap retain the exact seven-second
total (two persisted plus five live), and Lap records that exact split/total.

## Farm gates

- `172.20.0.170`, slot `func022-stopwatch-monotonic-test-r540`:
  `cargo test -p mde-shell-egui timers::tests::local_stopwatch_pause_and_lap_ignore_backward_wall_clock_correction -- --exact --nocapture`
  passed 1/1 (1,607 filtered out).
- `172.20.0.170`, same warmed slot:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui --no-deps -- -D warnings`
  passed. The package itself is strict-clean.
- `172.20.0.196`, slot `func022-stopwatch-monotonic-build-r540`:
  `cargo build -p mde-shell-egui --bin mde-shell-egui` passed.
- `172.20.0.196`, slot `func022-stopwatch-monotonic-filefmt-r540`:
  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/timers.rs`
  passed.
- Scoped `git diff --check` passed.

One broader strict Clippy run was recorded red after reaching an unrelated
concurrent edit: `crates/desktop/mde-vdi-rdp/src/session.rs:354` reported unused
`begin_connection_generation`. It is outside this slice's ownership. The
package-scoped `--no-deps` strict run above then passed; no duplicate broad gate
was launched. Package-wide `cargo fmt -p mde-shell-egui -- --check` likewise
reported only concurrent formatting drift in `src/iac/mod.rs:109`; the exact
owned Clock file passed.

## Remaining acceptance

Pre-release residuals remain the first signed release/package integration and
any still-uncovered Clock UI/audio/mesh implementation gaps found by subsequent
audit. After that release, deferred non-blocking acceptance remains: direct-DRM
responsive Clock captures, physical audible output and exact duck/restore,
fresh-install/non-importing upgrade, and one-node alarm/timer/stopwatch recovery
with selected-peer/lighthouse behavior where available.
