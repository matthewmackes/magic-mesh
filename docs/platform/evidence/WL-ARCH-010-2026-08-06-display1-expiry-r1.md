# WL-ARCH-010 Display1 expiry cleanup — 2026-08-06

The Display1 broker now treats lease expiry and relay shutdown as revocation
boundaries: it clears readiness, revokes the relay, and unlinks the stale
socket. A regression fixture proves a stale socket/readiness state cannot
survive expiry.

Verification:

- BigBoy `.130`, slot `arch010-display1-expiry-20260806-r1`:
  `cargo test -p mackesd display1_broker::tests -- --nocapture` passed **7/7**.
- Source SHA-256:
  `fdb3bfe73d7742ddc1853e048519655098726778cb9ab494362423267c53c7ec`.

This is a broker cleanup slice, not live DMA-BUF/KMS, input/audio/clipboard,
device-loss, package, or seat acceptance proof. Dell runtime was not modified.
