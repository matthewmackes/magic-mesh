# WL-ARCH-010 Display1 packet-safe relay — r21

Date: 2026-08-09
Source revision: `e827bc93ea0b33d1ed61b526fbef892d8ce2e100`

## Result

The node-local Display1 relay now uses Unix `SOCK_SEQPACKET` end to end instead
of treating a byte stream as if each read were one message. Handshake,
acknowledgement, frame, and damage records retain packet boundaries. A frame's
SCM_RIGHTS descriptor cannot migrate to a neighboring frame or damage record.

Both endpoints verify `SO_TYPE`, bound every packet, distinguish an orderly
disconnect from an invalid empty packet, and reject `MSG_TRUNC` or ancillary
`MSG_CTRUNC`. Rapid frame/damage delivery therefore remains ordered without
coalescing or splitting JSON envelopes, and an oversized or descriptor-bearing
damage packet fails closed before KMS import.

Exact source hashes:

```text
3704d596bde0c05d558a6edbade87fe34ed8b4de5342b8a1d987ab8541ae2a69  crates/mesh/mackesd/src/display1_broker.rs
61ea00500696b89dede3e23f0ca62efce7cf059ebb3693d49156464d98b6be6d  crates/desktop/mde-shell-egui/src/display1_client.rs
```

This packetization checkpoint composes with the earlier Display1 readiness
proofs in
`docs/platform/evidence/WL-ARCH-010-2026-08-09-display1-present-ack-r19.md`,
`docs/platform/evidence/WL-ARCH-010-2026-08-06-display1-expiry-r1.md`, and
`docs/platform/evidence/WL-ARCH-010-2026-08-09-display1-damage-delivery-r20.md`.

## Focused farm verification

- BigBoy `172.20.0.130`, slot `arch010-display1-seqpacket-daemon`:
  `cargo test -p mackesd --features async-services --lib display1_broker::tests -- --nocapture`
  — 9 passed, 0 failed.
- XEN-196 build VM `172.20.0.196`, slot
  `arch010-display1-seqpacket-client`:
  `cargo test -p mde-shell-egui --features drm display1_client -- --nocapture`
  — 9 passed, 0 failed. This includes rapid frame-plus-damage delivery,
  descriptor packet binding, oversized and empty packet refusal, nonblocking
  disconnect handling, lease/peer binding, and one-shot presentation ack.
- `172.20.0.50`, slot `arch010-display1-seqpacket-fmt`:
  exact `rustfmt --edition 2021 --check` over both changed Rust files passed.

## Remaining boundary

This closes the local Display1 packetization defect only. Lease-bound guest
input, audio transport, and installed live Dell/seat-15 frame proof remain
required by the owning epic.
