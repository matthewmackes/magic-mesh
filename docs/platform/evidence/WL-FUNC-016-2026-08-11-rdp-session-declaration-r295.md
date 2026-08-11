# WL-FUNC-016 RDP session declaration binding evidence — 2026-08-11

- Scope: each authenticated RDP transport is bound to the exact non-secret
  endpoint, user, domain, and geometry declaration authenticated at connect.
- Hostile boundary: host, user, domain, or geometry substitution is rejected
  before guest reads, clipboard callbacks, input drain, or shutdown effects.
- Focused gate: `cargo test -p mde-vdi-rdp --features live-connect connect::tests::connection_binding_rejects_cross_session_clipboard_transport_reuse -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 3.
- Result: **PASS**, 1 passed, 0 failed, 103 filtered out.
- Remaining boundary: live guest rich clipboard, permission/revocation, image
  ingest, and Windows acceptance remain open.
