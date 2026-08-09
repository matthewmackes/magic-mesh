# WL-FUNC-020 S1 — Android catalog side-effect retry boundary (r6)

Date: 2026-08-09

Base revision: `adf9d147`

## Boundary corrected

The Android catalog importer previously advanced its in-memory Bus cursor before
the admitted catalog was durably stored and published. A transient state-storage
or Bus publication failure therefore acknowledged the signed import while its
governed effects were incomplete, preventing another attempt until the daemon
restarted.

The importer now acknowledges each row according to its disposition:

- missing, oversized, malformed, untrusted, and stale rows are terminally
  refused and acknowledged;
- an admitted row is acknowledged only after both durable last-good storage and
  catalog-state publication succeed;
- a failed governed side effect leaves the row unacknowledged, so the next poll
  retries the same signed revision without producing a duplicate publication.

The hostile regression places a regular file where the catalog state directory
must be, proves the persistence pass fails without advancing the cursor, repairs
the path, and proves the same queued import publishes exactly once on retry.

## Focused farm verification

Host: machine 9, `172.20.0.50`

Slot: `android-catalog-retry-s1-r6`

Command:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=android-catalog-retry-s1-r6 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::android_catalog::tests::transient_side_effect_failure_keeps_signed_import_retryable -- --exact --nocapture
```

Result:

```text
running 1 test
test workers::android_catalog::tests::transient_side_effect_failure_keeps_signed_import_retryable ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4395 filtered out
```

A preliminary short-name `--exact` invocation compiled successfully but matched
zero tests; it is not counted as verification. No broad suite, package build, or
live-seat action was run.

## Scope and remaining work

This closes one importer restart/idempotency boundary within WL-FUNC-020 S1. It
does not claim Android release packaging, nested-KVM execution, remote-session
attachment, or live Cuttlefish acceptance.
