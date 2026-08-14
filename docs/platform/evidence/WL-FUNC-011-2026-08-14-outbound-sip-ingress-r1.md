# WL-FUNC-011 outbound SIP ingress

- BigBoy `.130` farm slot: `func011-sip-outbound`.
- `mackesd` bounded outbound-ingress hostile test — PASS, 1/1.
- `mde-voice-hud` SIP suite — PASS, 37/37.
- Empty, control-bearing, and oversized dial targets are refused before
  provider effects; accepted targets are trimmed and existing provider
  lifecycle/refusal behavior remains intact.
