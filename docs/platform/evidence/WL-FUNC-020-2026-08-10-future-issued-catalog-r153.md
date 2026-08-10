# WL-FUNC-020 — future-issued catalog refusal (r153)

Date: 2026-08-10

Android provider preflight now refuses a signed catalog whose issue time is in
the future relative to the admission clock. The refusal is unavailable,
fail-closed, and occurs before expiry or readiness can admit it.

## Farm proof

Build VM `.90` (`172.20.0.90`), slot `func020-future-catalog-r153`:

```text
cargo test -p mackesd --lib workers::cloud::android_provider::tests::future_issued_catalog_is_not_admitted_before_validity_window -- --nocapture
1 passed; 0 failed; 0 ignored; 0 measured; 4691 filtered out
```

This is provider-contract proof; nested-KVM and live Android acceptance remain open.
