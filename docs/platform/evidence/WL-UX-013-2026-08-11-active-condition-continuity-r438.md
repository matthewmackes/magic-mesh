# WL-UX-013 active-condition continuity — 2026-08-11

- Scope: forward Health generations retain every active condition or resolve it into history with stable provenance.
- Hostile boundary: a newer projection cannot silently erase an active condition or regress its observation identity.
- Focused gate: `cargo test -p mackesd workers::health_reconciler::tests::forward_generation_cannot_silently_erase_active_condition_history -- --exact --nocapture`.
- Farm: final coordinator-only rerun on `172.20.0.170`, slot 1, admitted with 10,716,868 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,884 filtered out.
- Remaining boundary: drop an active condition from a live forward provider generation and prove it remains visible until a sourced resolution arrives.
