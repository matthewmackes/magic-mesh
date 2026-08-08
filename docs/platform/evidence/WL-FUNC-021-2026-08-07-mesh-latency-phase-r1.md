# WL-FUNC-021 — mesh-latency sweep phase audit (2026-08-07)

`MeshLatencyWorker` performed its first full peer sweep immediately and then
anchored every seat to the same 30-second boundary. It now keeps the immediate
boot cache behavior, then applies a deterministic node-id phase of at most five
seconds before the first recurring sweep. The phase is shutdown-safe and the
next sweep remains no later than the configured cadence, preserving freshness.

Farm `.90`, slot `mesh-latency-phase-r1`:

```text
cargo test -p mackesd mesh_latency --features async-services --locked -- --nocapture
test result: ok. 7 passed; 0 failed; 4394 filtered out
```

The regression covers stable per-host phase, the five-second bound, short test
interval behavior, empty-host behavior, cache round-trip, and shutdown. This
is source/farm evidence; synchronized installed-seat CPU behavior remains open
while Dell is unreachable.
