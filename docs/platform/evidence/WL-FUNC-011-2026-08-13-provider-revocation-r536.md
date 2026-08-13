# WL-FUNC-011 provider-observed call revocation (r536)

Date: 2026-08-13

## Executable gap

The production SIP adapter consumed registration, inbound-call, established,
and remote-hangup events on its monitor thread. `RemoteHangup`, registration
failure, and agent termination were discarded, however, so the concrete media
provider could terminate while the durable Collaboration projection continued
to advertise the exact call as active. This violated S4's auditable revocation
and honest provider-failure requirements.

## Implementation

- `SipGatewayProvider` now binds the exact Collaboration `CallId` only after an
  Answer command is accepted or an outbound Dial is acknowledged.
- A remote hangup, registration failure, or SIP-agent termination atomically
  removes that binding and queues one deduplicated provider revocation.
- Local Decline/HangUp clears the provider binding without manufacturing a
  remote revocation.
- `CallMediaProviderRegistry` drains revocations from every concrete provider
  family and deduplicates the exact call identities.
- `CollabWorker` consumes those revocations before its next command sweep,
  derives the normal signed `HangUpCall` transition, durably appends it to the
  actor log, and only then publishes it. It deliberately does not send a second
  HangUp command back to the provider that reported termination.
- Stale or already-terminal provider events fail closed against the core call
  authority and cannot terminate another call.

The focused regression creates a real signed call through a registered media
provider, injects termination of that exact call, and checks both the inactive
projection and durable `call_ended` actor-log record.

## Farm gates

- `.196`, slot 1: `cargo clippy -p mackesd --all-targets --all-features -- -D warnings` — passed on the final source.
- `.196`, slot 1: `cargo test -p mackesd provider_observed_revocation_durably_ends_the_exact_call -- --nocapture` — passed 1/1 (4,976 unrelated library tests filtered; ancillary targets had no matching test).
- BigBoy `.130`, slot 1: `cargo build -p mackesd --features async-services --bin mackesd` — passed on the exact final source.
- `.170`, slot 2: exact Rust 1.94 `rustfmt --edition 2021 --check` for `collab.rs` and `collab_media.rs` — passed after applying the reported owned-file formatting delta.
- Local scoped `git diff --check` — passed.

The package-wide format check was not used as acceptance evidence because it
reported unrelated formatting drift across concurrent, unowned modules. No
such file was modified.

## Residual acceptance

This closes one executable provider-revocation gap. FUNC-011 still requires a
separate audit of inbound SIP identity-to-space admission and any remaining
group media, screen-share, consented-control, reconnect, office, transfer, and
hard-cut implementation gaps. After the first full release, the deferred
non-blocking live acceptance must record real SIP signaling/audio, advancing
RTP frames, provider loss/recovery, and one-node corrected-forward recovery.
