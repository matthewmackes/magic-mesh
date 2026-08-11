# WL-FUNC-017 MG90 source generation — 2026-08-11

- Scope: vehicle enrichment is bound to the currently admitted MG90 source generation.
- Hostile boundary: a replaced source cannot merge enrichment into retained vehicle state.
- Focused gate: `cargo test -p mackesd workers::vehicle::tests::replaced_mg90_source_cannot_merge_enrichment_into_retained_generation -- --exact --nocapture`.
- Farm: fixed coordinator snapshot on `172.20.0.90`, slot 1.
- Result: **PASS**, 1 passed, 0 failed.
- Related exact pass: `WL-FUNC-017-2026-08-11-atmosphere-source-identity-r471.md`.
- Remaining boundary: live MG90 radio and route capture.
