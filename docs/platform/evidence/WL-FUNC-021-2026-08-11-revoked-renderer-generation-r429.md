# WL-FUNC-021 revoked renderer generation — 2026-08-11

- Scope: Music renderer loss revokes the in-flight decoded-audio generation.
- Hostile boundary: a revoked generation cannot republish audio or advance queue boundaries after device loss.
- Focused gate: `cargo test -p mde-musicd engine::tests::revoked_renderer_generation_cannot_republish_inflight_audio -- --exact --nocapture`.
- Farm: clean isolated rerun on `172.20.0.170`, slot 2; an earlier `.196` collision was terminated and not claimed.
- Result: **PASS**, 1 passed, 0 failed, 263 filtered out.
- Remaining boundary: remove and return a live renderer during decode and prove no retired audio/queue transition is emitted.
