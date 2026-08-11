# WL-ARCH-009 — metrics observation finiteness r194

Date: 2026-08-10

## Scope

`Histogram::new` already rejected non-finite provider bucket bounds, but
`Histogram::observe` accepted `NaN`, positive infinity, and negative infinity.
That allowed one malformed process-isolated provider observation to poison the
histogram sum/count or produce a sample with no valid finite bucket boundary.

The shared metrics ownership boundary now discards every non-finite
observation before changing bucket counts, `_sum`, or `_count`. Finite samples
retain the existing cumulative-bucket behavior.

## Focused farm verification

Farm host: `.90` (`172.20.0.90`)

Explicit slot:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch009-finite-observation-r194
```

Command:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch009-finite-observation-r194 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  metrics::tests::histogram_discards_non_finite_provider_observations \
  -- --exact --nocapture
```

Result:

```text
1 passed; 0 failed; 0 ignored; 0 measured; 4727 filtered out
```

The regression injects all three non-finite IEEE-754 values, proves that the
histogram remains empty and finite, then proves a valid observation is still
counted in the correct bucket and sum.

## Live limits

No installed six-group process census, node_exporter scrape, physical-seat
provider fault injection, or fleet convergence proof was performed. Those
remain live/package acceptance work for ARCH-009 S4/S7.
