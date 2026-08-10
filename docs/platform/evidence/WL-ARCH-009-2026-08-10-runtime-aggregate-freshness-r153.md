# WL-ARCH-009 — runtime aggregate freshness (r153)

Date: 2026-08-10

Empty node runtime aggregates now expire on the same bounded freshness window
as populated aggregates. Decode rejects a snapshot older than
`RUNTIME_FRESHNESS_MS`, preventing retained empty state from looking live
indefinitely.

## Farm proof

BigBoy (`172.20.0.130`), slot `arch009-runtime-freshness-r153`:

```text
cargo test -p mackesd --lib workers::worker_runtime_status::tests::stale_empty_aggregate_is_rejected_at_decode -- --nocapture
1 passed; 0 failed; 0 ignored; 0 measured; 4691 filtered out
```

This proves the bounded decode contract; live fleet freshness remains open.
