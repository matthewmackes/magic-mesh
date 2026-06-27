# Mesh-Map Wallpaper — EtherApe-like global mesh traffic (MESHMAP)

**Status:** Locked (survey 2026-06-27). **Vision:** turn the static topology
wallpaper into an **EtherApe-like animated global-mesh-traffic map** — nodes placed
**geographically**, with **per-direction packet-particle trails colored by the
sending node**, intensity from the data each node collects. Ambient desktop
wallpaper (no interaction), zero-CPU at idle.

## Current state (research)
`mde-mesh-wallpaper` (`crates/workbench/mde-workbench/src/bin/mde-mesh-wallpaper.rs`)
is an iced/wgpu wlr-layer-shell background. It renders a **static** force-directed
`MapProgram` (shared with the Peers panel): presence-colored nodes, RTT-labeled
edges, lighthouse beacons. The particle-flow code exists in `MapProgram` but is
**hardcoded off** (`flow:0.0`). Data: `mackesd peers --json` roster, latency cache,
lighthouse IPs; the Netdata `system.net` flow sampler (`peers.rs::sample_flows`)
already exists for the Peers panel.

## Locks

| # | Area | Lock |
|---|------|------|
| W1 | Traffic data | **Netdata per-node throughput as the intensity proxy** (`sample_flows`); real per-link byte counters = Phase 2 |
| W2 | Node color | **Stable hash(hostname) → hue** (deterministic, same across reboots/nodes) |
| W3 | Path color | **Per-direction — two particle trails per edge, each colored by its SENDER** |
| W4 | Layout | **Geographic** — nodes placed by location, faint world-map backdrop |
| W5 | Animation | **Packet particles** (EtherApe): dots flow along edges, density+speed ∝ throughput |
| W6 | Geo source | **DO region** (zone1-do tofu) for droplets + **public-IP geo** (netassess `public_ip`) for on-prem |
| W7 | Relay paths | **Relayed paths bend through the lighthouse** (two segments via the relay node); direct = straight |
| W8 | Idle/perf | **Zero-CPU idle** (gate particles on `has_flow`); roster/geo refresh 30s AC / 5min battery |

## Architecture
- **Render:** extend `mde-mesh-wallpaper` with a **geographic map mode** — a faint,
  low-contrast Carbon-toned world/region backdrop; nodes projected from lat/long.
  Reuse the iced/wgpu canvas + the Netdata flow sampler; replace the force-layout
  with geo-projection (keep force-layout as a fallback when geo is unknown).
- **Geo placement (W4/W6):** per node → lat/long from DO region centroid (the
  `zone1-do` droplet region) or a geoIP lookup of the node's netassess `public_ip`
  (an offline geoIP table / coarse region centroids — no network call). Behind-NAT
  on-prem nodes resolve to their ISP/public-IP location (approximate, documented).
- **Node color (W2):** `hue = hash(hostname) % 360`, fixed saturation/lightness from
  Carbon tokens → a stable distinct per-node color. Node dot still carries the
  presence tint (online/idle/offline) as a ring; the hue identifies the node.
- **Paths + particles (W3/W5):** an edge exists between nodes that communicate (every
  node↔its lighthouses; peer↔peer where a path is tracked). Each edge renders **two**
  particle streams — one per direction — each colored by that direction's **sender**
  hue. Density/speed ∝ the sender's Netdata `system.net` throughput (the W1 proxy:
  a node's total throughput is attributed across its active edges).
- **Relay (W7):** `mesh_router` `PeerPath.primary` tells direct vs `NebulaRelay`; a
  relayed path draws as two segments **through the relaying lighthouse** node; direct
  paths are straight.
- **Idle/perf (W8):** particle ticks gated on `has_flow()` (zero ticks at rest, the
  current pattern); roster/geo refresh adaptive (30s AC / 5min battery); **reduce-motion
  → static** colored edges (no particles), per Carbon §4 + WCAG 2.3.1 (≤3Hz, 80ms cap).
- **Carbon (§4):** node hues from a Carbon-derived hue wheel; map backdrop from the
  Gray ramp; all motion via `mde_theme::motion`; no raw hex.

## Data gap (W1, honest)
There is **no per-link byte counter** today — only per-node Netdata `system.net`
throughput + path type (direct/relay). v1 attributes each node's throughput across
its active edges (a proxy: visually correct "who is busy", approximate per-edge
split). **Phase 2 (MESHMAP-6):** a mackesd collector reading per-peer-IP byte counters
(nftables accounting / Nebula stats) for true per-link traffic.

## Acceptance (runtime-observable)
- The wallpaper shows the live mesh: every node at its geographic position on a faint
  map, labeled, with a stable distinct hue + presence ring.
- Active edges animate per-direction packet particles colored by the sending node;
  speed/density rise with that node's throughput; an idle mesh shows no particles.
- A lighthouse-relayed path visibly bends through the lighthouse; direct paths are straight.
- reduce-motion → static colored edges (no animation); idle → zero CPU.

## Risks
- **GeoIP source:** needs an offline geoIP table (size/licensing) or coarse region
  centroids; behind-NAT nodes geo-locate to their ISP (approximate).
- **Per-node-as-per-link proxy** is approximate until MESHMAP-6.
- **Wallpaper GPU cost** must stay zero at idle (has_flow gate is load-bearing).

## Out of scope
Interactivity (it's an ambient wallpaper — clicks fall through); real-time packet
capture/DPI; protocol-level coloring (EtherApe colors by protocol — here we color by
node, per the operator's spec).

## Worklist → MESHMAP-1..6 (see docs/WORKLIST.md)
