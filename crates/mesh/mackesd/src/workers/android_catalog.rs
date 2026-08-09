//! WL-FUNC-020 S1 — signed Android catalog runtime importer.
//!
//! The release signer stays offline.  This worker reads one locally provisioned
//! Ed25519 *public* trust anchor, drains the node-scoped typed import topic, and
//! publishes only catalogs that pass the shared contract's complete admission.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::VerifyingKey;
use mackes_mesh_types::android_apps::{
    android_catalog_import_topic, android_catalog_state_topic, AndroidSignedCatalog,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};

const POLL: Duration = Duration::from_secs(1);
const MAX_IMPORT_BYTES: usize = 2 * 1024 * 1024;
const MAX_TRUST_BYTES: u64 = 256;
const MAX_ROWS_PER_POLL: usize = 32;
const CACHE_SCHEMA: u16 = 1;

const SIGNER_ID_ENV: &str = "MDE_ANDROID_CATALOG_SIGNER_ID";
const TRUST_KEY_ENV: &str = "MDE_ANDROID_CATALOG_TRUST_KEY_FILE";
const STATE_FILE_ENV: &str = "MDE_ANDROID_CATALOG_STATE_FILE";

#[derive(Debug, Clone)]
struct CatalogConfig {
    signer_id: String,
    verifying_key: VerifyingKey,
    state_file: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedCatalog {
    schema_version: u16,
    catalog: AndroidSignedCatalog,
}

/// Workstation runtime authority for one node's signed Android catalog.
pub struct AndroidCatalogWorker {
    host: String,
    bus_root: Option<PathBuf>,
    config: Option<CatalogConfig>,
}

impl AndroidCatalogWorker {
    /// Build from the production environment contract. Missing or invalid trust
    /// configuration leaves the worker alive but fail-closed and non-publishing.
    #[must_use]
    pub fn new(host: String) -> Self {
        let config = load_environment_config(&host)
            .map_err(|error| {
                tracing::warn!(target: "mackesd::android_catalog", %error, "Android catalog trust is unavailable; importer is fail-closed");
                error
            })
            .ok();
        Self {
            host,
            bus_root: crate::bus_publish::default_bus_root(),
            config,
        }
    }

    fn process_once(
        &self,
        persist: &mut Persist,
        cursor: &mut Option<String>,
        current: &mut Option<AndroidSignedCatalog>,
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
            *cursor = Some(row.ulid);
            let Some(body) = row.body else {
                continue;
            };
            if body.len() > MAX_IMPORT_BYTES {
                tracing::warn!(target: "mackesd::android_catalog", "refused oversized Android catalog import");
                continue;
            }
            let candidate = match serde_json::from_str::<AndroidSignedCatalog>(&body) {
                Ok(candidate) => candidate,
                Err(error) => {
                    tracing::warn!(target: "mackesd::android_catalog", %error, "refused malformed Android catalog import");
                    continue;
                }
            };
            let candidate = match candidate.admit(&config.signer_id, &config.verifying_key, now_ms)
            {
                Ok(candidate) => candidate,
                Err(error) => {
                    tracing::warn!(target: "mackesd::android_catalog", ?error, "refused untrusted Android catalog import");
                    continue;
                }
            };
            if current
                .as_ref()
                .is_some_and(|existing| candidate.payload.revision <= existing.payload.revision)
            {
                tracing::warn!(target: "mackesd::android_catalog", revision = candidate.payload.revision, "refused stale Android catalog revision");
                continue;
            }
            store_last_good(&config.state_file, &candidate)?;
            publish_admitted(persist, &self.host, &candidate)?;
            *current = Some(candidate);
            admitted += 1;
        }
        Ok(admitted)
    }
}

#[async_trait::async_trait]
impl Worker for AndroidCatalogWorker {
    fn name(&self) -> &'static str {
        "android_catalog"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut cursor = None;
        let mut current = self
            .config
            .as_ref()
            .and_then(|config| load_last_good(config, now_unix_ms()).ok().flatten());
        let mut published_replay = false;
        loop {
            if let (Some(root), Some(config)) = (self.bus_root.as_ref(), self.config.as_ref()) {
                match Persist::open(root.clone()) {
                    Ok(mut persist) => {
                        if !published_replay {
                            if let Some(catalog) = current.as_ref() {
                                publish_admitted(&mut persist, &self.host, catalog)?;
                            }
                            published_replay = true;
                        }
                        if let Err(error) = self.process_once(
                            &mut persist,
                            &mut cursor,
                            &mut current,
                            now_unix_ms(),
                        ) {
                            tracing::warn!(target: "mackesd::android_catalog", %error, state = %config.state_file.display(), "Android catalog import pass failed");
                        }
                    }
                    Err(error) => {
                        tracing::warn!(target: "mackesd::android_catalog", %error, "Android catalog Bus unavailable")
                    }
                }
            }
            tokio::select! {
                () = shutdown.wait() => break,
                () = tokio::time::sleep(POLL) => {}
            }
        }
        Ok(())
    }
}

fn load_environment_config(host: &str) -> io::Result<CatalogConfig> {
    android_catalog_state_topic(host).map_err(io_other)?;
    let signer_id = std::env::var(SIGNER_ID_ENV)
        .map_err(|_| io::Error::other(format!("{SIGNER_ID_ENV} is unset")))?;
    let key_path = std::env::var_os(TRUST_KEY_ENV)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other(format!("{TRUST_KEY_ENV} is unset")))?;
    let verifying_key = load_verifying_key(&key_path)?;
    let state_file = std::env::var_os(STATE_FILE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("/var/lib/mackesd/android-catalog").join(format!("{host}.json"))
        });
    Ok(CatalogConfig {
        signer_id,
        verifying_key,
        state_file,
    })
}

fn load_verifying_key(path: &Path) -> io::Result<VerifyingKey> {
    let mut file = open_regular_nofollow(path, MAX_TRUST_BYTES)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if file.metadata()?.mode() & 0o022 != 0 {
            return Err(io::Error::other(
                "Android catalog trust key is group/world writable",
            ));
        }
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let bytes = decode_hex_32(text.trim()).ok_or_else(|| {
        io::Error::other("Android catalog trust key must be 64 lowercase hex characters")
    })?;
    VerifyingKey::from_bytes(&bytes).map_err(io_other)
}

fn load_last_good(config: &CatalogConfig, now_ms: u64) -> io::Result<Option<AndroidSignedCatalog>> {
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
    persisted
        .catalog
        .admit(&config.signer_id, &config.verifying_key, now_ms)
        .map(Some)
        .map_err(io_other)
}

fn publish_admitted(
    persist: &mut Persist,
    host: &str,
    catalog: &AndroidSignedCatalog,
) -> io::Result<()> {
    let topic = android_catalog_state_topic(host).map_err(io_other)?;
    let body = serde_json::to_string(catalog).map_err(io_other)?;
    persist
        .write(&topic, Priority::Default, None, Some(&body))
        .map_err(io_other)?;
    Ok(())
}

fn store_last_good(path: &Path, catalog: &AndroidSignedCatalog) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Android catalog state path has no parent"))?;
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
    let temp = parent.join(format!(".android-catalog-{}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(0o400000); // Linux O_NOFOLLOW
    }
    let mut file = options.open(&temp)?;
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

fn open_regular_nofollow(path: &Path, max_bytes: u64) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0o400000); // Linux O_NOFOLLOW
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

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let mut out = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        out[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Some(out)
}

const fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
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
    use ed25519_dalek::SigningKey;
    use mackes_mesh_types::android_apps::{
        AndroidAppCapability, AndroidAppPermission, AndroidCatalogAppPolicy,
        AndroidCatalogGuestReadiness, AndroidCatalogPayload, AndroidImageManifest,
        AndroidImagePackage, AndroidImagePackageManifest, AndroidImageProvenance,
        AndroidPackageVersion, AndroidResourceClass, AndroidResourceProfile, AospStarterApp,
        AospStarterCatalog, ANDROID_SIGNED_CATALOG_SCHEMA_VERSION,
    };
    use tempfile::TempDir;

    const NOW: u64 = 1_786_000_000_300;

    fn signed_catalog(key: &SigningKey, revision: u64) -> AndroidSignedCatalog {
        let image = AndroidImageManifest::new(
            "aosp-cuttlefish-2026-08",
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "aosp-source-2026-08",
            "starter-catalog-v1",
            NOW - 300,
            NOW - 200,
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
        AndroidSignedCatalog::sign(
            "android-release-v1",
            AndroidCatalogPayload {
                schema_version: ANDROID_SIGNED_CATALOG_SCHEMA_VERSION,
                catalog_id: "aosp-starter-production".into(),
                revision,
                issued_at_unix_ms: NOW - 100,
                expires_at_unix_ms: NOW + 60_000,
                image_manifest: image,
                package_manifest,
                app_policies,
            },
            key,
        )
        .unwrap()
    }

    fn worker(temp: &TempDir, key: &SigningKey) -> AndroidCatalogWorker {
        AndroidCatalogWorker {
            host: "node-01".into(),
            bus_root: Some(temp.path().join("bus")),
            config: Some(CatalogConfig {
                signer_id: "android-release-v1".into(),
                verifying_key: key.verifying_key(),
                state_file: temp.path().join("state/catalog.json"),
            }),
        }
    }

    fn import(persist: &Persist, catalog: &AndroidSignedCatalog) {
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
        let key = SigningKey::from_bytes(&[7; 32]);
        let worker = worker(&temp, &key);
        let mut persist = Persist::open(temp.path().join("bus")).unwrap();
        import(&persist, &signed_catalog(&key, 7));
        import(&persist, &signed_catalog(&key, 6));
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
    fn tampered_or_untrusted_input_preserves_last_good() {
        let temp = TempDir::new().unwrap();
        let key = SigningKey::from_bytes(&[7; 32]);
        let hostile_key = SigningKey::from_bytes(&[8; 32]);
        let worker = worker(&temp, &key);
        let good = signed_catalog(&key, 7);
        store_last_good(&worker.config.as_ref().unwrap().state_file, &good).unwrap();
        let mut persist = Persist::open(temp.path().join("bus")).unwrap();
        import(&persist, &signed_catalog(&hostile_key, 8));
        let mut tampered = signed_catalog(&key, 9);
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
    fn restart_replays_persisted_valid_catalog_and_refuses_corruption() {
        let temp = TempDir::new().unwrap();
        let key = SigningKey::from_bytes(&[7; 32]);
        let worker = worker(&temp, &key);
        let catalog = signed_catalog(&key, 7);
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
}
