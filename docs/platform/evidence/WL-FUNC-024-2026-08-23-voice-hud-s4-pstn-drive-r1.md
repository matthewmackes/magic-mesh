# WL-FUNC-024 S4 — voice-hud PSTN drive honesty — r1

Date: 2026-08-23
Classification: in-tree implementation + focused farm gate; **not** live
PSTN / LiveKit SIP / installed-seat production proof
Unit: `03c5a4b2bb89` (`cargo test -p mde-voice-hud`)
Write scope: `crates/services/mde-voice-hud/src/sip.rs` only

## Deliverable

The SIP core now fail-closes the leftover S4 planner/presentation seams that
could still look like a driveable PSTN without a governed credential, or
present the inbound sub as the ExternalTrunk From:

- `plan_pstn_agent` treats an empty inbound password the same as an absent
  provider (`ABSENT_PSTN_PROVIDER`). Username + registrar alone is not Ready.
- `outbound_pstn_from_uri` presents a fail-closed E.164 shared-outbound
  caller-ID on ExternalTrunk INVITEs; empty / peer / SIP-URI values fall back
  to the inbound AOR and never invent a PSTN From. The agent Dial path uses
  `place_call_presenting`.
- Inbound `Answer` starts RTP before 200 / `Established`. No SDP offer or a
  failed bind returns 480 and does not claim Established
  (`inbound_media_offer`).

Mute/DTMF on a live `MediaSession` were already wired. Live two-seat /
provider PSTN remains the parked leftover
(`WL-FUNC-024-2026-08-22-live-leftover-park-r1.md`); this crate cannot mint
that proof.

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mde-voice-hud
Admission: 63988420 KiB free (required 8388608 KiB)
Result: 64 passed, 0 failed (unittests); 0 doc-tests
Elapsed compile+test: ~1m 36s compile, 0.62s tests
```

New focused cases: empty-credential Unavailable, shared-outbound From
canonicalization, inbound offer fail-closed without SDP. Local `cargo fmt -p
mde-voice-hud` only.

## Leftover

A governed provider completing a live PSTN leg still depends on WL-FUNC-030
gateway.toml on a current-revision seat and the WL-REL-002 unpublished
candidate + red alert + 5s seat-mutation lock. Farm-green crate tests are
not that proof.
