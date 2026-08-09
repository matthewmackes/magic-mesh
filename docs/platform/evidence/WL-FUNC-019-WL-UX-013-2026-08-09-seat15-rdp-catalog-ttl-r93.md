# WL-FUNC-019 / WL-UX-013 — seat 15 RDP catalog TTL recovery r93

Date: 2026-08-09

## Live diagnosis

Basement seat 15 (`172.20.0.15`, Fedora 44, `magic-mesh-12.1.6-24`) was
inspected read-only. Its observation, integrations, shell, and aggregate targets
were active. The Windows endpoint remained explicitly bounded in the root-owned,
mode-0600 `/root/.config/mde/probe-targets.toml` as `172.20.146.54`.

The route used seat 15's physical `eno1` interface, and a bounded TCP connection
to `172.20.146.54:3389` succeeded. The current probe inventory contained a host
record stamped `1786311862` with both SSH and `ms-wbt-server` on TCP 3389.

The universal catalog exposed the typed `probe-rdp/172.20.146.54` Desktop card
at generation `1786311961138`, then dropped it at generation `1786312021138`
while retaining a generic inspect-only `ms-wbt-server` service card. The endpoint
and probe record remained available throughout. This reproduces the reported
intermittent Nodes absence downstream of successful LAN discovery.

## Root cause and bounded correction

The service aggregator admits probe-only records for five minutes, matching the
bounded scan/freshness contract. `probed_rdp_card`, however, independently
stopped promoting the same valid record after two minutes (`CARD_MS`). Seat 15's
scan can run longer than that, creating a gap between typed-card expiry and the
next completed inventory publication.

RDP promotion now uses a dedicated five-minute maximum observation age. The
existing boundaries are unchanged: promotion still requires probe provenance,
a non-future timestamp, TCP port 3389, and a private/link-local endpoint. Records
older than five minutes, public endpoints, advertised-only rows, malformed
addresses, and wrong ports remain non-connectable.

The prior r74 `/24` quiet-host admission remains valid but is not seat 15's
complete live path: `172.20.146.54` is outside seat 15's local `/24` on its
`172.20.0.0/16` LAN, so the existing bounded explicit-target configuration is
still required.

## Focused farm verification

Host: machine 194 build VM `172.20.0.50`

Slot: `func019-seat15-rdp-ttl-r93`

```text
cargo test -p mackesd --features async-services \
  workers::service_catalog::tests::rdp_ -- --nocapture
```

Result: 4 passed, 0 failed, 4622 filtered out. This includes the new regression
that preserves a typed card just beyond the old two-minute cutoff and the
existing hostile/stale, unavailable, and successful-promotion cases. The build
emitted pre-existing warnings outside this bounded change.

## Live limitation

The corrected source was not packaged or installed on seat 15 in this slice.
Therefore this evidence does not claim continuous installed-card presence,
authenticated Windows login, rendered RDP pixels, or input. A corrected artifact
must be deployed and observed across the old two-minute boundary and a complete
scan transition before the live issue is closed.
