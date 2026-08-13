# WL-FUNC-011 — explicit outbound dialing and acknowledged mute (r536)

Date: 2026-08-13

## Production result

- Added the typed `start_outbound_call` collaboration command with an explicit,
  bounded SIP/P2P target. Empty, padded, control-bearing, whitespace-bearing,
  oversized, and unsupported-character targets fail at the signed collaboration
  authority.
- Collaboration authorization and event derivation now happen before any media
  provider side effect. Provider failure still prevents durable event commit.
- The concrete SIP provider lowers an authorized target to the voice agent and
  waits for a correlated result; queue acceptance is not reported as call
  success.
- The voice agent routes mesh names directly over the overlay and external
  numbers through the configured trunk, establishes SIP and duplex RTP before
  acknowledging, retains the outbound dialog for BYE, and bounds ringing by the
  requested 30-second deadline.
- Mute now changes the live `MediaSession`, reads the resulting state back, and
  returns that observed state to the provider. Missing media, timeout, agent
  failure, and mismatched acknowledgement all fail closed without minting a
  muted collaboration fact.

## Verification

- `.50`, explicit slot 1: `cargo test -p mde-collab-core
  outbound_call_requires_a_bounded_explicit_target -- --nocapture` — passed
  1/1.
- `.196`, explicit slot 1: the same final-source core authorization test —
  passed 1/1 after target-character tightening.
- `.196`, explicit slot 1: `cargo test -p mackesd --features async-services
  concrete_sip_provider_is_bounded_health_checked_and_fail_closed --
  --nocapture` — passed 1/1.
- `.50`, unique `func011-voice-route-r536` slot: `cargo test -p
  mde-voice-hud direct_peer_user_does_not_duplicate_an_explicit_host --
  --nocapture` — passed 1/1.
- `.170`, unique `func011-dial-clippy-r536` slot: `cargo clippy -p mackesd
  --features async-services --all-targets -- -D warnings` — passed on final
  source.
- `.90`, unique `func011-dial-build-r536` slot: `cargo build -p mackesd
  --features async-services` — passed on final source.
- Owned-file Rust 1.94 formatting and `git diff --check` passed locally; this is
  the permitted small formatting/probe lane, not a local build.

## Capacity and cleanup

No new BigBoy work was started after its low-space advisory. The blocked owned
`.130` slot-2 build and descendants were stopped; only
`/home/mm/magic-mesh-farm-2` was removed after ownership checks. BigBoy `/home`
recovered from 4.2 GiB to 14 GiB free. Final gates were routed to `.50`, `.90`,
`.170`, and `.196`.

## Live-only residual proof

After the first full release, use a real governed SIP account and one peer/PSTN
endpoint to record registration, audible duplex RTP, exact outbound target,
callee identity, mute/unmute acknowledgement, BYE, and provider-loss recovery.
Those are runtime/provider proofs; no known outbound-target or mute-
acknowledgement coding gap is deferred by this slice.
