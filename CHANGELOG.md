# Changelog

All notable changes to MCNF (magic-mesh) are documented here. The **11.4.x → 12.0** line is the **E12 "Quasar" pivot** to the egui-native, DRM-native thin-client VDI desktop (the libcosmic/iced + strict-IBM-Carbon stack retired); the 12.x codename spelling ("Quazar" vs "Quasar") is a pending operator decision — see [`docs/NEEDS-OPERATOR.md`](docs/NEEDS-OPERATOR.md). The 10.0.x series is codenamed "Magic Mesh"; historical entries below predate the 2026-06-17 rebrand. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning is the
single workspace version (`[workspace.package] version`, every crate
inherits). Release tags are **`magic-mesh-v<version>`**; the RPM NEVRA pairs
that version with the packaging `release` field (bumped alone for
asset-only changes).

Pre-release history (the E1–E11 epochs, the MackesWorkstation split, and the
v2.x–v6.x phase plans) lives in the git log and `docs/design/` — this file
starts at the first packaged release line.

## [Unreleased]

_Accumulating since the `magic-mesh-v12.0.0` tag (2026-07-02) toward the next
operator-gated cut — a broad, still-open 12.0.x wave that fills out the Quasar
desktop. It spans a full **native application suite** (terminal, editor, media
player, files, dual-engine browser), an OpenStack-based **cloud + VDI plane** with
multi-protocol remote desktops, a Windows-style **desktop shell**, a mesh-wide
**discovery + device** surface, **KDE-Connect-over-overlay** phone integration, the
**QBRAND** identity, and a resilience/perf/security/CI hardening pass. The pillars
below are grouped from the 12.0.x epic streams (TERM/TMUX, EDITOR, MEDIA, FILEMGR,
BOOKMARKS/web, QC "QUASAR-CLOUD"/IAC/CHOOSER, VDI, NAVBAR/WIN7/VDOCK/SETTINGS/
CURTAIN, EXPLORER, DEVMGR, KDC-MESH, QBRAND, POWER); this section will collapse
into the version entry at the next cut._

### Added
- **Native application suite — the surfaces that make it a desktop.**
  - **Terminal (`mde-term-egui`, TERM-1..16)** — an `alacritty_terminal`-core VT
    engine with scrollback, nested split panes + tabs, SGR/1006 mouse + true-color
    + ligatures, scrollback search, broadcast/grouped input, mesh-synced saved
    layouts, and a `mackesd` mesh PTY broker for remote shells (persistence +
    reattach + idle reap); mounted as `Surface::Terminal`. A **tmux control-mode**
    integration (`tmux -CC`, TMUX-FC-1..8) layers native session/window/pane chrome,
    layout presets, and persistence over the Bus PTY broker.
  - **Editor (`mde-editor-egui`, EDITOR/EDTB/EDITOR-LSP/EDITOR-COLLAB)** — a `ropey`
    buffer + custom egui text widget, tree-sitter highlighting, tabs/splits,
    multi-cursor, a fuzzy finder + command palette, a project tree, an LSP client
    (goto-definition / find-references / rename / format), mesh collaborative
    share-sessions + follow mode, a Word-97-style menu bar + toolbars, hunspell
    spell-check, code folding + symbol outline, an integrated terminal dock, and
    CUPS print.
  - **Media player (`mde-media-core` libmpv + `mde-media-egui`, MEDIA-1..18)** — a
    DRM overlay video plane, PipeWire audio with an EQ/loudness/ReplayGain/gapless
    af-graph, VA-API hwdec, subtitles + multi-track, playlists + a local library
    index + resume, network streams (yt-dlp) + v4l2 capture, a `mde-jellyfin`
    client with offline cache, a `mackesd` mesh media server (DLNA/UPnP discovery +
    source aggregation), playback session roaming, and sync-play party mode + cast;
    mounted as `Surface::Media`.
  - **Files (`mde-files-egui`, FILEMGR-1..12)** — a real FileOps backend with an
    async op queue + conflict engine, operation dialogs, a preview pane +
    thumbnails, archive create/extract/browse, a mesh sidebar tree over a
    `mackesd` sshfs overlay-mount worker, direct P2P transfer routing, and
    Send-To / Send-in-Chat / shared-clipboard integration.
  - **Browser (`mde-web-cef` Chromium/CEF + the sandboxed `mde-web-preview` Servo
    helper, `mde-adblock`, the `mde-bookmarks` CRDT, BOOKMARKS-1..10)** — a CRDT
    bookmark tree synced over Syncthing with Firefox/Chromium/Netscape importers, an
    out-of-process OS-sandboxed Servo helper (shm texture bridge), a mesh-wide
    ad-filter engine + fleet-enforced browser policy worker, send-tab sync +
    session restore + speed dial, and mesh integration (Send-in-Chat, add-from-page).
- **BOOKMARKS-9 — the Servo browser packaged + documented (ships securely).**
  - The `mde-web-preview` Servo helper is a first-class RPM asset
    (`/usr/bin/mde-web-preview`) in the base (Workstation) `magic-mesh` package,
    so it is present in the base bootc image; the BOOKMARKS-8 `browser_policy`
    worker runtime-gates whether it may spawn. Not in the headless
    `magic-mesh-server` variant.
  - The RPM declares Servo's **runtime libs as hard Requires** — mesa (EGL/GL/
    GLES/gbm/DRI), Vulkan (`mesa-vulkan-drivers` + `vulkan-loader`), and the font
    stack (`fontconfig`/`freetype`/`harfbuzz`) + `libxkbcommon`. **Deliberately no
    `firefox`/`gecko`/`nss`/`nspr`** — Servo is self-contained (rustls system-CA
    TLS).
  - The ad-filter **seed lists** ship loose at `/usr/share/magic-mesh/adblock/`,
    `include_str!`'d into the `mde-adblock` engine from
    `crates/services/mde-adblock/seed/*.txt` (one source, no drift).
  - A confined **enforcing SELinux domain** for the helper
    (`mde_web_preview_t`, `packaging/selinux/mde-web-preview.te`/`.fc`) ships and
    is compiled + loaded by the RPM `%post`
    (`setup-selinux-web-preview.sh`); least-privilege, default-deny to the
    operator's home/keys/mesh data. It is defense-in-depth over the helper's
    in-process OS sandbox and self-skips where SELinux is disabled (the platform
    standard) — never a permissive stub.
  - New **`docs/THREAT_MODEL.md`** — the browser attack surface, the sandbox +
    SELinux confinement layers, and the accepted residual risks (unrestricted
    egress, Servo fidelity, monthly-tracked churn).
- **Cloud + VDI plane — the OpenStack "QUASAR-CLOUD" cutover (QC-2..QC-23, IAC-1..5,
  CHOOSER-1..9).** A `mackesd` `openstack` supervision worker renders + seals Kolla
  config from a live doctrine reader over an airgapped image lane, standing up real
  Neutron/OVN flat-mesh networking, Cinder LVM + object-tier backup, a Glance local
  store + DIB image pipeline, capacity-derived flavors + per-user quotas, and typed
  `action/cloud/*` Bus verbs. The shell's **Cloud plane** replaces the old Controller
  plane; a **Desktop Chooser** (node-grouped card grid, always-ask protocol picker,
  live preview thumbnails, mesh-identity SSO + sealed external creds) fronts it, and
  **QC-14** places brokered desktops as Nova instances. An **Infra-as-Code** surface
  (Overview/Resources/Heat tabs; Heat/Horizon/Octavia/Designate) rides the same
  OpenStack client.
- **Multi-protocol remote desktops.** First-class **Spice** (`mde-vdi-spice`,
  CHOOSER-5), **VNC** with RFB security type 2 (VNC Authentication), and a live
  **RDP** transport. `mackesd` brokers local VM consoles onto the mesh overlay
  (`vdi-vm-1`), transforms guest pointer input into desktop pixels (`vdi-vm-2`),
  persists VDI sessions in the mesh store, and gates cold-migration undefine behind a
  target-commit ack (`vdi-vm-5`); TESTVM-1..4 add an Alpine + xrdp degradation path.
- **Windows-style desktop shell.** A pixel-per-Win10 bottom taskbar (NAVBAR-W10-1..6,
  NAVBAR-1..3) with a Carbon tray glyph set, micro-flyouts, and a brand-watermark
  backdrop; a two-pane **Start Menu** with a live-tile grid + rotation, type-to-launch
  search, and a Critical edge-cue auto-close (WIN7-1..8); a left **vertical dock** (app
  picker + status/system 2×2 quads + a Timers & Alarms surface, VDOCK-1..5); a grouped
  bottom **picker** (PICKER-1..2); a **lock curtain** with real seat-user PAM auth,
  boot/idle/lid triggers, and lock-screen media (CURTAIN-1..4); a master-detail
  **Settings** shell (SETTINGS-1..5); and shell-embedded per-surface menu bars across
  every app (MENU/MENUBAR-ALL).
- **Mesh discovery + device management.** The **Explorer** "Hero" discovery surface
  (a `unit_aggregator` worker + Unit model, an active LAN scan, edge derivation + a Bus
  edge stream, hero cards + telemetry sparklines, an IPAM prefix/IP table, a mosaic
  overview, universal search/pin/multi-select, EXPLORER-1..18); a **Device Manager**
  (per-node hardware inventory from `hardware_probe`, a host rail + by-node /
  by-connection views, export/print, an armed privileged device-exec seam, and
  debounced fleet-wide device-fault notify, DEVMGR-1..11); and a **node self-grade**
  worker + an A–F dock badge (NODE-GRADE-1..3).
- **KDE-Connect over the mesh (KDC-MESH-1..9) + Transfers.** Nebula-overlay-only
  transport with directed discovery, mesh-wide pairing + a universal `kdc_host` role,
  bidirectional notifications, remote input, run-commands (incl. OpenStack lifecycle +
  battery/telephony), two-way any-node file transfer + a service directory, and a
  **Phones hub** surface. A `mackesd` **transfers** worker (queue/ledger/typed verbs) +
  a Transfers tab land the transfer lanes (TRANSFERS-1/8).
- **QBRAND — the Quasar identity (QBRAND-1..11).** Compile-time build identity, an
  embedded Quasar SVG icon set + tintable rasterizer, the product logo lockup, a DRM
  boot-splash with real init progress, a chrome build tag + per-node role badges, the
  official wallpaper backdrop, and an About surface. **Intel One Mono** becomes the
  platform font (PLAT-FONT-2).
- **Power + seat (POWER-3..5, E12-9/10/17).** An `mde-seat` power backend v2
  (profiles/thresholds/telemetry/lid), a Power Settings panel, a DRM-native idle + lid
  honorer, a Hibernate verb + an honest on-AC line-power read, local audio, USB/VFIO
  passthrough (libvirt), and an actionable Bluetooth panel.
- **CI + packaging hardening.** An always-on **farm CI gate** (test-obs-1:
  fmt + clippy + tests published to the Bus) and a **`verify-rpm-payload.sh`** static
  gate against "compiles ≠ ships" + dead surfaces.

### Changed
- **Control plane → Cloud plane (QC-15).** cloud-hypervisor is deleted; libvirt/
  QEMU-KVM under Nova/OpenStack is the VM plane, and the shell's Controller plane is
  replaced by the Cloud plane.
- **`mackesd` resilience.** Leader-election split-brain is closed by a substrate-aware
  `LeaderGate` (all leader-gated workers routed through it); the worker supervisor
  gains half-open circuit-breaker recovery (mackesd-05) and bounds + off-runtimes its
  slow worker shell-outs (mackesd-02); CLI verb handlers are extracted into `src/cli/`
  modules (arch-1). The RPM cut feature list is de-duplicated under a `--locked`
  policy (build-deploy-3), with a Fedora target matrix + glibc compat contract
  (build-deploy-4).
- **Performance.** An event-driven DRM present loop + per-BO framebuffer cache
  (perf-1), the seat snapshot moved off the render thread (perf-2), a fast-path
  `Persist::open` + bounded Bus `read_latest`/`read_tail` (perf-3/4), Spice
  dirty-checked frames + an adaptive pump, and the CEF helper's 125 Hz idle spin
  eliminated.

### Security
- **Browser confinement + hardening.** The CEF/Chromium helper runs in an OS sandbox
  (security-1); the CDP port, WebRTC block, and passkey consent / `rp_id` paths are
  hardened (5 fixes). A **P0** fix redacts a leaked live root password from tracked docs
  (build-deploy-1), and injected content was scrubbed from a 2026-07-06 commit chain.
- **Servo pin + update cadence.** The browser engine is pinned to the
  `servo = "0.3"` crates.io publication (its own excluded workspace + `Cargo.lock`,
  so the pin is reproducible + tamper-evident per build). Cadence: **track Servo
  monthly** — re-pin to a current release each cycle to carry upstream security
  fixes, minding the API churn of a young engine. The helper runs the SpiderMonkey
  JIT on untrusted web content, so a stale pin is a security-relevant defect;
  bump it with the rest of the fleet.

## [12.0.0] "Quasar" — 2026-07-02
> **Identity cut (`release(E12-14)`, commit `37f28936`; tag `magic-mesh-v12.0.0`).**
> The **E12 "Quasar" pivot** — the egui-native, DRM-native thin-client VDI desktop —
> is now the platform's identity. Version bumped `11.4.5 → 12.0.0`; the codename is
> `12.0.0 "Quasar"` (the exact spelling "Quazar"/"Quasar" is a pending operator
> decision — see `docs/NEEDS-OPERATOR.md`). The large 12.0.x feature wave that
> followed the tag is under **[Unreleased]** above.
### Changed
- The egui shell **is** the platform: the retired libcosmic/iced + strict-IBM-Carbon
  stack is gone (completed across 11.4.x); the About/greeter/chrome now carry the
  12.0 Quasar identity.

## [11.4.5] — 2026-07-01
> Rolling DRM-shell cutover cut (deployed to fleet machines; **not** git-tagged).
> Completes the E12-14 toolkit retirement and lands the notification-interface pivot.
### Changed
- **E12-14c — the iced GUIs are retired; the egui shell is the Workbench.** The iced
  `mde-workbench` is deleted (the egui shell *is* the Workbench); `mde-card` folds into
  `mackesd_core::card` (one consumer, no crate); `mde-theme` is re-scoped to the egui
  `brand` module; the retired toolkit's name is purged from every crate + manifest.
### Added
- **NOTIFY-CHAT — one notification interface.** An ICQ-style mesh chat (`mde-chat`)
  replaces and **removes** the standalone Notifications + Clipboard surfaces and
  `mde-notify`: every host (local, remote, VM guest) is a roster contact whose system
  alerts + clipboard copies arrive as signed messages from that contact (a mackesd
  `chat` worker folds every alert lane).
- **E12-15 — `mde-seat`**, the seat hardware-access foundation library.
### Fixed
- A DRM-shell fix (the 11.4.5 bump).

## [11.4.4] — 2026-07-01
Rolling DRM-shell cutover cut (fleet-deployed, untagged). Version rollup.

## [11.4.3] — 2026-07-01
### Added
- **E12-3 — full keyboard + modifier support in the DRM backend** (the egui/DRM seat
  handles the complete key + modifier set).

## [11.4.2] — 2026-07-01
Rolling DRM-shell cutover cut (fleet-deployed, untagged). Version rollup.

## [11.4.1] — 2026-07-01
### Fixed
- **E12-3 — DRM-seat shell wiring + packaged: closes the 11.4.0 GUI gap.** The
  DRM-seat egui shell is wired and **added to the RPM assets**, so a Workstation
  again boots to a working desktop (11.4.0 had stripped the old GUIs without the
  replacement present — see below).

## [11.4.0] — 2026-07-01
> Rolling DRM-shell cutover cut (fleet-deployed, untagged). Begins the hard cutover
> to the egui/DRM shell.
### Removed
- **E12-14b — the old iced GUIs are stripped** (Files/Music/Voice + the cosmic
  applet).
### Fixed
- ⚠️ **Known GUI regression (fixed in 11.4.1).** The strip removed the iced GUIs
  but the egui shell replacement was **not yet in the RPM assets**, so 11.4.0
  shipped without a working desktop (the workspace built green the whole time —
  compiling ≠ shipping). Corrected in 11.4.1 by wiring + packaging the DRM shell.

## [11.3.1] — 2026-07-01
Patch: a security-relevant fix found by live-verifying 11.3.0 on the fleet.
### Fixed
- **Nebula Certificate V2 fingerprint parsing** — `nebula-cert print -json` emits a
  JSON *array* for V2 certs (V1 was a single object); `parse_fingerprint_json` read
  the array as absent, so on a V2 fleet the OW-10 self-test cert probe false-FAILED
  and the `leave` verb's revocation-eviction silently couldn't fingerprint a cert to
  blocklist it. Now accepts both shapes (+ a V2 wire-shape test).

## [11.3.0] — 2026-07-01
Feature release: the `mackesd onboard` engine gains its VDI + services verbs, and
the two operator-active iced-`mde-workbench` GUI epics (CTRLSURF, NOTIFY-REDESIGN)
land complete. Purpose: deploy the new onboard engine to the live mesh to unblock
the integration-gated live-verify seams (OW-3/4/5/10/13).
### Added
- **OW-8 `onboard first-desktop`** — plans + offers a Workstation's first local VM
  desktop (golden-image select → dual-homed `mde-kvm` `VmSpec` → create/boot →
  broker session), with create/reconnect/no-image branches + a gated
  `FirstDesktopApply` seam (never a fake success).
- **OW-11 `onboard service-add`** — a day-2 Services flow (Music → Navidrome on a
  media-lighthouse reading DO Spaces; Files → P2P Send-To, no VM; Voice → external
  SIP) over a gated `ServiceApply` seam; services never block the working network.
- **CTRLSURF 1–8 — Workbench "Command Watchfloor" complete** — the compact
  command-line + status surface, whole-home keyboard nav, the Expand activity rail,
  real CompactExpand window resize, one universal scope-first sidebar, a shared
  zebra `striped_list` helper (over a new `Palette::zebra_row` mde-theme token), a
  subtle density pass, and a mesh-native Workbench icon.
- **NOTIFY-REDESIGN A–C — Notification Hub redesign complete** — top
  Notifications/Clipboard tabs + a message-first list, Voice/Music footer icons +
  transport, and one generic reusable center-modal detail viewer
  (Notification / Clipboard / Lighthouse / Voice consumers).
### Changed
- **Onboarding reconciled to the 2-role model** — OW-9 (XCP-NG provisioning)
  dropped; XCP-ng is day-2 adopt (MV-7), not a role. E12-14 (iced decommission)
  readiness audited + sliced.

## [11.2.0] — 2026-06-29
Reconciliation release: the divergent `master` (pre-SUBSTRATE-V2) and the
fleet-proven `farm-autoscale-plan` lines were unified onto one validated trunk
(base-on-canonical + graft of master-unique deltas; LizardFS-era reimplementations
dropped). See `docs/POSTMORTEM-line-divergence.md` + `docs/RECONCILE-PLAN.md`.
### Added
- **XEN-194 grafted onto the `for_each` XAPI farm model** — fourth build dom0
  (`mcnf-build-53` @ .170) as a net-new `build_x194` pool + `x194` provider alias;
  surfaced in the `build_farm` output.
- **ABOUT-OSS acknowledgements** page (10 OSS projects) + NotifyCenter autostart.
- **MEDIA-9** — `mcnf-music-ingest.sh` (`upload`/`rescan`) now packaged in the RPM
  (`/usr/libexec/mackesd/mcnf-music-ingest`), so any fleet node can ingest music.
### Fixed
- **MEDIA-7** — `mackesd leave` now de-registers from the media plane
  (`<host>/media-registry.json`), so a torn-down Lighthouse_Media node leaves no
  stale "up" registry row.
### Removed
- Dead `ipc::bus_bridge` module (zero callers since the FDO Notifications server was
  retired to Cosmic, 2026-06-13).

## [11.1.0] — 2026-06-28
A large feature wave: the desktop launcher + mesh-map surfaces, the New-Mesh
genesis wizard, and the reproducible **DevOps backoffice** (DEVOPS-AUTOMATION-REBUILD).
### Added
- **MESHMAP — EtherApe-like global-mesh wallpaper.** Geographic node placement,
  stable per-node hue, per-direction packet-particle traffic colored by the sending
  node, relay paths bent through the lighthouse, reduce-motion/zero-CPU-idle, and a
  `link_traffic` mackesd collector reading per-peer nftables byte counters for real
  per-link volume (with an honest per-node-throughput fallback).
- **APPLAUNCH — the unified Front-Door launcher.** Filter chips (Local/Mesh/Workloads/
  Services) + favorites grid, fuzzy search + `>`-run, real app icons, operator groups,
  on-demand peer-app discovery + launch-on-peer, cache + lazy-mesh, keyboard-first.
  The standalone `mde-apps-applet` is **retired** — the Front Door is the sole launcher.
- **DATACENTER-18 — New-Mesh genesis wizard** ("give birth to a new Nebula"): plan +
  Tofu-write the founding lighthouse + DNS + first join token (live apply gated).
- **DATACENTER-21 — provisioning test-mesh + farm-scale UI** over the `action/dc/testbed-*`
  + `farm-scale` verbs.
- **VM-internal services** are discoverable: Instances rows correlate to their
  enrolled-peer services.
- **DEVOPS-AUTOMATION-REBUILD** — the DevOps backoffice is now reproducible + portable
  to a new Nebula on a dedicated control VM: mesh-etcd-backed Tofu state (off the dead
  LAN node, on the live lighthouse quorum), on-VM secret-zero (age-keygen + atomic
  multi-recipient reseal, no plaintext-in-state), `mackesd found --with-backoffice`
  + the `backoffice-up.sh` orchestrator (tiered Minimal/Full), self-hosted Forgejo CI,
  a plan-only systemd reconciler, a backoffice-provisioned sccache build farm
  (`for_each`+`moved{}`, 0-destroy), DR v2 (consistent etcd+Forgejo snapshot), per-mesh
  config + portability resolver, cred-store folding, and RPM packaging of the plane.
  (42/52 DAR tasks code-complete; the live stand-up + off-fleet DR/CA push stay
  operator-run.)
### Changed
- **DATACENTER-25 — panel consolidation.** `compute`/`snapshots`/`images`/`lighthouses`/
  `build_farm` fold into the Datacenter panel as a fold-bar of tabs; deep-links + the
  launcher search redirect the retired slugs. No unreachable modules left behind.
### Fixed
- **BROKER-RESILIENCE-3** — the ntfy notification broker is now turn-key *present* on a
  freshly provisioned lighthouse (the first-boot fetch oneshots start in cloud-init),
  not just non-fatal when absent.
- **MIG-3** — a joined lighthouse provisions its own sealed CA-backup passphrase, so it
  no longer boots `SEC-7/ENT-11: the CA is UNBACKED-UP`.

## [11.0.15] — 2026-06-27
### Added
- **`mackesd secret put|get [--local] <name>`** — a CLI for the leader-managed mesh
  secret store (DATACENTER-3). `put` reads plaintext on stdin and age-encrypts it;
  `get` decrypts to stdout. `--local` forces the Syncthing-replicated LocalAead
  store, so a repo node can seal a secret (e.g. `media-spaces`) that the
  lighthouses then read via their own LocalAead store keyed by the shared mesh age
  identity. Closes the operational put-path the readers (`media_registry`, VPN, DR)
  always assumed but no CLI exposed — the gate for provisioning MEDIA + DR
  credentials without hand-editing per-node files. (`age_key_path` made pub.)

## [11.0.14] — 2026-06-27
### Fixed
- **MIG-1: `remove-peer` now deletes the etcd `/mesh/peers/<host>` directory key.**
  It removed the etcd MEMBER + revoked + banned, but left the directory row, so the
  roster reconcile kept re-adding a node whose droplet was already gone (the stale
  lighthouse entries that had to be `etcdctl del`'d by hand during the 2026-06-27
  retire). Decommission is now complete — member + directory row both removed
  (`substrate::peers::delete_peer_blocking`).
- **MIG-2: overlay-IP assignments are recorded mesh-wide at SIGN time.** The peer
  directory is heartbeat-lagged, so two lighthouses signing within the heartbeat
  window could both pick the same IP (the cross-lighthouse collision that handed a
  node 10.42.0.1). The enroll signer now unions a shared `/mesh/ipalloc/` etcd
  reservation keyspace into its taken-set and writes the assignment there
  immediately after signing (`reserve_overlay_ip_blocking` /
  `reserved_overlay_ips_blocking`), so the next sign anywhere sees it at once —
  closing the practical window. (A fully same-instant CAS is a noted follow-up;
  it only matters for simultaneous multi-lighthouse signs, which sequential
  operator-driven adds never produce.)
### Notes
- **MIG-3 (CA-backup passphrase on joined lighthouses)** folds into the DATACENTER-23
  DR / secret-store workstream: post-migration there is no passphrase source on the
  fleet, so MIG-3 is the *establish-a-leader-managed-shared-passphrase* mechanism
  (+ off-fleet encrypted CA backup), done there rather than standalone.

## [11.0.13] — 2026-06-27
### Fixed
- **Overlay-IP allocation is now mesh-global, not per-lighthouse — no more
  collisions when a joined lighthouse signs.** `ca::sign::allocate_overlay_ip`
  scanned only the **local** `nebula_peer_certs` table for the next free
  `10.42.x.y`. A *joined* lighthouse's local store holds only the certs IT
  signed, so it restarted at `10.42.0.1` and handed a brand-new node an IP
  already in use mesh-wide — caught live in the 2026-06-27 migration: a node
  enrolled via the new nyc3 lighthouse was assigned **10.42.0.1**, the founding
  lighthouse's own IP. New `allocate_overlay_ip_excluding` also skips every
  overlay IP already assigned per the shared **etcd peer directory** (passed by
  the enroll path via `read_directory`), so every signing lighthouse allocates
  from one global view. Founding self-sign + epoch rotation pass an empty set
  (they run on the founder/leader with the full local store). This unblocks
  retiring the founding lighthouse: the new lighthouses now enroll new nodes with
  unique IPs. (A fully-atomic cross-lighthouse allocation — for two lighthouses
  signing in the same instant — is a follow-up; sequential enroll is unaffected.)

## [11.0.12] — 2026-06-27
### Fixed
- **A joined lighthouse now SERVES enrollment (`:4243` self-cert).** The turn-key
  add-lighthouse flow (#12) ships the sealed CA key to a `join --role lighthouse`
  node so it can *sign*, but `mackesd found` never runs on it — so it lacked the
  self-signed `/etc/nebula/enroll-endpoint.crt` and the `nebula-enroll-listener`
  skipped binding `:4243`. The new lighthouse was only *half* a lighthouse: a CA
  holder that could sign yet could not accept enrollments, so once the founding
  lighthouse was retired the mesh could no longer add nodes. Found live during the
  2026-06-27 migration — nyc3/sfo3/fra1 came up full (am_lighthouse + CA key + etcd
  voter, dialed by every peer) but `:4243` never bound. Fix: at serve-startup, if
  the node holds the CA key and the endpoint cert is absent, self-generate it (the
  same self-signed rcgen identity `found` writes; SAN = the node's primary public
  IPv4). Tokens later minted on that lighthouse pin its own fingerprint. Idempotent,
  best-effort, never blocks startup. A joined lighthouse is now a full enroll anchor.

## [11.0.11] — 2026-06-26
### Fixed
- **Lighthouse watchdog crash-loop (P1) — a down broker no longer SIGABRTs the
  control plane.** Both production lighthouses had been crash-looping on the 60 s
  systemd watchdog every ~90 s for ~40 h (1355/1348 aborts), which blocked every
  enrollment (`:4243` was only up between crashes). Root cause: on a 1-vCPU droplet
  the tokio runtime defaulted to a **single worker** that also owns the time driver;
  when that worker reached a blocking bridge (`substrate::peers::block_on` →
  `block_in_place` for an etcd round-trip) the **time driver froze**, so the in-loop
  `tokio::time::sleep` watchdog ping stopped firing and systemd aborted a daemon that
  was actually healthy. A missing `ntfy` broker reliably triggered it by adding
  `block_in_place` churn. Two complementary fixes:
  - **`worker_threads` floored at 4** even on a 1-vCPU box, so a second worker keeps
    timers firing while another blocks (they just time-share on a single core; the
    daemon is I/O-bound).
  - **The watchdog heartbeat now runs on a dedicated OS thread** gated on an async
    *liveness beacon* the serve loop stamps every 250 ms (`sd_notify::watchdog_should_ping`).
    The ping is kernel-scheduled (unstarvable by the executor) yet still reflects true
    liveness — a genuine runtime wedge stops the beat → the thread withholds the ping →
    systemd restarts, preserving the watchdog's purpose. Replaces the incorrect
    `BROKER-RESILIENCE-1` "own dedicated timer; no further isolation needed" claim.

## [11.0.10] — 2026-06-26
### Added
- **Turn-key multi-lighthouse HA.** A lighthouse is added to (or retired from) a
  running mesh as one push-button op — no manual `etcdctl`/`scp`:
  - `mackesd lighthouse add --region <r>` mints a role-scoped lighthouse token and
    provisions a DigitalOcean droplet (new `do-lighthouse-join.sh`, carrying the
    SSH-key lockout invariant guard) that JOINS the existing mesh as a full
    lighthouse; `mackesd lighthouse retire` drains it (holding the
    `HA_MIN_LIGHTHOUSES` floor), removes it from the etcd quorum, revokes its cert,
    and deletes the droplet.
  - **Auto-etcd cluster membership** (`substrate::etcd_membership`): a joining
    lighthouse adds itself to the Raft quorum as a voter via the native
    `etcd_client` member API; retire/leave/remove-peer removes it (move-leader off
    it first) — no hand-run `etcdctl member add/remove`.
  - **Auto-CA-key distribution**: a joiner enrolling under a role-scoped lighthouse
    bearer receives the sealed mesh CA key over the authed enroll channel + seeds
    the shared `nebula_ca` row, so it becomes a full *signing* lighthouse. The CA
    spread is gated to lighthouse-role bearers only (ENT-12, §8).
### Fixed
- **Multi-lighthouse roster propagation (the root of "only one lighthouse").** The
  `/enroll` roster reads the etcd directory (not the frozen fs snapshot); the
  CSR-watcher + `ca sign-csr` build the full directory roster (not a hardcoded
  `10.42.0.1`); and the nebula supervisor reconciles each node's own bundle from the
  live directory every tick — so an already-enrolled peer (e.g. Eagle) learns a
  newly-added lighthouse and reloads with no re-enroll. Verified live on UNIT-EAGLE.
- **DDNS ingress label.** `ingress_record_label` collapses an apex hostname
  (`host == zone`) to an empty label instead of publishing the zone as a bogus
  DDNS record.

## [11.0.9] — 2026-06-25
### Added
- **Datacenter plane (Workbench).** Full VM lifecycle — snapshot list/revert/delete
  (DATACENTER-11). A Storage tab — SR/VDI create + attach/detach (typed-confirm),
  scheduled snapshots with retention via a leader-gated `dc_snap_scheduler` worker,
  an ISO + template image library, and SR capacity-threshold alerts (DATACENTER-12).
  Health checks — cert-expiry, VM-crash, pool-degraded, and per-resource dom0 log
  aggregation into the Datacenter Logs view (DATACENTER-24). An append-only action
  audit carrying actor + result, with every destructive `action/dc/*` verb (incl.
  host reboot/shutdown/evacuate) confirm-gated — no RBAC, honoring the §8/§9
  flat-trust lock (DATACENTER-7). A DigitalOcean region picker with a geo-spread
  recommendation + a guided new-lighthouse flow that writes a `digitalocean_droplet`
  Tofu resource (DATACENTER-19). Build→Eagle→DO auto-promote-on-green driven by the
  L1–L3 test verdicts, gated by a persisted prod-arm switch (DATACENTER-20).
- **Mesh media.** A DigitalOcean Spaces media bucket + a least-privilege bucket-scoped
  S3 key, sealed as the leader-managed `media-spaces` mesh secret (MEDIA-2). Workstation
  auto-config that writes the music client's creds from the mesh service registry so
  `mde-music` auto-browses `music.mesh` instead of a manual first-run (MEDIA-8).
- **Connectivity.** Exposing a service auto-creates/removes its public DDNS name, and
  ROUTE-TRACE renders the real ingress path with live firewall verdicts (CONNECT-9).
- **Build farm.** A `farm-slot-gc` timer reclaims stale per-build slot dirs on every
  build VM so a node never wedges on a full disk; drain coordinator / park-blocker /
  worktree-isolation tooling for the autonomous drain (DRAIN-5/6/7).

### Changed
- **Mesh Sync.** The shared file plane is renamed "Mesh Sync" across the UI + enroll
  surfaces; the `/mnt/mesh-storage` path + `MDE_WORKGROUP_ROOT` env stay for
  back-compat (SUBSTRATE-12).

### Fixed
- The mesh secret store's `mcnf-secret.sh get` was globally broken on binary age
  ciphertext (a NUL-stripping shell capture) — now binary-safe (file-routed), so
  every sealed secret is retrievable.
- `xcp-build.sh`'s shape-routing fallback pointed at a non-existent `.52` host —
  corrected to the live fixed build VM `.50`.

### Removed
- **SUBSTRATE-6 — the full LizardFS rip-out (one-way).** The dead LizardFS plane
  is gone now that SUBSTRATE-V2 (etcd + Syncthing) is the substrate: the in-`mackesd`
  `meshfs_worker` supervisor + the `src/meshfs/` module (headroom + the
  `mfsmetadump`/`mfsadmin` state-snapshot), the `found`/`join` LizardFS provisioning
  (`provision_qnm_shared`/`qnm_setup_flags`), the `MeshFs*` CLI subcommands, the
  `meshfs_snapshot` CA-bundle field, and the LizardFS mesh-storage-leader VIP trace
  target. `shared_root_writable` collapsed to the plain-dir semantics (the
  ONBOARD-6 `/proc/mounts` FUSE poison guard dropped). The install/recovery scripts
  (`mesh-install-lizardfs.sh`, `setup-qnm-shared.sh`, `qnm-mount.sh`,
  `unwedge-lizardfs.sh`, `vendor-lizardfs-rpms.sh`, `phase-a-stabilize.sh`,
  `phase-b-retire-lizardfs.sh`) are deleted; the RPM no longer ships them, the
  bundled fc43 `lizardfs-client` RPM, or the `fuse-libs`/`fuse` Requires. On
  upgrade a retirement scriptlet masks+removes the stale `qnm-shared.service` and
  the `20-qnm.conf` ordering drop-in. `/mnt/mesh-storage` + `MDE_WORKGROUP_ROOT`
  stay as the plain Syncthing-replicated dir. A fresh install carries no LizardFS.

## [11.0.1] "Winter-Is-Coming" - 2026-06-20
### Fixed
- **FOUND-NEBULA-1** — a fresh-node founding/join failed to bring up the Nebula
  overlay: the `nebula` package's stale example `/etc/nebula/config.yml` got
  merged with mackesd's materialized `config.yaml` (the unit loads the whole
  `-config /etc/nebula` dir), so `am_lighthouse:false` + a bogus static_host_map
  won and the unit failed. `materialize_config` now removes the stock `config.yml`.

## [11.0.0] "Winter-Is-Coming" - 2026-06-20
> Major version: the SUBSTRATE-V2 split (etcd coordination + Syncthing files,
> LizardFS retiring) + the MCNF rename. See docs/design/substrate-v2.md
> (epic SUBSTRATE-1..14). **10.0.18 was the last 10.x cut.**
### Added
- **SUBSTRATE-V2** — the new mesh substrate ships in the binary: etcd-backed
  coordination (leader election / peer directory / health) and Syncthing-backed
  file replication of `/mnt/mesh-storage` (no FUSE), replacing the LizardFS
  "QNM-Shared" plane. The coordination bridges (`SUBSTRATE-1..10`) go etcd-only
  once `/etc/mackesd/etcd-endpoints` exists; the cutover is deliberately
  operator-driven (`install-helpers/cutover-substrate-v2.sh`, with `--no-flip`/
  `--no-files` for a fleet-safe staged roll) and additive until LizardFS is
  removed in a follow-up (SUBSTRATE-6). Validated by two live DO rehearsals
  (etcd quorum + Syncthing file sync + reboot drill all green).
- **MEDIA-LIGHTHOUSE** epic — Airsonic Podman container on every lighthouse as a
  hot-redundant, published "Auto Configuration host" for the Music System over a
  shared 100 GB object store (docs/design/media-lighthouse.md).
- **MUSIC** — playlist editor (`Route::Playlist`) with drag-reorder + remove via
  a track context menu, backed by the `playlist-reorder` musicd verb and a
  persistent warm Airsonic client (`refresh_airsonic_client`).
### Changed
- **OPROG-6 / SELinux** — `SELINUX=disabled` is the new platform standard;
  `install-helpers/setup-selinux-policy.sh` now disables SELinux (was: install a
  CIL policy for Enforcing).
- **Applet labels** — the panel Applications-menu applet now reads **`Start>`**
  and the Notification-Hub applet reads **`Activity`** (text labels, not icons).
- **mde-bus** — persisted events now use a monotonic ULID generator
  (`static ULID_GEN`) so same-millisecond writes stay ordered.
- **BRAND-11** — new 11.0 brand identity (the MCNF windowed-constellation logo,
  `assets/icons/Start5.png`). The background is flood-keyed to transparency
  (interior gridlines/nodes preserved) and regenerated across every brand
  surface: the panel launcher icon, the hicolor app/window icons (16–512), the
  brand-loader slots (app-icon / monogram / logo-lockup + the wordmark lockups,
  baked SVGs embedding the logo), and the greeter hero (logo on Carbon Gray-100).
  The brand is now **fixed-palette** (`is_tintable` → false). The logo is added
  as a **watermark** on the Notification Hub's lower area and as the **About
  panel hero**; the About codename auto-tracks the major version
  (11.x → "Winter-Is-Coming").

## [10.0.18] - 2026-06-19
> The final 10.x cut (operator: "10.0.18 can and will be the last cut").
### Added
- **RCLICK** — Win+X-style right-click power menu on the panel launcher (File
  Explorer/Settings/Terminal/Terminal-Admin/Task-Manager(btop)/Midnight-Commander/
  Device-Manager/Network/Disk/Event-Viewer/Apps&Features/About/Computer-Management/
  Mesh-Control/Lighthouses/Notification-Hub/Join-Mesh/Show-Desktop/Power), a Run
  (Win+R) box, and the bundled deps (btop, mc, cosmic-disks).
- **MUSIC-HOME** — the Music Home page is now a live Airsonic server-stats
  dashboard: hero Songs/Artists/Albums + a server card (host/version/scan/library/
  health) + Most-Played/Starred/mesh-Now-Playing strips, polled live
  (`action/music/library-stats` + `list-frequent`/`list-starred`).
- **LIGHTHOUSE** epic — Carbon beacon token, shared discovery/health module, an
  animated Notification-Hub footer, a Workbench Mesh▸Lighthouses tab, Hub→tab
  deep-link, and bash-login Network-Overview markers; lighthouses identified by
  Nebula `static_host_map` membership.
- **MESH-LAYOUT** — the canonical Cosmic panel layout is baked + enforced on every
  desktop each session (`mde-enforce-layout`).
- **APPS-ICON** — the Start3 brand icon on the panel launcher; the launcher is 2×
  wider (golden landscape) with a 3×3 Carbon-icon Favorites grid.
### Fixed
- **Boot recovery** — a reboot no longer stalls the mesh ~2 min (mackesd was
  queued behind the QNM-Shared mount loop); an idempotent RPM migration strips the
  stale ordering on every node, and a disconnected laptop now boots fast to a
  usable local desktop.
- **Music** — "Unknown Track" in the Hub (get-song `{"id":…}` parse), the Radio
  "daemon not responding" timeout (10s + auto-retry), and artist browse.
- **Notification Hub** — theme-aware (light/dark) + Carbon header + zebra rows +
  button coloring matched to the Application Menu + a mini-player with album art.
- **Artifact Manager** — peers populate after a cold boot (backend reconnect).
- **Data accuracy** — the mesh-status snapshot no longer leaks the unedited
  example nebula config into the cipher/gateway/lighthouse fields.

## [10.0.17] - 2026-06-18

### Added
- **Fleet-wide workloads (WORKLOAD-FLEET-1).** The Workbench ▸ Provisioning ▸
  Instances panel now lists every node's VMs + containers, not just the local
  box. `compute_registry` mirrors each node's inventory to the replicated
  QNM-Shared plane (`<host>/compute-inventory.json`); the panel folds all peers'
  files with a Node column, deduped, lifecycle actions gated to local rows.
- **Fleet-wide Published Services (SVC-VIEW-1).** The Mesh ▸ Published Services
  panel lists the 7 canonical services (SSH/NATS/Mesh FS/Media/rsync/WoL/AV) for
  every enrolled peer (read from the replicated peer roster), each with a Node
  column + reachability pill — was local-only and showed empty.
- **Nebula encryption-strength label (NEB-CRYPTO-LABEL).** The notification-bell
  applet shows the live overlay cipher (e.g. AES-256-GCM) next to the bell,
  sourced from the world-readable mesh-status snapshot (`network.cipher`).

### Fixed
- **GLYPH-FIX — slow first-paint + black panel icon.** Emoji-presentation glyphs
  routed through the color-emoji font ignored the Carbon tint (black-on-dark
  bell) and stalled first paint for seconds. Replaced with text-presentation BMP
  glyphs across the bell, apps applet, music, and notification center.
- **Music browse lockup** on large libraries (windowed art load), **art-focused
  Full View** scaling, and a **persistent playback bar** in every music view.

### Changed
- **Start menu / apps applet redesign** — click-to-toggle (no mouseover popup),
  Music-style zebra Carbon rows with right-aligned actions, light + dark themes,
  golden-ratio sizing, app names in primary text.
- **Shell login banner** gains a Network Overview (ASCII topology + routable
  subnets + external gateways).
- **XCP foundation (XCP-1, XCP-6)** — `mackes-xcp` hypervisor-access layer and
  the `xcp_host` capacity-advertising worker.

## [10.0.16] - 2026-06-18

### Added
- **Boot-status dialog (BOOT-STATUS epic, complete).** A `boot_readiness` mackesd
  worker publishes one ordered `state/boot-readiness` snapshot: the fabric
  dependency chain (Nebula → overlay-IP → mackesd → bus → QNM-Shared → peer
  directory), the app daemons (musicd / netdata / KDE Connect, active + port
  reachability), and a per-peer ping roll-up (RTT, lighthouse tagged). The
  Workbench HOME panel renders all three, collapsing to a green "Mesh ready" chip
  when all-green. A login autostart (`mde-workbench --boot-popup`) opens it at
  session start and stays silent once the mesh is up. A down app daemon shows an
  inline **Restart** (user-unit `systemctl --user` for musicd, pkexec for system
  units).
- **Peers "settling…" state (BOOT-PEERS-1).** During the cold-boot warm-up the
  Peers panel distinguishes "still converging" from a genuinely empty mesh.
- **Music client refactor (MUSIC-RFX-1..7).** Daemon queue model (reorder / remove
  / play-next) + bus verbs; engine **seek** + `seek` transport verb; Subsonic
  **playlist write** verbs (create / update / delete); a maxi now-playing view
  with an interactive scrub slider (hidden for live streams) on a dominant-colour
  art tint; an **editable queue** panel (select / reorder / play-next / remove);
  a **playlist editor** (create / rename / delete); and an **add-to-playlist**
  picker from album rows + now-playing.

### Fixed
- **Bus consumer stranding (BUS-INODE-ORPHAN-1).** A read-only `index.sqlite` is
  now self-healed by an in-place permission fix before any destructive recreate,
  the recreate is gated on ownership (a non-owner GUI can't unlink the root
  daemon's live index), and every long-running consumer reopens on an inode swap
  (`Persist::reopen_if_index_changed`) — fixing the "daemon not responding after
  long uptime" wedge.

## [10.0.15] - 2026-06-17

### Changed
- **Rebrand → MCNF (Mackes Cosmic Nebula Fedora).** The product display name is
  now **MCNF**; **"Magic Mesh" is the 10.0.x series codename** (shown as
  `MCNF 10.0 "Magic Mesh"` in About/greeter). The `magic-mesh` package, repo, dnf
  channels, release tags, icon name, and `org.magicmesh.*` IDs are **unchanged**
  (upgrade-safe; renames to `mcnf` at the 11.0 boundary) — only display strings
  changed across ~105 files.
- **New default app icon** (penguin-on-mesh, `Icon-MCNF`) regenerated to all 9
  hicolor sizes + brand masters; every app uses `Icon=magic-mesh`, so all apps
  re-brand at once.

### Added
- **APPS — the mesh-wide Applications Panel launcher** (replaces Cosmic's
  app-library; design `docs/design/apps-launcher.md`):
  - **APPS-1** mackesd `apps_aggregator` → `action/apps/list` (local XDG+Flatpak,
    mesh peers, workloads, services, each tagged kind/source/node/health).
  - **APPS-2** `mde-apps-applet` panel applet: grid glyph → tabbed dropdown
    (Favorites/Apps/Mesh/Workloads/Services), bus-fed, Carbon-styled, fuzzy search.
  - **APPS-3** header: live QNM-Shared disk + quick links (Workbench/Files/Settings).
  - **APPS-4** mesh-synced per-user Favorites (QNM-Shared) with a ★ pin toggle.
  - **APPS-5** launch paths: local exec + peer remote-desktop (`action/apps/launch`
    resolves the target; opens remmina).
  - **APPS-6** Workloads: inline Start/Stop/Attach (virsh/podman).
  - **APPS-7** Services: open the published endpoint over the overlay.
  - **APPS-8** right-click context strip (pin/unpin, primary, run-on-peer,
    flatpak uninstall).
  - **APPS-9** baked-layout swap: the MCNF launcher replaces Cosmic's app-library
    button in the panel + dock; applet packaged.
  - (Follow-ons: APPS-8b run-containerized + details; APPS-9b Super-key bind.)

## [10.0.14] - 2026-06-17

### Added
- **QNM-Shared HA shadow master (HA-1):** `setup-qnm-shared.sh --shadow` stands
  up a live LizardFS **shadow master** (`PERSONALITY=shadow` + `MASTER_HOST`,
  live metadata replication, promotable) plus a metalogger on a second
  lighthouse, so the QNM-Shared master is no longer a silent SPOF. `159.65.183.51`
  (overlay `10.42.0.2`) now tracks `45.55.33.179` as a warm standby. Design
  locked in `docs/design/ha-shadow-master.md` (10-Q survey); HA-2..HA-6 (floating
  VIP + auto-failover worker + 2-LH-minimum degraded flag + HA panel) tracked in
  the worklist.
- **KDE Connect plugins (KDC-PLUGINS):** the advertised KDE Connect plugin set is
  now implemented for real; false advertisements removed from the capability list.

### Changed
- **netdata RAM safety gate (NETDATA-1):** `mesh-install-netdata.sh` now refuses
  the 181 MB static install on nodes with `< 3 GB` RAM
  (`MDE_NETDATA_MIN_MB` / `MDE_NETDATA_FORCE=1` to override). The static build
  extracts to hundreds of MB and once OOM-thrashed a 947 MB lighthouse, killing
  its LizardFS master and cascading a mesh-wide QNM-Shared FUSE outage. The two
  lighthouses have `mesh-netdata-setup.service` masked; netdata runs only on
  high-RAM hosts.
- **Lighthouse `/tmp` shrink (HA-6):** both lighthouse `/tmp` tmpfs sized down to
  128 MB so a heavy transient can't OOM the master (pairs with the netdata gate).

- **Mesh-shared cover art (MUSIC-ART-SYNC):** cover art is now pulled down once
  and shared across the whole mesh. `mackesd` provisions a communal
  `<mesh-storage>/music/artwork` cache (0777) on QNM-Shared; `mde-musicd`
  reads-through / writes-through it, so art fetched by any node is reused
  mesh-wide and keeps rendering even when a node can't reach the Airsonic
  server. Falls back to a direct fetch when the mount is absent.

### Fixed
- **Notification CLI fan-out (NOTIFY-DIST-3):** `mde-bus publish` now flattens
  hierarchical topics (slashes → `_`) before the ntfy POST, so CLI-published
  alerts reach the broker on the same topic the subscribers watch.
- **Music: silent playback + missing artwork (MUSIC-RAWVER):** the Airsonic API
  version negotiation ran only on the JSON path; the raw-byte fetches (the
  playback stream + cover art) hit the server at the version ceiling and got an
  error-30 JSON body in place of media (played but silent, no art) against a
  server that caps below the ceiling. The negotiated version is now persisted and
  every client — including the playback engine's stream URL — seeds from it;
  cover-art fetches also self-heal on an error envelope.
- **Music: window locks up browsing folders (MUSIC-ARTGATE):** the album/folder
  grid fanned out one cover-art request per item, so a 200+-item folder
  stampeded the single-threaded music daemon + the shared bus and froze the
  window. Concurrent art fetches are now bounded by a semaphore.
- **Music: daemon stops responding (MUSIC-WEDGE + WEDGE-2):** two causes. (1) On
  startup `mde-musicd` had empty poll cursors and reprocessed the entire
  historical backlog of every action topic before answering anything new (and
  could replay a stale command) — it now seeds each cursor at the topic's current
  tail. (2) After long uptime the daemon could be stranded on a *deleted*
  `index.sqlite` inode when another process triggered the bus self-heal recreate
  (unlink + new file), so it stopped seeing new requests — the daemon now detects
  the inode swap and reopens the store. Music stays responsive without a manual
  restart.

## [10.0.13] - 2026-06-17

### Added
- **Live metrics (NETDATA-1):** netdata is now provisioned as a first-boot
  birthright (`mesh-netdata-setup.service` → `mesh-install-netdata`, a
  sha256-pinned static fetch; netdata isn't in the Fedora repos), confined to
  loopback + the node's overlay IP, so the PD-2 peer-health tiers and the PD-7
  live mesh map / flow particles finally have a data source. The
  `netdata_aggregator` confines the dashboard `[web]` bind on every tick
  (never the underlay — safe on the public lighthouses).
- **Music daemon autostart (MUSIC-DAEMON-AUTOSTART):** the `mde-musicd` user
  service is now `%post`-enabled (`systemctl --global enable`), so the music
  library works on a fresh Workstation with no manual daemon start.

## [10.0.12] - 2026-06-17

### Added
- **Mesh-wide SIP outbound gateway (VOIP-GW-1):** a new Workbench **Mesh → SIP
  Gateway** panel sets ONE outbound SIP/PSTN gateway (host/port/user/password/
  display) for the whole mesh. Apply sends it to `mackesd`
  (`action/voip/set-gateway`), which writes it to QNM-Shared
  (`<workgroup_root>/voip/gateway.toml`, the voice agent's `account.toml` shape,
  0600); every node's `mde-voice-hud` reads the mesh gateway first and registers
  to it — bare numbers route out via the gateway, intra-mesh peer calls stay P2P.
  Test probes registrar reachability; Clear reverts the mesh to P2P.
- **Music player navigation (MUSIC-NAV):** the `mde-music` app gained explicit
  **Back / Home / Exit** header controls (the window has no title-bar chrome).

### Fixed
- **Radio playback (MUSIC-RADIO):** infinite icecast/shoutcast streams now play —
  the engine streams them through an unseekable source instead of buffering the
  whole (never-ending) body, which had failed with a decode error + underrun.
  Finite songs are still buffered for seek support.

## [10.0.11] - 2026-06-17

This roll bundles everything since `v10.0.9`: the SETUP wizard, the LizardFS/ntfy/
starship birthrights, the notification distribution + sources work, the BULLETPROOF
node self-bounding, and the 2026-06-17 SELinux + Action Center fixes. Install+join
with zero manual steps is verified on a clean node on both XCP and local KVM.

### Added
- **Cross-node alert federation (NOTIFY-DIST-2):** each node mirrors its alert
  lanes into QNM-Shared and the Action Center reads every peer's mirror, so the
  panel is mesh-wide (`alert-mirror` worker + `AlertTail::poll_shared`).
- **Alert sources (NOTIFY-SRC-1/2/3):** SELinux AVC denials → the security lane,
  desktop-app (`fdo/*`) notifications captured off the session bus, and KDE
  Connect device events folded into the global Alert Center.
- **Node self-bounding (BULLETPROOF-1/2):** the bus retention GC runs inside
  mackesd with a tmpfs-safe, fs-relative cap + hard-cap eviction (a flooded lane
  can no longer fill `/run`), and the daemon runs under a systemd watchdog
  (`Type=notify` + `WatchdogSec` + `sd_notify`) so a wedged runtime auto-restarts.
- **`magic-setup` wizard (SETUP):** a full-screen TUI that takes a fresh node
  from install to running mesh member — Create a mesh, Join one, list peers, and
  check service Status — narrating each step. Runs on the console at first boot
  (unconfigured nodes) via `magic-setup.service` (tty1 only, never hijacks SSH)
  and on demand as `sudo magic-setup`. A thin narrated layer over the existing
  `mackesd found`/`join` verbs (which already provision LizardFS/QNM-Shared).
  Adds peers/lighthouses (`mackesd add-peer` mints a v3 token), removes them
  (`mackesd remove-peer` = decommission + revoke + ban), and emits a
  `/etc/mackesd/site.yml` Ansible playbook re-appliable with `mackesd converge`
  for idempotent steady-state convergence.
- **LizardFS is now a birthright (BIRTHRIGHT-1):** `mackesd found`/`join`
  auto-provision the QNM-Shared shared-state plane role-aware — install the
  LizardFS binaries (dnf on F43, the bundled fc43 RPMs on F44/offline) and run
  `setup-qnm-shared` (master+chunkserver+client on the founder; client/+chunk on
  peers) before the daemon starts. A fresh `dnf install` + enroll now yields a
  working shared-state mesh with no manual step. `mackesd` also logs a loud error
  at startup if `/mnt/mesh-storage` isn't actually mounted.
- **About panel** (System → About) now surfaces the GitHub repository, a
  Releases/changelog link, and a maintainer contact (all open via `xdg-open`),
  the build version, and the embedded changelog — alongside the existing
  disclaimer, each single-sourced from the repo (`CHANGELOG.md` / `DISCLAIMER.md`).
- **Air-gapped birthrights (BIRTHRIGHT-2):** the ntfy broker and starship prompt
  are now bundled in the RPM (`/usr/share/magic-mesh/vendor/`) and provisioned
  bundled-first at first boot, so an offline install still comes up fully
  provisioned; the network fetch remains a fallback.

### Fixed
- **Notification Center wouldn't open (NOTIFY-UI-4):** its startup read of the
  QNM-Shared (FUSE) dir ran inline on the iced update loop, so a wedged mount hung
  the loop and the layer surface never mapped. The shared read now runs on a helper
  thread and is picked up non-blockingly, so the panel opens regardless of mount
  health. The applet also launches the center detached (`setsid --fork`) so it no
  longer leaks a zombie per toggle.
- **SELinux denial flood (SELINUX-1):** mackesd's podman transition,
  libvirt `/proc` scans, and a logind check tripped tens of thousands of AVC
  denials per boot under Enforcing (audit-log flood + repeating "SELinux security
  alert" toasts). A shipped local CIL policy (`magicmesh-{base,podman,libvirt}`,
  loaded by the RPM `%post`) grants exactly those legitimate accesses — the node
  stays **Enforcing** and runs clean. SELinux denials are also recorded in the
  Action Center but no longer toast below Critical.
- **Shell responder hang (SHELL-RPC-1):** `healthz` mount-enrichment is now
  time-bounded, so a wedged FUSE read can't hang the shell bus responder.
- **Notification distribution (NOTIFY-DIST-3):** ntfy publish topics are flattened
  so hierarchical bus topics no longer 404 on the broker.
- **`~/Documents` mesh sync (AUDIT-MESH-15):** the FPG-7 XDG bind-mount now
  targets the real desktop user's home (not the daemon's `/root`), creates the
  communal mesh source tree, and logs bind failures loudly instead of swallowing
  them — so files dropped in `~/Documents` replicate mesh-wide.

## [10.0.0] - 2026-06-13

First packaged release.

### Added
- Full **libcosmic** desktop cutover: every GUI (`mde-workbench`, `mde-files`,
  `mde-music`, `mde-voice-hud`, `mde-cosmic-applet`, `mde-role-chooser`, the
  live-map wallpaper) runs on libcosmic's vendored iced fork with the IBM
  Carbon look; the Workbench exposes an accesskit (a11y) tree.
- Direct-vs-relay tunnel path classification + chosen underlay endpoint in the
  Peers map trace card, via Nebula's loopback debug-SSH hostmap.
- One-RPM packaging (`cargo generate-rpm`): every workspace binary, systemd
  units (incl. the disabled voice pair), `.desktop` launchers/autostarts,
  icons, the swappable brand pack, DISCLAIMER/LICENSE/NOTICE/SUPPORT, help
  docs, the dnf `.repo` + the project's public signing key.
- First-run deployment-role chooser GUI (`mde-role-chooser`) and the
  cosmic-panel mesh-health applet (`mde-cosmic-applet`).
- Real cross-mesh file transfer (Send-To over the LizardFS-replicated
  volume), confined to the operator share root.
- KDC outbound drainer (ring / send-file / clipboard / share reach devices).
- Live `healthz` (node-health buckets + audit-chain status), the Prometheus
  textfile exporter worker (node health, CA-cert days-remaining, the router
  decision-time histogram), and the configurable `[[alert_hooks]]` layer
  (event JSON on stdin, post-commit dispatch).
- Transport scorer in the routing path with a per-class encryption floor
  (AES-256-class for content; operator-tunable in policy.toml) and
  hash-chained PathSwitch audit events.
- Runtime disclaimer accept gate; governance lint gates (§2 bus names,
  §4 Carbon single-source, §6 mesh boundary) wired into CI; nightly
  `--include-ignored` CI job.

### Security
- FileXfer send-to source allowlist (no exfil outside the share root;
  symlink escapes refused).
- 64 KiB body cap on every Bus responder before parse.
- Worker shell-outs bounded by kill-on-timeout (15 s) helpers.
- Netdata dashboard confined to loopback + overlay bind.
- Own KDC RSA keys pinned 4096-bit (stock-client 2048 accepted for
  verify-interop only).
- Secrets never on argv/inherited env (`--*-stdin` / systemd-creds; env
  scrubbed at boot); enrollment passcode piped via stdin.
- Nebula debug-SSH for path introspection bound loopback-only, key-auth
  (Ed25519); GPG-signed RPM + `SHA256SUMS`/`.asc`; full GPL-3.0 text shipped
  + a `SECURITY.md` disclosure policy.

[Unreleased]: https://github.com/matthewmackes/magic-mesh/compare/magic-mesh-v12.0.0...HEAD
[12.0.0]: https://github.com/matthewmackes/magic-mesh/releases/tag/magic-mesh-v12.0.0
[10.0.0]: https://github.com/matthewmackes/magic-mesh/releases/tag/magic-mesh-v10.0.0

<!-- 11.4.0–11.4.5 were rolling DRM-shell cutover cuts deployed to fleet machines but not git-tagged; they carry no release-tag link. -->

