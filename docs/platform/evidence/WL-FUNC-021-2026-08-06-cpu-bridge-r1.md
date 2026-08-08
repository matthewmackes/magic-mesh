# WL-FUNC-021 — HTTPS fallback idle CPU audit (2026-08-07)

## Finding

Read-only five-second per-thread samples on both approved seats identified a
single `tokio-rt-worker` as the dominant `mackesd` thread: 543 ticks on Dell
release 5 and 518 ticks on seat 15 release 4. Both daemons had roughly 50
runtime threads. Source inspection found the configured HTTPS UDP fallback
bridge draining its inbox on a 5 ms timer even when no fallback frame existed.

## Change

`crates/mesh/mackesd/src/workers/mesh_router.rs` now uses a bounded 50 ms
inbox-drain cadence. Normal mesh-router decisions remain on their existing
10-second cadence, and each drain still empties all queued frames; this reduces
idle fallback wakeups by 10x without changing packet admission or queue caps.

## Verification

Farm host `.50`, slot `mackesd-mesh-router-inbound-r1`, passed the focused
mesh-router lane: 26 tests passed, 0 failed (4369 filtered). The gate covers
router construction, path selection, HTTPS activation, owned-channel reader
delivery, shutdown, and cadence override.

The rebuilt Fedora 44 release-5 RPM was installed on Dell `.225` and
`mackesd.service` was explicitly restarted. The installed package and process
executable matched the rebuilt artifact digest, with the process starting
after package installation and zero service restarts during observation.

The 30-second read-only CPU proof (2-second samples) passed with
`max=385‰` and `mean=283‰` of one host CPU, below the declared `850‰`/`500‰`
limits. The live-seat verifier also passed. The observation-only provider-loss
probe saw healthy provider/catalog/state for all 15 samples and refused because
no natural outage occurred; provider-loss continuity remains unproven.
