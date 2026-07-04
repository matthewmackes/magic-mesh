# EXPLORER — the Hero unit explorer

Operator-locked 2026-07-04 (25-Q `/plan` survey). A focused, cinematic interface
that presents **every discovered unit** — inside the mesh, outside the mesh, and
**every object created in OpenStack on any node** — one at a time as a large "Hero"
card, navigable like a media shelf. Carbon Design principles throughout.

## Locked decisions

| # | Area | Lock |
|---|------|------|
| 1 | Unit taxonomy | **Three kinds, one stream** — mesh peers (inside), off-mesh LAN devices (outside), AND OpenStack objects, all first-class units distinguished by a type badge. |
| 2 | Mesh source | **mackesd mesh mirror** — the live peer directory + lighthouse roster + health the tray/Fleet already read. |
| 3 | Off-mesh scan | **Active nmap-style probe** — subnet ping-sweep + light port fingerprint for type detection, plus mDNS/DNS-SD + ARP. The most complete discovery (noisier by choice). |
| 4 | Cloud objects | **All four resource kinds** — Nova instances + Cinder volumes + Glance images + Neutron networks, each its own hero unit. |
| 5 | Hero layout | **Full-bleed hero + filmstrip** — one unit fills the surface; a thin thumbnail filmstrip along the bottom shows neighbors + jumps. |
| 6 | Navigation | **Arrows + filmstrip + search** — Left/Right page, filmstrip-click jumps, `/` search jumps by name/type. |
| 7 | Ordering | **Proximity: mesh → LAN → cloud** — segmented by trust/proximity, name within each. |
| 8 | Grouping | **Segmented + filter chips** — category dividers in the filmstrip (Mesh / LAN / Cloud) + top chips to scope to one. |
| 9 | Hero visual | **Type glyph + live status ring** — a large Carbon type glyph wrapped in a health ring that animates with real telemetry. |
| 10 | Identity | **Name + type + reachability** — big display name, type/category badge, clear reachability line (in-mesh / on-LAN / cloud-object + address). |
| 11 | Telemetry | **Rich when reachable** — load/mem/net sparklines + uptime for units we can read (mesh peers, instances); summary badges for the rest. |
| 12 | Honesty | **Dimmed minimal card** — outside/unreachable units get a visually dimmed, stripped card showing only what's known (no faked fields, §7). |
| 13 | Action model | **Rich per-type action set** — each unit kind exposes its real verbs on the hero; the explorer is a launchpad, not just a viewer. |
| 14 | Peer actions | **Open + health-check + evict (armed)** — open in Fleet plane, live health/ping, evict-from-mesh behind **typed arming**. |
| 15 | Cloud actions | **Full lifecycle, armed destructive** — instances: console (SPICE/VNC) + start/stop/reboot + delete; volumes/images/networks: inspect + delete. Every mutating verb behind **typed arming**. |
| 16 | Adopt (LAN) | **Offer 'Invite to mesh'** — an off-mesh unit's hero offers an armed adopt/enroll action kicking the existing pairing flow. |
| 17 | Home | **Fold into the Discovery surface** — grow the existing Discovery surface into this hero view (reuses its plumbing; the join/pair purpose folds in as the adopt action). |
| 18 | Data path | **One aggregator worker → bus** — a mackesd discovery-aggregator worker unions the three sources into one `state/units/*` stream; the shell stays a thin renderer (§6). Scanning + privilege live in the daemon. |
| 19 | Refresh | **Event-driven + periodic rescan** — mesh/openstack push instantly; the LAN scan re-runs on an interval (~30–60s) + on manual Rescan. |
| 20 | Cloud union | **Union every node's mirror** — union all `state/openstack/<node>` (QC-2), dedup by object id, tag each unit with its host node. No center. |
| 21 | Motion | **Carbon slide + fade** — units slide horizontally with productive-motion ease + light cross-fade (~200ms Motion tokens). |
| 22 | Density | **Adaptive** — cinematic full-bleed when the surface is large; collapses to a productive list-detail in compact shell sizes. |
| 23 | Empty state | **Show self, scan in background** — immediately present THIS node as the first hero unit + an honest "scanning…" line as others stream in. Never blank. |
| 24 | Scan lifecycle | **Open + warm cache** — the active LAN scan runs only while Explorer is visible; the last result is cached so re-opening shows the prior set instantly, then refreshes. Mesh/openstack mirrors are always-maintained (free). |
| 25 | MVP cut | **All three at once** — mesh + LAN + cloud together in the first cut: the aggregator worker (incl. the scan) + the Discovery-surface hero fold all land before it ships. |

## Architecture

Two halves, split on the mesh/desktop boundary (§6):

**Daemon — `mackesd` `unit_aggregator` worker (new):**
- Unions three sources into a typed `Unit` model + publishes `state/units/<node>`
  (or a single folded `state/units`): (a) the mesh mirror (peers, rank/health), (b)
  an **active LAN scan** submodule (mDNS/DNS-SD listen + ARP/neighbor read + a bounded
  ping-sweep & light port fingerprint over the local subnet — reusing the bounded-proc
  path; scan only runs while a "scan active" flag is set by the open surface, lock #24),
  (c) the union of every node's `state/openstack/<node>` mirror from QC-2, dedup by
  object id, host-node-tagged (lock #20).
- Each `Unit`: `{ id, kind: Peer|LanHost|Instance|Volume|Image|Network, name,
  reachability: InMesh|OnLan|CloudObject{node}, address?, health?, telemetry?, actions[] }`.
  Unreachable/partial units carry explicit `unknown` for unprobed fields (lock #12/§7).
- Event-driven publish-on-change + a periodic rescan tick for the LAN half (lock #19);
  a warm cache persists the last scan (lock #24).

**Shell — the Discovery surface hero fold (`mde-shell-egui`):**
- The existing Discovery surface grows the hero presentation: full-bleed hero card +
  bottom filmstrip + top category chips + `/` search (locks #5–8); Carbon slide+fade
  paging (#21); adaptive cinematic↔list-detail by shell size (#22).
- The hero card: large type glyph + live status ring (#9), name/type/reachability
  headline (#10), rich telemetry sparklines when reachable else dimmed-minimal (#11/12),
  and a per-type action bar (#13): peer → open-in-Fleet / health-check / evict(armed);
  cloud → console / start / stop / reboot / delete (all mutating armed); LAN → invite
  (armed) (#14/15/16). Arming reuses the platform typed-confirm idiom.
- Subscribes to `state/units*`; renders self as the first unit at empty (#23); sets the
  aggregator's scan-active flag only while visible (#24).

## Acceptance (runtime-observable)
- Opening Discovery shows a hero card for THIS node immediately, then mesh peers,
  LAN hosts, and OpenStack objects stream in as distinct hero units with type badges.
- Left/Right pages the hero (Carbon slide+fade); the filmstrip jumps; `/` filters;
  category chips scope to Mesh/LAN/Cloud.
- A reachable peer's card shows live telemetry; an unreachable LAN host shows a dimmed
  minimal card with explicit unknowns (no faked fields).
- Per-type actions fire the REAL seams: open-in-Fleet switches surface; a cloud
  instance's console opens SPICE/VNC; stop/reboot/delete + evict + invite each require
  typed arming before executing.
- Cloud objects from multiple nodes appear once each, tagged with their host node.
- The active LAN scan runs only while the surface is visible; re-opening shows the
  cached set instantly then refreshes.

## Risks
- **Active scan privilege/noise** — raw ping-sweep/ARP needs the daemon's privilege;
  keep it bounded + surface-gated (#24) so it isn't constant background probing. It runs
  in the daemon, never the GUI (§6).
- **Discovery-surface fold** — the surface has a join/pair purpose today; the fold must
  preserve enrollment (it becomes the LAN "invite" action, #16) without regressing it.
- **Unit id stability** — dedup + stable ids across sources (a peer that's also a cloud
  host shouldn't double-list); define the id scheme carefully.
- **Telemetry availability** — sparklines only where a real source exists; everything
  else honestly summary/unknown (§7), never synthesized.

## Out of scope
- Editing OpenStack objects beyond lifecycle (create flows stay in the Cloud plane).
- Historical/time-travel views; alerting.
- Cross-mesh (federated) unit discovery beyond the local mesh + local LAN.

## Tasks → `docs/WORKLIST.md` EXPLORER-1..6.
