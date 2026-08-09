# WL-FUNC-021 / WL-ARCH-009 — media-source Bus recovery (r27)

Date: 2026-08-09

Production source: `crates/mesh/mackesd/src/workers/media_sources.rs`

Source SHA-256:
`5b2f367e2ce29842709129bfb1a795636af0edd406308b4d57f46012faa9a120`

## Correction

`MediaSourcesWorker` no longer terminates successfully when the shared Bus is
absent or unopenable during daemon startup. Explicit roots remain exact;
otherwise normal mde-bus resolution is used with the documented
`mde_bus::SYSTEM_BUS_ROOT` service fallback. Startup retries at the configured
tick clamped to 10 ms–2 s, and shutdown interrupts every retry wait.

The mDNS browser and source fold are initialized only after Bus storage opens.
The same worker then publishes its first honest mesh/gateway/mDNS roster
immediately, without a daemon restart or fabricated source. Existing
publish-on-change and heartbeat behavior is unchanged.

## Focused farm proof

Host: machine 193 (`172.20.0.90`)

Slot: `media-sources-bus-r27`

```text
cargo test -p mackesd --features async-services --lib \
  workers::media_sources::tests::media_sources_bus_root_preserves_override_and_has_system_fallback \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,442 filtered out`.

```text
cargo test -p mackesd --features async-services --lib \
  workers::media_sources::tests::late_bus_recovers_and_publishes_sources_without_worker_restart \
  -- --exact --nocapture
```

Result: `1 passed; 0 failed; 4,442 filtered out`. One worker remained alive
behind an unopenable Bus path, published its initial source roster when that
path became usable, remained active, and stopped promptly on shutdown.

The exact source passed remote `rustfmt --edition 2021 --check` and local scoped
`git diff --check`. No broad suite, package build, live mDNS fixture, or
unrelated test was run.
