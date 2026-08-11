# WL-FUNC-021 queue CAS framing — 2026-08-11

- Scope: durable queue revision identity length-frames every field, queue
  cardinality, and source presence.
- Hostile boundary: embedded delimiters cannot make distinct queues share CAS
  authority across restart.
- Focused gate: `cargo test -p mde-musicd queue::tests::embedded_delimiters_cannot_substitute_durable_queue_revision_after_restart -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 12.2 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 254 filtered out.
- Remaining boundary: live synchronized queue replacement remains.
