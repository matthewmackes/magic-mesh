# WL-FUNC-021 / WL-ARCH-009 — media-server Bus recovery (r28)

Date: 2026-08-09

Production source: `crates/mesh/mackesd/src/workers/media_server.rs`

Source SHA-256:
`e6bf6a81731d0486587d4391baed89fa4a99e752f0c4b646c59d4d0a9a28d0a0`

## Correction

`MediaServerWorker` no longer terminates successfully when the shared Bus is
absent or unopenable during startup. Explicit roots remain exact; otherwise
normal mde-bus resolution is used with the documented
`mde_bus::SYSTEM_BUS_ROOT` service fallback. Startup retries at the configured
tick clamped to 10 ms–2 s, and shutdown interrupts every retry wait.

Share-manifest materialization, HTTP/SSDP initialization, library aggregation,
and publication remain behind successful Bus activation. The same worker then
runs its immediate first cycle and publishes the honest library without a
daemon restart. No server, source, or media item is fabricated.

## Focused farm proof

Host: machine 193 (`172.20.0.90`)

Slot: `media-sources-bus-r27` (the already-warm isolated media runtime slot)

```text
cargo test -p mackesd --features async-services --lib \
  workers::media_server::tests::media_server_bus_root_preserves_override_and_has_system_fallback \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,445 filtered out`.

```text
cargo test -p mackesd --features async-services --lib \
  workers::media_server::tests::late_bus_recovers_and_publishes_library_without_worker_restart \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,445 filtered out`. One worker remained alive
behind an unopenable Bus, created no manifest before activation, then wrote its
empty honest manifest and published its initial library after recovery.

The exact source passed remote `rustfmt --edition 2021 --check` and local scoped
`git diff --check`. No broad suite, package build, live HTTP/SSDP fixture, or
unrelated test was run.
