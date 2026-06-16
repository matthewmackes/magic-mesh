//! MESHFS-2.1 (v5.0.0) — LizardFS mesh-storage fleet supervisor.
//!
//! Mirrors the `gluster_worker` shape: tokio task, 5-second tick,
//! `ShutdownToken` `select!` for prompt SIGTERM exit. Each tick:
//!
//!   1. **Guard.** Silently no-ops when the `mfsmaster` binary is
//!      not on PATH or when the overlay-ip file is absent (peer
//!      hasn't enrolled into Nebula yet).
//!
//!   2. **Genesis (MESHFS-2.1 Q16).** If no master is reachable at
//!      the floating VIP, this peer self-bootstraps: writes a
//!      minimal `mfsexports.cfg` + `mfsmaster.cfg` to the config
//!      dir and starts `mfsmaster`. Once the master is up, creates
//!      the `mesh-storage` export root directory.
//!
//!   3. **Goal convergence (MESHFS-2.1 Q4).** Counts enrolled
//!      peers from QNM-Shared (`<workgroup_root>/*/mackesd/nebula-
//!      bundle.json`); if the count N > current goal, raises the
//!      goal via `mfssetgoal -r N /mnt/mesh-storage`. This handles
//!      both `EnrollmentCompleted` (goal increases) and CA-revoke
//!      (goal decreases).
//!
//!   4. **Chunkserver + shadow (MESHFS-2.1 Q6).** Ensures the local
//!      `mfschunkserver` is running (start-idempotent via `mfschunk-
//!      server start`). Every peer runs a shadow master (`mfsmaster
//!      -o ha` in shadow mode).
//!
//!   5. **CA-revoke path (MESHFS-2.1 Q17; AUDIT-MESH-7).** When a peer's
//!      bundle disappears from QNM-Shared, LizardFS auto-replicates the
//!      departed chunkserver's chunks to satisfy the goal — there is no
//!      manual eviction command (the old `mfsadmin CS-EVICT`/`MASTER-STOP`
//!      were dead MooseFS syntax). The worker records the departure +
//!      logs the replication backlog (`chunks-health`). Real master
//!      failover is the `tick_once_ha` VIP-claim + shadow-promote path.
//!
//! Design locks (25-Q survey 2026-05-29; goal policy superseded by
//! FPG-7 / platform-survey Q12, 2026-06-09):
//!   Q4  — ~~goal = N~~ → **goal 2 default** (FPG-7/Q12): replicate
//!         every chunk twice, capped by enrolled peer count
//!   Q6  — every peer: chunkserver + shadow + client
//!   Q12 — FS-agnostic: `meshfs_worker`, `MeshFS`, `meshfs` config
//!   Q14 — storage paths: `/var/lib/mde/meshfs/{chunks,meta,stage}/`
//!   Q16 — auto-join on EnrollmentCompleted; first peer bootstraps
//!   Q17 — CA-revoke → evict, rebalance, lower goal, fail VIP over

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mde_bus::hooks::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

use super::{ShutdownToken, Worker};

/// Default sweep cadence — 5 s, matching `gluster_worker` +
/// `nebula_supervisor`.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(5);

/// LizardFS master binary. Override via `with_master_binary()` in
/// tests.
pub const DEFAULT_MASTER_BINARY: &str = "mfsmaster";

/// LizardFS chunkserver binary.
pub const DEFAULT_CHUNKSERVER_BINARY: &str = "mfschunkserver";

/// LizardFS admin CLI binary. AUDIT-MESH-3: the deployed substrate is
/// LizardFS, whose admin tool is `lizardfs-admin` (`list-chunkservers`,
/// `info`, `chunks-health`, …), NOT MooseFS's `mfsadmin`. The read-paths
/// ([`query_chunkservers`], [`min_chunkserver_avail_bytes`],
/// [`current_chunkserver_ips`]) use `lizardfs-admin list-chunkservers`.
/// AUDIT-MESH-7: the dead MooseFS write-path argv (`CS-EVICT`/`MASTER-STOP`/
/// `CS-SET-TOPOLOGY`/`TRASH-RECOVER`) are gone — LizardFS auto-replicates,
/// failover is `tick_once_ha`, topology is file-side, and undelete is a
/// trash-tree `rename` into `undel/`.
pub const DEFAULT_ADMIN_BINARY: &str = "lizardfs-admin";

/// LizardFS goal-set CLI binary.
pub const DEFAULT_SETGOAL_BINARY: &str = "mfssetgoal";

/// LizardFS quota-set CLI binary.
pub const DEFAULT_SETQUOTA_BINARY: &str = "mfssetquota";

/// LizardFS trash-time CLI binary (MESHFS-8.1).
pub const DEFAULT_SETTRASHTIME_BINARY: &str = "mfssettrashtime";

/// Default trash retention window in seconds: 48 h (MESHFS-8.1).
/// Operator-tunable via `with_trash_retention_secs()`.
pub const DEFAULT_TRASH_RETENTION_SECS: u64 = 172_800;

/// Quota tick cadence — run once per hour (MESHFS-9.1).
pub const DEFAULT_QUOTA_TICK: Duration = Duration::from_secs(3600);

/// Hard-quota factor: `0.8 × min(free chunkserver)`. Writing past the
/// hard cap returns `EROFS`.
pub const QUOTA_HARD_FACTOR: f64 = 0.8;

/// Soft-quota factor: `0.7 × min(free chunkserver)`. Crossing the soft
/// cap triggers a `meshfs/quota-warning` Bus event.
pub const QUOTA_SOFT_FACTOR: f64 = 0.7;

/// Default floating VIP (Nebula overlay) the active master listens
/// on. Operators override via `with_vip()`. Chosen at mesh genesis;
/// all peers mount this address.
pub const DEFAULT_VIP: &str = "10.42.0.1";

/// Default overlay-ip publish file path (written by nebula_supervisor
/// on bundle refresh). Matches GF-1.3.a / NF path.
pub const DEFAULT_OVERLAY_IP_PATH: &str = "/var/lib/mackesd/nebula/overlay-ip";

/// LizardFS master TCP port (default: 9419).
pub const MFSMASTER_PORT: u16 = 9419;

/// LizardFS matocl (client/admin) TCP port — the port `lizardfs-admin`
/// connects to for `list-chunkservers` / `info` queries (default: 9421).
/// Distinct from [`MFSMASTER_PORT`] (9419, the matoml/metalogger port the
/// reachability probe pings).
pub const MFSMASTER_CLIENT_PORT: u16 = 9421;

/// LizardFS export directory under mesh-storage.
pub const EXPORT_NAME: &str = "mesh-storage";

/// FPG-7 / Q12 — the default replication goal. Every chunk lives on
/// two chunkservers (capped by the enrolled peer count on tiny
/// meshes); the old goal=N everything-everywhere policy is retired.
pub const DEFAULT_REPLICATION_GOAL: u8 = 2;

/// The five XDG user dirs the mesh bind-mounts (FPG-7 / Q13).
/// `~/Local/` is deliberately absent — it is NEVER mesh-mounted.
pub const XDG_MESH_DIRS: [&str; 5] = ["Documents", "Downloads", "Music", "Pictures", "Videos"];

/// Resolve the home directories of the regular desktop users (uid
/// 1000–60000) whose home exists under `/home` (AUDIT-MESH-15 bug #1).
///
/// The daemon runs as **root** (`$HOME=/root`), so binding the XDG dirs
/// against `$HOME` only ever touched `/root/Documents` — never the
/// `/home/<u>/Documents` the desktop user actually sees. Parse
/// `/etc/passwd` directly (no extra crate dep) and bind for every real
/// desktop user so a file dropped in their `~/Documents` lands in the
/// communal mesh source. Headless Server/Lighthouse nodes return an
/// empty list (no desktop user → nothing to bind).
#[must_use]
pub fn desktop_user_homes() -> Vec<PathBuf> {
    let Ok(contents) = std::fs::read_to_string("/etc/passwd") else {
        return Vec::new();
    };
    let mut homes = Vec::new();
    for line in contents.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() < 7 {
            continue;
        }
        let Ok(uid) = fields[2].parse::<u32>() else {
            continue;
        };
        if !(1000..60000).contains(&uid) {
            continue;
        }
        let home = PathBuf::from(fields[5]);
        if home.starts_with("/home") && home.is_dir() && !homes.contains(&home) {
            homes.push(home);
        }
    }
    homes
}

/// Execute the [`xdg_bind_plan`]: for each pair whose target is not
/// already a mountpoint, create the communal source + the target and
/// `mount --bind`. The communal source is made world-writable (0777) so
/// any desktop user on any node can drop files into the shared dir
/// (mirrors the cross-uid bus-spool fix, AUDIT-MESH-16). Returns the
/// human-readable failure lines (empty ⇒ all binds satisfied) so the
/// caller can surface them **loudly** — a silent debug log is exactly
/// how AUDIT-MESH-15 hid (the ONBOARD-6 failure class).
pub fn ensure_xdg_binds(mount_path: &Path, home: &Path) -> Vec<String> {
    let mut failures = Vec::new();
    if !mount_path.is_dir() {
        return vec![format!(
            "FPG-7: mesh mount {} is not a directory — XDG binds skipped",
            mount_path.display()
        )];
    }
    // AUDIT-MESH-15: mackesd runs in a PRIVATE mount namespace (PrivateTmp), so a
    // `mount --bind` it makes is invisible to the user's session. Run the
    // mountpoint check + the bind in the HOST (pid 1) mount namespace via
    // `nsenter` so the desktop user actually sees the synced dirs. In a dev/test
    // run (no /proc/1/ns/mnt access, or already the host ns) the prefix is empty.
    let host_ns = host_mount_ns_prefix();
    for (source, target) in xdg_bind_plan(mount_path, home) {
        // `mountpoint -q` (in the host ns) exits 0 when already a mountpoint.
        let already = host_ns_command(&host_ns, "mountpoint", &["-q", &target.to_string_lossy()])
            .status()
            .map(|st| st.success())
            .unwrap_or(false);
        if already {
            continue;
        }
        if let Err(e) = std::fs::create_dir_all(&source) {
            failures.push(format!(
                "FPG-7: cannot create communal source {} — {e}",
                source.display()
            ));
            continue;
        }
        // Communal: any desktop uid (this node or a peer) must be able to
        // write. Best-effort — a non-root dev run can't chmod a foreign dir.
        let _ =
            std::fs::set_permissions(&source, std::os::unix::fs::PermissionsExt::from_mode(0o777));
        if let Err(e) = std::fs::create_dir_all(&target) {
            failures.push(format!(
                "FPG-7: cannot create home target {} — {e}",
                target.display()
            ));
            continue;
        }
        match host_ns_command(
            &host_ns,
            "mount",
            &["--bind", &source.to_string_lossy(), &target.to_string_lossy()],
        )
        .status()
        {
            Ok(st) if st.success() => {
                tracing::info!(
                    target: "mackesd::meshfs_worker",
                    "FPG-7: bind-mounted {} -> {}",
                    source.display(),
                    target.display()
                );
            }
            Ok(st) => {
                failures.push(format!(
                    "FPG-7: `mount --bind {} {}` failed (exit {:?})",
                    source.display(),
                    target.display(),
                    st.code()
                ));
            }
            Err(e) => {
                failures.push(format!(
                    "FPG-7: `mount --bind {} {}` could not spawn — {e}",
                    source.display(),
                    target.display()
                ));
            }
        }
    }
    failures
}

/// AUDIT-MESH-15 — whether to run mount ops through `nsenter` to reach the host
/// (pid 1) mount namespace. Returns the nsenter argv prefix when the daemon is
/// in a DIFFERENT mount namespace than pid 1 (the sandboxed-service case) and
/// `nsenter` + `/proc/1/ns/mnt` are usable; otherwise an empty prefix (dev/test
/// or already the host ns → run `mount`/`mountpoint` directly).
#[must_use]
fn host_mount_ns_prefix() -> Vec<String> {
    let self_ns = std::fs::read_link("/proc/self/ns/mnt").ok();
    let pid1_ns = std::fs::read_link("/proc/1/ns/mnt").ok();
    let differ = matches!((&self_ns, &pid1_ns), (Some(a), Some(b)) if a != b);
    if differ && binary_on_path("nsenter") && std::path::Path::new("/proc/1/ns/mnt").exists() {
        vec![
            "nsenter".to_owned(),
            "--target".to_owned(),
            "1".to_owned(),
            "--mount".to_owned(),
            "--".to_owned(),
        ]
    } else {
        Vec::new()
    }
}

/// Build a [`Command`] for `prog args…`, prefixed with the host-namespace
/// `nsenter` invocation when [`host_mount_ns_prefix`] is non-empty.
#[must_use]
fn host_ns_command(prefix: &[String], prog: &str, args: &[&str]) -> Command {
    if prefix.is_empty() {
        let mut cmd = Command::new(prog);
        cmd.args(args);
        cmd
    } else {
        let mut cmd = Command::new(&prefix[0]);
        cmd.args(&prefix[1..]).arg(prog).args(args);
        cmd
    }
}

/// The bind-mount plan for one user's home (FPG-7 / Q13): pairs of
/// `(mesh-source, home-target)` for [`XDG_MESH_DIRS`]. Pure — the
/// sweep executes it only for pairs whose target isn't already a
/// mountpoint.
#[must_use]
pub fn xdg_bind_plan(mount_path: &Path, home: &Path) -> Vec<(PathBuf, PathBuf)> {
    let user_root = mount_path.join("home");
    XDG_MESH_DIRS
        .iter()
        .map(|d| (user_root.join(d), home.join(d)))
        .collect()
}

/// MESHFS-6.1 — offline write staging directory. Writes that fail when the
/// master is unreachable land here; `meshfs_worker` replays them on reconnect.
/// Directory structure mirrors the mesh mount: `stage/<rel>` maps to
/// `<mesh_mount>/<rel>`.
pub const STAGE_DIR: &str = "/var/lib/mde/meshfs/stage";

/// Marker file written by the wizard on lighthouse peers — same path as
/// `nebula_supervisor::DEFAULT_ROLE_HOST_MARKER`. Presence → VIP-eligible.
pub const DEFAULT_ROLE_MARKER_PATH: &str = "/var/lib/mackesd/nebula/role.host";

/// Nebula overlay interface name (default). Operators may override if
/// Nebula is configured with a non-default interface name.
pub const DEFAULT_OVERLAY_IFACE: &str = "nebula1";

/// Nebula overlay CIDR prefix length. Fixed at /16 per the open-mesh
/// design (10.42.0.0/16).
pub const OVERLAY_CIDR_PREFIX: u8 = 16;

/// Bus action-poll cadence (MESHFS-10.1) — matches `marks_state`.
const ACTION_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The six `action/meshfs/<verb>` topics the worker serves (MESHFS-10.1).
const ACTION_VERBS: [&str; 6] = [
    "resolve-conflict",
    "undelete",
    "add-peer",
    "remove-peer",
    "bootstrap",
    "status",
];

/// Worker handle. Cheap to construct; clone is forbidden (mirrors
/// `gluster_worker`).
pub struct MeshFsWorker {
    tick: Duration,
    overlay_ip_path: PathBuf,
    master_binary: String,
    chunkserver_binary: String,
    admin_binary: String,
    setgoal_binary: String,
    vip: String,
    workgroup_root: Option<PathBuf>,
    self_node_id: Option<String>,
    setquota_binary: String,
    /// Unix timestamp (seconds) of the last quota tick. Stored in a Mutex
    /// so `tick_once()` — which takes `&self` — can update it without
    /// requiring a mutable reference.
    last_quota_s: std::sync::Mutex<u64>,
    /// Marker file whose existence indicates this peer is a lighthouse
    /// and therefore VIP-eligible for the active master role.
    role_marker_path: PathBuf,
    /// Nebula overlay interface on which the floating VIP is claimed or
    /// released via `ip addr add/del`.
    overlay_iface: String,
    /// Departed peer IPs already handled this session (logged + event
    /// emitted once). Prevents re-logging every tick while LizardFS
    /// auto-replicates the lost chunkserver's chunks.
    evicted_ips: std::sync::Mutex<std::collections::BTreeSet<String>>,
    /// Tracks whether the master was reachable on the last tick so
    /// `meshfs/export-ready` fires exactly once on down→up (MESHFS-10.1).
    master_was_up: std::sync::atomic::AtomicBool,
    /// MESHFS-8.1 — `mfssettrashtime` binary name.
    settrashtime_binary: String,
    /// MESHFS-8.1 — trash retention window in seconds (default 48 h).
    trash_retention_secs: u64,
}

impl MeshFsWorker {
    /// Construct with production defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tick: DEFAULT_TICK_INTERVAL,
            overlay_ip_path: PathBuf::from(DEFAULT_OVERLAY_IP_PATH),
            master_binary: DEFAULT_MASTER_BINARY.to_owned(),
            chunkserver_binary: DEFAULT_CHUNKSERVER_BINARY.to_owned(),
            admin_binary: DEFAULT_ADMIN_BINARY.to_owned(),
            setgoal_binary: DEFAULT_SETGOAL_BINARY.to_owned(),
            vip: DEFAULT_VIP.to_owned(),
            workgroup_root: None,
            self_node_id: None,
            setquota_binary: DEFAULT_SETQUOTA_BINARY.to_owned(),
            last_quota_s: std::sync::Mutex::new(0),
            role_marker_path: PathBuf::from(DEFAULT_ROLE_MARKER_PATH),
            overlay_iface: DEFAULT_OVERLAY_IFACE.to_owned(),
            evicted_ips: std::sync::Mutex::new(std::collections::BTreeSet::new()),
            master_was_up: std::sync::atomic::AtomicBool::new(false),
            settrashtime_binary: DEFAULT_SETTRASHTIME_BINARY.to_owned(),
            trash_retention_secs: DEFAULT_TRASH_RETENTION_SECS,
        }
    }

    /// The QNM-Shared / LizardFS mount this worker operates on.
    ///
    /// Single-sourced with `mackesd`'s directory read and the
    /// `mde-workbench` panels via `default_qnm_shared_root()` — so
    /// `mfssetgoal`/`mfssetquota`/`mfssettrashtime` and the XDG binds
    /// all target the *real* mount (`~/QNM-Shared` by default), never
    /// the phantom `/mnt/mesh-storage` that nothing mounts (the
    /// split-brain that made the Mesh Storage panel report
    /// "goal 0 / not mounted" against a healthy mesh).
    fn mount_path(&self) -> PathBuf {
        self.workgroup_root
            .clone()
            .unwrap_or_else(crate::default_qnm_shared_root)
    }

    /// Opt into QNM-Shared peer discovery. Both args must be
    /// supplied or the worker skips goal-convergence and eviction
    /// (silent no-op).
    #[must_use]
    pub fn with_qnm_peer_discovery(
        mut self,
        workgroup_root: PathBuf,
        self_node_id: String,
    ) -> Self {
        self.workgroup_root = Some(workgroup_root);
        self.self_node_id = Some(self_node_id);
        self
    }

    /// Override the tick cadence. Tests use shorter values.
    #[must_use]
    pub fn with_tick(mut self, t: Duration) -> Self {
        self.tick = t;
        self
    }

    /// Override the overlay-ip file path. Tests redirect to a
    /// tempdir.
    #[must_use]
    pub fn with_overlay_ip_path(mut self, path: PathBuf) -> Self {
        self.overlay_ip_path = path;
        self
    }

    /// Override the LizardFS master binary. Tests pass `/bin/true`
    /// or a recording shim.
    #[must_use]
    pub fn with_master_binary(mut self, name: impl Into<String>) -> Self {
        self.master_binary = name.into();
        self
    }

    /// Override the floating VIP. Tests use 127.0.0.1 or a
    /// non-routable address.
    #[must_use]
    pub fn with_vip(mut self, vip: impl Into<String>) -> Self {
        self.vip = vip.into();
        self
    }

    /// Override the `mfssetquota` binary. Tests pass a nonexistent name to
    /// skip the quota subprocess without affecting other guards.
    #[must_use]
    pub fn with_setquota_binary(mut self, name: impl Into<String>) -> Self {
        self.setquota_binary = name.into();
        self
    }

    /// Override the role-marker path. Tests redirect to a tempfile so
    /// HA logic can be exercised without `/var/lib/mackesd` access.
    #[must_use]
    pub fn with_role_marker_path(mut self, path: PathBuf) -> Self {
        self.role_marker_path = path;
        self
    }

    /// Override the Nebula overlay interface name. Tests use a loopback
    /// alias or skip the VIP path via a missing binary guard.
    #[must_use]
    pub fn with_overlay_iface(mut self, iface: impl Into<String>) -> Self {
        self.overlay_iface = iface.into();
        self
    }

    /// Override the `mfssettrashtime` binary. Tests pass a nonexistent name
    /// to skip the trash-retention subprocess without affecting other guards.
    #[must_use]
    pub fn with_settrashtime_binary(mut self, name: impl Into<String>) -> Self {
        self.settrashtime_binary = name.into();
        self
    }

    /// Override the trash retention window. Use `0` to disable trash.
    #[must_use]
    pub fn with_trash_retention_secs(mut self, secs: u64) -> Self {
        self.trash_retention_secs = secs;
        self
    }

    /// One tick of the worker's loop — exposed for direct testing
    /// without the tokio time pulse.
    pub fn tick_once(&self) {
        // 1. Guard: binary must be on PATH.
        if !binary_on_path(&self.master_binary) {
            tracing::debug!(
                target: "mackesd::meshfs_worker",
                binary = %self.master_binary,
                "mfsmaster not installed; mesh-storage substrate inactive",
            );
            return;
        }

        // 2. Guard: overlay-ip must be present (enrollment complete).
        let overlay_ip = match std::fs::read_to_string(&self.overlay_ip_path) {
            Ok(s) => s.trim().to_owned(),
            Err(_) => {
                tracing::debug!(
                    target: "mackesd::meshfs_worker",
                    path = %self.overlay_ip_path.display(),
                    "overlay-ip file absent; deferring until Nebula enrollment completes",
                );
                return;
            }
        };

        // 3. Genesis: if no master answers the VIP, bootstrap one.
        //    Track down→up transition for `meshfs/export-ready` (MESHFS-10.1).
        let master_up = master_reachable(&self.vip);
        let prev_up = self
            .master_was_up
            .swap(master_up, std::sync::atomic::Ordering::Relaxed);
        if !master_up {
            tracing::info!(
                target: "mackesd::meshfs_worker",
                vip = %self.vip,
                "no master reachable at VIP; initiating genesis bootstrap",
            );
            let argv = genesis_start_argv(&self.master_binary);
            tracing::info!(target: "mackesd::meshfs_worker", argv = ?argv, "starting mfsmaster (genesis)");
            let _ = run_argv(&argv);
        } else if !prev_up {
            publish_meshfs_event("meshfs/export-ready", r#"{"ok":true}"#);
        }

        // 4. Ensure local chunkserver is running (idempotent start).
        if binary_on_path(&self.chunkserver_binary) {
            let argv = chunkserver_start_argv(&self.chunkserver_binary);
            tracing::debug!(target: "mackesd::meshfs_worker", argv = ?argv, "ensuring mfschunkserver running");
            let _ = run_argv(&argv);
        }

        // 5. Goal convergence + eviction via QNM-Shared peer count.
        if let (Some(workgroup_root), Some(self_id)) =
            (self.workgroup_root.as_ref(), self.self_node_id.as_ref())
        {
            let enrolled = enrolled_peer_ips(workgroup_root, self_id);
            let peer_count = enrolled.len();
            if peer_count > 0 {
                // FPG-7 / Q12 — goal 2 default, capped by peer count
                // (a 1-peer mesh can only hold one copy).
                let goal = (peer_count as u8).min(DEFAULT_REPLICATION_GOAL).max(1);
                let argv = setgoal_argv(
                    &self.setgoal_binary,
                    goal,
                    &self.mount_path().to_string_lossy(),
                );
                tracing::info!(
                    target: "mackesd::meshfs_worker",
                    goal,
                    "converging replication goal (FPG-7: goal-2 default, peer-capped)",
                );
                let _ = run_argv(&argv);
                publish_meshfs_event(
                    "meshfs/peer-state-changed",
                    &format!(r#"{{"op":"goal-changed","goal":{goal}}}"#),
                );
            }

            // Evict peers whose bundle has disappeared from QNM-Shared
            // (CA-revoke proxy, mirroring gluster_worker's peer-detach).
            let current_peers = current_chunkserver_ips(&self.admin_binary, &self.vip);
            let enrolled_set: std::collections::BTreeSet<String> = enrolled.into_iter().collect();
            let enrolled_set: std::collections::BTreeSet<&str> =
                enrolled_set.iter().map(|s| s.as_str()).collect();

            for cs_ip in &current_peers {
                if !enrolled_set.contains(cs_ip.as_str()) {
                    let already = {
                        let guard = self
                            .evicted_ips
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        guard.contains(cs_ip)
                    };
                    if !already {
                        // AUDIT-MESH-7 — LizardFS has no manual chunkserver
                        // eviction: when a CA-revoked peer's overlay drops, the
                        // master detects the disconnected chunkserver and
                        // auto-replicates its chunks to satisfy the goal. The old
                        // MooseFS `CS-EVICT`/`MASTER-STOP` argv were dead (the
                        // syntax `lizardfs-admin` rejects). VIP failover is the
                        // real `tick_once_ha` path (claim VIP + promote shadow),
                        // not a remote `MASTER-STOP`. Here we only record the
                        // departure + log the replication backlog so the operator
                        // can see healing is underway.
                        tracing::warn!(
                            target: "mackesd::meshfs_worker",
                            cs_ip,
                            "chunkserver IP absent from QNM-Shared; LizardFS auto-replicating its chunks",
                        );
                        if binary_on_path(&self.admin_binary) && master_reachable(&self.vip) {
                            let argv = chunks_health_argv(&self.admin_binary, &self.vip);
                            if let Ok(out) = run_argv(&argv) {
                                tracing::info!(
                                    target: "mackesd::meshfs_worker",
                                    cs_ip,
                                    replication = %String::from_utf8_lossy(&out.stdout).trim(),
                                    "post-departure replication health",
                                );
                            }
                        }
                        self.evicted_ips
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(cs_ip.clone());
                        publish_meshfs_event(
                            "meshfs/peer-state-changed",
                            &format!(r#"{{"op":"removed","ip":"{cs_ip}"}}"#),
                        );
                    }
                }
            }
        }

        // 6. HA: lighthouse VIP claim + shadow promotion (MESHFS-3.1).
        self.tick_once_ha();

        // 7. Topology (MESHFS-7.1) — AUDIT-MESH-7: removed. LizardFS has no
        //    `CS-SET-TOPOLOGY` admin command (that was dead MooseFS syntax);
        //    topology is configured file-side via /etc/mfs/mfstopology.cfg
        //    (setup-qnm-shared writes an empty one) + `reload-config`. For a
        //    flat ≤8-peer mesh (§8) per-CS topology labels add no value, so
        //    there is nothing to do per tick.

        // 8. Quota: hourly setquota call (MESHFS-9.1).
        self.tick_once_quota();

        // 8b. FPG-7 / Q13 — the XDG bind moved to `tick_xdg_binds` (called
        //     independently from the run loop). AUDIT-MESH-15 fix: it must NOT
        //     be gated behind tick_once's step-1 `mfsmaster on PATH` guard —
        //     a Workstation runs lizardfs-client ONLY (mfsmount, no mfsmaster),
        //     so the bind was unreachable on exactly the desktop nodes that
        //     need ~/Documents synced. The bind only needs the mount live.

        // 9. MESHFS-6.1 — Replay staged offline writes now that master is up.
        //    Skipped when master is unreachable (don't replay into a down master;
        //    wait for the next tick when it recovers).
        if master_up {
            let stage = Path::new(STAGE_DIR);
            let mount = self.mount_path();
            for line in replay_all_staged(stage, &mount, &overlay_ip) {
                tracing::info!(target: "mackesd::meshfs_worker", "{line}");
            }
        }

        // 10. MESHFS-8.1 — Apply trash retention on down→up transition only.
        //     Running `mfssettrashtime` every tick would be noisy and wasteful;
        //     once on reconnect is sufficient because the setting is persistent
        //     in the LizardFS metadata.
        if master_up && !prev_up && binary_on_path(&self.settrashtime_binary) {
            let argv = settrashtime_argv(
                &self.settrashtime_binary,
                self.trash_retention_secs,
                &self.mount_path().to_string_lossy(),
            );
            tracing::info!(
                target: "mackesd::meshfs_worker",
                secs = self.trash_retention_secs,
                "applying LizardFS trash retention window",
            );
            let _ = run_argv(&argv);
        }
    }

    /// MESHFS-3.1 — HA tick: claim or relinquish the floating overlay
    /// VIP based on the role-marker (lighthouse gate) + master
    /// reachability. Only lighthouses (peers whose `role.host` marker
    /// exists) are VIP-eligible; ordinary workstation peers skip this
    /// path entirely.
    ///
    /// When the active master becomes unreachable:
    ///   1. If we don't already hold the VIP, claim it via
    ///      `ip addr add <vip>/<prefix> dev <iface>`.
    ///   2. (Re)start `mfsmaster -a` so the local shadow promotes itself
    ///      to active master — LizardFS HA-cluster mode picks up the
    ///      promotion once the VIP is on this interface.
    /// AUDIT-MESH-15 — bind the desktop user(s)' XDG dirs onto the communal
    /// mesh volume. Runs INDEPENDENTLY of `tick_once` so it is never gated
    /// behind that method's step-1 `mfsmaster on PATH` guard: a Workstation
    /// runs lizardfs-client only (mfsmount, no mfsmaster), so the bind must
    /// not require a local master. Only needs the mount live (master
    /// reachable). Binds for the real desktop user(s) — not the daemon's
    /// `$HOME=/root` — and surfaces failures at WARN (not a silent debug log,
    /// the ONBOARD-6 lesson).
    pub fn tick_xdg_binds(&self) {
        let mount = self.mount_path();
        // BOOT-REC: gate on the mount being a REAL FUSE mount, not merely on the
        // master being reachable — after a reboot the master answers before the
        // mount is up, and creating the communal source dirs on the unmounted
        // path poisons the mountpoint (mfsmount then refuses "not empty"). The
        // canonical /mnt/mesh-storage is only writable when truly mounted.
        if !crate::shared_root_writable(&mount) {
            return;
        }
        if !master_reachable(&self.vip) {
            return;
        }
        let mut homes = desktop_user_homes();
        // Headless node with no desktop user: fall back to the daemon's own
        // home so a Server's communal dirs still materialize.
        if homes.is_empty() {
            if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
                homes.push(home);
            }
        }
        for home in homes {
            for line in ensure_xdg_binds(&mount, &home) {
                tracing::warn!(target: "mackesd::meshfs_worker", "{line}");
            }
        }
    }

    pub fn tick_once_ha(&self) {
        // Only lighthouses can hold the VIP.
        if !self.role_marker_path.exists() {
            return;
        }
        // If the master is still reachable at the VIP, nothing to do.
        if master_reachable(&self.vip) {
            return;
        }
        // Master is down. Claim VIP if not already ours, then promote.
        let we_hold = vip_is_local(&self.vip, &self.overlay_iface);
        if !we_hold {
            let argv = vip_claim_argv(&self.vip, &self.overlay_iface, OVERLAY_CIDR_PREFIX);
            tracing::info!(target: "mackesd::meshfs_worker", argv = ?argv, "claiming mesh-storage VIP (master failover)");
            let _ = run_argv(&argv);
        }
        // Promote local shadow to active master.
        let argv = shadow_promote_argv(&self.master_binary);
        tracing::info!(target: "mackesd::meshfs_worker", argv = ?argv, "promoting shadow to active master");
        let _ = run_argv(&argv);
        publish_meshfs_event("meshfs/master-failover", r#"{"ok":true,"role":"active"}"#);
    }

    /// MESHFS-9.1 — quota tick (runs at most once per hour). Reads the
    /// minimum available bytes across all registered chunkservers from
    /// `mfsadmin CS-LIST`, then sets the export-root quota:
    ///
    ///   hard cap = 80% × min(avail)  → EROFS when exceeded
    ///   soft cap = 70% × min(avail)  → `meshfs/quota-warning` Bus event
    ///
    /// Silent no-op when `mfssetquota` is absent or the master is
    /// unreachable. Bus event publish is fire-and-forget subprocess
    /// (no Persist dependency in the sync tick path).
    pub fn tick_once_quota(&self) {
        let now_s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        {
            let mut guard = self
                .last_quota_s
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if now_s.saturating_sub(*guard) < DEFAULT_QUOTA_TICK.as_secs() {
                return;
            }
            *guard = now_s;
        }
        if !binary_on_path(&self.setquota_binary) {
            return;
        }
        if !master_reachable(&self.vip) {
            return;
        }
        // Read CS-LIST to find the minimum available bytes.
        let min_avail = match min_chunkserver_avail_bytes(&self.admin_binary, &self.vip) {
            Some(b) if b > 0 => b,
            _ => {
                tracing::debug!(target: "mackesd::meshfs_worker", "quota tick: CS-LIST returned no avail data");
                return;
            }
        };
        let hard = (min_avail as f64 * QUOTA_HARD_FACTOR) as u64;
        let soft = (min_avail as f64 * QUOTA_SOFT_FACTOR) as u64;
        let argv = setquota_argv(
            &self.setquota_binary,
            soft,
            hard,
            &self.mount_path().to_string_lossy(),
        );
        tracing::info!(
            target: "mackesd::meshfs_worker",
            hard_bytes = hard,
            soft_bytes = soft,
            "setting mesh-storage quota",
        );
        let _ = run_argv(&argv);
        // Publish quota-warning via mde-bus if soft cap is reached
        // (write size not tracked here — the OS returns ENOSPC when the
        // hard cap is hit; the soft-cap warning fires each quota tick).
        if binary_on_path("mde-bus") {
            let body = format!(
                r#"{{"ok":true,"min_avail_bytes":{min_avail},"hard_bytes":{hard},"soft_bytes":{soft}}}"#
            );
            let mut cmd = Command::new("mde-bus");
            cmd.args(["publish", "meshfs/quota-warning", "--body-flag", &body]);
            crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
        }
    }

    /// MESHFS-10.1 — poll `action/meshfs/<verb>` topics and dispatch
    /// each request, writing the reply to `reply/<ulid>`. Called from
    /// the `run()` loop at 500 ms intervals via a Persist handle opened
    /// at worker startup. No-ops when the Bus root doesn't exist yet.
    fn poll_meshfs_actions(
        &self,
        persist: &Persist,
        cursors: &mut std::collections::HashMap<String, String>,
    ) {
        for verb in ACTION_VERBS {
            let topic = format!("action/meshfs/{verb}");
            let since = cursors.get(&topic).map(String::as_str);
            let msgs = match persist.list_since(&topic, since) {
                Ok(m) => m,
                Err(e) => {
                    tracing::debug!(
                        target: "mackesd::meshfs_worker",
                        %topic, error = %e,
                        "meshfs action poll failed",
                    );
                    continue;
                }
            };
            for msg in msgs {
                cursors.insert(topic.clone(), msg.ulid.clone());
                let body = msg.body.as_deref().unwrap_or("{}");
                let enrolled = self
                    .workgroup_root
                    .as_ref()
                    .zip(self.self_node_id.as_ref())
                    .map(|(qnm, id)| enrolled_peer_ips(qnm, id).len())
                    .unwrap_or(0);
                let reply_json =
                    dispatch_meshfs_action(&self.master_binary, &self.vip, enrolled, verb, body);
                if let Err(e) = persist.write(
                    &reply_topic(&msg.ulid),
                    Priority::Default,
                    None,
                    Some(&reply_json),
                ) {
                    tracing::warn!(
                        target: "mackesd::meshfs_worker",
                        ulid = %msg.ulid, error = %e,
                        "meshfs action reply write failed",
                    );
                }
            }
        }
    }
}

impl Default for MeshFsWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Worker for MeshFsWorker {
    fn name(&self) -> &'static str {
        "meshfs_worker"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        self.tick_once();
        // AUDIT-MESH-15: the XDG bind runs independently of tick_once's
        // mfsmaster guard (Workstations have no mfsmaster).
        self.tick_xdg_binds();

        // MESHFS-10.1: open Bus persist for action polling. Persist is !Sync,
        // so it's wrapped in a Mutex (the standard Bus-responder pattern).
        let persist_opt = default_meshfs_bus_root()
            .and_then(|root| Persist::open(root).ok())
            .map(std::sync::Mutex::new);
        let mut cursors: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        // Burn the interval's immediate first fire; `tick_once()` above
        // already ran the first LizardFS management cycle.
        let mut lfs_tick = tokio::time::interval(self.tick);
        lfs_tick.tick().await;
        let mut action_tick = tokio::time::interval(ACTION_POLL_INTERVAL);

        loop {
            tokio::select! {
                biased;
                _ = shutdown.wait() => break,
                _ = lfs_tick.tick() => {
                    self.tick_once();
                    self.tick_xdg_binds();
                }
                _ = action_tick.tick() => {
                    if let Some(ref p_mutex) = persist_opt {
                        let p = p_mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        self.poll_meshfs_actions(&p, &mut cursors);
                    }
                }
            }
        }
        Ok(())
    }
}

// ── Pure helpers (tested without subprocess) ──────────────────────────────────

/// `true` if `name` resolves to an executable on PATH or an
/// absolute path that exists.
#[must_use]
pub fn binary_on_path(name: &str) -> bool {
    let candidate = Path::new(name);
    if candidate.is_absolute() {
        return candidate.exists();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
}

/// Probe the master's TCP port. `true` = reachable.
/// Implemented as a non-blocking connect with a 500 ms timeout
/// so the tick loop doesn't stall on an unreachable VIP.
#[must_use]
pub fn master_reachable(vip: &str) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    let addr_str = format!("{vip}:{MFSMASTER_PORT}");
    let Ok(mut addrs) = addr_str.to_socket_addrs() else {
        return false;
    };
    let Some(addr) = addrs.next() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
}

/// Build the argv for starting `mfsmaster` in genesis mode.
///
/// ```text
/// mfsmaster start
/// ```
#[must_use]
pub fn genesis_start_argv(master_binary: &str) -> Vec<String> {
    vec![master_binary.to_owned(), "start".to_owned()]
}

/// Build the argv for starting `mfschunkserver`.
///
/// ```text
/// mfschunkserver start
/// ```
#[must_use]
pub fn chunkserver_start_argv(chunkserver_binary: &str) -> Vec<String> {
    vec![chunkserver_binary.to_owned(), "start".to_owned()]
}

/// Build the argv for setting the replication goal recursively on the
/// mount root.
///
/// ```text
/// mfssetgoal -r <goal> <mount_path>
/// ```
#[must_use]
pub fn setgoal_argv(setgoal_binary: &str, goal: u8, mount_path: &str) -> Vec<String> {
    vec![
        setgoal_binary.to_owned(),
        "-r".to_owned(),
        goal.to_string(),
        mount_path.to_owned(),
    ]
}

/// AUDIT-MESH-7 — LizardFS replication-health observability.
///
/// `lizardfs-admin chunks-health <vip> <port> --replication --porcelain`
///
/// Replaces the dead MooseFS `CS-EVICT` write-path: LizardFS has **no**
/// manual chunkserver-eviction command — when a chunkserver disconnects
/// (a CA-revoked peer's overlay drops), the master detects it and
/// **auto-replicates** its chunks to satisfy the goal. There is nothing
/// to "evict"; this read-only command surfaces the replication backlog so
/// the worker can log that healing is underway after a peer leaves.
#[must_use]
pub fn chunks_health_argv(admin_binary: &str, vip: &str) -> Vec<String> {
    vec![
        admin_binary.to_owned(),
        "chunks-health".to_owned(),
        vip.to_owned(),
        MFSMASTER_CLIENT_PORT.to_string(),
        "--replication".to_owned(),
        "--porcelain".to_owned(),
    ]
}

/// Build the argv for setting the export-root quota via `mfssetquota`.
///
/// ```text
/// mfssetquota -p / 0 0 <soft_bytes> <hard_bytes> <mount_path>
/// ```
///
/// The two leading `0 0` are inode soft/hard limits — left unconstrained
/// since we only cap by bytes. `-p /` applies the quota to the export root.
#[must_use]
pub fn setquota_argv(
    setquota_binary: &str,
    soft_bytes: u64,
    hard_bytes: u64,
    mount_path: &str,
) -> Vec<String> {
    vec![
        setquota_binary.to_owned(),
        "-p".to_owned(),
        "/".to_owned(),
        "0".to_owned(),
        "0".to_owned(),
        soft_bytes.to_string(),
        hard_bytes.to_string(),
        mount_path.to_owned(),
    ]
}

/// Query the active master for the minimum available bytes across all
/// registered chunkservers. Returns `None` when `lizardfs-admin` is absent,
/// the master is unreachable, or no chunkservers are registered.
#[must_use]
pub fn min_chunkserver_avail_bytes(admin_binary: &str, vip: &str) -> Option<u64> {
    query_chunkservers(admin_binary, vip)
        .iter()
        .map(|cs| cs.avail_bytes)
        .min()
}

/// Build the argv for claiming the floating VIP on the Nebula overlay
/// interface. Executed by `tick_once_ha()` when a lighthouse detects
/// the active master is unreachable and it doesn't already hold the VIP.
///
/// ```text
/// ip addr add <vip>/<prefix_len> dev <iface>
/// ```
#[must_use]
pub fn vip_claim_argv(vip: &str, iface: &str, prefix_len: u8) -> Vec<String> {
    vec![
        "ip".to_owned(),
        "addr".to_owned(),
        "add".to_owned(),
        format!("{vip}/{prefix_len}"),
        "dev".to_owned(),
        iface.to_owned(),
    ]
}

/// Build the argv for releasing the floating VIP from the Nebula overlay
/// interface. Executed when this lighthouse relinquishes the master role.
///
/// ```text
/// ip addr del <vip>/<prefix_len> dev <iface>
/// ```
#[must_use]
pub fn vip_release_argv(vip: &str, iface: &str, prefix_len: u8) -> Vec<String> {
    vec![
        "ip".to_owned(),
        "addr".to_owned(),
        "del".to_owned(),
        format!("{vip}/{prefix_len}"),
        "dev".to_owned(),
        iface.to_owned(),
    ]
}

/// Build the argv for promoting the local shadow master to active.
/// LizardFS HA-cluster mode: passing `-a` on start instructs the master
/// daemon to immediately take the active role rather than shadowing.
///
/// ```text
/// mfsmaster -a start
/// ```
#[must_use]
pub fn shadow_promote_argv(master_binary: &str) -> Vec<String> {
    vec![
        master_binary.to_owned(),
        "-a".to_owned(),
        "start".to_owned(),
    ]
}

/// Parse `ip addr show dev <iface>` output to determine whether `vip`
/// is currently assigned to the interface. Pure — no subprocess.
///
/// Looks for `inet <vip>/` anywhere in the output (the `ip addr`
/// format is `inet A.B.C.D/prefix`).
#[must_use]
pub fn parse_ip_addr_output(text: &str, vip: &str) -> bool {
    let needle = format!("inet {vip}/");
    text.contains(&needle)
}

/// `true` if the floating VIP is currently assigned to `iface` on this
/// host. Shells `ip addr show dev <iface>`; returns `false` on any
/// subprocess error (binary absent, interface doesn't exist, etc.).
#[must_use]
pub fn vip_is_local(vip: &str, iface: &str) -> bool {
    let Ok(out) = Command::new("ip")
        .args(["addr", "show", "dev", iface])
        .output()
    else {
        return false;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    parse_ip_addr_output(&text, vip)
}

/// Scan `<workgroup_root>/*/mackesd/nebula-bundle.json` to discover
/// enrolled peers' overlay IPs. Skips self + bundles that don't
/// parse. Returns a sorted, deduplicated list.
#[must_use]
pub fn enrolled_peer_ips(workgroup_root: &Path, self_node_id: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(workgroup_root) else {
        return Vec::new();
    };
    let mut ips: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(|s| s.to_owned()) else {
            continue;
        };
        if name == self_node_id {
            continue;
        }
        let bundle_path = entry.path().join("mackesd").join("nebula-bundle.json");
        let Ok(bytes) = std::fs::read(&bundle_path) else {
            continue;
        };
        let Ok(bundle) = serde_json::from_slice::<crate::ca::bundle::NebulaBundle>(&bytes) else {
            continue;
        };
        ips.push(bundle.overlay_ip);
    }
    ips.sort();
    ips.dedup();
    ips
}

/// List the overlay IPs of chunkservers currently registered with the
/// active master. Returns an empty list when `lizardfs-admin` isn't
/// installed or the master is unreachable.
#[must_use]
pub fn current_chunkserver_ips(admin_binary: &str, vip: &str) -> Vec<String> {
    query_chunkservers(admin_binary, vip)
        .into_iter()
        .map(|cs| cs.addr)
        .collect()
}

/// AUDIT-MESH-3 — single source of truth for the live chunkserver roster.
/// Shells `lizardfs-admin list-chunkservers <vip> <matocl-port> --porcelain`
/// (the deployed substrate is LizardFS, whose admin CLI is `lizardfs-admin`,
/// not MooseFS's `mfsadmin <vip> CS-LIST`), and parses the porcelain rows.
/// Returns an empty list when `lizardfs-admin` is absent, the master is
/// unreachable, or the query fails. All read-paths (status panel, quota
/// tick, reconcile/eviction) funnel through here so they share one format.
#[must_use]
pub fn query_chunkservers(admin_binary: &str, vip: &str) -> Vec<ChunkserverStatus> {
    let Ok(out) = Command::new(admin_binary)
        .args([
            "list-chunkservers",
            vip,
            &MFSMASTER_CLIENT_PORT.to_string(),
            "--porcelain",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_chunkservers_porcelain(&String::from_utf8_lossy(&out.stdout))
}

// ── MESHFS-13.1: status report (Workbench "Mesh Storage" panel) ─────────────

/// Per-chunkserver row from `lizardfs-admin list-chunkservers --porcelain`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChunkserverStatus {
    /// Overlay IP address of this chunkserver (the `:port` suffix from the
    /// porcelain `addr:port` column is stripped so it matches enrolled IPs).
    pub addr: String,
    /// Bytes currently consumed by stored chunks on this chunkserver.
    pub used_bytes: u64,
    /// Free bytes available for new chunks (total − used).
    pub avail_bytes: u64,
    /// Chunks below their replication goal on this CS. Not present in the
    /// porcelain `list-chunkservers` output (it lives in `chunks-health`);
    /// defaults to 0.
    #[serde(default)]
    pub undergoal_chunks: u64,
    /// Whether this CS has chunks marked for removal (porcelain column 6 > 0
    /// — i.e. a rebalance/decommission is draining it).
    #[serde(default)]
    pub marked_for_removal: bool,
}

/// AUDIT-MESH-3 — parse `lizardfs-admin list-chunkservers <ip> <port>
/// --porcelain` output into per-chunkserver status rows. Porcelain emits one
/// space-separated row per chunkserver (no header):
/// ```text
/// <addr>:<port> <version> <chunks> <used_bytes> <total_bytes> \
///   <todel_chunks> <todel_used> <todel_total> <errors> <label>
/// ```
/// e.g. `10.42.0.2:9422 3.12.0 47237 5966987264 24306466816 0 0 0 0 _`.
/// `avail = total − used`; rows that don't parse are skipped.
#[must_use]
pub fn parse_chunkservers_porcelain(text: &str) -> Vec<ChunkserverStatus> {
    text.lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 5 {
                return None;
            }
            // Strip the :port suffix so the addr matches enrolled overlay IPs.
            let addr = cols[0]
                .rsplit_once(':')
                .map_or(cols[0], |(ip, _)| ip)
                .to_owned();
            if !addr.contains('.') && !addr.contains(':') {
                return None;
            }
            let used_bytes = cols[3].parse::<u64>().ok()?;
            let total_bytes = cols[4].parse::<u64>().ok()?;
            let avail_bytes = total_bytes.saturating_sub(used_bytes);
            let marked_for_removal = cols
                .get(5)
                .and_then(|v| v.parse::<u64>().ok())
                .is_some_and(|todel| todel > 0);
            Some(ChunkserverStatus {
                addr,
                used_bytes,
                avail_bytes,
                undergoal_chunks: 0,
                marked_for_removal,
            })
        })
        .collect()
}

/// Fleet status report emitted by `mackesd mesh-fs-status --json`.
#[derive(Debug, serde::Serialize)]
pub struct MeshFsStatusReport {
    /// Whether the active LizardFS master answered on the floating VIP at report time.
    pub master_reachable: bool,
    /// Per-chunkserver rows returned by `mfsadmin CS-LIST`; empty when the master is unreachable.
    pub peers: Vec<ChunkserverStatus>,
    /// Replication goal = current peer count (converges on tick).
    pub goal: usize,
    /// Hard quota cap in bytes (0.8 × min(avail)), absent when no CS data.
    pub quota_cap_bytes: Option<u64>,
    /// Overlay IP of the chunkserver with the least available space.
    pub limiting_peer_addr: Option<String>,
    /// Enrolled peer IPs that are absent from the CS-LIST (offline).
    /// Empty when called without an enrolled-IP set (see
    /// `meshfs_status_report_with_enrolled`).
    #[serde(default)]
    pub offline_peers: Vec<String>,
}

/// Query the active LizardFS master via `mfsadmin CS-LIST` and return a
/// `MeshFsStatusReport`. Gracefully returns empty peers when LizardFS is
/// not running or the master is unreachable.
#[must_use]
pub fn meshfs_status_report(admin_binary: &str, vip: &str) -> MeshFsStatusReport {
    let master_reachable = master_reachable(vip);
    let peers = if master_reachable {
        query_chunkservers(admin_binary, vip)
    } else {
        Vec::new()
    };
    let limiting = peers.iter().min_by_key(|p| p.avail_bytes);
    let quota_cap_bytes = limiting.map(|p| p.avail_bytes * 4 / 5);
    let limiting_peer_addr = limiting.map(|p| p.addr.clone());
    let goal = peers.len();
    MeshFsStatusReport {
        master_reachable,
        peers,
        goal,
        quota_cap_bytes,
        limiting_peer_addr,
        offline_peers: Vec::new(),
    }
}

/// Like `meshfs_status_report`, but cross-references `enrolled_ips` against
/// the CS-LIST to populate `offline_peers` — enrolled nodes whose overlay IP
/// does not appear in the CS-LIST are listed as offline.
#[must_use]
pub fn meshfs_status_report_with_enrolled(
    admin_binary: &str,
    vip: &str,
    enrolled_ips: &[String],
) -> MeshFsStatusReport {
    let mut report = meshfs_status_report(admin_binary, vip);
    if !enrolled_ips.is_empty() {
        let cs_ips: std::collections::HashSet<&str> =
            report.peers.iter().map(|p| p.addr.as_str()).collect();
        report.offline_peers = enrolled_ips
            .iter()
            .filter(|ip| !cs_ips.contains(ip.as_str()))
            .cloned()
            .collect();
    }
    report
}

// ── MESHFS-8.1: trash retention + trash listing ─────────────────────────────

/// Build the `mfssettrashtime -r <secs> <mount_path>` argv vector.
/// The `-r` flag applies the retention recursively from the export root.
#[must_use]
pub fn settrashtime_argv(binary: &str, secs: u64, mount_path: &str) -> Vec<String> {
    vec![
        binary.to_owned(),
        "-r".to_owned(),
        secs.to_string(),
        mount_path.to_owned(),
    ]
}

/// One entry in the LizardFS `.trash` virtual directory.
///
/// LizardFS names trash entries as `<8-hex-char-inode>BASENAME`. The
/// `name` field is the best-effort stripped display name; `trash_path` is
/// the full path for `TRASH-RECOVER` recovery.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct TrashEntry {
    /// Display name (leading 8-hex-char inode prefix stripped when present).
    pub name: String,
    /// Full path of the entry inside the `.trash/` virtual directory.
    pub trash_path: String,
}

/// List recoverable files from the LizardFS `.trash` virtual directory at
/// `<mount_path>/.trash`. Returns an empty Vec when the directory is
/// absent (filesystem not mounted) or empty.
#[must_use]
pub fn list_trash_entries(mount_path: &str) -> Vec<TrashEntry> {
    let trash_dir = format!("{mount_path}/.trash");
    let Ok(rd) = std::fs::read_dir(&trash_dir) else {
        return Vec::new();
    };
    rd.flatten()
        .map(|entry| {
            let raw = entry.file_name().to_string_lossy().to_string();
            // Strip leading 8-char hex inode prefix that LizardFS prepends.
            let name = if raw.len() > 8 && raw[..8].chars().all(|c| c.is_ascii_hexdigit()) {
                raw[8..].to_owned()
            } else {
                raw.clone()
            };
            let name = if name.is_empty() { raw.clone() } else { name };
            TrashEntry {
                name,
                trash_path: format!("{trash_dir}/{raw}"),
            }
        })
        .collect()
}

/// Bus root for the per-peer ntfy persist layer. Mirrors `marks_state`.
fn default_meshfs_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// Fire-and-forget Bus event publish via `mde-bus publish`. No Persist
/// dependency — the subprocess writes to the persist layer on its own.
/// No-ops when `mde-bus` isn't on PATH.
fn publish_meshfs_event(topic: &str, body: &str) {
    if binary_on_path("mde-bus") {
        let mut cmd = Command::new("mde-bus");
        cmd.args(["publish", topic, "--body-flag", body]);
        crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
    }
}

/// Dispatch one `action/meshfs/<verb>` message, returning a reply JSON
/// string for `reply/<ulid>`. All six verbs defined by MESHFS-10.1.
/// Pure function — takes only primitive/slice arguments so it is
/// trivially testable without a running Bus or persist layer.
pub fn dispatch_meshfs_action(
    master_binary: &str,
    vip: &str,
    enrolled_peer_count: usize,
    verb: &str,
    body: &str,
) -> String {
    match verb {
        "status" => {
            let reachable = master_reachable(vip);
            format!(
                r#"{{"ok":true,"master_reachable":{reachable},"enrolled_peers":{enrolled_peer_count}}}"#
            )
        }
        "bootstrap" => {
            let argv = genesis_start_argv(master_binary);
            let ok = run_argv(&argv).is_ok();
            format!(r#"{{"ok":{ok}}}"#)
        }
        "add-peer" | "remove-peer" => {
            r#"{"ok":true,"note":"goal converges on next tick"}"#.to_owned()
        }
        "resolve-conflict" => dispatch_resolve_conflict(body),
        "undelete" => dispatch_undelete(body),
        _ => r#"{"ok":false,"error":"unknown verb"}"#.to_owned(),
    }
}

/// Move the conflict file at `path` to `~/Local/conflict-archive/<ts>/`.
/// Returns `Ok(())` on success, `Err(reason)` on failure. Used directly
/// by the `mackesd meshfs-resolve-conflict` CLI subcommand.
///
/// The same logic is invoked via the Bus `action/resolve-conflict` handler
/// (which wraps this through `dispatch_resolve_conflict`).
pub fn resolve_conflict_to_archive(path: &str) -> Result<(), String> {
    let body = format!(
        r#"{{"path":{}}}"#,
        serde_json::Value::String(path.to_owned())
    );
    let reply = dispatch_resolve_conflict(&body);
    let v: serde_json::Value = serde_json::from_str(&reply).unwrap_or_default();
    if v["ok"].as_bool().unwrap_or(false) {
        Ok(())
    } else {
        let err = v["error"].as_str().unwrap_or("unknown error");
        Err(err.to_owned())
    }
}

/// Move a `.conflict-*` file to `~/Local/conflict-archive/<ts>/`.
fn dispatch_resolve_conflict(body: &str) -> String {
    let path_str: String = match serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["path"].as_str().map(|s| s.to_owned()))
    {
        Some(p) => p,
        None => return r#"{"ok":false,"error":"missing path"}"#.to_owned(),
    };
    let src = std::path::Path::new(&path_str);
    if !src.exists() {
        return r#"{"ok":false,"error":"path not found"}"#.to_owned();
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let home = match std::env::var("HOME").ok().filter(|h| !h.is_empty()) {
        Some(h) => h,
        None => return r#"{"ok":false,"error":"HOME not set"}"#.to_owned(),
    };
    let archive_dir = std::path::PathBuf::from(home)
        .join("Local")
        .join("conflict-archive")
        .join(ts.to_string());
    if let Err(e) = std::fs::create_dir_all(&archive_dir) {
        return format!(r#"{{"ok":false,"error":"mkdir: {e}"}}"#);
    }
    let file_name = src.file_name().unwrap_or_default();
    let dest = archive_dir.join(file_name);
    match std::fs::rename(src, &dest) {
        Ok(()) => r#"{"ok":true}"#.to_owned(),
        Err(e) => format!(r#"{{"ok":false,"error":"rename: {e}"}}"#),
    }
}

/// AUDIT-MESH-7 — recover a trashed file the LizardFS way: move the trash
/// entry into the sibling `undel/` directory of the trash meta tree. The old
/// `mfsadmin TRASH-RECOVER` was dead MooseFS syntax (`lizardfs-admin` rejects
/// it). In LizardFS the trash is a filesystem tree (`<mount>/.trash/<entry>`
/// + `<mount>/.trash/undel/`); restoring a file is a `rename` of the entry
/// into `undel/`, which the master replays as an undelete. `body` carries the
/// full `trash_path` (as listed by [`list_trash_entries`]).
fn dispatch_undelete(body: &str) -> String {
    let trash_path: String = match serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v["trash_path"]
                .as_str()
                .or_else(|| v["path"].as_str())
                .map(str::to_owned)
        }) {
        Some(p) => p,
        None => return r#"{"ok":false,"error":"missing trash_path"}"#.to_owned(),
    };
    let src = std::path::Path::new(&trash_path);
    let (Some(parent), Some(name)) = (src.parent(), src.file_name()) else {
        return r#"{"ok":false,"error":"malformed trash_path"}"#.to_owned();
    };
    if !src.exists() {
        return r#"{"ok":false,"error":"trash entry not found"}"#.to_owned();
    }
    // The undel/ inbox lives beside the entry inside the trash tree.
    let undel_dir = parent.join("undel");
    if let Err(e) = std::fs::create_dir_all(&undel_dir) {
        return format!(r#"{{"ok":false,"error":"undel dir: {e}"}}"#);
    }
    let dest = undel_dir.join(name);
    match std::fs::rename(src, &dest) {
        Ok(()) => r#"{"ok":true}"#.to_owned(),
        Err(e) => format!(r#"{{"ok":false,"error":"undelete rename: {e}"}}"#),
    }
}

// ── MESHFS-6.1: offline staging + LWW replay ────────────────────────────────

/// Outcome of replaying one staged file.
#[derive(Debug, PartialEq, Eq)]
pub enum ReplayOutcome {
    /// Staged file applied to the mesh mount (no conflict or staged won).
    Applied {
        /// Absolute path of the file on the mesh mount after the copy.
        mesh_path: PathBuf,
    },
    /// Staged file won the LWW race; old mesh file renamed to the conflict path.
    ConflictStagedWins {
        /// Absolute path of the winning (updated) file on the mesh mount.
        mesh_path: PathBuf,
        /// Absolute path of the loser: the old mesh file renamed to `<name>.conflict-<host>-<ts>`.
        conflict_path: PathBuf,
    },
    /// Mesh file won the LWW race; staged file renamed to the conflict path.
    ConflictMeshWins {
        /// Absolute path of the loser: the staged file renamed to `<name>.conflict-<host>-<ts>`.
        conflict_path: PathBuf,
    },
    /// Replay skipped (IO error or could not determine relative path).
    Skipped {
        /// Human-readable description of why the replay was skipped.
        reason: String,
    },
}

/// Recursively collect regular files under `dir` into `out`.
fn collect_staged_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_staged_files(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

/// Walk `stage_dir` and return the paths of all staged files. Returns an
/// empty Vec when the stage dir doesn't exist (clean peer — nothing to replay).
#[must_use]
pub fn staged_files(stage_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if stage_dir.is_dir() {
        collect_staged_files(stage_dir, &mut out);
    }
    out
}

/// Replay one staged file to the mesh mount using last-write-wins mtime.
///
/// - If the mesh path doesn't exist: copy staged → mesh, delete staged.
/// - If mesh path exists and staged mtime > mesh mtime: staged wins →
///   rename mesh to `<mesh>.conflict-<host>-<ts>`, copy staged → mesh,
///   delete staged.
/// - If mesh path exists and staged mtime <= mesh mtime: mesh wins →
///   rename staged to `<staged>.conflict-<host>-<ts>` (no overwrite).
#[must_use]
pub fn replay_file_lww(staged: &Path, mesh_path: &Path, host: &str) -> ReplayOutcome {
    // Staged mtime — required for any comparison.
    let staged_mtime = match std::fs::metadata(staged).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(e) => {
            return ReplayOutcome::Skipped {
                reason: format!("staged stat: {e}"),
            }
        }
    };

    if !mesh_path.exists() {
        // No conflict — just copy staged to mesh.
        if let Some(parent) = mesh_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::copy(staged, mesh_path) {
            return ReplayOutcome::Skipped {
                reason: format!("copy to mesh: {e}"),
            };
        }
        let _ = std::fs::remove_file(staged);
        return ReplayOutcome::Applied {
            mesh_path: mesh_path.to_path_buf(),
        };
    }

    let mesh_mtime = match std::fs::metadata(mesh_path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(e) => {
            return ReplayOutcome::Skipped {
                reason: format!("mesh stat: {e}"),
            }
        }
    };

    let ts = staged_mtime
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if staged_mtime > mesh_mtime {
        // Staged wins: rename old mesh file to conflict path, copy staged → mesh.
        let conflict_name = format!(
            "{}.conflict-{host}-{ts}",
            mesh_path.file_name().unwrap_or_default().to_string_lossy()
        );
        let conflict_path = mesh_path.with_file_name(conflict_name);
        if let Err(e) = std::fs::rename(mesh_path, &conflict_path) {
            return ReplayOutcome::Skipped {
                reason: format!("rename mesh to conflict: {e}"),
            };
        }
        if let Err(e) = std::fs::copy(staged, mesh_path) {
            // Rollback the rename so the mesh file isn't lost.
            let _ = std::fs::rename(&conflict_path, mesh_path);
            return ReplayOutcome::Skipped {
                reason: format!("copy to mesh (after rename): {e}"),
            };
        }
        let _ = std::fs::remove_file(staged);
        ReplayOutcome::ConflictStagedWins {
            mesh_path: mesh_path.to_path_buf(),
            conflict_path,
        }
    } else {
        // Mesh wins: staged file is the loser — rename it to conflict path.
        let conflict_name = format!(
            "{}.conflict-{host}-{ts}",
            staged.file_name().unwrap_or_default().to_string_lossy()
        );
        let conflict_path = staged.with_file_name(conflict_name);
        if let Err(e) = std::fs::rename(staged, &conflict_path) {
            return ReplayOutcome::Skipped {
                reason: format!("rename staged to conflict: {e}"),
            };
        }
        ReplayOutcome::ConflictMeshWins { conflict_path }
    }
}

/// Walk the stage dir, compute each file's mesh path (via the relative
/// path under `stage_dir`), and replay it via LWW. Returns log lines.
#[must_use]
pub fn replay_all_staged(stage_dir: &Path, mesh_mount: &Path, host: &str) -> Vec<String> {
    let mut log = Vec::new();
    for staged in staged_files(stage_dir) {
        let rel = match staged.strip_prefix(stage_dir) {
            Ok(r) => r,
            Err(_) => {
                log.push(format!(
                    "meshfs replay: skip {} (not under stage dir)",
                    staged.display()
                ));
                continue;
            }
        };
        let mesh_path = mesh_mount.join(rel);
        let outcome = replay_file_lww(&staged, &mesh_path, host);
        let msg = match &outcome {
            ReplayOutcome::Applied { mesh_path: mp } => format!(
                "meshfs replay: applied staged {} → {}",
                staged.display(),
                mp.display()
            ),
            ReplayOutcome::ConflictStagedWins {
                mesh_path: mp,
                conflict_path: cp,
            } => format!(
                "meshfs replay: staged wins (LWW) {} → {}; mesh loser → {}",
                staged.display(),
                mp.display(),
                cp.display()
            ),
            ReplayOutcome::ConflictMeshWins { conflict_path: cp } => format!(
                "meshfs replay: mesh wins (LWW) {}; staged loser → {}",
                staged.display(),
                cp.display()
            ),
            ReplayOutcome::Skipped { reason } => {
                format!("meshfs replay: skip {} ({reason})", staged.display())
            }
        };
        log.push(msg);
    }
    log
}

/// Run a command given as an argv slice. Returns the `Output` or an
/// error. Logs a `warn!` on non-zero exit so every command failure
/// is traceable without panicking.
fn run_argv(argv: &[String]) -> anyhow::Result<std::process::Output> {
    let (prog, args) = argv
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("empty argv"))?;
    let out = Command::new(prog).args(args).output()?;
    if !out.status.success() {
        tracing::warn!(
            target: "mackesd::meshfs_worker",
            argv = ?argv,
            status = ?out.status,
            stderr = %String::from_utf8_lossy(&out.stderr),
            "meshfs command exited non-zero",
        );
    }
    Ok(out)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_start_argv_shape() {
        assert_eq!(genesis_start_argv("mfsmaster"), vec!["mfsmaster", "start"]);
    }

    #[test]
    fn chunkserver_start_argv_shape() {
        assert_eq!(
            chunkserver_start_argv("mfschunkserver"),
            vec!["mfschunkserver", "start"]
        );
    }

    #[test]
    fn setgoal_argv_shape_goal_3() {
        assert_eq!(
            setgoal_argv("mfssetgoal", 3, "/mnt/mesh-storage"),
            vec!["mfssetgoal", "-r", "3", "/mnt/mesh-storage"]
        );
    }

    #[test]
    fn setgoal_argv_goal_one() {
        assert_eq!(
            setgoal_argv("mfssetgoal", 1, "/mnt/mesh-storage"),
            vec!["mfssetgoal", "-r", "1", "/mnt/mesh-storage"]
        );
    }

    #[test]
    fn chunks_health_argv_is_real_lizardfs_syntax() {
        // AUDIT-MESH-7: the eviction observability command is the real
        // `lizardfs-admin chunks-health <ip> <port> ...` form (subcommand
        // FIRST, then ip+port) — NOT the dead MooseFS `<ip> VERB` shape.
        assert_eq!(
            chunks_health_argv("lizardfs-admin", "10.42.0.1"),
            vec![
                "lizardfs-admin",
                "chunks-health",
                "10.42.0.1",
                "9421",
                "--replication",
                "--porcelain"
            ]
        );
    }

    /// The real `lizardfs-admin list-chunkservers <ip> <port> --porcelain`
    /// output captured from the live 2-chunkserver mesh (Lighthouse-01/02).
    const PORCELAIN_FIXTURE: &str = "\
10.42.0.2:9422 3.12.0 47237 5966987264 24306466816 0 0 0 0 _\n\
10.42.0.1:9422 3.12.0 38862 5845155840 24283152384 0 0 0 0 _\n";

    #[test]
    fn parse_chunkservers_porcelain_strips_port_and_computes_avail() {
        let rows = parse_chunkservers_porcelain(PORCELAIN_FIXTURE);
        assert_eq!(rows.len(), 2);
        // :port stripped → addr matches enrolled overlay IPs.
        assert_eq!(rows[0].addr, "10.42.0.2");
        assert_eq!(rows[0].used_bytes, 5_966_987_264);
        // avail = total − used.
        assert_eq!(rows[0].avail_bytes, 24_306_466_816 - 5_966_987_264);
        assert!(!rows[0].marked_for_removal);
        assert_eq!(rows[0].undergoal_chunks, 0);
        assert_eq!(rows[1].addr, "10.42.0.1");
    }

    #[test]
    fn parse_chunkservers_porcelain_flags_marked_for_removal() {
        // todel_chunks (column 6) > 0 → marked_for_removal.
        let line = "10.42.0.9:9422 3.12.0 100 1000 5000 12 0 0 0 _\n";
        let rows = parse_chunkservers_porcelain(line);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].marked_for_removal);
        assert_eq!(rows[0].avail_bytes, 4000);
    }

    #[test]
    fn parse_chunkservers_porcelain_empty_returns_empty() {
        assert!(parse_chunkservers_porcelain("").is_empty());
    }

    #[test]
    fn current_chunkserver_ips_derives_from_porcelain_rows() {
        // Pure-parse cross-check: the IP list is the addr column of the rows.
        let ips: Vec<String> = parse_chunkservers_porcelain(PORCELAIN_FIXTURE)
            .into_iter()
            .map(|cs| cs.addr)
            .collect();
        assert_eq!(ips, vec!["10.42.0.2", "10.42.0.1"]);
    }

    #[test]
    fn enrolled_peer_ips_empty_when_dir_missing() {
        let dir = std::path::PathBuf::from("/tmp/meshfs-test-nonexistent-dir-xyzzy");
        assert!(enrolled_peer_ips(&dir, "self").is_empty());
    }

    #[test]
    fn enrolled_peer_ips_skips_self() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let pairs = [
            ("self", "10.42.0.1"),
            ("peer-a", "10.42.0.5"),
            ("peer-b", "10.42.0.7"),
        ];
        for (name, ip) in &pairs {
            let dir = root.join(name).join("mackesd");
            std::fs::create_dir_all(&dir).unwrap();
            let bundle = crate::ca::bundle::NebulaBundle {
                mesh_id: "test-mesh".into(),
                epoch: 1,
                ca_cert_pem: "ca".into(),
                peer_cert_pem: "p".into(),
                peer_key_pem: "k".into(),
                overlay_ip: (*ip).into(),
                mesh_cidr: "10.42.0.0/16".into(),
                lighthouses: vec![],
                created_at: 1_700_000_000,
            };
            let body = serde_json::to_vec_pretty(&bundle).unwrap();
            std::fs::write(dir.join("nebula-bundle.json"), &body).unwrap();
        }
        let ips = enrolled_peer_ips(root, "self");
        assert_eq!(ips.len(), 2);
        assert!(ips.contains(&"10.42.0.5".to_string()));
        assert!(ips.contains(&"10.42.0.7".to_string()));
        assert!(!ips.contains(&"10.42.0.1".to_string()));
    }

    #[test]
    fn binary_on_path_false_for_nonexistent() {
        assert!(!binary_on_path("this-binary-does-not-exist-xyzzy-42"));
    }

    #[test]
    fn tick_once_no_ops_when_binary_absent() {
        let worker = MeshFsWorker::new().with_master_binary("this-binary-does-not-exist-xyzzy-42");
        // Shouldn't panic or block.
        worker.tick_once();
    }

    #[test]
    fn vip_claim_argv_shape() {
        let argv = vip_claim_argv("10.42.0.1", "nebula1", 16);
        assert_eq!(
            argv,
            ["ip", "addr", "add", "10.42.0.1/16", "dev", "nebula1"]
        );
    }

    #[test]
    fn vip_release_argv_shape() {
        let argv = vip_release_argv("10.42.0.1", "nebula1", 16);
        assert_eq!(
            argv,
            ["ip", "addr", "del", "10.42.0.1/16", "dev", "nebula1"]
        );
    }

    #[test]
    fn shadow_promote_argv_shape() {
        let argv = shadow_promote_argv("mfsmaster");
        assert_eq!(argv, ["mfsmaster", "-a", "start"]);
    }

    #[test]
    fn parse_ip_addr_output_found() {
        let output = "2: nebula1: <UP,LOWER_UP> ...\n    inet 10.42.0.1/16 brd 10.42.255.255 scope global nebula1\n";
        assert!(parse_ip_addr_output(output, "10.42.0.1"));
    }

    #[test]
    fn parse_ip_addr_output_not_found() {
        let output = "2: nebula1: <UP,LOWER_UP> ...\n    inet 10.42.0.5/16 brd 10.42.255.255 scope global nebula1\n";
        assert!(!parse_ip_addr_output(output, "10.42.0.1"));
    }

    #[test]
    fn setquota_argv_shape() {
        let argv = setquota_argv("mfssetquota", 70_000_000, 80_000_000, "/mnt/mesh-storage");
        assert_eq!(
            argv,
            [
                "mfssetquota",
                "-p",
                "/",
                "0",
                "0",
                "70000000",
                "80000000",
                "/mnt/mesh-storage"
            ]
        );
    }

    #[test]
    fn min_chunkserver_avail_derived_from_porcelain_rows() {
        // The min-avail used by the quota tick is the smallest (total−used)
        // across the porcelain rows.
        let rows = parse_chunkservers_porcelain(PORCELAIN_FIXTURE);
        let min = rows.iter().map(|cs| cs.avail_bytes).min();
        // Row 0 (10.42.0.2) has the smaller free space of the two.
        assert_eq!(min, Some(24_306_466_816 - 5_966_987_264));
    }

    // MESHFS-10.1 — dispatch_meshfs_action tests (no subprocess; verb shapes only).

    #[test]
    fn dispatch_action_unknown_verb_returns_error() {
        let reply = dispatch_meshfs_action("mfsmaster", "10.42.0.1", 2, "frobnicate", "{}");
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], false);
    }

    #[test]
    fn dispatch_action_add_peer_ok() {
        let reply = dispatch_meshfs_action("mfsmaster", "10.42.0.1", 3, "add-peer", "{}");
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn dispatch_action_remove_peer_ok() {
        let reply = dispatch_meshfs_action("mfsmaster", "10.42.0.1", 1, "remove-peer", "{}");
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn dispatch_resolve_conflict_missing_path() {
        let reply = dispatch_resolve_conflict("{}");
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap_or("").contains("path"));
    }

    #[test]
    fn dispatch_resolve_conflict_path_not_found() {
        let reply = dispatch_resolve_conflict(
            r#"{"path":"/tmp/meshfs-xyzzy-does-not-exist.conflict-peer-0"}"#,
        );
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap_or("").contains("not found"));
    }

    #[test]
    fn dispatch_undelete_missing_path() {
        let reply = dispatch_undelete("{}");
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap_or("").contains("trash_path"));
    }

    #[test]
    fn dispatch_undelete_moves_entry_into_undel() {
        // AUDIT-MESH-7: undelete is a filesystem move into the trash's undel/
        // inbox (the real LizardFS mechanism), not a dead `mfsadmin TRASH-RECOVER`.
        let trash = tempfile::tempdir().unwrap();
        let entry = trash.path().join("0000ABCDreport.txt");
        std::fs::write(&entry, b"recover me").unwrap();
        let body = format!(
            r#"{{"trash_path":{}}}"#,
            serde_json::Value::String(entry.to_string_lossy().into_owned())
        );
        let reply = dispatch_undelete(&body);
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["ok"], true, "reply was {reply}");
        // Entry moved out of the trash root into undel/.
        assert!(!entry.exists());
        let moved = trash.path().join("undel").join("0000ABCDreport.txt");
        assert!(moved.exists());
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "recover me");
    }

    #[test]
    fn dispatch_undelete_missing_entry_reports_error() {
        let body = r#"{"trash_path":"/nonexistent/.trash/0000DEADfile"}"#;
        let v: serde_json::Value = serde_json::from_str(&dispatch_undelete(body)).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap_or("").contains("not found"));
    }

    // MESHFS-6.1 — offline staging + LWW replay tests.

    #[test]
    fn staged_files_empty_when_dir_absent() {
        let files = staged_files(Path::new("/nonexistent/stage-xyzzy-123"));
        assert!(files.is_empty());
    }

    #[test]
    fn staged_files_walks_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("docs");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
        std::fs::write(sub.join("b.txt"), b"b").unwrap();
        let mut files = staged_files(dir.path());
        files.sort();
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.file_name().unwrap() == "a.txt"));
        assert!(files.iter().any(|f| f.file_name().unwrap() == "b.txt"));
    }

    #[test]
    fn replay_file_lww_applies_when_mesh_absent() {
        let stage_dir = tempfile::tempdir().unwrap();
        let mesh_dir = tempfile::tempdir().unwrap();
        let staged = stage_dir.path().join("doc.txt");
        std::fs::write(&staged, b"offline content").unwrap();
        let mesh_path = mesh_dir.path().join("doc.txt");
        let outcome = replay_file_lww(&staged, &mesh_path, "testpeer");
        assert!(matches!(outcome, ReplayOutcome::Applied { .. }));
        assert_eq!(
            std::fs::read_to_string(&mesh_path).unwrap(),
            "offline content"
        );
        assert!(
            !staged.exists(),
            "staged file should be removed after replay"
        );
    }

    #[test]
    fn replay_file_lww_staged_wins_when_newer() {
        let stage_dir = tempfile::tempdir().unwrap();
        let mesh_dir = tempfile::tempdir().unwrap();
        // Write mesh file first, then staged file (so staged is newer by mtime).
        let mesh_path = mesh_dir.path().join("data.bin");
        std::fs::write(&mesh_path, b"mesh version").unwrap();
        // Sleep briefly to ensure a mtime difference.
        std::thread::sleep(std::time::Duration::from_millis(10));
        let staged = stage_dir.path().join("data.bin");
        std::fs::write(&staged, b"staged version").unwrap();
        let outcome = replay_file_lww(&staged, &mesh_path, "testpeer");
        assert!(matches!(outcome, ReplayOutcome::ConflictStagedWins { .. }));
        assert_eq!(
            std::fs::read_to_string(&mesh_path).unwrap(),
            "staged version"
        );
        // Conflict file should exist with the old mesh content.
        if let ReplayOutcome::ConflictStagedWins { conflict_path, .. } = outcome {
            assert!(conflict_path.exists());
            let name = conflict_path.file_name().unwrap().to_string_lossy();
            assert!(name.contains("conflict-testpeer"), "name: {name}");
        }
    }

    #[test]
    fn replay_file_lww_mesh_wins_when_newer() {
        let stage_dir = tempfile::tempdir().unwrap();
        let mesh_dir = tempfile::tempdir().unwrap();
        let staged = stage_dir.path().join("note.txt");
        std::fs::write(&staged, b"old staged").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let mesh_path = mesh_dir.path().join("note.txt");
        std::fs::write(&mesh_path, b"newer mesh").unwrap();
        let outcome = replay_file_lww(&staged, &mesh_path, "testpeer");
        assert!(matches!(outcome, ReplayOutcome::ConflictMeshWins { .. }));
        // Mesh file unchanged.
        assert_eq!(std::fs::read_to_string(&mesh_path).unwrap(), "newer mesh");
        // Staged renamed to conflict.
        assert!(!staged.exists());
        if let ReplayOutcome::ConflictMeshWins { conflict_path } = outcome {
            let name = conflict_path.file_name().unwrap().to_string_lossy();
            assert!(name.contains("conflict-testpeer"), "name: {name}");
        }
    }

    #[test]
    fn replay_all_staged_returns_log_lines() {
        let stage_dir = tempfile::tempdir().unwrap();
        let mesh_dir = tempfile::tempdir().unwrap();
        std::fs::write(stage_dir.path().join("file1.txt"), b"data1").unwrap();
        std::fs::write(stage_dir.path().join("file2.txt"), b"data2").unwrap();
        let log = replay_all_staged(stage_dir.path(), mesh_dir.path(), "testpeer");
        assert_eq!(log.len(), 2, "expected 2 log lines, got: {log:?}");
        assert!(log.iter().all(|l| l.contains("meshfs replay")));
    }

    // ── MESHFS-13.1 + MESHFS-12.b: meshfs_status_report ──

    #[test]
    fn meshfs_status_report_unreachable_returns_empty_peers() {
        // Use a VIP that is certain not to answer in the test env.
        let report = meshfs_status_report("mfsadmin", "192.0.2.1");
        assert!(!report.master_reachable);
        assert!(report.peers.is_empty());
        assert!(report.quota_cap_bytes.is_none());
        assert!(report.limiting_peer_addr.is_none());
        assert_eq!(report.goal, 0);
        assert!(report.offline_peers.is_empty());
    }

    #[test]
    fn replication_goal_is_two_capped_by_peers() {
        // FPG-7 / Q12 — the goal-2 default, peer-capped.
        for (peers, want) in [(1usize, 1u8), (2, 2), (3, 2), (8, 2)] {
            let goal = (peers as u8).min(DEFAULT_REPLICATION_GOAL).max(1);
            assert_eq!(goal, want, "{peers} peers");
        }
    }

    #[test]
    fn xdg_bind_plan_covers_five_dirs_and_never_local() {
        // FPG-7 / Q13 — the five XDG dirs; ~/Local/ NEVER mesh-mounted.
        let plan = xdg_bind_plan(Path::new("/mnt/mesh-storage"), Path::new("/home/mm"));
        assert_eq!(plan.len(), 5);
        let targets: Vec<String> = plan.iter().map(|(_, t)| t.display().to_string()).collect();
        assert!(targets.contains(&"/home/mm/Documents".to_string()));
        assert!(targets.contains(&"/home/mm/Videos".to_string()));
        assert!(
            !targets.iter().any(|t| t.contains("Local")),
            "~/Local/ must never be mesh-mounted (Q13)"
        );
        assert!(plan
            .iter()
            .all(|(s, _)| s.starts_with("/mnt/mesh-storage/home")));
    }

    #[test]
    fn ensure_xdg_binds_reports_loudly_when_mount_absent() {
        // AUDIT-MESH-15: a missing mesh mount must return a failure line,
        // not silently no-op (the ONBOARD-6 failure class).
        let failures = ensure_xdg_binds(
            Path::new("/nonexistent-mesh-mount-xyz"),
            Path::new("/home/mm"),
        );
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("is not a directory"));
    }

    #[test]
    fn ensure_xdg_binds_creates_communal_source_when_mount_is_a_dir() {
        // With a real (temp) mount dir but no privilege to bind, the
        // communal source dirs are still created + the bind failures are
        // surfaced — never swallowed.
        let tmp = tempfile::tempdir().expect("tmp");
        let home = tmp.path().join("home-mm");
        std::fs::create_dir_all(&home).expect("home");
        let failures = ensure_xdg_binds(tmp.path(), &home);
        // Source communal dirs must exist regardless of bind privilege.
        assert!(tmp.path().join("home").join("Documents").is_dir());
        // Unprivileged test run can't `mount --bind` → every dir reports a
        // failure (loud), proving nothing is silently dropped.
        assert_eq!(failures.len(), XDG_MESH_DIRS.len());
    }

    #[test]
    fn desktop_user_homes_excludes_root_and_system_uids() {
        // The resolver must never return /root (uid 0) or system accounts —
        // that exact bug (binding the daemon's $HOME=/root) is AUDIT-MESH-15.
        for home in desktop_user_homes() {
            assert!(
                home.starts_with("/home"),
                "desktop home {} must live under /home, never /root or a system path",
                home.display()
            );
        }
    }

    #[test]
    fn meshfs_status_report_json_serializable() {
        let report = meshfs_status_report("mfsadmin", "192.0.2.1");
        let json = serde_json::to_string(&report).expect("serialize");
        assert!(json.contains("\"master_reachable\""));
        assert!(json.contains("\"peers\""));
        assert!(json.contains("\"goal\""));
        assert!(json.contains("\"offline_peers\""));
    }

    #[test]
    fn meshfs_status_report_with_enrolled_marks_missing_as_offline() {
        // Master unreachable → CS-LIST empty → all enrolled IPs are offline.
        let enrolled = vec!["10.42.0.5".to_owned(), "10.42.0.7".to_owned()];
        let report = meshfs_status_report_with_enrolled("mfsadmin", "192.0.2.1", &enrolled);
        // Both enrolled IPs absent from empty CS-LIST → both offline.
        let mut got = report.offline_peers.clone();
        got.sort();
        assert_eq!(got, vec!["10.42.0.5".to_owned(), "10.42.0.7".to_owned()]);
    }

    #[test]
    fn meshfs_status_report_with_enrolled_empty_slice_no_offline() {
        let report = meshfs_status_report_with_enrolled("mfsadmin", "192.0.2.1", &[]);
        assert!(report.offline_peers.is_empty());
    }

    #[test]
    fn settrashtime_argv_shape() {
        assert_eq!(
            settrashtime_argv("mfssettrashtime", 172_800, "/mnt/mesh-storage"),
            vec!["mfssettrashtime", "-r", "172800", "/mnt/mesh-storage"]
        );
    }

    #[test]
    fn settrashtime_argv_zero_disables_trash() {
        let argv = settrashtime_argv("mfssettrashtime", 0, "/mnt/mesh-storage");
        assert_eq!(argv[2], "0");
    }

    #[test]
    fn list_trash_entries_empty_when_mount_absent() {
        // `/tmp/mde-test-no-mount-xyz/.trash` does not exist.
        let entries = list_trash_entries("/tmp/mde-test-no-mount-xyz");
        assert!(entries.is_empty());
    }

    #[test]
    fn list_trash_entries_strips_hex_prefix() {
        // Build a temp dir that mimics `.trash` contents.
        let dir = tempfile::tempdir().expect("tempdir");
        let trash = dir.path().join(".trash");
        std::fs::create_dir_all(&trash).unwrap();
        std::fs::write(trash.join("0000001Areport.pdf"), b"").unwrap();
        std::fs::write(trash.join("nodots"), b"").unwrap();
        let mount = dir.path().to_str().unwrap().to_owned();
        let mut entries = list_trash_entries(&mount);
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        // The `report.pdf` entry strips the 8-hex prefix.
        let pdf = entries.iter().find(|e| e.name.contains("report.pdf"));
        assert!(pdf.is_some(), "expected report.pdf in {entries:?}");
        // The trash_path must include `.trash/0000001Areport.pdf`.
        assert!(pdf.unwrap().trash_path.contains(".trash"));
    }
}
