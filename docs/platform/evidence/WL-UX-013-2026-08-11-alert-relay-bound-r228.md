# WL-UX-013 alert relay input bound — 2026-08-11

- Scope: alert JSON ingestion rejects symlinks, non-regular files, invalid UTF-8, and payloads over 64 KiB before JSON parsing.
- Farm: BigBoy `172.20.0.130`; focused agent farm lane reported 1 test passed.
- Test: `workers::alert_relay::tests::tick_once_skips_oversized_and_symlinked_alert_inputs`.
- Result: PASS, 1 passed, 0 failed.
