# WL-FUNC-021 — roaming owner release on workgroup-root loss

Date: 2026-08-06
Scope: `crates/desktop/mde-media-core/src/roaming.rs`
Gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=media-core-roaming-root-loss-r1 install-helpers/xcp-build.sh cargo test -p mde-media-core roaming --locked -- --nocapture`

## Audit finding

An already logged-in owner previously returned `PollOutcome::Offline` when the
workgroup root disappeared. That left an actively playing seat running without
a readable or writable lease, so another seat could acquire the session after a
remount while the old seat continued playing.

## Change

Active sessions now fail closed when the workgroup root is unavailable:

- `held_gen == 0` remains the pre-login `Offline` case.
- A session that already holds a lease returns `PollOutcome::Released`, pauses
  `Loading`/`Playing` once, and clears any deferred resume.
- The same release helper is used when a pending resume discovers that its lease
  has vanished, so a later pump cannot seek stale handoff state.

## Verification

The farm gate completed successfully on `.90`:

```text
running 18 tests
test result: ok. 18 passed; 0 failed; 0 ignored
```

The new regression `active_owner_yields_when_the_workgroup_root_disappears`
removes the shared root after login and playback start, then verifies the first
poll returns `Released`, pauses the player, and remains `Released` on the next
poll. Existing two-seat resume, owner-yield, failed-load, failed-seek, and
record-integrity tests also passed.

This is source/farm evidence only; live cross-seat remount and provider-loss
recovery remain separate acceptance gates.
