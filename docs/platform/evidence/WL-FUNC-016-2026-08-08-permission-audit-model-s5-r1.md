# WL-FUNC-016 permission and audit model — 2026-08-08

The shell now owns one bounded clipboard permission controller and a live,
metadata-only modal. It exposes source/target/MIME/size/expiry summaries,
one-use approval, deny/cancel, progress, and typed terminal failures. Focus,
session, lease, and expiry changes revoke active work. Replay high-water marks
and credential/payload-redacted audit rows are independently capped at 128.

The live VNC adapter now routes both host-to-guest `ClientCutText` and
guest-to-host legacy/V2 publication through one-use materialization tickets.
This closes the legacy publication bypass before mackesd can translate it into
a signed guest-to-client action. Rich offers still require approval when VNC
selects a plain-text fallback.

## Verification

BigBoy `.130`, slot `func016-permission-audit-s5-r1`:

- `.90`, slot `func016-s5-vnc-gate-r1`: focused model/controller/render and
  bidirectional VNC gate tests passed 11/11 with `live-vdi` enabled.
- Approval replay, secret refusal, generation revocation, bounded FIFO audit,
  progress rules, and structural redaction were exercised.
- Scoped rustfmt check and `git diff --check` passed.

## Remaining acceptance gap

RDP/SPICE expose no live clipboard callback here. Downstream DRM-consumption
acknowledgement, package policy, CAS cleanup, and five-seat proof remain, so
FUNC-016 stays `Remaining`.
