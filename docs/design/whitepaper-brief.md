# Instructions for AI: Create a Graphical One-Page Whitepaper for Magic Mesh (MCNF)

Use the following project understanding as your source material. **Do not fabricate details beyond what is provided below.** Produce a single-page, visually structured, graphical whitepaper suitable as a PDF or poster (infographic/architectural poster style).

---

## WHAT TO PRODUCE

A one-page graphical whitepaper (dense infographic / architectural poster) that explains what **Magic Mesh (MCNF)** is, what it does, and who it's for. It must be visually scannable with clear hierarchy: headers, callout boxes, a layered architecture diagram as the centerpiece, and icon-labeled capability grids.

---

## PLATFORM IDENTITY

**Full name:** MCNF — Mackes Cosmic Nebula Fedora
**Brand:** Magic Mesh
**Version:** 12.0 "Quazar" series
**One-line summary:** A secure, no-fixed-center workgroup mesh that turns up to 12 machines into one private, encrypted workgroup — with no central server.

**Core model:** Any node can author fleet policy. Every node enforces it itself. The whole system runs over a [Nebula](https://github.com/slackhq/nebula) encrypted overlay (Ed25519 identity, AES-256-GCM wire encryption) with etcd coordination and Syncthing-backed mesh file storage.

**Desktop surface:** egui-native — owns DRM/KMS directly (no X11, no Wayland compositor). IBM Carbon design language (carbondesignsystem.com) throughout, single-sourced in `mde-theme`, lint-gated (no raw hex colors permitted).

**Stack:** Pure Rust, 23+ crates, one workspace. GPL-3.0-or-later. Solo project by Matthew Mackes. Repository: github.com/matthewmackes/magic-mesh

---

## LAYERED ARCHITECTURE (render as a vertical stack — this is the centerpiece)

Label from bottom to top:

**1. SUBSTRATE** (tag: "Locked Foundation §1–§3")
- Nebula encrypted overlay — the wire (Ed25519 identity, AES-256-GCM tunnels)
- etcd — distributed coordination + leader election
- Syncthing — replicated mesh file storage
- rustls / ring — all TLS; **zero OpenSSL** (cargo-deny-banned, §3)

**2. CONTROL PLANE** (tag: "~50 role-gated workers")
- `mackesd` — supervised systemd daemon: Nebula CA, enrollment, scanning, reconcile loop, healthz/metrics/alerts, leader election, audit trail, breaker monitoring
- `magic-fleet` — desired-state automation engine (ansible-backed): any node authors fleet revisions, every peer self-converges, replicated revision log with diff/rollback
- `meshctl` — operator CLI facade (~70 subcommands)

**3. IPC PLANE** (tag: "mde-bus")
- File-backed pub/sub + RPC: `action/<prefix>/<verb>` → `reply/<ulid>`
- D-Bus reserved exclusively for freedesktop.org interop (org.freedesktop.*, org.mpris.*)
- Mesh-native: ntfy-backed, topic-based, SQLite-indexed, mDNS-discoverable

**4. LOOK STACK** (tag: "IBM Carbon §4")
- Strictly IBM Carbon Design System (carbondesignsystem.com)
- Gray 10 / Gray 90 / Gray 100 token palette
- Single-sourced in `mde-theme` crate (lint-gated; no raw hex anywhere)
- Typography, spacing, radii, shadows, density — all tokenized

**5. DESKTOP SURFACE** (tag: "egui · DRM-native")
- ONE egui shell owns the DRM/KMS seat directly (libinput pump + page-flip loop via smithay DRM/GBM runner)
- Workbench — 5-plane operator console: Controller · Compute · Connect · Provisioning · Front Door
- mde-files — mesh-first file manager with Send-to-peer
- mde-voice-hud — SIP softphone (layer-shell overlay, 3×4 keypad, live registration state)
- mde-music — Airsonic/Subsonic client + PipeWire daemon (MPRIS)
- mde-cosmic-applet — mesh-health pip in the panel (Healthy/Degraded/Down)
- mde-enroll — ratatui TUI for zero-friction mesh join (works headless over SSH)

Below the stack, add a tagline: **"No fixed center: every node is author + enforcer + relay-eligible"**

---

## DEPLOYMENT ROLES (render as two side-by-side cards, nesting arrow between them)

| Role | Adds | Typical host |
|---|---|---|
| **Lighthouse** | Nebula relay + CA authority + leader election + health/scan/observability control plane | Cloud VPS (headless) |
| **Workstation** | Everything above + egui DRM-native desktop + all GUIs + libvirt/QEMU-KVM/Podman VM+container host (OpenStack-orchestrated) | Daily-driver laptop or server |

Key fact: **One signed RPM serves both roles.** A headless machine is a Workstation without a local display (full daemon stack, no egui seat — serves VMs/containers to the mesh). Role = a config flag, not a build — a box is re-roleable without reinstall.

---

## CAPABILITY GRID (render as 5 labeled icon-tiles or boxes, arranged 3 + 2)

**FLEET CONTROLS** (icon: gears/sliders)
- Desired-state revisions — any node authors, every peer self-converges
- Drift detection + auto-remediation (hash-chained audit events)
- Jobs & playbooks — ansible-backed, tag/role/peer target selectors, optional cron
- Node lifecycle: role-pin (upgrade-only), upgrade, decommission (cert-revoke + evict), Wake-on-LAN, --except exceptions
- No push-SSH, no central controller, no SPOF

**SERVICES** (icon: grid/apps)
- File sharing: Send-to-peer over replicated volume, native tar/gz browse + extract
- Voice: SIP REGISTER/INVITE/RTP softphone with latency-aware dispatcher priority (Kamailio + RTPengine)
- Music: Airsonic/Subsonic client + daemon (FLAC/MP3/Vorbis/AAC/Opus, MPRIS, internet radio)
- Device sync: native KDE Connect host (pair, ring, clipboard, SMS, notifications) — RSA-4096, AES-256-GCM sessions
- Remote access: unified SSH + RDP + VNC status/launcher per peer
- Compute: Podman + libvirt/QEMU-KVM via OpenStack (Nova), Glance image catalog + Cloud-plane wizard
- Name discovery: `.mesh` DNS domain, cross-segment mDNS relay, peer service directory

**SECURITY** (icon: shield/lock)
- Ed25519 node identity, token-scoped CSR enrollment (v3 join tokens with fingerprint pinning)
- CA lifecycle: mint → rotate (epoch bump + auto re-sign) → real revocation (revoke/ban/blocklist)
- Crypto floor: AES-256-GCM / ChaCha20-Poly1305 session, RSA-4096 KDC identity
- rustls everywhere — **no OpenSSL, anywhere** (cargo-deny-banned, supply-chain SBOM)
- Secrets hygiene: systemd-creds or --*-stdin (never argv), daemon scrubs env at boot
- Hash-chained tamper-evident audit trail (audit-log / audit-verify)
- Overlay-confined firewalld presets, additive sshd listener (public listener always kept)

**OBSERVABILITY** (icon: eye/graph)
- `healthz` — live node-health buckets, worker liveness, audit-chain status, ready verdict
- Prometheus textfile exporter — 10+ gauge families written every 30s (nodes, cert, workers, breakers, disk, backup)
- Alert hooks — configurable `[[alert_hooks]]` (event JSON on stdin) + severity-mapped journal alerts
- `meshctl doctor` / `fleet status` / `test {connectivity,dns,firewall}`
- Workbench Health + Logs/metrics panels

**ONBOARDING & SCANNING** (icon: magnifying glass/radar)
- Magic onboarding: `mackesd found` (lighthouse) + `mackesd join '<token>'` (peer)
- Fingerprint-pinned TLS enrollment endpoint — no pre-shared filesystem required; works for NAT'd/remote peers
- Full-screen ratatui enrollment TUI (headless over SSH)
- nmap inventory probes: two-tier cadence (fast liveness → periodic deep -sV/NSE)
- Network-security scanners: ARP-spoof, evil-twin AP, rogue-DHCP, captive-portal, DNS-leak detection
- Surrounding-networks survey with trust ledger

---

## THREE AUDIENCE PERSONAS (render as horizontal cards near the bottom)

**The Solo Operator / Homelabber**
Runs their own infra across a handful of machines (laptop, NAS, cloud VPS). Wants one private encrypted fabric without a cloud dependency or management-plane subscription. Gets fleet automation, mesh storage, device sync, and observability — all self-hosted, all under their control. No SaaS, no license server, no phone-home.

**The Small Workgroup (≤8 peers, flat trust)**
A family office, research lab, or boutique firm that needs shared files, voice calling, device pairing, and remote access across physical sites — without standing up Active Directory, a VPN concentrator, or a fleet-management SaaS. Any peer can administer; there is no root account to compromise. The trust model is explicit: flat trust among peers who already know each other.

**The VDI / Thin-Client Operator**
Runs VM desktops brokered through the mesh: libvirt/QEMU-KVM guests placed by OpenStack Nova, reached via SPICE into the egui shell, or remote RDP (ironrdp, primary) / VNC (fallback) desktops over Nebula. The host runs no native apps — browser, office, and games live inside VM guests. Sessions roam per-peer (etcd/Syncthing state). Ships as an immutable bootc image. USB passthrough and multi-monitor in scope.

---

## KEY ARCHITECTURAL PRINCIPLES (render as a sidebar or footer callout strip)

- **No fixed center** — any node authors fleet policy; peers gossip + self-converge. No controller, no SPOF.
- **Bus, not D-Bus** — surfaces and daemon talk over `mde-bus`; freedesktop.org interop only.
- **Maximum crypto by lock** — Ed25519, AES-256-GCM, ChaCha20-Poly1305, RSA-4096; rustls; zero OpenSSL (§3).
- **Flat trust, documented** — designed for ≤8 peers who already trust each other. Not zero-trust; accepted and documented trade-off (§8).
- **Boundary CI-gated** — no control-plane crate may depend on a desktop-shell crate (lint-enforced).
- **DRM-native** — egui owns the KMS seat directly. No X11, no Wayland compositor. VDI brokering is a capability, not the identity.
- **Role = config flag** — one RPM, one stack; Lighthouse or Workstation chosen at install, changeable without reinstall.

---

## VISUAL GUIDANCE

- **Color palette:** Deep grays from IBM Carbon — background #161616 (Gray 100), cards #262626 (Gray 90), borders #393939 (Gray 80). Accent: teal/cyan (#009D9A or similar) for headers, callout borders, and version badges. Use semantic color sparingly: green for "Healthy/secure," amber for "Degraded," red only for "Down/danger."
- **Architecture diagram** is the centerpiece: a 5-layer vertical stack rendered as nested rounded rectangles or horizontal bands, each labeled with its layer name on the left and its key components on the right. Role badges ("Lighthouse," "Workstation") beside layers that apply.
- **Capability grid:** 5 labeled boxes (2×3 grid with one merged or centered row), each with a prominent icon, title, and 3-5 bullet points. Flank the architecture diagram (2 left, 3 right, or top-and-bottom).
- **Audience personas:** 3 horizontal cards near the bottom, each with a subtle icon/avatar area, a bold persona title, and 2-3 sentences of description.
- **Typography:** IBM Plex Sans for headers and body; IBM Plex Mono for code references, CLI commands, and crate names. Title: ~32pt bold. Section headers: ~18pt. Body: ~10-11pt. Code: ~9pt.
- **Header area:** "MCNF" in large type, "Magic Mesh" as subtitle, "v12.0 Quazar" as a pill/badge. One-line description immediately below.
- **Footer:** GPL-3.0-or-later | github.com/matthewmackes/magic-mesh | Built in Rust | 23+ crates | ~215K lines
- **Layout:** Portrait (US Letter or A4). Single page. Dense but not crowded — use whitespace deliberately between sections. No scroll — everything fits one page.

---

## DO NOT INCLUDE

- Build farm details, CI pipeline, or development infrastructure
- Specific IP addresses, hostnames, or credentials (these are private operator infra)
- Historical epics (ONBOARD-#, EFF-#, AUDIT-# identifiers)
- Git branches or commit hashes
- Memory-file provenance or Claude Code tooling
- Any mention of the operator by name beyond the author credit and repo URL
- The LizardFS-to-etcd migration (internal detail, not user-facing)
- The XCP-ng build farm (dev infrastructure, not the product)
