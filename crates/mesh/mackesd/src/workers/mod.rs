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

use std::sync::Arc;

use anyhow::Context;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tracing::{error, info, warn};

/// Shutdown signal handed to every worker. Workers should `select!`
/// on the underlying `watch::Receiver` so they exit promptly when
/// the supervisor requests stop. Cloning is cheap (it's a watch
/// receiver under the hood).
#[derive(Clone, Debug)]
pub struct ShutdownToken {
    pub(crate) rx: watch::Receiver<bool>,
}

impl ShutdownToken {
    /// Construct a token from a raw watch receiver. Crate-private —
    /// the supervisor's [`Supervisor::token`] is the public surface
    /// for normal callers; this constructor lets sibling worker
    /// modules build a token from a freshly-paired sender/receiver
    /// pair in their unit tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn from_receiver(rx: watch::Receiver<bool>) -> Self {
        Self { rx }
    }

    /// `true` once shutdown has been requested. Workers should poll
    /// or `await` on [`Self::changed`] for prompt notification.
    #[must_use]
    pub fn is_shutdown(&self) -> bool {
        *self.rx.borrow()
    }

    /// Async wait for shutdown. Resolves the first time the
    /// supervisor flips the flag to `true`. Returns immediately if
    /// shutdown was already requested.
    pub async fn wait(&mut self) {
        if self.is_shutdown() {
            return;
        }
        // `changed()` errors only when the sender is dropped — at
        // which point we're shutting down anyway, so treat it as
        // shutdown-requested.
        let _ = self.rx.changed().await;
    }
}

// v2.0.0 Phase B workers reparented under workers/. Each is a thin
// adapter over an existing sync implementation today; they grow real
// bodies as Phase B fills in.
pub mod ansible_pull;
// EPIC-SYNC-APP-CONFIG (Q26) — native-Rust app-config sync
// (Sublime Music / Delfin). Replaces the retired `media_sync`
// subprocess worker + the Python `media_sync_daemon.py` it drove.
pub mod app_sync;
pub mod heartbeat;
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
// KDC2-6.6 — legacy `kdc_bridge` retired alongside the upstream
// kdeconnectd wrapper. The native KDC host worker
// (`workers::kdc_host`) replaces it in the v2.1+ stack.
pub mod kdc_host;
pub mod mdns_relay;
pub mod mesh_latency;
pub mod mesh_router;
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
// NF-1.5 (v2.5) — Lighthouse-side TCP/443 covert listener.
// Binds the TLS 1.3 listener on :443, spawns one demux pump
// per accepted stream (TLS ↔ UDP 127.0.0.1:4242). Inner Nebula
// stack runs unmodified.
pub mod nebula_https_listener;
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
// MESHFS-2.1 (v5.0.0) — LizardFS mesh-storage fleet supervisor.
// Silent no-op when the mfsmaster/mfschunkserver binaries are
// absent or the overlay-ip publish file doesn't yet exist.
pub mod meshfs_worker;
// FWMON-2..4 (v5.0.0) — firewall-denied event monitor. Reads
// kernel journal entries logged by firewalld's LogDenied=all
// setting, filters overlay + established traffic, appends
// net-new denials to <mesh-storage>/firewall/<host>.jsonl,
// trims 7-day window, and fires Bus alert on threshold.
// Separate from `firewall_preset` (port-open convergence).
pub mod firewall_monitor;
// VIRT-1 (v5.0.0) — unified KVM + Podman compute inventory.
// Polls `virsh list --all --uuid` + `virsh dominfo`/`domblklist`/
// `domstats` for KVM guests and `podman ps`/`podman stats` for
// containers on a 10 s tick; publishes per-peer inventory to
// `compute/inventory/<peer-nebula-addr>` per docs/design/v5.0.0-
// compute.md §3. Silent no-op when virsh/podman are absent.
pub mod compute_registry;
// VIRT-5 (v5.0.0) — VM Nebula cert signing via Bus. Every peer
// drains `action/compute/cert-sign-request`; on the CA peer
// (detected by ~/.config/mde/nebula/ca.key) calls `nebula-cert
// sign` and replies on `reply/<ulid>`; non-CA peers advance the
// cursor and skip. Topic shape locked to `action/<domain>/<verb>`
// per Q96 + rpc.rs convention (design doc §3's per-ULID notation
// reinterpreted accordingly).
pub mod cert_authority;
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
// notification_relay retired in BUS-4.2 (2026-05-26). Cross-peer
// notification routing is now handled by the BUS-4.4 FDO bridge:
// every Notify call publishes to `fdo/<app>` on the Mackes Bus,
// and every peer subscribes via the standard Bus path.
// perf retired 2026-05-27 (TUNE-3.b): the Rust port of
// `mackes/mesh_perf.py`'s read-only sysfs surface was destined for
// the Workbench Mesh Performance panel (Python GTK), which retires
// under EPIC-RETIRE-PY-WORKBENCH. The Iced mde-workbench panel
// equivalent doesn't yet exist; if a future v2.x panel needs the
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
// NF-21.3 — owns the firewalld preset that opens Nebula's
// UDP/4242 (all peers) + TCP/443 (lighthouses) inbound. Replaces
// mesh_nebula.py::apply_nebula_firewall_preset so the Python
// helper can retire (DEAD-2.14 plan).
pub mod firewall_preset;
pub mod stun_gather;
pub mod subprocess_tick;
// thumbnailer retired 2026-05-26 (TUNE-3.b): the worker module
// shelled out to `mackes/mesh_thumbnailer.py` which already
// retired with EPIC-RETIRE-PY-WORKBENCH; Thunar / GTK thumbnail
// dispatch is irrelevant in the v2.0+ Wayland-only sway + mde-
// files (Cosmic Files fork) stack. No live consumer of the pure
// helpers existed in tree.
// VV-2 (v4.1.0) — voice-config worker that owns the
// /var/lib/mackesd/voice-desired.json document + triggers
// `systemctl try-reload-or-restart` on kamailio-mde +
// rtpengine-mde when it changes.
pub mod voice_config;
pub mod wol;
// BUS-1.1 (v6.x Mackes Bus) — `mde-bus` subprocess supervisor.
// Spawns `mde-bus daemon`, restarts on exit, gracefully degrades
// when the binary is absent (development boxes that don't have
// the RPM installed yet). The outer supervisor's
// RestartPolicy::Always wraps this worker; inner respawn cooldown
// paces clean-exit restarts. Broker + mDNS + persistence land
// inside the binary in BUS-1.2/1.3/1.4.
pub mod bus_supervisor;
// BUS-5.1 — clipboard daemon supervisor. Spawns one `mde-clipd` process
// per Wayland session; idles when $WAYLAND_DISPLAY is unset.
pub mod clipd_supervisor;
// SWAY-4 (v6.0 Q89/Q94) — per-window mark state. Bridges the sway-native
// mark API to the Mackes Bus so Portal (mark pills), border_tinter, and
// elevation-shadow workers can subscribe to mark deltas without querying
// the compositor. Ported from HYP-14 (mde-x c913bed1): pure MarksStore +
// Bus action responder; IPC replaced hyprland-rs → swayipc_async.
pub mod marks_state;
// Portal-41 (v6.0 R12-Q1, 2026-05-26) — auto-derived workspace names.
// Subscribes to sway's Window event stream; debounces 200 ms; renames
// the focused workspace to `<num>: <app_id>` whenever the focused
// window changes. Operator-set names are preserved.
pub mod workspace_namer;
// Portal-48 (v6.0 R12-Q8 + R12-Q10, 2026-05-26) — auto-mark daemon.
// Subscribes to sway window::new events; classifies app_id against a
// 25-entry compile-time taxonomy table (editor/web/shell/mail/chat);
// fires `mark --add <category>` when matched + no existing marks.
// The cross-peer zbus surface from the original Portal-48 spec is
// deferred to Portal-48.b per Q20+Q96 Bus-migration lock.
pub mod auto_mark;
// Portal-42 (v6.0 R12-Q2, 2026-05-26) — tag-driven workspace output
// assignment. Subscribes to sway workspace::init events; looks up the
// owning tag for each new workspace from Portal-18.a's tag store;
// fires `move workspace to output <name>` when the tag has a
// `preferred_output` field set.
pub mod workspace_router;
// Portal-44 (v6.0 R12-Q4, 2026-05-26) — per-tag default_layout
// enforcement. Subscribes to sway window::new events; flips the
// new window's workspace to the owning tag's `default_layout`
// when it's the only window AND the current layout differs.
pub mod tag_layout;
// Portal-54 (v6.0 R12-Q16, 2026-05-26) — per-tag autostart.
// Subscribes to sway workspace::init events; fires `exec <cmd>` for
// each app_id in the owning tag's `autostart` list, once per
// workspace per mded-lifetime.
pub mod tag_autostart;
// Portal-56 (v6.0 R12-Q21, 2026-05-26) — per-workspace focused-
// border tinting. Subscribes to sway workspace::focus events;
// fires `client.focused` with the owning tag's group_color (or
// the platform Carbon blue when no tag owns the workspace).
pub mod border_tinter;
// Portal-57.a (v6.0 R12-Q22, 2026-05-26) — channel 1 of the
// urgent-window three-channel cascade. Subscribes to sway
// window::urgent events; spawns `mde-bus publish` to the
// `bus/mbadge/pulse` topic with the urgent payload. Portal mini-
// tree + Dock-segment channels (2 + 3) ship as Portal-57.b once
// their UI prerequisites land.
pub mod urgency_router;
// Portal-52.a (v6.0 R12-Q13, 2026-05-26) — sway session-restore
// worker (workspace-structure half). 5s snapshot of workspaces +
// outputs + layouts to <XDG_DATA_HOME>/mde/session.json; first-
// run-after-start restore replays the structure via swayipc.
// Window-placeholder swallows ship as Portal-52.b.
pub mod session_persist;
// Portal-53.a (v6.0 R12-Q14, 2026-05-27) — window-rules subsystem
// backend. Reads `~/.config/mde/window-rules.toml`, applies each
// rule via swayipc `for_window` registrations on startup +
// mtime-poll-detected TOML changes. Hub right-click modal +
// Control panel CRUD UIs ship as Portal-53.b/.c.
pub mod window_rules;
// HYP-8.5.watch (v6.5, 2026-05-27) — mtime-poll watcher on
// `~/.config/mde/tags/` that publishes `event/config/tags/loaded`
// + `event/config/tags/unloaded` on per-file diffs at 5 s cadence.
// Pairs with HYP-8.5's startup wire to give the operator runtime
// reload of tag manifests without daemon restart.
pub mod tag_manifest_watcher;
// Portal-47 (v6.0 R12-Q7, 2026-05-29) — one-shot startup worker
// that writes per-tag sway mode blocks into
// `~/.config/sway/config.d/mde-tag-modes.conf` and calls
// `swaymsg reload` if the content changed. Backing for Hub's
// "Enter mode" action (HubMenuEnterMode).
pub mod tag_mode_writer;
// SWAY-8 (Q52–Q54, 2026-05-30) — mtime-poll config watcher + EDID
// hardware overlay writer. Polls ~/.config/sway/ and
// ~/.local/share/mde/mesh-storage/sway/ for changes; fires
// `swaymsg reload` on any diff. Also writes
// ~/.config/sway/config.d/00-hardware.conf at startup from
// `swaymsg -t get_outputs` (Q53 per-peer EDID overlay).
pub mod sway_config_watcher;
// TUNE-16.d (2026-05-30) — Q22 8-peer cap counter. Counts enrolled
// `role = 'peer'` nodes (phones count, federated external-mesh peers
// are excluded by virtue of not appearing in the local store). Writes
// ~/.cache/mde/peer-cap.json every 30 s; publishes to
// mesh/peer-cap/updated Bus topic for real-time UI consumers.
pub mod peer_cap;

/// Every worker registered with the supervisor implements this
/// trait. The trait is `async_trait` because the supervisor stores
/// `Box<dyn Worker>`, which native async-fn-in-trait doesn't yet
/// support.
#[async_trait::async_trait]
pub trait Worker: Send + 'static {
    /// Short, stable identifier used in logs + `mackesd healthz`
    /// output. Should be `kebab-case` and match the matching
    /// `crates/mackesd/src/workers/<name>.rs` module name (e.g.
    /// `clipd_supervisor`, `mdns`, `notifications-server`).
    fn name(&self) -> &'static str;

    /// Body of the worker. Runs on the tokio runtime until
    /// `shutdown.wait().await` resolves OR the body returns. Errors
    /// returned here surface to the supervisor's restart logic
    /// (Phase B); for Phase A the supervisor simply logs and exits
    /// the join.
    async fn run(&mut self, shutdown: ShutdownToken) -> anyhow::Result<()>;
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
    /// long-running watchers (`clipd_supervisor`, `mdns`, etc.).
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

/// Minimal in-process supervisor. Phase A scope: spawn each worker
/// once, log restarts, broadcast shutdown via a watch channel,
/// `join_all` on stop. Phase B re-wraps this in `task-supervisor` for
/// per-task back-off + add/remove-at-runtime semantics.
pub struct Supervisor {
    shutdown_tx: Arc<watch::Sender<bool>>,
    shutdown_rx: watch::Receiver<bool>,
    join: JoinSet<(&'static str, anyhow::Result<()>)>,
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
        }
    }

    /// Issue every spawned worker a fresh shutdown token cloned from
    /// our channel.
    #[must_use]
    pub fn token(&self) -> ShutdownToken {
        ShutdownToken {
            rx: self.shutdown_rx.clone(),
        }
    }

    /// Spawn a worker. The supervisor honors `Spawn::policy` for
    /// restart decisions (Phase A: `Never`/`OnFailure`/`Always`
    /// implemented via a self-spawning loop inside `run_one`).
    pub fn spawn(&mut self, spec: Spawn) {
        let token = self.token();
        let Spawn { mut worker, policy } = spec;
        let name = worker.name();
        let shutdown = token;
        self.join.spawn(async move {
            // `break outcome` carries the worker's final result out
            // of the loop, so we don't need a pre-initialized
            // `last_result` slot (which would dead-code in the
            // can-never-be-empty `loop {}`).
            let last_result: anyhow::Result<()> = loop {
                info!(worker = %name, "starting worker");
                let token_for_run = shutdown.clone();
                let outcome = worker.run(token_for_run).await;
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
                if !should_restart {
                    break outcome;
                }
                if shutdown.is_shutdown() {
                    info!(worker = %name, "shutdown requested; not restarting");
                    break outcome;
                }
                // Phase A: fixed 250 ms back-off so a hot-looping
                // bug doesn't pin a core. Phase B replaces this
                // with task-supervisor's exponential back-off.
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                // No `shutdown.wait().await` here — that would block
                // restarts indefinitely. The 250 ms sleep is the
                // restart delay; the worker's next `run()` should
                // observe `shutdown.is_shutdown()` itself.
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
        Ok(self.join_all().await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
}
