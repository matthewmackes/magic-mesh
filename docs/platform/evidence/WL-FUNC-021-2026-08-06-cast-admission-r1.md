# WL-FUNC-021 cast admission boundary — 2026-08-06

`mde-media-core/src/cast.rs` now rejects oversized or control-character media
URLs/titles and control/whitespace-injected HTTP endpoints before the cast
network gate. The existing bounded discovery, finite resume, DLNA ordering,
and typed gated-outcome behavior remains intact.

Verification:

- BigBoy `.130`, slot `media-cast-hostile-boundary-20260806-r1`:
  `cargo test -p mde-media-core cast::tests:: -- --nocapture` passed **20/20**.
- Source SHA-256:
  `78e7c43306c2419e50d4df5e59ee288066a16f3932073c2399e495480f4744c9`.

Live renderer/Chromecast, mesh-owner, seat handoff, and rendered Music proof
remain open. Dell was not modified.
