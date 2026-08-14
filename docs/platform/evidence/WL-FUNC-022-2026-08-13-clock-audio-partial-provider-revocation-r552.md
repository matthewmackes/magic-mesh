# WL-FUNC-022 S3 — partial external Clock renderer revocation

Date: 2026-08-13  
Branch: `agent/drain-worklist-20260725`  
Scope: `crates/services/mde-musicd/src/clock_audio.rs`

## Production result

The Music-owned Clock audio authority now revokes an external-source renderer
when `start_music` reports failure before it opens the governed bundled
fallback. A provider that fails after allocating or starting its independent
renderer therefore cannot overlap the fallback or replace it later through a
delayed callback.

The hostile regression models failure after the Music renderer has started and
proves that:

- the partial external renderer is revoked exactly once before fallback;
- the fallback starts while the Music queue, history, and bookmarks retain
  their original generations;
- exact request replay neither restarts nor revokes the fallback;
- Music and other seat streams remain at exactly 25 percent while the fallback
  rings; and
- Stop restores the exact pre-alert Music gain and seat-stream levels.

This closes an executable S3 handoff gap. It does not claim physical audibility,
provider-network, package, or installed-release acceptance.

## Farm gates

- Exact hostile regression — `172.20.0.90`, slot `1`:
  `cargo test -p mde-musicd clock_audio::tests::partial_external_start_is_revoked_before_queue_isolated_fallback -- --exact --nocapture`
  passed `1/1` (`273` filtered).
- Strict Clippy — BigBoy `172.20.0.130`, slot `3`:
  `cargo clippy -p mde-musicd --all-targets --all-features -- -D warnings`
  reached the crate and reported no warning in the owned Clock-audio scope, but
  the crate-wide gate is not green: it stopped on eight pre-existing warnings
  in unrelated `bus_responder.rs`, `cache.rs`, and `queue.rs`.
- Build — `172.20.0.170`, slot `1`:
  `cargo build -p mde-musicd --all-targets --all-features` passed (`dev`
  profile, all targets built successfully).
- Scoped `git diff --check` passed.

## Remaining FUNC-022 acceptance

- Complete first-release package installation with governed tone/catalog
  payloads.
- After that release, run the deferred non-blocking one-node provider loss,
  three-second fallback, simultaneous Music playback, PipeWire duck/restore,
  restart, and physical-audio acceptance.
