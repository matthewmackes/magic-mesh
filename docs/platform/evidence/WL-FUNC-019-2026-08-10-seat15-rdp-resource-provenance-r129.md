# WL-FUNC-019 — seat 15 RDP resource and provenance lifetime (r129)

Date: 2026-08-10

Base revision: `5acd2238`

## Live Release 32 observation

Seat 15 (`172.20.0.15`) was inspected read-only after the signed Fedora 44
Release 32 rollout. It reported `magic-mesh-12.1.6-32.x86_64`, all six grouped
`mackesd` services active, `mde-shell-egui.service` active with zero restarts,
and `mcnf-resource-publisher-credential.service` active.

The latest service projection preserved two independent records for the same
Windows host instead of collapsing them:

- `172.20.146.54:3389`, kind `ms-wbt-server`, health `up`, provenance `probe`;
- `172.20.146.54:22`, kind `ssh`, health `up`, provenance `probe,enrichment`.

Catalog revision
`peer-basement-test-workstation-1786364953569` promoted the first record into
`Remote Desktop · 172.20.146.54`, class `desktop`, RDP transport, health
`available`, with `connect-rdp-0` explicitly `requires_approval`. The generic
SSH service card remained separate. The retained discovery projection carried
the same revision and catalog digest
`catalog:v1:dd4f71ff87399cfc63a098fabea5c360e3b5666191a3fd88cac39ebcfabfad01`.

This proves network discovery through the authoritative resource catalog and
matching discovery projection. `state/desktops/sources` is a legacy roster and
is not the universal-resource authority.

## Provenance correction

The promoted card and connect action correctly retained the probe observation
for the bounded five-minute probe lease, but the replacement mesh-directory
provenance used the generic one-minute service lifetime. The card therefore
remained visible and approval-gated after its only displayed provenance had
expired.

`probed_rdp_card` now binds that provenance to the same five-minute authority
window as the probe-only service observation. Checked timestamp arithmetic
still fails closed, and public, malformed, stale, unconfirmed, or wrong-port
records remain excluded by the existing admission boundary.

## Focused farm proof

Machine 193 (`172.20.0.90`), slot `func019-rdp-provenance-r129`, passed:

```text
cargo test -p mackesd \
  rdp_promotion_survives_the_gap_between_card_and_probe_ttls -- --nocapture

test result: ok. 1 passed; 0 failed; 4674 filtered out
```

No broad suite or duplicate live-seat activity was run.

## Remaining boundary

This checkpoint does not claim that a screenshot was captured or that Windows
credentials were entered. A local approval, authenticated RDP login, rendered
session, and disconnect/reconnect trace remain required for end-to-end S5
acceptance.
