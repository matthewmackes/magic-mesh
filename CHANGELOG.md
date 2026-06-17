# Changelog

All notable changes to Magic Mesh are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning is the
single workspace version (`[workspace.package] version`, every crate
inherits). Release tags are **`magic-mesh-v<version>`**; the RPM NEVRA pairs
that version with the packaging `release` field (bumped alone for
asset-only changes).

Pre-release history (the E1–E11 epochs, the MackesWorkstation split, and the
v2.x–v6.x phase plans) lives in the git log and `docs/design/` — this file
starts at the first packaged release line.

## [Unreleased]

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

### Fixed
- **Notification CLI fan-out (NOTIFY-DIST-3):** `mde-bus publish` now flattens
  hierarchical topics (slashes → `_`) before the ntfy POST, so CLI-published
  alerts reach the broker on the same topic the subscribers watch.

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
