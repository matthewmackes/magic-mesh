# WL-FUNC-021 stale Resume authority revocation — 2026-08-13

- Scope: `mde-musicd` accepts Resume only while the current engine still owns
  active decoding or a retained decoded audio tail.
- Production boundary: a delayed MPRIS/Bus Resume can no longer recreate
  `playing` authority after provider exhaustion, Stop, or renderer loss.
  A finite track that completed decoding while paused remains resumable while
  its exact buffered tail is retained. The renderer independently observes the
  Stop authority, and Stop performs a final projection revocation after joining
  the decoder, closing concurrent Resume races without emitting stale audio.
- Hostile provider gate: `MCNF_BUILD_HOST=172.20.0.170
  MCNF_BUILD_SLOT=func021-resume-test install-helpers/xcp-build.sh cargo test -p
  mde-musicd provider_failure_clears_playing_authority_after_decode_exits --
  --nocapture` — **PASS**, 1 passed, 0 failed, 269 filtered out.
- Retained-tail gate: `MCNF_BUILD_HOST=172.20.0.170
  MCNF_BUILD_SLOT=func021-resume-test install-helpers/xcp-build.sh cargo test -p
  mde-musicd fully_decoded_paused_tail_retains_resume_authority -- --nocapture`
  — **PASS**, 1 passed, 0 failed, 269 filtered out.
- Strict gate: `MCNF_BUILD_HOST=172.20.0.196
  MCNF_BUILD_SLOT=func021-resume-clippy install-helpers/xcp-build.sh cargo clippy
  -p mde-musicd --lib -- -D warnings` — **PASS**.
- Formatting: crate-wide `cargo fmt -p mde-musicd -- --check` reached the crate
  on `.170` and exposed existing drift in `bus_responder.rs`, `domain.rs`,
  `state.rs`, and older untouched `engine.rs` lines. This slice did not absorb
  that unrelated formatting cleanup; its changed hunks are rustfmt-conformant.
- Remaining acceptance: first-release packaging followed by the deferred,
  non-blocking installed-seat provider-switch/loss, queue continuity, restart,
  and audible playback proof.
