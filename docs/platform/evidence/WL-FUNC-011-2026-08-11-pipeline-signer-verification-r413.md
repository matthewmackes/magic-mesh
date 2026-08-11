# WL-FUNC-011 pipeline signer verification — 2026-08-11

- Scope: collaboration authoring may return only an envelope whose signature verifies for its claimed actor and event body.
- Hostile boundary: a signer implementation that substitutes the actor or emits a malformed signature fails with `InvalidEvent` before leaving the pipeline.
- Focused gate: `cargo test -p mde-collab-core pipeline::tests::signer_actor_substitution_cannot_escape_the_authoring_pipeline -- --exact --nocapture`.
- Farm: `172.20.0.130`, slot 2, admitted with 15,802,644 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 114 filtered out.
- Remaining boundary: real actor-to-public-key trust binding and corrected-forward cross-seat replication proof remain.
