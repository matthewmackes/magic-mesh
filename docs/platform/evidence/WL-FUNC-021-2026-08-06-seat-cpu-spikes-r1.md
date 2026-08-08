# WL-FUNC-021 — common seat CPU investigation and mitigation

Date: 2026-08-06
Scope: read-only live investigation of seat `172.20.0.15` and Dell
`172.20.146.225`, followed by source-side bounded retry mitigations. Dell
runtime was not mutated.

## Finding

The persistent load is in `mackesd`, not Music playback. Both canonical seats
held one hot Tokio worker at roughly one full core while `mde-musicd` was idle
or only around 1–2% CPU and no `mpv` playback process was present. The live
pattern was common-mode and phase-aligned:

- `node_grade` sampled every 10 seconds and its workstation audio observation
  spawned several `runuser`/PipeWire/PulseAudio commands on every sample.
- `vehicle` and `airspace` both attempted the configured MG90 gateway/scan path
  every 5 seconds. The gateway was unavailable, so this repeatedly launched
  bounded SSH/curl/`iw` work.
- `nws_alert_overlay` logged “fresh same-host MG90 vehicle fix unavailable” at
  approximately 5-second intervals because its no-fix path had no backoff.
- The runtime status publisher sampled every 5 seconds and repeatedly rejected
  an unregistered worker; the rejection was a secondary diagnostic/publish
  amplifier. Syncthing and the shared Bus also showed churn and retention
  pressure, but were not the primary proof of the full-core daemon thread.

The live hot thread was a running `tokio-rt-worker`; seat tooling did not have
`perf`, `strace`, or `gdb`, so the exact inner future was not individually
symbolized. The cadence and child-process evidence identify the recurring
failure paths above without claiming a narrower stack trace than the hosts
provided.

## Source mitigation

The current source now:

- caches workstation audio health probes for 60 seconds, including failed
  probes, while retaining the 10-second health contract;
- backs off NWS no-fix and fetch failures to bounded 5/10/20/40/60-second
  retries, resetting immediately after a healthy response;
- backs off failed airspace surveys to the same bounded retry ladder, while
  preserving 5-second cadence after a ready survey;
- backs off failed or timed-out vehicle current-status probes to the same
  bounded retry ladder, while preserving normal current-status and heartbeat
  behavior after recovery.
- backs off rejected runtime-status samples from 5 seconds through a bounded
  60-second ceiling, while retaining the 5-second publication cadence after a
  valid supervisor snapshot. This prevents an invalid worker registration from
  repeatedly rebuilding the full status projection and Bus/file publication
  path.

Relevant implementation: `node_grade.rs`, `nws_alert_overlay.rs`,
`airspace.rs`, and `vehicle.rs` under
`crates/mesh/mackesd/src/workers/`, plus the runtime publisher in
`crates/mesh/mackesd/src/bin/mackesd/spawn.rs`.

These changes are source-only at this checkpoint. The installed seat remains
on `magic-mesh-12.1.6-4`; the current workspace release identity is newer, so
no live CPU improvement is claimed until a rebuilt package is explicitly
installed and observed.

## Verification

- BigBoy `172.20.0.130`, slot `cpu-spike-fix-nws`: file-scoped rustfmt passed
  for all four changed worker files.
- BigBoy focused tests passed: NWS `15/15`, node-grade `10/10`, airspace
  `13/13`, vehicle `58/58`.
- BigBoy full `mackesd` serial gate reached `4386 passed, 1 failed, 1
  ignored`; the sole failure was the unrelated pre-existing
  `workers::cloud::tests::a_missing_mutation_schema_is_rejected_before_any_backend_call`
  assertion. The changed worker suites passed within that run.
- Farm `cargo check -p mackesd --bin mackesd --features async-services --locked`
  passed after the runtime-status publisher backoff change. File-scoped
  rustfmt showed only the two pre-existing `spawn.rs` formatting regions; the
  new publisher block was formatter-clean.
- Farm `.90`, slot `cpu-status-retry-regression-r1`, passed the focused bounded
  retry-ladder regression: `1 passed, 0 failed`.
- Farm `.50`, slot `cpu-status-bin-check-r1`, passed the `mackesd` binary
  `cargo check --features async-services --locked` gate.

## Remaining proof

Package and install proof, post-install CPU sampling on both seats, and the
remaining Music/Media live loss, handoff, renderer, second-seat, and current
package checks remain open. The goal therefore stays active.
