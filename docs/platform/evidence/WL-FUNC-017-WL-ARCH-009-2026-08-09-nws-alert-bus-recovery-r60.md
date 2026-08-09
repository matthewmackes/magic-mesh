# WL-FUNC-017 / WL-ARCH-009: NWS alert Bus recovery (r60)

Date: 2026-08-09

Base revision: `119e2635b591309a6a9237de521bbbb075589214`

## Scope and resulting semantics

The change is confined to `crates/mesh/mackesd/src/workers/nws_alert_overlay.rs` and this evidence record.

- Bus selection is resolved for every transaction in explicit override, current user Bus, then canonical `SYSTEM_BUS_ROOT` order. A missing, late, unopenable, replaced, or externally updated Bus no longer freezes the worker at construction time.
- Vehicle context acquisition is `Result<Option<GeoPoint>>`: Bus open/read/body/decode failures defer the cycle without publishing or changing cadence/cache state. Only a successfully read absent, invalid, or stale semantic fix can produce typed unavailable state.
- Retained degraded-source reads likewise distinguish unavailable storage or malformed data from a successfully read missing row; read/decode failure defers publication.
- Fresh, NotModified, degraded, and unavailable writes return `Result`. `last_good` and cadence advance only after a required write succeeds.
- A completed provider response is retained in memory across Bus publication failure, so corrected-forward publication does not refetch. Before publishing it, the worker reopens the Bus and re-reads the exact vehicle point. Movement retracts prior-location alerts and retries for the new point.
- ETag/NotModified identity checks and alert/zone cardinality bounds remain enforced.

## Farm verification

Host: machine9, `172.20.0.50`

Explicit slot: `nws-alert-bus-r60`

The clean base tree was materialized in `/home/mm/magic-mesh-farm-nws-alert-bus-r60`; only the owned source file was overlaid. The isolated target was `/home/mm/target-nws-alert-bus-r60`.

Formatting:

```text
rustfmt --edition 2021 crates/mesh/mackesd/src/workers/nws_alert_overlay.rs
rustfmt --edition 2021 --check /home/mm/magic-mesh-farm-nws-alert-bus-r60/crates/mesh/mackesd/src/workers/nws_alert_overlay.rs
PASS
```

Exact focused tests used this command shape:

```text
CARGO_TARGET_DIR=/home/mm/target-nws-alert-bus-r60 \
  cargo test -p mackesd \
  workers::nws_alert_overlay::tests::<test-name> -- --exact --nocapture
```

Results:

```text
late_and_replaced_bus_recovers_external_point_and_shutdown: PASS (1 passed, 0 failed)
failed_vehicle_and_retained_source_reads_are_effect_free: PASS (1 passed, 0 failed)
post_fetch_movement_retracts_prior_point_alerts_before_retry: PASS (1 passed, 0 failed)
failed_write_retains_prepared_result_and_corrects_forward_without_refetch: PASS (1 passed, 0 failed)
not_modified_cannot_relabel_or_retain_a_prior_points_snapshot: PASS (1 passed, 0 failed)
spurious_not_modified_without_a_sent_validator_is_rejected: PASS (1 passed, 0 failed)
hostile_cardinality_is_capped_and_zone_urls_are_deduplicated: PASS (1 passed, 0 failed)
```

The first cold invocation completed in 8m38s. Subsequent exact tests reused the warmed isolated target. Existing repository warnings were emitted; there were no test failures.

Scoped integrity checks:

```text
git diff --check -- crates/mesh/mackesd/src/workers/nws_alert_overlay.rs
PASS

git diff --numstat -- crates/mesh/mackesd/src/workers/nws_alert_overlay.rs
645  114  crates/mesh/mackesd/src/workers/nws_alert_overlay.rs
```

The farm and workspace source SHA-256 values matched exactly:

```text
be1bccd9094263f7883cd209ab446c88569e9fd4609081a6bf12d84a4e7da793  crates/mesh/mackesd/src/workers/nws_alert_overlay.rs
```

## Residual non-atomic boundaries

- Vehicle location can change after the final recheck and before the Bus append; the Bus has no cross-topic conditional transaction to close that interval.
- A prepared provider result is process-memory state. A crash discards it and may require a new fetch after restart.
- Bus append and in-memory `last_good`/cadence commit are ordered but not one atomic durable transaction. A crash after append and before memory commit can cause a later semantically duplicate append.
- Retained-source observation and output append are not a compare-and-swap transaction.

No blocker was found. No commit, push, or `WORKLIST.md` edit was performed.
