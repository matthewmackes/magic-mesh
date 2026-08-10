# WL-FUNC-017 — future offline-cache timestamps (r157)

Date: 2026-08-10

Offline map entries with future `verified_at_ms` or `last_access_ms` are now
treated as corrupt and removed rather than admitted as fresh. BigBoy proof:

```text
MCNF_BUILD_HOST=172.20.0.130
MCNF_BUILD_SLOT=func017-map-future-r157
install-helpers/xcp-build.sh cargo test -p mde-maps-location-egui --lib \
  future_cache_timestamps_are_rejected_and_removed -- --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 309 filtered out
```

