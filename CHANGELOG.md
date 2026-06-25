# Changelog

All notable changes to MCNF (Mackes Cosmic Nebula Fedora) are documented here. The 10.0.x series is codenamed "Magic Mesh"; historical entries below predate the 2026-06-17 rebrand. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning is the
single workspace version (`[workspace.package] version`, every crate
inherits). Release tags are **`magic-mesh-v<version>`**; the RPM NEVRA pairs
that version with the packaging `release` field (bumped alone for
asset-only changes).

Pre-release history (the E1–E11 epochs, the MackesWorkstation split, and the
v2.x–v6.x phase plans) lives in the git log and `docs/design/` — this file
starts at the first packaged release line.

## [Unreleased]
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

[Unreleased]: https://github.com/matthewmackes/magic-mesh/compare/magic-mesh-v10.0.0...HEAD
[10.0.0]: https://github.com/matthewmackes/magic-mesh/releases/tag/magic-mesh-v10.0.0
