# WL-FUNC-021 — alert-relay startup phase (2026-08-07)

## Finding

`AlertRelayWorker` used the same `tokio::time::sleep(2s)` boundary on every
seat. When daemons started together, each seat performed the alert-directory
`read_dir` sweep together, creating avoidable common-mode filesystem work even
when no alert was present.

## Change

`crates/mesh/mackesd/src/workers/alert_relay.rs` now derives a stable phase from
`MACKESD_NODE_ID`, `HOSTNAME`, or `/etc/hostname`. The first sweep waits for the
configured tick minus that phase; the phase is bounded to half the configured
tick and at most one second. Therefore the first poll is no later than the
previous two-second freshness boundary, while identical daemon startups are
spread across that boundary. Subsequent polls remain on the existing cadence.
Shutdown remains selected during the initial delay and every recurring wait.

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.90
MCNF_BUILD_SLOT=alert-relay-phase-r1
install-helpers/xcp-build.sh cargo test -p mackesd --lib alert_relay \
  --features async-services --locked -- --nocapture
```

Result: **13 passed, 0 failed, 4,390 filtered out** on the Fedora farm VM
`.90`.

## Caveat

This is source-level and farm-test evidence only. It does not prove live
five-seat CPU reduction or alert latency on installed hardware; those require
the authorized live-seat acceptance run when the target is reachable.
