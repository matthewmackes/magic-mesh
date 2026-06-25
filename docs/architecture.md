# MCNF — Architecture

A secure, **no-fixed-center** workgroup mesh (≤8 peers, §8 trust envelope) and
the Cosmic-native apps that run on top of it. This is the system map; the
binding rules live in [`AI_GOVERNANCE.md`](../AI_GOVERNANCE.md), the operator
view in [`ADMIN.md`](../ADMIN.md) + [`docs/help/`](help/).

## The four planes

```
┌─ Desktop plane (Cosmic owns the shell) ──────────────────────────────┐
│  mde-workbench · mde-files · mde-music(+d) · mde-voice-hud           │
│  mde-cosmic-applet (panel) · mde-role-chooser (first-run)            │
│  mde-mesh-wallpaper (layer-shell live map)                           │
│  Look: IBM Carbon, single-sourced in mde-theme (§4)                  │
├─ IPC plane ──────────────────────────────────────────────────────────┤
│  mde-bus: file-backed pub/sub + RPC (action/<p>/<verb> → reply/<ulid>)│
│  D-Bus only for FDO interop (notifications, MPRIS) — §2              │
├─ Control plane ──────────────────────────────────────────────────────┤
│  mackesd: SQLite store · worker supervisor (ENT-6 breaker) ·          │
│  reconcile loop · Nebula CA · enrollment · healthz/metrics/alerts    │
│  magic-fleet: desired-state engine (ansible-backed), revision log    │
├─ Substrate (§1–§3 locks) ────────────────────────────────────────────┤
│  Nebula overlay (Ed25519 identity, AES-256-GCM/ChaCha20) — the wire  │
│  etcd (coordination) + Syncthing (files, "/mnt/mesh-storage") — §1   │
│  rustls everywhere; RSA-4096 own KDC identity                        │
└──────────────────────────────────────────────────────────────────────┘
```

**No fixed center:** any node can author fleet revisions; peers replicate
them over Syncthing and each node elects + applies the head itself
(`magic-fleet reconcile`, FPG-8). The Lighthouse is a *relay + CA*, not a
controller — losing it is a recoverable event
([mesh-recovery](help/mesh-recovery.md)), not a head decapitation.

## Crates (22)

| Group | Crates | Role |
|---|---|---|
| platform | `mde-bus`, `mde-role`, `mde-cosmic-applet`, `mde-role-chooser` | pub/sub backbone · role gating · panel applet · first-run chooser |
| mesh | `mackesd` (+`meshctl` bin), `mackes-config`, `mackes-mesh-types`, `mackes-nebula-https-tunnel`, `mackes-transport`, `magic-fleet` | control-plane daemon · config · shared types · TCP/443 covert tunnel · transport scoring · fleet engine |
| services | `mde-files`, `mde-voice-hud`, `mde-voice-config`, `mde-music`, `mde-musicd` | file manager · SIP HUD · voice config gen · music GUI · music daemon |
| workbench | `mde-workbench` | the operator control surface (fleet, devices, health, logs) |
| shared | `mde-theme`, `mde-iced-components`, `mde-card`, `mde-disclaimer` | Carbon tokens (single source, §4) · iced widgets · object-card model · accept gate |
| kdc | `mde-kdc-host`, `mde-kdc-proto` | KDE Connect host (phones) · wire protocol |

## Key mechanisms

**Bus RPC.** A caller writes `action/<prefix>/<verb>` with a JSON body;
the responder polls (`list_since` + cursor), replies on `reply/<ulid>`.
Bodies are capped (64 KiB) before parse; reply/action topics reap on a 1 h
ephemeral TTL; `audit/*` is retention-forever. Responders catch panics and
answer error envelopes (the thread never dies).

**Worker supervisor.** `mackesd serve` spawns ~30 workers gated by the
pinned role (Lighthouse ⊂ Server ⊂ Workstation, `worker_role::WORKER_TIERS`).
Restart policy with exponential back-off + a circuit breaker (ENT-6); panics
are caught and fed through the same path (EFF-4). A live `WorkerStatusMap`
feeds the `ready` verdict on healthz and the exporter's gauges (EFF-24/26).

**Mesh routing.** `mesh_router` ticks 10 s: HTTPS-fallback activation
(UDP-failure threshold → TCP/443 TLS tunnel), then the KDC2-1.9 scorer picks
primary/fallback per peer from the transport registry under the operator
policy (`/etc/mde/connect/policy.toml`) — including the CV-1 encryption
floor: content classes (clipboard/file/SMS) never ride a transport below
AES-256-class. Every path flip is a hash-chained audit event.

**File transfer.** Send-To copies into `<qnm>/inbox/<peer>/<sender>/` —
Syncthing replication *is* the wire; the receiving Inbox lists its directory.
Sources are confined to the operator share root (symlink-escape refused,
EFF-2).

**Fleet.** A revision (YAML baseline + version) lands in the replicated
revision log; every node's `fleet_reconcile` worker shells
`magic-fleet reconcile`, which elects the newest head, converges host-local
(no push-SSH), and writes an apply-ack the author's FSM reads.

**Observability.** `healthz` (CLI = store view; Bus = + live workers +
`ready`), the Prometheus textfile exporter (node health, CA-cert
days-remaining, router decision histogram, worker/breaker, disk headroom,
backup posture), hash-chained audit events with configurable
`[[alert_hooks]]` (event JSON on stdin), and severity-mapped journal alerts
(`target: mackesd::alert`) as the headless surface.

**Security posture.** Enrollment: token (`--mesh-id`-scoped) → CSR → CA
sign under the active epoch; revocation is real (blocklist fingerprints,
nebula refuses tunnels). CA rotation bumps the epoch and re-signs peers.
Daily encrypted state backup (`MDE_BACKUP_PASSPHRASE`, XChaCha20-Poly1305 +
Argon2id) to the replicated volume; restore via
`mackesd state-restore <bundle>`. The trust model (flat trust, ≤8 peers) is
an accepted, documented trade-off — [`DISCLAIMER.md`](../DISCLAIMER.md).

## What is deliberately NOT here

- No desktop shell — Cosmic owns panel/lock/greeter/settings (E11 pivot).
- No `mde <subcommand>` dispatcher — separate binaries.
- No central server, no SaaS, no telemetry egress.
- No OpenSSL (rustls; `cargo deny` bans it), no Gluster/LizardFS/Ceph
  (etcd + Syncthing), no Tailscale/Headscale (Nebula).
- No i18n — en-US only, in-envelope decision (SUPPORT.md).
