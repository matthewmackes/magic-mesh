# MCNF — Architecture

A secure, **no-fixed-center** workgroup mesh and the **egui-native, DRM-native
thin-client VDI desktop** that runs on top of it (the E12 "Construct" epoch). This
is the system map; the binding rules live in
[`AI_GOVERNANCE.md`](../AI_GOVERNANCE.md), the operator view in
[`ADMIN.md`](../ADMIN.md) + [`docs/help/`](help/).

## The three tiers

The dependency graph is **three lint-gated tiers**; dependencies point only
inward (shell → services → mesh), so an outward edge is a CI failure and the
substrate stays headless-capable (§6).

```
┌─ Desktop-shell tier (one egui shell owns the DRM/KMS seat — no compositor) ────┐
│  mde-shell-egui: the chrome bar + the five-plane Workbench + surface panels    │
│    surfaces: mde-{files,music,media,voice,term,editor,bookmarks,panel}-egui,   │
│              mde-mesh-view; VDI clients mde-vdi-{rdp,vnc,spice}                 │
│    browser (excluded workspaces): mde-web-cef (Chromium/CEF) + mde-web-preview │
│              (sandboxed Servo), driven via mde-web-preview-client              │
│    harness/look: mde-egui (eframe/wgpu + smithay DRM/GBM + libinput);          │
│                  mde-theme (the brand module — QBRAND); shared Style (§4)      │
├─ Platform-services tier ───────────────────────────────────────────────────────┤
│  mackesd: SQLite store · worker supervisor (ENT-6 breaker) · reconcile loop ·  │
│    Nebula CA · enrollment · the session-broker / vm-lifecycle / cloud /         │
│    container / chat workers · healthz/metrics/alerts                           │
│  mde-bus: file-backed pub/sub + RPC (action/<p>/<verb> → reply/<ulid>);        │
│           D-Bus only for FDO interop (notifications, MPRIS) — §2               │
│  magic-fleet: the no-fixed-center desired-state engine (ansible-backed)        │
├─ Mesh-substrate tier (§1–§3 locks) ────────────────────────────────────────────┤
│  Nebula overlay (Ed25519 identity, AES-256-GCM/ChaCha20) — the wire            │
│  etcd (coordination) + Syncthing (files, "/mnt/mesh-storage") — §1             │
│  rustls everywhere; RSA-4096 own KDC identity; no OpenSSL                      │
└────────────────────────────────────────────────────────────────────────────────┘
```

**No fixed center:** any node can author fleet revisions; peers replicate them
over Syncthing and each node elects + applies the head itself (`magic-fleet
reconcile`, FPG-8). The Lighthouse is a *relay + CA + control plane*, not a
controller — losing it is a recoverable event
([mesh-recovery](help/mesh-recovery.md)), not a head decapitation.

## The desktop model (E12)

The host is an **egui thin client, not a general desktop**. There are no native
host apps: a browser / office suite / game runs **inside a VM guest**. A
Workstation **brokers and displays full OS desktops** that run locally on
**libvirt/QEMU-KVM through the Workloads plane** or **remotely on any mesh peer**,
rendered egui-native (ironrdp/VNC/Spice → an egui texture) over Nebula. A
"session" is a fullscreen VM desktop; sessions **roam** per-peer via
etcd/Syncthing. VM desktop guests are **first-class, dual-homed mesh members**.

**Two roles, one image.** One immutable bootc/ostree image ships for every role;
**role is a configuration flag, not a build**. **Lighthouse** (rank 0 — relay +
CA/signer + leader + media server, no display) and **Workstation** (rank 1 — the
full egui thin client + libvirt/QEMU-KVM + Podman). A **headless machine is a
Workstation without a local display**. (The pre-E12 Server/XCP-NG roles folded
into Workstation; an external XCP-ng host is adopted day-2, never produced.)

## The five planes

The Workbench IA is **five scope-first planes**, ordered by blast radius, with
the **Peers directory as the Front Door** (§9; source doc
[`design/planes.md`](design/planes.md)). The live enum is exactly
`Plane::{ThisNode, Cloud, Network, Fleet, Provisioning}`
(`crates/desktop/mde-shell-egui/src/workbench.rs`):

- **This Node** — this host's hardware, seat, node-local services + health.
- **Cloud** — provider-neutral Workloads: local libvirt VMs, Podman/Quadlet
  services, images, networks, configuration, and lifecycle through typed verbs.
- **Network** — the Nebula overlay, lighthouses, routes, reachability, nmstate.
- **Fleet** — every peer and the VM desktops they serve (a rollup lens + per-node
  KVM reality off the Bus).
- **Provisioning** — golden images, enrollment, bringing new peers online.

Doctrine (§9): **no RBAC** (a valid mesh cert is the authorization) · GUIs
**render**, they are never authorities · **one-state doctrine** (etcd + TOML/YAML on
Syncthing + typed `mackesd` Bus verbs; CLI parity).

## Crates

One workspace of **~40 crates** grouped by directory, plus the two **excluded**
browser-engine crates (`mde-web-cef`, `mde-web-preview`, each its own workspace +
`Cargo.lock`). The canonical member list is the workspace `Cargo.toml`.

| Group | Representative crates | Role |
|---|---|---|
| platform | `mde-bus`, `mde-role`, `mde-role-chooser` | pub/sub + RPC backbone · the 2-role model · first-run chooser |
| mesh | `mackesd` (+`meshctl` bin), `mackes-{config,mesh-types,nebula-https-tunnel,transport,xcp}`, `mde-enroll`, `magic-fleet` | control-plane daemon · covert TCP/443 tunnel · types/config · transport scoring · enrollment · fleet engine |
| services | `mde-files`, `mde-musicd`, `mde-voice-{hud,config}`, `mde-chat`, `mde-adblock`, `mde-bookmarks` | mesh file transport · music daemon · SIP · mesh-chat model · ad-filter · bookmark CRDT |
| desktop | `mde-shell-egui`, `mde-{files,music,media,voice,term,editor,bookmarks,panel}-egui`, `mde-vdi-{rdp,vnc,spice}`, `mde-mesh-view`, `mde-seat`, `mde-media-core`, `mde-jellyfin`, `mde-web-preview-client` | the one egui/DRM shell + surfaces · VDI clients · seat hardware access · media core |
| shared | `mde-egui`, `mde-theme`, `mde-disclaimer` | egui/DRM harness + shared `Style` · the `brand` module · runtime accept gate |
| kdc | `mde-kdc-host`, `mde-kdc-proto` | KDE Connect host (phones) · wire protocol |

## Key mechanisms

**Bus RPC.** A caller writes `action/<prefix>/<verb>` with a JSON body; the
responder polls (`list_since` + cursor), replies on `reply/<ulid>`. Bodies are
capped (64 KiB) before parse; reply/action topics reap on a 1 h ephemeral TTL;
`audit/*` is retention-forever. Responders catch panics and answer error
envelopes (the thread never dies).

**Worker supervisor.** `mackesd serve` spawns a large set of workers gated by the
pinned role (Lighthouse ⊂ Workstation, `worker_role::WORKER_TIERS`). Restart
policy with exponential back-off + a circuit breaker (ENT-6); panics are caught
and fed through the same path (EFF-4). A live `WorkerStatusMap` feeds the `ready`
verdict on healthz and the exporter's gauges (EFF-24/26).

**VM desktops & the cloud.** The `session_broker` + `vm_lifecycle` workers drive
VM desktops over libvirt/QEMU-KVM. The `cloud` worker owns typed
`action/cloud/*` verbs, renders OpenTofu for provisioning, and runs Ansible for
configuration against local libvirt, NetworkManager/nmstate, Podman/Quadlet,
and bootc/osbuild. The shell never speaks a provider API directly.

**Mesh routing.** `mesh_router` ticks 10 s: HTTPS-fallback activation
(UDP-failure threshold → TCP/443 TLS tunnel), then the scorer picks
primary/fallback per peer from the transport registry under the operator policy
(`/etc/mde/connect/policy.toml`) — including the CV-1 encryption floor. Every
path flip is a hash-chained audit event.

**File transfer.** Send-To copies into `<qnm>/inbox/<peer>/<sender>/` — Syncthing
replication *is* the wire; the receiving Inbox lists its directory. Sources are
confined to the operator share root (symlink-escape refused, EFF-2).

**Fleet.** A revision (YAML baseline + version) lands in the replicated revision
log; every node's `fleet_reconcile` worker shells `magic-fleet reconcile`, which
elects the newest head, converges host-local (no push-SSH), and writes an
apply-ack the author's FSM reads.

**Notifications = Mesh Chat.** The one notification surface is an ICQ-style mesh
chat: a `mackesd` `chat` worker subscribes every alert lane and folds each host's
alerts + clipboard copies into signed messages from that host's roster contact
(Bus-live + Syncthing-history, Ed25519-signed). This subsumed the retired
standalone Notifications and Clipboard surfaces (NOTIFY-CHAT).

**Observability.** `healthz` (CLI = store view; Bus = + live workers + `ready`),
the Prometheus textfile exporter (node health, CA-cert days-remaining, router
decision histogram, worker/breaker, disk headroom, backup posture), hash-chained
audit events with configurable `[[alert_hooks]]`, and severity-mapped journal
alerts (`target: mackesd::alert`) as the headless surface.

**Security posture.** Enrollment: single-use token (`--mesh-id`-scoped) → CSR →
CA sign under the active epoch; revocation is real (blocklist fingerprints,
Nebula refuses tunnels). CA rotation bumps the epoch and re-signs peers. Daily
encrypted state backup (`MDE_BACKUP_PASSPHRASE`, XChaCha20-Poly1305 + Argon2id)
to the replicated volume; restore via `mackesd state-restore <bundle>`. The trust
model (flat trust within a small workgroup, VM guests as full peers) is an
accepted, documented trade-off — [`DISCLAIMER.md`](../DISCLAIMER.md).

## What is deliberately NOT here

- **No Wayland compositor** — the egui shell owns the DRM/KMS seat directly (E12).
- **No native host apps** — a browser/office/game runs inside a VM guest.
- **No third-party desktop toolkit, no external design-token gate** — egui-native,
  one shared `Style` module is the whole look discipline (§4).
- No `mde <subcommand>` dispatcher — separate binaries.
- No central server, no SaaS, no telemetry egress.
- No OpenSSL (rustls; `cargo deny` bans it), no Gluster/LizardFS/Ceph (etcd +
  Syncthing), no Tailscale/Headscale (Nebula).
- No i18n — en-US only, in-envelope decision (SUPPORT.md).
