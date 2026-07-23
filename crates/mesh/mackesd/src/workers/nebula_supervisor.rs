//! NF-3.4 (v2.5) — Nebula supervisor worker.
//!
//! Watches the leader-election lease (already shipped via
//! `crate::leader`) and the QNM-Shared bundle file
//! (`~/QNM-Shared/<self>/mackesd/nebula-bundle.json`). On
//! leader-promotion this worker:
//!
//!   1. Writes the `role.host` marker at
//!      `/var/lib/mackesd/nebula/role.host`. Systemd's
//!      `ConditionPathExists=` on `nebula-lighthouse.service`
//!      + `mackes-nebula-https-tunnel.service` flips them
//!      from "skipped" → "ready to start." The supervisor
//!      then calls `systemctl start` on each.
//!   2. If no CA exists, calls `ca::mint::mint_ca` (idempotent
//!      — re-runs on existing meshes are no-ops).
//!
//! On leader-demotion the worker removes the marker + stops
//! the lighthouse/tunnel units (preserves nebula.service so
//! the local tun device stays up).
//!
//! On every tick (default 5 s) the worker watches the bundle
//! file's mtime; on change, it re-runs the config writer
//! (NF-3.5) so a freshly-replicated bundle takes effect
//! without a daemon restart.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::Mutex;

use super::{ShutdownToken, Worker};

/// Default sweep cadence.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(5);

/// Default marker file path that systemd's
/// `ConditionPathExists=` checks for lighthouse/tunnel
/// units.
pub const DEFAULT_ROLE_HOST_MARKER: &str = "/var/lib/mackesd/nebula/role.host";

/// GF-1.3.a (v5.0.0) — plain-text file containing the local
/// peer's Nebula overlay IP, written by the supervisor on
/// every `refresh_config` once a signed bundle is in place.
/// Consumed by downstream services that need to bind to the
/// overlay address without speaking the full bundle JSON
/// (notably `mackes-glusterd-nebula-bind.service` in GF-1.3.b
/// which rewrites `/etc/glusterfs/glusterd.vol` so glusterd
/// listens on the overlay rather than the public underlay).
pub const DEFAULT_OVERLAY_IP_PATH: &str = "/var/lib/mackesd/nebula/overlay-ip";

/// Worker handle. Holds the shared store (so CA mint can
/// query / insert) + the bundle-watch state.
pub struct NebulaSupervisor {
    store: Arc<Mutex<rusqlite::Connection>>,
    node_id: String,
    mesh_id: String,
    bundle_path: PathBuf,
    role_marker_path: PathBuf,
    config_dir: PathBuf,
    overlay_ip_path: PathBuf,
    tick_interval: Duration,
    /// Cached bundle mtime so a change triggers a re-write.
    last_bundle_mtime: Option<SystemTime>,
    /// ENT-3 — the replicated root carrying ca/blocklist.
    workgroup_root: PathBuf,
    /// ENT-3 — last-applied blocklist union (change triggers reload).
    last_blocklist: Vec<String>,
    /// Last-known leader state — flipping this triggers the
    /// promote / demote transition.
    last_is_leader: bool,
}

impl NebulaSupervisor {
    /// Construct a supervisor bound to the given store + node.
    /// `bundle_path` is normally
    /// `~/QNM-Shared/<self>/mackesd/nebula-bundle.json`; pass
    /// an explicit path for tests.
    #[must_use]
    pub fn new(
        store: Arc<Mutex<rusqlite::Connection>>,
        node_id: String,
        mesh_id: String,
        bundle_path: PathBuf,
    ) -> Self {
        // ENT-3 — the blocklist union lives on the replicated root;
        // derive it once (override via with_workgroup_root in tests).
        let workgroup_root = crate::default_qnm_shared_root();
        Self {
            store,
            node_id,
            mesh_id,
            bundle_path,
            role_marker_path: PathBuf::from(DEFAULT_ROLE_HOST_MARKER),
            config_dir: PathBuf::from("/etc/nebula"),
            overlay_ip_path: PathBuf::from(DEFAULT_OVERLAY_IP_PATH),
            tick_interval: DEFAULT_TICK_INTERVAL,
            last_bundle_mtime: None,
            last_is_leader: false,
            workgroup_root,
            last_blocklist: Vec::new(),
        }
    }

    /// ENT-3 test seam — point the blocklist union at a scratch root.
    #[must_use]
    pub fn with_workgroup_root(mut self, root: PathBuf) -> Self {
        self.workgroup_root = root;
        self
    }

    /// Override the marker path — used by tests that can't
    /// write to /var.
    #[must_use]
    pub fn with_role_marker(mut self, path: PathBuf) -> Self {
        self.role_marker_path = path;
        self
    }

    /// Override the systemd config dir — used by tests.
    #[must_use]
    pub fn with_config_dir(mut self, path: PathBuf) -> Self {
        self.config_dir = path;
        self
    }

    /// GF-1.3.a — override the overlay-ip publish path. Tests
    /// that don't run as root point this at a tempdir.
    #[must_use]
    pub fn with_overlay_ip_path(mut self, path: PathBuf) -> Self {
        self.overlay_ip_path = path;
        self
    }

    /// Override the tick interval — used by tests.
    #[must_use]
    pub fn with_tick_interval(mut self, interval: Duration) -> Self {
        self.tick_interval = interval;
        self
    }

    /// One sweep. Pure-ish (touches disk + may shell out
    /// to systemctl, but no network). Returns Ok(()) on
    /// success; logs + swallows individual step failures so
    /// a single bad tick doesn't kill the worker.
    async fn tick(&mut self) {
        // 1. Check current leader state — the role-host marker (configurable
        //    path, so tests don't read the production marker).
        let is_leader_now = check_leader(&self.role_marker_path);
        if is_leader_now != self.last_is_leader {
            if is_leader_now {
                if let Err(e) = self.promote().await {
                    tracing::warn!(error = %e, "nebula-supervisor: promote failed");
                }
            } else if let Err(e) = self.demote() {
                tracing::warn!(error = %e, "nebula-supervisor: demote failed");
            }
            self.last_is_leader = is_leader_now;
        }

        // 1.5 HA — keep THIS node's bundle lighthouse roster in sync with the
        //     canonical directory so a newly-added lighthouse propagates to an
        //     already-enrolled peer (e.g. Eagle) without a re-enroll. Rewrites the
        //     bundle (bumping its mtime) only on a real change; the mtime watch in
        //     step 2 then re-renders /etc/nebula + reloads nebula.
        self.reconcile_lighthouse_roster();

        // 2. Watch the bundle file + the revocation blocklist for
        //    changes (ENT-3: a revoke anywhere must evict here).
        let blocklist_now = crate::ca::blocklist::all_fingerprints(&self.workgroup_root);
        let blocklist_changed = blocklist_now != self.last_blocklist;
        if let Ok(meta) = std::fs::metadata(&self.bundle_path) {
            if let Ok(mtime) = meta.modified() {
                if self.last_bundle_mtime.map_or(true, |t| t != mtime) || blocklist_changed {
                    if let Err(e) = self.refresh_config().await {
                        tracing::warn!(error = %e, "nebula-supervisor: config refresh failed");
                    }
                    self.last_bundle_mtime = Some(mtime);
                    self.last_blocklist = blocklist_now;
                }
            }
        }
    }

    /// Leader-promotion: mint CA if missing, write
    /// role.host marker, start lighthouse + tunnel units.
    async fn promote(&self) -> Result<(), String> {
        tracing::info!(node = %self.node_id, "nebula-supervisor: promoting to host role");
        // a. Mint the CA if no active row exists.
        {
            let conn = self.store.lock().await;
            // NF-7.1 wizard takes the operator-input mesh-id;
            // for boot-time auto-mint we use the configured
            // mesh_id field.
            let _ = crate::ca::mint::mint_ca(
                &crate::ca::SubprocessBackend,
                &conn,
                &self.mesh_id,
                None,
                None,
            )
            .map_err(|e| e.to_string());
            // mint_ca's idempotent + the BinaryMissing error
            // is expected on dev hosts without nebula
            // installed — log + continue.
        }
        // b. Write the role marker.
        write_role_marker(&self.role_marker_path)?;
        // c. Start the systemd units. systemctl invocations
        //    are best-effort — we still proceed if systemctl
        //    is unreachable (containerized test envs).
        let _ = systemctl_start("nebula-lighthouse.service");
        let _ = systemctl_start("mackes-nebula-https-tunnel.service");
        Ok(())
    }

    /// Leader-demotion: stop lighthouse + tunnel, remove
    /// marker. nebula.service stays up — the local peer
    /// needs its tun device regardless of role.
    fn demote(&self) -> Result<(), String> {
        tracing::info!(node = %self.node_id, "nebula-supervisor: demoting to peer role");
        let _ = systemctl_stop("mackes-nebula-https-tunnel.service");
        let _ = systemctl_stop("nebula-lighthouse.service");
        if self.role_marker_path.exists() {
            std::fs::remove_file(&self.role_marker_path).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Re-materialize the on-disk Nebula config from the
    /// QNM-Shared bundle + signal the running nebula
    /// process to reload.
    async fn refresh_config(&self) -> Result<(), String> {
        let bundle =
            crate::ca::bundle::read_bundle(&self.bundle_path).map_err(|e| e.to_string())?;
        // Bug #3 (operator decision 2026-06-10): a node's nebula lighthouse
        // role is STATIC — it's a lighthouse iff its own overlay IP is in
        // the bundle's lighthouse set — NOT a function of FPG leadership.
        // Tying am_lighthouse to `last_is_leader` made the founding host
        // render a peer config (am_lighthouse: false) pointing
        // static_host_map at itself, so the overlay never formed. FPG
        // leadership stays a separate control-plane concern.
        let role = if bundle
            .lighthouses
            .iter()
            .any(|lh| lh.overlay_ip == bundle.overlay_ip)
        {
            ConfigRole::Host
        } else {
            ConfigRole::Peer
        };
        // ENT-3 — the replicated revocation union rides every render.
        let blocklist = crate::ca::blocklist::all_fingerprints(&self.workgroup_root);
        materialize_config(
            &self.config_dir,
            &bundle,
            role,
            &blocklist,
            &self.workgroup_root,
            Some(b"key-pem"),
        )?;
        // GF-1.3.a — publish the overlay IP so downstream
        // services (notably mackes-glusterd-nebula-bind in
        // GF-1.3.b) can rewrite their bind config without
        // re-parsing the full NebulaBundle JSON. Best-effort —
        // a publish failure is logged but doesn't abort the
        // Nebula-config refresh (the daemon itself still has
        // a valid /etc/nebula tree).
        if let Err(e) = publish_overlay_ip(&self.overlay_ip_path, &bundle.overlay_ip) {
            tracing::warn!(
                error = %e,
                path = %self.overlay_ip_path.display(),
                "nebula-supervisor: publishing overlay-ip failed",
            );
        }
        let _ = systemctl_reload("nebula.service");
        if self.last_is_leader {
            let _ = systemctl_reload("nebula-lighthouse.service");
        }
        Ok(())
    }

    /// HA — propagate a changed lighthouse SET into THIS node's own bundle so an
    /// already-enrolled peer (e.g. Eagle) picks up a newly-added lighthouse WITHOUT
    /// re-enrolling. The full roster is assembled from the directory only at first
    /// enroll (the `/enroll` listener), so without this an enrolled peer's bundle —
    /// and thus its `static_host_map` / `lighthouse.hosts` — is frozen and never
    /// learns a lighthouse added later (Gap C). Each tick this reads the canonical
    /// directory (etcd-first), derives the lighthouse roster, and — only when it
    /// differs from the bundle's current roster AND is non-empty — rewrites the
    /// bundle's `lighthouses`. The atomic write bumps the bundle mtime, so the
    /// mtime watch in [`Self::tick`] re-renders `/etc/nebula` + reloads nebula on
    /// the same/next tick. Runs on EVERY node and only ever rewrites its OWN
    /// bundle — no cross-node fs assumptions. A node that is itself a directory
    /// lighthouse self-includes here and `refresh_config` then renders
    /// `am_lighthouse: true` (the self-promotion path for a newly-joined LH).
    ///
    /// The non-empty guard is load-bearing: a transient empty/failed directory read
    /// must NEVER wipe a peer's lighthouse set and strand it off the overlay.
    fn reconcile_lighthouse_roster(&self) {
        let mut bundle = match crate::ca::bundle::read_bundle(&self.bundle_path) {
            Ok(b) => b,
            // No bundle yet (pre-enroll) — nothing to reconcile.
            Err(_) => return,
        };
        let peers = crate::substrate::peers::read_directory(&self.workgroup_root);
        let authority = bundle.relay_trust_authority.clone();
        let mut roster: Vec<crate::ca::bundle::LighthouseEntry> =
            mackes_mesh_types::lighthouse::roster_from_directory(&peers)
                .into_iter()
                .map(|a| {
                    crate::ca::bundle::lighthouse_entry_with_relay_trust(
                        &self.workgroup_root,
                        a.node_id,
                        a.overlay_ip,
                        a.external_addr,
                        authority.as_deref(),
                    )
                })
                .collect();
        if roster.is_empty() {
            // Never strand a peer on a transient empty/failed read — keep the
            // bundle's existing roster untouched.
            return;
        }
        // Replication of a directory row and that lighthouse's self-bundle is
        // not atomic. Preserve already authenticated trust during that window;
        // a missing advertisement must never erase a usable pin.
        for entry in &mut roster {
            if entry.relay_tls.is_none() {
                entry.relay_tls = bundle
                    .lighthouses
                    .iter()
                    .find(|current| current.node_id == entry.node_id)
                    .and_then(|current| current.relay_tls.clone())
                    .filter(|identity| {
                        authority.as_deref().is_some_and(|public_key| {
                            crate::ca::bundle::verify_relay_tls_identity(
                                identity,
                                &entry.node_id,
                                &entry.overlay_ip,
                                &entry.external_addr,
                                public_key,
                            )
                        })
                    });
            }
        }
        // Compare as sets (sorted by node_id) so render-order differences alone
        // don't trigger a rewrite/reload every tick.
        roster.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        let mut current = bundle.lighthouses.clone();
        current.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        if current == roster {
            return;
        }
        let count = roster.len();
        bundle.lighthouses = roster;
        match crate::ca::bundle::write_bundle(&self.bundle_path, &bundle) {
            Ok(()) => tracing::info!(
                count,
                "nebula-supervisor: reconciled lighthouse roster from directory \
                 (bundle rewritten; nebula will reload via the mtime watch)"
            ),
            Err(e) => tracing::warn!(
                error = %e,
                "nebula-supervisor: lighthouse roster reconcile write failed"
            ),
        }
    }
}

#[async_trait::async_trait]
impl Worker for NebulaSupervisor {
    fn name(&self) -> &'static str {
        "nebula-supervisor"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // One immediate tick so the marker / config land on
        // boot before we wait the full interval.
        self.tick().await;
        loop {
            tokio::select! {
                _ = shutdown.wait() => break,
                _ = tokio::time::sleep(self.tick_interval) => self.tick().await,
            }
        }
        Ok(())
    }
}

/// Distinct from `ca::sign::PeerRole` — this enum drives
/// the *config-file* shape rather than the cert groups.
/// Host gets the full lighthouse listener section; Peer
/// gets the lighthouse-roster client section only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigRole {
    /// Lighthouse-eligible role — config file carries the
    /// full lighthouse listener section.
    Host,
    /// Mesh-peer role — config file carries the lighthouse-
    /// roster client section only (no listener).
    Peer,
}

fn install_identity_generation(
    config_dir: &Path,
    bundle: &crate::ca::bundle::NebulaBundle,
    requester_private_key: Option<&[u8]>,
) -> Result<(), String> {
    use std::os::unix::fs::{symlink, DirBuilderExt};

    let identity_dir = config_dir.join("identity");
    let mut identity_builder = std::fs::DirBuilder::new();
    identity_builder.mode(0o700).recursive(true);
    identity_builder
        .create(&identity_dir)
        .map_err(|e| format!("create identity dir {}: {e}", identity_dir.display()))?;

    // Replicated steady-state refreshes may update topology/config, but cannot
    // replace an already-active identity. Only the fingerprint-pinned network
    // enrollment path supplies `requester_private_key` and is authorized to
    // activate a new cert/key generation.
    let current_cert = identity_dir.join("current/host.crt");
    if requester_private_key.is_none() && current_cert.exists() {
        let active_cert = std::fs::read(&current_cert)
            .map_err(|e| format!("read active identity cert {}: {e}", current_cert.display()))?;
        if active_cert != bundle.peer_cert_pem.as_bytes() {
            return Err(
                "replicated bundle attempted to replace the active Nebula identity; authenticated enrollment is required"
                    .into(),
            );
        }
        replace_symlink(
            &config_dir.join("host.crt"),
            Path::new("identity/current/host.crt"),
        )?;
        replace_symlink(
            &config_dir.join("host.key"),
            Path::new("identity/current/host.key"),
        )?;
        return Ok(());
    }

    let owned_key;
    let key_bytes = if let Some(key) = requester_private_key {
        key
    } else {
        let current_key = identity_dir.join("current/host.key");
        let legacy_key = config_dir.join("host.key");
        owned_key = crate::ca::seal::read_sealed(if current_key.exists() {
            &current_key
        } else {
            &legacy_key
        })
        .map_err(|e| format!("local requester-owned Nebula key unavailable: {e}"))?;
        &owned_key
    };

    let generation_dir = (0..16)
        .find_map(|_| {
            let candidate = identity_dir.join(format!(
                "generation-{}-{:016x}",
                std::process::id(),
                rand::random::<u64>()
            ));
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&candidate) {
                Ok(()) => Some(Ok(candidate)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(format!(
                    "create identity generation {}: {error}",
                    candidate.display()
                ))),
            }
        })
        .unwrap_or_else(|| Err("identity generation tempfile collisions".into()))?;
    let stage_result = (|| {
        crate::ca::seal::write_atomic_sealed(
            &generation_dir.join("host.crt"),
            bundle.peer_cert_pem.as_bytes(),
        )
        .map_err(|e| e.to_string())?;
        crate::ca::seal::write_atomic_sealed(&generation_dir.join("host.key"), key_bytes)
            .map_err(|e| e.to_string())?;
        std::fs::File::open(&generation_dir)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| {
                format!(
                    "fsync identity generation {}: {e}",
                    generation_dir.display()
                )
            })
    })();
    if let Err(error) = stage_result {
        let _ = std::fs::remove_dir_all(&generation_dir);
        return Err(error);
    }

    let generation_leaf = generation_dir
        .file_name()
        .ok_or_else(|| "identity generation has no filename".to_string())?;
    let temp_link = identity_dir.join(format!(
        ".current.tmp.{}.{:016x}",
        std::process::id(),
        rand::random::<u64>()
    ));
    symlink(generation_leaf, &temp_link)
        .map_err(|e| format!("create identity switch {}: {e}", temp_link.display()))?;
    if let Err(error) = std::fs::rename(&temp_link, identity_dir.join("current")) {
        let _ = std::fs::remove_file(&temp_link);
        let _ = std::fs::remove_dir_all(&generation_dir);
        return Err(format!("activate identity generation: {error}"));
    }
    std::fs::File::open(&identity_dir)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| format!("fsync identity dir {}: {e}", identity_dir.display()))?;

    // Compatibility paths point through the single atomic `current` switch;
    // replacing either link cannot expose a mismatched pair to Nebula because
    // the generated config reads the identity/current paths directly.
    replace_symlink(
        &config_dir.join("host.crt"),
        Path::new("identity/current/host.crt"),
    )?;
    replace_symlink(
        &config_dir.join("host.key"),
        Path::new("identity/current/host.key"),
    )?;
    Ok(())
}

fn replace_symlink(path: &Path, target: &Path) -> Result<(), String> {
    use std::os::unix::fs::symlink;
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    let temp = parent.join(format!(
        ".link.tmp.{}.{:016x}",
        std::process::id(),
        rand::random::<u64>()
    ));
    symlink(target, &temp).map_err(|e| format!("create symlink {}: {e}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("activate symlink {}: {e}", path.display())
    })?;
    std::fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| format!("fsync symlink parent {}: {e}", parent.display()))
}

/// NF-3.5 — write the four canonical Nebula config files
/// atomically (temp + rename per file). Caller is the
/// supervisor's `refresh_config` path; tests pass a tempdir
/// so the production paths stay untouched.
pub fn materialize_config(
    config_dir: &Path,
    bundle: &crate::ca::bundle::NebulaBundle,
    role: ConfigRole,
    blocklist: &[String],
    workgroup_root: &Path,
    requester_private_key: Option<&[u8]>,
) -> Result<(), String> {
    std::fs::create_dir_all(config_dir)
        .map_err(|e| format!("mkdir {}: {e}", config_dir.display()))?;

    write_atomic(&config_dir.join("ca.crt"), bundle.ca_cert_pem.as_bytes())?;
    install_identity_generation(config_dir, bundle, requester_private_key)?;
    // PLANES-17 — fold the fleet's hop/exit routes into this node's
    // unsafe_routes. Exits ride only behind a passing validation verdict.
    let routes = crate::nebula_topology::derive_routes(
        &crate::nebula_topology::read_adverts(workgroup_root),
        &bundle.overlay_ip,
        crate::nebula_topology::exits_validated(workgroup_root),
    );
    // NET-1 (PD-6/PD-7): append the loopback debug-SSH block so nebula exposes
    // per-tunnel direct/relay introspection. Empty string (no block) when keys
    // can't be generated — honest degradation, classification stays "overlay".
    let sshd = crate::nebula_admin::ensure_and_render_sshd(config_dir);
    let yaml = render_config_yaml_with_routes(bundle, role, blocklist, &routes);
    write_atomic(
        &config_dir.join("config.yaml"),
        format!("{yaml}{sshd}").as_bytes(),
    )?;
    // FOUND-NEBULA (2026-06-20): the `nebula` Fedora package ships an EXAMPLE
    // `/etc/nebula/config.yml` (am_lighthouse:false, pki.cert=host.crt with a
    // bogus 192.168.100.1 static_host_map). The nebula unit runs
    // `-config /etc/nebula` (the whole DIRECTORY), so nebula MERGES that stale
    // example with our `config.yaml` — the example's am_lighthouse:false +
    // garbage static_host_map win, the overlay never forms, and (since it's a
    // hard config error) the unit fails on a fresh node. Found bringing up a
    // clean v11 lighthouse on F43. Remove the stock `.yml` so only our
    // `.yaml` (+ `lighthouse-config.yaml`) drive nebula. Best-effort.
    let stock = config_dir.join("config.yml");
    if stock.exists() {
        let _ = std::fs::remove_file(&stock);
    }
    if role == ConfigRole::Host {
        let lh_yaml = render_lighthouse_config_yaml_with_routes(bundle, &routes);
        write_atomic(
            &config_dir.join("lighthouse-config.yaml"),
            format!("{lh_yaml}{sshd}").as_bytes(),
        )?;
    }
    Ok(())
}

/// VIRT-4.a (v5.0.0) — VM Nebula subnet announced via
/// `tun.unsafe_routes` on every peer's nebula config so guests
/// across the mesh remain mutually routable per
/// `docs/design/v5.0.0-compute.md` §4. The `128` bit splits the
/// `10.42.0.0/16` mesh between the peer subnet (`10.42.0.0/17`,
/// existing enrollment) and this VM subnet.
///
/// Exposed at module scope so VIRT-4.b (`nebula_enroll` dynamic
/// re-render), VIRT-5 (cert sign-request CN/ip allocation), and
/// VIRT-6 (`compute_provision` cert request payload) all reference
/// the single source of truth.
pub const VM_SUBNET_CIDR: &str = "10.42.128.0/17";

/// Pure helper — build the regular peer-role config YAML.
/// Pulled out for testing without filesystem IO.
#[must_use]
pub fn render_config_yaml(bundle: &crate::ca::bundle::NebulaBundle, role: ConfigRole) -> String {
    render_config_yaml_inner(bundle, role, true, &[], &[])
}

/// ENT-3 — as [`render_config_yaml`] with the revocation blocklist
/// folded into `pki.blocklist`.
#[must_use]
pub fn render_config_yaml_with_blocklist(
    bundle: &crate::ca::bundle::NebulaBundle,
    role: ConfigRole,
    blocklist: &[String],
) -> String {
    render_config_yaml_inner(bundle, role, true, blocklist, &[])
}

/// PLANES-17 — as [`render_config_yaml_with_blocklist`] plus the
/// fleet-derived hop/exit `tun.unsafe_routes` edges (`(route, via)`).
#[must_use]
pub fn render_config_yaml_with_routes(
    bundle: &crate::ca::bundle::NebulaBundle,
    role: ConfigRole,
    blocklist: &[String],
    extra_routes: &[(String, String)],
) -> String {
    render_config_yaml_inner(bundle, role, true, blocklist, extra_routes)
}

/// VIRT-6 (v5.0.0) — render a **guest VM's** Nebula config. Identical
/// to the peer-role config (it inherits the host's lighthouse roster
/// + the open-mesh firewall + listen stanza so the VM joins the
/// overlay as a normal node on `10.42.128.0/17`), but WITHOUT the
/// VIRT-4.a `tun.unsafe_routes` block: a guest is a leaf node on the
/// VM subnet and must not route the VM subnet to itself. The
/// VM-subnet route lives only on the **host** peers (they advertise
/// reachability of the VM subnet on the operator's behalf).
///
/// `compute_provision` writes this into the guest at
/// `/etc/nebula/config.yml` via cloud-init `write_files`, alongside
/// the VM's `host.key` (requester-side keygen), `host.crt` + `ca.crt`
/// (from the cert_authority reply).
#[must_use]
pub fn render_guest_config_yaml(bundle: &crate::ca::bundle::NebulaBundle) -> String {
    render_config_yaml_inner(bundle, ConfigRole::Peer, false, &[], &[])
}

fn external_addr_host_is_numeric(addr: &str) -> bool {
    let host = addr
        .rsplit_once(':')
        .map_or(addr, |(host, _port)| host)
        .trim_matches(['[', ']']);
    host.parse::<std::net::IpAddr>().is_ok()
}

fn unique_lighthouse_static_maps<'a>(
    bundle: &'a crate::ca::bundle::NebulaBundle,
) -> Vec<&'a crate::ca::bundle::LighthouseEntry> {
    let mut entries: Vec<&crate::ca::bundle::LighthouseEntry> = Vec::new();
    for lh in &bundle.lighthouses {
        // Never map ourselves — a lighthouse that lists its own overlay
        // IP here tries to handshake itself ("Refusing to handshake with
        // myself"). Bug #3, found on the VM bed 2026-06-10.
        if lh.overlay_ip == bundle.overlay_ip {
            continue;
        }
        if let Some(existing) = entries
            .iter()
            .position(|existing| existing.overlay_ip == lh.overlay_ip)
        {
            if !external_addr_host_is_numeric(&entries[existing].external_addr)
                && external_addr_host_is_numeric(&lh.external_addr)
            {
                entries[existing] = lh;
            }
            continue;
        }
        entries.push(lh);
    }
    entries
}

fn address_host(address: &str) -> &str {
    address
        .rsplit_once(':')
        .map_or(address, |(host, port)| {
            if port.parse::<u16>().is_ok() {
                host
            } else {
                address
            }
        })
        .trim_matches(['[', ']'])
}

fn https_proxy_endpoint_for(
    bundle: &crate::ca::bundle::NebulaBundle,
    lighthouse: &crate::ca::bundle::LighthouseEntry,
    fallback_host: Option<&str>,
    bridge_bind: Option<&str>,
) -> Option<String> {
    let fallback_host = address_host(fallback_host?);
    if address_host(&lighthouse.external_addr) != fallback_host {
        return None;
    }
    let authority = bundle.relay_trust_authority.as_deref()?;
    let identity = lighthouse.relay_tls.as_ref()?;
    if !crate::ca::bundle::verify_relay_tls_identity(
        identity,
        &lighthouse.node_id,
        &lighthouse.overlay_ip,
        &lighthouse.external_addr,
        authority,
    ) {
        return None;
    }
    let bind: std::net::SocketAddr = bridge_bind
        .unwrap_or(crate::workers::mesh_router::DEFAULT_HTTPS_UDP_BRIDGE_BIND)
        .parse()
        .ok()?;
    let dial_ip = if bind.ip().is_unspecified() {
        if bind.is_ipv4() {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        }
    } else {
        bind.ip()
    };
    Some(std::net::SocketAddr::new(dial_ip, bind.port()).to_string())
}

fn render_config_yaml_inner(
    bundle: &crate::ca::bundle::NebulaBundle,
    role: ConfigRole,
    include_vm_route: bool,
    blocklist: &[String],
    extra_routes: &[(String, String)],
) -> String {
    let mut out = String::new();
    out.push_str("# Generated by mackesd nebula-supervisor (NF-3.4)\n");
    out.push_str("# Do not edit by hand — the supervisor rewrites this\n");
    out.push_str("# on every bundle refresh.\n\n");
    out.push_str("pki:\n");
    out.push_str("  ca: /etc/nebula/ca.crt\n");
    out.push_str("  cert: /etc/nebula/identity/current/host.crt\n");
    out.push_str("  key: /etc/nebula/identity/current/host.key\n");
    // ENT-3 (C2) — revoked-cert fingerprints: nebula refuses tunnels
    // with these certs immediately, fleet-wide, instead of trusting
    // them until expiry.
    if blocklist.is_empty() {
        out.push('\n');
    } else {
        out.push_str("  blocklist:\n");
        for fp in blocklist {
            out.push_str(&format!("    - \"{fp}\"\n"));
        }
        out.push('\n');
    }
    out.push_str("static_host_map:\n");
    for lh in unique_lighthouse_static_maps(bundle) {
        let proxy = https_proxy_endpoint_for(
            bundle,
            lh,
            std::env::var(crate::transport::https443::FALLBACK_HOST_ENV)
                .ok()
                .as_deref(),
            std::env::var(crate::workers::mesh_router::HTTPS_UDP_BRIDGE_BIND_ENV)
                .ok()
                .as_deref(),
        );
        match proxy {
            Some(proxy) => out.push_str(&format!(
                "  \"{}\": [\"{}\", \"{}\"]\n",
                lh.overlay_ip, lh.external_addr, proxy,
            )),
            None => out.push_str(&format!(
                "  \"{}\": [\"{}\"]\n",
                lh.overlay_ip, lh.external_addr,
            )),
        }
    }
    out.push_str("\nlighthouse:\n");
    match role {
        ConfigRole::Host => {
            out.push_str("  am_lighthouse: true\n");
        }
        ConfigRole::Peer => {
            out.push_str("  am_lighthouse: false\n");
            out.push_str("  hosts:\n");
            let mut seen = std::collections::BTreeSet::new();
            for lh in &bundle.lighthouses {
                if seen.insert(&lh.overlay_ip) {
                    out.push_str(&format!("    - \"{}\"\n", lh.overlay_ip));
                }
            }
        }
    }
    out.push_str("\nlisten:\n");
    out.push_str("  host: 0.0.0.0\n");
    out.push_str("  port: 4242\n\n");
    // Per the open-mesh / flat-trust directive:
    // a single open firewall rule — every peer can reach
    // every other peer on every port + protocol.
    out.push_str("# Open-mesh directive (2026-05-23):\n");
    out.push_str("# every peer fully trusts every other.\n");
    out.push_str("firewall:\n");
    out.push_str("  outbound:\n");
    out.push_str("    - port: any\n");
    out.push_str("      proto: any\n");
    out.push_str("      host: any\n");
    out.push_str("  inbound:\n");
    out.push_str("    - port: any\n");
    out.push_str("      proto: any\n");
    out.push_str("      host: any\n");
    // VIRT-4.a (v5.0.0) — VM subnet announcement. Every HOST peer
    // advertises 10.42.128.0/17 via its own overlay IP so guests on
    // peer A can reach guests on peer B directly via the Nebula
    // overlay (docs/design/v5.0.0-compute.md §4). The `via` value
    // is this peer's overlay IP (bundle.overlay_ip); the lighthouse
    // inherits the same block from this renderer. Guest VM configs
    // (VIRT-6 render_guest_config_yaml) pass include_vm_route=false
    // since a leaf node must not route the VM subnet to itself.
    // The overlay interface MUST be named `nebula1` — mackesd's workers
    // and the per-service overlay bindings resolve the interface by that
    // name (compute_provision::DEFAULT_NEBULA_INTERFACE). Without an
    // explicit `tun.dev`, nebula auto-names it `tun0` and every
    // overlay-bound lookup fails ("Failed to resolve interface nebula1").
    // The `tun:` block is therefore ALWAYS emitted (was: only when an
    // unsafe_route existed). Found bringing up the local VM bed 2026-06-10.
    out.push_str("\ntun:\n");
    out.push_str("  dev: nebula1\n");
    if include_vm_route || !extra_routes.is_empty() {
        // VM subnet routing (VIRT-4.a) + hop/exit routes (PLANES-17):
        out.push_str("  unsafe_routes:\n");
        if include_vm_route {
            out.push_str(&format!("    - route: {VM_SUBNET_CIDR}\n"));
            out.push_str(&format!("      via: {}\n", bundle.overlay_ip));
        }
        // PLANES-17 — fleet-derived hop subnet routes + (validated) exits.
        out.push_str(&crate::nebula_topology::render_unsafe_route_items(
            extra_routes,
        ));
    }
    out
}

/// Pure helper — lighthouse-role config (overrides
/// am_lighthouse + adds the relay/punchy stanzas).
#[must_use]
pub fn render_lighthouse_config_yaml(bundle: &crate::ca::bundle::NebulaBundle) -> String {
    render_lighthouse_config_yaml_with_routes(bundle, &[])
}

/// PLANES-17 — lighthouse config with the fleet-derived hop/exit routes
/// folded into its `tun.unsafe_routes` before the relay/punchy stanzas.
#[must_use]
pub fn render_lighthouse_config_yaml_with_routes(
    bundle: &crate::ca::bundle::NebulaBundle,
    extra_routes: &[(String, String)],
) -> String {
    let mut out = render_config_yaml_with_routes(bundle, ConfigRole::Host, &[], extra_routes);
    out.push_str("\n# Lighthouse-only:\n");
    out.push_str("relay:\n");
    out.push_str("  am_relay: true\n");
    out.push_str("  use_relays: true\n");
    out.push_str("punchy:\n");
    out.push_str("  punch: true\n");
    out.push_str("  respond: true\n");
    out
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    crate::ca::seal::write_atomic_sealed(path, bytes).map_err(|e| e.to_string())
}

/// GF-1.3.a — atomic-write the plain-text overlay IP file.
/// Creates parent dirs if missing. Idempotent: a re-write of
/// the same IP still bumps mtime, but the bytes match so
/// downstream mtime-gate consumers can use a byte-compare to
/// skip the reload step.
///
/// Exposed at module scope so the gluster bind helper (and
/// future consumers) have a single shared path constant +
/// writer signature to lean on.
///
/// # Errors
///
/// Returns the formatted error string from the underlying
/// `std::fs` call when directory creation or rename fails.
pub fn publish_overlay_ip(path: &Path, overlay_ip: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let body = format!("{overlay_ip}\n");
    let tmp = path.with_extension("ip.tmp");
    std::fs::write(&tmp, body.as_bytes())
        .map_err(|e| format!("write tmp {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} → {}: {e}", tmp.display(), path.display()))
}

fn write_role_marker(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    std::fs::write(path, b"role:host\n")
        .map_err(|e| format!("write marker {}: {e}", path.display()))
}

/// Lightweight `systemctl <verb> <unit>` invocation. Returns
/// Ok(()) on success or Err(stderr) on failure. Tolerates
/// missing systemctl (returns Err so the caller can log +
/// continue).
fn systemctl(verb: &str, unit: &str) -> Result<(), String> {
    let out = std::process::Command::new("systemctl")
        .args([verb, unit])
        .output()
        .map_err(|e| format!("systemctl {verb}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn systemctl_start(unit: &str) -> Result<(), String> {
    systemctl("start", unit)
}

fn systemctl_stop(unit: &str) -> Result<(), String> {
    systemctl("stop", unit)
}

fn systemctl_reload(unit: &str) -> Result<(), String> {
    systemctl("reload-or-restart", unit)
}

/// Pure helper — `true` when this node currently holds the host (leader)
/// role, signalled by the presence of the role-host marker at
/// `marker_path`. The marker is the leader signal in both directions: the
/// boot-time wizard / a promotion writes it (via [`write_role_marker`],
/// reached from `promote`), and `demote` (or an external actor) removes it.
/// `marker_path` is the supervisor's configurable `role_marker_path` so
/// tests point it at a tempdir rather than the production
/// `/var/lib/mackesd/nebula/role.host`.
fn check_leader(marker_path: &Path) -> bool {
    marker_path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::bundle::{LighthouseEntry, NebulaBundle};

    fn sample_bundle() -> NebulaBundle {
        NebulaBundle {
            mesh_id: "m1".into(),
            epoch: 0,
            ca_cert_pem: "ca-pem".into(),
            peer_cert_pem: "peer-pem".into(),
            overlay_ip: "10.42.0.5".into(),
            mesh_cidr: "10.42.0.0/16".into(),
            lighthouses: vec![LighthouseEntry {
                node_id: "peer:lh1".into(),
                overlay_ip: "10.42.0.1".into(),
                external_addr: "lh1.example.com:4242".into(),
                relay_tls: None,
            }],
            relay_trust_authority: None,
            created_at: 1,
        }
    }

    #[test]
    fn materialize_writes_four_files_for_peer() {
        let tmp = tempfile::tempdir().expect("tempdir");
        materialize_config(
            tmp.path(),
            &sample_bundle(),
            ConfigRole::Peer,
            &[],
            tmp.path(),
            Some(b"key-pem"),
        )
        .expect("write");
        assert!(tmp.path().join("ca.crt").exists());
        assert!(tmp.path().join("host.crt").exists());
        assert!(tmp.path().join("host.key").exists());
        assert!(tmp.path().join("config.yaml").exists());
        assert!(!tmp.path().join("lighthouse-config.yaml").exists());
    }

    #[test]
    fn authenticated_identity_rotation_switches_cert_and_key_as_one_generation() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = sample_bundle();
        materialize_config(
            tmp.path(),
            &first,
            ConfigRole::Peer,
            &[],
            tmp.path(),
            Some(b"key-generation-a"),
        )
        .expect("first identity");
        let mut second = first.clone();
        second.peer_cert_pem = "cert-generation-b".into();
        materialize_config(
            tmp.path(),
            &second,
            ConfigRole::Peer,
            &[],
            tmp.path(),
            Some(b"key-generation-b"),
        )
        .expect("authenticated rotation");
        assert_eq!(
            std::fs::read(tmp.path().join("identity/current/host.crt")).unwrap(),
            b"cert-generation-b"
        );
        assert_eq!(
            crate::ca::seal::read_sealed(&tmp.path().join("identity/current/host.key")).unwrap(),
            b"key-generation-b"
        );
    }

    #[test]
    fn replicated_bundle_cannot_replace_active_identity() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = sample_bundle();
        materialize_config(
            tmp.path(),
            &first,
            ConfigRole::Peer,
            &[],
            tmp.path(),
            Some(b"local-requester-key"),
        )
        .expect("first identity");
        let mut hostile = first;
        hostile.peer_cert_pem = "hostile-replicated-cert".into();
        let error = materialize_config(
            tmp.path(),
            &hostile,
            ConfigRole::Peer,
            &[],
            tmp.path(),
            None,
        )
        .expect_err("replicated identity replacement must fail closed");
        assert!(error.contains("authenticated enrollment is required"));
        assert_eq!(
            std::fs::read(tmp.path().join("identity/current/host.crt")).unwrap(),
            b"peer-pem"
        );
        assert_eq!(
            crate::ca::seal::read_sealed(&tmp.path().join("identity/current/host.key")).unwrap(),
            b"local-requester-key"
        );
    }

    #[test]
    fn materialize_removes_stock_nebula_config_yml() {
        // FOUND-NEBULA: the nebula package's stale example /etc/nebula/config.yml
        // must be removed so the `-config /etc/nebula` directory load doesn't
        // merge it with our config.yaml (which broke a fresh v11 lighthouse).
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("config.yml"), b"am_lighthouse: false\n")
            .expect("seed stock");
        materialize_config(
            tmp.path(),
            &sample_bundle(),
            ConfigRole::Host,
            &[],
            tmp.path(),
            Some(b"key-pem"),
        )
        .expect("write");
        assert!(
            !tmp.path().join("config.yml").exists(),
            "stock config.yml must be removed"
        );
        assert!(tmp.path().join("config.yaml").exists());
    }

    #[test]
    fn materialize_writes_lighthouse_config_for_host() {
        let tmp = tempfile::tempdir().expect("tempdir");
        materialize_config(
            tmp.path(),
            &sample_bundle(),
            ConfigRole::Host,
            &[],
            tmp.path(),
            Some(b"key-pem"),
        )
        .expect("write");
        assert!(tmp.path().join("lighthouse-config.yaml").exists());
    }

    #[test]
    fn materialize_folds_hop_routes_into_config_but_gates_exits() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // A hop advertising a LAN subnet + a full exit.
        crate::nebula_topology::write_advert(
            tmp.path(),
            &crate::nebula_topology::HopAdvert {
                hop: "gw".into(),
                overlay_ip: "10.42.0.9".into(),
                subnets: vec!["192.168.50.0/24".into(), "0.0.0.0/0".into()],
            },
        )
        .expect("advert");
        // No validation verdict yet → the LAN route lands, the exit doesn't.
        materialize_config(
            tmp.path(),
            &sample_bundle(),
            ConfigRole::Peer,
            &[],
            tmp.path(),
            Some(b"key-pem"),
        )
        .expect("write");
        let cfg = std::fs::read_to_string(tmp.path().join("config.yaml")).unwrap();
        assert!(cfg.contains("route: 192.168.50.0/24"), "hop subnet routed");
        assert!(cfg.contains("via: 10.42.0.9"));
        assert!(
            !cfg.contains("route: 0.0.0.0/0"),
            "exit gated until validation"
        );
    }

    #[test]
    fn render_peer_config_includes_lighthouse_roster() {
        let yaml = render_config_yaml(&sample_bundle(), ConfigRole::Peer);
        assert!(yaml.contains("am_lighthouse: false"));
        assert!(yaml.contains("\"10.42.0.1\""));
    }

    #[test]
    fn signed_configured_lighthouse_gets_local_https_proxy_endpoint() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[9_u8; 32]);
        let mut bundle = sample_bundle();
        bundle.relay_trust_authority =
            Some(crate::ca::bundle::relay_trust_authority_public_key(&key));
        let lighthouse = &mut bundle.lighthouses[0];
        let identity = crate::ca::bundle::RelayTlsIdentity::from_certificate_pem(
            "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n",
        )
        .expect("identity");
        lighthouse.relay_tls = Some(crate::ca::bundle::sign_relay_tls_identity(
            identity,
            &lighthouse.node_id,
            &lighthouse.overlay_ip,
            &lighthouse.external_addr,
            &key,
        ));
        assert_eq!(
            https_proxy_endpoint_for(
                &bundle,
                &bundle.lighthouses[0],
                Some("lh1.example.com:443"),
                Some("0.0.0.0:4244"),
            )
            .as_deref(),
            Some("127.0.0.1:4244"),
        );
    }

    #[test]
    fn unsigned_lighthouse_never_gets_local_https_proxy_endpoint() {
        let bundle = sample_bundle();
        assert!(https_proxy_endpoint_for(
            &bundle,
            &bundle.lighthouses[0],
            Some("lh1.example.com"),
            Some("127.0.0.1:4244"),
        )
        .is_none());
    }

    #[test]
    fn duplicate_lighthouse_rows_render_one_static_map_preferring_numeric_addr() {
        let mut b = sample_bundle();
        b.lighthouses.push(LighthouseEntry {
            node_id: "peer:lh1".into(),
            overlay_ip: "10.42.0.1".into(),
            external_addr: "203.0.113.7:4242".into(),
            relay_tls: None,
        });

        let yaml = render_config_yaml(&b, ConfigRole::Peer);
        assert_eq!(
            yaml.matches("  \"10.42.0.1\":").count(),
            1,
            "static_host_map must not emit duplicate YAML keys:\n{yaml}"
        );
        assert!(
            yaml.contains("203.0.113.7:4242"),
            "numeric underlay address should win over hostname fallback:\n{yaml}"
        );
        assert!(
            !yaml.contains("lh1.example.com:4242"),
            "hostname fallback must not survive as duplicate static_host_map:\n{yaml}"
        );
        assert_eq!(
            yaml.matches("    - \"10.42.0.1\"").count(),
            1,
            "lighthouse.hosts should be deduped too:\n{yaml}"
        );
    }

    #[test]
    fn render_host_config_marks_am_lighthouse_true() {
        let yaml = render_config_yaml(&sample_bundle(), ConfigRole::Host);
        assert!(yaml.contains("am_lighthouse: true"));
    }

    #[test]
    fn render_includes_open_mesh_firewall_rule() {
        // Open-mesh directive (2026-05-23) — flat trust;
        // every port + proto allowed inbound/outbound.
        let yaml = render_config_yaml(&sample_bundle(), ConfigRole::Peer);
        assert!(yaml.contains("port: any"));
        assert!(yaml.contains("proto: any"));
        assert!(yaml.contains("host: any"));
    }

    #[test]
    fn lighthouse_config_adds_relay_section() {
        let yaml = render_lighthouse_config_yaml(&sample_bundle());
        assert!(yaml.contains("am_relay: true"));
        assert!(yaml.contains("punch: true"));
    }

    // VIRT-4.a (v5.0.0) — VM subnet `unsafe_routes` announcement.

    #[test]
    fn a_lighthouse_node_never_maps_itself() {
        // Bug #3 (decouple decision): a node that IS a bundle lighthouse
        // must render am_lighthouse + must NOT list its own overlay IP in
        // static_host_map (else nebula "refuses to handshake with myself").
        let mut b = sample_bundle();
        // Make THIS node the lighthouse: own overlay IP == the lh entry.
        b.overlay_ip = "10.42.0.1".into();
        let yaml = render_config_yaml(&b, ConfigRole::Host);
        assert!(yaml.contains("am_lighthouse: true"));
        assert!(
            !yaml.contains("lh1.example.com:4242"),
            "a lighthouse must not map itself in static_host_map:\n{yaml}"
        );
    }

    #[test]
    fn a_second_lighthouse_is_still_mapped() {
        // With two lighthouses, a lighthouse maps the OTHER one (relay
        // mesh) but still not itself.
        let mut b = sample_bundle();
        b.overlay_ip = "10.42.0.1".into(); // self = lh1
        b.lighthouses.push(LighthouseEntry {
            node_id: "peer:lh2".into(),
            overlay_ip: "10.42.0.2".into(),
            external_addr: "lh2.example.com:4242".into(),
            relay_tls: None,
        });
        let yaml = render_config_yaml(&b, ConfigRole::Host);
        assert!(!yaml.contains("lh1.example.com:4242"), "self excluded");
        assert!(yaml.contains("lh2.example.com:4242"), "other lh mapped");
    }

    #[test]
    fn every_config_names_the_tun_device_nebula1() {
        // The overlay interface must be `nebula1`, else mackesd's
        // overlay-bound lookups fail (it auto-named `tun0` without this).
        // Found on the VM bed 2026-06-10.
        for role in [ConfigRole::Peer, ConfigRole::Host] {
            let yaml = render_config_yaml(&sample_bundle(), role);
            assert!(
                yaml.contains("tun:") && yaml.contains("dev: nebula1"),
                "config for {role:?} must name the tun device nebula1:\n{yaml}"
            );
        }
    }

    #[test]
    fn render_peer_config_includes_vm_subnet_unsafe_route() {
        let yaml = render_config_yaml(&sample_bundle(), ConfigRole::Peer);
        assert!(
            yaml.contains("unsafe_routes:"),
            "missing unsafe_routes block in:\n{yaml}"
        );
        assert!(
            yaml.contains(VM_SUBNET_CIDR),
            "missing VM subnet CIDR in:\n{yaml}"
        );
        // sample_bundle().overlay_ip == "10.42.0.5" — the `via` is
        // this peer's own overlay IP, not the lighthouse's.
        assert!(
            yaml.contains("via: 10.42.0.5"),
            "missing `via: <local-overlay-ip>` in:\n{yaml}"
        );
    }

    #[test]
    fn render_lighthouse_config_inherits_vm_subnet_unsafe_route() {
        let yaml = render_lighthouse_config_yaml(&sample_bundle());
        assert!(
            yaml.contains(VM_SUBNET_CIDR),
            "lighthouse YAML missing VM subnet route in:\n{yaml}"
        );
        assert!(yaml.contains("via: 10.42.0.5"));
    }

    #[test]
    fn vm_subnet_cidr_is_the_design_locked_value() {
        // Locks the constant against accidental drift — the design
        // doc (v5.0.0-compute.md §4) names this CIDR explicitly.
        assert_eq!(VM_SUBNET_CIDR, "10.42.128.0/17");
    }

    // VIRT-6 — guest VM config inherits lighthouses but OMITS the
    // host-only VM-subnet unsafe_route.

    #[test]
    fn render_guest_config_inherits_lighthouses_but_omits_vm_route() {
        let yaml = render_guest_config_yaml(&sample_bundle());
        // Guest is a normal peer node: lighthouse roster present.
        assert!(yaml.contains("am_lighthouse: false"));
        assert!(
            yaml.contains("\"10.42.0.1\""),
            "guest needs lighthouse roster"
        );
        // But NOT the host-only VM-subnet route.
        assert!(
            !yaml.contains("unsafe_routes"),
            "guest (leaf node) must not carry the VM-subnet unsafe_route:\n{yaml}"
        );
        assert!(!yaml.contains(VM_SUBNET_CIDR));
        // Open-mesh firewall still applies so the VM is reachable.
        assert!(yaml.contains("port: any"));
    }

    #[test]
    fn write_role_marker_creates_parent_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let marker = tmp.path().join("var/lib/mackesd/nebula/role.host");
        write_role_marker(&marker).expect("write");
        assert!(marker.exists());
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "role:host\n");
    }

    #[test]
    fn check_leader_reads_the_given_marker_path() {
        // AUD7-1: leadership is the presence of the *configurable* marker, not
        // the hardcoded production path — so a test marker is honoured.
        let tmp = tempfile::tempdir().expect("tempdir");
        let marker = tmp.path().join("role.host");
        assert!(!check_leader(&marker), "absent marker → not leader");
        write_role_marker(&marker).expect("write");
        assert!(check_leader(&marker), "present marker → leader");
    }

    #[tokio::test]
    async fn worker_name_locks_phase_b_lock() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = tmp.path().join("store.sqlite");
        let conn = crate::store::open(&db).expect("open");
        let s = NebulaSupervisor::new(
            Arc::new(Mutex::new(conn)),
            "peer:test".into(),
            "m1".into(),
            tmp.path().join("nebula-bundle.json"),
        );
        assert_eq!(s.name(), "nebula-supervisor");
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_token() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = tmp.path().join("store.sqlite");
        let conn = crate::store::open(&db).expect("open");
        let mut s = NebulaSupervisor::new(
            Arc::new(Mutex::new(conn)),
            "peer:test".into(),
            "m1".into(),
            tmp.path().join("nebula-bundle.json"),
        )
        .with_role_marker(tmp.path().join("role.host"))
        .with_config_dir(tmp.path().join("nebula"))
        .with_tick_interval(Duration::from_millis(50));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(3), s.run(token))
            .await
            .expect("worker must exit");
        assert!(result.is_ok());
    }

    // HA / Gap-C — an already-enrolled peer (e.g. Eagle) picks up newly-added
    // lighthouses via the supervisor's directory→bundle reconcile, with no
    // re-enroll. Tests use the fs fallback (no etcd endpoints file → fs union).

    fn seed_lighthouse(root: &Path, host: &str, overlay: &str, external: &str) {
        let mut p = mackes_mesh_types::peers::PeerRecord::now(host, None, "healthy");
        p.role = Some(mackes_mesh_types::lighthouse::LIGHTHOUSE_ROLE.to_string());
        p.overlay_ip = Some(overlay.to_string());
        p.external_addr = Some(external.to_string());
        mackes_mesh_types::peers::write_peer_record(&mackes_mesh_types::peers::peers_dir(root), &p)
            .expect("seed lighthouse record");
    }

    #[test]
    fn reconcile_grows_an_enrolled_peers_bundle_to_the_full_roster() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        // The canonical directory carries THREE lighthouses.
        seed_lighthouse(&root, "lh-01", "10.42.0.1", "203.0.113.1:4242");
        seed_lighthouse(&root, "lh-02", "10.42.0.2", "203.0.113.2:4242");
        seed_lighthouse(&root, "lh-03", "10.42.0.3", "203.0.113.3:4242");
        // An EXISTING peer (Eagle-like, overlay .9) whose frozen bundle still
        // lists only the founder — the pre-LIGHTHOUSE-10 single-entry case.
        let bundle_path = root.join("nebula-bundle.json");
        let mut b = sample_bundle();
        b.overlay_ip = "10.42.0.9".into(); // a peer, not a lighthouse
        b.lighthouses = vec![LighthouseEntry {
            node_id: "lh-01".into(),
            overlay_ip: "10.42.0.1".into(),
            external_addr: "203.0.113.1:4242".into(),
            relay_tls: None,
        }];
        crate::ca::bundle::write_bundle(&bundle_path, &b).expect("seed bundle");

        let conn = crate::store::open(&root.join("store.sqlite")).expect("open");
        let s = NebulaSupervisor::new(
            Arc::new(Mutex::new(conn)),
            "peer:eagle".into(),
            "m1".into(),
            bundle_path.clone(),
        )
        .with_workgroup_root(root.clone());
        s.reconcile_lighthouse_roster();

        let after = crate::ca::bundle::read_bundle(&bundle_path).expect("read");
        let mut ids: Vec<_> = after
            .lighthouses
            .iter()
            .map(|l| l.node_id.clone())
            .collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "lh-01".to_string(),
                "lh-02".to_string(),
                "lh-03".to_string()
            ],
            "an enrolled peer's bundle must grow to the full directory roster"
        );
    }

    #[test]
    fn reconcile_never_wipes_the_roster_on_an_empty_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        // No lighthouse records in the directory (transient empty / failed read).
        let bundle_path = root.join("nebula-bundle.json");
        crate::ca::bundle::write_bundle(&bundle_path, &sample_bundle()).expect("seed bundle");

        let conn = crate::store::open(&root.join("store.sqlite")).expect("open");
        let s = NebulaSupervisor::new(
            Arc::new(Mutex::new(conn)),
            "peer:test".into(),
            "m1".into(),
            bundle_path.clone(),
        )
        .with_workgroup_root(root.clone());
        s.reconcile_lighthouse_roster();

        let after = crate::ca::bundle::read_bundle(&bundle_path).expect("read");
        assert_eq!(
            after.lighthouses.len(),
            1,
            "an empty directory read must NOT wipe the existing roster (anti-strand guard)"
        );
    }

    #[test]
    fn write_atomic_does_not_leave_tempfile_on_success() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("config.yaml");
        write_atomic(&path, b"body").expect("write");
        let tmp_path = path.with_extension("yaml.tmp");
        assert!(!tmp_path.exists());
        assert_eq!(
            std::fs::metadata(path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600,
        );
    }

    // GF-1.3.a — overlay-ip publisher.

    #[test]
    fn publish_overlay_ip_creates_parent_dir_and_writes_ip() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("var/lib/mackesd/nebula/overlay-ip");
        publish_overlay_ip(&path, "10.42.0.5").expect("publish");
        assert!(path.exists());
        let body = std::fs::read_to_string(&path).expect("read");
        assert_eq!(body, "10.42.0.5\n");
    }

    #[test]
    fn publish_overlay_ip_overwrites_existing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("overlay-ip");
        publish_overlay_ip(&path, "10.42.0.5").expect("first");
        publish_overlay_ip(&path, "10.42.0.7").expect("second");
        let body = std::fs::read_to_string(&path).expect("read");
        assert_eq!(body, "10.42.0.7\n");
    }

    #[test]
    fn publish_overlay_ip_leaves_no_tempfile_on_success() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("overlay-ip");
        publish_overlay_ip(&path, "10.42.0.5").expect("publish");
        let tmp_path = path.with_extension("ip.tmp");
        assert!(
            !tmp_path.exists(),
            "tempfile {} should have been renamed away",
            tmp_path.display()
        );
    }

    #[test]
    fn publish_overlay_ip_handles_ipv6_format() {
        // The publisher itself doesn't validate IP shape — it's
        // intentionally a pass-through so the supervisor can
        // publish whatever the bundle says without re-parsing.
        // Document the contract via test.
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("overlay-ip");
        publish_overlay_ip(&path, "fd42::5").expect("publish");
        let body = std::fs::read_to_string(&path).expect("read");
        assert_eq!(body, "fd42::5\n");
    }

    #[test]
    fn default_overlay_ip_path_matches_design_doc() {
        assert_eq!(
            DEFAULT_OVERLAY_IP_PATH,
            "/var/lib/mackesd/nebula/overlay-ip"
        );
    }
}
