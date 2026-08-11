# WL-ARCH-010 Display1 pre-presentation disconnect — 2026-08-11

- Scope: the installed Display1 broker distinguishes pending KMS presentation
  from shell EOF or an invalid acknowledgement. A shell that vanishes after
  receiving the QEMU DMA-BUF but before presentation loses its relay, frame
  authority, false readiness, and held-input epoch; further frame/FD delivery
  stops until a fresh one-use attachment lease is installed.
- Farm: `172.20.0.90`, slot `1`.
- Focused gate: `install-helpers/xcp-build.sh cargo test -p mackesd --lib display1_broker::tests::pre_presentation_disconnect_revokes_dead_relay_and_frame_authority -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed, 4,801 filtered out.
- Live boundary: crash the installed shell between real DMA-BUF receive and KMS
  acknowledgement, then prove a fresh `StartAndAttach` lease recovers.
