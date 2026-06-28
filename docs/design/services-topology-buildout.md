# Services Topology — full build-out (3 Media Lighthouses · 4 Workstations · 4 XCP-ng hosts · 1 VyOS)

**Status:** reference build-out, derived from the locked designs (no new locks).
**Sources:** [`architecture.md`](../architecture.md) · [`media-lighthouse.md`](media-lighthouse.md) ·
[`xcp-ng-integration.md`](xcp-ng-integration.md) · [`substrate-v2.md`](substrate-v2.md) ·
[`vpn-gateway.md`](vpn-gateway.md) · [`ddns-egress.md`](ddns-egress.md) ·
[`datacenter-control.md`](datacenter-control.md) · [`zones.md`](zones.md) ·
`crates/mesh/mackesd/src/worker_role.rs` (the 27-worker tier census) ·
`crates/platform/mde-role` (`Lighthouse ⊂ Server ⊂ Workstation`).

This is a **services-oriented** view: it maps *which service runs where* across a concrete
fleet, not the code layout. It does **not** change any §1–§3 substrate lock or the §8
trust envelope — it instantiates them.

## The fleet (this build-out)

| Class | Count | Role pin | mackesd member? | Headline service |
|---|---|---|---|---|
| **Lighthouse (All Media)** | **3** | `Lighthouse_Media` (extends Lighthouse, rank 0) | yes — full | Nebula relay+CA · **etcd quorum** · **Navidrome** |
| **Workstation** | **4** | `workstation` (rank 2) | yes — full | Cosmic desktop + the GUI suite · etcd **client** |
| **XCP-ng host (headless)** | **4** | *hypervisor* (no role pin; XCP-6) | **no** — static-Nebula only | hosts **MDE-VM** Server guests; XAPI compute provider |
| **VyOS machine** | **1** | *appliance* (no role pin) | **no** — static-Nebula only | WAN edge · **VPN egress gateway** · DDNS |

**§8 envelope (load-bearing).** The flat-trust ≤8-peer envelope counts **full mackesd
peers**: here that is **3 media lighthouses + 4 workstations = 7** — inside the envelope,
one slot to spare. The **4 XCP-ng dom0s** and the **VyOS box** are *appliances* that join
the **overlay only** (static `nebula`, no mackesd — XCP-6 glibc wall), so they do **not**
consume flat-trust peer slots. **MDE-VM** guests spawned on the hypervisors are Server-role
mackesd peers and **do** count against §8 — provision them within the remaining envelope
(or across additional meshes, per `do-lighthouses.md` §8).

## Service-oriented topology

```
                                   ┌──────────────── EXTERNAL / OFF-MESH ────────────────┐
   DigitalOcean Spaces  (1× 100 GB S3 bucket, music source of truth)                     │
   DigitalOcean DNS  (matthewmackes.com — DDNS A/AAAA writer target)                      │
   Commercial VPN exits  (Mullvad · Proton · IVPN · Nord · Surfshark)                     │
                                   └───────▲───────────────────▲──────────────────▲──────┘
                                  rclone mount (read)     DO API (DNS write)   WG/OVPN tunnels
                                           │                    │                  │
══════════════════════════════════ NEBULA OVERLAY (Ed25519 · AES-256-GCM/ChaCha20) ══════════════════════════════
   wire = 4242/udp · relay+discovery anchored on the lighthouses · 443/tcp covert TCP fallback
                                           │                    │                  │
        ┌──────────────────────────────── COORDINATION + MEDIA PLANE (anchors) ───────────────────────────────┐
        │                                                                                                      │
        │   LH-MEDIA-1                    LH-MEDIA-2                    LH-MEDIA-3        (Lighthouse_Media ×3)  │
        │  ┌───────────────┐            ┌───────────────┐            ┌───────────────┐                          │
        │  │ nebula lighthouse (relay + discovery + mesh CA signing root)              │  control plane         │
        │  │ etcd MEMBER  :2379/:2380 ──── quorum (3-node, leader election) ───────────┤  (rank-0 worker set:   │
        │  │ syncthing    :22000  ──────── full-mesh file share /mnt/mesh-storage ─────┤   nebula_supervisor,   │
        │  │ mackesd: heartbeat · health_reconciler · mesh_router · mesh_dns · leader  │   mesh_dns, leader,    │
        │  │ Navidrome (podman) :4533 ◀── rclone /music ◀── DO Spaces  ────────────────┤   fleet_reconcile, …)  │
        │  └───────────────┘            └───────────────┘            └───────────────┘                          │
        │        ▲   music.mesh:4533  → A-records = {LH-MEDIA-1, -2, -3 overlay IPs}  (round-robin + failover)  │
        └────────┼─────────────────────────────────────────────────────────────────────────────────────────────┘
                 │  (active-active: kill any one LH, music.mesh still resolves + streams; etcd keeps quorum 2/3)
                 │
        ┌────────┴───────────────────────────── DESKTOP / CLIENT PLANE ──────────────────────────────────────┐
        │   WS-1            WS-2            WS-3            WS-4                       (Workstation ×4, rank 2)   │
        │  ┌──────────┐   ┌──────────┐   ┌──────────┐   ┌──────────┐                                            │
        │  │ Cosmic desktop shell (mde-cosmic-applet · mde-mesh-wallpaper · mde-role-chooser)                  │
        │  │ mde-workbench (operator surface) · mde-files · mde-music + mde-musicd · mde-voice-hud · mde-kdc   │
        │  │ etcd CLIENT → anchors :2379   ·   syncthing :22000 full-mesh   ·   mde-bus (file pub/sub + RPC)   │
        │  │ mde-musicd ◀── airsonic-creds.json (birthright @ enroll) → server_url = http://music.mesh:4533    │
        │  └──────────┘   └──────────┘   └──────────┘   └──────────┘                                            │
        └────────────────────────────────────────────────────────────────────────────────────────────────────┘
                 │                                                                  │
        ┌────────┴───────────────── COMPUTE PLANE (hypervisors) ──────────┐   ┌─────┴──────── EDGE / EGRESS PLANE ────────┐
        │  XCP-1   XCP-2   XCP-3   XCP-4   (XCP-ng dom0 ×4, headless)      │   │  VyOS machine (network appliance)          │
        │  ┌─────────────────────────────────────────────────────────┐   │   │  ┌──────────────────────────────────────┐  │
        │  │ static nebula ONLY (SlackHQ static build) — overlay member│   │   │  │ static nebula ONLY — overlay member   │  │
        │  │ NO mackesd / NO etcd / NO lizardfs on dom0 (glibc 2.17)   │   │   │  │ WAN edge router + firewall (L3/NAT)   │  │
        │  │ XAPI / `xe` driven over the overlay via SSH (XeSsh, A1)   │   │   │  │ VPN egress: WG-first / OVPN fallback  │  │
        │  │   ┌────────────┐  ┌────────────┐                          │   │   │  │   → Mullvad/Proton/IVPN/Nord/Surfshark│  │
        │  │   │ MDE-VM-web1│  │ MDE-VM-…   │  Server-role guests       │   │   │  │ policy-routing + fwmark + kill-switch  │  │
        │  │   │ (mackesd,  │  │ (mackesd,  │  auto-enrolled at spawn    │   │   │  │ DDNS → services.matthewmackes.com     │  │
        │  │   │  etcd clnt)│  │  etcd clnt)│  hostname MUST start MDE-VM│   │   │  └──────────────────────────────────────┘  │
        │  │   └────────────┘  └────────────┘                          │   │   │  driven mesh-side: vpn_gateway + ddns       │
        │  └─────────────────────────────────────────────────────────┘   │   │  workers on a Server/leader (not on the box)│
        │  capacity (CPU/RAM/SR/running-VMs) advertised by the `xcp_host` │   │  + Gateway tab in the Datacenter plane      │
        │  worker running on a SERVER/leader node, pointed at dom0 (XCP-6)│   └─────────────────────────────────────────────┘
        └────────────────────────────────────────────────────────────────┘
```

## Per-class service stack

### Lighthouse (All Media) ×3 — `Lighthouse_Media`, rank 0
The stable anchors. Each runs the **full rank-0 worker set** (the 20 Lighthouse-tier
workers: `nebula_supervisor`, `heartbeat`, `health_reconciler`, `mesh_router`,
`stun_gather`, `mdns_relay`, `mesh_latency`, **`mesh_dns`**, `hardware_probe`,
`bus_supervisor`, `firewall_preset`, `sshd_overlay_bind`, `ssh_pubkey_gossip`,
`fleet_reconcile`, `presence_watch`, `lifecycle_exec`, `reconcile`, `netstate_apply`,
`validation_suite`, `metrics_exporter`) **plus**:

- **Nebula lighthouse** — relay + discovery anchor + the **mesh CA signing root**
  (enrollment auto-signs CSRs).
- **etcd member** (`:2379` client / `:2380` peer, overlay-bound) — a **3-node quorum**
  holds leader election + the peer directory + health (SUBSTRATE-V2 §5/§6).
- **Syncthing** (`:22000`) — full-mesh member of `/mnt/mesh-storage` (Mesh Sync).
- **Navidrome** (podman, `:4533`) — the media headline: an `rclone mount` of the shared
  **DO Spaces** bucket at `/music` (read-mostly), scanned into a container-local SQLite,
  `MemoryMax`/`CPUQuota` capped. **Active-active**: all three serve; `mesh_dns` publishes
  `music.mesh` → the three overlay IPs (A-record round-robin + failover).

Why *Media* and not stock lighthouses: the container is RAM-gated to this dedicated role
class so it never lands on a tiny 947 MB master (MEDIA lock #9).

### Workstation ×4 — `workstation`, rank 2 (Server ⊂ Workstation superset)
Full desktops. Everything a Server runs **plus** the rank-1 (`ansible-pull`, `app-sync`,
`job_exec`) and rank-2 workers (`voice_config`, `clipboard_sync`, `kdc_host`,
`remmina-sync`), **plus** the Cosmic GUI surfaces:

- **Desktop:** Cosmic shell + `mde-cosmic-applet` (panel) · `mde-mesh-wallpaper` (live map)
  · `mde-role-chooser` (first-run).
- **Apps:** `mde-workbench` (the operator control surface — fleet/devices/health/logs, the
  Datacenter & VPN/Routing panels) · `mde-files` · `mde-music` + `mde-musicd` ·
  `mde-voice-hud` · `mde-kdc-host` (phone/KDE-Connect).
- **Substrate:** **etcd client** only (targets the 3 anchor endpoints — workstations are
  not quorum members) · **Syncthing** full-mesh · `mde-bus`.
- **Auto-config:** at enroll, `mackesd` writes `airsonic-creds.json` →
  `server_url = http://music.mesh:4533` + the shared service account, so `mde-music` opens
  and browses the library with **zero manual connect**.

### XCP-ng host (headless) ×4 — hypervisor, *not* a mackesd member
A distributed **compute plane**. Per XCP-6, a dom0 (CentOS-7 / glibc 2.17) **cannot** run
the Fedora-built `mackesd`/`etcd`/`lizardfs` binaries, so each host:

- runs **only the SlackHQ static `nebula`** (the script refuses a dynamic binary) →
  overlay reachability + SSH-over-mesh + an XAPI control target. A boot unit re-asserts it
  so an XCP upgrade that clobbers it self-heals.
- is **driven mesh-side**: the `xcp_host` capacity worker runs on a **Server/leader** mesh
  node (NOT on dom0), reaching `xe`/XAPI over SSH via the `XeSsh` `Hypervisor` impl (A1),
  and advertises CPU/RAM/SR-free/running-VMs into the directory.
- **hosts MDE-VM guests**: `xe vm-clone MDE-VM-golden → MDE-VM-<name>` (UEFI, fresh
  identity seed), `mackesd role-pin server`, `network-enroll join` → the guest boots
  already a **Server** peer. Hostnames **must** start `MDE-VM` (operator rule).

These four are the **Dev/Xen** compute substrate (`zones.md`); the MDE-VMs are the real
mackesd members they produce.

### VyOS machine ×1 — network appliance, *not* a mackesd member
The **WAN edge / egress gateway**. Like the dom0s, it joins the **overlay only** (static
`nebula`) and is **controlled from the mesh**, not by a local mackesd:

- **Edge routing / firewall** — L3 router + NAT at the network boundary; the Datacenter
  plane's **Gateway tab** surface manages it (status, leases, firewall/port-forward) over
  SSH + its API, mirroring the UniFi-gateway pattern.
- **VPN egress (VPN-GW)** — terminates commercial-VPN tunnels (**WireGuard-first**,
  OpenVPN fallback) to Mullvad / ProtonVPN / IVPN / NordVPN / Surfshark. The
  `vpn_gateway` worker (on a Server/leader) brings tunnels up, applies **selective
  policy-routing** (`fwmark` + `ip rule` + nftables masquerade) with a **leak-proof
  kill-switch**, and steers assigned nodes' internet egress over the overlay to this box.
- **DDNS (DDNS-EGRESS)** — the `ddns` worker publishes each verified exit IP (and the WAN
  IP) as `<node>-<provider>.services.matthewmackes.com` via the **DigitalOcean DNS API**
  (short TTL, age-encrypted token).

> Note: VyOS is the chosen **edge appliance** for this build-out; it is not yet a named
> lock in the design docs. It slots into the **appliance / static-Nebula** membership
> model already proven for the XCP dom0s (overlay reachability + SSH-over-mesh + an API
> control target), so no new trust primitive is introduced.

## Ports / endpoints (canonical)

| Service | Port | Transport | Where |
|---|---|---|---|
| Nebula overlay | **4242** | udp | every node (LH/WS/XCP/VyOS) |
| Enroll `/enroll` | **4243** | tcp | lighthouses |
| Covert TCP tunnel | **443** | tcp | fallback when 4242/udp fails |
| etcd client | **2379** | tcp (overlay) | LH members; WS/MDE-VM clients |
| etcd peer | **2380** | tcp (overlay) | LH members (quorum) |
| Syncthing | **22000** | tcp (overlay) | every full mesh member |
| Navidrome / `music.mesh` | **4533** | tcp (overlay) | the 3 media lighthouses |

## Failure / redundancy properties

- **Lose one media lighthouse** → etcd holds quorum (**2/3**), `music.mesh` resolves +
  streams from the surviving two, CA/relay degrade gracefully (recoverable, not a
  decapitation — `mesh-recovery`).
- **Lose two lighthouses** → etcd quorum lost (coordination read-only/stalled); files
  (Syncthing) and direct overlay traffic still flow; restore quorum to fully recover.
- **Lose an XCP host** → its MDE-VMs go down; capacity drops in the directory; survivors
  unaffected. (Datacenter-control's auto-replace re-provisions.)
- **Lose the VyOS box** → VPN egress + DDNS for routed nodes drops; the kill-switch blocks
  marked traffic (no WAN leak); the mesh overlay itself is unaffected (Nebula ≠ VPN-GW).
- **Workstations** are stateless clients — losing one loses only that desktop.
