# WL-FUNC-021 media hardening checkpoint (2026-08-06)

## Scope

The media-core roaming and cast boundaries now fail closed on hostile replicated
rows, yielded or exhausted leases, malformed discovery records, invalid network
endpoints, duplicate targets, oversized metadata, and non-finite seek input.
The live-seat verifier also checks installed RPM integrity while allowing only
the known runtime-mutated secret-helper path.

## Farm verification

- BigBoy `.130`, slot `music-media-hardening-20260806-r3`:
  `cargo test --locked -p mde-media-core -- --nocapture` passed **250/250**;
  doc-tests passed **1/1**.
- `.90`, slot reported by the cast audit:
  `cargo test -p mde-media-core cast::tests:: -- --nocapture` passed **24/24**
  before the full BigBoy gate.
- `git diff --check` passed. The full media-core format check remains
  unavailable because the inherited `roaming.rs` formatting drift is outside
  the scoped cast audit; no formatting claim is made here.

## Proof helpers

The four bounded helper self-tests passed: live-seat RPM/payload validation,
Music DRM frame validation, network-loss loopback, and cast loopback. The
network-loss fixture reported zero fallback requests and cleaned its temporary
trace. No physical Chromecast, DLNA renderer, mesh cast receiver, or second
live seat was present, so those live claims remain open.

## Files

- `crates/desktop/mde-media-core/src/roaming.rs`
- `crates/desktop/mde-media-core/src/cast.rs`
- `install-helpers/verify-music-live-seat.sh`

## Limitations

This is source/fixture proof. WL-FUNC-021 still needs live provider network-loss
recovery, owner-yield/resume continuity, rendered acceptance, and current
installed-seat package proof.
