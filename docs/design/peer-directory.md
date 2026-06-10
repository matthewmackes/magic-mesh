# PEERS — Directory of Mesh Peers

**Date:** 2026-06-09 · **Survey:** 26 questions (3 rounds) + 3 operator directives ·
**Status:** locked, lifted into `docs/WORKLIST.md` (### PEERS)

**The Front Door to the platform** (operator directive D2): when the mesh is fully
running, the Peers directory is the first thing you see — what the network *offers*
(every peer, every service: remote access, Podman, KVM, media, voice) and an advanced
view of the health and design of the mesh (presence, sync currency, drift, Netdata
health, the live path map). It is the single operational roster: every known peer, its
live state, the services it provides, and every operation you can perform on it —
Call, SSH, RDP, VNC, augmented trace (Ping+Traceroute merged), update nudge, metrics.
Grown out of `mesh_topology`, not bolted beside it.

## Pre-survey ground truth

No directory existed. Peer data was scattered: `mesh_topology` (table+graph, read-only
modal), `remote_desktop` (RDP/VNC via remmina), `drift` (per-peer events), `home`
(count badge), mde-files sidebar. SSH was the dead B1 nav stub; Ping/Traceroute existed
nowhere; presence/version/sync lived in `PeerRecord` but had no per-peer surface.

## Locks

| # | Question | Lock |
|---|----------|------|
| Q1 | Placement | **Evolve `mesh_topology`** into the directory; graph stays as secondary view |
| Q2 | Name | Nav label **"Peers"** |
| Q3 | Layout | **Master-detail** — peer list left, detail + action toolbar right |
| Q4 | Roster scope | **All known peers, grouped** — Online first, Offline grayed, ops disabled |
| Q5 | Self | **Pinned first**, "(this machine)"; self-inapplicable ops hidden, diagnostics stay |
| Q6 | KDC devices | **Yes, separate group** below mesh peers, reduced op set (presence/ping-class only) |
| Q7 | CR-6.c modal | **Retired** — graph node click jumps to directory detail |
| Q8 | Remote Access panel | **Keep both, shared launcher engine** — remmina/launch code extracted into one module both consume |
| Q9 | Roster source | **Enriched Bus verb** — mackesd joins nodes + PeerRecord + role + descriptors once; no GUI shell-outs |
| Q10 | Refresh | **Bus subscription + ~30 s poll floor** |
| Q11 | Presence | **Heartbeat age tiers** — Online ≤ 2 min, Idle ≤ 10 min, Offline beyond (`last_seen_ms`) |
| Q12 | Presence rows | **Two fields** — machine (heartbeat) + voice (`state/voice/status`); Call gates on voice |
| Q13 | Ping | **Operator directive: Ping + Traceroute merge into one visual augmented traceroute**, leveraging each host's scanning/polling |
| Q14 | Trace data | **Overlay path report** — handshake state, direct vs lighthouse relay, underlay endpoint, NAT class |
| Q15 | Sync level | **Revision currency** — synced / behind N / unknown (FPG apply-acks) |
| Q16 | Update | **Show + nudge** — version + currency badge; "Apply now" publishes a targeted reconcile nudge; never forks per-peer state |
| Q17 | Voice Call | **Bus verb** `action/voice/dial {peer}` → the autostarted agent pops the HUD |
| Q18 | SSH | **cosmic-term** spawning `ssh <user>@<overlay-ip>` |
| Q19 | Op availability | **Peer-published service descriptors** (no live probing); buttons enable from the roster record |
| Q20 | Drift | **Count + last event** in detail → link to Drift panel pre-filtered to the peer |
| Q21 | Roles | **Read-only badge** — mutation stays the ENT-2 `role pin` flow at the box |
| Q22 | Feedback | **Inline result strip** under the action toolbar (errors in Carbon danger) |
| Q23 | CLI parity | **`mackesd peers`** prints the same joined record (feeds ENT-8 / ENT-15) |
| Q24 | Trace visual | **The graph view becomes the live map** — RTT-weighted edges, direct/relay styling, edge click → augmented trace card |
| Q25 | Live wallpaper | **In this epic** — the live map scene renders as the Cosmic desktop background |
| Q26 | Netdata | **All four roles** (operator: "Netdata should be playing a large role"): detail-pane live sparklines · trace-map probe layer · health badge source · dashboard deep-link |
| D1 | Services inventory | **Operator directive:** detail pane lists services each peer provides — **Podman** (containers), **KVM/libvirt** (guests + state), **media services** (mde-musicd etc.), alongside remote access |
| D2 | Front Door | **Operator directive:** the Peers directory is the **Front Door to the platform** — the Workbench launches into it; it is the canonical view of what the running mesh offers and of its health + design. The Overview (home) panel remains but is demoted from landing surface |

**Q26 vs the old Q95/96 lock:** Netdata stays local and there is still **no central
aggregation** — the directory does peer-to-peer pulls of each peer's own Netdata
(REST :19999 over the overlay). The old lock is amended, not broken.

## Architecture

### Data plane
- **`action/mesh/directory`** (new mackesd Bus verb): returns the joined per-peer
  record — hostname, overlay IP, role, machine presence (heartbeat tier), voice
  presence, `mde_version`, revision currency, drift count + last event, health
  (Netdata-alarm-derived), and the **service descriptor set**.
- **Descriptor publishing:** each peer's mackesd probes locally (sshd / xrdp / vnc
  listening; `podman ps`; libvirt guest list + state; media daemons; Netdata alarm
  state) and writes the result into its replicated `PeerRecord` / `PeerProbe`
  (`descriptors.mesh_services` grows). No remote probing anywhere.
- **Netdata pulls:** detail-pane sparklines and trace-map edge activity query the
  *selected* peer's `:19999` REST API over the overlay, on demand. Health badges come
  from the replicated alarm summary, not live pulls.
- **Path probes:** the transport RTT + path probe (shared implementation with ENT-13)
  feeds both the directory RTT figure and the live-map edges.

### Operations plane
- **Shared launcher module** (extracted from `remote_desktop.rs`): remmina (RDP/VNC),
  cosmic-term + ssh. Both the Peers directory and the Remote Access panel consume it.
- **Call** = `action/voice/dial`. **Apply now** = targeted `action/fleet/reconcile`
  nudge. **Metrics** = browser deep-link to the peer's Netdata dashboard.
- Buttons gate on: machine presence (offline → disabled), voice presence (Call),
  descriptors (SSH/RDP/VNC), revision currency (Apply now only when behind).

### UI
- `mesh_topology` panel → **"Peers"**: master-detail list view (default) + live-map
  view (the grown GraphProgram). Self pinned first; Online / Offline groups; KDC
  devices as a third group with reduced ops.
- Detail pane: identity header (role badge, overlay IP, version), two presence fields,
  action toolbar, inline result strip, Netdata sparklines, Services Provided section
  (remote access / Podman / KVM / media), drift + sync rows.
- **Live map:** nodes = peers (presence-styled), edges = transport paths (RTT label,
  direct vs relay styling, Netdata-driven activity weight, unreachable ×). Edge click
  → augmented trace card (the Q13/Q14 merged op).
- **Wallpaper target:** the same live-map scene rendered to the Cosmic desktop
  background as a separate output of the canvas engine.

## Acceptance (epic-level)

- **Front Door:** launching `mde-workbench` with no `--focus` flag lands on Peers; the
  nav lists Peers first.
- Open Workbench → Peers: every known peer listed, grouped Online/Offline, self first,
  phones in a Devices group; all data from one Bus verb (zero GUI shell-outs).
- On an online peer offering them: Call pops the HUD ringing the peer; SSH opens
  cosmic-term connected; RDP/VNC open remmina connected; the buttons are *absent or
  disabled* on peers whose descriptors don't offer the service.
- Trace on any peer renders the visual path (direct or via lighthouse, RTT, NAT class)
  and the live map shows the same edge; results land in the inline strip.
- A peer behind on revisions shows "behind N" + Apply now; the nudge converges it.
- Detail pane shows live CPU/net sparklines from the peer's own Netdata; Metrics opens
  its dashboard; a Netdata alarm flips the health badge.
- Podman containers, KVM guests (with run state), and media services appear per peer.
- `mackesd peers` prints the same record set the GUI shows.
- The live map renders as the desktop wallpaper when enabled.

## Risks

- **Wallpaper surface under Cosmic:** cosmic-bg owns the background; rendering an iced
  scene there needs a layer-shell surface or cosmic-bg integration — the riskiest item;
  prototype early (it is why Q25 was offered as a separate epic).
- **FPG dependency:** revision currency needs FPG-5 apply-acks; until FPG lands the
  sync field degrades to "unknown" honestly (no fake data, §7).
- **Voice dependency:** Call gating needs SVC-4 (every peer publishes voice status).
- **Netdata exposure:** :19999 must bind/firewall to the overlay interface only.
- **Descriptor freshness:** service lists are heartbeat-replicated, so a just-stopped
  sshd may be offered stale for one cycle — acceptable; the launch fails honestly in
  the result strip.

## Out of scope

- Remote role re-pin (ENT-2 owns role mutation at the box).
- Per-peer divergent updates (broadcast FPG model holds; the nudge only hurries
  convergence).
- Central metrics aggregation (Q95/96 holds — peer-to-peer pulls only).
- Actions on Podman/KVM/media entries (start/stop containers or guests) — v1 is
  inventory display; operations on those services are a future epic.
- KDC device operations beyond presence/ping-class (phones keep their KDC surface).
