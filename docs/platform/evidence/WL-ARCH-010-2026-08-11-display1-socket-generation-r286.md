# WL-ARCH-010 Display1 socket-generation cleanup evidence — 2026-08-11

- Scope: Display1 broker cleanup now removes a lease socket only when the
  pathname still resolves to the exact inode created by that broker instance.
- Hostile boundary: a stale/expired broker generation drops after a newer
  generation has rebound the same lease pathname. Cleanup preserves the newer
  live socket instead of unlinking by pathname alone.
- Focused gate: `cargo test -p mackesd --lib --features async-services display1_broker::tests::stale_generation_drop_cannot_unlink_newer_live_broker_socket -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 2.
- Result: **PASS**, 1 passed, 0 failed, 4,831 filtered out.
- Remaining boundary: this proves local generation-safe cleanup; live Display1
  attachment/presentation and release acceptance remain open.
