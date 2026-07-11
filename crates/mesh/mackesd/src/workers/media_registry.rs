//! MEDIA-7 — register the navidrome/media service into the mesh registry.
//!
//! Runs ONLY on a `Lighthouse_Media` node — it is capability-gated on
//! MEDIA-1's [`Capability::Media`](mde_role::Capability::Media) (worker name
//! `navidrome` in `worker_role::WORKER_CAPABILITIES`), so the spawn site gates
//! it through `worker_role::runs_in("navidrome", deploy_class)` and it is
//! absent on every non-media node.
//!
//! Each tick it:
//!   1. probes the local navidrome instance's health
//!      ([`mesh_media::probe_navidrome`]),
//!   2. builds the registration ([`mesh_media::MediaRegistration`], carrying
//!      the per-instance `health` field), and
//!   3. publishes it into the SAME mesh service registry the other published
//!      services use — the per-peer Bus topic
//!      `mesh/services/media/<peer>` AND the replicated QNM-Shared plane at
//!      `<mount>/<host>/media-registry.json` (the registry plane every node
//!      already reads, alongside `compute-inventory.json` /
//!      `running-apps.json`).
//!
//! Publish cadence mirrors `compute_registry` (BUS-RUN-FULL-1 / ADR-0005):
//! on-change plus a slow heartbeat, so a registry whose body hasn't changed
//! isn't republished every tick, but a late subscriber / pruned topic still
//! finds a recent doc. The QNM-Shared mirror is written every tick (atomic
//! tmp+rename), since a reader of the replicated file always wants the latest.
//!
//! Out of scope (MEDIA-2): the live-stream / bucket acceptance and the running
//! navidrome itself. This is the registry-publish + per-instance-health half —
//! real, spawned, and gated (§7).

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{ShutdownToken, Worker};
use crate::ipc::secret_store::{self, SecretStore};
use crate::mesh_media::{self, MediaRegistration, SharedAccount};

/// 30 s registry tick — a published service-presence registration is slow-
/// changing (the instance is up or it isn't); 30 s keeps the `health` field
/// fresh without a tight loop. The on-change publish below means an up→down
/// flip still propagates on the very next tick.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Slow heartbeat for the on-change Bus publish — republish an unchanged
/// registration at most this often so a freshly-pruned topic / late subscriber
/// still finds a recent doc. Matches `compute_registry::PUBLISH_HEARTBEAT`'s
/// rationale.
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(300);

/// Publish a media registration to its per-peer Bus topic via the `mde-bus`
/// CLI (typed argv, §9 — no shell). Pub so a future immediate-on-start publish
/// can reuse it. Best-effort: a missing/failing `mde-bus` is logged by the
/// reaper, never fatal.
pub fn publish_registration(reg: &MediaRegistration) {
    let topic = mesh_media::media_registry_topic(&reg.node_id);
    publish_registration_to(
        crate::bus_publish::default_bus_root().as_deref(),
        &topic,
        reg,
    );
}

/// Root-injectable in-process publish for [`publish_registration`] (perf-10 /
/// arch-6) — no fork+exec of the `mde-bus` CLI per registration. Fresh-opens the
/// Bus at `bus_root` (the CLI-equivalent [`crate::bus_publish::default_bus_root`]
/// in production, honouring `MDE_BUS_ROOT`) and writes the compact `serde_json`
/// of `reg` — the exact body the old `--body-flag` carried. Best-effort; tests
/// pass a temp root.
fn publish_registration_to(
    bus_root: Option<&std::path::Path>,
    topic: &str,
    reg: &MediaRegistration,
) {
    if let Some(mut persist) =
        crate::bus_publish::open_bus(bus_root.map(std::path::Path::to_path_buf))
    {
        crate::bus_publish::publish_json(&mut persist, topic, reg);
    }
}

/// Mirror a node's media registration to the replicated QNM-Shared registry
/// plane at `<mount>/<hostname>/media-registry.json` — the SAME plane the
/// other published services replicate through. Atomic (tmp + rename) so a
/// reader never sees a half-written file. Best-effort: a missing mount / write
/// error is logged, never fatal.
pub fn write_shared_registration(mount: &Path, hostname: &str, reg: &MediaRegistration) {
    if hostname.is_empty() {
        return;
    }
    let dir = mount.join(hostname);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("media_registry: mkdir {} failed: {e}", dir.display());
        return;
    }
    let Ok(body) = serde_json::to_string(reg) else {
        return;
    };
    let tmp = dir.join("media-registry.json.tmp");
    let final_path = dir.join(mesh_media::MEDIA_REGISTRY_FILE);
    if let Err(e) = std::fs::write(&tmp, body.as_bytes()) {
        tracing::warn!("media_registry: write {} failed: {e}", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        tracing::warn!("media_registry: rename registration failed: {e}");
    }
}

/// MEDIA-8 — resolve the read-only shared account to publish from the
/// `media-spaces` leader secret. Reads the sealed secret (the same one
/// `setup-media-navidrome.sh` consumes) and parses `ND_ADMIN_USER`/`ND_ADMIN_PASS`
/// out of its `.env` body. `None` when:
///   * the secret isn't distributed to this node yet (honest "no account"), or
///   * the store faults / the body is malformed (logged; we publish a
///     registration WITHOUT an account rather than a fabricated one).
///
/// Pulled out + `store`-parameterized so the worker tick can call it and tests
/// can drive it against a `LocalAead` store with a seeded secret. Best-effort by
/// design — a media node with no shared creds still publishes its health, just
/// without the auto-config account.
fn resolve_shared_account(store: &SecretStore) -> Option<SharedAccount> {
    match store.get(&secret_store::media_spaces_creds_ref()) {
        Ok(Some(body)) => {
            let acct = SharedAccount::from_media_spaces_env(&body);
            if acct.is_none() {
                tracing::warn!(
                    "media_registry: media-spaces secret present but ND_ADMIN_USER/PASS \
                     missing/empty — publishing registration without a shared account"
                );
            }
            acct
        }
        // Not distributed yet — honest absence, no account to publish.
        Ok(None) => None,
        Err(e) => {
            tracing::warn!("media_registry: reading media-spaces secret: {e}");
            None
        }
    }
}

/// Worker handle.
pub struct MediaRegistryWorker {
    /// Registering node-id (the registry key / topic suffix).
    node_id: String,
    /// Hostname the QNM-Shared mirror is keyed under.
    hostname: String,
    /// Navidrome port to probe + register.
    port: u16,
    /// Registry tick cadence.
    tick: Duration,
    /// Replicated QNM-Shared registry root.
    mount: PathBuf,
    /// MEDIA-8 — the secret store the shared account is read from. Resolved once
    /// at construction (`Mesh` when the helper script is reachable from the repo
    /// root, else the local-AEAD fallback) — same as every other secret-store
    /// consumer (`copilot`, `vpn_gw`).
    secret_store: SecretStore,
    /// Slow heartbeat for the on-change Bus publish.
    publish_heartbeat: Duration,
    /// Last published body + when, so we publish on-change and only
    /// heartbeat-republish an unchanged registration.
    last_publish: Mutex<Option<(String, Instant)>>,
}

impl MediaRegistryWorker {
    /// Construct with production defaults. `node_id` is the registry key;
    /// `hostname` keys the QNM-Shared mirror.
    #[must_use]
    pub fn new(node_id: String, hostname: String) -> Self {
        let mount = crate::default_qnm_shared_root();
        let secret_store = SecretStore::resolve(&secret_store::repo_root(), &mount);
        Self {
            node_id,
            hostname,
            port: mesh_media::NAVIDROME_PORT,
            tick: DEFAULT_TICK_INTERVAL,
            mount,
            secret_store,
            publish_heartbeat: PUBLISH_HEARTBEAT,
            last_publish: Mutex::new(None),
        }
    }

    /// Override the secret store the shared account is read from. Used in tests
    /// to drive a seeded `LocalAead` store without the mesh helper script.
    #[must_use]
    pub fn with_secret_store(mut self, store: SecretStore) -> Self {
        self.secret_store = store;
        self
    }

    /// Override the replicated registry root. Used in tests + to honor a
    /// `--workgroup-root` override at the spawn site (so the worker writes
    /// where the registry readers look).
    #[must_use]
    pub fn with_mount(mut self, p: PathBuf) -> Self {
        self.mount = p;
        self
    }

    /// Override the probed/registered port. Used in tests.
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Build this tick's registration from a live health probe + the shared
    /// account read from the `media-spaces` secret (MEDIA-8). Re-read each tick
    /// so a freshly-distributed secret is picked up without a worker restart;
    /// `None` when the secret isn't here yet (the registration still publishes,
    /// just without the auto-config account).
    fn build_registration(&self) -> MediaRegistration {
        let health = mesh_media::probe_navidrome(self.port);
        let account = resolve_shared_account(&self.secret_store);
        mesh_media::registration_with_account(&self.node_id, self.port, &health, account)
    }

    fn tick_once(&self) {
        let reg = self.build_registration();
        // Bus publish: on-change + slow heartbeat (BUS-RUN-FULL-1). Serialize
        // once and compare against the last published body.
        if let Ok(body) = serde_json::to_string(&reg) {
            let mut last = self
                .last_publish
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let now = Instant::now();
            let prev_body = last.as_ref().map(|(b, _)| b.as_str());
            let prev_at = last.as_ref().map(|(_, at)| *at);
            if crate::workers::compute_registry::should_publish(
                prev_body,
                &body,
                prev_at,
                now,
                self.publish_heartbeat,
            ) {
                publish_registration(&reg);
                *last = Some((body, now));
            }
        }
        // QNM-Shared mirror: the replicated registry plane every node reads.
        // Written every tick (the reader always wants the latest); the helper
        // is a no-op when the mount/host is absent.
        write_shared_registration(&self.mount, &self.hostname, &reg);
    }
}

#[async_trait::async_trait]
impl Worker for MediaRegistryWorker {
    fn name(&self) -> &'static str {
        "media_registry"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.tick) => {
                    self.tick_once();
                }
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_media::{HEALTH_DOWN, NAVIDROME_KIND};

    /// perf-10 / arch-6 — `publish_registration_to` writes the registration
    /// in-process (no fork+exec of `mde-bus`) with EXACTLY the row a
    /// `mde-bus publish mesh/services/media/<node> --body-flag <json>` produced:
    /// the topic, default priority, no title/actions/reply, and a body that is
    /// the compact `serde_json` of the registration (typed `publish_json` path).
    #[test]
    fn publish_registration_to_writes_cli_equivalent_row_in_process() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = mesh_media::registration("peer:eagle", mesh_media::NAVIDROME_PORT, "up");
        let topic = mesh_media::media_registry_topic(&reg.node_id);

        publish_registration_to(Some(tmp.path()), &topic, &reg);

        let reader = mde_bus::persist::Persist::open(tmp.path().to_path_buf()).unwrap();
        let rows = reader.list_since(&topic, None).unwrap();
        assert_eq!(rows.len(), 1, "exactly one registration published");
        let row = &rows[0];
        assert_eq!(row.topic, topic);
        assert_eq!(row.priority, "default");
        assert!(row.title.is_none());
        assert!(row.actions.is_empty());
        assert!(row.reply_to.is_none());
        // Body is the compact serialization `--body-flag` carried, and decodes
        // back to the original typed registration.
        assert_eq!(
            row.body.as_deref(),
            Some(serde_json::to_string(&reg).unwrap().as_str())
        );
        let back: MediaRegistration = serde_json::from_str(row.body.as_deref().unwrap()).unwrap();
        assert_eq!(back, reg);
    }

    #[test]
    fn build_registration_probes_health_and_pins_kind() {
        // Port 1 is unbound → the probe degrades to `down`; the worker still
        // registers the navidrome kind + the configured port (registry is
        // service-presence: the instance is declared, health says it's down).
        let w = MediaRegistryWorker::new("peer:eagle".into(), "eagle".into()).with_port(1);
        let reg = w.build_registration();
        assert_eq!(reg.node_id, "peer:eagle");
        assert_eq!(reg.kind, NAVIDROME_KIND);
        assert_eq!(reg.port, 1);
        assert_eq!(reg.health, HEALTH_DOWN);
    }

    #[test]
    fn shared_mirror_writes_atomic_registry_file() {
        let tmp = std::env::temp_dir().join(format!("mde-mediareg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let reg = mesh_media::registration("peer:eagle", mesh_media::NAVIDROME_PORT, "up");
        write_shared_registration(&tmp, "eagle", &reg);
        let path = tmp.join("eagle").join(mesh_media::MEDIA_REGISTRY_FILE);
        let body = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
        let back: MediaRegistration = serde_json::from_str(&body).unwrap();
        assert_eq!(back, reg);
        // No leftover tmp file (atomic rename consumed it).
        assert!(!tmp.join("eagle").join("media-registry.json.tmp").exists());
    }

    #[test]
    fn shared_mirror_skips_empty_hostname() {
        let tmp = std::env::temp_dir().join(format!("mde-mediareg-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let reg = mesh_media::registration("peer:x", mesh_media::NAVIDROME_PORT, "down");
        write_shared_registration(&tmp, "", &reg);
        // Nothing written for an empty hostname.
        assert!(!tmp.exists());
    }

    // ── MEDIA-8: the shared account read from the media-spaces secret ──

    /// Stand up a `LocalAead` store with a real mesh age identity so the
    /// round-trip seal/unseal exercises the same path production uses.
    fn seeded_store(dir: &std::path::Path) -> SecretStore {
        let key_path = dir.join("mcnf-age-key");
        std::fs::write(
            &key_path,
            "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQSXKLP0E\n",
        )
        .unwrap();
        SecretStore::LocalAead {
            dir: dir.join("secrets"),
            key_path,
        }
    }

    #[test]
    fn resolve_shared_account_none_when_secret_absent() {
        // No media-spaces secret distributed → honest None (no account to
        // publish), never a fabricated one.
        let tmp = tempfile::tempdir().unwrap();
        let store = seeded_store(tmp.path());
        assert_eq!(resolve_shared_account(&store), None);
    }

    #[test]
    fn resolve_shared_account_reads_sealed_media_spaces_secret() {
        // Seal a realistic media-spaces .env body, then the worker reads back
        // the ND_ADMIN_* shared account, pinned to music.mesh.
        let tmp = tempfile::tempdir().unwrap();
        let store = seeded_store(tmp.path());
        let body = "\
DO_SPACES_KEY=AKIAEXAMPLE\n\
DO_SPACES_SECRET=secret\n\
ND_ADMIN_USER=mesh-music\n\
ND_ADMIN_PASS=hunter2\n";
        store
            .put(&secret_store::media_spaces_creds_ref(), body)
            .unwrap();
        let acct = resolve_shared_account(&store).expect("account read back");
        assert_eq!(acct.server, "http://music.mesh:4533");
        assert_eq!(acct.username, "mesh-music");
        assert_eq!(acct.password, "hunter2");
    }

    #[test]
    fn build_registration_attaches_the_shared_account() {
        // End-to-end (sans socket): a worker pointed at a store holding the
        // secret builds a registration carrying the shared account.
        let tmp = tempfile::tempdir().unwrap();
        let store = seeded_store(tmp.path());
        store
            .put(
                &secret_store::media_spaces_creds_ref(),
                "ND_ADMIN_USER=mesh-music\nND_ADMIN_PASS=hunter2\n",
            )
            .unwrap();
        let w = MediaRegistryWorker::new("peer:eagle".into(), "eagle".into())
            .with_port(1)
            .with_secret_store(store);
        let reg = w.build_registration();
        let acct = reg.shared_account.expect("account attached");
        assert_eq!(acct.username, "mesh-music");
        assert_eq!(acct.server, "http://music.mesh:4533");
    }
}
