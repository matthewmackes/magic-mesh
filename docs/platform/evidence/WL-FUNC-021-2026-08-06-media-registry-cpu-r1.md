# WL-FUNC-021 — Media registry failed-probe backoff (2026-08-06)

`MediaRegistryWorker` probes the local Navidrome port before publishing the
typed media registration. A persistently absent local service previously caused
that network probe to run on every 30-second tick on every media-capable seat.

## Change

The worker now retains the honest `down` registration but backs off repeated
failed health probes at 30/60/120/240/300 seconds. A healthy probe or an
operator-configured record resets the delay to the normal 30-second cadence.
The shutdown select remains responsive, and no credentials or service state
are inferred from the backoff.

Implementation: `crates/mesh/mackesd/src/workers/media_registry.rs`.

## Verification

Farm `.50`, slot `media-registry-cpu-r1`:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=media-registry-cpu-r1 \
./install-helpers/xcp-build.sh cargo test -p mackesd \
workers::media_registry::tests::failed_health_probes_use_a_bounded_retry_ladder \
--features async-services --locked -- --nocapture
```

Result: **1 passed, 0 failed**; 4,390 library tests filtered and all other
filtered targets passed with zero selected failures. Existing unrelated
warnings were unchanged.

The source change is not installed on a live seat in this evidence slice, so
post-install CPU improvement remains an explicit acceptance gate.
