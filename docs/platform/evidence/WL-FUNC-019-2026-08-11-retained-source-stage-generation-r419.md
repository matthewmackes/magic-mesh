# WL-FUNC-019 retained-source staging generation — 2026-08-11

- Scope: universal resource-catalog staging remains bound to one retained-source generation.
- Hostile boundary: desktop, SSH/X11, or UPnP input advancement during staging cannot publish a mixed-generation catalog.
- Focused gate: `cargo test -p mackesd workers::service_aggregator::tests::retained_source_advance_during_stage_cannot_publish_mixed_generation -- --exact --nocapture`.
- Farm: `172.20.0.170`, slot 2, admitted with 13,515,136 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,876 filtered out.
- Remaining boundary: race live retained producers against catalog staging and prove corrected-forward publication on the following cycle.
