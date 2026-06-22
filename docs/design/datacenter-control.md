# Datacenter Control — full Xen + Tofu + DO control plane from Workbench

**Status:** design locked via a 100-question operator survey (2026-06-22).
**Pairs with:** the DEVOPS-SUBSTRATE/DS epic, `infra/tofu/` (Xen) + `infra/tofu/zone1-do/` (DO), and the zone model in [`zones.md`](zones.md).
**Master rule (AI_GOVERNANCE §0):** *Secure, Simple, No-Fixed-Center Workgroup* — this design removes the center rather than building one.

## 1. Summary

A new Workbench **Datacenter** plane gives the operator vSphere/XO-class control of the
whole platform — bare-metal Xen hosts, VMs, storage, networking, the UniFi gateway, and
the DigitalOcean production fleet — plus a first-class **Tofu (IaC)** console, all from one
surface. It is explicitly **no-fixed-center**: there is no single control host whose loss is
fatal. The control plane is a leader-gated mackesd worker that any eligible node can assume,
talking **XAPI directly** (no central XO), with **mesh-replicated Tofu state** and secrets in
the **etcd+age** store. The plane both *controls* infrastructure and *rolls it out*: XCP hosts
become a deployable platform role, and a New-Mesh wizard can **give birth to a new Nebula**.

## 2. Zones (recap — see `zones.md`)

| Zone | Compute | Workloads | IaC state |
|------|---------|-----------|-----------|
| **Dev** | Xen hosts only (3 XCP-ng dom0s) | build + test/CI | `infra/tofu/` |
| **Production** | DigitalOcean droplets + **Eagle** (LAN member) | the real fleet (lighthouse-anchored) | `infra/tofu/zone1-do/` |

The plane presents **separate per-zone top tabs** (Dev / Prod) **+ a Gateway tab**. Separate
Tofu states; no `apply` ever spans both zones.

## 3. The no-fixed-center control plane

The naïve build has four hidden centers — XO on one host, Tofu state as local files, secrets in
`/root`, and a pinned worker. All four are dissolved:

- **Compute/control:** a leader-gated **`datacenter_orchestrator`** mackesd worker (one per zone:
  a Xen-control leader among always-on **on-LAN** mackesd nodes, and a DO/global leader electable
  anywhere since the DO API is internet-reachable). Leader loss → any eligible survivor re-assumes
  it (same pattern as `farm_orchestrator`). dom0s can't run mackesd (XCP-6) so the worker runs on
  mesh nodes, not the hypervisors.
- **Xen API:** **XAPI directly** to each pool master — no central XO. XAPI (LAN-only) is **routed
  over the Nebula overlay** so any elected leader can reach it. *(This supersedes the survey's
  earlier "via XO" answers — newest-wins.)*
- **Tofu state:** **mesh-replicated remote backend** (SUBSTRATE-V2: etcd/Syncthing) with locking, so
  any leader plans/applies against the same state. The Xen IaC migrates from the `xenorchestra`
  provider to a **XAPI-native provider** (XO fully dropped); DO stays on the `digitalocean` provider.
- **Secrets:** XAPI/DO/UniFi credentials live in the **etcd+age-over-Nebula mesh secret store
  (DS-8)**, replicated to every leader-eligible node. No host-local secret dependency.
- **Panel reach:** any node renders state from the **mesh-replicated Bus**; actions are **routed to
  whichever node holds that zone's leader**. The GUI works everywhere; control flows to the right place.

**Transport:** the worker samples/poll XAPI + DO + UniFi and publishes deltas to **`event/dc/*`**
(`hosts`, `vms`, `storage`, `net`, `tofu`, `power`, `audit`, `promote`); the panel **subscribes**.
Long operations are **async jobs on the Bus** (resumable across reloads, cancelable, with progress);
mutations take **per-resource op-locks** + state reconciliation for the multi-operator/RBAC future.

## 4. Information architecture

Top tabs: **Dev (Xen)** · **Prod (DO)** · **Gateway**. Each zone tab has sub-tabs:

| Sub-tab | Contents |
|---------|----------|
| **Overview** | cross-zone capacity/health rollup, active alerts, recent Tofu runs, the **Build→Eagle→DO promotion strip**, and the **version matrix** (which RPM runs on farm/Eagle/each lighthouse) |
| **Hosts** | per-host capacity+health rollup, pools (membership/master/join), full host lifecycle (maintenance, reboot, shutdown, **evacuate**, patch), copy/launch-ssh console |
| **VMs** | full lifecycle (power/suspend/migrate/clone/snapshot/resize/delete); **create via the golden-template wizard, Tofu-backed**; **embedded noVNC** console; bulk actions |
| **Storage** | SRs + VDIs (attach/detach/create), **scheduled snapshots w/ retention + backup target**, **ISO + template library** (absorbs `images`), SR capacity threshold alerts |
| **Network** | full L2 (PIFs/VLANs/NIC mgmt/create), **overlay management**, **interactive topology map**, **unified IP/DNS** (UniFi leases ↔ DO DNS ↔ overlay IPs) |
| **Tofu** | plan/apply/destroy with streamed output, **state browser + drift**, **per-zone workspace cards**, persisted Bus **run-log**; **plan→review-diff→explicit Apply** gate |

**Gateway tab** (UniFi `172.20.0.1`): full control — status, DHCP leases (fleet discovery),
firewall/port-forward edits, reboot — via the worker over SSH + the UniFi API.

**Look:** card-grid layout, **color-dot** status (dots sourced from `mde-theme` Carbon tokens, never
raw hex, paired with a glyph/label for color-blind safety per §4), global search + per-tab filters +
saved views, graceful-degrade on unreachable (last-known + stale badge + retry), toast errors.

## 5. Power orchestration (energy-aware)

Bare-metal Xen hosts are powered by demand:

- **Idle → shutdown** immediately when a host has **zero running VMs** (graceful `host-disable` +
  shutdown).
- **Wake on demand:** assigning a workload wakes the target host first — **IPMI/iDRAC primary, WOL
  magic-packet fallback** (sent from an always-on LAN peer), then poll XAPI until the toolstack is up.
- **Boot-time ETAs:** every wake is timed (WOL→dom0-ready), kept as a rolling per-host average, and
  drives a **phased progress bar** (POST → XCP up → toolstack ready) with a live ETA — accuracy
  improves over time.
- Placement/scheduling integrates power state: *assign → wake (show boot progress) → place VM*.

## 6. Provisioning, rollout & genesis

- **One-click flows:** add a build VM, spin an ephemeral N-node test mesh (hermetic up/down, wraps
  `farm-testbed.sh`), cut a new lighthouse, clone the golden template — all Tofu-backed.
- **Build-farm scaling:** a scale control adjusts desired build-VM count; Tofu spreads them across hosts.
- **XCP host rollout (new platform role):** XCP-ng hosts are a **first-class "Hypervisor" node role**.
  Deploy via a **USB/ISO installer with a prebaked answerfile**; the host joins the overlay as a
  **static-Nebula member** (no mackesd on dom0, per XCP-6) managed through a mesh-side peer/agent.
  Hosts are a **uniform hypervisor type** — role comes from the VMs they run. Day-2 **care-and-feeding**:
  rolling **patching** (evacuate-first), health/care alerts, **Nebula cert rotation**, and
  **auto-replace** (re-provision + onboard a replacement when a host dies).
- **DO provisioning:** a **full region picker** (geo labels + latency/price hints) with a
  **multi-region spread recommendation** for the (soon two) lighthouses; droplets use a **fixed
  lighthouse profile** (region is the only knob); minimal add-ons. **Guided new-lighthouse**:
  create droplet (Tofu) → bootstrap mackesd → found/join the prod mesh → add the DNS record.
- **Give birth to a new Nebula:** a **New-Mesh wizard** — generate the CA, provision + found the first
  lighthouse, seed config, register DNS, hand out the first join token. The **source of truth for
  genesis secrets is the mesh secret store**; an optional private repo may hold only non-secret
  templates/manifests + **age-encrypted** key material (never plaintext).

## 7. Enhanced Workstation profile

Stack: **Hardware → XCP-ng → dom0 (hidden, management-only) → Primary Desktop VM (owns
monitor/keyboard/mouse/audio/apps via PCI passthrough) → user experiences the VM as the local
desktop.** The Primary Desktop VM **auto-launches** at boot and binds the passed-through GPU/USB/audio;
dom0 stays hidden. A small **management VM mediates** the console — it can reclaim the display for
recovery if the desktop VM fails to start.

## 8. Promotion pipeline

The Overview strip shows the artifact moving **Build (Xen) → Eagle → DO**. **Auto-promote on green**
L1–L3 tests advances Build→Eagle automatically; the **DO step is gated by the prod-arm switch** (armed
= green auto-promotes; disarmed = queued for arming). The **version matrix** makes stage drift obvious.

## 9. Safety, RBAC & audit

- **Confirmation:** typed-name confirm for prod + destructive ops; single confirm for routine dev ops.
- **Prod guardrails:** the Prod tab starts **disarmed**; an explicit **"arm prod"** toggle + typed
  confirm + audit gate every production change.
- **Impact preview:** before acting, show affected VMs/expected downtime; for structural changes, the
  **Tofu plan diff**.
- **RBAC:** principals come from **mesh identity (Nebula cert/peer) + a role map**; **mesh-authenticated
  session** (being a trusted node = auth). Two effective roles ship — **viewer** (read) and **operator**
  (do-all) — on an RBAC framework with **admin** reserved.
- **Audit:** every action → **append-only `event/dc/audit/*`** log with an in-panel viewer.

## 10. Observability & DR

- **Metrics:** live XAPI query per panel load + worker-sampled **short rolling history** on the Bus for
  sparklines (~24–48h). **Alerts** (→ Bus/Notification Hub): host down, VM crash, pool degraded, drift
  detected, token/cert expiry, SR capacity.
- **Logs:** host/VM/worker logs aggregated into the existing **fleet_logs/Bus** pipeline with a
  per-resource view.
- **Health:** periodic checks (host/VM/overlay/cert) feed the existing **health_check** panel + alerts.
- **DR:** periodic **encrypted backup of Tofu state + Nebula CA + secret store** to an off-fleet target,
  with a **one-click restore** that can rebirth the control plane.

## 11. Acceptance (§7 — runtime-observable, the "done" bar)

From the Datacenter plane, the operator can, **with each step observable on the Bus/UI**:
1. Birth a new mesh (New-Mesh wizard → working lighthouse + first join token).
2. Roll out an XCP host (USB/ISO answerfile → host joins as a Hypervisor-role static-Nebula member).
3. Provision and fully control VMs, storage, and networking across both zones.
4. Run `tofu plan/apply/destroy` per zone with the review gate, against mesh-replicated state.
5. Manage the UniFi gateway (leases + firewall + reboot).
6. Promote a build Build→Eagle→DO with the version matrix reflecting it.
7. Wake/sleep hosts with accurate boot-ETA progress.
8. **Kill the node currently holding a zone leader and have the control plane survive** — another
   eligible node assumes it and the panel keeps working (read-via-Bus, act-via-new-leader).

## 12. Phasing (big-bang epic, foundations sequenced first)

The operator chose a complete build; foundations are prerequisite **phases**, not a reduced MVP:
**Phase 0 Foundations** (XAPI-native Tofu provider · SUBSTRATE-V2 remote state · DS-8 secret store ·
XAPI-over-overlay) → **Phase 1 Worker** (`datacenter_orchestrator`, async jobs/op-locks, RBAC+audit) →
**Phase 2 Plane+Tabs** → **Phase 3 Power** → **Phase 4 Rollout/Genesis/DO/Promotion** →
**Phase 5 Workstation+DR+Observability** → **Phase 6 Consolidation** (absorb/deprecate the overlapping
panels — `compute`, `vm_wizard`, `snapshots`, `images`, `lighthouses`, `build_farm` — into Datacenter tabs).

## 13. Risks & open items

- **XAPI-native Tofu provider** maturity vs the current `xenorchestra` provider — migrating
  `infra/tofu/` Xen resources is non-trivial; validate import parity before cutover.
- **XAPI-over-overlay routing** adds a hop + failure mode; needs a reliable on-LAN relay peer.
- **WOL/IPMI prerequisites:** BIOS/NIC WOL enabled, or out-of-band mgmt present — a per-host setup gate.
- **SUBSTRATE-V2 + DS-8 are unbuilt** — Phase 0 is real, blocking work (this is the chosen sequence).
- **Mesh-birth secret boundary** (Q85 "mesh store" vs Q86 "templates in repo") reconciled to:
  store = source of truth, repo = templates + age-encrypted only. Confirm before building genesis.
- **Color-dot status** vs §4 — implemented through `mde-theme` tokens + glyph/label, not raw color.

## 14. Out of scope

Only **billing/cost management** is excluded. Everything else technical (incl. non-XCP hypervisors and
clouds beyond DO) is *eventually* in scope but not part of this epic's first delivery.

## Appendix — the 100 locks

The full survey (Q1–Q100) is captured section-by-section above; the load-bearing reversals from
newest-wins reconciliation are: XO→XAPI (Q74/Q77 over Q7/Q10/Q11), live-XAPI metrics (over Q38 "XO"),
RBAC two-role (Q48 over Q42), auto-promote×prod-arm (Q43+Q50), mesh-store genesis (Q85 over Q86),
token-sourced color dots (Q66 within §4), and big-bang-with-foundations-first (Q93+Q94).
