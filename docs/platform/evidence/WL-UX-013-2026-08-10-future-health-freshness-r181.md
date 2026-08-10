# WL-UX-013 future health freshness — r181

- Scope: health projections with zero or future generation timestamps fail freshness admission instead of appearing current.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=ux013-health-freshness-r181 install-helpers/xcp-build.sh cargo test -p mackes-mesh-types --lib health::tests::snapshot_freshness_rejects_future_and_zero_timestamp_projections -- --nocapture`
- Result: `1 passed; 0 failed; 515 filtered out` on seat `.50`.
