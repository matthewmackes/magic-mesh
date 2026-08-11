# WL-FUNC-016 RDP guest key generation — 2026-08-11

- Scope: a restarted RDP endpoint remains bound to the prior guest-generation TLS public-key pin until explicit generation authority replaces it.
- Hostile boundary: a replacement guest key is rejected before credentials, while first use and same-key certificate renewal remain valid.
- Focused gate: `cargo test -p mde-vdi-rdp --features live-connect connect::tests::restarted_guest_key_replacement_cannot_be_adopted_without_generation_authority -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on BigBoy `172.20.0.130`, slot 2.
- Result: **PASS**, 1 passed, 0 failed, 106 filtered out.
- Remaining boundary: prove the same pin continuity against a live restarted guest and production credential handoff.
