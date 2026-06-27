# Router Control — per-node Vyatta-CLI router/firewall discovery + control

**Status:** Locked (survey 2026-06-27). **Scope:** generalize the single
hardcoded EdgeRouter integration into a **per-node, auto-discovered Vyatta-CLI
router/firewall control plane**. **Master rule (§0):** Secure, Simple,
No-Fixed-Center Workgroup — discovery + control run per-node, creds live in the
replicated mesh secret store, no central router-controller.

## 1. Problem

Each node may sit behind its own router/firewall it can leverage. Today MCNF
controls exactly **one** appliance: the EdgeRouter at a hardcoded `172.20.0.1`,
via `infra/tofu/edgeos/` (Vyatta CLI over SSH: `configure/set/delete/commit/save`,
creds in `/root/.mcnf-ubnt-cred`) plus a single `MCNF_UNIFI_HOST` + `unifi-cred`
in `datacenter_orchestrator` (`gateway-status`/`gateway-dhcp`/`gateway-reboot`).
There is **no auto-discovery** of a node's default-route appliance and **no VyOS
support**.

This epic makes every node:
- **(A)** find its default-route appliance (+ enumerate LAN management appliances),
- **(B)** fingerprint it as a Vyatta-CLI router (EdgeRouter/EdgeOS or VyOS) and
  match a stored credential, and
- **(C)** surface its controls in a dedicated Router panel,

reusing the existing `edgeos` tofu pattern (Vyatta CLI over SSH) as the engine.

## 2. Locks (survey + re-ask confirmed)

| # | Decision | Lock |
|---|----------|------|
| 1 | Discovery source | Default-route hop **+ all LAN management appliances** (reuse `netassess::parse_default_gateway` + `surrounding_hosts`) |
| 2 | Where it runs | **Per-node, locally** (always-on, not leader-gated) |
| 3 | Multi-homed | Manage the **lowest-metric default route** as primary |
| 4 | Unknown / no creds | Surface **read-only** with a **"needs credentials"** prompt; never guess |
| 5 | Fingerprint | **Layered**: passive (MAC-OUI + SSH banner) → credentialed `show version` |
| 6 | Vendor scope | **EdgeRouter / Vyatta-CLI only** — one adapter (VyOS rides the same CLI). No UniFi-OS, no REST APIs |
| 7 | EdgeOS vs VyOS | **Post-login `show version`** ("EdgeOS"/"UBNT" vs "VyOS") |
| 8 | UniFi-OS | **Out of scope** |
| 9 | Cred keying | **Per-appliance `router/<id>`** in the age-encrypted mesh secret store |
| 10 | Cred id | **Gateway MAC** (hardware-stable, from ARP/`ip neigh`) |
| 11 | Discovery → tofu | Discovery writes a **per-node tfvars** (`edgeos_host` + cred-ref) |
| 12 | Tofu state | **Per-appliance http backend** `state/router/<id>` (mirrors xen-xapi/zone1-do) |
| 13 | Control surfaces | Interfaces+status (read), DHCP (exists), **Firewall**, **Port-forward/NAT**, **VPN endpoint**, **Reboot** |
| 14 | GUI | **New dedicated Router panel** (mde-workbench, under Fleet) |
| 15 | Mutation safety | **Vyatta `commit-confirm`** auto-rollback + typed-confirm + hash-chain audit |
| 16 | Rollout | **Read slice first** (discover→fingerprint→cred-match→read), mutations fast-follow |

## 3. Architecture

### A. Discovery (per-node, in mackesd)
A per-node `router_discovery` source (always-on, in `datacenter_orchestrator`
or a sibling worker) that each tick:
1. Reads the **lowest-metric default route** via `netassess::parse_default_gateway`
   (`ip route show default`) → the primary appliance IP.
2. Enumerates **LAN management appliances** from `surrounding_hosts` (hosts typed
   `HostType::Router` by MAC-OUI / HTTP banner / nmap).
3. Resolves each appliance's **MAC** (ARP/`ip neigh`) → the stable `<id>`.
4. Emits candidates into the **router-registry** (below), marked `managed` only
   when a `router/<mac>` cred exists; otherwise `unmanaged` + `needs-creds`.

### B. Fingerprint (layered)
- **Passive:** MAC-OUI (Ubiquiti…) + SSH banner — cheap, reuses `surrounding_hosts`.
- **Active (when a cred exists):** SSH in over the Vyatta CLI and run
  `show version` → classify `EdgeOS` vs `VyOS`. Vendor family = **Vyatta-CLI only**;
  anything else is surfaced `unmanaged/unknown-vendor`.

### C. Credentials
- Per-appliance, **`router/<mac>`** in the mesh secret store (the `mcnf-secret.sh`
  / `secret_store` path that already does `xcp/<host>`, `vpn/<if>`). Generalizes
  `/root/.mcnf-ubnt-cred`/`unifi-cred`.
- **Operator-sealed only** (GUI cred form or `mackesd secret put router/<mac>`);
  the agent never invents creds (matches the live `unifi-cred` flow).
- Cred body reuses the EdgeOS shape (`user:pass`, default user `ubnt`/`vyos`),
  consumed exactly like the `edgeos` tofu `cred_file` (`sshpass -f`, never argv).

### C′. Control (the Router panel + tofu engine)
- **New `router` panel** in `mde-workbench` (wired app.rs/model.rs/panels/mod.rs,
  Carbon tokens), one card per discovered appliance (per node), driven by
  `action/router/*` Bus RPCs (mirrors `action/dc/*`).
- **Reads:** status/interfaces/version/uptime/model + DHCP reservations & live
  leases (reuse the `edgeos` `poll-leases.sh` + a `show interfaces`/`show version`
  probe).
- **Mutations are tofu-as-code:** a per-appliance generalization of
  `infra/tofu/edgeos/` — discovery writes a **per-node tfvars** (`edgeos_host`,
  cred-ref), state in the **per-appliance http backend** `state/router/<mac>`. New
  `null_resource` converge scripts for **firewall**, **port-forward/NAT**, and
  **VPN endpoint** rulesets, same converge-to-exact UX as DHCP.
- **Safety:** every mutating apply wraps the Vyatta `commit-confirm <min>` so a
  self-lockout auto-reverts; plus the existing typed-confirm + prod-arm + hash-chain
  audit (`event/dc/audit/*`). **Reboot** stays a direct confirm-gated SSH command
  (not tofu).

### Registry
A `router-registry` mirroring `media_registry`: per-node publish to Bus
`mesh/devices/router/<mac>` + QNM-Shared `<workgroup_root>/<node>/router-registry.json`
(on-change + heartbeat). The panel unions these for a fleet view.

## 4. Rollout (read slice first)

1. **Read slice:** discovery + layered fingerprint + cred-match + READ
   (status/version/interfaces/leases) across all nodes; router-registry; the
   Router panel **read** view; `mackesd secret put router/<mac>` + GUI cred form.
2. **Mutations fast-follow:** generalize the `edgeos` tofu root to per-appliance
   (per-node tfvars + `state/router/<mac>`); firewall + port-forward/NAT + VPN-endpoint
   converge scripts; commit-confirm + typed-confirm + audit; reboot.

## 5. Acceptance (runtime-observable)

- A node behind an EdgeRouter auto-lists its appliance (IP+MAC+vendor+version) in
  the Router panel within one discovery tick, with no `MCNF_UNIFI_HOST` set.
- A second node behind a *different* EdgeRouter lists *its* appliance, keyed by
  that appliance's MAC, using `router/<mac>` creds.
- An appliance with no sealed cred shows `unmanaged — needs credentials`; sealing
  `router/<mac>` flips it to managed + populates `show version`.
- A firewall/port-forward edit applies via tofu converge, is visible on the live
  router, and **auto-reverts** if not re-confirmed (commit-confirm), with an audit row.

## 6. Risks

- **Self-lockout** (a firewall/NAT edit cuts the node off its own router) →
  mitigated by Vyatta `commit-confirm` auto-rollback. Load-bearing.
- **MAC-keyed cred orphaning** if an appliance NIC changes — acceptable; re-seal.
- **VyOS-on-Ubiquiti-hardware** misfingerprint by MAC-OUI → the active `show version`
  is authoritative.
- **el9 nodes can't run mackesd** (e.g. `rocky9-kvm2`, glibc 2.34) → those nodes
  can't host the per-node worker; their appliance is managed from a Vyatta-capable
  mesh node or left to the existing single-`edgeos`-root path. (See
  `eagle-offmesh-restore` memory.)
- **Multi-default ambiguity** → lowest-metric only (lock #3); others surfaced but
  unmanaged.

## 7. Out of scope

UniFi-OS / Dream Machine (mca-cli-op, controller API); the VyOS HTTP API;
non-Vyatta vendors (MikroTik/OPNsense/pfSense/generic); WAN-side / ISP-gateway
mutation. These can extend the adapter registry later.
