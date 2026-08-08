# WL-FUNC-016 — strict rich clipboard envelope V2 (2026-08-05)

The shared collaboration types now admit a signed `ClipboardEnvelopeV2` with
typed node/seat/session identity, monotonic sequence, finite MIME offers,
inline UTF-8 limits, Files references for binary/large content, hashes, expiry,
explicit unsupported/unavailable states, and bounded echo guards. Ed25519
attribution covers the complete canonical envelope; unknown fields, malformed
JSON, replay, oversized content, unsafe identities, and signature mismatches
fail closed.

## Verification

- BigBoy `.130`: `6 passed; 0 failed; 57 filtered out`.
- Farm `.50`: exact clipboard file formatting passed.
- This is a shared transport-contract slice; direct-DRM adapters, consent
  fanout, VDI CLIPRDR/SPICE/RDP support, and live proof remain open.
