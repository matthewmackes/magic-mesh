# ROUTE-TRACE — connectivity path map in the Routing interface (design)

**Status:** locked via a 4-Q operator survey, 2026-06-19.
**Trigger:** Operator — "In the Routing interface create a traceroute layout
which shows the path traffic is taking to reach the published services. This
should connect the concepts — Hosting and VMs, The Mesh Network, VPN Egress and
Ingress, Firewalls and Control, and Dynamic DNS."
**Scope/version:** a new feature epic; lands **11.0+**. It is the **capstone
visualization** that unifies the connectivity stack — it reads from VPN-GW
(`docs/design/vpn-gateway.md`), DDNS-EGRESS (`docs/design/ddns-egress.md`), the
CONNECT model (the 3-tier Public/Mesh/Ingress posture being surveyed), the
firewall/control layer, and the existing mesh routing/netstate (`routing.rs`,
service-publishing). It **renders existing state** — glue, not a new control
plane (§6).

## The five concepts it must connect
1. **Hosting & VMs** — the source/destination endpoints (host-level service, a
   VM, a container) and which physical/virtual node hosts them.
2. **The Mesh Network** — the Nebula overlay hop(s): direct vs lighthouse-relay,
   overlay IPs, per-link RTT.
3. **VPN Egress & Ingress** — the gateway + tunnel a flow exits through (egress),
   and the lighthouse reverse-proxy ingress a published service enters through.
4. **Firewalls & Control** — every control point the flow crosses (Nebula
   firewall, nftables/firewalld, the VPN kill-switch) with its allow/block
   verdict + matching rule.
5. **Dynamic DNS** — the name the destination/exit resolves to (the
   `services.matthewmackes.com` DDNS records) at the relevant hop.

## Locked decisions

| # | Question | Lock |
|---|----------|------|
| 1 | Path derivation | **Hybrid — modeled + live overlay.** Build the logical path from platform state (endpoints, overlay route, gateway/tunnel, firewall rules, DNS) AND overlay **live measurements** where measurable: per-overlay-link RTT/loss (from the Nebula debug-SSH path classifier / netstate), and a real public hop list via `traceroute`/`mtr` beyond the VPN exit. Always renders fully-labeled; live data enriches it. |
| 2 | Direction | **Both, selectable.** Egress (a mesh node → external destination, through gateway+tunnel) and ingress (external client → a published service, through the lighthouse ingress). A perspective toggle. |
| 3 | Layout | **Topology graph (nodes + edges)** with the **active path highlighted**; click an edge/node to inspect. Nodes = endpoints + waypoints (host/VM, overlay peers, gateway, VPN exit, ingress, internet cloud, the service); edges = links carrying the per-segment data. |
| 4 | Firewall/control display | **Both — inline badge + detail panel.** Each edge crossing a control point carries a compact allow/block badge; selecting it opens a detail panel with the full rule chain (which firewall, which rule, the verdict). A blocked path highlights the failing edge in red (Carbon red token). |

## Architecture

### Backend — a path builder + measurer
- A **`route_trace` responder** in `mackesd` (beside `vpn_gateway`/`ddns`):
  `action/route/trace { from, to, direction }` → returns a typed **PathGraph**.
- **Model step (always):** assemble the segment list from existing state —
  - endpoint resolution (host/VM/container + hosting node) from the
    compute-inventory + service-publishing inventory;
  - overlay route (direct vs relay, peer overlay IPs) from `routing.rs` /
    netstate;
  - egress gateway + tunnel + verified exit IP from VPN-GW; ingress
    lighthouse + reverse-proxy mapping from the CONNECT/ingress model;
  - control points + verdicts by **evaluating the rule sets** the flow crosses
    (Nebula firewall config, nftables/firewalld, the VPN kill-switch state) —
    a real allow/block decision per segment, with the matching rule cited;
  - DNS name(s) from the DDNS records / resolver.
- **Measure step (best-effort overlay):** per-overlay-link RTT/loss from the
  existing path classifier; public hops/RTT via `traceroute`/`mtr` run **from the
  relevant node** (the source for egress; the ingress node for ingress) over the
  bus — typed verb, not raw shell (§9 W21). Missing measurements degrade to the
  modeled segment (never a guess, never a panic — §2 graceful degrade).
- Reuses the bus + typed verbs; no new transport.

### PathGraph (the typed result)
```
PathGraph {
  direction: Egress | Ingress,
  nodes: [ { id, kind: Host|VM|Container|OverlayPeer|Gateway|VpnExit|Ingress|Internet|Service,
             label, node/overlay/public ip, dns_name?, hosting_node? } ],
  edges: [ { from, to, layer: Host|Mesh|Vpn|Public,
             rtt_ms?, loss?, transport: DirectOverlay|RelayOverlay|VpnTunnel|Public|Loopback,
             control?: { point: NebulaFw|Nftables|Firewalld|KillSwitch,
                         verdict: Allow|Block, rule: "<the matching rule>" } } ],
  blocked_at?: edge-id,   // first denying control point, if any
}
```

### UI — the Routing-panel trace view (`panels/routing.rs`, extended)
- A **trace toolbar**: pick **source** (a mesh node) + **destination** (a
  published service from the inventory, or any host/IP) + a **direction toggle**
  (Egress / Ingress) + **Trace**.
- A **topology graph** render: endpoint nodes + waypoints laid out source→dest,
  the **active path highlighted**; each node glyph keyed by kind (host/VM, overlay
  peer, gateway, VPN exit, ingress, internet, service). Edges show **layer color**
  (Host/Mesh/Vpn/Public — Carbon tokens), an **RTT/loss** label, and an
  **inline allow/block badge** on control edges. A blocked path shows the failing
  edge red + a banner "blocked at <point> by <rule>".
- **Detail panel** (on node/edge select): the segment's full data — endpoints,
  transport (direct/relay/tunnel/public), RTT/loss, the **full firewall rule
  chain** + verdict, and the **DNS name** resolved at that hop. The five concepts
  are each surfaced: hosting node (Host), overlay direct/relay (Mesh), gateway +
  tunnel + exit IP (VPN egress) / ingress mapping (VPN ingress), the rule chain
  (Firewalls & Control), and the DDNS record (Dynamic DNS).
- Carbon tokens only (§4); the graph reuses the VPN-GW topology-map primitives.

## Acceptance (high level; per-task bullets in the worklist)
- Picking a source node + a published service + Egress/Ingress renders a path
  graph from real platform state: the hosting node, the overlay hop(s)
  (direct/relay), the gateway+tunnel+exit (egress) or ingress mapping (ingress),
  every control point with an allow/block verdict + the matching rule, and the
  DNS name(s) — all five concepts present, no placeholders (§7).
- Live overlay RTT/loss appears on overlay edges and real public hops appear
  beyond the VPN exit where measurable; an unmeasurable segment degrades to the
  modeled hop without error.
- A flow that a firewall/kill-switch would block renders the failing edge red
  with the denying rule named; an allowed flow renders end-to-end green.
- The direction toggle switches between the egress and ingress paths for the same
  endpoints; selecting any hop opens a detail panel with its full rule chain +
  DNS name. All data over `action/route/trace` (CLI parity, §9 W27).

## Risks / notes
- **Firewall verdict accuracy** — the modeled allow/block must reflect the *real*
  rule evaluation order (Nebula fw + nftables/firewalld + kill-switch), not a
  guess; where the platform owns the rules (Nebula fw, the VPN-GW nftables) this
  is exact; for host-local firewalld, evaluate the actual ruleset, and mark
  anything it cannot statically resolve "indeterminate" rather than asserting.
- **traceroute over the public internet** beyond a VPN exit may be rate-limited /
  ICMP-filtered — show what returns, mark gaps; never block the render on it.
- **Ingress path** depends on the CONNECT reverse-proxy ingress model (still being
  surveyed) — build the trace against its locked shape; until CONNECT lands, the
  ingress direction can render the modeled mesh+firewall+DNS segments and fill the
  proxy hop when CONNECT ships.
- **Privacy/secrets** — the trace shows exit IPs + DNS names + rules; it's an
  operator surface (mesh-cert-gated, §8) — no creds, no tunnel keys.
- **Cost** — live mtr per trace is on-demand only (operator clicks Trace), not a
  background sweep; cache recent results briefly.

## Out of scope (v1)
- Continuous/animated live-traffic flow (it's an on-demand trace, not a real-time
  packet monitor).
- Per-application/per-port path differentiation beyond the selected
  service/destination (one flow at a time).
- Editing rules from the trace view (it links to the firewall/routing editors;
  it doesn't own them).
