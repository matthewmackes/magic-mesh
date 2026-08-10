# WL-ARCH-009 metrics bucket normalization — r184

Date: 2026-08-10

## Correction

The shared metrics owner now normalizes optional/provider histogram schedules
before they enter Prometheus rendering: non-finite bounds are discarded,
remaining bounds are sorted, and duplicates are removed. This prevents a
malformed provider schedule from publishing invalid bounds or corrupting the
percentile estimate used by the runtime metrics snapshot.

## Focused verification

Farm seat `.90` (`172.20.0.90`), slot `arch009-metrics-bucket-normalize-r184b`:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch009-metrics-bucket-normalize-r184b \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  metrics::tests::histogram_normalizes_hostile_provider_bucket_schedule -- --nocapture
```

Result: **1 passed; 0 failed; 0 ignored; 0 measured; 4717 filtered out**.
