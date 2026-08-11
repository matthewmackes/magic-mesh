# WL-FUNC-016 V2 consent ordering evidence — 2026-08-11

- Scope: V2 clipboard envelopes drain in durable Bus order. A row whose consent
  is not yet available stops the drain instead of being skipped.
- Hostile boundary: a later authorized row from another source cannot advance
  the durable cursor past the earlier consent-withheld envelope, preserving its
  retry authority after consent arrives.
- Focused gate: `cargo test -p mackesd --lib workers::clipboard_sync::tests::v2_consent_withheld_row_blocks_later_authorized_cursor_advance -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 2.
- Result: **PASS**, 1 passed, 0 failed, 4,833 filtered out.
- Remaining boundary: live DRM/mesh/VDI adapters, rich guest materialization,
  permissions, cleanup, and release proof remain open.
