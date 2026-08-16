//! WL-FUNC-020 S1 — Android runtime catalog importer.
//!
//! The catalog is discovery and policy data, not a security authority. This
//! worker drains the node-scoped typed import topic and publishes only catalogs
//! that pass the shared structural, freshness, and revision contract.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::android_apps::{
    android_catalog_import_topic, android_catalog_state_topic, AndroidRuntimeCatalog,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};

const POLL: Duration = Duration::from_secs(1);
const SYSTEM_BUS_ROOT: &str = mde_bus::SYSTEM_BUS_ROOT;
const MAX_IMPORT_BYTES: usize = 2 * 1024 * 1024;
const MAX_ROWS_PER_POLL: usize = 32;
const MAX_CACHE_TEMP_ATTEMPTS: usize = 32;
const CACHE_SCHEMA: u16 = 1;

const STATE_FILE_ENV: &str = "MDE_ANDROID_CATALOG_STATE_FILE";

#[derive(Debug, Clone)]
struct CatalogConfig {
    state_file: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedCatalog {
    schema_version: u16,
    catalog: AndroidRuntimeCatalog,
}

/// Workstation importer for one node's Android runtime catalog.
pub struct AndroidCatalogWorker {
    host: String,
    /// Explicit override for tests/deployments. `None` resolves the user Bus
    /// root afresh on every pass and then falls back to the system spool.
    bus_root: Option<PathBuf>,
    config: Option<CatalogConfig>,
    poll: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BusIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

#[derive(Debug, Clone, Default)]
struct ImportState {
    published_replay: bool,
    cursor: Option<String>,
    current: Option<AndroidRuntimeCatalog>,
    bus_identity: Option<BusIdentity>,
}

impl AndroidCatalogWorker {
    /// Build from the production environment contract. Invalid node or state
    /// configuration leaves the worker alive but quiescent.
    #[must_use]
    pub fn new(host: String) -> Self {
        let config = load_environment_config(&host)
            .map_err(|error| {
                tracing::warn!(target: "mackesd::android_catalog", %error, "Android catalog configuration is unavailable; importer is quiescent");
                error
            })
            .ok();
        Self {
            host,
            bus_root: None,
            config,
            poll: POLL,
        }
    }

    fn resolved_bus_root(&self) -> PathBuf {
        resolve_bus_root(self.bus_root.clone(), mde_bus::default_data_dir())
    }

    #[cfg(test)]
    const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    fn process_once(
        &self,
        persist: &mut Persist,
        cursor: &mut Option<String>,
        current: &mut Option<AndroidRuntimeCatalog>,
        now_ms: u64,
    ) -> io::Result<usize> {
        let Some(config) = self.config.as_ref() else {
            return Ok(0);
        };
        let import_topic = android_catalog_import_topic(&self.host).map_err(io_other)?;
        let rows = persist
            .list_since_limit(&import_topic, cursor.as_deref(), MAX_ROWS_PER_POLL)
            .map_err(io_other)?;
        let mut admitted = 0;
        for row in rows {
            let row_ulid = row.ulid;
            let Some(body) = row.body else {
                *cursor = Some(row_ulid);
                continue;
            };
            if body.len() > MAX_IMPORT_BYTES {
                tracing::warn!(target: "mackesd::android_catalog", "refused oversized Android catalog import");
                *cursor = Some(row_ulid);
                continue;
            }
            let candidate = match serde_json::from_str::<AndroidRuntimeCatalog>(&body) {
                Ok(candidate) => candidate,
                Err(error) => {
                    tracing::warn!(target: "mackesd::android_catalog", %error, "refused malformed Android catalog import");
                    *cursor = Some(row_ulid);
                    continue;
                }
            };
            let candidate = match candidate.admit(now_ms) {
                Ok(candidate) => candidate,
                Err(error) => {
                    tracing::warn!(target: "mackesd::android_catalog", ?error, "refused invalid Android catalog import");
                    *cursor = Some(row_ulid);
                    continue;
                }
            };
            if current
                .as_ref()
                .is_some_and(|existing| candidate.payload.catalog_id != existing.payload.catalog_id)
            {
                tracing::warn!(
                    target: "mackesd::android_catalog",
                    catalog_id = %candidate.payload.catalog_id,
                    "refused Android catalog identity switch"
                );
                *cursor = Some(row_ulid);
                continue;
            }
            if current
                .as_ref()
                .is_some_and(|existing| candidate.payload.revision <= existing.payload.revision)
            {
                tracing::warn!(target: "mackesd::android_catalog", revision = candidate.payload.revision, "refused stale Android catalog revision");
                *cursor = Some(row_ulid);
                continue;
            }
            store_last_good(&config.state_file, &candidate)?;
            publish_admitted(persist, &self.host, &candidate)?;
            *current = Some(candidate);
            *cursor = Some(row_ulid);
            admitted += 1;
        }
        Ok(admitted)
    }

    fn activate_bus(
        &self,
        persist: &mut Persist,
        state: &mut ImportState,
        identity: BusIdentity,
        now_ms: u64,
    ) -> io::Result<()> {
        if state.bus_identity == Some(identity) {
            return Ok(());
        }

        // Imports are durable content declarations. A replacement index is a new
        // history, so replay it from the beginning after restoring the durable
        // last-good projection.
        let mut staged = state.clone();
        staged.cursor = None;
        staged.published_replay = false;
        if let Some(catalog) = staged.current.as_ref() {
            match catalog.clone().admit(now_ms) {
                Ok(catalog) => publish_admitted(persist, &self.host, &catalog)?,
                Err(error) => tracing::warn!(
                    target: "mackesd::android_catalog",
                    ?error,
                    revision = catalog.payload.revision,
                    "refused stale or invalid Android catalog replay on replacement Bus"
                ),
            }
        }
        staged.published_replay = true;
        staged.bus_identity = Some(identity);
        *state = staged;
        Ok(())
    }

    fn process_bus_pass(
        &self,
        persist: &mut Persist,
        bus_root: &Path,
        state: &mut ImportState,
        now_ms: u64,
    ) -> io::Result<()> {
        persist.reopen_if_index_changed();
        let identity = bus_identity(bus_root)?;
        verify_bus_identity(persist, bus_root, identity)?;

        // Do not acknowledge replay/import progress until every effect is known
        // to have reached the same Bus generation this pass opened. A path swap
        // can otherwise strand publication on the retired SQLite inode while
        // advancing the replacement generation's cursor in memory.
        let mut staged = state.clone();
        self.activate_bus(persist, &mut staged, identity, now_ms)?;
        self.process_once(persist, &mut staged.cursor, &mut staged.current, now_ms)?;
        verify_bus_identity(persist, bus_root, identity)?;
        *state = staged;
        Ok(())
    }
}

/// Load the host's durable last-good catalog under the same validation policy
/// used by the importer.
///
/// Mutation consumers use the durable cache instead of trusting an arbitrary
/// publication on the shared Bus. Loading re-checks the complete payload
/// contract and validity window, so an expired catalog cannot drive new state.
pub(crate) fn load_admitted_catalog(host: &str, now_ms: u64) -> io::Result<AndroidRuntimeCatalog> {
    let config = load_environment_config(host)?;
    load_last_good(&config, now_ms)?.ok_or_else(|| {
        io::Error::other(format!(
            "no admitted Android release catalog is cached for `{host}`"
        ))
    })
}

fn resolve_bus_root(configured: Option<PathBuf>, user: Option<PathBuf>) -> PathBuf {
    configured
        .or(user)
        .unwrap_or_else(|| PathBuf::from(SYSTEM_BUS_ROOT))
}

fn bus_identity(bus_root: &Path) -> io::Result<BusIdentity> {
    let metadata = fs::metadata(bus_root.join("index.sqlite"))?;
    if !metadata.is_file() {
        return Err(io::Error::other("Bus index is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(BusIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(BusIdentity {})
    }
}

fn verify_bus_identity(
    persist: &Persist,
    bus_root: &Path,
    expected: BusIdentity,
) -> io::Result<()> {
    if bus_identity(bus_root)? != expected {
        return Err(io::Error::other(
            "Android catalog Bus generation changed during transaction",
        ));
    }
    #[cfg(unix)]
    if persist.index_inode() != Some(expected.inode) {
        return Err(io::Error::other(
            "Android catalog Bus handle does not match the live generation",
        ));
    }
    Ok(())
}

#[async_trait::async_trait]
impl Worker for AndroidCatalogWorker {
    fn name(&self) -> &'static str {
        "android_catalog"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        if self.config.is_none() {
            tracing::info!(
                target: "mackesd::android_catalog",
                "Android catalog configuration is unavailable; worker quiescent until shutdown"
            );
            shutdown.wait().await;
            return Ok(());
        }
        let mut state = ImportState::default();
        let mut durable_state_loaded = false;
        loop {
            if !durable_state_loaded {
                match load_last_good(
                    self.config
                        .as_ref()
                        .expect("configured Android catalog worker"),
                    now_unix_ms(),
                ) {
                    Ok(current) => {
                        state.current = current;
                        durable_state_loaded = true;
                    }
                    Err(error) => {
                        tracing::warn!(
                            target: "mackesd::android_catalog",
                            %error,
                            "Android catalog durable state is invalid; importer remains quiescent"
                        );
                        tokio::select! {
                            () = shutdown.wait() => break,
                            () = tokio::time::sleep(self.poll) => {}
                        }
                        continue;
                    }
                }
            }
            let root = self.resolved_bus_root();
            match Persist::open(root.clone()) {
                Ok(mut persist) => {
                    if let Err(error) =
                        self.process_bus_pass(&mut persist, &root, &mut state, now_unix_ms())
                    {
                        tracing::warn!(target: "mackesd::android_catalog", %error, "Android catalog Bus pass deferred");
                    }
                }
                Err(error) => {
                    tracing::warn!(target: "mackesd::android_catalog", %error, "Android catalog Bus unavailable; worker will retry")
                }
            }
            tokio::select! {
                () = shutdown.wait() => break,
                () = tokio::time::sleep(self.poll) => {}
            }
        }
        Ok(())
    }
}

fn load_environment_config(host: &str) -> io::Result<CatalogConfig> {
    android_catalog_state_topic(host).map_err(io_other)?;
    let state_file = std::env::var_os(STATE_FILE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("/var/lib/mackesd/android-catalog").join(format!("{host}.json"))
        });
    Ok(CatalogConfig { state_file })
}

fn load_last_good(
    config: &CatalogConfig,
    now_ms: u64,
) -> io::Result<Option<AndroidRuntimeCatalog>> {
    ensure_directory_chain_nofollow(
        config
            .state_file
            .parent()
            .ok_or_else(|| io::Error::other("Android catalog state path has no parent"))?,
    )?;
    if !config.state_file.exists() {
        return Ok(None);
    }
    let mut file = open_regular_nofollow(&config.state_file, MAX_IMPORT_BYTES as u64)?;
    let mut body = String::new();
    file.read_to_string(&mut body)?;
    let persisted: PersistedCatalog = serde_json::from_str(&body).map_err(io_other)?;
    if persisted.schema_version != CACHE_SCHEMA {
        return Err(io::Error::other("unsupported Android catalog cache schema"));
    }
    persisted.catalog.admit(now_ms).map(Some).map_err(io_other)
}

fn publish_admitted(
    persist: &mut Persist,
    host: &str,
    catalog: &AndroidRuntimeCatalog,
) -> io::Result<()> {
    let topic = android_catalog_state_topic(host).map_err(io_other)?;
    let body = serde_json::to_string(catalog).map_err(io_other)?;
    #[cfg(test)]
    if FAIL_NEXT_PUBLICATION.with(|fail| fail.replace(false)) {
        return Err(io::Error::other(
            "injected Android catalog publication failure",
        ));
    }
    persist
        .write(&topic, Priority::Default, None, Some(&body))
        .map_err(io_other)?;
    Ok(())
}

#[cfg(test)]
std::thread_local! {
    static FAIL_NEXT_PUBLICATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn store_last_good(path: &Path, catalog: &AndroidRuntimeCatalog) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Android catalog state path has no parent"))?;
    ensure_directory_chain_nofollow(parent)?;
    fs::create_dir_all(parent)?;
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(io::Error::other("Android catalog state path is a symlink"));
    }
    let body = serde_json::to_vec(&PersistedCatalog {
        schema_version: CACHE_SCHEMA,
        catalog: catalog.clone(),
    })
    .map_err(io_other)?;
    if body.len() > MAX_IMPORT_BYTES {
        return Err(io::Error::other(
            "admitted Android catalog exceeds persistence bound",
        ));
    }
    let (temp, mut file) = create_cache_temp(parent)?;
    let result = (|| {
        file.write_all(&body)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Open a private staging file without trusting or replacing debris left by a
/// killed importer. PID reuse made the former single staging name a permanent
/// denial of catalog updates after an unclean exit. A bounded suffix search
/// keeps that failure recoverable while preserving create-new/no-follow safety.
fn create_cache_temp(parent: &Path) -> io::Result<(PathBuf, File)> {
    let mut collision = None;
    for attempt in 0..MAX_CACHE_TEMP_ATTEMPTS {
        let path = parent.join(format!(
            ".android-catalog-{}-{attempt}.tmp",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(0o400_000); // Linux O_NOFOLLOW
        }
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                collision = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other(format!(
        "Android catalog cache staging slots are exhausted: {}",
        collision
            .map(|error| error.to_string())
            .unwrap_or_else(|| "no staging slot available".into())
    )))
}

/// Refuse an existing symlink or non-directory anywhere in the cache parent
/// before `create_dir_all` or a cache replay can follow it. The final state
/// file is separately opened/replaced with no-follow semantics.
fn ensure_directory_chain_nofollow(path: &Path) -> io::Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        let Ok(metadata) = fs::symlink_metadata(&current) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other(
                "Android catalog state parent contains a symlink",
            ));
        }
        if !metadata.is_dir() {
            return Err(io::Error::other(
                "Android catalog state parent is not a directory",
            ));
        }
    }
    Ok(())
}

fn open_regular_nofollow(path: &Path, max_bytes: u64) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0o400_000); // Linux O_NOFOLLOW
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err(io::Error::other(
            "Android catalog file is not a bounded regular file",
        ));
    }
    Ok(file)
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn io_other(error: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("{error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::android_apps::{
        AndroidAppCapability, AndroidAppPermission, AndroidCatalogAppPolicy,
        AndroidCatalogGuestReadiness, AndroidCatalogPayload, AndroidImageManifest,
        AndroidImagePackage, AndroidImagePackageManifest, AndroidImageProvenance,
        AndroidPackageVersion, AndroidResourceClass, AndroidResourceProfile, AospStarterApp,
        AospStarterCatalog, ANDROID_RUNTIME_CATALOG_SCHEMA_VERSION,
    };
    use tempfile::TempDir;

    const NOW: u64 = 1_786_000_000_300;

    fn runtime_catalog(revision: u64) -> AndroidRuntimeCatalog {
        runtime_catalog_with_id(revision, "aosp-starter-production")
    }

    fn runtime_catalog_with_id(revision: u64, catalog_id: &str) -> AndroidRuntimeCatalog {
        runtime_catalog_at_with_id(revision, NOW, catalog_id)
    }

    fn runtime_catalog_at(revision: u64, now_ms: u64) -> AndroidRuntimeCatalog {
        runtime_catalog_at_with_id(revision, now_ms, "aosp-starter-production")
    }

    fn runtime_catalog_at_with_id(
        revision: u64,
        now_ms: u64,
        catalog_id: &str,
    ) -> AndroidRuntimeCatalog {
        let image = AndroidImageManifest::new(
            "aosp-cuttlefish-2026-08",
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "aosp-source-2026-08",
            "starter-catalog-v1",
            now_ms - 300,
            now_ms - 200,
            AospStarterCatalog::v1(),
        )
        .unwrap();
        let provenance = AndroidImageProvenance::from_manifest(&image).unwrap();
        let packages = AospStarterApp::ALL
            .into_iter()
            .map(|app| {
                AndroidImagePackage::for_app(
                    app,
                    AndroidPackageVersion::new("2026.08.1", 1).unwrap(),
                )
            })
            .collect();
        let package_manifest = AndroidImagePackageManifest::new(provenance, packages).unwrap();
        let app_policies = AospStarterApp::ALL
            .into_iter()
            .map(|app| AndroidCatalogAppPolicy {
                app,
                permissions: vec![AndroidAppPermission::Network],
                capabilities: vec![
                    AndroidAppCapability::VdiDisplay,
                    AndroidAppCapability::AudioPlayback,
                ],
                resources: AndroidResourceProfile {
                    class: AndroidResourceClass::Standard,
                    vcpus: 4,
                    memory_mib: 4_096,
                    disk_mib: 16_384,
                },
                guest_readiness: AndroidCatalogGuestReadiness::BootedInventoryAndLauncherReady,
            })
            .collect();
        AndroidRuntimeCatalog {
            payload: AndroidCatalogPayload {
                schema_version: ANDROID_RUNTIME_CATALOG_SCHEMA_VERSION,
                catalog_id: catalog_id.into(),
                revision,
                issued_at_unix_ms: now_ms - 100,
                expires_at_unix_ms: now_ms + 60_000,
                image_manifest: image,
                package_manifest,
                app_policies,
            },
        }
    }

    fn worker(temp: &TempDir) -> AndroidCatalogWorker {
        AndroidCatalogWorker {
            host: "node-01".into(),
            bus_root: Some(temp.path().join("bus")),
            config: Some(CatalogConfig {
                state_file: temp.path().join("state/catalog.json"),
            }),
            poll: POLL,
        }
    }

    fn import(persist: &Persist, catalog: &AndroidRuntimeCatalog) {
        persist
            .write(
                &android_catalog_import_topic("node-01").unwrap(),
                Priority::Default,
                None,
                Some(&serde_json::to_string(catalog).unwrap()),
            )
            .unwrap();
    }

    #[test]
    fn imports_and_publishes_only_newer_valid_revisions() {
        let temp = TempDir::new().unwrap();
        let worker = worker(&temp);
        let mut persist = Persist::open(temp.path().join("bus")).unwrap();
        import(&persist, &runtime_catalog(7));
        import(&persist, &runtime_catalog(6));
        let mut cursor = None;
        let mut current = None;
        assert_eq!(
            worker
                .process_once(&mut persist, &mut cursor, &mut current, NOW)
                .unwrap(),
            1
        );
        assert_eq!(current.unwrap().payload.revision, 7);
        assert_eq!(
            persist
                .list_since(&android_catalog_state_topic("node-01").unwrap(), None)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn malformed_or_identity_switching_input_preserves_last_good() {
        let temp = TempDir::new().unwrap();
        let worker = worker(&temp);
        let good = runtime_catalog(7);
        store_last_good(&worker.config.as_ref().unwrap().state_file, &good).unwrap();
        let mut persist = Persist::open(temp.path().join("bus")).unwrap();
        let mut malformed = runtime_catalog(8);
        malformed.payload.app_policies.clear();
        import(&persist, &malformed);
        let mut tampered = runtime_catalog(9);
        tampered.payload.catalog_id = "tampered".into();
        import(&persist, &tampered);
        let mut cursor = None;
        let mut current = Some(good.clone());
        assert_eq!(
            worker
                .process_once(&mut persist, &mut cursor, &mut current, NOW)
                .unwrap(),
            0
        );
        assert_eq!(current, Some(good.clone()));
        assert_eq!(
            load_last_good(worker.config.as_ref().unwrap(), NOW).unwrap(),
            Some(good)
        );
    }

    #[test]
    fn higher_revision_cannot_switch_catalog_identity() {
        let temp = TempDir::new().unwrap();
        let worker = worker(&temp);
        let good = runtime_catalog(7);
        let mut persist = Persist::open(temp.path().join("bus")).unwrap();
        import(&persist, &good);
        import(
            &persist,
            &runtime_catalog_with_id(8, "aosp-alternate-production"),
        );
        let mut cursor = None;
        let mut current = None;
        assert_eq!(
            worker
                .process_once(&mut persist, &mut cursor, &mut current, NOW)
                .unwrap(),
            1
        );
        assert_eq!(current.as_ref(), Some(&good));
        assert!(cursor.is_some(), "refused terminal import advances cursor");
        assert_eq!(
            persist
                .list_since(&android_catalog_state_topic("node-01").unwrap(), None)
                .unwrap()
                .len(),
            1,
            "identity switch must not publish a second catalog"
        );
    }

    #[test]
    fn restart_replays_persisted_valid_catalog_and_refuses_corruption() {
        let temp = TempDir::new().unwrap();
        let worker = worker(&temp);
        let catalog = runtime_catalog(7);
        let path = &worker.config.as_ref().unwrap().state_file;
        store_last_good(path, &catalog).unwrap();
        let replay = load_last_good(worker.config.as_ref().unwrap(), NOW)
            .unwrap()
            .expect("valid restart state");
        let mut persist = Persist::open(temp.path().join("bus")).unwrap();
        publish_admitted(&mut persist, &worker.host, &replay).unwrap();
        assert_eq!(
            persist
                .list_since(&android_catalog_state_topic("node-01").unwrap(), None)
                .unwrap()
                .len(),
            1
        );
        fs::write(path, b"not-json").unwrap();
        assert!(load_last_good(worker.config.as_ref().unwrap(), NOW).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn catalog_cache_rejects_a_symlinked_state_parent() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let worker = worker(&temp);
        let catalog = runtime_catalog(7);
        let real_parent = temp.path().join("real-state");
        let real_path = real_parent.join("catalog.json");
        store_last_good(&real_path, &catalog).unwrap();
        let before = fs::read(&real_path).unwrap();
        let link_parent = temp.path().join("linked-state");
        symlink(&real_parent, &link_parent).unwrap();
        let linked_path = link_parent.join("catalog.json");
        let linked_config = CatalogConfig {
            state_file: linked_path.clone(),
            ..worker.config.as_ref().unwrap().clone()
        };

        assert!(load_last_good(&linked_config, NOW).is_err());
        assert!(store_last_good(&linked_path, &catalog).is_err());
        assert_eq!(fs::read(&real_path).unwrap(), before);
        let real_config = CatalogConfig {
            state_file: real_path,
            ..worker.config.as_ref().unwrap().clone()
        };
        assert_eq!(load_last_good(&real_config, NOW).unwrap(), Some(catalog));
    }

    #[test]
    fn transient_side_effect_failure_keeps_catalog_import_retryable() {
        let temp = TempDir::new().unwrap();
        let worker = worker(&temp);
        let state_parent = worker.config.as_ref().unwrap().state_file.parent().unwrap();
        fs::write(state_parent, b"hostile non-directory").unwrap();

        let mut persist = Persist::open(temp.path().join("bus")).unwrap();
        import(&persist, &runtime_catalog(7));
        let mut cursor = None;
        let mut current = None;
        assert!(worker
            .process_once(&mut persist, &mut cursor, &mut current, NOW)
            .is_err());
        assert_eq!(cursor, None, "failed governed effects must not acknowledge");
        assert_eq!(current, None);

        fs::remove_file(state_parent).unwrap();
        assert_eq!(
            worker
                .process_once(&mut persist, &mut cursor, &mut current, NOW)
                .unwrap(),
            1
        );
        assert!(cursor.is_some(), "successful retry acknowledges the import");
        assert_eq!(current.unwrap().payload.revision, 7);
        assert_eq!(
            persist
                .list_since(&android_catalog_state_topic("node-01").unwrap(), None)
                .unwrap()
                .len(),
            1,
            "retry publishes exactly once"
        );
    }

    #[test]
    fn stale_cache_staging_file_cannot_wedge_catalog_updates() {
        let temp = TempDir::new().unwrap();
        let worker = worker(&temp);
        let path = &worker.config.as_ref().unwrap().state_file;
        let parent = path.parent().unwrap();
        fs::create_dir_all(parent).unwrap();
        let stale = parent.join(format!(".android-catalog-{}-0.tmp", std::process::id()));
        fs::write(&stale, b"incomplete catalog from killed importer").unwrap();

        let catalog = runtime_catalog(7);
        store_last_good(path, &catalog).unwrap();

        assert_eq!(
            load_last_good(worker.config.as_ref().unwrap(), NOW).unwrap(),
            Some(catalog),
            "stale staging debris must not prevent corrected-forward catalog authority"
        );
        assert_eq!(
            fs::read(stale).unwrap(),
            b"incomplete catalog from killed importer",
            "the importer must neither trust nor overwrite stale staging content"
        );
    }

    #[test]
    fn replay_and_import_publication_failures_preserve_state_for_retry() {
        let temp = TempDir::new().unwrap();
        let worker = worker(&temp);
        let bus_root = temp.path().join("bus");
        let mut persist = Persist::open(bus_root.clone()).unwrap();
        let catalog_7 = runtime_catalog(7);
        store_last_good(&worker.config.as_ref().unwrap().state_file, &catalog_7).unwrap();
        let mut state = ImportState {
            current: Some(catalog_7),
            ..ImportState::default()
        };

        FAIL_NEXT_PUBLICATION.with(|fail| fail.set(true));
        assert!(worker
            .process_bus_pass(&mut persist, &bus_root, &mut state, NOW)
            .is_err());
        assert!(!state.published_replay);
        assert!(state.cursor.is_none());
        assert_eq!(state.current.as_ref().unwrap().payload.revision, 7);
        assert!(state.bus_identity.is_none());

        worker
            .process_bus_pass(&mut persist, &bus_root, &mut state, NOW)
            .unwrap();
        assert!(state.published_replay);
        assert!(state.bus_identity.is_some());

        import(&persist, &runtime_catalog(8));
        FAIL_NEXT_PUBLICATION.with(|fail| fail.set(true));
        assert!(worker
            .process_bus_pass(&mut persist, &bus_root, &mut state, NOW)
            .is_err());
        assert!(
            state.cursor.is_none(),
            "failed import remains unacknowledged"
        );
        assert_eq!(state.current.as_ref().unwrap().payload.revision, 7);
        assert_eq!(
            load_last_good(worker.config.as_ref().unwrap(), NOW)
                .unwrap()
                .unwrap()
                .payload
                .revision,
            8,
            "cache-first ordering preserves crash recovery authority"
        );

        worker
            .process_bus_pass(&mut persist, &bus_root, &mut state, NOW)
            .unwrap();
        assert!(state.cursor.is_some());
        assert_eq!(state.current.as_ref().unwrap().payload.revision, 8);
        assert_eq!(
            persist
                .list_since(&android_catalog_state_topic("node-01").unwrap(), None)
                .unwrap()
                .len(),
            2,
            "one replay and one corrected-forward import publication"
        );
    }

    #[test]
    fn expired_catalog_cannot_replay_into_replaced_bus() {
        let temp = TempDir::new().unwrap();
        let worker = worker(&temp);
        let bus_root = temp.path().join("replacement-bus");
        let mut persist = Persist::open(bus_root.clone()).unwrap();
        let expired = runtime_catalog(7);
        let mut state = ImportState {
            current: Some(expired.clone()),
            ..ImportState::default()
        };
        let after_expiry = expired.payload.expires_at_unix_ms + 1;

        worker
            .process_bus_pass(&mut persist, &bus_root, &mut state, after_expiry)
            .unwrap();

        assert!(
            persist
                .read_latest(&android_catalog_state_topic("node-01").unwrap())
                .unwrap()
                .is_none(),
            "replacement Bus must not receive an expired catalog replay"
        );
        assert_eq!(
            state.current.as_ref().unwrap().payload.revision,
            7,
            "expired authority remains only as the anti-rollback revision anchor"
        );

        import(&persist, &runtime_catalog_at(8, after_expiry));
        worker
            .process_bus_pass(&mut persist, &bus_root, &mut state, after_expiry)
            .unwrap();
        assert_eq!(
            state.current.as_ref().unwrap().payload.revision,
            8,
            "a freshly admitted successor must restore publication authority"
        );
    }

    #[test]
    fn replacement_after_open_cannot_strand_catalog_replay_on_retired_index() {
        let temp = TempDir::new().unwrap();
        let worker = worker(&temp);
        let bus_root = temp.path().join("bus");
        let mut retired = Persist::open(bus_root.clone()).unwrap();
        let catalog = runtime_catalog(7);
        let mut state = ImportState {
            current: Some(catalog),
            ..ImportState::default()
        };

        let replacement_root = temp.path().join("replacement-bus");
        drop(Persist::open(replacement_root.clone()).unwrap());
        fs::rename(
            replacement_root.join("index.sqlite"),
            bus_root.join("index.sqlite"),
        )
        .unwrap();

        worker
            .process_bus_pass(&mut retired, &bus_root, &mut state, NOW)
            .unwrap();

        let replacement = Persist::open(bus_root).unwrap();
        assert!(
            replacement
                .read_latest(&android_catalog_state_topic("node-01").unwrap())
                .unwrap()
                .is_some(),
            "catalog replay must follow the replacement index opened after the stale handle"
        );
    }

    async fn wait_for_revision(bus_root: &Path, revision: u64) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(persist) = Persist::open(bus_root.to_path_buf()) {
                    if let Ok(Some(row)) =
                        persist.read_latest(&android_catalog_state_topic("node-01").unwrap())
                    {
                        if row.body.as_deref().is_some_and(|body| {
                            serde_json::from_str::<AndroidRuntimeCatalog>(body)
                                .is_ok_and(|catalog| catalog.payload.revision == revision)
                        }) {
                            break;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("timed out waiting for Android catalog revision");
    }

    #[tokio::test]
    async fn same_worker_recovers_late_and_replaced_bus_with_governed_replay() {
        let temp = TempDir::new().unwrap();
        let bus_root = temp.path().join("late-bus");
        fs::write(&bus_root, b"temporarily unavailable").unwrap();

        let wall_now = now_unix_ms();
        let seeded_root = temp.path().join("seeded-bus");
        let seeded = Persist::open(seeded_root.clone()).unwrap();
        import(&seeded, &runtime_catalog_at(7, wall_now));
        drop(seeded);

        let mut worker = worker(&temp).with_poll(Duration::from_millis(10));
        worker.bus_root = Some(bus_root.clone());
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            !task.is_finished(),
            "unopenable Bus must not terminate worker"
        );

        fs::remove_file(&bus_root).unwrap();
        fs::rename(&seeded_root, &bus_root).unwrap();
        wait_for_revision(&bus_root, 7).await;

        let replacement_root = temp.path().join("replacement-bus");
        drop(Persist::open(replacement_root.clone()).unwrap());
        fs::rename(
            replacement_root.join("index.sqlite"),
            bus_root.join("index.sqlite"),
        )
        .unwrap();
        wait_for_revision(&bus_root, 7).await;

        let live = Persist::open(bus_root.clone()).unwrap();
        import(&live, &runtime_catalog_at(8, wall_now));
        wait_for_revision(&bus_root, 8).await;

        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("worker shutdown timeout")
            .expect("worker task joins")
            .expect("worker shutdown succeeds");
        assert_eq!(
            resolve_bus_root(None, None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
    }

    #[tokio::test]
    async fn corrupt_restart_cache_cannot_erase_catalog_identity_authority() {
        let temp = TempDir::new().unwrap();
        let wall_now = now_unix_ms();
        let bus_root = temp.path().join("bus");
        let persist = Persist::open(bus_root.clone()).unwrap();
        import(
            &persist,
            &runtime_catalog_at_with_id(8, wall_now, "aosp-hostile-alternate"),
        );
        drop(persist);

        let mut worker = worker(&temp).with_poll(Duration::from_millis(10));
        worker.bus_root = Some(bus_root.clone());
        let state_file = worker.config.as_ref().unwrap().state_file.clone();
        fs::create_dir_all(state_file.parent().unwrap()).unwrap();
        fs::write(&state_file, b"corrupt durable authority").unwrap();

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );
        tokio::time::sleep(Duration::from_millis(40)).await;

        let persist = Persist::open(bus_root.clone()).unwrap();
        assert!(
            persist
                .read_latest(&android_catalog_state_topic("node-01").unwrap())
                .unwrap()
                .is_none(),
            "invalid durable authority must block Bus replay"
        );
        assert_eq!(
            fs::read(&state_file).unwrap(),
            b"corrupt durable authority",
            "fail-closed startup must not replace the identity anchor from Bus history"
        );

        let baseline = runtime_catalog_at(7, wall_now);
        store_last_good(&state_file, &baseline).unwrap();
        wait_for_revision(&bus_root, 7).await;
        assert_eq!(
            load_last_good(&CatalogConfig { state_file }, wall_now,).unwrap(),
            Some(baseline),
            "repair restores the original identity; the alternate stays refused"
        );

        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("worker shutdown timeout")
            .expect("worker task joins")
            .expect("worker shutdown succeeds");
    }

    #[tokio::test]
    async fn unconfigured_worker_quiesces_without_creating_bus_state() {
        let temp = TempDir::new().unwrap();
        let bus_root = temp.path().join("bus");
        let mut worker = AndroidCatalogWorker {
            host: "node-01".into(),
            bus_root: Some(bus_root.clone()),
            config: None,
            poll: POLL,
        };
        let (tx, rx) = tokio::sync::watch::channel(false);
        let handle =
            tokio::spawn(async move { worker.run(ShutdownToken::from_receiver(rx)).await });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !bus_root.exists(),
            "an unconfigured worker must not open Bus state"
        );
        assert!(
            !handle.is_finished(),
            "the quiescent worker waits for shutdown"
        );

        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("quiescent worker exits promptly")
            .expect("worker task joins")
            .expect("worker shutdown succeeds");
    }
}
