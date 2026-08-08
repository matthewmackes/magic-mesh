# WL-FUNC-021 — Music final progress finalization (2026-08-06)

## Scope

The Music daemon now uses one source-admission resolver for explicit typed
`scrobble` and daemon-owned final progress boundaries. Typed `pause` and `stop`
attempt a final progress write for the current queue item before changing
transport state; the serve loop makes the same best-effort write when it closes.
The resolver uses the explicit retained `ContentRef` when supplied, otherwise
projects the current queue id through the bounded catalog, then requires the
matching admitted provider client. An unavailable provider is logged with a
redacted boundary/source diagnostic and does not prevent the requested pause or
stop. The unsupported `transfer` action remains refused until target handoff
authority is migrated, so no transfer parity is claimed.

This keeps provider writes behind the daemon's existing queue/engine/Bus
authority and prevents a source-less queue id from being written to an
unrelated configured provider.

## Farm verification

- `.50`, slot `music-progress-fmt-r1`: `cargo fmt -p mde-musicd -- --check`
  passed.
- `.90`, slot `music-progress-focused-r1`: the hostile
  `bus_responder::tests::typed_scrobble_uses_the_selected_admitted_provider`
  regression passed 1/1; the selected provider succeeded and an unadmitted
  identity was refused.
- BigBoy `.130`, slot `music-progress-full-r1`: `cargo test -p mde-musicd
  --lib` passed 161/161 with 0 failures.
- BigBoy `.130`, slot `music-progress-full-r1`: `cargo test -p mde-musicd
  --doc` passed 0/0 with 0 failures.
- Local `git diff --check` passed after the farm wave.

## Source integrity

The implementation source was checksum-verified before staging:

```text
7719058e7c734ec12a62817b58d4cd43a24db74a9882dc4353e6e6a223866e7f  crates/services/mde-musicd/src/bus_responder.rs
```

This is fixture-backed typed provider-admission evidence. It is not live
two-catalog provider acceptance, audible playback proof, target/DLNA handoff,
GUI-worker removal, or Dell runtime acceptance.
