# Network Reachability Topology — multi-site fleet behind NAT

**Status:** reference topology, derived from the locked transport design (no new locks).
**Companion to:** [`services-topology-buildout.md`](services-topology-buildout.md) (the
*services* view). This one answers **"who can reach whom, and over what path"** — NAT
traversal, hole-punching, relay fallback, and the covert `:443` path.
**Sources:** [`architecture.md`](../architecture.md) (mesh routing) ·
`install-helpers/onboard-xcp-host.sh` (the canonical Nebula `static_host_map` + `punchy`
config) · `crates/mesh/mackesd/src/topology/mod.rs` (the three transport flavors:
direct UDP · lighthouse relay · `NebulaHttps443`) · `https_fallback` · the `mesh_router`
+ `stun_gather` rank-0 workers.

## The fleet (this topology)

| Site | Members | Public reachability |
|---|---|---|
| **Public anchors** | **3 Lighthouses** (LH-1/2/3) | **public IP — inbound-reachable** (the only such nodes) |
| **Location A** | 1 Workstation · 1 XCP-ng headless host | **Firewalled + NAT'd** (outbound-only) |
| **Location B** | 1 Workstation · 1 XCP-ng headless host | **Firewalled + NAT'd** (outbound-only) |
| **Location C** | 1 Workstation · 1 XCP-ng headless host | **Firewalled + NAT'd** (outbound-only) |
| **Location D** | 1 Workstation · 3 XCP-ng headless hosts · **VyOS router** | **Firewalled + NAT'd at the VyOS edge** (outbound-only) |

Only the **3 lighthouses** accept inbound connections from the public internet. **Every
other node is firewalled and NAT'd** — it can dial *out* but nothing on the internet can
dial *in*. The mesh is built entirely from outbound connections that the lighthouses
stitch together.

## Reachability topology

```
                        ╔════════════════ PUBLIC INTERNET (inbound-reachable) ════════════════╗
                        ║                                                                      ║
                        ║   ┌──────────┐        ┌──────────┐        ┌──────────┐               ║
                        ║   │  LH-1    │        │  LH-2    │        │  LH-3    │  3 Lighthouses ║
                        ║   │ public IP│        │ public IP│        │ public IP│  (anchors)     ║
                        ║   │ :4242/udp│        │ :4242/udp│        │ :4242/udp│                ║
                        ║   │ :4243/tcp│        │ :4243/tcp│        │ :4243/tcp│  enroll/CA     ║
                        ║   │ :443/tcp │        │ :443/tcp │        │ :443/tcp │  covert relay  ║
                        ║   └────▲─────┘        └────▲─────┘        └────▲─────┘                ║
                        ╚═══════│════════════════════│════════════════════│═════════════════════╝
       reachability the lighthouses provide:        │                    │
         (1) hole-punch coordination (STUN/punchy)  │   outbound-only     │   all sites dial OUT to the
         (2) relay fallback when a punch fails       \  (NAT/firewall)    /    public LH IPs; the LHs
         (3) the :443 covert path when UDP is blocked \                  /     coordinate everything else
                        ┌───────────────┬──────────────┴───┬──────────────┴───┬───────────────────────┐
                        │               │                  │                  │                       │
              ┌─────────┴────────┐ ┌────┴───────────┐ ┌────┴───────────┐ ┌────┴───────────────────────┴───────┐
              │  LOCATION A      │ │  LOCATION B    │ │  LOCATION C    │ │  LOCATION D                        │
              │ [Firewalled+NAT] │ │[Firewalled+NAT]│ │[Firewalled+NAT]│ │ [Firewalled + NAT @ VyOS edge]     │
              │ ──────────────── │ │ ────────────── │ │ ────────────── │ │ ────────────────────────────────── │
              │                  │ │                │ │                │ │   ┌────────────────────────────┐   │
              │  ┌────────────┐  │ │ ┌────────────┐ │ │ ┌────────────┐ │ │   │  VyOS ROUTER (site edge)   │   │
              │  │Workstation │  │ │ │Workstation │ │ │ │Workstation │ │ │   │  NAT + firewall for site D │   │
              │  │ (full peer)│  │ │ │ (full peer)│ │ │ │ (full peer)│ │ │   │  static-nebula overlay mbr │   │
              │  └────────────┘  │ │ └────────────┘ │ │ └────────────┘ │ │   │  (opt: 4242/udp port-fwd)  │   │
              │  ┌────────────┐  │ │ ┌────────────┐ │ │ ┌────────────┐ │ │   └─────────────▲──────────────┘   │
              │  │ XCP-ng host│  │ │ │ XCP-ng host│ │ │ │ XCP-ng host│ │ │       all site-D egress ▲          │
              │  │static-neb. │  │ │ │static-neb. │ │ │ │static-neb. │ │ │   ┌────────┬────────┴┬─────────┐   │
              │  │+ MDE-VMs   │  │ │ │+ MDE-VMs   │ │ │ │+ MDE-VMs   │ │ │   │Workstn │XCP host1 │XCP host2│…  │
              │  └────────────┘  │ │ └────────────┘ │ │ └────────────┘ │ │   │(peer)  │static-neb│static-nb│   │
              └──────────────────┘ └────────────────┘ └────────────────┘ │   │        │+MDE-VMs  │+MDE-VMs │   │
                                                                          │   └────────┴──────────┴─────────┘   │
                                                                          │      (1 WS + 3 XCP hosts)           │
                                                                          └────────────────────────────────────┘

   Once the lighthouses introduce two NAT'd peers, traffic prefers a DIRECT path and only
   falls back as needed:

      A.Workstation ──── direct UDP hole-punched tunnel (4242/udp) ────────────── C.Workstation
      B.XCP-host    ──── relay via LH-2 (punch failed: symmetric NAT) ─────────── D.Workstation
      A.Workstation ──── :443 covert TCP tunnel (UDP egress blocked) ──────────── LH-1 ↔ D.host
```

## How reachability is achieved (NAT-traversal mechanics)

Every site node ships the same Nebula posture (`onboard-xcp-host.sh`): `am_lighthouse:
false`, a `static_host_map` pinning the **public lighthouse IPs**, and
`punchy: {punch: true, respond: true}`. From that, three path tiers, picked per-peer by
the `mesh_router` scorer (10 s tick):

1. **Direct UDP (preferred).** Both NAT'd peers send keepalives outbound to the
   lighthouses; the lighthouse tells each peer the other's observed `ip:port`
   (STUN-style, fed by `stun_gather`); `punchy` fires simultaneous UDP packets to open
   the NAT mappings → a **direct peer-to-peer encrypted tunnel** on `4242/udp`. No traffic
   transits the lighthouse after the punch. *This is the common case for Locations A–D.*

2. **Lighthouse relay (fallback).** When the punch can't open a mapping — **symmetric
   NAT**, strict CGNAT, or a firewall that rewrites ports — the peers can't meet directly.
   Nebula then **relays** the tunnel through a lighthouse (the lighthouse is relay-eligible
   and forwards the encrypted payload; it never sees plaintext). Slower path, always works.

3. **`:443` covert TCP (last resort).** When **`4242/udp` egress is blocked entirely**
   (hotel/corporate firewalls that only allow `80/443`), `https_fallback` trips on the
   UDP-failure threshold and the path **switches to a TLS tunnel over TCP/443**
   (`NebulaHttps443`, the `:443` covert-relay listener on the lighthouses). `mesh_router`
   records the `path_switch → nebula_https443` as a hash-chained audit event.

The lighthouses are a **rendezvous + relay + CA**, not a controller — losing one degrades
reachability (fewer punch coordinators / relays) but the surviving two keep introducing
peers and relaying; direct tunnels already punched stay up (recoverable, not a
decapitation).

## Per-location reachability notes

- **Locations A / B / C** (1 Workstation + 1 XCP-ng host each, behind the site's own
  NAT/firewall). Both nodes are **outbound-only**:
  - The **Workstation** is a full mackesd peer — punches direct tunnels to every other
    peer, relays/`:443`-fallbacks as needed.
  - The **XCP-ng host** joins the overlay as a **static-Nebula member** (no mackesd —
    XCP-6 glibc wall): it is reachable *on the overlay* for **SSH-over-mesh / XAPI
    control**, driven by the `xcp_host` worker on a mesh peer. The **MDE-VM** guests it
    runs are full Server peers and punch their own tunnels.

- **Location D** (1 Workstation + 3 XCP-ng hosts behind a **VyOS router**). The VyOS box
  is the **site's NAT + firewall edge** — all four D-nodes egress through it, and like the
  XCP dom0s it joins the **overlay as a static-Nebula member** (no mackesd):
  - By default the four D-nodes traverse NAT exactly like A/B/C — outbound to the
    lighthouses, then punch/relay/`:443`.
  - Because Location D has a **controllable router**, VyOS can optionally **port-forward
    `4242/udp`** to a designated node (or hairpin the site), making D's nodes **directly
    punchable with a stable mapping** — the most reliable NAT-traversal case (fewer relay
    fallbacks). VyOS firewall rules still deny all *other* inbound.
  - VyOS can additionally serve as the site's egress/VPN gateway (see
    [`vpn-gateway.md`](vpn-gateway.md)) — out of scope for this reachability view.

## Reachability matrix

| From → To | Lighthouse (public) | Workstation (NAT'd) | XCP host (NAT'd, static-neb) | MDE-VM (NAT'd) |
|---|---|---|---|---|
| **Any site node → Lighthouse** | **direct** (public IP, outbound) | n/a | n/a | n/a |
| **Lighthouse → site node** | n/a | via the peer's outbound tunnel (never unsolicited inbound) | overlay only (SSH/XAPI) | via the VM's tunnel |
| **Site node → Site node** | n/a | **punch → direct**, else **relay**, else **:443** | overlay-reachable (SSH/XAPI), same path tiers | same path tiers |
| **Public internet → any site node** | n/a | **blocked** (firewall+NAT) | **blocked** | **blocked** |

**Bottom line:** the public internet can reach **only the 3 lighthouses**. Every
Workstation, XCP-ng host, MDE-VM, and the VyOS router is firewalled + NAT'd and reachable
**only over the Nebula overlay**, established by outbound connections the lighthouses
coordinate — direct-punched when possible, relayed or `:443`-tunneled when the NAT/firewall
forces it.
