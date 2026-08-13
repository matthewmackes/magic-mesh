# WL-FUNC-021 — renderer-backed handoff commit (r539)

Date: 2026-08-13

## Executable gap

The target-side Music handoff previously published `playing=true` and consumed
its one-use intent/completion immediately after spawning the decoder. A valid
source could therefore buffer audio, fail before the physical output callback,
and still appear to have completed the cross-seat handoff.

## Production result

- `EngineHandle` now exposes a monotonic count of frames actually emitted by
  the physical renderer callback.
- Target handoff is a two-phase commit: it retains the exact completion and
  prior queue after decoder start, then publishes ownership and consumes the
  authorization only after that renderer generation advances.
- Renderer revocation or silent source termination before the first emitted
  frame stops the attempted playback, restores the prior queue, and retains the
  transfer for an honest retry/source-side lease recovery.
- Ordinary playback heartbeat/workspace publication is suppressed while the
  target commit is pending, so no parallel projection can bypass the emitted
  audio requirement.

## Farm gates

- `.50`, slot `func021-handoff-r539`: focused regression
  `cargo test -p mde-musicd bus_responder::tests::target_handoff_commits_only_after_physical_renderer_progress -- --exact --nocapture`
  passed 1/1.
- `.90`, slot `func021-handoff-clippy-r539`: strict production-library Clippy
  `cargo clippy -p mde-musicd --lib -- -D warnings` passed.
- `.170`, slot `func021-handoff-build-r539`: relevant all-target build
  `cargo build -p mde-musicd --all-targets` passed.
- Scoped `git diff --check` passed for both modified production files.
- `cargo fmt -p mde-musicd -- --check` was run on `.50`; it reports existing
  formatting drift in untouched regions of `bus_responder.rs`, `domain.rs`,
  `engine.rs`, and `state.rs`. No broad formatting rewrite was made because
  this slice owns only the handoff behavior. Strict all-target Clippy likewise
  reached eight existing test-only warnings outside this slice; the strict
  production-library gate above is green.

## Remaining acceptance

FUNC-021 still needs the first-release-installed physical renderer/audio proof,
DLNA and Chromecast loopback/hardware proof, and one-node seat handoff/recovery
acceptance. Those live proofs remain deferred and non-blocking until after the
first full release. This slice does not claim that hardware acceptance.
