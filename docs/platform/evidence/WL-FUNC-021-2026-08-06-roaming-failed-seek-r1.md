# WL-FUNC-021 — roaming failed-seek recovery (2026-08-06)

Status: bounded owner-yield/target-resume regression covered; live peer
propagation, physical target control, and installed-seat acceptance remain
`Remaining`.

## Change

`RoamingSession::apply_pending` no longer clears a target resume request when
`Player::seek` is rejected. The target is paused best-effort so a failed
handoff cannot continue from position zero, and the pending request remains
retryable. A failed pause after a successful seek also leaves the request
pending rather than claiming that the source's paused intent was applied.

The new `failed_target_seek_stays_paused_and_retries_the_resume` fixture uses a
test-only engine wrapper that rejects the first seek. It proves that the target
stays paused at zero with the resume retained, then accepts the retry and lands
at the original 45-second position.

## Focused verification

- BigBoy `.130`, slot `media-roaming-failed-seek-test-r1`:
  `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=media-roaming-failed-seek-test-r1
  ./install-helpers/xcp-build.sh cargo test -p mde-media-core
  roaming::tests::failed_target_seek_stays_paused_and_retries_the_resume
  --locked -- --nocapture` — **1 passed, 0 failed** (252 filtered).
- `.50`, slot `media-roaming-failed-seek-fmt-r2`: the crate format check reports
  only three inherited diffs in older tests at the lease-unavailable, owner-yield,
  and prior pending-resume assertions; the new wrapper and regression are
  formatter-clean.
- `git diff --check -- crates/desktop/mde-media-core/src/roaming.rs` — passed.

Source SHA-256 at capture:

```text
7fd415e585604680cd3414cffb19a4192dff13cb1f8cc529ce9a1b3b54fef54d  crates/desktop/mde-media-core/src/roaming.rs
```

## Remaining boundary

This is a deterministic engine-seam fixture, not proof of a live Syncthing
two-seat handoff or a physical/DLNA/mesh target. Those acceptance paths remain
open and are not inferred from this test.
