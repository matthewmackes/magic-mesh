# WL-FUNC-021 — bounded mid-stream provider reconnect fixture

Date: 2026-08-06
Scope: native `mde-musicd` decoder behavior after a provider TCP reset during
an audible track.

## Fixture

BigBoy `172.20.0.130` ran a localhost provider that:

- served a non-content-length WAV response whose declared track was longer than
  the bytes delivered;
- reset the first connection after emitting 4,800 decoded frames;
- accepted a second request for the same Subsonic `id=song-7`;
- returned a valid continuation response containing 2,400 frames.

The test set the audible playhead to 2,400 frames (50 ms at 48 kHz), leaving
decoded audio ahead of the playhead in the ring. The reconnect request carried
the production integer-second contract, `timeOffset=1`; the fixture then
verified that `discard_buffered_tail` reset the enqueued-frame count to the
audible position before continuation samples were added. No fallback request
was admitted.

## Verification

- Focused BigBoy test passed:
  `midstream_reset_reconnects_at_audible_offset_and_discards_ahead_buffer`.
- Full BigBoy `cargo test -p mde-musicd --locked` passed: `180/180` library
  tests, `0/0` binary tests, and `0/0` doctests.
- The fixture uses `socket2` only as a dev-dependency to create a portable TCP
  reset; production reconnect code and arbitrary direct/radio URL refusal are
  unchanged.

## Boundary

This is bounded decoder/provider proof, not live provider outage, audible
speaker judgment, package-install proof, or post-install CPU proof. Those
requirements remain open in the active worklist.
