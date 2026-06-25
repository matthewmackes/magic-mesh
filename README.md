<p align="center">
  <img src="assets/brand/logo-lockup.png" alt="MCNF — Mackes Cosmic Nebula Fedora, Network Mesh Platform" width="340">
</p>

# MCNF

**A secure, no-fixed-center workgroup mesh — and its native-Rust IBM-Carbon
GUIs — on stock Fedora-Cosmic.**

MCNF turns up to **eight machines** into one private, encrypted workgroup
with no central server: any node can author fleet policy, every node enforces it
itself, and the whole thing runs over a [Nebula](https://github.com/slackhq/nebula)
overlay with [etcd](https://etcd.io/) for coordination and
[Syncthing](https://syncthing.net/) replicating the shared disk. The desktop is
[COSMIC](https://github.com/pop-os/cosmic-epoch) — MCNF ships everything
Cosmic *doesn't* give you (the mesh, fleet automation, storage, telephony, device
sync, security scanning, observability) as Cosmic apps + applets.

Split out of the [MackesWorkstation](https://github.com/matthewmackes/MackesWorkstation)
monorepo (the labwc/Windows-era *MackesDE* desktop, now end-of-life) by the
**E11 "MCNF" pivot**.

---

## Layers of abstraction

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  COSMIC DESKTOP  (Cosmic owns panel · settings · lock · greeter · WM)          │
│                                                                                │
│   Apps & applets (Cosmic-native, libcosmic):                                   │
│     mde-workbench · mde-files · mde-music · mde-voice-hud                       │
│     mde-cosmic-applet (panel) · mde-role-chooser (first-run)                   │
└───────────────┬────────────────────────────────────────────────────────────── ┘
                │ renders through
┌───────────────▼──────────────────────────────────────────────────────────────┐
│  LOOK STACK  —  strict IBM Carbon, single-sourced                              │
│     mde-theme (Gray 10/90/100 tokens) · mde-iced-components · mde-card          │
└───────────────┬────────────────────────────────────────────────────────────── ┘
                │ talks over
┌───────────────▼──────────────────────────────────────────────────────────────┐
│  IPC PLANE  —  mde-bus  (file-backed pub/sub + RPC)                            │
│     action/<prefix>/<verb> → reply/<ulid>   ·   D-Bus only for FDO interop      │
└───────────────┬────────────────────────────────────────────────────────────── ┘
                │ served by
┌───────────────▼──────────────────────────────────────────────────────────────┐
│  CONTROL PLANE                                                                 │
│     mackesd      — supervised daemon: ~50 role-gated workers, reconcile loop,   │
│                    Nebula CA, enrollment, scanning, healthz/metrics/alerts      │
│     magic-fleet  — desired-state engine (ansible-backed), replicated revisions │
│     meshctl      — the friendly operator CLI facade                            │
└───────────────┬────────────────────────────────────────────────────────────── ┘
                │ rides
┌───────────────▼──────────────────────────────────────────────────────────────┐
│  SUBSTRATE  (the locked foundation — §1–§3)                                    │
│     Nebula encrypted overlay   — the wire (Ed25519 identity, AES-256-GCM)      │
│     etcd + Syncthing           — coordination + the replicated disk            │
│     rustls / ring               — all TLS; no OpenSSL, anywhere                 │
└────────────────────────────────────────────────────────────────────────────── ┘

         no fixed center: every node is author + enforcer + relay-eligible
```

The **mesh/desktop boundary** is gated in CI — no control-plane crate may depend
on a desktop-shell crate. Everything above the IPC plane is replaceable UI;
everything below it runs headless on a Lighthouse with no desktop at all.

---

## Functionality

Deployment roles nest by capability — **Lighthouse ⊂ Server ⊂ Workstation** —
and one signed RPM serves all three; the install-time chooser decides what runs.

| Role | Adds | Typical host |
|---|---|---|
| **Lighthouse** | Nebula relay + CA + leader + health/scan/observability control plane | a VPS, headless |
| **Server** | + fleet automation + a Syncthing storage replica + jobs | a NAS / always-on box |
| **Workstation** | + the COSMIC desktop and every GUI | a daily-driver laptop |

### Fleet controls

No-fixed-center desired-state automation (`magic-fleet` + the `fleet`/`jobs` Bus
surfaces, driven from the Workbench **Controller** plane):

- **Revisions** — any node authors a desired-state baseline (`push-revision`);
  it lands in the Syncthing-replicated revision log and every peer **elects the
  head and converges itself** (`reconcile`) — no push-SSH, no controller.
  `list-revisions` · `diff-revisions` · `rollback` · `nudge`.
- **Drift detection + remediation** — the reconcile loop diffs desired vs.
  observed topology each tick, records drift as hash-chained audit events, and
  (12.14+) repairs it; `remediate` / `repair` drive it on demand.
- **Jobs & playbooks** — saved ansible-backed job templates with tag/role/peer
  target selectors + optional cron schedules; run history with per-target
  results.
- **Node lifecycle** — `role-pin` (upgrade-only, fail-closed) · `role-gate` /
  `role-workers` · `take-leadership` · `wake-peer` (WoL) · `upgrade` ·
  `decommission` (cert-revoke + evict) · node-local `--except` exceptions.

### Services

What the mesh actually *does* for its users — each reachable from a Workbench
panel and/or a Cosmic app:

- **File sharing** — `mde-files`: a Cosmic file manager with **Send-to-peer**
  over the replicated volume (Syncthing replication *is* the transport; sources
  confined to the operator's share root), native tar/gz browse + extract.
- **Telephony / voice** — `mde-voice-hud`: SIP REGISTER/INVITE/RTP softphone;
  mesh-internal extensions auto-configured (Kamailio + RTPengine units,
  rendered from replicated policy) with **latency-aware dispatcher priority**.
- **Music** — `mde-music` + `mde-musicd`: an Airsonic/Subsonic client + daemon
  (FLAC/MP3/Vorbis/AAC/Opus decode, MPRIS, internet radio, mesh hand-off).
- **Device sync** — `mde-kdc-host`: a native KDE-Connect host (pair, ring,
  notifications, clipboard, file send) over mutual-TLS.
- **Remote access** — unified SSH + RDP + VNC status/launcher per peer.
- **Printing** — mesh-wide CUPS queue discovery + sharing.
- **Compute** — Podman + KVM provisioning, an image catalog, and a VM wizard
  (the Workbench **Compute** plane).
- **Name & service discovery** — a `.mesh` DNS domain, cross-segment mDNS relay,
  and a peer service directory ("the Front Door").

### Scanning

Two distinct scan engines on the control plane:

- **Inventory probe** (`probe` / `scan`) — `nmap` is the sole engine: a two-tier
  cadence (fast liveness + curated ports → periodic deep `-sV`/NSE), parsed into
  per-host **service descriptors** and replicated for the peer directory.
- **Network-security scanners** (`netassess`) — active LAN/Wi-Fi threat checks:
  **ARP-spoof**, **evil-twin AP**, **rogue-DHCP**, **captive-portal**,
  **DNS-leak**, plus a **surrounding-networks** survey with a trust ledger.
  Findings are recorded (`record-attack`) and surfaced as alerts.

### Security

Maximum-crypto by lock (`AI_GOVERNANCE.md` §3); the trust model is flat trust
among ≤8 peers, an accepted, documented trade-off (see `DISCLAIMER.md`):

- **Identity & enrollment** — Ed25519 node identity; token-scoped CSR enrollment
  (`enroll-token` → `enroll` → CA sign under the active epoch); `reenroll`.
- **CA lifecycle** — mint / **rotate** (epoch bump + auto re-sign) / `sign-csr` /
  encrypted off-cluster `export`+`import`; **real revocation** (`revoke` / `ban`
  / blocklist — Nebula refuses revoked tunnels).
- **Crypto floor** — AES-256-GCM / ChaCha20-Poly1305 session, RSA-4096 KDC
  identity, rustls everywhere; **no OpenSSL** (cargo-deny-banned).
- **Secrets hygiene** — passphrases/passcodes read via systemd-creds or
  `--*-stdin`, never argv/inherited-env; the daemon scrubs its env at boot.
- **Hardening** — overlay-confined firewalld presets, an additive sshd
  overlay listener (the public listener is always kept),
  encrypted daily state backup + verifiable restore, a root-by-design daemon
  with a dropped capability set, and supply-chain gating (`cargo deny` +
  CycloneDX SBOM).

### Reporting & observability

- **`healthz`** — live node-health buckets, audit-chain status, worker liveness,
  and a `ready` verdict (store view from the CLI; full live view on the Bus).
- **Prometheus textfile exporter** — a control-plane gauge set written every
  30 s for a node_exporter textfile collector:
  `mackesd_mesh_nodes_{total,healthy,degraded,unreachable}` ·
  `mackesd_audit_chain_intact` · `mackesd_ca_cert_days_remaining` ·
  `mackesd_workers_{alive,total}` · `mackesd_breaker_tripped` ·
  `mackesd_disk_available_bytes` · `mackesd_backup_{passphrase_set,age_seconds}`
  · the router decision-time histogram.
- **Audit trail** — a hash-chained, tamper-evident event log (`audit-log` /
  `audit-verify`); every config change, auth event, and path switch is recorded.
- **Alerting** — configurable `[[alert_hooks]]` (event JSON on stdin; wire your
  own pager) **plus** severity-mapped journal alerts (`mackesd::alert`) as the
  headless surface; continuous disk-headroom, breaker-trip, cert-expiry, and
  backup-staleness alerts.
- **Operator surfaces** — `meshctl doctor` / `fleet status` / `test
  {connectivity,dns,firewall}`, and the Workbench **Health** + **Logs/metrics**
  panels.

---

## What's here

| Group | Crates | Role |
|---|---|---|
| `platform` | `mde-bus`, `mde-role`, `mde-cosmic-applet`, `mde-role-chooser` | pub/sub backbone · deployment-role gating · cosmic-panel mesh-health applet · first-run role chooser |
| `mesh` | `mackesd` (+ `meshctl`), `mackes-{config,mesh-types,nebula-https-tunnel,transport}`, `magic-fleet` | the supervised control-plane daemon, Nebula overlay + TCP/443 covert tunnel, transport/types/config, and the no-fixed-center Automation Mesh engine |
| `services` | `mde-files`, `mde-voice-{hud,config}`, `mde-music`, `mde-musicd` | file manager · voice/SIP HUD + config · music player + daemon |
| `workbench` | `mde-workbench` | the COSMIC **control surface** (dashboard, fleet, devices, compute, mesh health, logs) |
| `shared` | `mde-theme`, `mde-iced-components`, `mde-card`, `mde-disclaimer` | the **Carbon** look stack + the runtime accept gate |
| `kdc` | `mde-kdc-host`, `mde-kdc-proto` | the canonical KDE-Connect host + wire protocol |

22 crates, one workspace; full map in [`docs/architecture.md`](docs/architecture.md).

## Architecture locks

The load-bearing identity (full detail in [`AI_GOVERNANCE.md`](AI_GOVERNANCE.md)):

- **Mesh:** Nebula encrypted overlay · **no fixed center** (any node authors
  fleet revisions; peers gossip + self-converge) · etcd + Syncthing mesh
  substrate.
- **Bus, not D-Bus:** surfaces and `mackesd` talk over `mde-bus`; FDO interop
  (`org.freedesktop.*`, `org.mpris.*`) only.
- **Security:** maximum crypto — Ed25519 / AES-256-GCM / ChaCha20-Poly1305 /
  RSA-4096 KDC identity; rustls, never OpenSSL.
- **Look:** strictly **IBM Carbon** (carbondesignsystem.com), Gray 10/90/100,
  single-sourced in `mde-theme` (lint-gated). Pure-Rust stack.
- **Boundary:** no mesh-side crate depends on a desktop-shell crate (gated).
- **Envelope:** designed for a **≤8-peer flat-trust** workgroup (§8) — not a
  zero-trust / hyperscale product.

## Build

```sh
cargo build --workspace        # needs alsa-lib-devel (the audio chain links ALSA)
cargo test
cargo clippy --all-targets
cargo fmt --all
```

> On the EL9 dev host, gcc 11.5 rejects `mold`, so build with
> `RUSTFLAGS="-C link-arg=-fuse-ld=gold"`, and `opus-devel` comes from CRB.

The **canonical build environment** (the dev host, the IaC-managed build farm,
reproduce-from-scratch, and every gotcha) is
[`docs/BUILD-ENVIRONMENT.md`](docs/BUILD-ENVIRONMENT.md). Prerequisites, the
serial-mackesd test rule, and the lint/deny/coverage gates:
[`CONTRIBUTING.md`](CONTRIBUTING.md). Packaging is one signed `magic-mesh` RPM
(`cargo generate-rpm`) with an install-time role chooser; cut via `/release`.

## Documentation

| For | Read |
|---|---|
| Building / the dev environment | [`docs/BUILD-ENVIRONMENT.md`](docs/BUILD-ENVIRONMENT.md) |
| The build farm + IaC | [`docs/farm.md`](docs/farm.md) · [`infra/`](infra/) |
| Understanding the system | [`docs/architecture.md`](docs/architecture.md) |
| Running a mesh, day-2 | [`ADMIN.md`](ADMIN.md) |
| Installing | [`docs/help/install.md`](docs/help/install.md) |
| Per-role setup | [`docs/help/node-setup.md`](docs/help/node-setup.md) |
| When it breaks | [`docs/help/troubleshooting.md`](docs/help/troubleshooting.md) |
| Losing a lighthouse | [`docs/help/mesh-recovery.md`](docs/help/mesh-recovery.md) |
| Contributing | [`CONTRIBUTING.md`](CONTRIBUTING.md) |
| What's supported | [`SUPPORT.md`](SUPPORT.md) |
| The rules of the repo | [`AI_GOVERNANCE.md`](AI_GOVERNANCE.md) |

GPL-3.0-or-later. See [`DISCLAIMER.md`](DISCLAIMER.md).
</content>
</invoke>
