# WL-FUNC-021 — finite target-side handoff seek fixture

Date: 2026-08-06
Scope: native `mde-musicd` target playback start after an owner-yield
completion supplies a finite-track position.

## Fixture

BigBoy `172.20.0.130` served a bounded finite WAV track over localhost. The
test invoked the same `EngineHandle::play_from_candidates_at` seam used by the
target-side handoff completion path with a 50 ms starting position. The
decoder applied the pending seek before consuming its first audio packet,
retained the 50 ms playhead, and enqueued non-silent audio from the requested
position onward.

## Verification

- Focused BigBoy test passed:
  `finite_handoff_start_seeks_before_decoding_audio`.
- Full BigBoy `cargo test -p mde-musicd --locked` passed: `181/181` library
  tests, `0/0` binary tests, and `0/0` doctests.
- File-scoped engine formatting showed only pre-existing formatting regions;
  the new fixture is formatter-clean. `git diff --check` passed.

## Boundary

This proves the finite decoder position-continuity seam, not live cross-seat
owner-yield delivery, target hardware output, DLNA/Chromecast availability, or
package-install acceptance. Those requirements remain open in WL-FUNC-021.
