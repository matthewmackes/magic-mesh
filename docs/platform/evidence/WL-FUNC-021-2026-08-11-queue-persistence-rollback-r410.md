# WL-FUNC-021 queue persistence rollback — 2026-08-11

- Scope: daemon-owned queue mutation succeeds only when the durable queue write succeeds.
- Hostile boundary: persistence failure rolls memory back and publishes `ok:false`; no mutation leaks into a later corrected-forward request.
- Focused gate: `cargo test -p mde-musicd bus_responder::tests::failed_queue_persistence_cannot_publish_success_or_leak_mutation_forward -- --exact --nocapture`.
- Farm: `172.20.0.130`, slot 1, admitted with 17,640,276 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 262 filtered out.
- Remaining boundary: real playback queue-storage loss with truthful GUI/audible state and corrected-forward recovery proof remains.
