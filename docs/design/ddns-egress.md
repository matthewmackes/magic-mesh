# DDNS-EGRESS — dynamic DNS for VPN-egress (and WAN) IPs (design)

**Status:** locked via a 4-Q operator survey, 2026-06-19.
**Trigger:** Operator — "When a VPN provider assigns a new IP to a connection,
automatically assign a subdomain name from my private subdomain
matthewmackes.com (registered with DigitalOcean). Create a configuration setting
that allows for Dynamic DNS setup for egress IP addressing provided by dynamic
providers like VPN. Include the top 5 providers plus DigitalOcean's Services."
**Scope/version:** a new feature epic; lands **11.0+**, **on top of VPN-GW**
(`docs/design/vpn-gateway.md`). It is the DNS-publishing layer for the egress IPs
VPN-GW already discovers/verifies — additive, not a rewrite.

## Locked decisions

| # | Question | Lock |
|---|----------|------|
| 1 | What "top 5 providers + DigitalOcean" means | **IP source = the 5 VPN providers; DNS write target = DigitalOcean.** Pull the assigned exit IP from each of **Mullvad, ProtonVPN, IVPN, NordVPN, Surfshark** (their API/CLI + the VPN-GW exit-IP verifier), then **write the record via the DigitalOcean DNS API** (where `matthewmackes.com` is hosted). DO is the only writer in v1; the writer is an adapter so other DNS hosts can be added later. |
| 2 | Subdomain naming | **Records live under `services.matthewmackes.com`** (the operator's "DigitalOcean Services" zone). Each node/tunnel gets a **stable hostname** in that zone — `<node>-<provider>[-<n>].services.matthewmackes.com` — whose **A record is rewritten to the new IP on every reconnect**. Predictable + bookmarkable; the name is stable, only the address changes. |
| 3 | Purpose (record semantics) | **Both reachability + identity.** Primary: **inbound reachability** when the tunnel offers port-forwarding / a dedicated IP (reach a service behind the VPN by a friendly name). Also a live **"who exits where"** identity record. The record is `A → current exit IP` (+ `AAAA` when an IPv6 exit exists). |
| 4 | Which IPs to track | **VPN exit + node WAN.** When a tunnel is up, publish its **VPN-assigned exit IP**; also publish the node's **dynamic WAN IP** (classic home-DDNS) under a separate name, so the node is addressable both on- and off-VPN. |

## Architecture

### Where it lives
A **`ddns` worker in `mackesd`** (sits beside the VPN-GW `vpn_gateway` worker).
It subscribes to egress-IP changes and reconciles DNS records. No new daemon —
glue over VPN-GW + the DO API (§6: glue, not reimplementation).

### IP discovery (the "dynamic" sources)
- **VPN exit IP** — reuse VPN-GW's existing per-tunnel **exit-IP verification**
  (VPN-GW-6: it already curls the exit IP through the tunnel and compares it to
  the provider's expected range). The verified exit IP is the value DDNS
  publishes. Per-provider specifics where the API exposes the assigned IP/port
  directly (Mullvad/Proton/IVPN/Nord/Surfshark) feed the same value + any
  **forwarded port**.
- **Node WAN IP** — a STUN/`https://…/ip`-style check (overlay-independent) for
  the raw dynamic WAN address, on its own schedule.
- A change is detected by **diffing the last-published value** (stored per record
  in the mesh state) — only a real change triggers a DO API call (no churn).

### DNS writing (DigitalOcean adapter)
- A **`DnsWriter` trait** with a **DigitalOcean implementation** (DO API v2:
  `GET/POST/PUT/DELETE /v2/domains/{matthewmackes.com}/records`). v1 ships DO; the
  trait keeps Cloudflare/Route53/etc. addable without touching the worker.
- The adapter **upserts** the A/AAAA record for each managed hostname under
  `services.matthewmackes.com` to the current IP with a **short TTL** (e.g. 60 s)
  so failover/reconnect propagates fast. On tunnel-down it can **remove** the
  record (or point it at a sentinel) per the route's kill-switch policy.
- The DO API token is a **secret** — stored age-encrypted in the mesh secret
  store, leader-distributed (rides the same secret plumbing as VPN-GW-2); never
  in `ps`/logs.

### Config (the "configuration setting that allows DDNS setup")
A `ddns` block in the mesh config (TOML, the one-state doctrine §9 W88):
```toml
[ddns]
enabled = true
provider = "digitalocean"        # DnsWriter adapter
zone     = "services.matthewmackes.com"
token_ref = "secret://ddns/do-token"
ttl_seconds = 60

# one entry per tracked egress (auto-created from VPN-GW tunnels/routes,
# or hand-added for the raw WAN)
[[ddns.record]]
name   = "{node}-{provider}"     # -> eagle-mullvad.services.matthewmackes.com
source = "tunnel:mullvad-1"      # a VPN-GW tunnel id … or "wan" for the node WAN
on_down = "remove"               # remove | sentinel | keep
```
Auto-population: when VPN-GW creates a tunnel/route, the DDNS worker can
**auto-add a record entry** for it (templated name) so "assign a VPN IP → get a
subdomain" happens with **zero extra steps**, exactly as asked.

### Bus surface (RPCs)
- `action/ddns/{list-records, set-record, remove-record, sync-now, record-status}`
  — backend = the `ddns` worker + the DO adapter. The UI is a thin renderer.

### UI
- A **DDNS section in the VPN/Routing panel** (extends VPN-GW's panels, no new
  top-level plane): a table of managed names — `hostname · source (tunnel/WAN) ·
  current IP · last-updated · TTL · status (synced/stale/error)` — plus
  add/edit/remove and a **Sync now** button. Carbon tokens only (§4).
- The VPN panel's per-tunnel card gains a **"published as <hostname>"** line
  showing the live DNS name + whether it currently resolves to the exit IP.

## Acceptance (high level; per-task bullets in the worklist)
- Bringing up a VPN tunnel (any of the 5 providers) **auto-creates/updates** an
  A record under `services.matthewmackes.com` pointing at the verified exit IP;
  on reconnect-with-new-IP the record is rewritten within ~TTL.
- The node's raw **WAN IP** is published under its own name and updated when it
  changes (works with no VPN up).
- Records are written through the **DigitalOcean API** with a token kept as an
  encrypted mesh secret (never logged); a wrong/expired token surfaces a clear
  `ddns/auth` error, not a silent no-op (§7).
- On tunnel-down the record follows the configured `on_down` policy
  (remove/sentinel/keep) — no stale record silently pointing at a dead/leaking
  exit.
- The VPN/Routing panel shows each managed name, its source, current IP,
  last-update, and sync status — all real over the RPCs (no stubs, §7).

## Risks / notes
- **VPN exit IPs are usually shared + not inbound-reachable** unless the provider
  offers **port-forwarding or a dedicated IP** (Mullvad/Proton/IVPN expose a
  forwarded port; some plans give a dedicated IP). The reachability use only
  works on such tunnels — the UI must flag a name whose exit can't accept inbound
  (publish the identity record regardless; mark reachability "port-forward only").
- **DO API rate limits / propagation** — short TTL helps clients but DO still
  has a propagation floor; debounce updates and only write on a real IP change.
- **Secret hygiene** — the DO token is full DNS-zone write access; scope it to the
  one zone if DO supports it, age-encrypt, leader-distribute, rotate on removal.
- **Leak-coupling with the kill-switch** — when VPN-GW's kill-switch blocks a
  dropped tunnel, DDNS must not leave a record pointing at the (now blocked or
  re-leaking) IP; tie `on_down` to the route's kill-switch state.
- **Naming collisions** — two nodes using the same `{node}-{provider}` template
  must resolve to distinct hostnames; the template includes the node id and an
  optional `-{n}` for same-provider multi-instance (mirrors VPN-GW multi-instance).

## Out of scope (v1)
- DNS writers other than DigitalOcean (the `DnsWriter` trait makes
  Cloudflare/Route53/Google/Namecheap additive later).
- ACME/Let's-Encrypt cert issuance for the published names (a natural follow-on
  once names are stable — pairs with the CONNECT ingress plan).
- Round-robin / GeoDNS across multiple exits for one name (single A per name in
  v1; failover rewrites the same record).
