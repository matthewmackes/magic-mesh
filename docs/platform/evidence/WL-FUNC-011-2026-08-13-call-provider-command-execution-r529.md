# WL-FUNC-011 Calls provider command execution evidence — 2026-08-13

- Scope: bind authorized local Calls commands to exactly one deterministic
  registered provider before signed call state is authored. A proof-only or
  failing provider cannot mint start/answer/mute/DTMF state; decline and hang-up
  remain available after provider loss so revocation cannot be trapped.
- Production path: `CollabWorker::drain_commands` now invokes the provider
  registry's command executor after capability admission. Audio prefers WebRTC,
  then LiveKit, then SIP; other call kinds use only compatible adapters. A
  provider failure is bounded, visible in daemon diagnostics, and does not fall
  through to a second provider or append a call event.
- Hostile focused gates on `.90` (`172.20.0.90`), workspace
  `func011-call-exec-tests-r529c`:
  - `cargo test -p mackesd --lib workers::collab_media::tests::execution_is_single_provider_fail_closed_and_cleanup_survives_loss -- --exact --nocapture`
    — **PASS**, 1 passed, 0 failed.
  - `cargo test -p mackesd --lib workers::collab::tests::proof_only_provider_failure_never_authors_call_state -- --exact --nocapture`
    — **PASS**, 1 passed, 0 failed.
  - `cargo test -p mackesd --lib call_media -- --nocapture` — **PASS**, 1
    existing end-to-end provider/readiness test passed.
- Strict crate gate on `.90`, workspace `func011-call-exec-clippy-r529b`:
  `cargo clippy -p mackesd --all-targets -- -D warnings` — **PASS**.
- Owned-file format gate on `.196` (`172.20.0.196`), workspace
  `func011-call-exec-fmt-r529d`: `rustfmt --edition 2021 --check
  crates/mesh/mackesd/src/workers/collab.rs
  crates/mesh/mackesd/src/workers/collab_media.rs` — **PASS**.
- Remaining S4 boundary: register and package a concrete production media
  adapter, execute remote inbound signaling and consented control/revocation,
  then perform the deferred post-release one-node live media fixture. This slice
  does not claim provider availability or live media proof.
