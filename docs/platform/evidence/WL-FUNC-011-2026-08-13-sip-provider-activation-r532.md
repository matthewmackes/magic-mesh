# WL-FUNC-011 Calls S4: concrete SIP provider activation

Date: 2026-08-13

## Implemented slice

- Registered the existing `mde-voice-hud` SIP/RTP media core as the production `SipGateway` provider in `mackesd` instead of leaving the provider registry proof-only.
- Activation is limited to a pinned Workstation role with a governed SIP account. Missing configuration, registration loss, thread-start failure, queue saturation, or queue disconnection fails closed.
- The adapter uses a bounded 16-command channel and registration-event health monitor. It routes inbound answer/decline/hangup and RFC 4733 DTMF to the existing agent.
- Outbound calls remain refused because the current collaboration command has no dial target. Mute remains refused because the existing agent has no acknowledgement path. RTP frame proof remains refused because the core does not expose advancing counters; no live proof was fabricated.
- The `async-services` feature now links the existing voice core. The base RPM already owns `alsa-lib`, PipeWire, WirePlumber, PipeWire ALSA/PulseAudio compatibility, and PipeWire utilities, so no duplicate service or package asset was added.

## Farm evidence

- `172.20.0.170`, slot `func011-sip-provider-test-r532b`: focused exact provider test — PASS (1 passed, 0 failed, 4,970 filtered).
- `172.20.0.130`, slot `func011-sip-provider-final-clippy-r532`: `cargo clippy -p mackesd --all-targets --features async-services -- -D warnings` — PASS.
- `172.20.0.196`, slot `func011-sip-provider-final-fmt-r532`: exact `rustfmt --edition 2021 --check` for `collab_media.rs` and `collab.rs` — PASS.
- `172.20.0.130`, slot `func011-sip-provider-rpm-r532`: RPM requirements gate — PASS; explicit payload declaration audit confirmed the `mde-voice-hud` dependency and PipeWire/WirePlumber package ownership — PASS.
- The broader RPM payload verifier was also observed but is not claimed as passing: it reported unrelated pre-existing Maps verifier naming and missing UX-014 KIRON assets. The Calls package requirements exercised by this slice passed.

## Deferred post-release acceptance

No live SIP endpoint, audio device, or advancing-frame proof was performed. Those acceptance proofs remain post-release and non-blocking under the operator directive.
