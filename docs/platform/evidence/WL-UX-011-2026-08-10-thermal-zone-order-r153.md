# WL-UX-011 — deterministic thermal-zone ordering (r153)

Date: 2026-08-10

The sysfs hardware provider now sorts thermal-zone directory entries before
applying its bounded sixteen-zone limit. Hardware summaries therefore have a
stable source order despite filesystem directory-order variation.

## Farm proof

Build VM `.50` (`172.20.0.50`), slot `ux011-thermal-order-r153`:

```text
cargo test -p mde-seat --lib hardware::tests::hostile_sensor_values_fail_closed -- --nocapture
1 passed; 0 failed; 0 ignored; 0 measured; 111 filtered out
```

This is provider-contract proof; live hardware publication remains open.
