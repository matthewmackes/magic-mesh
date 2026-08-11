# WL-FUNC-016 RDP advertised clipboard generation — 2026-08-11

- Scope: an RDP format-data request may read only the local clipboard generation previously advertised to that peer.
- Hostile boundary: a delayed request from the old offer cannot inherit replacement content before the replacement generation's format list is advertised.
- Focused gate: `cargo test -p mde-vdi-rdp --features live-connect clipboard::tests::stale_request_cannot_read_replacement_before_its_generation_is_advertised -- --exact --nocapture`.
- Farm: `172.20.0.50`, slot 2, admitted with 9,765,160 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 105 filtered out.
- Remaining boundary: live RDP delayed request raced against replacement and format-list advertisement remains.
