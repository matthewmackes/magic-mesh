# WL-FUNC-019 — retained UPnP/SSDP catalog fold (2026-08-06)

## Goal slice

Consume the already admitted, bounded UPnP/SSDP roster through the same
universal resource catalog and discovery publication path used by desktop and
SSH/X11 source rosters. Keep the connect action honest until a typed session
handoff exists.

## Implementation

- `upnp_sources.rs` now exposes `append_upnp_cards`, which validates the strict
  retained state, collapses exact duplicate cards deterministically, and rejects
  source or catalog identity collisions.
- `service_catalog.rs` adds an optional UPnP source input while preserving the
  existing constructor for older callers. The new fold feeds the same digest and
  catalog validation boundary as every other resource lane.
- `service_aggregator/mod.rs` reads `state/resources/upnp` with the existing
  2 MiB pre-decode bound and strict semantic decoder. Missing state is normal;
  malformed state fails closed before either catalog or discovery mirror is
  published.
- The UPnP card remains unavailable for `connect-dlna-upnp` because no typed
  mackesd session handoff exists yet. Discovery is not represented as a fake
  executable client.

## Farm verification

Commands were run through `install-helpers/xcp-build.sh` with explicit farm
hosts and isolated slots:

- `.90`, `MCNF_BUILD_SLOT=upnp-r1`: `cargo test -p mackesd optional_upnp_state`
  — 1 passed, 0 failed.
- `.90`, same slot: `cargo test -p mackesd upnp_sources` — 5 passed, 0 failed.
- `.90`, same slot: `cargo test -p mackesd
  valid_retained_upnp_row_appears_in_catalog_and_discovery` — 1 passed, 0
  failed.
- BigBoy `.130`, `MCNF_BUILD_SLOT=upnp-full-r1`: `cargo test -p mackesd` —
  4409 core tests passed, 55 `mackesd` binary tests passed, 6 `meshctl` tests
  passed, all package integration tests passed, one live-fleet test ignored by
  its explicit live-environment contract, and doc-tests passed.
- `.50`, `MCNF_BUILD_SLOT=upnp-fmt-r1`: package `cargo fmt --check` reports
  pre-existing formatting drift across the shared dirty `mackesd` crate. A
  direct farm `rustfmt --check` on the three touched files reports the same
  pre-existing drift plus import/order differences; no bulk formatter rewrite
  was applied because the worktree contains broad user-owned changes.
- Local `git diff --check` passed.

## Honest remaining boundary

This slice does not claim live `rupnp` socket discovery, interface-bound
kernel reception, device-description fetch/control, DLNA session handoff,
Moonlight, or live seat acceptance. The existing adapter still requires an
explicit interface/source context from its eventual receiver, and its connect
action stays unavailable until that receiver and typed client are implemented.

## Source scope

The change is limited to the UPnP source adapter, service catalog and
aggregator integration, this evidence record, and the canonical Worklist
checkpoint. The worktree contains unrelated user changes that were preserved.

## SHA-256 at handoff

```text
38700fdd5140825508dcdabfa524c17011d3a012a013e2f1bab4be178a6559f9  crates/mesh/mackesd/src/workers/upnp_sources.rs
9b9eabecc437e8aa78f2ddd465292d075076f8746e1687263a24a20049e90703  crates/mesh/mackesd/src/workers/service_catalog.rs
e52b9d1122fd5156f4e880e3b20f679ada701522c7f563432166c86fd8ac4077  crates/mesh/mackesd/src/workers/service_aggregator/mod.rs
517debff00b3d385c430d3cf46f14d2209bd99bdf7202e54c5e0b76ff4365449  docs/platform/WORKLIST.md
```
