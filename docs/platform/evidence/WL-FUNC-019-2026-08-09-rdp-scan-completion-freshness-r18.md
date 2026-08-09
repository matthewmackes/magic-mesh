# WL-FUNC-019 RDP scan-completion freshness — r18

Date: 2026-08-09

## Live diagnosis

Basement seat 15 routes directly to the Windows laptop at
`172.20.146.54`, and a bounded seat-side TCP connection to port 3389
succeeded. The live probe inventory also contains one host card for that
address with `ms-wbt-server` on port 3389.

The scheduled nmap pass took approximately four minutes. Probe cards were
stamped before nmap started, so most of the service aggregator's five-minute
freshness lease elapsed while the scan was still running. The RDP record was
observed absent from `state/services/peer:Basement-Test-Workstation`, then
reappeared after the next inventory write. This explains intermittent loss
without claiming a network outage.

The installed `magic-mesh-12.1.6-23.x86_64` payload has a second, independent
deployment gap: it projects the live endpoint as a generic inspect-only
`ms-wbt-server` service. The current source has the newer approval-gated typed
Desktop/RDP projection, but that source is not installed on seat 15 yet.

## Correction

Probe inventory cards now receive their host/service and `last_seen` timestamp
when the complete snapshot becomes available, after the bounded scan and deep
fallback finish. Slow scans can no longer consume their own freshness lease.

## Focused verification

Farm machine 194 (`172.20.0.170`), slot
`func019-rdp-freshness-r18`:

```text
cargo test -p mackesd --lib \
  probe_nmap::tests::completed_probe_owns_the_snapshot_freshness_timestamp \
  --locked -- --exact --nocapture

cargo test -p mackesd --lib --features async-services \
  workers::probe::tests::slow_probe_does_not_add_an_extra_cadence_delay \
  --locked -- --exact --nocapture
```

Both exact tests passed: **2 passed, 0 failed** across the two invocations. No
broad test was requested or used for this boundary.

## Remaining live boundary

Seat 15 still needs a governed package carrying both the typed RDP projection
and this scan-completion timestamp correction. Authenticated Windows login and
render proof additionally require operator-supplied Windows credentials.
