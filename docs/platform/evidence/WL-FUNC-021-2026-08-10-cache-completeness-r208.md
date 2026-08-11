# WL-FUNC-021 — Music cache completeness (r208)

- Scope: truncated or replaced non-empty cache files are refused unless their
  size matches the durable index; suffix reads and LRU timestamps remain safe.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func021-cache-completeness-r208 install-helpers/xcp-build.sh cargo test -p mde-musicd --lib cache::tests::truncated_cached_track_is_not_admitted_as_complete -- --exact --nocapture`.
- Result: `.90` passed: `1 passed; 0 failed; 0 ignored; 0 measured; 242 filtered out`.
