# WL-FUNC-021 — retained provider capability truth (2026-08-06)

## Goal slice

Make the daemon-owned Music source record distinguish provider authentication
failure from an ordinary transport outage. A failed Subsonic browse response
with API code 40 (wrong credentials) or 41 (token expired) now marks the
retained source `authentication_required=true` and `reachable=false`. A
successful browse clears the auth-required state, marks the source reachable,
and records the successful typed feature. Malformed replies and transport
failures still mark reachability false without inventing an authentication
claim. This keeps setup/offline/error UI state grounded in the same retained
source authority as catalog rows and bookmark shelves.

## Farm verification

- `.90`, `MCNF_BUILD_SLOT=music-capability-focus-r2`: `cargo test -p mde-musicd
  authentication_required` — 1 passed, 0 failed.
- `.90`, `MCNF_BUILD_SLOT=music-capability-full-r2`: `cargo test -p mde-musicd`
  — 165 passed, 0 failed; doctests — 0 passed, 0 failed.
- `.50`, `MCNF_BUILD_SLOT=music-capability-fmt-r2`: `cargo fmt -p mde-musicd
  -- --check` — passed.
- BigBoy `.130`, `MCNF_BUILD_SLOT=music-capability-bigboy-r1`: the required
  long-pole retry reached the sync stage but returned `Connection timed out`;
  no BigBoy result is claimed for this slice.
- Local `git diff --check` and canonical governance lints remain clean after
  the source/worklist/evidence refresh.

## Honest remaining boundary

This is fixture-backed source-state evidence. It does not claim full provider
feature negotiation beyond observed browse/auth state, live credentials,
provider podcast/audiobook playback, GUI setup rendering, two-catalog outage
acceptance, target/DLNA handoff, direct DRM, or Dell runtime installation.

## Source integrity

Hashes are recorded after the final Worklist/evidence refresh and the review
bundle synchronization.

```text
4f7e2671cde7b6516a03a2f0fbc606a70054080e95d947e2c07369859b3a9d0c  crates/services/mde-musicd/src/bus_responder.rs
3b1da0fac147495de76a73783544e3fe1cfd2f0b2d8e083636554bcc04e2727e  docs/platform/WORKLIST.md
```
