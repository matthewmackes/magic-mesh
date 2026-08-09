# WL-FUNC-018 S1/S3 — catalog side-effect retry boundary

Date: 2026-08-09

## Production correction

The production-registered `AppCatalogWorker` no longer advances an import row's
in-memory cursor before its governed Bus effects succeed. A transient projection
or status write failure therefore leaves the row eligible for the next bounded
poll instead of losing the catalog update after last-good persistence. Catalog
expiry likewise retains the current authority until both the empty projection
and unavailable status have been published, so a failed retraction is retried.

This changes ordering inside the existing signed catalog importer only. It does
not add another catalog, cursor store, or lifecycle authority.

## Hostile regression

The regression injects a projection write failure after the signed catalog has
already reached the durable last-good file. It proves that the cursor, current
catalog, and watermark remain unadvanced; the next pass consumes the same row
and publishes exactly one admitted projection.

Farm machine: `172.20.0.170` (`.170`), slot 1.

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=1 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  --features async-services \
  workers::app_catalog::tests::projection_failure_does_not_acknowledge_import_and_retry_admits_once \
  -- --exact --nocapture

running 1 test
test workers::app_catalog::tests::projection_failure_does_not_acknowledge_import_and_retry_admits_once ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4390 filtered out
```

The crate emitted existing warnings outside this change's scope. There were no
implementation or verification blockers.
