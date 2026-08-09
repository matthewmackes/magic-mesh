# WL-CRIT-007 — Boot Readiness Bus fallback (r18)

Date: 2026-08-09

Source baseline: `f14d3a0c`

`BootReadinessWorker` previously returned permanently when
`mde_bus::default_data_dir()` could not resolve a user data directory. That is
a valid system-service environment with no `HOME` or `XDG_DATA_HOME`, and the
Bus contract explicitly identifies `/run/mde-bus` as the shared daemon spool.

The worker now preserves an explicit/configured root and otherwise falls back
to `mde_bus::SYSTEM_BUS_ROOT`. A resolved spool that is temporarily unopenable
continues to be retried by the existing two-second publication loop, so the
readiness authority no longer disappears for the lifetime of mackesd.

Focused farm verification on machine 193 (`172.20.0.90`), slot
`boot-bus-fallback-r13`:

```text
cargo test -p mackesd --lib bus_root_has_the_documented_system_fallback -- --nocapture
running 1 test
test workers::boot_readiness::tests::bus_root_has_the_documented_system_fallback ... ok
test result: ok. 1 passed; 0 failed; 4412 filtered out
```

No broad suite or unrelated test was added.
