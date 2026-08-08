# WL-FUNC-021 — desktop-source recurring scan phase audit (2026-08-07)

## Finding

`DesktopSourcesWorker` performs one immediate Workload snapshot and roster
publication, then polls actions and mDNS on a two-second cadence. The existing
fingerprint gate already suppresses unchanged non-heartbeat writes; refreshes
and the 30-second heartbeat intentionally remain forced publications.

## Scoped change

`crates/mesh/mackesd/src/workers/desktop_sources.rs` now derives a stable
node-id phase bounded to 0–1,500 ms for the first recurring scan. The first
recurring scan is scheduled at `tick - phase`, so it is no later than the old
two-second boundary; later scans retain the configured cadence. The immediate
initial roster, refresh-triggered re-enumeration, forced refresh publication,
heartbeat publication, and shutdown handling are unchanged.

## Verification

Farm host `.90`, slot `desktop-sources-phase-r1`:

```text
MCNF_BUILD_HOST=172.20.0.90
MCNF_BUILD_SLOT=desktop-sources-phase-r1
install-helpers/xcp-build.sh cargo test -p mackesd --lib desktop_sources \
  --features async-services --locked -- --nocapture

test result: ok. 44 passed; 0 failed; 0 ignored; 0 measured;
4363 filtered out
```

`docs/platform/WORKLIST.md` was not edited. This is source/farm evidence only;
live multi-seat CPU reduction still requires reachable installed seats.
