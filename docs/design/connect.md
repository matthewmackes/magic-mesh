# CONNECT — unified connectivity & exposure model (design)

**Status:** locked via a 12-Q operator survey, 2026-06-19 (Round 1 prior session,
Rounds 2–3 this session).
**Trigger:** Operator — "The mesh will present many services — some at host level,
some at VM, some at container. The platform will also have VPN Egress and Ingress
for which services should present services to the public over. Only the mesh
foundational layer and SSH access should travel the public internet. Create an
interface that combines these ideas into a clear method of connecting source,
destination, enforcing these patterns, etc. Should we employ a Network Management
tool?"
**Scope/version:** a new feature epic; lands **11.0+**. It is the **ingress /
exposure / unified-control** half of the connectivity stack — sibling to
**VPN-GW** (egress, `docs/design/vpn-gateway.md`), **DDNS-EGRESS** (public names,
`docs/design/ddns-egress.md`), and **ROUTE-TRACE** (path visualization,
`docs/design/route-trace.md` — its *ingress* direction builds on the shape locked
here). CONNECT answers "how do I expose a service to the public, safely, from one
place" and is the **mesh-native Network Management tool** the operator asked about.

## Locked decisions

| # | Question | Lock |
|---|----------|------|
| 1 | Tier model | **3 tiers — Public (Nebula + SSH only) / Mesh / Ingress-exposed.** The internet sees only the foundational layer; everything else is overlay or reaches the public solely through the ingress. |
| 2 | Public ingress | **Lighthouses act as the public reverse-proxy ingress** (they already hold the public IPs). |
| 3 | Posture | **Mesh-allow / public-deny** — default-deny on the public boundary; allow within the mesh. |
| 4 | Exposure surfaces | **Mesh (overlay) + public-via-ingress** — nothing else faces the internet. |
| 5 | Network-management tool | **Build a mesh-native Connectivity Manager** — one Workbench surface + a `mackesd` worker orchestrating the tools *already in place* (firewalld, nmstate, Nebula fw, VPN-GW, DDNS, the new Caddy ingress). **No external NMS/SDN** (mesh-tooling-first, D-W1). |
| 6 | Ingress proxy tech | **Caddy** — automatic HTTPS (Let's Encrypt) for the DDNS names; config rendered from the exposure policy. |
| 7 | Enforcement | **Managed firewalld profile, drift-corrected** by a worker: the public zone denies all inbound except **Nebula/4242 + SSH/22 + enroll/4243** (+ the ingress proxy ports on **lighthouses only**). |
| 8 | Discovery | **Auto-discover candidates** (compute_registry virsh/podman + listening ports + PD-2 descriptors) **+ opt-in to expose** — discovery never exposes anything by itself. |
| 9 | Operator flow | **Both — an exposure matrix (overview) + a guided Expose wizard** (service → tier → ingress lighthouse + DDNS hostname → auto-render proxy + firewall + DNS in one action). |
| 10 | Granularity | **Per-service policy, with reusable group templates** (apply one policy to many services/nodes). |
| 11 | Public protocols | **HTTP/HTTPS (proxied, auto-TLS) + arbitrary TCP/UDP** (stream/port-forward) — full reach, deliberately chosen. |
| 12 | Ingress auth | **None at the ingress — each service handles its own auth.** The proxy forwards; it does not gate. *(See the risk note — newly-exposed services are as open as the service itself.)* |

## What CONNECT does NOT change

- **Intra-mesh trust stays flat / open-mesh (§8).** Nebula's overlay firewall
  remains any-to-any; a valid mesh cert still reaches every peer + service. CONNECT
  governs **only the public boundary** (what crosses internet ⇄ mesh), not
  mesh-internal ACLs. The §8 flat-trust lock is intact.
- **Enrollment / Nebula transport** are unchanged — they ARE the permitted public
  layer (tier 1).

## Architecture

### Connectivity Manager (control plane)
- A new **`connectivity` responder + worker** in `mackesd`
  (`crates/mesh/mackesd/src/ipc/connect.rs` + `workers/`), RPC namespace
  **`action/connect/*`**, following the existing `action/<domain>/<verb>` →
  `reply/<ulid>` convention (per-domain responder loop, like `ipc/nebula.rs`).
- A **Workbench surface** that **folds in** the existing network panels
  (`firewall.rs`, `routing.rs`, `service_publishing.rs`, `connect.rs`) rather than
  adding a parallel one — §6 glue, not reimplementation. It becomes the single
  "Connectivity" home alongside the VPN/Routing panels.

### Exposure policy (state — one-state doctrine §9 W88)
Per-service records, TOML on the shared substrate (rides Syncthing post-SUBSTRATE-V2):
```toml
[[connect.service]]
id      = "grafana"
source  = { node = "eagle", kind = "container", host_port = 3000, proto = "tcp" }
tier    = "public-via-ingress"     # mesh-only | public-via-ingress
ingress = { lighthouse = "Lighthouse-01", hostname = "grafana.services.matthewmackes.com" }
mode    = "http"                   # http | tcp | udp
template = "web-apps"              # optional group template
```
- `mesh-only` services need no ingress/firewall change (already overlay-reachable).
- `public-via-ingress` services drive **three** rendered outputs (below).
- **Group templates** (`[[connect.template]]`) carry a tier+mode+ingress pattern
  applied to many services at once (e.g. "all web-apps → public-via-ingress, http,
  Lighthouse-01").
- RPCs: `action/connect/{list-services, set-policy, expose, unexpose,
  list-templates, set-template}`. CLI parity (§9 W27).

### Enforcement (firewalld profile, drift-corrected)
- A worker reconciles a firewalld profile on **every** node from the policy +
  baseline — extends the existing `firewall_preset.rs` / `mesh_firewall.rs`
  pattern (firewall-cmd, idempotent, role-aware):
  - **All nodes, public zone:** deny inbound except **Nebula/4242 (UDP)**,
    **SSH/22**, **enroll/4243** (+ covert/443 where used). This *is* tier-1
    "Public = Nebula + SSH only".
  - **Lighthouses only:** additionally open the **Caddy ingress ports** (80/443
    for HTTP/HTTPS + any allowlisted raw TCP/UDP stream ports).
  - Overlay interface stays in the `trusted` zone (open-mesh, §8).
- Drift-corrected on a tick: the actual ruleset is brought back to the policy, and
  a deviation surfaces as an alert (reuses `firewall_monitor.rs` plumbing).

### Public ingress (Caddy on lighthouses)
- **Caddy** runs on the ingress lighthouse; its config is **rendered from the
  exposure policy**:
  - `http` services → a Caddy site `https://<ddns-hostname> →
    <service overlay IP>:<host_port>` with **automatic Let's Encrypt TLS** for the
    DDNS name (pairs with DDNS-EGRESS' `services.matthewmackes.com`).
  - `tcp`/`udp` services → a Caddy **`layer4`/stream** block (allowlisted port →
    overlay IP:port).
- The proxy reaches the service over the **Nebula overlay** (public → lighthouse →
  overlay → host/VM/container), so the service never binds a public interface
  itself. On `expose`, CONNECT also triggers the **DDNS record** creation
  (DDNS-EGRESS) for the chosen hostname.

### Discovery (candidates → opt-in)
- Reads **`compute_registry`** (virsh/podman inventory) + a **listening-port
  scan** + **PD-2 service descriptors** to build a candidate list of
  host/VM/container services. The operator **opts a candidate in** to make it
  exposable; nothing is exposed by discovery alone (default-deny intent).

### UI
- **Exposure matrix** — rows = discovered/declared services (with their source
  node + kind host/VM/container), columns = tier + ingress; each cell shows the
  current policy + status (mesh-only / public @ hostname / blocked). Folds in the
  firewall + routing + service-publishing views.
- **Expose wizard** — pick service → tier → (if public) ingress lighthouse +
  DDNS hostname + protocol mode → preview the rendered proxy + firewall + DNS →
  apply (one action wires all three). An **Unexpose** reverses all three.
- Carbon tokens only (§4); reuses the VPN-GW/Routing panel primitives so egress
  (VPN-GW) and ingress (CONNECT) share one Routing home.

## Acceptance (high level; per-task bullets in the worklist)
- A discovered host/VM/container service can be exposed via the wizard: one action
  renders the Caddy site/stream + the firewalld allow + the DDNS record, and the
  service becomes reachable at `https://<hostname>` (or the chosen TCP/UDP port)
  from the public internet, while still overlay-reachable.
- On a non-ingress node, the public boundary is **default-deny**: only Nebula/4242,
  SSH/22, and enroll/4243 are reachable from the internet — asserted by a test.
- `mesh-only` services are never publicly reachable; `unexpose` removes the proxy
  + firewall opening + DNS record.
- The exposure matrix shows every service's tier + public hostname + status;
  group templates apply a policy across many services at once. All real over
  `action/connect/*` (no stubs, §7), Carbon tokens (§4).
- Egress (VPN-GW) and ingress (CONNECT) share the Routing surface; ROUTE-TRACE
  renders the ingress path CONNECT defines.

## Risks / notes
- **No ingress auth (lock #12)** — a public-via-ingress service is exactly as open
  as the service's own auth. The wizard MUST warn at expose time when a service
  has no/weak auth, and the matrix MUST visibly flag "public, no ingress auth".
  (Forward-auth/SSO is a natural follow-on epic if the posture tightens.)
- **Arbitrary TCP/UDP (lock #11)** widens the public surface beyond HTTP — every
  stream port is a real opening; keep it allowlisted + visible, and lean on the
  default-deny baseline so only explicitly-exposed ports ever open.
- **Caddy/ACME on the thin 947 MB lighthouse** — Caddy is light, but ACME issuance
  + cert storage + the proxy add memory/IO; validate headroom (the netdata-thrash
  lesson) and prefer the larger anchor as the primary ingress.
- **Public-deny correctness** — the firewalld reconcile must not lock out SSH or
  Nebula (the netstate self-test discipline applies); apply additively + verify
  reachability before committing a tightened ruleset.
- **Single-ingress SPOF** — one ingress lighthouse is a choke point; multi-ingress
  (a service exposable via >1 lighthouse with DNS failover) is a follow-on (pairs
  with DDNS multi-record).
- **Open-mesh unchanged (§8)** — CONNECT must not be read as adding intra-mesh
  ACLs; it is strictly the public boundary.

## Out of scope (v1)
- Forward-auth / SSO / per-service identity at the ingress (lock #12 = none; a
  later epic if the posture tightens).
- Per-group Nebula overlay firewall rules / intra-mesh micro-segmentation (§8
  open-mesh stays).
- Multi-ingress load-balancing / DNS-failover for one service (single ingress per
  service in v1).
- WAF / rate-limiting / DDoS protection at the ingress (rely on Caddy defaults +
  the provider).
