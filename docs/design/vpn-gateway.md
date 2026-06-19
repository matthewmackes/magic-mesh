# VPN-GW — commercial-VPN exit gateways on every node (design)

**Status:** locked via a 10-Q operator survey, 2026-06-19.
**Trigger:** Operator — "VPN provider exit gateway on every node. Multiple
providers configurable (top-5 integration methods). Controls in the VPN + Routing
panels. Gateways to ANY or multiple nodes. Every node can have multiple VPN
tunnels to different providers; a provider can be used more than once (if it
allows multi-instance). World-class, professional interface."
**Scope/version:** a new feature epic; lands **11.0+** (after the 10.0.18 final
10.x cut). Additive to the Nebula overlay — Nebula stays the mesh transport; VPN
tunnels are an **internet-egress** layer on top.

## Locked decisions

| # | Question | Lock |
|---|----------|------|
| 1 | Integration methods | **All four** — WireGuard configs (primary), OpenVPN `.ovpn`, provider CLIs, provider APIs/config-generators. |
| 2 | First-class providers | **Mullvad, ProtonVPN, IVPN, NordVPN, Surfshark** + a **generic WireGuard** ("paste any WG config") + **generic OpenVPN** ("import any .ovpn") path so ANY provider works. |
| 3 | Secret storage | **Encrypted mesh secret (age), leader-managed** — tunnel configs/keys stored encrypted in the mesh secret store, leader-distributed to assigned gateway nodes (so any node can be given a tunnel without re-pasting). *(Rides the 11.0 Syncthing+etcd substrate; until then QNM-Shared `secrets/vpn/`.)* |
| 4 | Default tunnel tech | **WireGuard-first, OpenVPN fallback** (per provider/route; obfuscation/TCP → OpenVPN). |
| 5 | Egress model | **Selective policy-routing + NAT** — the gateway NATs out the chosen tunnel; egress steered by `fwmark`/`ip rule` so specific nodes/groups (not the whole box) go through the VPN; the gateway's own mesh traffic stays direct. |
| 6 | Route scopes | **Per-node → gateway+tunnel · ANY/all-mesh default · node-group → one gateway.** (Per-destination/CIDR split-tunnel out of scope for v1.) |
| 7 | Multi-tunnel selection | **Named tunnel + ordered failover chain** per route. |
| 8 | Kill-switch | **Block egress on drop (no leak), per-route default** (failover tried first); overridable per route. |
| 9 | Health/failover | **Active health-check + exit-IP/leak verification + auto-failover** — verify the tunnel is up AND its public exit IP is the provider's (not the WAN) + a DNS-leak check; on failure fail over the chain + alert. |
| 10 | Interface | **All three, integrated** — VPN panel: per-tunnel **cards** (provider, server/region, protocol, live exit-IP + status + throughput, kill-switch) + an **add-tunnel wizard**; Routing panel: an **egress routing matrix** (nodes→gateway→tunnel + failover chain + kill-switch per route) **and** a **topology/route map** (mesh→gateways→provider exits) + an **assign-route wizard**. |

## Architecture

### Per-node VPN engine (`mackesd` `vpn_gateway` worker)
- Manages **tunnels** — each a named definition `{id, provider, method (wg|ovpn|cli|api), server/region, protocol, creds-ref}`. A node can run **N tunnels** (different providers, or the same provider multiple times where multi-instance is allowed — distinct interface names `mvpn-<id>`).
- Brings tunnels up/down: WireGuard via `wg-quick`/`wg` on a dedicated netns-free interface; OpenVPN via `openvpn`; provider CLIs (`mullvad`, `protonvpn-cli`, `nordvpn`) where chosen; provider APIs to mint WG configs/pick servers.
- **Egress** (selective policy-routing): each active tunnel gets a routing table + an `ip rule` keyed on an `fwmark`; nftables masquerades marked traffic out the tunnel interface; a **kill-switch** drop rule blocks marked traffic when the tunnel is down (no leak). The gateway's own + Nebula traffic is unmarked → stays direct.
- **Routing a node's egress via a gateway:** the assigned node's selected egress is sent over the **Nebula overlay** to the gateway's overlay IP (an overlay route), where the gateway marks + NATs it out the tunnel. Builds on the existing overlay routing/netstate (routing.rs). ANY/all-mesh = a mesh default; node-group = several nodes → one gateway.

### Credentials / distribution
- Tunnel configs/keys encrypted with **age**, stored in the mesh secret store, **leader-managed**: the leader pushes a tunnel's secret only to the gateway node(s) assigned to run it. Never in `ps`/logs.

### Health + failover
- A checker per tunnel: handshake/liveness + **curl the exit IP through the tunnel** and compare to the provider's expected ASN/IP + a **DNS-leak probe**. On failure → fail over to the next tunnel in the route's chain; if none, the kill-switch blocks (no leak) + raise a `vpn/tunnel-down` alert. Exit-IP + health surface live in the UI.

### Bus surface (RPCs)
- `action/vpn/{list-tunnels, add-tunnel, update-tunnel, remove-tunnel, tunnel-up, tunnel-down, tunnel-status}` and `action/vpn/{list-routes, set-route, clear-route, route-status}`. Backend = the `vpn_gateway` worker + the secret store. The Workbench panels are thin renderers over these RPCs.

### UI (world-class)
- **VPN panel** (`panels/vpn.rs`): a grid of tunnel **cards** — provider logo, server/region, protocol badge (WG/OVPN), **live exit-IP + a green/red status + throughput**, a kill-switch toggle, up/down + edit. An **add-tunnel wizard**: pick provider → method (WG config / .ovpn / CLI / API) → paste config or auth → server/region → multi-instance name → verify (exit-IP check) → save (encrypts to the mesh secret).
- **Routing panel** (`panels/routing.rs`, extended): an **egress routing matrix** (rows = nodes/groups, columns = gateway+tunnel, cells = the assignment with its failover chain + kill-switch flag), a **topology/route map** (mesh nodes → gateways → provider exit points; click an edge to assign/inspect), and an **assign-route wizard** (pick node(s)/ANY → gateway → primary tunnel → failover chain → kill-switch). A live "who exits where" summary (each node's current exit IP + provider).
- Carbon tokens only (§4); no raw hex.

## Acceptance (high level; per-task bullets in the worklist)
- A node can define multiple VPN tunnels (any of the 5 providers + generic WG/OVPN, incl. the same provider twice) via the add-tunnel wizard; configs are stored encrypted + leader-distributed.
- The Routing panel assigns a node's / a group's / ANY's internet egress through a chosen gateway+tunnel with a failover chain + kill-switch; the assignment takes effect (the node's real exit IP becomes the provider's).
- A tunnel drop fails over down the chain; if none, egress is blocked (no WAN leak) + an alert fires; the exit-IP/leak check catches a silently-leaking tunnel.
- The VPN panel shows live per-tunnel exit-IP/status/throughput; the Routing map/matrix shows who exits where. All real (no stubs, §7).

## Risks / notes
- **Provider CLIs/APIs vary** — ship WireGuard/OVPN config paths as the always-works baseline; CLI/API integrations are per-provider adapters added incrementally behind the generic path.
- **Policy-routing + Nebula interaction** — the fwmark/table rules must not capture Nebula's own traffic (mesh must never tunnel through the VPN) or the overlay breaks; carve out the overlay subnet explicitly.
- **Kill-switch correctness** — the drop rule must be leak-proof on tunnel flap (test with the tunnel killed mid-transfer).
- **Secrets** — age-encrypted, leader-pushed, never logged; rotate on tunnel delete.
- **Multi-instance** — only where the provider's ToS/keys allow concurrent sessions; the UI flags providers that forbid it.

## Out of scope (v1)
- Per-destination/domain split-tunnel routing (route scopes are per-node / group / ANY).
- Load-balancing a single route across tunnels (failover chain only).
- Inbound port-forwarding through the provider.
