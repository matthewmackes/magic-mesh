//! v2.0.0 Phase A.2 (locked 2026-05-19) — in-process worker pool.
//!
//! The unified backend folds 8 standalone Python daemons (and one
//! Rust bridge) into a single `mackesd` process. Each former-daemon
//! becomes a [`Worker`] task driven by [`Supervisor`]. Worker bodies
//! land in Phase B; this module ships the trait surface, the shutdown
//! plumbing, and the per-worker join semantics every Phase B worker
//! will share.
//!
//! Design choices (locked via the 2026 stack survey 2026-05-19):
//!
//! * **Async runtime: tokio** (full features). The legacy reconcile
//!   loop (`crate::worker`) keeps its `std::thread` model — they
//!   coexist by living in separate scheduler domains.
//! * **Per-worker future: native `async fn` via `async_trait`**.
//!   Object-safety matters because the supervisor stores
//!   `Box<dyn Worker>`; native async-fn-in-trait drops object safety,
//!   so we keep `async_trait` for this trait only.
//! * **Restart policy: Erlang OTP-ish**. Phase B layers the
//!   `task-supervisor` crate (already a dep) on top of this trait so
//!   each worker gets per-task restart back-off + health-tick
//!   semantics. Phase A ships only the *contract*; the supervisor
//!   here is the minimal "spawn-and-shutdown" version.
//!
//! All public types are gated behind the `async-services` feature so
//! a fresh checkout that only builds the sync read-API doesn't pull
//! tokio into its dep tree.

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tracing::{error, info, warn};

/// EFF-24 — live per-worker status the supervisor maintains and the
/// Bus `healthz` (+ the metrics exporter) read. One row per spawned
/// worker, updated at every lifecycle transition.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkerStatus {
    /// Worker name (the `Worker::name()` kebab/snake token).
    pub name: &'static str,
    /// True while the worker's `run()` future is live (incl. between
    /// restarts only during the back-off sleep — set false on exit).
    pub alive: bool,
    /// Restart count since daemon start.
    pub restarts: u32,
    /// True while the ENT-6 circuit breaker is OPEN — the worker
    /// tripped and is cooling down between half-open recovery trials
    /// (mackesd-05). Cleared once a trial proves stable (breaker
    /// closes) so a transient trip stops counting against readiness;
    /// a still-broken worker keeps it set while it backs off to the
    /// cooldown cap.
    pub breaker_tripped: bool,
    /// Cumulative count of genuine breaker TRIPS (closed→open
    /// transitions) since daemon start. Unlike `breaker_tripped` (a
    /// live gauge that clears on recovery), this only grows — it is the
    /// scrapeable `mackesd_breaker_trips_total{worker=…}` failure
    /// signal the metrics exporter surfaces (test-obs-9). Half-open
    /// relapses are deliberately NOT counted here (same trip episode).
    pub breaker_trips: u32,
    /// Outcome of the most recent `run()` exit: `Some(true)` clean,
    /// `Some(false)` error/panic, `None` while still on first run.
    pub last_exit_ok: Option<bool>,
}

/// Shared map: worker name → live status. `std::sync::Mutex` (brief
/// lock-and-update, never held across await).
pub type WorkerStatusMap = Arc<std::sync::Mutex<HashMap<&'static str, WorkerStatus>>>;

/// Fresh empty status map for [`Supervisor::set_status_map`].
#[must_use]
pub fn new_status_map() -> WorkerStatusMap {
    Arc::new(std::sync::Mutex::new(HashMap::new()))
}

/// EFF-24 — apply one status mutation for `name`, inserting the row
/// on first touch. No-op when no registry is attached (`None`).
fn update_status(
    map: &Option<WorkerStatusMap>,
    name: &'static str,
    f: impl FnOnce(&mut WorkerStatus),
) {
    if let Some(map) = map {
        let mut g = map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(g.entry(name).or_insert(WorkerStatus {
            name,
            alive: false,
            restarts: 0,
            breaker_tripped: false,
            breaker_trips: 0,
            last_exit_ok: None,
        }));
    }
}

/// EFF-24 — the readiness reduction over a status map: every spawned
/// worker alive and no breaker tripped. (The daemon-level `ready`
/// verdict ANDs this with the store/audit health — see
/// `ipc::shell::build_healthz`.)
#[must_use]
pub fn workers_ready(map: &WorkerStatusMap) -> (u32, u32, u32) {
    let g = map
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let total = u32::try_from(g.len()).unwrap_or(u32::MAX);
    let alive = u32::try_from(g.values().filter(|w| w.alive).count()).unwrap_or(u32::MAX);
    let tripped =
        u32::try_from(g.values().filter(|w| w.breaker_tripped).count()).unwrap_or(u32::MAX);
    (alive, total, tripped)
}

// arch-7 (2026-07-11) — the shared worker-pool contract (`Worker` +
// `ShutdownToken`) moved into the `mde-worker-core` crate so worker
// IMPLEMENTATIONS can live in their own crates (e.g. the per-seat browser
// workers in `mde-browser-workers`) without a circular dependency back on this
// bin crate. Re-exported here so every in-crate worker + the ~120 call sites
// keep referencing `super::{Worker, ShutdownToken}` / `crate::workers::Worker`
// unchanged.
pub use mde_worker_core::{ShutdownToken, Worker};

// v2.0.0 Phase B workers reparented under workers/. Each is a thin
// adapter over an existing sync implementation today; they grow real
// bodies as Phase B fills in.
pub mod ansible_pull;
// EPIC-SYNC-APP-CONFIG (Q26) — native-Rust app-config sync
// (Sublime Music / Delfin). Replaces the retired `media_sync`
// subprocess worker + the Python `media_sync_daemon.py` it drove.
pub mod app_sync;
// BOOKMARKS-2 — the mesh-synced bookmarks worker: per-node append-only op
// segments in the encrypted Syncthing share, replay-merge through the pure
// mde-bookmarks CRDT, snapshot/prune for bounded growth, in-memory index +
// periodic-flush durability, and offline-first editing. Drains
// action/bookmarks/* + publishes state/bookmarks/*. No external transport to
// fake — file I/O against the same /mnt/mesh-storage share the ssh-gossip /
// chat workers use; the honest gate is the shared_root_writable offline state.
pub mod bookmarks;
// BOOKMARKS-7 — the mesh-wide ad-blocker worker: replicates each node's serialized
// filter-list store over the encrypted Syncthing share (per-node single-writer
// store.json, LWW-merged into one converged store), the leader compiles the
// converged store into the shared engine blob + refreshes the enabled lists from an
// airgap-safe local mirror (honest Staleness fallback, never fabricated), and drains
// action/adfilter/{allow,block} into the mesh-synced per-site allowlist. Publishes
// state/adfilter/<node>. No external transport to fake — file I/O against the same
// /mnt/mesh-storage share the bookmarks / ssh-gossip / chat workers use.
pub mod adfilter;
// BOOKMARKS-8 — the mesh-wide browser/ad-blocker POLICY worker. Replicates the
// operator-authored fleet policy doc over the encrypted Syncthing share (per-node
// single-writer doc.json, LWW-converged by the newest authored stamp), folds it
// for THIS node's role, and enforces at the browser launch/spawn seam: it refuses
// to spawn the browser on a disallowed role, injects the forced ad-blocker + URL
// allowlist + custom filter lists on launch, and rejects out-of-policy navigate /
// adblock-off actions (draining action/browser/*). Disable stops the browser-data
// sync + hides the surface but retains the node-local data (no destructive wipe).
// Publishes state/browser-policy/<node> for the Workbench fleet view. No external
// transport to fake — file I/O against the same share the adfilter worker uses.
pub use mde_browser_workers::browser_policy;
// BROWSER-DD-6 — Browser passkey/WebAuthn ceremony owner. Drains strict
// action/browser/passkey handoffs, persists pending challenges locally, mirrors
// them into the Syncthing-backed workgroup root, and publishes honest pending or
// error state without minting fake credentials.
//
// arch-7 (2026-07-11) — the 11th and final browser worker, now moved into
// `mde-browser-workers` alongside the other 10. It seals platform passkey
// private keys with the audited Argon2id + XChaCha20-Poly1305 primitives that
// ALSO back the CA disaster-recovery bundle and the VPN secret-store fallback;
// those primitives (+ `age_key_path`) were factored into the shared leaf crate
// `mde-seal`, so the worker now depends on `mde-seal` directly rather than back
// on this daemon. Re-exported here so `crate::workers::browser_passkeys::…`
// spawn sites resolve unchanged.
pub use mde_browser_workers::browser_passkeys;
// BROWSER-DD-12 — Browser external-protocol owner. Drains
// action/browser/protocol handoffs for external schemes Browser refused to
// navigate, validates mailto/email and magnet/transfers routes, and publishes
// retained route status/events without faking the downstream surface.
pub use mde_browser_workers::browser_protocol;
// BROWSER-DD-12 — Browser platform-share owner. Drains
// action/browser/share handoffs, validates Peer/Email/QR routes, and publishes
// retained route status/events without faking downstream delivery.
pub use mde_browser_workers::browser_share;
// BROWSER-DD-11 — Browser voice-command/dictation STT owner. Drains
// action/browser/voice-command, invokes a locally configured offline STT/capture
// command when present, emits bounded transcript events, and publishes honest
// unavailable/error/transcribed state for the shell.
pub use mde_browser_workers::browser_voice_command;
// BROWSER-DD-12 — Browser private offline/mesh translation owner. Drains
// action/browser/translate, invokes a locally configured offline/mesh translation
// command when present, emits bounded translation events, and publishes honest
// unavailable/error/translated state for the shell.
pub use mde_browser_workers::browser_translate;
// BROWSER-DD-12 — Browser offline/mesh cache owner. Drains explicit Browser
// cache snapshots, validates private offline/mesh payloads, writes local durable
// records, and mirrors them into the Syncthing-backed workgroup root.
pub use mde_browser_workers::browser_offline_cache;
// BROWSER-DD-12 — Browser CEF security-update status owner. Watches the
// packaged fast-update manifest and active CEF runtime, publishing an honest
// current/missing/mismatch posture for the independent browser-engine update path.
pub use mde_browser_workers::browser_security_update;
// BROWSER-DD-12 — Browser idle-tab suspend owner. Drains shell-published
// action/browser/tab-suspend handoffs after the shell has stopped the inactive
// helper, validates the payload, and publishes retained suspend status/events.
pub use mde_browser_workers::browser_tab_suspend;
// KDC-MESH-6 — Seat-side consumer for phone-originated remote input. Drains the
// KDC worker's action/seat/remote-input handoff, validates the event, and invokes
// the configured uinput/seat helper when available.
pub mod seat_remote_input;
// BROWSER-DD-7 — Browser session-sync owner. Drains the Browser's
// action/browser/session-sync snapshots, validates the restore-compatible JSON
// shape, persists the latest local copy, and mirrors it to the Syncthing-backed
// workgroup root as browser-session-sync/<host>/latest.json. No external
// transport to fake — Syncthing replicates the file plane.
pub use mde_browser_workers::browser_session_sync;
// BROWSER-DD-11 — Browser read-aloud/TTS owner. Drains bounded
// action/browser/read-aloud page-text requests, invokes a locally configured
// offline TTS command when present, and publishes honest unavailable/error/spoken
// state for the shell.
pub use mde_browser_workers::browser_read_aloud;
pub mod heartbeat;
// mackesd-06 — the legacy reconcile loop (`crate::worker`) brought UNDER
// the supervisor: an async adapter that runs the same blocking tick on
// its dedicated OS thread but gets restart-on-panic + breaker treatment
// like every other worker (was a raw std::thread with no supervision).
pub mod reconcile;
// OV-7.a (v2.6) — Health reconciler. Reads each known peer's
// QNM-Shared heartbeat.json on a 5 s tick, applies the
// `telemetry::health_state_from_age` threshold table, writes
// the result back into `nodes.health`, and fires the
// `dev.mackes.MDE.Nebula.Status.PeerStateChanged` signal on
// transitions (so the Workbench Overview / applets / mde-files
// re-probe without polling). Quietly skips peers without a
// heartbeat file (peer hasn't enrolled yet) and the local peer
// (heartbeat-self is unreachable by definition).
pub mod health_reconciler;
// EFF-9 — Prometheus textfile exporter. Snapshots store-derivable
// control-plane gauges into <textfile_collector>/mackesd.prom on a
// 30 s cadence; the renderer (metrics::write_textfile) existed with
// no production caller until this worker.
pub mod metrics_exporter;
// MEDIA-3 — exact-role-gated Navidrome supervisor for Lighthouse_Media.
pub mod media_navidrome;
// EFF-20 — timeout-bounded subprocess execution shared by the workers
// that shell out on a tick, so a hung child can't pin a runtime thread.
pub mod proc;
// KDC2-6.6 — legacy `kdc_bridge` retired alongside the upstream
// kdeconnectd wrapper. The native KDC host worker
// (`workers::kdc_host`) replaces it in the v2.1+ stack.
pub mod kdc_host;
pub mod mdns_relay;
pub mod mesh_latency;
// MESHMAP-6 (2026-06-27) — real per-link byte counters. Maintains an
// nftables accounting table (`inet mde_linkacct`) with one passive counter
// per peer overlay IP per direction on the Nebula interface, reads byte
// deltas on a 5 s tick, and publishes per-link tx/rx rates to
// ~/.cache/mde/link-traffic.json. The mesh wallpaper / Peers-Map flow
// particles consume it as the REAL per-edge source, falling back to the
// per-node `sample_flows` proxy (MESHMAP-3) when the cache is absent
// (no nft / non-root / pre-delta). Cheap at idle: one `nft list` + one
// reconcile per tick, never a busy loop.
pub mod link_traffic;
// FILEMGR-5 — the mesh-mount worker: owns the sshfs mount lifecycle over the
// Nebula overlay for the Files surface. Drains `action/mesh-mount/<host>` +
// publishes `state/mesh-mount/*` (home-vs-escalated scope, idle-unmount,
// reconnect+backoff, frozen-mount recovery). The live sshfs/fusermount impl is
// integration-gated behind the injectable `MountBackend` seam (§9, §7); the
// planning + state-machine folds are pure + unit-tested.
pub mod mesh_mount;
// TERM-7 — the mesh PTY-broker worker: owns the remote-shell lifecycle over the
// Nebula overlay for the mde-term-egui terminal surface. Drains
// `action/pty/<peer>` (typed verb — open/write/resize/close, each carrying the
// client-minted session id) + publishes an append log on `state/pty/<id>`
// (base64 output chunks + the terminal exit). Opens a real remote shell via
// `ssh -tt` on the sealed mesh key (FILEMGR-6, reused from mesh_mount), with
// honest typed gating on an unreachable peer / unprovisioned key (never fakes a
// session) + idle/dead-session reap. The live ssh impl is integration-gated
// behind the injectable `PtyBackend` seam (§9, §7); the plan + state-machine +
// reap folds are pure + unit-tested.
pub mod mesh_router;
pub mod pty_broker;
// TRANSFERS-1 — the `transfers` worker: the daemon-owned queue/ledger/verb/state-
// machine spine of the Transfers surface (docs/design/transfers-surface.md). Owns a
// typed TransferJob envelope, a persistent node-local ledger, the submit/cancel/
// pause/resume/list verbs, and the parallel cap — with an injectable LaneRunner seam
// the per-protocol lanes (TRANSFERS-2..6) implement (honestly gated until then).
pub mod transfers;
// NF-3.4 (v2.5) — Nebula supervisor worker (CA mint +
// role-marker management + bundle-watch + systemctl
// reload).
pub mod nebula_supervisor;
// NF-3.6.c (v2.5) — Auto-signer worker. Polls QNM-Shared for
// pending-enroll CSRs + calls nebula_enroll::sign_pending_csr
// on each new one, replacing the manual `mackesd ca sign-csr`
// step for the common case (single-lighthouse mesh with an
// active CA).
pub mod nebula_csr_watcher;
// PLANES-24 W63 — scheduled one-puller mirror sync. Every node writes its
// dnf .repo to self-serve (W62); only the leader pulls upstream + indexes,
// Syncthing replicating the result fleet-wide.
pub mod mirror_syncd;
// NF-1.5 (v2.5) — Lighthouse-side TCP/443 covert listener.
// Binds the TLS 1.3 listener on :443, spawns one demux pump
// per accepted stream (TLS ↔ UDP 127.0.0.1:4242). Inner Nebula
// stack runs unmodified.
pub mod nebula_https_listener;
// ONBOARD-2 — the lighthouse `/enroll` rustls HTTPS listener. Serves
// network bootstrap for NAT'd peers (MESH-1 fix): POST /enroll signs a
// peer CSR via the shared core + returns the bundle, authed by the
// single-use bearer; endpoint identity is the token-pinned self-signed
// cert. Spawned under am_lighthouse; warn-and-exit on peer-role boxes.
pub mod nebula_enroll_listener;
// ONBOARD-6 — continuous leader election: renews the lease on
// <QNM-Shared>/.mackesd-leader.lock every 20s so a leader is always
// elected (the upgrade watcher only acquired it opportunistically, so
// steady-state meshes had NO LEADER + dark leader-gated surfaces).
pub mod leader_election;
// NF-18.4 (v2.5) — Daily encrypted CA backup worker. Writes
// sealed (Argon2id + XChaCha20-Poly1305) bundles to
// QNM-Shared/<self>/mackesd/ca-backup.enc on a 24h tick.
// Opt-in: requires MDE_BACKUP_PASSPHRASE env var; silently
// skips when unset.
pub mod nebula_ca_backup;
// PRINT-2..PRINT-6 + PRINT-8 (v5.0.0) — auto CUPS print sharing +
// sync. Converges fleet printers via mesh-storage (write-own-file
// + import-union as `<queue>@<host>`); jobs route through the host
// peer's CUPS over the overlay. Headless + full only (lighthouse
// skips at spawn). Silent no-op without cups/lpadmin or before
// Nebula enrollment.
pub mod cups_sync;
// FWMON-2..4 (v5.0.0) — firewall-denied event monitor. Reads
// kernel journal entries logged by firewalld's LogDenied=all
// setting, filters overlay + established traffic, appends
// net-new denials to <mesh-storage>/firewall/<host>.jsonl,
// trims 7-day window, and fires Bus alert on threshold.
// Separate from `firewall_preset` (port-open convergence).
pub mod firewall_monitor;
/// NOTIFY-SRC — SELinux AVC denials → the `fleet/sec/selinux/<host>` alert lane.
pub mod selinux_monitor;
// VIRT-1 (v5.0.0) — unified KVM + Podman compute inventory.
// Polls `virsh list --all --uuid` + `virsh dominfo`/`domblklist`/
// `domstats` for KVM guests and `podman ps`/`podman stats` for
// containers on a 10 s tick; publishes per-peer inventory to
// `compute/inventory/<peer-nebula-addr>` per docs/design/v5.0.0-
// compute.md §3. Silent no-op when virsh/podman are absent.
pub mod compute_registry;
// ROUTER-3/4 — the router_registry worker: per-node + always-on. Discovers the
// node's primary router/firewall (lowest-metric default route + gateway MAC),
// matches a sealed `router/<mac>` cred + fingerprints it over the Vyatta CLI,
// and publishes a RouterEntry to `mesh/devices/router/<mac>` + the QNM-Shared
// `<host>/router-registry.json`. Design: docs/design/router-control.md.
pub mod router_registry;
// WL-RUN-006 — the router_action worker: the privileged firewall-edit executor.
// Drains this node's replicated `action/router/<self>/` dir, gates each edit
// behind a typed-confirm token, wraps the Vyatta apply in commit-confirm
// (auto-revert), and hash-chain audits every edit. Rank-0 universal like
// device_control; the live mutation is operator-gated (MDE_ROUTER_ACTION_LIVE).
// Design: docs/design/router-control.md (mutations fast-follow).
pub mod router_action;
// MEDIA-7 — the media_registry worker: on a Lighthouse_Media node only
// (capability-gated on MEDIA-1's Capability::Media), register the local
// navidrome/media instance into the mesh service registry — the per-peer
// Bus topic `mesh/services/media/<peer>` + the replicated QNM-Shared plane
// `<host>/media-registry.json` (same registry plane the other published
// services use) — with a per-instance health field.
pub mod media_registry;
// MEDIA-pkg-2 — the navidrome_supervisor worker: on a Lighthouse_Media node only,
// adopt + self-heal the mcnf-navidrome.service systemd unit (restart-if-down,
// re-provision-if-missing via the RPM-shipped setup-media-navidrome).
pub mod navidrome_supervisor;
// MEDIA-8 — the Workstation half of the media birthright: read the published
// shared account off the registry plane and idempotently write the desktop
// user's airsonic-creds.json so mde-music auto-browses (no first-run connect).
pub mod music_autoconfig;
// APPS-LIVE-1 — the apps_running worker: mirror this node's set of
// currently-running launchable apps to <QNM-Shared>/<host>/running-
// apps.json so the launcher can badge every entry "running on <host>"
// mesh-wide (same replicated plane as compute-inventory.json; the bus
// is per-node). Process ↔ .desktop match, reachable from the root
// daemon without a per-seat compositor probe.
pub mod apps_running;
// APPLAUNCH-5 — the apps_installed worker: mirror this node's set of
// INSTALLED launchable .desktop apps to <QNM-Shared>/<host>/apps-
// installed.json so the Front Door's Mesh filter can answer a focused
// peer's app set on demand (the launch-on-peer target list) without a
// blocking live RPC — the on-demand `action/apps/peer-list` verb reads
// this replicated file locally (lazy-mesh: a dead peer never blocks).
pub mod apps_installed;
// VIRT-5 (v5.0.0) — VM Nebula cert signing via Bus. Every peer
// drains `action/compute/cert-sign-request`; on the CA peer
// (detected by ~/.config/mde/nebula/ca.key) calls `nebula-cert
// sign` and replies on `reply/<ulid>`; non-CA peers advance the
// cursor and skip. Topic shape locked to `action/<domain>/<verb>`
// per Q96 + rpc.rs convention (design doc §3's per-ULID notation
// reinterpreted accordingly).
pub mod cert_authority;
// WL-ARCH-001 Phase B — the OpenTofu + Ansible cloud backend worker (the
// successor to the deleted `openstack` worker tree). Drains `action/cloud/*`
// verbs (leader-gated), shells OpenTofu (`infra/tofu/cloud`) + Ansible + virsh
// with live mutation operator-gated behind `MDE_CLOUD_APPLY=1`, and publishes
// `state/cloud/<node>` (provider health + resource roster via the neutral
// `mackes_mesh_types::cloud` types). Rank-0 universal like service_aggregator.
pub mod cloud;
// Rolling Node — the `vehicle` worker: the workstation-side adapter that
// SSH/HTTP-polls a mobile Sierra AirLink MG90 (oMG) gateway and publishes a
// latest-wins `state/vehicle/<node>` mirror (GPS/IMU + WAN + MCU power via the
// neutral `mackes_mesh_types::vehicle` types). Rank-0 universal like `cloud`, but a
// genuine no-op on the nodes with no gateway configured (`MDE_VEHICLE_GATEWAY`
// unset). Mirrors `cloud`'s injectable-transport + bus-mirror lifecycle.
pub mod vehicle;
// WL-FUNC-012 / OVERLAY-10 — opt-in workstation-side, keyless USGS earthquake
// adapter. Polls the all-hour GeoJSON feed over rustls and publishes normalized
// latest-wins `state/overlay/usgs-earthquakes/<node>` snapshots. Unconfigured is
// an idle no-op; fetch failures retain the honestly aging last-good snapshot.
pub mod earthquake_overlay;
// WL-FUNC-012 / OVERLAY-1 — opt-in workstation-side NWS active-alert adapter;
// resolves null alert geometry through affectedZones and publishes normalized
// latest-wins `state/overlay/nws-alerts/<node>` snapshots.
pub mod nws_alert_overlay;
pub mod nws_forecast_overlay;
// WL-FUNC-012 / OVERLAY-8 — opt-in, vehicle-scoped adsb.lol point adapter;
// retains only fresh direct/rebroadcast low-altitude positions and publishes a
// bounded latest-wins `state/overlay/adsb-aircraft/<node>` snapshot.
pub mod aircraft_overlay;
pub mod caltrans_camera_overlay;
// WL-FUNC-012 / OVERLAY-9 — opt-in MBTA GTFS-Realtime vehicle positions,
// filtered against a fresh local vehicle point before latest-wins publication.
pub mod transit_overlay;
// VIRT-7 (v5.0.0) — per-network firewalld port forwarding. Each
// peer subscribes to `compute/{expose,unexpose}/<own-peer-addr>`
// and writes firewalld rich rules per selected network
// (mesh→trusted, lan→public, wan→detected). Publishes the
// in-memory active-rule shadow set to
// `compute/exposed/<own-peer-addr>` for the Workbench display.
// Silent no-op when firewall-cmd is absent.
pub mod compute_expose;
// VIRT-8.a (v5.0.0) — cold VM migration source-side worker.
// Each peer drains `action/compute/migrate`; when own nebula
// IP == request.source_peer, runs virsh shutdown + 120s SHUTOFF
// poll + rsync --compress over Nebula + publishes
// `event/compute/migrate-ready` + virsh undefine. VIRT-8.b
// (target-side compute_provision handler) ships with VIRT-6.
pub mod compute_migrate;
// VIRT-21 (v5.0.0) — desktop toasts on VM lifecycle changes. Drains
// every `compute/event/<peer>` topic + fires `notify-send`.
pub mod compute_event_toast;
// MESH-A-1 (v5.0.0) — per-peer network assessment. Collects the 9
// items from docs/design/v6.0-mde-portal.md §7.1 (wifi / arp /
// gateway-dns / public-ip / speedtest / ipv4-6 / mtu / tunnel /
// subnet) on an hourly tick, writes a timestamped JSON snapshot to
// ~/.local/share/mde/netassess/<host>/<iso>-<hash>.json, and trims
// the 30-day rolling window. Pure parsers per item; shell-outs
// degrade to None when a tool is absent.
pub mod netassess;
// MESH-A-4.c.2 (v5.0.0) — surrounding_worker. Sweeps the LAN for
// non-mesh-peer neighbours (mDNS + ARP-MAC + OUI) every 10 min and
// writes a per-peer snapshot under ~/.local/share/mde/surrounding/.
pub mod surrounding_worker;
// VOIP-4.b (v5.0.0) — voip_rtt_worker. Broadcasts this peer's Vitelity-link
// RTT to voip/link-rtt/<peer> every 60s for the dialer route override.
pub mod voip_rtt_worker;
// MESH-A-5.2 (v5.0.0) — mesh_firewall. Reconciles firewalld source-DROP
// rich-rules against the mesh-synced Blocked-host consensus every minute.
pub mod mesh_firewall;
// VIRT-6 (v5.0.0) — compute_provision. Drains
// `compute/create/<own-addr>`: ensures the mde-vms pool (VIRT-3),
// allocates a per-peer /24 VM IP, requester-side nebula-cert keygen
// + cert-sign RPC (VIRT-5), builds the NoCloud cloud-init seed,
// virt-installs (libvirt-managed virtiofs when share_meshfs +
// mounted), acks on compute/create-ack/<ulid>, fires an immediate
// compute/inventory publish. Guest config via
// nebula_supervisor::render_guest_config_yaml.
pub mod compute_provision;
// INST-11 + INST-12 + INST-13 (v2.7) — fleet upgrade-barrier
// worker. Runs on every peer: watches `<mesh-home>/upgrade-
// intent/*.json` (written by `mde-update --coordinate`), runs
// `dnf upgrade mde-core` on its own schedule + marks itself
// `ready`, fires `mde-install --yes` once quorum + grace are
// met (marking `complete`), and — when leader — deletes
// fully-complete intent files after a +24h grace. Silent
// no-op when the upgrade-intent dir doesn't exist.
pub mod upgrade_intent_watcher;
// MON-4 (v2.6) — alert relay worker. Polls
// `~/.local/share/mde/alerts/*.json` (written by
// `mde-alert-emit` via Netdata's `health_alarm_notify.conf`
// custom-sender hook) on a 2s tick + forwards each new
// event as an FDO desktop notification via `notify-send`.
// Deduplicates via the deterministic-ULID `id` field so
// idempotent re-emissions don't re-toast.
pub mod alert_relay;
// MON-1.b (v2.6) — Netdata aggregator-IP publisher. On
// every tick (a) checks leader-state via the role-host
// marker; if leader, publishes a JSON pointer
// {node_id, overlay_ip, epoch_s} under
// `<workgroup_root>/<self>/mackesd/netdata-aggregator.json`. (b)
// always scans `<workgroup_root>/*/mackesd/netdata-aggregator
// .json`, picks the freshest pointer + mirrors the IP to
// `/var/lib/mackesd/netdata/aggregator-ip` + rewrites
// `/etc/netdata/netdata.conf`'s `[stream]` block + reloads
// netdata. Fail-soft per the v2.6 MON-1 design lock —
// missing/unreachable aggregator strips the `[stream]`
// block so netdata falls back to local-only with the 7-day
// dbengine retention `apply_netdata_monitor` locked.
pub mod netdata_aggregator;
// EPIC-MESH-PROBE (MESH-PROBE-4) — scheduled two-tier nmap probe
// worker; writes the per-peer probe-inventory.json + announces
// probe/changed on the Bus. Spawned in run_serve; reuses probe_nmap.
pub mod probe;
// SUBAUDIT-D2 — the missing PeerProbe producer: gathers this node's
// hardware probe + writes it to the replicated directory so the
// Workbench Hardware panel renders the fleet. Spawned in run_serve.
pub mod hardware_probe;
// DEVMGR-1 — the device-inventory enumeration engine the `hardware_probe`
// worker calls on its tick (NOT a new worker — lock #16). Walks the full Linux
// hardware taxonomy sysfs-first + publishes `device-inventory/<host>.json` for
// the About → Device-Manager surface (docs/design/about-device-manager.md).
pub mod device_inventory;
// E12-19 (Construct host controls) — mirrors this node's seat snapshot to
// state/host/<node>/seat and executes remote typed verbs (volume/BT/
// display/power) behind the allowlist + safety interlocks. Runs on every
// node; spawned in run_serve.
pub mod host_state;
// WL-FUNC-012 / OVERLAY-2 — keyless IEM/NWS animated NEXRAD tiles.
#[cfg(feature = "async-services")]
pub mod iem_radar_overlay;
pub mod traffic_overlay;
pub mod wildfire_overlay;
// notification_relay retired in BUS-4.2 (2026-05-26). Cross-peer
// notification routing is now handled by the BUS-4.4 FDO bridge:
// every Notify call publishes to `fdo/<app>` on the Mackes Bus,
// and every peer subscribes via the standard Bus path.
// perf retired 2026-05-27 (TUNE-3.b): the Rust port of
// `mackes/mesh_perf.py`'s read-only sysfs surface was destined for
// the Workbench Mesh Performance panel (Python GTK), which retires
// under EPIC-RETIRE-PY-WORKBENCH. No Workbench panel
// equivalent exists; if a future v2.x panel needs the
// same sysfs reads, restoring from `git log -p
// crates/mackesd/src/workers/perf.rs` is trivial. No live consumer
// of the pure helpers (`kernel_module_loaded` / `current_mtu` /
// `gso_enabled` / etc.) existed in tree.
pub mod remmina_sync;
// NF-21.1 — owns the /etc/ssh/sshd_config.d/mackes-mesh.conf
// drop-in that binds sshd to this peer's Nebula overlay IP.
// Replaces mesh_nebula.py::write_sshd_overlay_bind so the
// Python module can fully retire (DEAD-2.14 plan).
pub mod sshd_overlay_bind;
// SVC-2 (Q60) — gossips each peer's user ed25519 SSH pubkey through the
// replicated workgroup root into every peer's authorized_keys managed
// block, making peer-to-peer SSH passwordless mesh-wide.
pub mod ssh_pubkey_gossip;
// PD-9 / FPG — drives `magic-fleet reconcile` on cadence + on nudge.
pub mod fleet_reconcile;
// PLANES-9 — runs jobs targeting this box locally (execution-gated).
pub mod job_exec;
// PD-13 — presence-transition alerts riding the alert_relay pipeline.
pub mod presence_watch;
// SUBSTRATE-10 — etcd WATCH worker: instant peer-down / leader-change alerts
// pushed (not polled) off `/mesh/peers/` + `/mesh/leader` watch streams.
pub mod etcd_watch;
// SEC-5 / KDC2-4 — relays neighbors' paired phones mesh-wide.
pub mod mesh_shunt;
// PLANES-18 — feeds <host>.mesh into resolved + /etc/hosts.
pub mod mesh_dns;
// PLANES-15 — converges the baseline's netstate desired-state under a
// rollback checkpoint with a post-apply overlay-reachability self-test.
pub mod netstate_apply;
// PLANES-19 — the overlay-reachability validation suite: participate in
// runs, leader mints nightly/run-now + writes the pass/fail verdict.
pub mod validation_suite;
// PD-11 — executes descriptor-gated container/VM lifecycle requests.
pub mod lifecycle_exec;
// DEVMGR-8 — executes privileged device-control ops (enable/disable, reload
// module, rescan bus) on this node for the Device-Manager surface, over the
// replicated fleet/device-control/<self> request dir.
pub mod device_control;
// NF-21.3 — owns the firewalld preset that opens Nebula's
// UDP/4242 (all peers) + TCP/443 (lighthouses) inbound. Replaces
// mesh_nebula.py::apply_nebula_firewall_preset so the Python
// helper can retire (DEAD-2.14 plan).
pub mod firewall_preset;
// CONNECT-3 — exposure-driven (additive) firewall enforcement.
pub mod connect_firewall;
// FARM-AUTO-1 — build-farm orchestrator: bridges the farm job lifecycle onto the Bus.
pub mod farm_orchestrator;
// DATACENTER-5 — datacenter orchestrator: samples the DC substrate (DO/Xen/gateway)
// onto the Bus as `event/dc/<kind>/<id>` for the Workbench Datacenter plane.
pub mod datacenter_orchestrator;
// DATACENTER-7 (audit half) — passive audit subscriber: watches the `action/dc/*`
// Bus lanes and emits one append-only `event/dc/audit/<ulid>` record per request,
// without touching the action handlers. Leader-gated; dedups on request ulid.
pub mod dc_auditor;
// DATACENTER-6 — passive async job-status tracker: watches the `action/dc/*` Bus
// lanes + their `reply/<ulid>` replies and emits one `event/dc/job/<ulid>` event
// per status transition (pending→ok/error), without touching the action
// handlers. Leader-gated; dedups on (ulid, status).
pub mod dc_jobs;
// DATACENTER-24 — passive care-and-feeding health checker: on a 30 s tick probes
// each configured Xen dom0's SSH reachability, the SUBSTRATE-V2 etcd `/health`,
// and the mesh secret-store helper, and emits one `event/dc/health/<check>` per
// check (deduped on status), without touching the substrate it watches.
// Leader-gated; a pure side-observer.
pub mod dc_health;
// DATACENTER-23 — scheduled DR backups: a leader-gated periodic worker that runs
// `automation/dr/dr-backup.sh` at most once per `MCNF_DR_INTERVAL_SECS` (default
// daily) and publishes the outcome to `event/dc/dr/last`
// ({"status":"ok"|"fail",…}). Coarse tick (~5 min) decides via the pure `due`
// helper; the leader runs exactly one backup per interval mesh-wide.
pub mod dr_scheduler;
// DATACENTER-12 (scheduled-snapshot executor) — the missing consumer of the
// Storage tab's "Save schedule". A leader-gated periodic worker that reads each
// SR's latest `event/dc/snap-schedule/<sr>` config off the Bus, decides per-tick
// whether each SR is due per its cadence, and when due takes the snapshot by
// reusing the EXISTING storage `xe vdi-snapshot` path over the mesh-key SSH
// (the same `xen_ssh_key`/`xen_dom0s` injection-guarded, dom0-allow-listed
// contract `ipc::storage_ops` uses). After snapshotting it enforces retention by
// destroying its OWN (prefix-tagged) oldest snapshots beyond the configured
// count — never an operator's hand-made snapshot — emits a run result to
// `event/dc/snap-schedule-run/<sr>`, and alerts on failure via the alert_relay
// lane. Degrades cleanly (no Bus / no schedule / no dom0 → idle, no panic).
pub mod dc_snap_scheduler;
// DATACENTER-20 — passive promotion tracker: publishes the version running at
// each promotion stage (Build→Eagle→DO) to `event/dc/promote/<stage>` so the
// Workbench Datacenter plane can render the promotion matrix. Leader-gated;
// dedups on (stage, version+status). Build version is the newest release RPM
// (else `git describe`); Eagle/DO are honest `"unknown"` placeholders until
// those hosts are reachable.
pub mod dc_promote;
// VPN-GW-1 — per-node commercial-VPN tunnel engine (WireGuard/OpenVPN baseline).
// DDNS-EGRESS-3 — the dynamic-DNS reconcile loop + the DigitalOcean DnsWriter
// adapter. Tails `event/vpn/signals` (VPN-GW exit-IP changes) + runs a periodic
// WAN check, resolves each `[ddns]` record's live SourceState, and reconciles via
// the pure plan_action predicate → the DO A/AAAA-record API (§9-safe fixed-arg
// curl, token from the mesh secret store). Spawned in run_serve next to the DDNS
// responder.
pub mod ddns;
pub mod stun_gather;
pub mod subprocess_tick;
// thumbnailer retired 2026-05-26 (TUNE-3.b): the worker module
// shelled out to `mackes/mesh_thumbnailer.py` which already
// retired with EPIC-RETIRE-PY-WORKBENCH; Thunar / GTK thumbnail
// dispatch is irrelevant in the v2.0+ Wayland-only sway + mde-
// files (Cosmic Files fork) stack. No live consumer of the pure
// helpers existed in tree.
// VOIP-GW-3 — leader-gated Vitelity per-node SIP sub-account provisioner
// (create + seal creds + reconcile + publish state/voice/<node>).
pub mod voice_provision;
pub mod wol;
// BUS-1.1 (v6.x Mackes Bus) — `mde-bus` subprocess supervisor.
// Spawns `mde-bus daemon`, restarts on exit, gracefully degrades
// when the binary is absent (development boxes that don't have
// the RPM installed yet). The outer supervisor's
// RestartPolicy::Always wraps this worker; inner respawn cooldown
// paces clean-exit restarts. Broker + mDNS + persistence land
// inside the binary in BUS-1.2/1.3/1.4.
pub mod boot_readiness;
pub mod bus_supervisor;
// XCP-6 — on an XCP-ng dom0, advertise hypervisor capacity into the compute
// plane (`compute/xcp-host/<node>`) so the mesh can place VMs on it.
pub mod xcp_host;
// XCP-3 — the A-plane provision flow: drains `action/provision/spawn`, then
// clones MDE-VM-golden → attaches the fresh identity seed → starts → resolves
// the IP over the mackes-xcp Hypervisor layer (the runtime caller of
// set_identity_seed, so a provisioned VM actually gets its identity seed).
pub mod xcp_provision;
// KVM-HEALTH (MV-2) — the Fedora+KVM successor to xcpng_health. Probes the
// per-node KVM virtualization service catalog (`crate::kvm::KVM_SERVICES`,
// `systemctl is-active` each) every 30 s and publishes a whole-host health
// summary to `event/kvm/services` so the Datacenter panels + the alert lane see
// the live stack state. Universal — every mesh node runs the same KVM stack.
pub mod kvm_health;
// MV-3 — the vm_lifecycle worker: the libvirt/KVM VM-lifecycle actuator (the
// Fedora+KVM equivalent of xapi/xenopsd/sm/xcp-networkd). Drains
// `action/vm/lifecycle` (create-from-image / start / stop / destroy / list,
// each addressed to a target node id) and publishes this node's VM instance
// roster to `event/vm/instances`. Shells `virsh`/`qemu-img` through the bounded
// proc path behind an injectable `LibvirtBackend` trait. Universal, like
// kvm_health — every node can host datacenter VMs.
pub mod vm_lifecycle;
// MV-4 — the container worker: the Podman container-lifecycle actuator (the
// container half of the mesh management layer, companion to MV-3 vm_lifecycle).
// Drains `action/container/lifecycle` (run / stop / rm / list, each addressed to a
// target node id) via an injectable `PodmanBackend` that shells `podman` through
// the bounded proc path, and publishes this node's container roster to
// `event/podman/containers`. Universal like vm_lifecycle — every node can host
// datacenter containers.
pub mod container;
// WL-UX-005 — the peer_app_launch worker: the peer-app remote-execution executor.
// Drains `action/apps/launch` (published by the shell's unified Front Door as
// `{node, app_id, name}`) and, only for requests addressed to its own node id,
// actually launches the requested app locally. Security is load-bearing: it execs
// ONLY an app this node itself advertises in its own app catalog (resolves the
// opaque `app_id` against `ipc::apps::scan_local_apps`, never an arbitrary command
// from the wire), and logs every launch + refusal. Workstation-tier — a headless
// relay has no seat to launch onto — and idles gracefully when no requests arrive.
pub mod peer_app_launch;
// EXPLORER-1 — the unit_aggregator worker: the daemon spine of the Hero unit
// explorer (docs/design/unit-explorer.md). Unions three sources into one typed
// `Unit` stream and publishes `state/units/<node>`: the mesh mirror (peers +
// `/mesh/leader` + health — source (a)), the union of every node's
// `state/openstack/<node>` mirror deduped by object id + host-tagged (source (b),
// QC-2, consumed through the Bus read path — never an openstack file), and the
// surface-gated active LAN scan (EXPLORER-2's producer seam, a no-op today). Pure
// fold over three injectable seams (self-first #23, first/last-seen E10), a
// publish-on-change mirror + heartbeat, and the E9 `action/units/get-stream` read
// verb. Universal (rank 0) — every node publishes its own unit view.
pub mod unit_aggregator;
// WL-FUNC-008 — the service_aggregator worker: the unified service
// provenance/health view. Merges three service sources — the published KDC
// directory (`kdc-services/<host>.json`), the nmap probe inventory
// (`probe-inventory.json`), and the Explorer's `service → openable-action`
// enrichment map — into one deduped `ServiceRecord` set (with stale-entry TTL
// age-out) published on `state/services/<node>`. Two injectable source seams + a
// pure fold, so the whole merge folds headless. Universal (rank 0) like
// unit_aggregator — every node publishes its own mesh-wide service view, no center.
pub mod service_aggregator;
// E12-20 — the storage worker: the privileged owner of the Workbench Storage plane
// (GParted for the mesh, docs/design/workbench-storage-plane.md). Owns a typed
// StorageOp pending-queue executor over a live UDisks2 zbus topology — stage-time
// advisory + apply-time authoritative validation, hard-wall interlocks (root/boot/
// EFI · mesh-storage backer · in-use VM/container backers), typed arming (lock 8),
// per-op Bus progress, and the `state/storage/<node>` mirror + `action/storage/
// <node>` verbs. Injectable UDisks2/executor/wall seams keep it headless-testable.
pub mod storage;
// E12-23 — filesystem depth: the typed fs-tooling verb layer under the storage
// executor's format/label/resize/LUKS/subvolume verbs. The honest per-fs capability
// matrix (lock 6), the pure shrink/move choreography state machine (lock 4), and the
// injectable FsToolRunner (production LiveFsTools shells mkfs.*/resize2fs/xfs_growfs/
// btrfs/ntfsresize/cryptsetup/parted; absent tool → typed Unavailable). No raw shell
// in the executor (§9); the whole matrix + mid-failure choreography fold headless.
pub mod fs_tools;
// E12-22 — virtual disks first-class: KVM images (qemu-img) + Podman storage as
// citizens of the Storage plane's staged op queue, beside the physical StorageOp
// queue. A parallel VirtualStorageOp pipeline (create/resize/snapshot/revert/convert/
// clone/attach-detach + volume create/remove/prune) walled by the same in-use sources
// (a running VM's image / a mounted volume), published to the sibling
// `state/storage/<node>/virtual` mirror. Owned by the storage worker (no new spawn).
pub mod virtual_storage;
// MV-5a — the scheduler worker: the placement slice of the no-center scheduler.
// Drains `action/schedule/place`, folds each node's latest `event/kvm/services`
// capacity, chooses the target node (healthy pin → most-active → node_id
// tie-break), and forwards a host-targeted create/run onto
// `action/vm/lifecycle` / `action/container/lifecycle` (plus the decision to
// `event/schedule/placements`). Rank-0-default like vm_lifecycle/container; an
// interim lowest-node-id single-actor election prevents duplicate placements.
pub mod scheduler;
// E12-5b — the session_broker worker: the mackesd side of the E12-5 VDI
// remote-desktop milestone. Drains `action/vdi/session`, folds each op into the
// live VDI-session roster (which peer serves which VM to which client + state)
// via a pure state machine, and — leader-gated — reconciles that roster into the
// shared roaming-session plane through an injectable `SessionStore` seam so any
// peer sees the active sessions. The live etcd/Syncthing cross-peer publish is
// integration-gated (typed `SessionStoreError::IntegrationGated`, §7); the pure
// core + fold + reconcile ship green behind the seam.
pub mod session_broker;
// E12-8 — the session_roaming worker: the roaming + persistence POLICY over the
// E12-5b `session_broker`'s sessions. Drains `action/vdi/roaming`, folds arrivals
// / per-VM disconnect policy / monitor layouts, and — leader-gated — makes a
// user's desktops follow them to any Workstation and survive disconnect: pure
// `reconcile_roaming` (desktops-follow-me), `on_disconnect` (default KeepRunning),
// and `on_node_loss` (hold reconnectable). REUSES the broker's `VdiSession` +
// `SessionStore` verbatim (no parallel session model); `MonitorLayout` rides the
// companion replicated layout store and preloads on worker start so saved layouts
// survive daemon restarts before the next roam.
pub mod session_roaming;
// E12-9 — the clipboard_bridge worker: the first of the E12-9 VDI client↔VM
// bridges. Drains `action/vdi/clipboard`, applies a per-session [`ClipboardPolicy`]
// (allow/deny + one-way + a size cap) via the pure `relay` decision
// (Forward/Drop/Truncate), and relays each clip into the connected VM desktop
// through the injectable `ClipboardAccess` seam — with an echo guard so a re-applied
// clip doesn't loop. Per-session + node-local, so NOT leader-gated (every serving
// node relays its own session's clips); rank-0-default like session_broker. The live
// OS/guest clipboard channel (SPICE/RDP vdagent / wl-clipboard) is integration-gated
// (typed `ClipboardAccessError::IntegrationGated`, §7); the pure model + relay
// pipeline ship green behind the seam.
pub mod clipboard_bridge;
// CHOOSER-1 — the desktop_sources worker: the mesh-side (§6) desktop-source
// discovery aggregator behind the Chooser surface (docs/design/desktop-
// chooser.md). Folds four lanes into one deduped roster published to
// `state/desktops/sources`: mesh-registry peer-advertised desktops (the
// peers-plane RemoteAccess/vms rows — no second advertisement channel),
// mDNS RDP/VNC/Spice on the local LAN (the mdns_relay machinery + its
// anti-loop TXT guard), local KVM guest consoles (the MV-3 LibvirtBackend
// seam, honestly Gated when virsh is absent — §7), and manual sources via
// typed `action/desktops/{add-source,remove-source,refresh}` verbs (§9).
// Reachability derives from roster presence / VM power state — never a
// blocking probe (design lock 14).
pub mod desktop_sources;
// VDI-VM-1 — the console_broker worker: makes a LOCAL KVM VM's loopback SPICE/VNC
// console reachable on the mesh. Every VM binds its graphics to 127.0.0.1
// (vm_lifecycle's domain XML), so a peer can open a broker session for a local VM
// yet never has a reachable `host:port` to dial. Serving-peer-gated: for each VDI
// `Open` naming a VM this node serves, it resolves the live console via `virsh
// domdisplay`, relays that loopback port onto the Nebula overlay with a scoped
// `socat` (the compute_expose forwarding pattern), and publishes the overlay
// endpoint back on the session record (`state/vdi/console`, keyed by session id)
// for the client shell to resolve. Honest-gates (never a fake endpoint) when the
// VM is off / has no graphics / socat|virsh|overlay is absent — §7.
pub mod console_broker;
// MEDIA-14 — the media_sources worker: the mesh-side (§6) media-source
// discovery aggregator behind the mde-media Sources panel (docs/design/
// mesh-media-player.md, row 26). Folds two lanes into one deduped roster
// published to `state/media/sources`: the mesh-registry peer-advertised media
// (each peer's `descriptors.media` Jellyfin/DLNA rows + its `descriptors.mesh_fs`
// file share — the same peers plane desktop_sources reads, no new channel), and
// mDNS `_jellyfin._tcp` on the local LAN (the mdns_relay machinery + its
// anti-loop TXT guard). Reachability derives from roster presence / peer health
// — never a blocking probe; music-only services (navidrome/mpd) are honestly
// excluded (mde-music's domain), and SSDP-only DLNA is a `gated:` mDNS-lane note.
pub mod media_sources;
// MEDIA-15 — the media_server worker: the mesh-side (§6) PRODUCER half of the
// mesh media plane (docs/design/mesh-media-player.md, rows 27 + 30). Scans this
// node's chosen shared folders into a `media-library.json` share manifest on the
// replicated QNM-Shared plane (so peers aggregate it), binds the mesh HTTP media
// server on MESH_MEDIA_PORT — which the localhost descriptor probe folds into
// this peer's `descriptors.media` as the `mde-media` service so peers' MEDIA-14
// discovery finds it — and serves a DLNA/UPnP MediaServer (device description +
// DIDL-Lite; the SSDP multicast announce is the honestly-gated live leg). Reads
// every peer's manifest off the plane + folds them into ONE deduped, per-node-
// attributed mesh library published to `state/media/library` for the MEDIA-8
// Library panel.
pub mod media_server;
// OW-11 (Bus half) — the service_onboard worker: `onboard service-add` reachable
// over the Bus for the shell's Services flow. Drains `action/onboard/service-add`
// (a typed ServiceAddAction: ServiceKind + optional SIP params + dry_run), runs
// the EXISTING onboard::service_add engine (`plan_service_add` + the injectable
// `ServiceApply` seam — §6 glue, no re-planning), and — leader-gated so an N-node
// mesh answers once — publishes the typed ServiceAddEvent (plan steps / outcome /
// typed error) on `event/onboard/service-add`. Production applies run over
// `LiveServiceApply`, whose typed `IntegrationGated` is the honest live answer
// today (§7 — never a fake success).
pub mod service_onboard;
// OW-7 (Bus half) — the spawn_lighthouse_onboard worker: `onboard spawn-lighthouse`
// reachable over the Bus for the shell's Spawn Lighthouse flow. Drains
// `action/onboard/spawn-lighthouse` (a typed cloud SpawnLighthouseAction + the
// --pair HA flag + dry_run), runs the EXISTING onboard::spawn_lighthouse engine
// (`plan_spawn` + the injectable `Provisioner` seam — §6 glue, no re-planning),
// and — leader-gated so an N-node mesh answers once — publishes the typed
// SpawnLighthouseEvent (plan summary / CA-migration steps / LAN-only retry hint /
// typed error) on `event/onboard/spawn-lighthouse`. Production provisions run over
// `LiveProvisioner`, whose typed `IntegrationGated` is the honest live answer today
// (§7 — the live cloud/SSH provision + CA-migrate stays gated, never a fake success).
pub mod spawn_lighthouse_onboard;
// OW-15 (target-side, day-2) — the onboard_apply worker: the §9-native receiver
// for the BusApply remote-push transport. Drains `action/onboard/apply` (a signed
// JobBundle + the claimed issuer), applies it ONLY when addressed to this node,
// from a leadership-authorized issuer (the CA `nodes` registry resolves the issuer
// to a leader-eligible `host`/lighthouse identity key), and validly
// signed/fresh/single-use — reusing the pure `onboard::remote_push` core verbatim
// (allow-listed Action enum, no raw shell — §9). A failure at any gate leaves the
// target unchanged (§7) and publishes a typed rejection on `event/onboard/apply`;
// the observed-state echo carries only redacted (secret-free) actions (§8). Runs
// on every node (any peer can be a target); the live cross-node round-trip is
// operator/live-gated behind BusApply.
pub mod onboard_apply;
// NOTIFY-CHAT-2 — the mackesd `chat` worker: the live plumbing behind the pure
// `mde-chat` model (design docs/design/mesh-chat-icq.md). Runs on EVERY node incl.
// headless (emit + relay, no UI). Drains `action/chat/send` (signs + relays a
// Message on `event/chat/message` + persists it to this node's Syncthing ring-log
// for offline backfill), folds every alert/event Bus lane into a message from the
// originating host (lock 11, no emitter changes), derives presence from the
// mesh-status snapshot + manual gossip, and republishes the `state/chat/roster` +
// `state/chat/conversation/<key>` read-model the Surface::Chat UI (NOTIFY-CHAT-3)
// renders. Bus + Syncthing roots are injectable seams so the whole worker is
// headless-testable; live 2-node delivery + real backfill are integration-gated.
pub mod chat;
// WL-FUNC-011 Phase 2 — the mesh `collab` worker: the live spine of the
// Communications suite, driving the headless `mde-collab-core` CollabEngine on
// the mesh. Drains `action/collab/*` commands (validate + Ed25519-sign via this
// node's identity → signed events), appends each to the per-space
// Syncthing-replicable actor log + projects it into the SQLite read models,
// publishes the live signed event on `collab/event/<space>/<actor>` +
// republishes the affected `state/collab/*` read models, and converges by merging
// foreign `collab/event/*` (bus fast-path) + replicated actor logs (Syncthing
// durable-path) — signature-checked (forged events dropped), idempotent,
// order-independent. Universal (rank 0), the same shape as the chat worker it
// will EVENTUALLY replace (Phase 4; it runs ALONGSIDE chat for now). Bus + actor
// -log roots are injectable seams so the whole flow is headless-testable.
pub mod collab;
// CHAT-FIX-2 — the local-notification producer worker (design
// docs/design/console-frontdoor.md Q34/46/47). The real empty-Chat fix: with the
// chat worker running but no peer chatter, nothing produced local system events,
// so the Chat surface stayed blank. This worker watches the node's OWN sources
// (mesh peer join/leave, dnf/platform updates, systemctl --failed, df/SMART,
// journal WARN+) on bounded/edge-triggered polls and publishes typed
// notifications on `event/notify/<source>` — a lane the chat worker already folds
// (ALERT_LANE_PREFIXES) into the `alert:<self>` conversation the Chat surface
// renders as a timestamped feed + tray badge. Every external source runs through
// an injectable probe (absent binary ⇒ skipped honestly), so the whole worker is
// headless-testable with fixtures; runs on EVERY node (rank 0).
pub mod notify;
// WL-SEC-002 — the federation runtime-enforcement worker. Reads the accepted
// cross-mesh grants (`federation.yaml`) and ENFORCES them at runtime: drains the
// cross-mesh ingress spool through the DEFAULT-DENY decision gate (only granted,
// non-excluded topics from accepted/non-revoked foreign meshes cross onto the local
// bus; everything else is dropped + audited), drains the shell Federation panel's
// accept/revoke/refuse-mint actions (accept installs the cross-mesh Nebula trust
// cert; revoke deletes it), and publishes the `state/federation/<node>` mirror the
// shell renders. Universal (rank 0): a lighthouse relays cross-mesh traffic so it
// especially must enforce the boundary; a workstation enforces its own ingress too.
pub mod federation_enforcer;
// NODE-GRADE-1 — the per-node self-grade worker (design docs/design/node-grade.md).
// Every node computes + publishes its OWN A–F capability grade from telemetry the
// platform already gathers (§6, no new probes): CPU headroom, RAM + disk free,
// role/worker health (the supervisor's live status + systemctl --failed), and mesh
// reachability (the replicated peer directory). Five factor sub-scores → a
// resource-heaviest weighted average → a smoothed 0–100 score + trend → an A–F
// band, published to `<workgroup_root>/node-grade/<hostname>.json` (the SEC-5
// mesh-shunt own-row idiom) so every peer reads every node's grade. A debounced
// drop into D/F fires an `event/notify/node-grade` alert the chat worker folds into
// the Chat feed (CHAT-FIX-2). Pure scoring/smoothing/debounce cores fold headless
// behind an injectable sampler; runs on EVERY node (rank 0).
pub mod node_grade;
// CLIP-SYNC-1 — mesh clipboard sync. Watches the local Wayland clipboard
// (`wl-paste --watch`, the Cosmic clipboard-manager hook), broadcasts every
// text clip on the bus + appends to ONE mesh-global `clipboard/history.json`
// (last 50 unpinned + unlimited pinned). All nodes tail it.
pub mod clipboard_sync;
// TUNE-16.d (2026-05-30) — Q22 8-peer cap counter. Counts enrolled
// `role = 'peer'` nodes (phones count, federated external-mesh peers
// are excluded by virtue of not appearing in the local store). Writes
// ~/.cache/mde/peer-cap.json every 30 s; publishes to
// mesh/peer-cap/updated Bus topic for real-time UI consumers.
pub mod peer_cap;
// LIGHTHOUSE-8 (2026-06-24) — per-lighthouse deep-probe lane. Every ~15 s probes
// each lighthouse for Nebula handshake / public IP / overlay peer count / uptime
// / CA cert-expiry (GLUE over nebula_admin + transport_probe + ca::expiry + the
// replicated directory) and publishes a `LighthouseProbe` to
// `compute/lighthouse-probe/<name>`. The Workbench Lighthouses tab renders it.
pub mod lighthouse_probe;
// FRONTDOOR-9 (2026-06-25) — the Copilot codex backend. A LEADER-only worker
// that drains `action/copilot/ask`, reads the sealed codex API key from the
// mesh secret-store, runs `codex exec` (external, pulled at runtime) per ask,
// and replies on `reply/<ulid>`. ASK/SUGGEST only — it spawns the AI subprocess
// itself but never executes OS actions on the operator's behalf (§9; typed
// actions are FRONTDOOR-11). Degrades gracefully when codex/key/network is down.
pub mod copilot;
// FRONTDOOR-11 — the typed action worker. Drains `action/exec/request` carrying a
// TYPED ActionRequest enum (an allowlisted KIND + typed params, never a command
// string — §9), dispatches each through an EXISTING verb mechanism (the PD-11
// lifecycle verb), writes a hash-chain audit row (the events plane — §8), and
// replies. Leader-gated; graceful degrade. NO raw-shell/arbitrary-command channel.
pub mod action;
// SURFACE-3 — the per-node `surface_enable` worker (a leader-of-self worker:
// it activates iptsd + applies the per-model config + walks the guided MOK
// enrollment on its OWN hardware, never a remote node). It lives beside its
// SURFACE-2 detection sibling under `crate::surface::enable` (not a file under
// workers/); re-exported here so the supervisor spawn site + SURFACE-4 reach
// it through the workers namespace like every other worker.
#[cfg(feature = "async-services")]
pub use crate::surface::enable::SurfaceEnableWorker;
// SURFACE-4 — the per-node `surface_verify` worker (a leader-of-self worker:
// it probes its OWN hardware into a tri-state board + publishes the compact
// fleet summary, never a remote node). Sibling of the SURFACE-2 detection +
// SURFACE-3 enable under `crate::surface::verify`; re-exported here so the
// supervisor spawn site reaches it through the workers namespace like every
// other worker.
#[cfg(feature = "async-services")]
pub use crate::surface::verify::SurfaceVerifyWorker;
// SURFACE-5 — the per-node `surface_firmware` worker (a leader-of-self worker:
// it lists + applies its OWN firmware via fwupd/LVFS, never a remote node, and
// re-runs SURFACE-4's verify after a successful apply). Sibling of the
// SURFACE-2/3/4 units under `crate::surface::firmware`; re-exported here so the
// supervisor spawn site reaches it through the workers namespace.
#[cfg(feature = "async-services")]
pub use crate::surface::firmware::SurfaceFirmwareWorker;

// arch-7 — the `Worker` trait moved to `mde-worker-core` and is re-exported
// above (`pub use mde_worker_core::{ShutdownToken, Worker};`).

// ── ENT-6: supervisor restart policy constants ──────────────────────

/// Restart back-off floor (the old fixed delay).
pub const INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_millis(250);
/// Restart back-off ceiling.
pub const BACKOFF_CAP: std::time::Duration = std::time::Duration::from_secs(60);
/// Failures within [`BREAKER_WINDOW`] that trip the circuit breaker.
pub const BREAKER_TRIP: u32 = 8;
/// The rapid-failure observation window.
pub const BREAKER_WINDOW: std::time::Duration = std::time::Duration::from_secs(120);

/// ENT-6 — one restart decision (pure; the spawn loop applies it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartDecision {
    /// Sleep this long, then restart.
    Backoff(std::time::Duration),
    /// The breaker tripped — stop restarting.
    Trip,
}

/// Advance the per-worker restart state after a failure. Returns the
/// new `(window_elapsed_reset, rapid_failures, delay)` state + the
/// decision. Pure — fully unit-testable without tokio time.
#[must_use]
pub fn advance_restart_state(
    window_elapsed: std::time::Duration,
    rapid_failures: u32,
    delay: std::time::Duration,
) -> (bool, u32, std::time::Duration, RestartDecision) {
    let (reset, mut failures, mut delay) = if window_elapsed > BREAKER_WINDOW {
        (true, 0, INITIAL_BACKOFF)
    } else {
        (false, rapid_failures, delay)
    };
    failures += 1;
    if failures >= BREAKER_TRIP {
        return (reset, failures, delay, RestartDecision::Trip);
    }
    let decision = RestartDecision::Backoff(delay);
    delay = (delay * 2).min(BACKOFF_CAP);
    (reset, failures, delay, decision)
}

// ── mackesd-05: half-open circuit-breaker recovery ──────────────────
//
// ENT-6 tripped the breaker and then left the worker PERMANENTLY dead
// for the rest of the daemon's life — a transient cause (a dependency
// briefly down) that tripped the breaker killed the worker until the
// operator restarted mackesd. mackesd-05 adds the standard half-open
// recovery: a tripped worker cools down, then gets a SINGLE trial
// restart; a trial that stays healthy for [`STABILITY_WINDOW`] closes
// the breaker (a full reset to the closed/healthy state), while one
// that fails fast re-opens it with a longer — but capped — cooldown.
// A genuinely-broken worker therefore backs off to at most one attempt
// per [`COOLDOWN_CAP`] (never a hot spin, never permanently dead); a
// transient trip heals itself without operator intervention. A
// `RestartPolicy::Never` worker never reaches this path (it is not
// restarted at all), so a deliberately one-shot worker is unaffected.

/// First cooldown a tripped worker waits before its half-open trial.
pub const COOLDOWN_INITIAL: std::time::Duration = std::time::Duration::from_secs(30);
/// Cooldown ceiling: a still-failing worker retries at most this
/// slowly. Conservative so a hard-broken dependency can't be hammered.
pub const COOLDOWN_CAP: std::time::Duration = std::time::Duration::from_secs(600);
/// Multiplier applied to the cooldown each time a half-open trial
/// relapses (fails within [`STABILITY_WINDOW`]).
pub const COOLDOWN_FACTOR: u32 = 3;
/// A half-open trial must stay alive at least this long to be judged
/// recovered (the breaker closes). Generous on purpose.
pub const STABILITY_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

/// mackesd-05 — grow the open-state cooldown after a relapsed trial,
/// saturating at [`COOLDOWN_CAP`]. Pure — unit-testable without tokio.
#[must_use]
pub fn grow_cooldown(cooldown: std::time::Duration) -> std::time::Duration {
    cooldown
        .checked_mul(COOLDOWN_FACTOR)
        .unwrap_or(COOLDOWN_CAP)
        .min(COOLDOWN_CAP)
}

/// mackesd-05 — the conceptual circuit-breaker phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerPhase {
    /// Healthy. Rapid failures accrue toward a trip (ENT-6).
    Closed,
    /// Tripped and cooling down before the next half-open trial.
    Open,
    /// A single trial restart is in flight.
    HalfOpen,
}

/// mackesd-05 — the verdict on a half-open trial, judged purely from
/// how long the trial stayed alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialOutcome {
    /// Alive >= [`STABILITY_WINDOW`]: the breaker closes (full reset).
    Recovered,
    /// Failed within [`STABILITY_WINDOW`]: the breaker re-opens with a
    /// longer, capped cooldown.
    Relapsed,
}

/// mackesd-05 — judge a half-open trial from its alive-duration. Pure.
#[must_use]
pub fn judge_trial(trial_alive: std::time::Duration) -> TrialOutcome {
    if trial_alive >= STABILITY_WINDOW {
        TrialOutcome::Recovered
    } else {
        TrialOutcome::Relapsed
    }
}

/// mackesd-05 — apply a trial verdict to the breaker. Pure. Returns the
/// next phase + the next open-state cooldown:
///  * `Recovered` ⇒ (`Closed`, [`COOLDOWN_INITIAL`]) — breaker reset.
///  * `Relapsed`  ⇒ (`Open`, grown+capped cooldown) — back off further.
#[must_use]
pub fn apply_trial_outcome(
    outcome: TrialOutcome,
    cooldown: std::time::Duration,
) -> (BreakerPhase, std::time::Duration) {
    match outcome {
        TrialOutcome::Recovered => (BreakerPhase::Closed, COOLDOWN_INITIAL),
        TrialOutcome::Relapsed => (BreakerPhase::Open, grow_cooldown(cooldown)),
    }
}

/// Restart policy for a worker. Phase A only honors `Never` and
/// `OnFailure` — Phase B integrates the `task-supervisor` crate to
/// implement back-off + max-restarts + circuit-breaker semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Don't restart — once the worker returns (Ok or Err), the
    /// supervisor records the outcome and moves on. Right for
    /// one-shot timer workers like `app_sync`.
    Never,
    /// Restart only if the worker returned `Err`. Right for
    /// long-running watchers (`clipboard_sync`, `mdns`, etc.).
    OnFailure,
    /// Restart on any return (Ok or Err). Right for "should never
    /// exit" workers like `notifications_server`.
    Always,
}

/// Declarative registration: a worker + its restart policy. The
/// supervisor builds its task list from a `Vec<Spawn>`.
pub struct Spawn {
    /// Worker to spawn. Boxed for trait-object storage.
    pub worker: Box<dyn Worker>,
    /// Restart policy.
    pub policy: RestartPolicy,
}

impl Spawn {
    /// Convenience constructor.
    pub fn new<W: Worker>(worker: W, policy: RestartPolicy) -> Self {
        Self {
            worker: Box::new(worker),
            policy,
        }
    }
}

/// test-obs-10 — a sink for NAMED worker-health alerts (currently a
/// circuit-breaker trip). The production sink writes the alert to the mesh Bus;
/// tests inject a recording sink so the Trip → alert path is observable without
/// a live Bus. Publishing is a rare, best-effort side effect — a failed publish
/// is logged, never fatal.
pub trait BreakerAlertSink: Send + Sync {
    /// Publish one already-built alert.
    fn publish(&self, alert: &notify::AlertMsg);
}

/// test-obs-10 — production [`BreakerAlertSink`]: writes the alert to the mesh
/// Bus on its own topic. Opens the Bus `Persist` per trip (trips are rare) so
/// the supervisor holds only a cheap `PathBuf`, never a live handle.
pub struct BusBreakerAlertSink {
    bus_root: std::path::PathBuf,
}

impl BusBreakerAlertSink {
    /// Wrap an explicit Bus spool root.
    #[must_use]
    pub fn new(bus_root: std::path::PathBuf) -> Self {
        Self { bus_root }
    }
}

impl BreakerAlertSink for BusBreakerAlertSink {
    fn publish(&self, alert: &notify::AlertMsg) {
        match mde_bus::persist::Persist::open(self.bus_root.clone()) {
            Ok(persist) => {
                if let Err(e) = persist.write(
                    &alert.topic,
                    mde_bus::hooks::config::Priority::Default,
                    None,
                    Some(&alert.body),
                ) {
                    warn!(topic = %alert.topic, error = %e, "test-obs-10: breaker-trip alert publish failed");
                }
            }
            Err(e) => warn!(
                bus_root = %self.bus_root.display(),
                error = %e,
                "test-obs-10: breaker-trip alert — bus open failed",
            ),
        }
    }
}

/// test-obs-10 — the default production alert sink: the mesh Bus at
/// [`mde_bus::default_data_dir`]. `None` when no Bus root resolves — the alert
/// then degrades to the journal `error!` line, exactly as before this change.
fn default_breaker_alert_sink() -> Option<Arc<dyn BreakerAlertSink>> {
    let root = mde_bus::default_data_dir()?;
    Some(Arc::new(BusBreakerAlertSink::new(root)))
}

/// Minimal in-process supervisor. Phase A scope: spawn each worker
/// once, log restarts, broadcast shutdown via a watch channel,
/// `join_all` on stop. Phase B re-wraps this in `task-supervisor` for
/// per-task back-off + add/remove-at-runtime semantics.
pub struct Supervisor {
    shutdown_tx: Arc<watch::Sender<bool>>,
    shutdown_rx: watch::Receiver<bool>,
    join: JoinSet<(&'static str, anyhow::Result<()>)>,
    /// EFF-24 — optional live status registry; when set, every spawn
    /// records lifecycle transitions into it.
    status: Option<WorkerStatusMap>,
    /// test-obs-10 — sink for the NAMED circuit-breaker-trip alert.
    /// Defaults to the mesh Bus ([`default_breaker_alert_sink`]); tests
    /// inject a recorder via [`Supervisor::set_breaker_alert_sink`].
    alert_sink: Option<Arc<dyn BreakerAlertSink>>,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl Supervisor {
    /// Construct an empty supervisor. Use [`Self::spawn`] to register
    /// workers, then [`Self::join_all`] / [`Self::shutdown_and_join`]
    /// to drive them.
    #[must_use]
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            shutdown_tx: Arc::new(tx),
            shutdown_rx: rx,
            join: JoinSet::new(),
            status: None,
            alert_sink: default_breaker_alert_sink(),
        }
    }

    /// EFF-24 — attach the shared per-worker status registry. Call
    /// before the first `spawn` so every worker is tracked.
    pub fn set_status_map(&mut self, map: WorkerStatusMap) {
        self.status = Some(map);
    }

    /// test-obs-10 — override the circuit-breaker-trip alert sink. Production
    /// keeps the default mesh-Bus sink; tests inject a recorder so the
    /// Trip → alert path is asserted without a live Bus.
    pub fn set_breaker_alert_sink(&mut self, sink: Arc<dyn BreakerAlertSink>) {
        self.alert_sink = Some(sink);
    }

    /// LIGHTHOUSE-8 — register the per-lighthouse deep-probe worker following the
    /// sibling spawn pattern (`Spawn::new(worker, policy)` + the role gate the
    /// inline spawns use). `RestartPolicy::OnFailure` mirrors the other
    /// long-running tick workers (`mesh_latency`, `peer_cap`).
    ///
    /// This is the module-owned registration the worker pool calls so the probe
    /// joins the supervisor without `bin/mackesd.rs`'s inline spawn list being
    /// edited. The probe is a rank-0 relay control-plane concern — every node
    /// probes the lighthouse set — so it is gated by the same
    /// [`crate::worker_role`] resolver as its siblings (unknown workers default
    /// to rank 0 ⇒ runs everywhere), and its workgroup root self-resolves from
    /// the daemon's `MDE_WORKGROUP_ROOT` env (set by the systemd unit). Returns
    /// the spawned worker's name for the caller's `worker_names` roster, or
    /// `None` when the role gate skips it.
    pub fn spawn_lighthouse_probe(&mut self) -> Option<&'static str> {
        let rank = crate::worker_role::resolve_rank();
        if !crate::worker_role::runs("lighthouse_probe", rank) {
            return None;
        }
        let root = mackes_mesh_types::peers::default_workgroup_root();
        let worker = lighthouse_probe::LighthouseProbeWorker::new(root);
        let name = worker.name();
        self.spawn(Spawn::new(worker, RestartPolicy::OnFailure));
        Some(name)
    }

    /// Issue every spawned worker a fresh shutdown token cloned from
    /// our channel.
    #[must_use]
    pub fn token(&self) -> ShutdownToken {
        ShutdownToken::from_receiver(self.shutdown_rx.clone())
    }

    /// Spawn a worker. The supervisor honors `Spawn::policy` for
    /// restart decisions (Phase A: `Never`/`OnFailure`/`Always`
    /// implemented via a self-spawning loop inside `run_one`).
    pub fn spawn(&mut self, spec: Spawn) {
        let token = self.token();
        let Spawn { mut worker, policy } = spec;
        let name = worker.name();
        let shutdown = token;
        // EFF-24 — register + maintain the live status row.
        let status = self.status.clone();
        // test-obs-10 — the breaker-trip alert sink, moved into the task.
        let alert_sink = self.alert_sink.clone();
        update_status(&status, name, |w| w.alive = true);
        self.join.spawn(async move {
            // ENT-6 — closed-state restart-policy state for this worker.
            let mut delay = INITIAL_BACKOFF;
            let mut rapid_failures: u32 = 0;
            let mut window_start = std::time::Instant::now();
            let mut first_run = true;
            // mackesd-05 — half-open recovery state. `cooldown` is the
            // current open-state wait; `trial` marks that the UPCOMING
            // run is a single half-open trial restart (we just cooled
            // down from a trip or a relapse).
            let mut cooldown = COOLDOWN_INITIAL;
            let mut trial = false;
            // `break outcome` carries the worker's final result out
            // of the loop, so we don't need a pre-initialized
            // `last_result` slot (which would dead-code in the
            // can-never-be-empty `loop {}`).
            let last_result: anyhow::Result<()> = loop {
                info!(worker = %name, "starting worker");
                if !first_run {
                    update_status(&status, name, |w| {
                        w.restarts += 1;
                        w.alive = true;
                    });
                }
                first_run = false;
                let token_for_run = shutdown.clone();
                // EFF-4 — a worker that PANICS (not just returns Err) must be
                // restarted too. Without this, a panic unwinds the whole
                // supervisor task and `join_all` only logs the JoinError —
                // the worker is silently dead for the daemon's lifetime.
                // Catch the unwind so it flows through the same restart-policy
                // + back-off + circuit-breaker path as an Err below.
                let run_fut = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
                    worker.run(token_for_run),
                ));
                // mackesd-05 — measure the trial's alive-duration on the
                // TOKIO clock (so paused-time tests and the
                // STABILITY_WINDOW sleep share a clock), and — for a
                // half-open trial — race a stability timer against the
                // run so a trial that survives STABILITY_WINDOW clears
                // the trip in the status map LIVE. That matters because
                // most workers are tick-loops that never exit once the
                // transient cause clears: without the live clear,
                // healthz would show the recovered worker as tripped
                // forever.
                let trial_start = tokio::time::Instant::now();
                let joined = if trial {
                    tokio::pin!(run_fut);
                    let stability = tokio::time::sleep(STABILITY_WINDOW);
                    tokio::pin!(stability);
                    let mut cleared = false;
                    loop {
                        tokio::select! {
                            res = run_fut.as_mut() => break res,
                            () = stability.as_mut(), if !cleared => {
                                cleared = true;
                                update_status(&status, name, |w| w.breaker_tripped = false);
                                info!(
                                    worker = %name,
                                    "mackesd-05: half-open trial stable — clearing breaker",
                                );
                            }
                        }
                    }
                } else {
                    run_fut.await
                };
                let outcome = match joined {
                    Ok(result) => result,
                    Err(panic) => {
                        let msg = panic
                            .downcast_ref::<&str>()
                            .map(|s| (*s).to_string())
                            .or_else(|| panic.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "worker panicked".to_string());
                        Err(anyhow::anyhow!("worker panicked: {msg}"))
                    }
                };
                let should_restart = match (policy, &outcome) {
                    (RestartPolicy::Never, _) => false,
                    (RestartPolicy::OnFailure, Err(_)) => true,
                    (RestartPolicy::OnFailure, Ok(())) => false,
                    (RestartPolicy::Always, _) => true,
                };
                match &outcome {
                    Ok(()) => info!(worker = %name, "worker returned Ok"),
                    Err(e) => warn!(worker = %name, error = ?e, "worker returned Err"),
                }
                // EFF-24 — record the exit; `alive` flips back on if a
                // restart follows.
                let exit_ok = outcome.is_ok();
                update_status(&status, name, |w| {
                    w.alive = false;
                    w.last_exit_ok = Some(exit_ok);
                });
                if !should_restart {
                    break outcome;
                }
                if shutdown.is_shutdown() {
                    info!(worker = %name, "shutdown requested; not restarting");
                    break outcome;
                }
                // mackesd-05 — resolve a half-open trial before the
                // normal closed-state path. A trial that stayed alive
                // >= STABILITY_WINDOW recovers (breaker closes, full
                // reset); one that failed fast relapses (breaker
                // re-opens with a longer, capped cooldown).
                if trial {
                    trial = false;
                    let (next_phase, next_cooldown) =
                        apply_trial_outcome(judge_trial(trial_start.elapsed()), cooldown);
                    cooldown = next_cooldown;
                    match next_phase {
                        BreakerPhase::Closed => {
                            // Recovered: reset to the closed/healthy state
                            // and resume normal supervision.
                            rapid_failures = 0;
                            delay = INITIAL_BACKOFF;
                            window_start = std::time::Instant::now();
                            update_status(&status, name, |w| w.breaker_tripped = false);
                            info!(
                                worker = %name,
                                "mackesd-05: half-open trial recovered — breaker closed",
                            );
                            continue;
                        }
                        BreakerPhase::Open | BreakerPhase::HalfOpen => {
                            // Relapsed: cool down (longer), then try once
                            // more. `apply_trial_outcome` only ever yields
                            // `Closed`/`Open`; the `HalfOpen` arm is folded
                            // in so the match stays exhaustive.
                            update_status(&status, name, |w| w.breaker_tripped = true);
                            warn!(
                                worker = %name,
                                cooldown_s = cooldown.as_secs(),
                                "mackesd-05: half-open trial relapsed — re-opening breaker",
                            );
                            let mut cool_tok = shutdown.clone();
                            tokio::select! {
                                () = cool_tok.wait() => break outcome,
                                () = tokio::time::sleep(cooldown) => {}
                            }
                            if shutdown.is_shutdown() {
                                break outcome;
                            }
                            trial = true;
                            continue;
                        }
                    }
                }
                // ENT-6 — bounded exponential back-off + circuit
                // breaker (replaces the Phase-A fixed 250 ms retry):
                // a worker that keeps dying restarts at 250 ms, 500 ms,
                // 1 s … capped at BACKOFF_CAP; one that dies
                // BREAKER_TRIP times within BREAKER_WINDOW trips the
                // breaker. A healthy run longer than BREAKER_WINDOW
                // resets both counters.
                let now = std::time::Instant::now();
                let (reset, failures, next_delay, decision) =
                    advance_restart_state(now.duration_since(window_start), rapid_failures, delay);
                if reset {
                    window_start = now;
                }
                rapid_failures = failures;
                delay = next_delay;
                match decision {
                    RestartDecision::Trip => {
                        // mackesd-05 — a trip no longer permanently kills
                        // the worker (ENT-6 used to `break` here). Cool
                        // down, then a SINGLE half-open trial restart
                        // follows; the trip stays visible on healthz
                        // (readiness=false) until a trial proves stable.
                        error!(
                            worker = %name,
                            failures = rapid_failures,
                            window_s = BREAKER_WINDOW.as_secs(),
                            cooldown_s = COOLDOWN_INITIAL.as_secs(),
                            "ENT-6/mackesd-05: circuit breaker tripped — cooling down before \
                             a half-open recovery trial",
                        );
                        // test-obs-9 — the real, scrapeable failure signal: a
                        // genuine closed→open trip bumps the cumulative
                        // per-worker counter the metrics exporter surfaces as
                        // `mackesd_breaker_trips_total{worker=…}`.
                        update_status(&status, name, |w| {
                            w.breaker_tripped = true;
                            w.breaker_trips += 1;
                        });
                        // test-obs-10 — a trip also surfaces a NAMED alert on
                        // the Bus so an operator can tell WHICH worker died and
                        // that it is a breaker-open (not transient) failure,
                        // rather than an anonymous "N journal warnings" blob.
                        // Fired only on the fresh Trip decision — half-open
                        // relapses take the `trial` path above, not this arm —
                        // so exactly one alert per trip.
                        if let Some(sink) = &alert_sink {
                            let reason = match &outcome {
                                Err(e) => format!(
                                    "{rapid_failures} failures within {}s (last error: {e})",
                                    BREAKER_WINDOW.as_secs()
                                ),
                                Ok(()) => format!(
                                    "{rapid_failures} rapid restarts within {}s",
                                    BREAKER_WINDOW.as_secs()
                                ),
                            };
                            sink.publish(&notify::breaker_trip_alert(name, &reason));
                        }
                        cooldown = COOLDOWN_INITIAL;
                        let mut cool_tok = shutdown.clone();
                        tokio::select! {
                            () = cool_tok.wait() => break outcome,
                            () = tokio::time::sleep(cooldown) => {}
                        }
                        if shutdown.is_shutdown() {
                            break outcome;
                        }
                        trial = true;
                    }
                    RestartDecision::Backoff(d) => tokio::time::sleep(d).await,
                }
            };
            (name, last_result)
        });
    }

    /// Wait until every spawned worker has finished. The runtime
    /// drives them; this just blocks until the join set drains.
    pub async fn join_all(&mut self) -> Vec<(&'static str, anyhow::Result<()>)> {
        let mut outcomes = Vec::new();
        while let Some(joined) = self.join.join_next().await {
            match joined {
                Ok(o) => outcomes.push(o),
                Err(e) => {
                    error!(error = ?e, "worker task panicked");
                }
            }
        }
        outcomes
    }

    /// Signal shutdown and drain. The watch channel's atomic flip
    /// means every cloned [`ShutdownToken`] sees `true` on its next
    /// poll.
    ///
    /// # Errors
    ///
    /// Returns an error only if the watch sender is somehow already
    /// closed, which would indicate a programmer error.
    pub async fn shutdown_and_join(
        &mut self,
    ) -> anyhow::Result<Vec<(&'static str, anyhow::Result<()>)>> {
        self.shutdown_tx
            .send(true)
            .context("broadcasting shutdown to workers")?;
        // REL-1 (2026-06-16) — bound the drain. A single worker that doesn't
        // honor the shutdown token within the grace (a blocking subprocess /
        // sync I/O outside a `select!`) used to hold the whole shutdown until
        // systemd's TimeoutStopSec SIGKILLed the daemon — which left
        // `systemctl restart mackesd` wedged ("active" but not answering RPCs)
        // for ~20s on every node during the v10.0.9 roll. After the grace,
        // abort the stragglers so the daemon exits promptly and a restart
        // comes back responsive fast.
        const GRACE: std::time::Duration = std::time::Duration::from_secs(6);
        match tokio::time::timeout(GRACE, self.join_all()).await {
            Ok(outcomes) => Ok(outcomes),
            Err(_) => {
                warn!(
                    grace_s = GRACE.as_secs(),
                    "shutdown: workers did not finish within grace; aborting stragglers"
                );
                self.join.abort_all();
                let mut outcomes = Vec::new();
                while let Some(joined) = self.join.join_next().await {
                    if let Ok(o) = joined {
                        outcomes.push(o);
                    }
                }
                Ok(outcomes)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// test-obs-10 — records every published breaker-trip alert so a test can
    /// assert the Trip → alert path fired (and exactly how often) without a
    /// live Bus.
    #[derive(Default)]
    struct RecordingSink {
        alerts: std::sync::Mutex<Vec<notify::AlertMsg>>,
    }

    impl BreakerAlertSink for RecordingSink {
        fn publish(&self, alert: &notify::AlertMsg) {
            self.alerts.lock().unwrap().push(alert.clone());
        }
    }

    impl RecordingSink {
        fn recorded(&self) -> Vec<notify::AlertMsg> {
            self.alerts.lock().unwrap().clone()
        }
    }

    /// test-obs-10 — attach a fresh recording sink and hand back its handle so
    /// the test can read what the Trip path published. Keeps the trip tests
    /// hermetic (no writes to the real mesh Bus the default sink targets).
    fn recording_sink(sup: &mut Supervisor) -> Arc<RecordingSink> {
        let sink = Arc::new(RecordingSink::default());
        sup.set_breaker_alert_sink(Arc::clone(&sink) as Arc<dyn BreakerAlertSink>);
        sink
    }

    struct CountdownWorker {
        remaining: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Worker for CountdownWorker {
        fn name(&self) -> &'static str {
            "countdown"
        }
        async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
            loop {
                let n = self.remaining.fetch_sub(1, Ordering::SeqCst);
                if n == 0 {
                    return Ok(());
                }
                tokio::select! {
                    _ = shutdown.wait() => return Ok(()),
                    _ = tokio::time::sleep(std::time::Duration::from_millis(5)) => {}
                }
            }
        }
    }

    struct ShutdownObserver {
        observed: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Worker for ShutdownObserver {
        fn name(&self) -> &'static str {
            "observer"
        }
        async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
            shutdown.wait().await;
            self.observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FailOnce {
        attempts: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Worker for FailOnce {
        fn name(&self) -> &'static str {
            "fail-once"
        }
        async fn run(&mut self, _shutdown: ShutdownToken) -> anyhow::Result<()> {
            let n = self.attempts.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                anyhow::bail!("intentional first-attempt failure")
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn worker_runs_to_completion_under_never_policy() {
        let mut sup = Supervisor::new();
        let counter = Arc::new(AtomicUsize::new(3));
        sup.spawn(Spawn::new(
            CountdownWorker {
                remaining: counter.clone(),
            },
            RestartPolicy::Never,
        ));
        let outcomes = sup.join_all().await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].0, "countdown");
        assert!(outcomes[0].1.is_ok());
    }

    #[tokio::test]
    async fn shutdown_token_propagates_to_workers() {
        let mut sup = Supervisor::new();
        let observed = Arc::new(AtomicUsize::new(0));
        sup.spawn(Spawn::new(
            ShutdownObserver {
                observed: observed.clone(),
            },
            RestartPolicy::Never,
        ));
        sup.shutdown_and_join().await.unwrap();
        assert_eq!(observed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn on_failure_policy_restarts_until_ok() {
        let mut sup = Supervisor::new();
        // test-obs-10 — a normal restart (one failure, then Ok) is NOT a trip,
        // so it must publish NO named breaker alert.
        let sink = recording_sink(&mut sup);
        let attempts = Arc::new(AtomicUsize::new(0));
        sup.spawn(Spawn::new(
            FailOnce {
                attempts: attempts.clone(),
            },
            RestartPolicy::OnFailure,
        ));
        let outcomes = sup.join_all().await;
        assert_eq!(outcomes.len(), 1);
        // Final attempt should have returned Ok.
        assert!(outcomes[0].1.is_ok());
        assert!(attempts.load(Ordering::SeqCst) >= 2);
        // test-obs-10 — a plain restart never fires the breaker alert.
        assert!(
            sink.recorded().is_empty(),
            "a normal restart must not emit a breaker-trip alert",
        );
    }

    struct PanicOnce {
        attempts: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Worker for PanicOnce {
        fn name(&self) -> &'static str {
            "panic-once"
        }
        async fn run(&mut self, _shutdown: ShutdownToken) -> anyhow::Result<()> {
            let n = self.attempts.fetch_add(1, Ordering::SeqCst);
            assert!(n != 0, "intentional first-attempt panic");
            Ok(())
        }
    }

    #[tokio::test]
    async fn on_failure_policy_restarts_after_a_panic() {
        // EFF-4 — a worker that PANICS (not just returns Err) is caught + fed
        // through the restart policy, not silently lost as a JoinError.
        let mut sup = Supervisor::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        sup.spawn(Spawn::new(
            PanicOnce {
                attempts: attempts.clone(),
            },
            RestartPolicy::OnFailure,
        ));
        let outcomes = sup.join_all().await;
        assert_eq!(outcomes.len(), 1);
        assert!(
            outcomes[0].1.is_ok(),
            "worker recovered on the post-panic restart"
        );
        assert!(
            attempts.load(Ordering::SeqCst) >= 2,
            "panicked then restarted"
        );
    }

    #[tokio::test]
    async fn status_map_tracks_lifecycle_and_restarts() {
        // EFF-24 — the registry records spawn (alive), restart count,
        // and the final clean exit.
        let mut sup = Supervisor::new();
        let status = new_status_map();
        sup.set_status_map(Arc::clone(&status));
        let attempts = Arc::new(AtomicUsize::new(0));
        sup.spawn(Spawn::new(
            FailOnce {
                attempts: attempts.clone(),
            },
            RestartPolicy::OnFailure,
        ));
        let _ = sup.join_all().await;
        let g = status.lock().unwrap();
        let w = g.get("fail-once").expect("status row exists");
        assert!(!w.alive, "exited cleanly after the restart");
        assert!(w.restarts >= 1, "first failure produced a restart");
        assert_eq!(w.last_exit_ok, Some(true), "final exit was Ok");
        assert!(!w.breaker_tripped);
        drop(g);
        let (alive, total, tripped) = workers_ready(&status);
        assert_eq!((alive, total, tripped), (0, 1, 0));
    }

    #[tokio::test]
    async fn status_map_alive_while_running() {
        // A long-running worker shows alive=true until shutdown.
        let mut sup = Supervisor::new();
        let status = new_status_map();
        sup.set_status_map(Arc::clone(&status));
        let observed = Arc::new(AtomicUsize::new(0));
        sup.spawn(Spawn::new(
            ShutdownObserver {
                observed: observed.clone(),
            },
            RestartPolicy::Never,
        ));
        // Give the spawn a beat to register.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let (alive, total, tripped) = workers_ready(&status);
        assert_eq!((alive, total, tripped), (1, 1, 0));
        sup.shutdown_and_join().await.unwrap();
        let (alive, _, _) = workers_ready(&status);
        assert_eq!(alive, 0, "exit recorded after shutdown");
    }

    #[test]
    fn restart_policy_match_completeness() {
        // Compile-time check that every variant is named here. If a
        // new variant is added, this match will fail to compile.
        for p in [
            RestartPolicy::Never,
            RestartPolicy::OnFailure,
            RestartPolicy::Always,
        ] {
            match p {
                RestartPolicy::Never | RestartPolicy::OnFailure | RestartPolicy::Always => {}
            }
        }
    }

    // ── mackesd-05: half-open recovery — behavioral (paused-time) ───

    /// Fails its first `fail_first` attempts, then stays alive until
    /// shutdown. `fail_first == BREAKER_TRIP` ⇒ it trips, then the very
    /// next (half-open trial) attempt is the stable one; a larger value
    /// forces one or more relapses before it finally recovers.
    struct FlakyThenPark {
        fail_first: usize,
        attempts: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Worker for FlakyThenPark {
        fn name(&self) -> &'static str {
            "flaky-park"
        }
        async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
            let n = self.attempts.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_first {
                anyhow::bail!("flaky attempt {n}")
            }
            // Stable: hold the run alive so the half-open stability
            // window can elapse and the breaker closes.
            shutdown.wait().await;
            Ok(())
        }
    }

    /// Always fails immediately — a genuinely-broken worker.
    struct AlwaysErr {
        attempts: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Worker for AlwaysErr {
        fn name(&self) -> &'static str {
            "always-err"
        }
        async fn run(&mut self, _shutdown: ShutdownToken) -> anyhow::Result<()> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("always broken")
        }
    }

    /// Drive the paused clock forward until `pred` holds or the budget
    /// is exhausted. In paused-time tests `sleep` auto-advances to the
    /// next timer, so this yields to the supervisor while stepping
    /// virtual time. Returns whether `pred` was observed.
    async fn drive_until(
        status: &WorkerStatusMap,
        name: &'static str,
        budget: usize,
        pred: impl Fn(&WorkerStatus) -> bool,
    ) -> bool {
        for _ in 0..budget {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let hit = {
                let g = status.lock().unwrap();
                g.get(name).map_or(false, |w| pred(w))
            };
            if hit {
                return true;
            }
        }
        false
    }

    #[tokio::test(start_paused = true)]
    async fn tripped_worker_recovers_via_half_open() {
        // mackesd-05 core: a transient trip is NOT permanently fatal.
        // The worker trips after BREAKER_TRIP rapid failures, cools
        // down, gets ONE trial restart, survives the stability window,
        // and the breaker closes — all within the daemon's lifetime.
        let mut sup = Supervisor::new();
        let status = new_status_map();
        sup.set_status_map(Arc::clone(&status));
        let sink = recording_sink(&mut sup);
        let attempts = Arc::new(AtomicUsize::new(0));
        sup.spawn(Spawn::new(
            FlakyThenPark {
                fail_first: BREAKER_TRIP as usize,
                attempts: attempts.clone(),
            },
            RestartPolicy::OnFailure,
        ));
        // Recovered == it went through the full trip cycle
        // (restarts >= BREAKER_TRIP), the trial is alive, AND the
        // breaker cleared. The `restarts` gate rules out the healthy
        // pre-trip runs (which are also alive + not-tripped).
        let recovered = drive_until(&status, "flaky-park", 200, |w| {
            w.restarts >= BREAKER_TRIP && w.alive && !w.breaker_tripped
        })
        .await;
        assert!(
            recovered,
            "tripped worker recovered through a half-open trial"
        );
        // BREAKER_TRIP failing runs + exactly ONE stable trial run.
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            BREAKER_TRIP as usize + 1,
            "one half-open trial after the trip",
        );
        {
            let g = status.lock().unwrap();
            let w = g.get("flaky-park").unwrap();
            assert!(w.restarts >= 1, "the trial counts as a restart");
            // test-obs-9 — the genuine closed→open trip bumped the
            // cumulative failure counter exactly once (the half-open
            // trial recovered, so no relapse re-open followed).
            assert_eq!(
                w.breaker_trips, 1,
                "one genuine breaker trip recorded on trips_total",
            );
        }
        // test-obs-10 — the single trip surfaced exactly ONE named alert on the
        // worker's stable topic, carrying its name + the breaker-open fact.
        let alerts = sink.recorded();
        assert_eq!(
            alerts.len(),
            1,
            "one trip ⇒ exactly one named breaker alert"
        );
        assert_eq!(alerts[0].topic, "fleet/health/breaker/flaky-park");
        assert!(
            alerts[0].body.contains("flaky-park"),
            "alert names the worker"
        );
        assert!(
            alerts[0].body.contains("breaker_open"),
            "alert flags breaker-open"
        );
        sup.shutdown_and_join().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn tripped_worker_relapses_then_recovers_never_permanently_dead() {
        // A trip followed by a couple of fast relapses must still lead
        // to re-attempts (never permanent death), and once the worker
        // finally holds it recovers. `fail_first = BREAKER_TRIP + 2`
        // forces two half-open relapses (each re-opening with a longer
        // cooldown) before the stable trial.
        let mut sup = Supervisor::new();
        let status = new_status_map();
        sup.set_status_map(Arc::clone(&status));
        let sink = recording_sink(&mut sup);
        let attempts = Arc::new(AtomicUsize::new(0));
        sup.spawn(Spawn::new(
            FlakyThenPark {
                fail_first: BREAKER_TRIP as usize + 2,
                attempts: attempts.clone(),
            },
            RestartPolicy::OnFailure,
        ));
        let recovered = drive_until(&status, "flaky-park", 400, |w| {
            w.restarts >= BREAKER_TRIP && w.alive && !w.breaker_tripped
        })
        .await;
        assert!(recovered, "worker recovered after relapsing twice");
        // 8 initial fails + 2 relapsed trials + 1 stable trial.
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            BREAKER_TRIP as usize + 3,
            "re-attempted after each relapse rather than dying",
        );
        // test-obs-10 — only the FRESH trip alerts; half-open relapses take the
        // trial path, not the Trip arm, so it is still exactly one alert.
        assert_eq!(
            sink.recorded().len(),
            1,
            "one alert per trip even across relapses",
        );
        sup.shutdown_and_join().await.unwrap();
    }

    #[tokio::test]
    async fn never_policy_worker_is_never_restarted_or_tripped() {
        // A `RestartPolicy::Never` worker must be untouched by the
        // recovery machinery: it runs exactly once (even on Err) and
        // never enters the breaker path.
        let mut sup = Supervisor::new();
        let status = new_status_map();
        sup.set_status_map(Arc::clone(&status));
        let attempts = Arc::new(AtomicUsize::new(0));
        sup.spawn(Spawn::new(
            AlwaysErr {
                attempts: attempts.clone(),
            },
            RestartPolicy::Never,
        ));
        let outcomes = sup.join_all().await;
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].1.is_err(), "the single run returned Err");
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "never-restart ⇒ exactly one run",
        );
        let g = status.lock().unwrap();
        let w = g.get("always-err").unwrap();
        assert!(!w.breaker_tripped, "a Never worker never trips");
    }
}

#[cfg(test)]
mod ent6_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn backoff_doubles_to_the_cap() {
        let mut delay = INITIAL_BACKOFF;
        let mut seen = Vec::new();
        for _ in 0..12 {
            let (_, _, next, decision) = advance_restart_state(Duration::ZERO, 0, delay);
            seen.push(decision);
            delay = next;
        }
        assert_eq!(
            seen[0],
            RestartDecision::Backoff(Duration::from_millis(250))
        );
        assert_eq!(
            seen[1],
            RestartDecision::Backoff(Duration::from_millis(500))
        );
        assert!(delay <= BACKOFF_CAP, "ceiling holds: {delay:?}");
    }

    #[test]
    fn rapid_failures_trip_the_breaker_within_the_window() {
        // ENT-6 acceptance: a hot-looping worker stops being
        // restarted instead of spinning forever.
        let mut failures = 0;
        let mut delay = INITIAL_BACKOFF;
        let mut tripped = false;
        for _ in 0..BREAKER_TRIP {
            let (_, f, d, decision) =
                advance_restart_state(Duration::from_secs(1), failures, delay);
            failures = f;
            delay = d;
            if decision == RestartDecision::Trip {
                tripped = true;
                break;
            }
        }
        assert!(tripped, "the {BREAKER_TRIP}th rapid failure must trip");
    }

    #[test]
    fn a_healthy_stretch_resets_the_window() {
        // 7 rapid failures, then a long-lived run: counters reset,
        // the next failure backs off at the floor instead of tripping.
        let (reset, failures, _, decision) = advance_restart_state(
            BREAKER_WINDOW + Duration::from_secs(1),
            BREAKER_TRIP - 1,
            BACKOFF_CAP,
        );
        assert!(reset);
        assert_eq!(failures, 1);
        assert_eq!(decision, RestartDecision::Backoff(INITIAL_BACKOFF));
    }
}

/// mackesd-05 — pure tests for the half-open recovery state machine,
/// written in the same tokio-free style as `ent6_tests`.
#[cfg(test)]
mod mackesd05_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cooldown_grows_by_the_factor_and_saturates_at_the_cap() {
        // 30s → 90s → 270s → 600s(cap) → 600s.
        let mut cd = COOLDOWN_INITIAL;
        assert_eq!(cd, Duration::from_secs(30));
        cd = grow_cooldown(cd);
        assert_eq!(cd, Duration::from_secs(90));
        cd = grow_cooldown(cd);
        assert_eq!(cd, Duration::from_secs(270));
        cd = grow_cooldown(cd);
        assert_eq!(cd, COOLDOWN_CAP, "saturates at the cap");
        cd = grow_cooldown(cd);
        assert_eq!(cd, COOLDOWN_CAP, "stays capped, never a hot spin");
    }

    #[test]
    fn grow_cooldown_never_overflows() {
        // A pathological near-MAX cooldown saturates instead of
        // panicking on the multiply.
        assert_eq!(grow_cooldown(Duration::MAX), COOLDOWN_CAP);
    }

    #[test]
    fn a_trial_shorter_than_the_window_relapses() {
        assert_eq!(
            judge_trial(STABILITY_WINDOW - Duration::from_millis(1)),
            TrialOutcome::Relapsed,
        );
        assert_eq!(judge_trial(Duration::ZERO), TrialOutcome::Relapsed);
    }

    #[test]
    fn a_trial_that_survives_the_window_recovers() {
        assert_eq!(judge_trial(STABILITY_WINDOW), TrialOutcome::Recovered);
        assert_eq!(
            judge_trial(STABILITY_WINDOW + Duration::from_secs(5)),
            TrialOutcome::Recovered,
        );
    }

    #[test]
    fn recovery_closes_the_breaker_and_resets_the_cooldown() {
        // Even after the cooldown had grown to the cap, recovering
        // resets it to the initial value (a fresh transient later
        // starts from the floor again).
        let (phase, cd) = apply_trial_outcome(TrialOutcome::Recovered, COOLDOWN_CAP);
        assert_eq!(phase, BreakerPhase::Closed);
        assert_eq!(cd, COOLDOWN_INITIAL);
    }

    #[test]
    fn relapse_reopens_the_breaker_with_a_longer_capped_cooldown() {
        let (phase, cd) = apply_trial_outcome(TrialOutcome::Relapsed, COOLDOWN_INITIAL);
        assert_eq!(phase, BreakerPhase::Open);
        assert_eq!(cd, Duration::from_secs(90));
        // From near the cap it grows no further than the cap.
        let (phase, cd) = apply_trial_outcome(TrialOutcome::Relapsed, COOLDOWN_CAP);
        assert_eq!(phase, BreakerPhase::Open);
        assert_eq!(cd, COOLDOWN_CAP);
    }

    #[test]
    fn full_cycle_trip_cooldown_relapse_then_recover() {
        // Trace the state machine the supervisor loop drives:
        // trip → open(initial) → trial fails → open(grown) → trial
        // fails → open(grown) → trial holds → closed(reset).
        let mut cooldown = COOLDOWN_INITIAL; // the trip's first cooldown

        // First trial relapses.
        let (phase, cd) = apply_trial_outcome(judge_trial(Duration::from_secs(1)), cooldown);
        cooldown = cd;
        assert_eq!(phase, BreakerPhase::Open);
        assert_eq!(cooldown, Duration::from_secs(90));

        // Second trial relapses — cooldown grows again.
        let (phase, cd) = apply_trial_outcome(judge_trial(Duration::from_secs(2)), cooldown);
        cooldown = cd;
        assert_eq!(phase, BreakerPhase::Open);
        assert_eq!(cooldown, Duration::from_secs(270));

        // Third trial holds past the stability window — full reset.
        let (phase, cd) = apply_trial_outcome(judge_trial(STABILITY_WINDOW), cooldown);
        cooldown = cd;
        assert_eq!(phase, BreakerPhase::Closed);
        assert_eq!(cooldown, COOLDOWN_INITIAL, "recovery resets the cooldown");
    }

    #[test]
    fn a_genuinely_broken_worker_backs_off_but_stays_bounded() {
        // Repeated relapses can never exceed the cap, so a hard-broken
        // worker retries at most once per COOLDOWN_CAP — never dead,
        // never a spin.
        let mut cooldown = COOLDOWN_INITIAL;
        for _ in 0..50 {
            let (phase, cd) = apply_trial_outcome(TrialOutcome::Relapsed, cooldown);
            cooldown = cd;
            assert_eq!(phase, BreakerPhase::Open);
            assert!(cooldown <= COOLDOWN_CAP);
        }
        assert_eq!(cooldown, COOLDOWN_CAP);
    }
}
