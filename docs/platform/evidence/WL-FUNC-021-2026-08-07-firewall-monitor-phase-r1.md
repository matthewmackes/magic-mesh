# WL-FUNC-021 — firewall monitor startup-phase audit (2026-08-07)

## Finding

`firewall_monitor` already doubled quiet/failed passes up to a bounded
60-second interval, but its first pass still followed the same five-second
sleep on every seat. That pass synchronously runs `journalctl`, reads/writes
the journal cursor, and may perform retention I/O, leaving a common-mode
startup burst before idle backoff can help.

## Change

`FirewallMonitorWorker::run` now derives a stable FNV-1a phase from the local
firewall log owner (the hostname supplied by the spawn path), capped at 1.5
seconds. It waits for `tick - phase` before the first pass, so the first probe
remains within the existing five-second freshness deadline while different
seat identities avoid launching the synchronous work together. The delay is
shutdown-aware; the existing activity reset and bounded idle backoff are
unchanged.

## Farm verification

Command, routed to Fedora farm VM `.90` (`172.20.0.90`):

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=firewall-monitor-phase-r1 \
  install-helpers/xcp-build.sh cargo test -p mackesd firewall_monitor \
  --features async-services --locked -- --nocapture
```

Result: **21 passed, 0 failed, 0 ignored; 4,391 filtered out**.

The regression covers stable identity mapping, distinct seat phases, the
1.5-second bound, the preserved first-pass deadline, and the empty-identity
fallback. Existing parser, filter, retention, threshold, and idle-backoff
tests also passed.

## Scope and remaining proof

Changed files for this audit are this evidence record and
`crates/mesh/mackesd/src/workers/firewall_monitor.rs`; `docs/platform/WORKLIST.md`
was not edited. Live multi-seat CPU acceptance still requires reachable Dell
seats and an installed package containing the current source; this farm gate
does not claim that runtime proof.
