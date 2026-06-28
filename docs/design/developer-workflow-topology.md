# Developer Workflow Topology — the Workstation at the center

**Status:** reference UX/workflow view, synthesized from the two topology diagrams
(no new locks).
**Builds on:** [`services-topology-buildout.md`](services-topology-buildout.md) (what runs
where) + [`services-reachability-topology.md`](services-reachability-topology.md) (who
reaches whom). This third view answers **"what does the *developer* do, and which nodes
does each workflow touch"** — the human-centered relationship map, with the **Developer's
Workstation at the hub**.
**Sources:** the `mde-workbench` panel set (`crates/workbench/.../panels/*` — `provisioning`,
`datacenter`, `build_farm`, `routing`, `dns`, `music`, `mesh_services`, `peers_map`,
`fleet_*`, `remote_desktop`, …) · the Send-To pre-flight pipeline · `mackesd provision *`
/ `action/provision/*` · the Build→Eagle→DO promotion pipeline ([`zones.md`](zones.md)).

## The hub

The **Developer's Workstation** (a `workstation`-pinned, rank-2 Cosmic desktop) is where
the human sits. Every workflow below originates from a surface on **that** box:

- **`mde-workbench`** — the operator control surface (fleet · devices · health · logs · the
  Datacenter / Provisioning / Routing / Build-Farm panels).
- **`mde-files`** · **`mde-music` + `mde-musicd`** · **`mde-voice-hud`** · **`mde-kdc-host`** —
  the daily-use apps.

The Workstation is itself **NAT'd + firewalled** (reachability diagram): every spoke below
rides the **Nebula overlay**, anchored by the lighthouses (direct-punch → relay → `:443`).
The developer never thinks about that — they click a panel; the mesh makes the node reachable.

## Workflow relationship map (Developer-centric)

```
            ┌──────────────────────────────┐                 ┌──────────────────────────────┐
            │     MESH SYNC + etcd          │                 │      3 LIGHTHOUSES            │
            │     (shared fabric)           │                 │      (public anchors)         │
            │  • files sync everywhere      │                 │  • enroll → CA-signed join    │
            │  • fleet revisions reconcile  │                 │  • etcd: directory/leader/    │
            │  • directory/health/leader    │                 │    health (peers panel)       │
            └──────────────┬───────────────┘                 │  • relay + music.mesh DNS     │
                           │                                  └──────────────┬───────────────┘
              save a file / author a revision                   "who's up?" + join + stream
               → it appears on every node                        (home · peers · lighthouses)
                           │                                                  │
                           │  ╔══════════════════════════════════════════╗   │
                           └─▶║                                          ║◀──┘
                              ║        ★ DEVELOPER  (WORKSTATION) ★       ║
       ┌──────────────────────║  Cosmic desktop · mde-workbench          ║──────────────────────┐
       │   collaborate         ║  mde-files · mde-music · voice · kdc     ║   provision + build   │
       │   (peer workflows)    ║                                          ║   (the dev loop)      │
       │                       ╚════╦═══════════════════╦═════════════════╝                       │
       ▼                            ║                   ║                                          ▼
┌──────────────────────┐           ║                   ║                          ┌──────────────────────────────┐
│  PEER WORKSTATIONS    │          egress             remote                       │   XCP-ng HEADLESS HOSTS       │
│  • Send-To → Inbox    │          control           desktop                       │  • spawn/destroy MDE-VMs      │
│    (files over sync)  │           ║                  ║                            │    (Provisioning panel ·      │
│  • clipboard sync     │           ▼                  ▼                            │     mackesd provision)        │
│  • voice HUD (SIP)    │  ┌──────────────────┐  ┌──────────────────┐               │  • build-farm scale + L0–L3   │
│  • KDE-Connect phone  │  │   VyOS ROUTER    │  │   MDE-VMs         │               │    tests (Build-Farm panel)   │
└──────────────────────┘  │  (site D edge)   │  │  (Server peers)   │               │  • driven via SSH/XAPI        │
                          │ • assign VPN      │  │ • build targets   │               │    (xcp_host worker)          │
                          │   egress routes   │  │ • run the test    │◀──────────────┤  hosts the VMs the dev builds │
                          │ • gateway/firewall│  │   pyramid         │   spawned on  └──────────────────────────────┘
                          │ • DDNS names      │  │ • promote ▶       │   the hosts
                          └──────────────────┘  └────────┬─────────┘
                                                          │  promotion pipeline (zones.md)
                                                          ▼
                                          Build (Xen/MDE-VM) ──▶ Eagle ──▶ DO lighthouses
```

## The developer's journeys (each spoke)

| # | Workflow (what the dev does) | Surface on the Workstation | Target node(s) | Underlying mechanism |
|---|---|---|---|---|
| 1 | **Join / trust the mesh** — enroll a box, get a CA-signed cert | first-run chooser / `mackesd enroll` | **Lighthouses** | CSR → CA auto-sign at the lighthouse; `static_host_map` to public LH IPs |
| 2 | **See the fleet** — who's up, who's leader, health | `home` · `peers` · `peers_map` · `lighthouses` panels | **Lighthouses (etcd)** | etcd client reads `/mesh/peers/`, leader, health |
| 3 | **Provision compute** — spawn/list/destroy headless VMs | `provisioning` · `datacenter` panels | **XCP-ng hosts** → **MDE-VMs** | `mackesd provision` / `action/provision/*`; `xe vm-clone` over SSH/XAPI (`xcp_host`) |
| 4 | **Build & test** — cut an RPM, run the pyramid | `build_farm` · `jobs` · `run_history` panels | **MDE-VMs** (build farm) | L0 build+unit → L1 install → L2 feature → L3 stability on the snapshot-reset pool |
| 5 | **Promote a release** — Build → Eagle → DO | `datacenter` Overview (promotion strip) | **MDE-VMs → Eagle → Lighthouses** | auto-promote on green to Eagle; DO step gated by the prod-arm switch |
| 6 | **Steer egress / network** — route nodes out a VPN, edit the gateway | `routing` · `connect`/VPN · `dns` panels | **VyOS router** | `vpn_gateway` + `ddns` workers; policy-routing + kill-switch; DO DNS writes |
| 7 | **Remote into a node** — console / desktop | `remote_desktop` · `connect` panels | any **peer / MDE-VM / host** | overlay reach (SSH/XAPI/RDP/VNC) — never public inbound |
| 8 | **Share files** — Send-To a peer; receive in Inbox | `mde-files` (Send-To dialog, pre-flight) | **Peer Workstations** | copy into `inbox/<peer>/<sender>/`; **Syncthing replication is the wire** |
| 9 | **Collaborate live** — clipboard, voice, phone | `mde-voice-hud` · `mde-kdc-host` · applet | **Peer Workstations / phone** | clipboard_sync · SIP HUD (relay-routed) · KDE-Connect over the overlay |
| 10 | **Play music** — browse + stream the shared library | `mde-music` (auto-configured) | **Media Lighthouses** | `music.mesh:4533` (auto-creds at enroll); active-active Navidrome |
| 11 | **Save anything shared** — docs, fleet revisions, tags | `mde-files` · `fleet_revisions` · `config_apply` | **Mesh Sync fabric** | write to `/mnt/mesh-storage`; Syncthing full-mesh; per-node `fleet_reconcile` |

## How the three diagrams stack

- **Services** ([1](services-topology-buildout.md)) — *what* runs on each node (Navidrome on
  media lighthouses, etcd quorum, MDE-VMs on hypervisors, VPN-GW on VyOS).
- **Reachability** ([2](services-reachability-topology.md)) — *how* the developer's NAT'd
  Workstation reaches those nodes (lighthouse-anchored hole-punch → relay → `:443`).
- **Workflow** (this one) — *why* the developer touches each node, and through which
  Workbench surface. The Workstation is the single pane of glass; the lighthouses make the
  fleet reachable; the hypervisors/VMs are where work runs; VyOS governs egress; the Mesh
  Sync fabric makes every save and every fleet decision propagate without a central server.

**The through-line:** one operator, at one Cosmic Workstation, drives an entire multi-site
fleet — provision, build, promote, route, collaborate, and play — with **no fixed center**.
Any Workstation can be that hub; losing one loses only that desktop, not the mesh.
