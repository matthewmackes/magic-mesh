# Voice — per-node Vitelity SIP addresses (VOIP-GW-2..7)

> **Status: LOCKED 2026-07-02** — 20-question operator survey (this session, per
> `/plan`). Every Dialer (node) gets its **own callable inbound SIP address** via a
> per-node Vitelity **sub-account**; **outbound continues on the ONE shared account**;
> **all client config for the whole fleet is surfaced in the Workbench Voice panel**.
> Continues the VOIP-GW / OW-11 Voice lineage (VOIP-GW-1 = the shared-account
> softphone that exists today).

## The locks

| # | Fork | Lock |
|---|------|------|
| 1 | Inbound identity | **Per-node Vitelity sub-account.** Each node registers its own sub-account (distinct SIP creds) as its inbound identity — directly callable, ringing that node. |
| 2 | Provisioning | **Vitelity-API auto-provision, node-sealed.** A mackesd worker calls Vitelity's sub-account API to create/fetch a sub-account per node and seals its creds to that node; the operator supplies the master API key once. |
| 3 | Address scheme | **Hostname-derived SIP URI** — the sub-account username derives from the node's hostname, so its callable address is a stable `<hostname>@<realm>` (matches hostname=identity). |
| 4 | Outbound | **Shared trunk, shared caller-ID.** All outbound PSTN routes through the ONE shared Vitelity account and presents the shared number as caller-ID; the sub-account is receive-only. |
| 5 | Panel scope | **Fleet config board + local dialer.** The Voice panel gains a fleet section: every node with its SIP address, provisioning state, live registration status, DID routing, failover — plus the existing local dialer/HUD. |
| 6 | Registration | **Single registration.** Each node registers only its sub-account (for inbound); **Vitelity is configured to bridge that sub-account's outbound onto the shared trunk** — so "outbound = shared account" holds Vitelity-side with a simple one-registration client. |
| 7 | Secrets | **Mesh secret store.** Each sub-account's SIP password is age-sealed to its node (only it decrypts); the **Vitelity master API key is held by the leader/provisioner only** (never distributed). Reuses the DS-8/media-spaces pattern; no creds in `account.toml` or the UI. |
| 8 | Provision trigger | **Auto at enrollment + a panel "Provision/Re-provision" button** (zero-touch for new nodes; operator control for retries/re-key/legacy nodes). |
| 9 | Status | **Live per-node REGISTER state + presence.** Each node publishes `Registered/Unregistered/Provisioning/Error+reason` to a Bus topic; the board shows it live; the roster contact gets a "reachable via Vitelity" pip. Honest — a failing node shows the real error. |
| 10 | Offline inbound | **Vitelity failover: voicemail / forward, per-node policy** (set in the panel). No lost calls; policy is per-Dialer. |
| 11 | DIDs | **Route existing master-account DIDs only** — the panel lists the master account's already-assigned DIDs and routes one to a node's sub-account; **no new DID provisioning** via API. |
| 12 | Intra-mesh | **Stays P2P over Nebula.** Node-to-node calls resolve directly peer-to-peer as today; a node's Vitelity sub-account is purely its EXTERNAL inbound identity — the mesh never hairpins internal calls through Vitelity. |
| 13 | Shared account | **Fleet-level, leader-held.** The shared account (trunk creds, caller-ID, dial rules) is one fleet config set once in the panel, sealed to the leader/provisioner. |
| 14 | Vitelity API | **Typed client behind an injectable seam** (sub-account create/list, DID list/route, failover/voicemail config); headless-tested with fakes; the live impl is integration-gated with typed errors, never faked (the mde-kvm/mde-seat pattern). |
| 15 | Architecture | **mackesd `voice_provision` worker** (leader-gated: Vitelity client + provisioning + reconcile + secret sealing + `state/voice/<node>` publish) + **`mde-voice-hud` split** (inbound-sub config vs shared-outbound config) + **`mde-voice-egui` panel** (fleet board + local dialer). §6-clean tiers. |
| 16 | Panel form | **Extend the Voice dock surface with a "Fleet" tab** beside the local dialer (not a new Workbench plane) — matches the Music/Files/Voice embed pattern. |
| 17 | SIP security | **SIP/TLS (5061) + SRTP where Vitelity supports it, honest UDP (5060) fallback** with the downgrade surfaced in the panel. Max available confidentiality to the provider within PSTN interop. |
| 18 | Migration | **Hard cutover** — every node re-provisioned onto the split model before voice resumes (clean end state, accepted flag day). |
| 19 | Self-healing | **Leader reconciles Vitelity ⇄ roster** (desired = every enrolled node has a sealed sub-account + DID routing + failover, fixed idempotently + rate-limited) + **nodes auto-re-REGISTER on drop** with backoff. Panel shows unreconciled drift. |
| 20 | Epic | **VOIP-GW lineage** (VOIP-GW-2..7). |

## Architecture

```
mde-voice-egui (Voice dock surface)         mackesd (leader-gated)
┌──────────────────────────────┐            ┌──────────────────────────────────┐
│ [Dialer]  [Fleet ▾]          │  action/   │ voice_provision worker           │
│ Fleet board:                 │  voice/*   │  · vitelity client (typed seam): │
│  node · SIP URI · reg-state  │───────────▶│    sub-account create/list,      │
│  · DID routing · failover    │◀───────────│    DID list/route, failover/vm   │
│ shared-account fleet config  │  state/    │  · reconcile Vitelity ⇄ roster   │
└──────────────┬───────────────┘  voice/*   │  · seal sub-creds (node) +       │
               │                             │    hold master key (leader)      │
   mde-voice-hud (per node)                  │  · publish state/voice/<node>    │
   · SipAccount → { inbound_sub,             └──────────────────────────────────┘
│    shared_outbound }  (single REGISTER      secret store (age-into-etcd):
│    of the sub; Vitelity bridges outbound)    sub-creds node-sealed · master key leader
│  · SIP/TLS+SRTP, UDP fallback, auto-re-REG
│  · intra-mesh stays P2P (resolve.rs)
```

- **`mackesd::workers::voice_provision`** (leader-gated): the typed `vitelity` client
  (injectable seam, live impl integration-gated), the provision-on-enrollment + the
  reconcile loop, secret sealing (sub → node, master → leader), and the
  `state/voice/<node>` reg-state + fleet-board mirror.
- **`mde-voice-hud`**: `SipAccount` splits into `InboundSub` (the registered
  sub-account) + `SharedOutbound` (fleet config, outbound bridged Vitelity-side);
  single REGISTER of the sub; SIP/TLS+SRTP with honest UDP fallback; auto-re-REGISTER
  on drop; **intra-mesh calls keep resolving P2P** (`resolve.rs` unchanged for the
  mesh path — Vitelity only for external).
- **`mde-voice-egui`**: the Voice surface grows a **Fleet tab** — the config board
  (per-node SIP URI, provisioning/reg state, DID routing, failover policy) + the
  shared-account fleet config; the local dialer stays.
- **Reuse (§6 glue)**: the mesh secret store (DS-8), the roster (`mde-voice-hud::roster`),
  the presence snapshot, the onboard enrollment hook (auto-provision), the existing
  SIP REGISTER/digest core (`sip.rs`).

## The units (VOIP-GW-2..7)

- **VOIP-GW-2 — the typed Vitelity API client.** A `vitelity` module (mackesd or a
  small shared crate): sub-account create/list, DID list/route, failover/voicemail
  config, over Vitelity's HTTP API behind an injectable trait; pure request/response
  folds unit-tested; the live impl integration-gated (needs the master key + net),
  typed errors never faked.
- **VOIP-GW-3 — the mackesd `voice_provision` worker.** Leader-gated: auto-provision
  a sub-account at enrollment + on the panel button; seal sub-creds to the node +
  hold the master key at the leader; reconcile Vitelity ⇄ roster idempotently +
  rate-limited; publish `state/voice/<node>` (reg state) + the fleet-board data.
- **VOIP-GW-4 — the `mde-voice-hud` split + secure register.** `SipAccount` →
  inbound-sub + shared-outbound; single REGISTER of the sub-account; SIP/TLS+SRTP
  with honest UDP fallback; auto-re-REGISTER on drop with backoff; intra-mesh P2P
  preserved; the reg state published for VOIP-GW-3's mirror.
- **VOIP-GW-5 — the Voice panel Fleet tab.** `mde-voice-egui` grows a Fleet tab: the
  per-node config board (SIP URI, sub-account/reg state via `state/voice/*`, live
  presence pip) + the shared-account fleet config, editable where sensible
  (nickname, provision/re-provision, enable/disable inbound); the local dialer stays.
- **VOIP-GW-6 — DID routing + per-node failover.** List the master account's existing
  DIDs + route one to a node's sub-account; set each node's voicemail/forward failover
  — both via the VOIP-GW-2 client, surfaced in the Fleet tab.
- **VOIP-GW-7 — hard-cutover migration.** Lift the existing `account.toml` account to
  the fleet-level shared-outbound config (leader-held); require every node
  re-provisioned onto the split model; the flag-day cutover + a clear panel prompt.

**Serialization**: VOIP-GW-2 first (the client everything drives); VOIP-GW-3 + VOIP-GW-4
parallelize on it (worker vs hud, disjoint crates); VOIP-GW-5 (panel) on 3's state
contract; VOIP-GW-6 on 2+3; VOIP-GW-7 last (touches the account model + is the cutover).

## Acceptance (epic-level, runtime-observable)

1. A node auto-provisions a Vitelity sub-account at enrollment; its `<hostname>@<realm>`
   SIP address is listed in the Fleet tab and an external SIP call to it rings that node.
2. Outbound PSTN from any node routes through the shared trunk and shows the shared
   caller-ID (verified on the callee).
3. The Fleet tab shows every node's live reg-state + presence; a node that can't
   register shows the real error, not a fake online.
4. An existing master-account DID routed to a node's sub-account in the panel makes a
   PSTN call to that number ring that node; an offline node hits its configured
   voicemail/forward failover.
5. Node-to-node calls still connect P2P over Nebula (no Vitelity hairpin).
6. Sub-account SIP creds are node-sealed in the secret store; the master API key is
   leader-only; the leader reconcile heals a re-imaged node's sub-account without manual
   steps.
7. SIP registers over TLS+SRTP where Vitelity supports it; a UDP downgrade is shown
   honestly in the panel.

## Risks / out of scope

- **Risks**: Vitelity API shape/rate-limits (mitigate: injectable client + idempotent
  rate-limited reconcile); the hard-cutover flag day (mitigate: clear panel prompt +
  the shared-outbound lift keeps outbound alive through cutover); SIP/TLS support
  varying by Vitelity endpoint (honest UDP fallback); the master API key blast radius
  (leader-only, never distributed).
- **Out of scope**: provisioning NEW DIDs (route existing only, lock 11); a non-Vitelity
  provider abstraction (Vitelity-specific for v1); PBX/IVR features; call recording;
  per-node outbound trunks/caller-ID (outbound stays shared, lock 4); routing intra-mesh
  calls through Vitelity (lock 12).
