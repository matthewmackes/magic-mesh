# WL-FUNC-021 — service aggregator startup phase (2026-08-07)

## Finding

`service_aggregator` performed its first directory/inventory fold and resource
catalog projection immediately on every seat, making daemon restarts converge
the most expensive service-census work into one burst.

## Change

The worker now derives a stable FNV-1a identity phase capped at 1.5 seconds
before its first fold. Shutdown remains cancellation-safe, and the existing
15-second polling and 60-second publication-heartbeat semantics are unchanged.

## Verification

BigBoy farm lane `service-aggregator-phase-r1`:

```text
cargo test -p mackesd service_aggregator --features async-services --locked -- --nocapture
18 passed, 0 failed, 4387 filtered
```

This is source/farm evidence; Dell deployment and live CPU proof remain open
while the authorized Dell endpoints are unreachable.
