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

/// A stale or unreachable coordination plane must not starve local Nebula
/// config repair. Leadership reconciliation is advisory for this worker's
/// config-refresh path, so fail closed quickly and retry on the next tick.
const DEFAULT_LEADERSHIP_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

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
    relay_trust_authority_pin_path: PathBuf,
    overlay_ip_path: PathBuf,
    tick_interval: Duration,
    /// Cached bundle mtime so a change triggers a re-write.
    last_bundle_mtime: Option<SystemTime>,
    /// ENT-3 — the replicated root carrying ca/blocklist.
    workgroup_root: PathBuf,
    /// ENT-3 — last-applied blocklist union (change triggers reload).
    last_blocklist: Vec<String>,
    /// Coordination-plane endpoints. Empty means the legacy filesystem lease
    /// is authoritative; non-empty means etcd is authoritative.
    leadership_endpoints: Vec<String>,
    leadership_lookup_timeout: Duration,
    /// Last-known leader state — flipping this triggers the
    /// promote / demote transition.
    last_is_leader: bool,
    /// Systemd command path. Kept configurable for deterministic supervisor
    /// tests; production uses the normal PATH lookup for `systemctl`.
    systemctl_path: PathBuf,
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
            relay_trust_authority_pin_path: PathBuf::from(
                crate::ca::bundle::RELAY_TRUST_AUTHORITY_PIN_PATH,
            ),
            overlay_ip_path: PathBuf::from(DEFAULT_OVERLAY_IP_PATH),
            tick_interval: DEFAULT_TICK_INTERVAL,
            last_bundle_mtime: None,
            last_is_leader: false,
            workgroup_root,
            last_blocklist: Vec::new(),
            leadership_endpoints: crate::substrate::etcd::default_endpoints(),
            leadership_lookup_timeout: DEFAULT_LEADERSHIP_LOOKUP_TIMEOUT,
            systemctl_path: PathBuf::from("systemctl"),
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

    /// Override the root-local relay authority pin — used by tests.
    #[must_use]
    pub(crate) fn with_relay_trust_authority_pin(mut self, path: PathBuf) -> Self {
        self.relay_trust_authority_pin_path = path;
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

    /// Override the coordination-plane authority — used by isolated tests.
    #[must_use]
    pub(crate) fn with_leadership_endpoints(mut self, endpoints: Vec<String>) -> Self {
        self.leadership_endpoints = endpoints;
        self
    }

    /// Override the systemctl executable — used by deterministic runtime
    /// retry tests without requiring a live systemd instance.
    #[must_use]
    pub(crate) fn with_systemctl_path(mut self, path: PathBuf) -> Self {
        self.systemctl_path = path;
        self
    }

    /// One sweep. Pure-ish (touches disk + may shell out
    /// to systemctl, but no network). Returns Ok(()) on
    /// success; logs + swallows individual step failures so
    /// a single bad tick doesn't kill the worker.
    async fn tick(&mut self) {
        // Local config repair is deliberately first. A stale/unreachable
        // coordination plane must not keep an already-enrolled peer from
        // sanitizing a legacy bundle, materializing `/etc/nebula`, or retrying a
        // failed reload.
        if !self.refresh_changed_config().await {
            return;
        }

        // 1. Check the authoritative lease. Promotion owns creation/repair of
        //    the local marker, so requiring that marker here would make a
        //    clean elected lighthouse unable to bootstrap. The mirror puller
        //    below keeps the stricter marker-plus-lease check.
        let is_leader_now = authoritative_lease_names_node(
            &self.workgroup_root,
            &self.node_id,
            &self.leadership_endpoints,
            self.leadership_lookup_timeout,
        )
        .await;
        let marker_is_ours = read_role_marker(&self.role_marker_path)
            .is_some_and(|marker_node_id| marker_node_id == self.node_id);
        let needs_transition = if is_leader_now {
            !self.last_is_leader || !marker_is_ours
        } else {
            self.last_is_leader || self.role_marker_path.exists()
        };
        if needs_transition {
            let transition = if is_leader_now {
                self.promote().await
            } else {
                self.demote()
            };
            match transition {
                Ok(()) => {
                    // Keep the transition pending after any error so a later
                    // tick retries the marker/service change instead of
                    // treating a partially-applied role change as complete.
                    self.last_is_leader = is_leader_now;
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    leader = is_leader_now,
                    "nebula-supervisor: role transition failed; will retry"
                ),
            }
        }

        // 1.5 HA — keep THIS node's bundle lighthouse roster in sync with the
        //     canonical directory so a newly-added lighthouse propagates to an
        //     already-enrolled peer (e.g. Eagle) without a re-enroll. Rewrites the
        //     bundle (bumping its mtime) only on a real change; the mtime watch in
        //     step 2 then re-renders /etc/nebula + reloads nebula.
        self.reconcile_lighthouse_roster();

        // 2. Re-run after roster reconciliation so a rewritten lighthouse set is
        //    applied on the same tick instead of waiting for the next cadence.
        let _ = self.refresh_changed_config().await;
    }

    /// Watch the bundle file + the revocation blocklist for changes
    /// (ENT-3: a revoke anywhere must evict here).
    async fn refresh_changed_config(&mut self) -> bool {
        let blocklist_now = crate::ca::blocklist::all_fingerprints(&self.workgroup_root);
        let blocklist_changed = blocklist_now != self.last_blocklist;
        if let Some(mtime) = bundle_watch_mtime(&self.bundle_path) {
            if self.last_bundle_mtime.map_or(true, |t| t != mtime) || blocklist_changed {
                match self.refresh_config().await {
                    Ok(()) => {
                        // A watch marker is an acknowledgement, not an
                        // observation.  Do not advance it after a failed
                        // refresh: a transiently partial or hostile
                        // bundle must be retried on the next tick instead
                        // of being recorded as applied.
                        self.last_bundle_mtime = Some(mtime);
                        self.last_blocklist = blocklist_now;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "nebula-supervisor: config refresh failed; will retry");
                        return false;
                    }
                }
            }
        }
        true
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
        write_role_marker(&self.role_marker_path, &self.node_id)?;
        // c. Start the systemd units. systemctl invocations
        //    are best-effort — we still proceed if systemctl
        //    is unreachable (containerized test envs).
        let _ = systemctl_start(&self.systemctl_path, "nebula-lighthouse.service");
        let _ = systemctl_start(&self.systemctl_path, "mackes-nebula-https-tunnel.service");
        Ok(())
    }

    /// Leader-demotion: stop lighthouse + tunnel, remove
    /// marker. nebula.service stays up — the local peer
    /// needs its tun device regardless of role.
    fn demote(&self) -> Result<(), String> {
        tracing::info!(node = %self.node_id, "nebula-supervisor: demoting to peer role");
        let _ = systemctl_stop(&self.systemctl_path, "mackes-nebula-https-tunnel.service");
        let _ = systemctl_stop(&self.systemctl_path, "nebula-lighthouse.service");
        if self.role_marker_path.exists() {
            std::fs::remove_file(&self.role_marker_path).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Re-materialize the on-disk Nebula config from the
    /// QNM-Shared bundle + signal the running nebula
    /// process to reload.
    async fn refresh_config(&self) -> Result<(), String> {
        let (bundle, sanitized_legacy_secret_bundle) =
            crate::ca::bundle::read_bundle_sanitizing_legacy_secrets(&self.bundle_path)
                .map_err(|e| e.to_string())?;
        if sanitized_legacy_secret_bundle {
            tracing::warn!(
                path = %self.bundle_path.display(),
                "nebula-supervisor: sanitized legacy replicated bundle by stripping private-key fields before local identity migration"
            );
        }
        if !relay_authority_is_trusted(&bundle, &self.relay_trust_authority_pin_path) {
            return Err(
                "replicated Nebula bundle relay trust authority does not match the local enrollment pin"
                    .into(),
            );
        }
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
            // Refreshes come from replicated public state. Keep the
            // fingerprint-pinned enrollment identity already activated on
            // this host; a placeholder/requester key here would overwrite
            // the live Nebula key and make the service fail to reload.
            None,
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
        // The on-disk render is not effective until Nebula accepts it.  Keep
        // the bundle watch unacknowledged when the reload/reconnect fails so
        // the next sweep retries the same rotated or revoked state.
        systemctl_reload(&self.systemctl_path, "nebula.service")
            .map_err(|e| format!("reload nebula.service: {e}"))?;
        if self.last_is_leader {
            systemctl_reload(&self.systemctl_path, "nebula-lighthouse.service")
                .map_err(|e| format!("reload nebula-lighthouse.service: {e}"))?;
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
        if !relay_authority_is_trusted(&bundle, &self.relay_trust_authority_pin_path) {
            tracing::warn!(
                "nebula-supervisor: refusing lighthouse roster reconcile with an unpinned relay trust authority"
            );
            return;
        }
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
        roster = normalize_lighthouse_roster(roster, authority.as_deref());
        if roster.is_empty() {
            // Same anti-strand guard as above: invalid/duplicate-only input
            // from a transient directory read cannot wipe a usable roster.
            return;
        }
        // Compare normalized sets so render-order differences alone don't
        // trigger a rewrite/reload every tick, but still rewrite when the
        // stored bundle carries duplicate overlay-IP claims.
        let current = normalize_lighthouse_roster(bundle.lighthouses.clone(), authority.as_deref());
        if current == roster && bundle.lighthouses == current {
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

/// Replicated bundles may carry the public relay authority, but that value is
/// usable only when it matches the root-local pin created during authenticated
/// enrollment. A relay identity without an authority is likewise invalid: it
/// has no trust context and must not survive a supervisor refresh.
fn relay_authority_is_trusted(
    bundle: &crate::ca::bundle::NebulaBundle,
    authority_pin_path: &Path,
) -> bool {
    match bundle.relay_trust_authority.as_deref() {
        Some(_) => crate::ca::bundle::relay_trust_authority_matches_pin(bundle, authority_pin_path),
        None => bundle
            .lighthouses
            .iter()
            .all(|entry| entry.relay_tls.is_none()),
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
/// gets the lighthouse-roster client section and uses relays, but does not
/// advertise itself as a relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigRole {
    /// Lighthouse-eligible role — config file carries the
    /// full lighthouse listener section.
    Host,
    /// Mesh-peer role — config file carries the lighthouse-roster client
    /// section, relay usage, and punchy, but no lighthouse listener.
    Peer,
}

/// Create or validate a directory path without ever resolving a symlinked or
/// non-directory component.  The sealed writer performs the same check for
/// each file write, but identity installation must validate the roots before
/// it creates a generation directory: otherwise `create_dir_all` could first
/// follow an attacker-controlled `config` or `identity` link.
fn ensure_directory_tree(path: &Path, mode: u32, label: &str) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use std::path::Component;

    if path.as_os_str().is_empty() {
        return Err(format!("{label} path is empty"));
    }

    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(format!(
                "refusing {label} path with parent component {}",
                path.display()
            ));
        }
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "refusing symlinked {label} directory component {}",
                    current.display()
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!(
                    "{label} path component is not a directory: {}",
                    current.display()
                ));
            }
            Ok(metadata) => {
                // The identity root contains the requester private key. A
                // pre-existing root must be held to the same owner/mode
                // contract as one created by this process; otherwise a
                // group/world-writable directory can alter the next staged
                // generation before the atomic identity switch.
                if mode == 0o700
                    && current == path
                    && (metadata.uid() != rustix::process::getuid().as_raw()
                        || metadata.permissions().mode() & 0o777 != 0o700)
                {
                    return Err(format!(
                        "unsafe {label} directory component {}: owner/mode must be current uid and 0700",
                        current.display()
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut builder = std::fs::DirBuilder::new();
                builder.mode(mode);
                builder.create(&current).map_err(|error| {
                    format!("create {label} directory {}: {error}", current.display())
                })?;
                let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
                    format!("inspect {label} directory {}: {error}", current.display())
                })?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(format!(
                        "created {label} path component is not a directory: {}",
                        current.display()
                    ));
                }
                if mode == 0o700
                    && current == path
                    && (metadata.uid() != rustix::process::getuid().as_raw()
                        || metadata.permissions().mode() & 0o777 != 0o700)
                {
                    return Err(format!(
                        "unsafe {label} directory component {}: owner/mode must be current uid and 0700",
                        current.display()
                    ));
                }
            }
            Err(error) => {
                return Err(format!(
                    "inspect {label} directory component {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

fn install_identity_generation(
    config_dir: &Path,
    bundle: &crate::ca::bundle::NebulaBundle,
    workgroup_root: &Path,
    local_node_id: Option<&str>,
    node_key_path: &Path,
    requester_private_key: Option<&[u8]>,
    fingerprint_cert: fn(&str) -> Option<String>,
) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;

    let identity_dir = config_dir.join("identity");
    ensure_directory_tree(&identity_dir, 0o700, "Nebula identity")?;

    // Replicated steady-state refreshes may update topology/config, but cannot
    // replace an already-active identity. Only the fingerprint-pinned network
    // enrollment path supplies `requester_private_key` and is authorized to
    // activate a new cert/key generation.
    let current_cert = identity_dir.join("current/host.crt");
    if requester_private_key.is_none() && current_cert.exists() {
        let active_cert = crate::ca::seal::read_no_follow(&current_cert)
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
        if let Some(active_generation) = active_identity_generation_leaf(&identity_dir)? {
            prune_stale_identity_generations(&identity_dir, &active_generation);
        }
        return Ok(());
    }

    let previous_generation = if requester_private_key.is_some() {
        active_identity_generation_leaf(&identity_dir)?
    } else {
        None
    };
    let superseded_cert = if requester_private_key.is_some() && current_cert.exists() {
        let active_cert = crate::ca::seal::read_no_follow(&current_cert)
            .map_err(|e| format!("read active identity cert {}: {e}", current_cert.display()))?;
        if active_cert == bundle.peer_cert_pem.as_bytes() {
            None
        } else {
            Some(active_cert)
        }
    } else {
        None
    };
    if superseded_cert.is_some() && local_node_id.is_none() {
        return Err(
            "authenticated Nebula identity rotation needs the local node-id to blocklist the superseded cert"
                .into(),
        );
    }

    let owned_key;
    let key_bytes = if let Some(key) = requester_private_key {
        key
    } else {
        let current_key = identity_dir.join("current/host.key");
        let current_cert = identity_dir.join("current/host.crt");
        let legacy_cert = config_dir.join("host.crt");
        let legacy_key = config_dir.join("host.key");
        if !current_cert.exists() && (legacy_cert.exists() || legacy_key.exists()) {
            let legacy_cert_bytes = crate::ca::seal::read_no_follow(&legacy_cert).map_err(|e| {
                format!(
                    "read legacy local Nebula cert {}: {e}",
                    legacy_cert.display()
                )
            })?;
            if legacy_cert_bytes != bundle.peer_cert_pem.as_bytes() {
                return Err(
                    "legacy local Nebula cert does not match replicated bundle; authenticated enrollment is required"
                        .into(),
                );
            }
        }
        let key_path = if current_key.exists() {
            current_key.as_path()
        } else {
            tighten_legacy_private_key_permissions(&legacy_key)?;
            legacy_key.as_path()
        };
        owned_key = crate::ca::seal::read_sealed(key_path)
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
        .ok_or_else(|| "identity generation has no filename".to_string())?
        .to_os_string();
    if let Err(error) = activate_identity_generation(&identity_dir, &generation_leaf) {
        let _ = std::fs::remove_dir_all(&generation_dir);
        return Err(error);
    }

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
    if let (Some(node_id), Some(old_cert)) = (local_node_id, superseded_cert.as_deref()) {
        if let Err(error) = record_superseded_identity_blocklist(
            workgroup_root,
            node_id,
            old_cert,
            node_key_path,
            fingerprint_cert,
        ) {
            let rollback_error = previous_generation.as_deref().and_then(|generation| {
                activate_identity_generation(&identity_dir, generation).err()
            });
            let _ = std::fs::remove_dir_all(&generation_dir);
            return Err(match rollback_error {
                Some(rollback) => {
                    format!("{error}; rollback to prior identity generation failed: {rollback}")
                }
                None => error,
            });
        }
    }
    // A rotation leaves the previous generation holding the old private key.
    // Once the atomic switch and compatibility links are installed, only the
    // active generation is useful; retaining every historical key would make
    // local identity storage grow without bound.
    prune_stale_identity_generations(&identity_dir, &generation_leaf);
    Ok(())
}

fn activate_identity_generation(
    identity_dir: &Path,
    generation_leaf: &std::ffi::OsStr,
) -> Result<(), String> {
    use std::os::unix::fs::symlink;

    let temp_link = identity_dir.join(format!(
        ".current.tmp.{}.{:016x}",
        std::process::id(),
        rand::random::<u64>()
    ));
    symlink(generation_leaf, &temp_link)
        .map_err(|e| format!("create identity switch {}: {e}", temp_link.display()))?;
    if let Err(error) = std::fs::rename(&temp_link, identity_dir.join("current")) {
        let _ = std::fs::remove_file(&temp_link);
        return Err(format!("activate identity generation: {error}"));
    }
    std::fs::File::open(identity_dir)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| format!("fsync identity dir {}: {e}", identity_dir.display()))
}

fn record_superseded_identity_blocklist(
    workgroup_root: &Path,
    node_id: &str,
    cert_bytes: &[u8],
    node_key_path: &Path,
    fingerprint_cert: fn(&str) -> Option<String>,
) -> Result<(), String> {
    validate_blocklist_node_id(node_id)?;
    let cert_pem = std::str::from_utf8(cert_bytes)
        .map_err(|e| format!("superseded Nebula identity cert is not UTF-8: {e}"))?;
    let fingerprint = fingerprint_cert(cert_pem).ok_or_else(|| {
        "could not fingerprint superseded Nebula identity cert; refusing to rotate without a blocklist retract"
            .to_string()
    })?;
    if fingerprint.len() != 64 || !fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "superseded Nebula identity fingerprint has invalid shape: {fingerprint:?}"
        ));
    }
    let fingerprints = vec![fingerprint];
    let written = crate::node_key::load_or_create(node_key_path).map_or_else(
        |_| crate::ca::blocklist::record_revoked(workgroup_root, node_id, &fingerprints),
        |key| {
            crate::ca::blocklist::record_revoked_signed(
                workgroup_root,
                node_id,
                &fingerprints,
                node_id,
                &key,
            )
        },
    );
    written
        .map(|_| ())
        .map_err(|e| format!("record superseded Nebula identity blocklist: {e}"))
}

fn validate_blocklist_node_id(node_id: &str) -> Result<(), String> {
    if !node_id.is_empty()
        && node_id.len() <= 255
        && node_id.trim() == node_id
        && node_id != "."
        && node_id != ".."
        && !node_id
            .chars()
            .any(|character| character == '/' || character == '\\' || character.is_control())
    {
        Ok(())
    } else {
        Err("invalid local node-id for superseded Nebula identity blocklist".into())
    }
}

fn tighten_legacy_private_key_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    match crate::ca::seal::read_sealed(path) {
        Ok(_) => return Ok(()),
        Err(crate::ca::CaError::InsecurePermissions { .. }) => {}
        Err(error) => {
            return Err(format!(
                "legacy local Nebula key is not readable through the sealed-file boundary: {error}"
            ));
        }
    }

    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("inspect legacy local Nebula key {}: {e}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "legacy local Nebula key is not a regular file: {}",
            path.display()
        ));
    }
    if metadata.uid() != rustix::process::getuid().as_raw() {
        return Err(format!(
            "legacy local Nebula key is not owned by the current verifier uid: {}",
            path.display()
        ));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if (mode & !0o644) != 0 || (mode & 0o600) != 0o600 {
        return Err(format!(
            "legacy local Nebula key has unsafe permissions {mode:#o}; only owner read/write plus stale group/other read bits can be auto-tightened"
        ));
    }
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(path, permissions)
        .map_err(|e| format!("tighten legacy local Nebula key {}: {e}", path.display()))?;
    Ok(())
}

fn active_identity_generation_leaf(
    identity_dir: &Path,
) -> Result<Option<std::ffi::OsString>, String> {
    use std::path::Component;

    let current = identity_dir.join("current");
    let target = match std::fs::read_link(&current) {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "read active Nebula identity switch {}: {error}",
                current.display()
            ));
        }
    };
    let mut components = target.components();
    let Some(Component::Normal(name)) = components.next() else {
        return Err(format!(
            "refusing active Nebula identity switch {} -> {}",
            current.display(),
            target.display()
        ));
    };
    if components.next().is_some() || !is_identity_generation_name(name) {
        return Err(format!(
            "refusing active Nebula identity switch {} -> {}",
            current.display(),
            target.display()
        ));
    }
    Ok(Some(name.to_os_string()))
}

fn prune_stale_identity_generations(identity_dir: &Path, active_generation: &std::ffi::OsStr) {
    if let Err(error) = prune_identity_generations(identity_dir, active_generation) {
        tracing::warn!(
            error = %error,
            path = %identity_dir.display(),
            "nebula-supervisor: stale identity generation cleanup failed"
        );
    }
}

fn is_identity_generation_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(suffix) = name.strip_prefix("generation-") else {
        return false;
    };
    let Some((pid, nonce)) = suffix.split_once('-') else {
        return false;
    };
    !pid.is_empty()
        && pid.bytes().all(|byte| byte.is_ascii_digit())
        && nonce.len() == 16
        && nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Remove only supervisor-created, owner-controlled generation directories
/// other than the active one. Unexpected names, symlinks, and unsafe metadata
/// are left untouched rather than turning cleanup into a destructive sweep.
fn prune_identity_generations(
    identity_dir: &Path,
    active_generation: &std::ffi::OsStr,
) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let entries = std::fs::read_dir(identity_dir).map_err(|error| {
        format!(
            "scan identity generations {}: {error}",
            identity_dir.display()
        )
    })?;
    let mut first_error = None;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                first_error.get_or_insert_with(|| {
                    format!(
                        "read identity generation entry {}: {error}",
                        identity_dir.display()
                    )
                });
                continue;
            }
        };
        let name = entry.file_name();
        if name.as_os_str() == active_generation || !is_identity_generation_name(name.as_os_str()) {
            continue;
        }
        let path = entry.path();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                first_error.get_or_insert_with(|| {
                    format!(
                        "inspect stale identity generation {}: {error}",
                        path.display()
                    )
                });
                continue;
            }
        };
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || metadata.uid() != rustix::process::getuid().as_raw()
            || metadata.permissions().mode() & 0o777 != 0o700
        {
            continue;
        }
        if let Err(error) = std::fs::remove_dir_all(&path) {
            first_error.get_or_insert_with(|| {
                format!(
                    "remove stale identity generation {}: {error}",
                    path.display()
                )
            });
        }
    }
    first_error.map_or(Ok(()), Err)
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
    materialize_config_inner(
        config_dir,
        bundle,
        role,
        blocklist,
        workgroup_root,
        None,
        requester_private_key,
        Path::new(crate::node_key::DEFAULT_KEY_PATH),
        crate::ca::blocklist::fingerprint_cert_pem,
    )
}

/// Node-aware variant of [`materialize_config`] for authenticated enrollment.
///
/// When the fingerprint-pinned enrollment path supplies a requester private key
/// and replaces an already-active local cert, this function records the
/// superseded cert fingerprint in the replicated Nebula blocklist before stale
/// key generations are pruned. Replicated steady-state refreshes should keep
/// using [`materialize_config`]: they are not authorized to rotate identity.
pub fn materialize_config_for_node(
    config_dir: &Path,
    bundle: &crate::ca::bundle::NebulaBundle,
    role: ConfigRole,
    blocklist: &[String],
    workgroup_root: &Path,
    local_node_id: &str,
    requester_private_key: Option<&[u8]>,
) -> Result<(), String> {
    materialize_config_inner(
        config_dir,
        bundle,
        role,
        blocklist,
        workgroup_root,
        Some(local_node_id),
        requester_private_key,
        Path::new(crate::node_key::DEFAULT_KEY_PATH),
        crate::ca::blocklist::fingerprint_cert_pem,
    )
}

#[allow(clippy::too_many_arguments)]
fn materialize_config_inner(
    config_dir: &Path,
    bundle: &crate::ca::bundle::NebulaBundle,
    role: ConfigRole,
    blocklist: &[String],
    workgroup_root: &Path,
    local_node_id: Option<&str>,
    requester_private_key: Option<&[u8]>,
    node_key_path: &Path,
    fingerprint_cert: fn(&str) -> Option<String>,
) -> Result<(), String> {
    ensure_directory_tree(config_dir, 0o777, "Nebula config")?;
    // Validate the identity root before writing even the public CA file.  The
    // identity installer repeats this check immediately before generation
    // staging, while the sealed writer checks every secret-bearing leaf.
    ensure_directory_tree(&config_dir.join("identity"), 0o700, "Nebula identity")?;

    write_atomic(&config_dir.join("ca.crt"), bundle.ca_cert_pem.as_bytes())?;
    install_identity_generation(
        config_dir,
        bundle,
        workgroup_root,
        local_node_id,
        node_key_path,
        requester_private_key,
        fingerprint_cert,
    )?;
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

fn relay_identity_verified(
    authority: Option<&str>,
    entry: &crate::ca::bundle::LighthouseEntry,
) -> bool {
    authority.is_some_and(|public_key| {
        entry.relay_tls.as_ref().is_some_and(|identity| {
            crate::ca::bundle::verify_relay_tls_identity(
                identity,
                &entry.node_id,
                &entry.overlay_ip,
                &entry.external_addr,
                public_key,
            )
        })
    })
}

fn lighthouse_entry_wins_overlay_tie(
    candidate: &crate::ca::bundle::LighthouseEntry,
    incumbent: &crate::ca::bundle::LighthouseEntry,
    authority: Option<&str>,
) -> bool {
    let candidate_verified = relay_identity_verified(authority, candidate);
    let incumbent_verified = relay_identity_verified(authority, incumbent);
    if candidate_verified != incumbent_verified {
        return candidate_verified;
    }
    let candidate_numeric = external_addr_host_is_numeric(&candidate.external_addr);
    let incumbent_numeric = external_addr_host_is_numeric(&incumbent.external_addr);
    if candidate_numeric != incumbent_numeric {
        return candidate_numeric;
    }
    (&candidate.node_id, &candidate.external_addr) < (&incumbent.node_id, &incumbent.external_addr)
}

fn normalize_lighthouse_roster(
    entries: Vec<crate::ca::bundle::LighthouseEntry>,
    authority: Option<&str>,
) -> Vec<crate::ca::bundle::LighthouseEntry> {
    let mut by_overlay =
        std::collections::BTreeMap::<String, crate::ca::bundle::LighthouseEntry>::new();
    for entry in entries {
        if entry.overlay_ip.trim().is_empty() || entry.external_addr.trim().is_empty() {
            continue;
        }
        match by_overlay.get_mut(&entry.overlay_ip) {
            Some(incumbent) if lighthouse_entry_wins_overlay_tie(&entry, incumbent, authority) => {
                *incumbent = entry;
            }
            Some(_) => {}
            None => {
                by_overlay.insert(entry.overlay_ip.clone(), entry);
            }
        }
    }
    let mut entries: Vec<_> = by_overlay.into_values().collect();
    entries.sort_by(|a, b| a.node_id.cmp(&b.node_id));
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
    // A workstation-sized root volume cannot safely retain Nebula's default
    // info-level handshake chatter. In particular, a stale peer certificate
    // can otherwise emit a line for every retry and starve the node of disk.
    // Keep warnings/errors for diagnosis while leaving the generated config
    // self-contained across supervisor refreshes.
    out.push_str("logging:\n");
    out.push_str("  level: warn\n");
    out.push_str("  format: text\n");
    out.push_str("  disable_timestamp: false\n\n");
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
    // Enable the built-in relay and punchy paths on every node. A peer does not
    // relay for others, but it must be willing to use lighthouse relays and
    // respond to punchy probes. Without this, same-NAT / hairpin paths can strand
    // a freshly-enrolled seat even while every node can still reach a lighthouse.
    out.push_str("relay:\n");
    match role {
        ConfigRole::Host => out.push_str("  am_relay: true\n"),
        ConfigRole::Peer => out.push_str("  am_relay: false\n"),
    }
    out.push_str("  use_relays: true\n");
    out.push_str("punchy:\n");
    out.push_str("  punch: true\n");
    out.push_str("  respond: true\n\n");
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
    render_config_yaml_with_routes(bundle, ConfigRole::Host, &[], extra_routes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    crate::ca::seal::write_atomic_sealed(path, bytes).map_err(|e| e.to_string())
}

/// GF-1.3.a — atomic-write the plain-text overlay IP file.
/// Creates parent dirs if missing. Idempotent: a re-write of
/// the same IP still bumps mtime, but the bytes match so
/// downstream mtime-gate consumers can use a byte-compare to
/// skip the reload step. Use the sealed writer even though this
/// is not secret material: it rejects symlinked parent components
/// and uses a unique `create_new` staging file, so a hostile
/// replacement cannot redirect the publish outside its intended tree.
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
    let body = format!("{overlay_ip}\n");
    write_atomic(path, body.as_bytes())
}

/// Marker schema version. This is intentionally local and owner-bound: the
/// marker is not a replicated authorization document.
const ROLE_MARKER_SCHEMA: &str = "mackesd-role-host-v1";

pub(crate) fn write_role_marker(path: &Path, node_id: &str) -> Result<(), String> {
    if node_id.is_empty() || node_id.bytes().any(|b| matches!(b, b'\n' | b'\r' | b'\t')) {
        return Err(format!("invalid role-marker node id {node_id:?}"));
    }
    let body = format!("schema:{ROLE_MARKER_SCHEMA}\nrole:host\nnode_id:{node_id}\n");
    crate::ca::seal::write_atomic_sealed(path, body.as_bytes()).map_err(|e| e.to_string())
}

/// Read and validate the local leadership marker. `read_sealed` rejects
/// symlink leaves/parents, non-regular files, group/world permissions, and a
/// different owner, so marker existence alone is never treated as authority.
pub(crate) fn read_role_marker(path: &Path) -> Option<String> {
    let bytes = crate::ca::seal::read_sealed(path).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    let mut lines = text.lines();
    if lines.next()? != format!("schema:{ROLE_MARKER_SCHEMA}") || lines.next()? != "role:host" {
        return None;
    }
    let node_id = lines.next()?.strip_prefix("node_id:")?;
    if node_id.is_empty()
        || lines.next().is_some()
        || node_id.bytes().any(|b| matches!(b, b'\n' | b'\r' | b'\t'))
    {
        return None;
    }
    Some(node_id.to_owned())
}

/// Lightweight `systemctl <verb> <unit>` invocation. Returns
/// Ok(()) on success or Err(stderr) on failure. Missing
/// systemctl is reported as an error; promotion/demotion may
/// tolerate it, while config refresh keeps its watch pending
/// so a later sweep can retry the reconnect.
fn systemctl(path: &Path, verb: &str, unit: &str) -> Result<(), String> {
    let out = std::process::Command::new(path)
        .args([verb, unit])
        .output()
        .map_err(|e| format!("{} {verb} {unit}: {e}", path.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if stderr.is_empty() {
            Err(format!(
                "{} {verb} {unit} exited with {}",
                path.display(),
                out.status
            ))
        } else {
            Err(stderr)
        }
    }
}

fn systemctl_start(path: &Path, unit: &str) -> Result<(), String> {
    systemctl(path, "start", unit)
}

fn systemctl_stop(path: &Path, unit: &str) -> Result<(), String> {
    systemctl(path, "stop", unit)
}

fn systemctl_reload(path: &Path, unit: &str) -> Result<(), String> {
    systemctl(path, "reload-or-restart", unit)
}

/// Observe a bundle only when its final path component is a regular, non-link
/// file. `read_bundle` consumes through a no-follow descriptor, but using
/// `metadata` here would follow a hostile final symlink and make the watcher
/// schedule a read of an input it did not authorize. Returning `None` leaves
/// the acknowledgement state unchanged, so a later replacement is retried.
fn bundle_watch_mtime(path: &Path) -> Option<SystemTime> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return None;
    }
    metadata.modified().ok()
}

/// Verify both halves of the local leadership contract:
///
/// 1. the marker is an owner-checked, exact-schema file naming this node; and
/// 2. the current non-expired filesystem/etcd lease names the same node.
///
/// Any missing, malformed, replaced, stale, or unavailable authority returns
/// `false`. This function is shared by the supervisor and mirror puller so the
/// latter cannot accidentally turn marker existence into leadership.
pub(crate) async fn role_marker_is_current_leader(
    marker_path: &Path,
    workgroup_root: &Path,
    expected_node_id: &str,
    leadership_endpoints: &[String],
) -> bool {
    let Some(marker_node_id) = read_role_marker(marker_path) else {
        return false;
    };
    if marker_node_id != expected_node_id {
        return false;
    }
    authoritative_lease_names_node(
        workgroup_root,
        expected_node_id,
        leadership_endpoints,
        DEFAULT_LEADERSHIP_LOOKUP_TIMEOUT,
    )
    .await
}

/// Read only the authoritative coordination-plane lease. This is intentionally
/// separate from [`role_marker_is_current_leader`]: the supervisor must be able
/// to create a missing marker during promotion, while privileged mirror pulls
/// must require both the marker and the lease.
async fn authoritative_lease_names_node(
    workgroup_root: &Path,
    expected_node_id: &str,
    leadership_endpoints: &[String],
    lookup_timeout: Duration,
) -> bool {
    let lookup = async {
        if leadership_endpoints.is_empty() {
            crate::leader::read_current_lease(&workgroup_root.join(".mackesd-leader.lock"))
        } else {
            let Ok(mut client) = crate::substrate::etcd::connect(leadership_endpoints).await else {
                return None;
            };
            crate::substrate::leader::current_leader(&mut client)
                .await
                .ok()
                .flatten()
        }
    };
    let Ok(lease) = tokio::time::timeout(lookup_timeout, lookup).await else {
        tracing::debug!(
            timeout_ms = lookup_timeout.as_millis(),
            endpoints = leadership_endpoints.len(),
            "nebula-supervisor: leadership lookup timed out; retrying next tick"
        );
        return false;
    };
    lease.is_some_and(|lease| lease.node_id == expected_node_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::bundle::{LighthouseEntry, NebulaBundle};

    const NODE_ID: &str = "peer:self";
    const FP_CERT_GENERATION_A: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const FP_PEER_PEM: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn fake_identity_fingerprint(cert_pem: &str) -> Option<String> {
        match cert_pem {
            "cert-generation-a" => Some(FP_CERT_GENERATION_A.to_string()),
            "peer-pem" => Some(FP_PEER_PEM.to_string()),
            _ => None,
        }
    }

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

    fn materialize_authenticated_test_identity(
        config_dir: &Path,
        bundle: &NebulaBundle,
        workgroup_root: &Path,
        key: &[u8],
    ) -> Result<(), String> {
        materialize_config_inner(
            config_dir,
            bundle,
            ConfigRole::Peer,
            &[],
            workgroup_root,
            Some(NODE_ID),
            Some(key),
            &workgroup_root.join("node-signing.key"),
            fake_identity_fingerprint,
        )
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

    #[cfg(unix)]
    #[test]
    fn materialize_rejects_symlinked_config_root_before_secret_write() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("outside");
        let config_dir = tmp.path().join("nebula");
        symlink(outside.path(), &config_dir).expect("config symlink");

        let error = materialize_config(
            &config_dir,
            &sample_bundle(),
            ConfigRole::Peer,
            &[],
            tmp.path(),
            Some(b"must-not-escape"),
        )
        .expect_err("a symlinked config root must fail closed");
        assert!(error.contains("symlinked Nebula config directory component"));
        assert!(
            std::fs::read_dir(outside.path())
                .expect("outside directory")
                .next()
                .is_none(),
            "no secret-bearing file or generation may be created through config symlink"
        );
    }

    #[cfg(unix)]
    #[test]
    fn materialize_rejects_non_directory_config_component_before_secret_write() {
        let tmp = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("outside");
        let component = tmp.path().join("config-component");
        std::fs::write(&component, b"not a directory").expect("component file");
        let config_dir = component.join("nebula");

        let error = materialize_config(
            &config_dir,
            &sample_bundle(),
            ConfigRole::Peer,
            &[],
            tmp.path(),
            Some(b"must-not-escape"),
        )
        .expect_err("a non-directory config component must fail closed");
        assert!(error.contains("Nebula config path component is not a directory"));
        assert!(
            std::fs::read_dir(outside.path())
                .expect("outside directory")
                .next()
                .is_none(),
            "no secret-bearing file may be created after a non-directory component"
        );
    }

    #[cfg(unix)]
    #[test]
    fn materialize_rejects_symlinked_identity_root_before_secret_write() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("outside");
        let config_dir = tmp.path().join("nebula");
        std::fs::create_dir(&config_dir).expect("config directory");
        symlink(outside.path(), config_dir.join("identity")).expect("identity symlink");

        let error = materialize_config(
            &config_dir,
            &sample_bundle(),
            ConfigRole::Peer,
            &[],
            tmp.path(),
            Some(b"must-not-escape"),
        )
        .expect_err("a symlinked identity root must fail closed");
        assert!(error.contains("symlinked Nebula identity directory component"));
        assert!(
            std::fs::read_dir(outside.path())
                .expect("outside directory")
                .next()
                .is_none(),
            "no secret-bearing file or generation may be created through identity symlink"
        );
        assert!(
            !config_dir.join("host.key").exists(),
            "compatibility key link must not be published after identity validation fails"
        );
    }

    #[test]
    fn materialize_rejects_non_directory_identity_root_before_secret_write() {
        let tmp = tempfile::tempdir().expect("tmp");
        let config_dir = tmp.path().join("nebula");
        std::fs::create_dir(&config_dir).expect("config directory");
        std::fs::write(config_dir.join("identity"), b"not a directory").expect("identity file");

        let error = materialize_config(
            &config_dir,
            &sample_bundle(),
            ConfigRole::Peer,
            &[],
            tmp.path(),
            Some(b"must-not-write"),
        )
        .expect_err("a non-directory identity root must fail closed");
        assert!(error.contains("Nebula identity path component is not a directory"));
        assert!(
            !config_dir.join("host.key").exists(),
            "no compatibility key may be published after identity validation fails"
        );
    }

    #[cfg(unix)]
    #[test]
    fn materialize_rejects_an_unsafe_existing_identity_root_before_secret_write() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tmp");
        let config_dir = tmp.path().join("nebula");
        let identity_dir = config_dir.join("identity");
        std::fs::create_dir_all(&identity_dir).expect("identity directory");
        std::fs::set_permissions(&identity_dir, std::fs::Permissions::from_mode(0o777))
            .expect("weaken identity root");

        let error = materialize_config(
            &config_dir,
            &sample_bundle(),
            ConfigRole::Peer,
            &[],
            tmp.path(),
            Some(b"must-not-write"),
        )
        .expect_err("an unsafe existing identity root must fail closed");
        assert!(error.contains("unsafe Nebula identity directory"));
        assert!(!config_dir.join("ca.crt").exists());
        assert!(std::fs::read_dir(&identity_dir)
            .expect("identity directory")
            .next()
            .is_none());
    }

    #[test]
    fn authenticated_identity_rotation_switches_cert_and_key_as_one_generation() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = sample_bundle();
        materialize_authenticated_test_identity(
            tmp.path(),
            &first,
            tmp.path(),
            b"key-generation-a",
        )
        .expect("first identity");
        let mut second = first.clone();
        second.peer_cert_pem = "cert-generation-b".into();
        materialize_authenticated_test_identity(
            tmp.path(),
            &second,
            tmp.path(),
            b"key-generation-b",
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
        assert_eq!(
            crate::ca::blocklist::all_fingerprints(tmp.path()),
            vec![FP_PEER_PEM],
            "the superseded cert must be revoked in the replicated Nebula blocklist"
        );
    }

    #[test]
    fn authenticated_identity_rotation_prunes_previous_generation() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = sample_bundle();
        materialize_authenticated_test_identity(
            tmp.path(),
            &first,
            tmp.path(),
            b"key-generation-a",
        )
        .expect("first identity");
        let first_generation = std::fs::read_link(tmp.path().join("identity/current"))
            .expect("first identity switch")
            .file_name()
            .expect("first generation name")
            .to_os_string();

        let mut second = first;
        second.peer_cert_pem = "cert-generation-b".into();
        materialize_authenticated_test_identity(
            tmp.path(),
            &second,
            tmp.path(),
            b"key-generation-b",
        )
        .expect("authenticated rotation");
        let second_generation = std::fs::read_link(tmp.path().join("identity/current"))
            .expect("second identity switch")
            .file_name()
            .expect("second generation name")
            .to_os_string();

        assert_ne!(first_generation, second_generation);
        let generations: Vec<_> = std::fs::read_dir(tmp.path().join("identity"))
            .expect("identity directory")
            .map(|entry| entry.expect("identity entry").file_name())
            .filter(|name| is_identity_generation_name(name.as_os_str()))
            .collect();
        assert_eq!(
            generations,
            vec![second_generation],
            "rotation must not retain prior private-key generations"
        );
        assert!(
            !tmp.path().join("identity").join(first_generation).exists(),
            "the previous generation must be removed after the atomic switch"
        );
    }

    #[test]
    fn replicated_refresh_retries_stale_identity_generation_prune() {
        use std::os::unix::fs::DirBuilderExt as _;

        let tmp = tempfile::tempdir().expect("tempdir");
        let first = sample_bundle();
        materialize_authenticated_test_identity(
            tmp.path(),
            &first,
            tmp.path(),
            b"key-generation-a",
        )
        .expect("first identity");

        let mut second = first;
        second.peer_cert_pem = "cert-generation-b".into();
        materialize_authenticated_test_identity(
            tmp.path(),
            &second,
            tmp.path(),
            b"key-generation-b",
        )
        .expect("authenticated rotation");
        let active_generation = std::fs::read_link(tmp.path().join("identity/current"))
            .expect("active identity switch")
            .file_name()
            .expect("active generation name")
            .to_os_string();

        let stale_generation = tmp
            .path()
            .join("identity")
            .join("generation-999999999-0123456789abcdef");
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(&stale_generation)
            .expect("seed stale generation");
        crate::ca::seal::write_atomic_sealed(&stale_generation.join("host.key"), b"stale-key")
            .expect("seed stale private key");

        materialize_config(tmp.path(), &second, ConfigRole::Peer, &[], tmp.path(), None)
            .expect("replicated refresh");

        assert!(
            !stale_generation.exists(),
            "a later steady-state refresh must retry stale private-key generation cleanup"
        );
        assert!(
            tmp.path()
                .join("identity")
                .join(&active_generation)
                .exists(),
            "the active generation must remain installed"
        );
        assert_eq!(
            crate::ca::seal::read_sealed(&tmp.path().join("identity/current/host.key")).unwrap(),
            b"key-generation-b",
            "replicated refresh must not replace local private-key material"
        );
    }

    #[test]
    fn replicated_refresh_migrates_matching_legacy_flat_identity() {
        use std::os::unix::fs::PermissionsExt as _;

        let tmp = tempfile::tempdir().expect("tempdir");
        let bundle = sample_bundle();
        std::fs::write(tmp.path().join("host.crt"), bundle.peer_cert_pem.as_bytes())
            .expect("legacy cert");
        std::fs::write(tmp.path().join("host.key"), b"legacy-local-key").expect("legacy key");
        std::fs::set_permissions(
            tmp.path().join("host.key"),
            std::fs::Permissions::from_mode(0o644),
        )
        .expect("legacy key mode");

        materialize_config(tmp.path(), &bundle, ConfigRole::Peer, &[], tmp.path(), None)
            .expect("legacy flat identity migration");

        assert_eq!(
            std::fs::read(tmp.path().join("identity/current/host.crt")).unwrap(),
            bundle.peer_cert_pem.as_bytes()
        );
        assert_eq!(
            crate::ca::seal::read_sealed(&tmp.path().join("identity/current/host.key")).unwrap(),
            b"legacy-local-key"
        );
        assert_eq!(
            std::fs::read_link(tmp.path().join("host.crt")).unwrap(),
            std::path::PathBuf::from("identity/current/host.crt")
        );
        assert_eq!(
            std::fs::read_link(tmp.path().join("host.key")).unwrap(),
            std::path::PathBuf::from("identity/current/host.key")
        );
        assert!(std::fs::read_to_string(tmp.path().join("config.yaml"))
            .unwrap()
            .contains("key: /etc/nebula/identity/current/host.key"));
    }

    #[test]
    fn replicated_refresh_refuses_mismatched_legacy_flat_cert() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bundle = sample_bundle();
        std::fs::write(tmp.path().join("host.crt"), b"different-cert").expect("legacy cert");
        crate::ca::seal::write_atomic_sealed(tmp.path().join("host.key").as_path(), b"legacy-key")
            .expect("legacy key");

        let error =
            materialize_config(tmp.path(), &bundle, ConfigRole::Peer, &[], tmp.path(), None)
                .expect_err("mismatched legacy cert must fail closed");

        assert!(error.contains("legacy local Nebula cert does not match replicated bundle"));
        assert!(
            !tmp.path().join("identity/current").exists(),
            "no identity switch should be activated for a mismatched legacy cert"
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
    fn oversized_active_identity_cert_fails_closed_before_comparison() {
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
        std::fs::write(
            tmp.path().join("identity/current/host.crt"),
            vec![b'x'; crate::ca::seal::MAX_SEALED_FILE_BYTES as usize + 1],
        )
        .expect("oversized active cert");

        let error = materialize_config(tmp.path(), &first, ConfigRole::Peer, &[], tmp.path(), None)
            .expect_err("oversized active certificate must fail closed");
        assert!(error.contains("exceeds"), "unexpected error: {error}");
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
    fn render_peer_config_uses_lighthouse_relay_and_punchy_paths() {
        let yaml = render_config_yaml(&sample_bundle(), ConfigRole::Peer);
        assert!(yaml.contains("relay:"));
        assert!(yaml.contains("am_relay: false"));
        assert!(yaml.contains("use_relays: true"));
        assert!(yaml.contains("punchy:"));
        assert!(yaml.contains("punch: true"));
        assert!(yaml.contains("respond: true"));
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
    fn render_config_bounds_nebula_log_verbosity() {
        let yaml = render_config_yaml(&sample_bundle(), ConfigRole::Peer);
        assert!(yaml.contains("logging:\n  level: warn\n"));
        assert!(yaml.contains("  format: text\n"));
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
        write_role_marker(&marker, "peer:test").expect("write");
        assert!(marker.exists());
        assert_eq!(read_role_marker(&marker).as_deref(), Some("peer:test"));
    }

    #[test]
    fn role_marker_requires_exact_owner_checked_schema() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let marker = tmp.path().join("role.host");
        assert!(
            read_role_marker(&marker).is_none(),
            "absent marker → invalid"
        );
        write_role_marker(&marker, "peer:test").expect("write");
        assert_eq!(read_role_marker(&marker).as_deref(), Some("peer:test"));
        std::fs::write(&marker, b"role:host\n").expect("legacy overwrite");
        assert!(
            read_role_marker(&marker).is_none(),
            "legacy marker must fail closed"
        );
    }

    #[tokio::test]
    async fn marker_requires_the_current_local_leader_lease() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let lock = tmp.path().join(".mackesd-leader.lock");
        let marker = tmp.path().join("role.host");
        write_role_marker(&marker, "peer:test").expect("write");
        assert!(!role_marker_is_current_leader(&marker, tmp.path(), "peer:test", &[],).await);
        crate::leader::force_take(&lock, "peer:test").expect("acquire lease");
        assert!(role_marker_is_current_leader(&marker, tmp.path(), "peer:test", &[],).await);
        assert!(!role_marker_is_current_leader(&marker, tmp.path(), "peer:other", &[],).await);
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

    #[tokio::test]
    async fn failed_demotion_remains_pending_for_a_later_retry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = tmp.path().join("store.sqlite");
        let conn = crate::store::open(&db).expect("open");
        let marker = tmp.path().join("role.host");
        // A directory at the marker path makes the demotion's remove_file
        // fail, modelling a hostile replacement or filesystem error.
        std::fs::create_dir(&marker).expect("marker directory");
        let mut supervisor = NebulaSupervisor::new(
            Arc::new(Mutex::new(conn)),
            "peer:test".into(),
            "m1".into(),
            tmp.path().join("nebula-bundle.json"),
        )
        .with_role_marker(marker);
        supervisor.last_is_leader = true;

        supervisor.tick().await;

        assert!(
            supervisor.last_is_leader,
            "a failed demotion must remain pending so the next tick retries it"
        );
    }

    #[cfg(unix)]
    fn fake_systemctl_fails_first_reload(root: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = root.join("systemctl");
        std::fs::write(
            &path,
            br####"#!/bin/sh
if [ "$1" = "reload-or-restart" ] && [ ! -e "${0}.failed" ]; then
    touch "${0}.failed"
    echo "simulated Nebula reconnect failure" >&2
    exit 1
fi
exit 0
"####,
        )
        .expect("write fake systemctl");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make fake systemctl executable");
        path
    }

    #[cfg(unix)]
    fn fake_systemctl_succeeds(root: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = root.join("systemctl");
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").expect("write fake systemctl");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make fake systemctl executable");
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_nebula_reload_keeps_bundle_pending_for_reconnect() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let bundle_path = root.join("nebula-bundle.json");
        let bundle = sample_bundle();
        crate::ca::bundle::write_bundle(&bundle_path, &bundle).expect("seed bundle");

        // Seed the already-materialized local identity. The supervisor must
        // not acknowledge the bundle until Nebula has accepted the rendered
        // config and reconnected; a later bundle change may never be required.
        let config_dir = root.join("nebula");
        materialize_config(
            &config_dir,
            &bundle,
            ConfigRole::Peer,
            &[],
            root,
            Some(b"local-key"),
        )
        .expect("seed local identity");

        let systemctl_path = fake_systemctl_fails_first_reload(root);
        let conn = crate::store::open(&root.join("store.sqlite")).expect("open");
        let mut supervisor = NebulaSupervisor::new(
            Arc::new(Mutex::new(conn)),
            "peer:test".into(),
            "m1".into(),
            bundle_path,
        )
        .with_workgroup_root(root.to_path_buf())
        .with_role_marker(root.join("role.host"))
        .with_config_dir(config_dir)
        .with_overlay_ip_path(root.join("overlay-ip"))
        .with_leadership_endpoints(Vec::new())
        .with_systemctl_path(systemctl_path.clone());

        supervisor.tick().await;
        assert!(
            supervisor.last_bundle_mtime.is_none(),
            "a failed Nebula reconnect must leave the bundle pending"
        );

        supervisor.tick().await;
        assert!(
            supervisor.last_bundle_mtime.is_some(),
            "the unchanged bundle must be acknowledged after reconnect recovery"
        );
        assert!(
            systemctl_path.with_file_name("systemctl.failed").exists(),
            "the fixture must exercise the failed-reload branch"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_refresh_remains_pending_for_a_later_retry() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let bundle_path = root.join("nebula-bundle.json");
        crate::ca::bundle::write_bundle(&bundle_path, &sample_bundle()).expect("seed bundle");
        let fingerprint = "a".repeat(64);
        crate::ca::blocklist::record_revoked(root, "peer:test", &[fingerprint.clone()])
            .expect("seed blocklist");

        let config_dir = root.join("nebula");
        let outside = tempfile::tempdir().expect("outside");
        symlink(outside.path(), &config_dir).expect("config symlink");
        let conn = crate::store::open(&root.join("store.sqlite")).expect("open");
        let mut supervisor = NebulaSupervisor::new(
            Arc::new(Mutex::new(conn)),
            "peer:test".into(),
            "m1".into(),
            bundle_path,
        )
        .with_workgroup_root(root.to_path_buf())
        .with_role_marker(root.join("role.host"))
        .with_config_dir(config_dir.clone())
        .with_overlay_ip_path(root.join("overlay-ip"))
        .with_leadership_endpoints(Vec::new())
        .with_systemctl_path(fake_systemctl_succeeds(root));

        supervisor.tick().await;
        assert!(
            supervisor.last_bundle_mtime.is_none(),
            "a failed refresh must not acknowledge the bundle mtime"
        );
        assert!(
            supervisor.last_blocklist.is_empty(),
            "a failed refresh must not acknowledge the blocklist"
        );

        std::fs::remove_file(&config_dir).expect("remove hostile symlink");
        materialize_config(
            &config_dir,
            &sample_bundle(),
            ConfigRole::Peer,
            &[],
            root,
            Some(b"local-key"),
        )
        .expect("seed valid identity");
        supervisor.tick().await;

        assert!(
            supervisor.last_bundle_mtime.is_some(),
            "the unchanged bundle must be retried and acknowledged after recovery"
        );
        assert_eq!(supervisor.last_blocklist, vec![fingerprint]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn final_symlinked_bundle_is_ignored_until_replaced() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let bundle_path = root.join("nebula-bundle.json");
        let outside = tempfile::tempdir().expect("outside");
        let target = outside.path().join("bundle.json");
        crate::ca::bundle::write_bundle(&target, &sample_bundle()).expect("seed target bundle");
        symlink(&target, &bundle_path).expect("bundle symlink");

        let config_dir = root.join("nebula");
        materialize_config(
            &config_dir,
            &sample_bundle(),
            ConfigRole::Peer,
            &[],
            root,
            Some(b"local-key"),
        )
        .expect("seed local identity");
        let conn = crate::store::open(&root.join("store.sqlite")).expect("open");
        let mut supervisor = NebulaSupervisor::new(
            Arc::new(Mutex::new(conn)),
            "peer:test".into(),
            "m1".into(),
            bundle_path.clone(),
        )
        .with_workgroup_root(root.to_path_buf())
        .with_role_marker(root.join("role.host"))
        .with_config_dir(config_dir)
        .with_overlay_ip_path(root.join("overlay-ip"))
        .with_leadership_endpoints(Vec::new())
        .with_systemctl_path(fake_systemctl_succeeds(root));

        supervisor.tick().await;
        assert!(
            supervisor.last_bundle_mtime.is_none(),
            "a final bundle symlink must be ignored and remain unacknowledged"
        );

        std::fs::remove_file(&bundle_path).expect("remove bundle symlink");
        crate::ca::bundle::write_bundle(&bundle_path, &sample_bundle()).expect("replace bundle");
        supervisor.tick().await;
        assert!(
            supervisor.last_bundle_mtime.is_some(),
            "a regular replacement must be retried and acknowledged"
        );
    }

    #[tokio::test]
    async fn non_regular_bundle_is_ignored_until_replaced() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let bundle_path = root.join("nebula-bundle.json");
        std::fs::create_dir(&bundle_path).expect("bundle directory");

        let config_dir = root.join("nebula");
        materialize_config(
            &config_dir,
            &sample_bundle(),
            ConfigRole::Peer,
            &[],
            root,
            Some(b"local-key"),
        )
        .expect("seed local identity");
        let conn = crate::store::open(&root.join("store.sqlite")).expect("open");
        let mut supervisor = NebulaSupervisor::new(
            Arc::new(Mutex::new(conn)),
            "peer:test".into(),
            "m1".into(),
            bundle_path.clone(),
        )
        .with_workgroup_root(root.to_path_buf())
        .with_role_marker(root.join("role.host"))
        .with_config_dir(config_dir)
        .with_overlay_ip_path(root.join("overlay-ip"))
        .with_leadership_endpoints(Vec::new())
        .with_systemctl_path(fake_systemctl_succeeds(root));

        supervisor.tick().await;
        assert!(
            supervisor.last_bundle_mtime.is_none(),
            "a non-regular bundle must be ignored and remain unacknowledged"
        );

        std::fs::remove_dir(&bundle_path).expect("remove bundle directory");
        crate::ca::bundle::write_bundle(&bundle_path, &sample_bundle()).expect("replace bundle");
        supervisor.tick().await;
        assert!(
            supervisor.last_bundle_mtime.is_some(),
            "a regular replacement must be retried and acknowledged"
        );
    }

    #[tokio::test]
    async fn elected_leader_bootstraps_a_missing_role_marker() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db = tmp.path().join("store.sqlite");
        let conn = crate::store::open(&db).expect("open");
        let lock = tmp.path().join(".mackesd-leader.lock");
        crate::leader::force_take(&lock, "peer:test").expect("acquire lease");
        let marker = tmp.path().join("role.host");
        let mut supervisor = NebulaSupervisor::new(
            Arc::new(Mutex::new(conn)),
            "peer:test".into(),
            "m1".into(),
            tmp.path().join("nebula-bundle.json"),
        )
        .with_workgroup_root(tmp.path().to_path_buf())
        .with_role_marker(marker.clone())
        .with_leadership_endpoints(Vec::new());

        assert!(!marker.exists(), "test starts without a role marker");
        supervisor.tick().await;

        assert!(supervisor.last_is_leader);
        assert_eq!(read_role_marker(&marker).as_deref(), Some("peer:test"));
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
    fn reconcile_normalizes_duplicate_lighthouse_overlay_claims() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        seed_lighthouse(&root, "lh-hostname", "10.42.0.1", "lh1.example.com:4242");
        seed_lighthouse(&root, "lh-numeric", "10.42.0.1", "203.0.113.1:4242");
        seed_lighthouse(&root, "lh-02", "10.42.0.2", "203.0.113.2:4242");

        let bundle_path = root.join("nebula-bundle.json");
        let mut b = sample_bundle();
        b.overlay_ip = "10.42.0.9".into();
        b.lighthouses = vec![
            LighthouseEntry {
                node_id: "lh-hostname".into(),
                overlay_ip: "10.42.0.1".into(),
                external_addr: "lh1.example.com:4242".into(),
                relay_tls: None,
            },
            LighthouseEntry {
                node_id: "lh-stale".into(),
                overlay_ip: "10.42.0.1".into(),
                external_addr: "198.51.100.99:4242".into(),
                relay_tls: None,
            },
        ];
        crate::ca::bundle::write_bundle(&bundle_path, &b).expect("seed duplicate bundle");

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
        assert_eq!(
            after
                .lighthouses
                .iter()
                .filter(|lh| lh.overlay_ip == "10.42.0.1")
                .count(),
            1,
            "a stored bundle must not retain duplicate lighthouse overlay IP claims"
        );
        let winner = after
            .lighthouses
            .iter()
            .find(|lh| lh.overlay_ip == "10.42.0.1")
            .expect("deduped lighthouse");
        assert_eq!(winner.node_id, "lh-numeric");
        assert_eq!(winner.external_addr, "203.0.113.1:4242");
        assert!(
            after
                .lighthouses
                .iter()
                .any(|lh| lh.overlay_ip == "10.42.0.2"),
            "normalization must preserve other unique lighthouses"
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

    #[tokio::test]
    async fn foreign_relay_authority_cannot_reconcile_or_refresh() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        seed_lighthouse(&root, "lh-01", "10.42.0.1", "203.0.113.1:4242");

        let trusted_key = ed25519_dalek::SigningKey::from_bytes(&[7_u8; 32]);
        let foreign_key = ed25519_dalek::SigningKey::from_bytes(&[8_u8; 32]);
        let pin_path = root.join("relay-authority.pub");
        let mut pinned_bundle = sample_bundle();
        pinned_bundle.relay_trust_authority = Some(
            crate::ca::bundle::relay_trust_authority_public_key(&trusted_key),
        );
        crate::ca::bundle::write_relay_trust_authority_pin(&pinned_bundle, &pin_path)
            .expect("write local relay authority pin");

        let mut bundle = sample_bundle();
        bundle.lighthouses[0].node_id = "lh-01".into();
        bundle.lighthouses[0].overlay_ip = "10.42.0.1".into();
        bundle.lighthouses[0].external_addr = "203.0.113.1:4242".into();
        bundle.relay_trust_authority = Some(crate::ca::bundle::relay_trust_authority_public_key(
            &foreign_key,
        ));
        let identity = crate::ca::bundle::RelayTlsIdentity::from_certificate_pem(
            "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n",
        )
        .expect("relay identity");
        bundle.lighthouses[0].relay_tls = Some(crate::ca::bundle::sign_relay_tls_identity(
            identity,
            &bundle.lighthouses[0].node_id,
            &bundle.lighthouses[0].overlay_ip,
            &bundle.lighthouses[0].external_addr,
            &foreign_key,
        ));
        let bundle_path = root.join("nebula-bundle.json");
        crate::ca::bundle::write_bundle(&bundle_path, &bundle).expect("seed foreign bundle");

        let conn = crate::store::open(&root.join("store.sqlite")).expect("open");
        let config_dir = root.join("nebula");
        let supervisor = NebulaSupervisor::new(
            Arc::new(Mutex::new(conn)),
            "peer:test".into(),
            "m1".into(),
            bundle_path.clone(),
        )
        .with_workgroup_root(root.clone())
        .with_config_dir(config_dir.clone())
        .with_relay_trust_authority_pin(pin_path);

        supervisor.reconcile_lighthouse_roster();
        assert_eq!(
            crate::ca::bundle::read_bundle(&bundle_path).expect("read bundle"),
            bundle,
            "roster reconciliation must not adopt state under a foreign authority"
        );
        let error = supervisor
            .refresh_config()
            .await
            .expect_err("a foreign relay authority must block config refresh");
        assert!(error.contains("does not match the local enrollment pin"));
        assert!(
            !config_dir.exists(),
            "blocked refresh must not materialize config from foreign relay state"
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

    #[cfg(unix)]
    #[test]
    fn publish_overlay_ip_rejects_symlinked_parent_before_writing_outside_tree() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        let redirected = root.path().join("var");
        symlink(outside.path(), &redirected).expect("symlink parent");

        let error = publish_overlay_ip(&redirected.join("overlay-ip"), "10.42.0.5")
            .expect_err("symlinked parent must be refused");

        assert!(
            error.contains("symlinked parent"),
            "unexpected error: {error}"
        );
        assert!(
            !outside.path().join("overlay-ip").exists(),
            "a hostile parent link must not redirect the overlay-IP write"
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
