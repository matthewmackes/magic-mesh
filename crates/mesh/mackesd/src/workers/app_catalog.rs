//! WL-FUNC-018 S2 — production signed Flatpak catalog importer.
//!
//! Registration note: after the concurrent Clock registration work lands, add
//! `pub mod app_catalog;` to `workers/mod.rs`, add an `app_catalog` role entry to
//! `worker_role.rs`, and spawn `AppCatalogWorker::new(host)` from `spawn.rs` for
//! workstation roles. No other module or store writer is required.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::VerifyingKey;
use mackes_mesh_types::app_catalog::{
    FlatpakInstallState, SignedFlatpakAppCatalog, SignedFlatpakCatalogEntry,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::{Deserialize, Serialize};

use crate::workers::{ShutdownToken, Worker};

const POLL: Duration = Duration::from_secs(1);
const MAX_TRUST_KEY_BYTES: u64 = 256;
const MAX_CATALOG_WIRE_BYTES: u64 = 512 * 1024;
const MAX_IMPORTS_PER_POLL: usize = 32;
const PROJECTION_SCHEMA_VERSION: u16 = 1;
const STATUS_SCHEMA_VERSION: u16 = 1;
const PRODUCTION_OWNER_UID: u32 = 0;

const SIGNER_ID_ENV: &str = "MDE_FLATPAK_CATALOG_SIGNER_ID";
const TRUST_KEY_ENV: &str = "MDE_FLATPAK_CATALOG_TRUST_KEY_FILE";
const LAST_GOOD_ENV: &str = "MDE_FLATPAK_CATALOG_LAST_GOOD_FILE";

/// Bus projection containing only signed, admitted, installed application rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedFlatpakCatalogProjection {
    pub schema_version: u16,
    pub host: String,
    pub catalog_id: String,
    pub revision: u64,
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub content_digest: String,
    pub provider_id: String,
    pub repository_id: String,
    pub entries: Vec<AdmittedFlatpakAppProjection>,
}

/// One locator-free row projected to catalog consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedFlatpakAppProjection {
    pub app_id: String,
    pub display_name: String,
    pub summary: String,
    pub version: String,
    pub icon_id: String,
    pub permissions: Vec<String>,
    pub guest_profile: String,
    pub supported_actions: Vec<String>,
    pub search_terms: Vec<String>,
    pub search_weight: u16,
}

/// Closed outcome vocabulary. It cannot echo rejected payloads or secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCatalogImportOutcome {
    Admitted,
    IdempotentReplay,
    Refused,
    Recovered,
    Unavailable,
}

/// Closed refusal reason suitable for an operator-facing status surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCatalogStatusReason {
    None,
    TrustUnavailable,
    MissingPayload,
    AdmissionFailed,
    CatalogIdentityChanged,
    RevisionRollback,
    RevisionConflict,
    PersistenceFailed,
    StartupRecoveryFailed,
    RetainedCatalogExpired,
}

/// Closed next action; no arbitrary command, path, URL, or text is projected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCatalogRemedy {
    None,
    ConfigureTrust,
    PublishValidCatalog,
    PublishNewerRevision,
    RepairStateStorage,
}

/// Payload-free import health projection. Last-good identity is safe metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppCatalogImportStatus {
    pub schema_version: u16,
    pub host: String,
    pub outcome: AppCatalogImportOutcome,
    pub reason: AppCatalogStatusReason,
    pub remedy: AppCatalogRemedy,
    pub retained_catalog_id: Option<String>,
    pub retained_revision: Option<u64>,
    pub observed_at_unix_ms: u64,
}

#[derive(Debug, Clone)]
struct CatalogConfig {
    signer_id: String,
    verifying_key: VerifyingKey,
    last_good_file: PathBuf,
    required_owner_uid: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ImportTally {
    admitted: usize,
    replayed: usize,
    refused: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogWatermark {
    catalog_id: String,
    revision: u64,
    content_digest: String,
}

impl CatalogWatermark {
    fn from_catalog(catalog: &SignedFlatpakAppCatalog) -> io::Result<Self> {
        Ok(Self {
            catalog_id: catalog.payload.catalog_id.clone(),
            revision: catalog.payload.revision,
            content_digest: catalog.payload.content_digest().map_err(io_other)?,
        })
    }
}

struct RecoveredCatalog {
    watermark: CatalogWatermark,
    catalog: SignedFlatpakAppCatalog,
    is_fresh: bool,
}

/// Workstation authority for one node's signed Flatpak catalog.
pub struct AppCatalogWorker {
    host: String,
    bus_root: Option<PathBuf>,
    config: Option<CatalogConfig>,
}

impl AppCatalogWorker {
    /// Build from root-owned production trust configuration.
    #[must_use]
    pub fn new(host: String) -> Self {
        let config = load_environment_config(&host)
            .map_err(|error| {
                tracing::warn!(target: "mackesd::app_catalog", %error, "Flatpak catalog trust unavailable; importer is fail-closed");
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
        current: &mut Option<SignedFlatpakAppCatalog>,
        watermark: &mut Option<CatalogWatermark>,
        last_status: &mut Option<AppCatalogImportStatus>,
        now_ms: u64,
    ) -> io::Result<ImportTally> {
        if current
            .as_ref()
            .is_some_and(|catalog| now_ms >= catalog.payload.expires_at_unix_ms)
        {
            let expired = current.take().expect("expired catalog exists");
            publish_empty_projection(persist, &self.host, &expired)?;
            publish_status_if_changed(
                persist,
                &status_for(
                    &self.host,
                    AppCatalogImportOutcome::Unavailable,
                    AppCatalogStatusReason::RetainedCatalogExpired,
                    AppCatalogRemedy::PublishNewerRevision,
                    watermark.as_ref(),
                    now_ms,
                ),
                last_status,
            )?;
        }
        let Some(config) = self.config.as_ref() else {
            publish_status_if_changed(
                persist,
                &status_for(
                    &self.host,
                    AppCatalogImportOutcome::Unavailable,
                    AppCatalogStatusReason::TrustUnavailable,
                    AppCatalogRemedy::ConfigureTrust,
                    watermark.as_ref(),
                    now_ms,
                ),
                last_status,
            )?;
            return Ok(ImportTally::default());
        };

        let rows = persist
            .list_since_limit(
                &app_catalog_import_topic(&self.host)?,
                cursor.as_deref(),
                MAX_IMPORTS_PER_POLL,
            )
            .map_err(io_other)?;
        let mut tally = ImportTally::default();
        for row in rows {
            *cursor = Some(row.ulid);
            let Some(body) = row.body else {
                tally.refused += 1;
                publish_status_if_changed(
                    persist,
                    &status_for(
                        &self.host,
                        AppCatalogImportOutcome::Refused,
                        AppCatalogStatusReason::MissingPayload,
                        AppCatalogRemedy::PublishValidCatalog,
                        watermark.as_ref(),
                        now_ms,
                    ),
                    last_status,
                )?;
                continue;
            };

            let candidate = match SignedFlatpakAppCatalog::decode_and_admit_json(
                body.as_bytes(),
                &config.signer_id,
                &config.verifying_key,
                now_ms,
            ) {
                Ok(candidate) => candidate,
                Err(error) => {
                    tracing::warn!(target: "mackesd::app_catalog", ?error, "refused Flatpak catalog admission");
                    tally.refused += 1;
                    publish_status_if_changed(
                        persist,
                        &status_for(
                            &self.host,
                            AppCatalogImportOutcome::Refused,
                            AppCatalogStatusReason::AdmissionFailed,
                            AppCatalogRemedy::PublishValidCatalog,
                            watermark.as_ref(),
                            now_ms,
                        ),
                        last_status,
                    )?;
                    continue;
                }
            };
            let candidate_digest = candidate.payload.content_digest().map_err(io_other)?;

            if let Some(existing) = watermark.as_ref() {
                if candidate.payload.catalog_id != existing.catalog_id {
                    tally.refused += 1;
                    publish_status_if_changed(
                        persist,
                        &status_for(
                            &self.host,
                            AppCatalogImportOutcome::Refused,
                            AppCatalogStatusReason::CatalogIdentityChanged,
                            AppCatalogRemedy::PublishValidCatalog,
                            watermark.as_ref(),
                            now_ms,
                        ),
                        last_status,
                    )?;
                    continue;
                }
                if candidate.payload.revision < existing.revision {
                    tally.refused += 1;
                    publish_status_if_changed(
                        persist,
                        &status_for(
                            &self.host,
                            AppCatalogImportOutcome::Refused,
                            AppCatalogStatusReason::RevisionRollback,
                            AppCatalogRemedy::PublishNewerRevision,
                            watermark.as_ref(),
                            now_ms,
                        ),
                        last_status,
                    )?;
                    continue;
                }
                if candidate.payload.revision == existing.revision {
                    if candidate_digest == existing.content_digest {
                        tally.replayed += 1;
                        publish_status_if_changed(
                            persist,
                            &status_for(
                                &self.host,
                                AppCatalogImportOutcome::IdempotentReplay,
                                AppCatalogStatusReason::None,
                                AppCatalogRemedy::None,
                                watermark.as_ref(),
                                now_ms,
                            ),
                            last_status,
                        )?;
                    } else {
                        tally.refused += 1;
                        publish_status_if_changed(
                            persist,
                            &status_for(
                                &self.host,
                                AppCatalogImportOutcome::Refused,
                                AppCatalogStatusReason::RevisionConflict,
                                AppCatalogRemedy::PublishNewerRevision,
                                watermark.as_ref(),
                                now_ms,
                            ),
                            last_status,
                        )?;
                    }
                    continue;
                }
            }

            if let Err(error) = store_last_good(config, &candidate) {
                tracing::warn!(target: "mackesd::app_catalog", %error, "refused Flatpak catalog because last-good persistence failed");
                tally.refused += 1;
                publish_status_if_changed(
                    persist,
                    &status_for(
                        &self.host,
                        AppCatalogImportOutcome::Refused,
                        AppCatalogStatusReason::PersistenceFailed,
                        AppCatalogRemedy::RepairStateStorage,
                        watermark.as_ref(),
                        now_ms,
                    ),
                    last_status,
                )?;
                continue;
            }

            *watermark = Some(CatalogWatermark::from_catalog(&candidate)?);
            *current = Some(candidate);
            publish_projection(persist, &self.host, current.as_ref().expect("catalog set"))?;
            publish_status_if_changed(
                persist,
                &status_for(
                    &self.host,
                    AppCatalogImportOutcome::Admitted,
                    AppCatalogStatusReason::None,
                    AppCatalogRemedy::None,
                    watermark.as_ref(),
                    now_ms,
                ),
                last_status,
            )?;
            tally.admitted += 1;
        }
        Ok(tally)
    }
}

#[async_trait::async_trait]
impl Worker for AppCatalogWorker {
    fn name(&self) -> &'static str {
        "app_catalog"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // No signed catalog can be admitted without the locally provisioned
        // public trust anchor. Stay fully quiescent instead of opening Bus
        // state and waking every second to repeat the same unavailable result.
        if self.config.is_none() {
            shutdown.wait().await;
            return Ok(());
        }
        let mut cursor = None;
        let mut current = None;
        let mut watermark = None;
        let mut last_status = None;
        let mut recovery_attempted = false;
        loop {
            if let Some(root) = self.bus_root.as_ref() {
                match Persist::open(root.clone()) {
                    Ok(mut persist) => {
                        if !recovery_attempted {
                            recovery_attempted = true;
                            if let Some(config) = self.config.as_ref() {
                                match recover_last_good(config, now_unix_ms()) {
                                    Ok(Some(recovered)) => {
                                        watermark = Some(recovered.watermark);
                                        if recovered.is_fresh {
                                            current = Some(recovered.catalog);
                                        } else {
                                            publish_empty_projection(
                                                &mut persist,
                                                &self.host,
                                                &recovered.catalog,
                                            )?;
                                        }
                                        if let Some(catalog) = current.as_ref() {
                                            publish_projection(&mut persist, &self.host, catalog)?;
                                            publish_status_if_changed(
                                                &mut persist,
                                                &status_for(
                                                    &self.host,
                                                    AppCatalogImportOutcome::Recovered,
                                                    AppCatalogStatusReason::None,
                                                    AppCatalogRemedy::None,
                                                    watermark.as_ref(),
                                                    now_unix_ms(),
                                                ),
                                                &mut last_status,
                                            )?;
                                        } else {
                                            publish_status_if_changed(
                                                &mut persist,
                                                &status_for(
                                                    &self.host,
                                                    AppCatalogImportOutcome::Unavailable,
                                                    AppCatalogStatusReason::RetainedCatalogExpired,
                                                    AppCatalogRemedy::PublishNewerRevision,
                                                    watermark.as_ref(),
                                                    now_unix_ms(),
                                                ),
                                                &mut last_status,
                                            )?;
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(error) => {
                                        tracing::warn!(target: "mackesd::app_catalog", %error, "Flatpak catalog last-good recovery refused");
                                        publish_status_if_changed(
                                            &mut persist,
                                            &status_for(
                                                &self.host,
                                                AppCatalogImportOutcome::Refused,
                                                AppCatalogStatusReason::StartupRecoveryFailed,
                                                AppCatalogRemedy::PublishValidCatalog,
                                                None,
                                                now_unix_ms(),
                                            ),
                                            &mut last_status,
                                        )?;
                                    }
                                }
                            }
                        }
                        if let Err(error) = self.process_once(
                            &mut persist,
                            &mut cursor,
                            &mut current,
                            &mut watermark,
                            &mut last_status,
                            now_unix_ms(),
                        ) {
                            tracing::warn!(target: "mackesd::app_catalog", %error, "Flatpak catalog import pass failed");
                        }
                    }
                    Err(error) => {
                        tracing::warn!(target: "mackesd::app_catalog", %error, "Flatpak catalog Bus unavailable")
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
    validate_host(host)?;
    let signer_id = std::env::var(SIGNER_ID_ENV)
        .map_err(|_| io::Error::other(format!("{SIGNER_ID_ENV} is unset")))?;
    let trust_path = std::env::var_os(TRUST_KEY_ENV)
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other(format!("{TRUST_KEY_ENV} is unset")))?;
    let verifying_key = load_verifying_key(&trust_path, PRODUCTION_OWNER_UID)?;
    let last_good_file = std::env::var_os(LAST_GOOD_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("/var/lib/mackesd/flatpak-catalog").join(format!("{host}.json"))
        });
    Ok(CatalogConfig {
        signer_id,
        verifying_key,
        last_good_file,
        required_owner_uid: PRODUCTION_OWNER_UID,
    })
}

fn load_verifying_key(path: &Path, required_owner_uid: u32) -> io::Result<VerifyingKey> {
    let mut file = open_secure_regular_nofollow(path, MAX_TRUST_KEY_BYTES, required_owner_uid)?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let bytes = decode_hex_32(text.trim()).ok_or_else(|| {
        io::Error::other("Flatpak catalog trust key must be 64 lowercase hex characters")
    })?;
    VerifyingKey::from_bytes(&bytes).map_err(io_other)
}

fn recover_last_good(config: &CatalogConfig, now_ms: u64) -> io::Result<Option<RecoveredCatalog>> {
    match open_secure_regular_nofollow(
        &config.last_good_file,
        MAX_CATALOG_WIRE_BYTES,
        config.required_owner_uid,
    ) {
        Ok(mut file) => {
            let mut body = Vec::new();
            file.read_to_end(&mut body)?;
            let parsed: SignedFlatpakAppCatalog =
                serde_json::from_slice(&body).map_err(io_other)?;
            let signature_check_time = parsed.payload.issued_at_unix_ms;
            let verified = SignedFlatpakAppCatalog::decode_and_admit_json(
                &body,
                &config.signer_id,
                &config.verifying_key,
                signature_check_time,
            )
            .map_err(io_other)?;
            let watermark = CatalogWatermark::from_catalog(&verified)?;
            let is_fresh = SignedFlatpakAppCatalog::decode_and_admit_json(
                &body,
                &config.signer_id,
                &config.verifying_key,
                now_ms,
            )
            .is_ok();
            Ok(Some(RecoveredCatalog {
                watermark,
                catalog: verified,
                is_fresh,
            }))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn store_last_good(config: &CatalogConfig, catalog: &SignedFlatpakAppCatalog) -> io::Result<()> {
    let path = &config.last_good_file;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Flatpak catalog state path has no parent"))?;
    reject_symlink_parent(path)?;
    fs::create_dir_all(parent)?;
    validate_directory_owner(parent, config.required_owner_uid)?;
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(io::Error::other("Flatpak catalog state path is a symlink"));
    }
    let body = serde_json::to_vec(catalog).map_err(io_other)?;
    if body.len() > usize::try_from(MAX_CATALOG_WIRE_BYTES).unwrap_or(usize::MAX) {
        return Err(io::Error::other(
            "admitted Flatpak catalog exceeds persistence bound",
        ));
    }

    let temp = parent.join(format!(
        ".flatpak-catalog-{}-{}.tmp",
        std::process::id(),
        now_unix_ms()
    ));
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
        validate_open_metadata(&file, MAX_CATALOG_WIRE_BYTES, config.required_owner_uid)?;
        fs::rename(&temp, path)?;
        let persisted =
            open_secure_regular_nofollow(path, MAX_CATALOG_WIRE_BYTES, config.required_owner_uid)?;
        persisted.sync_all()?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn open_secure_regular_nofollow(
    path: &Path,
    max_bytes: u64,
    required_owner_uid: u32,
) -> io::Result<File> {
    reject_symlink_parent(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0o400000); // Linux O_NOFOLLOW
    }
    let file = options.open(path)?;
    validate_open_metadata(&file, max_bytes, required_owner_uid)?;
    Ok(file)
}

fn validate_open_metadata(file: &File, max_bytes: u64, required_owner_uid: u32) -> io::Result<()> {
    let metadata = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return validate_secure_metadata(
            metadata.is_file(),
            metadata.len(),
            metadata.uid(),
            metadata.mode(),
            max_bytes,
            required_owner_uid,
        );
    }
    #[cfg(not(unix))]
    {
        let _ = required_owner_uid;
        if !metadata.is_file() || metadata.len() > max_bytes {
            return Err(io::Error::other(
                "catalog file is not a bounded regular file",
            ));
        }
        Ok(())
    }
}

fn validate_secure_metadata(
    is_file: bool,
    length: u64,
    owner_uid: u32,
    mode: u32,
    max_bytes: u64,
    required_owner_uid: u32,
) -> io::Result<()> {
    if !is_file || length > max_bytes {
        return Err(io::Error::other(
            "catalog file is not a bounded regular file",
        ));
    }
    if owner_uid != required_owner_uid {
        return Err(io::Error::other(
            "catalog file is not owned by the required account",
        ));
    }
    if mode & 0o022 != 0 {
        return Err(io::Error::other("catalog file is group/world writable"));
    }
    Ok(())
}

fn validate_directory_owner(path: &Path, required_owner_uid: u32) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || metadata.uid() != required_owner_uid
            || metadata.mode() & 0o022 != 0
        {
            return Err(io::Error::other(
                "catalog state directory is not securely owned",
            ));
        }
    }
    #[cfg(not(unix))]
    if !metadata.is_dir() {
        return Err(io::Error::other("catalog state parent is not a directory"));
    }
    Ok(())
}

fn reject_symlink_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("catalog file has no parent"))?;
    match fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(io::Error::other("catalog file parent is a symlink"))
        }
        Ok(metadata) if !metadata.is_dir() => {
            Err(io::Error::other("catalog file parent is not a directory"))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn projection_from(
    host: &str,
    catalog: &SignedFlatpakAppCatalog,
    include_installed: bool,
) -> io::Result<AdmittedFlatpakCatalogProjection> {
    let entries = if include_installed {
        catalog
            .payload
            .entries
            .iter()
            .filter(|entry| entry.state == FlatpakInstallState::Installed)
            .map(project_entry)
            .collect()
    } else {
        Vec::new()
    };
    Ok(AdmittedFlatpakCatalogProjection {
        schema_version: PROJECTION_SCHEMA_VERSION,
        host: host.to_owned(),
        catalog_id: catalog.payload.catalog_id.clone(),
        revision: catalog.payload.revision,
        issued_at_unix_ms: catalog.payload.issued_at_unix_ms,
        expires_at_unix_ms: catalog.payload.expires_at_unix_ms,
        content_digest: catalog.payload.content_digest().map_err(io_other)?,
        provider_id: catalog.payload.origin.provider_id.clone(),
        repository_id: catalog.payload.origin.repository_id.clone(),
        entries,
    })
}

fn project_entry(entry: &SignedFlatpakCatalogEntry) -> AdmittedFlatpakAppProjection {
    AdmittedFlatpakAppProjection {
        app_id: entry.app_id.clone(),
        display_name: entry.display_name.clone(),
        summary: entry.summary.clone(),
        version: entry.version.clone(),
        icon_id: entry.icon_id.clone(),
        permissions: entry.permissions.clone(),
        guest_profile: entry.guest_profile.clone(),
        supported_actions: entry.supported_actions.clone(),
        search_terms: entry.search.terms.clone(),
        search_weight: entry.search.weight,
    }
}

fn publish_projection(
    persist: &mut Persist,
    host: &str,
    catalog: &SignedFlatpakAppCatalog,
) -> io::Result<()> {
    let projection = projection_from(host, catalog, true)?;
    let body = serde_json::to_string(&projection).map_err(io_other)?;
    persist
        .write(
            &app_catalog_projection_topic(host)?,
            Priority::Default,
            None,
            Some(&body),
        )
        .map_err(io_other)?;
    Ok(())
}

fn publish_empty_projection(
    persist: &mut Persist,
    host: &str,
    catalog: &SignedFlatpakAppCatalog,
) -> io::Result<()> {
    let projection = projection_from(host, catalog, false)?;
    let body = serde_json::to_string(&projection).map_err(io_other)?;
    persist
        .write(
            &app_catalog_projection_topic(host)?,
            Priority::Default,
            None,
            Some(&body),
        )
        .map_err(io_other)?;
    Ok(())
}

fn publish_status_if_changed(
    persist: &mut Persist,
    status: &AppCatalogImportStatus,
    last_status: &mut Option<AppCatalogImportStatus>,
) -> io::Result<bool> {
    if last_status
        .as_ref()
        .is_some_and(|previous| same_status_state(previous, status))
    {
        return Ok(false);
    }
    let body = serde_json::to_string(status).map_err(io_other)?;
    persist
        .write(
            &app_catalog_status_topic(&status.host)?,
            Priority::Default,
            None,
            Some(&body),
        )
        .map_err(io_other)?;
    *last_status = Some(status.clone());
    Ok(true)
}

fn same_status_state(left: &AppCatalogImportStatus, right: &AppCatalogImportStatus) -> bool {
    left.schema_version == right.schema_version
        && left.host == right.host
        && left.outcome == right.outcome
        && left.reason == right.reason
        && left.remedy == right.remedy
        && left.retained_catalog_id == right.retained_catalog_id
        && left.retained_revision == right.retained_revision
}

fn status_for(
    host: &str,
    outcome: AppCatalogImportOutcome,
    reason: AppCatalogStatusReason,
    remedy: AppCatalogRemedy,
    watermark: Option<&CatalogWatermark>,
    now_ms: u64,
) -> AppCatalogImportStatus {
    AppCatalogImportStatus {
        schema_version: STATUS_SCHEMA_VERSION,
        host: host.to_owned(),
        outcome,
        reason,
        remedy,
        retained_catalog_id: watermark.map(|catalog| catalog.catalog_id.clone()),
        retained_revision: watermark.map(|catalog| catalog.revision),
        observed_at_unix_ms: now_ms,
    }
}

fn app_catalog_import_topic(host: &str) -> io::Result<String> {
    validate_host(host)?;
    Ok(format!("action/app-catalog/import/{host}"))
}

fn app_catalog_projection_topic(host: &str) -> io::Result<String> {
    validate_host(host)?;
    Ok(format!("state/app-catalog/{host}"))
}

fn app_catalog_status_topic(host: &str) -> io::Result<String> {
    validate_host(host)?;
    Ok(format!("state/app-catalog-status/{host}"))
}

fn validate_host(host: &str) -> io::Result<()> {
    if host.is_empty()
        || host.len() > 128
        || host.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        })
    {
        return Err(io::Error::other("invalid app catalog host identity"));
    }
    Ok(())
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
    use mackes_mesh_types::app_catalog::{
        FlatpakCatalogOrigin, FlatpakSearchMetadata, SignedFlatpakCatalogPayload,
        SIGNED_FLATPAK_CATALOG_SCHEMA_VERSION,
    };
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use tempfile::TempDir;

    const NOW: u64 = 1_800_000_000_000;

    fn entry(app_id: &str, state: FlatpakInstallState) -> SignedFlatpakCatalogEntry {
        SignedFlatpakCatalogEntry {
            app_id: app_id.into(),
            display_name: app_id.rsplit('.').next().unwrap().into(),
            summary: "Governed guest application".into(),
            version: "2026.08.1".into(),
            icon_id: format!("icon:{app_id}"),
            permissions: vec!["audio".into(), "clipboard".into()],
            guest_profile: "wayland-standard-v1".into(),
            supported_actions: vec!["launch".into(), "resume".into()],
            search: FlatpakSearchMetadata {
                terms: vec!["app".into(), "guest".into()],
                weight: 500,
            },
            state,
        }
    }

    fn signed_catalog(
        key: &SigningKey,
        catalog_id: &str,
        revision: u64,
    ) -> SignedFlatpakAppCatalog {
        signed_catalog_window(key, catalog_id, revision, NOW - 1_000, NOW + 60_000)
    }

    fn signed_catalog_window(
        key: &SigningKey,
        catalog_id: &str,
        revision: u64,
        issued_at_unix_ms: u64,
        expires_at_unix_ms: u64,
    ) -> SignedFlatpakAppCatalog {
        SignedFlatpakAppCatalog::sign(
            "flatpak-release-v1",
            SignedFlatpakCatalogPayload {
                schema_version: SIGNED_FLATPAK_CATALOG_SCHEMA_VERSION,
                catalog_id: catalog_id.into(),
                revision,
                issued_at_unix_ms,
                expires_at_unix_ms,
                origin: FlatpakCatalogOrigin {
                    provider_id: "mcnf-curated".into(),
                    repository_id: "flathub-stable".into(),
                },
                entries: vec![
                    entry("org.example.Available", FlatpakInstallState::Available),
                    entry("org.example.Editor", FlatpakInstallState::Installed),
                ],
            },
            key,
        )
        .unwrap()
    }

    fn owner_uid() -> u32 {
        rustix::process::getuid().as_raw()
    }

    fn worker(temp: &TempDir, key: &SigningKey) -> AppCatalogWorker {
        let state_dir = temp.path().join("state");
        fs::create_dir(&state_dir).unwrap();
        fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700)).unwrap();
        AppCatalogWorker {
            host: "node-01".into(),
            bus_root: Some(temp.path().join("bus")),
            config: Some(CatalogConfig {
                signer_id: "flatpak-release-v1".into(),
                verifying_key: key.verifying_key(),
                last_good_file: state_dir.join("catalog.json"),
                required_owner_uid: owner_uid(),
            }),
        }
    }

    fn import(persist: &Persist, catalog: &SignedFlatpakAppCatalog) {
        persist
            .write(
                &app_catalog_import_topic("node-01").unwrap(),
                Priority::Default,
                None,
                Some(&serde_json::to_string(catalog).unwrap()),
            )
            .unwrap();
    }

    fn latest_status(persist: &Persist) -> AppCatalogImportStatus {
        let rows = persist
            .list_since(&app_catalog_status_topic("node-01").unwrap(), None)
            .unwrap();
        serde_json::from_str(rows.last().unwrap().body.as_deref().unwrap()).unwrap()
    }

    #[test]
    fn admits_persists_and_projects_only_installed_rows() {
        let temp = TempDir::new().unwrap();
        let key = SigningKey::from_bytes(&[7; 32]);
        let worker = worker(&temp, &key);
        let mut persist = Persist::open(temp.path().join("bus")).unwrap();
        import(&persist, &signed_catalog(&key, "flatpak-production", 7));
        let mut cursor = None;
        let mut current = None;
        let mut watermark = None;
        let mut last_status = None;
        let tally = worker
            .process_once(
                &mut persist,
                &mut cursor,
                &mut current,
                &mut watermark,
                &mut last_status,
                NOW,
            )
            .unwrap();
        assert_eq!(tally.admitted, 1);
        let rows = persist
            .list_since(&app_catalog_projection_topic("node-01").unwrap(), None)
            .unwrap();
        let projection: AdmittedFlatpakCatalogProjection =
            serde_json::from_str(rows[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(projection.entries.len(), 1);
        assert_eq!(projection.entries[0].app_id, "org.example.Editor");
        let json = rows[0].body.as_deref().unwrap();
        for forbidden in [
            "signature",
            "https://",
            "file://",
            "/var/",
            "command",
            "token=",
        ] {
            assert!(!json.contains(forbidden), "projection leaked {forbidden}");
        }
        let metadata = fs::metadata(&worker.config.as_ref().unwrap().last_good_file).unwrap();
        assert_eq!(metadata.uid(), owner_uid());
        assert_eq!(metadata.mode() & 0o077, 0);
        assert_eq!(
            latest_status(&persist).outcome,
            AppCatalogImportOutcome::Admitted
        );

        worker
            .process_once(
                &mut persist,
                &mut cursor,
                &mut current,
                &mut watermark,
                &mut last_status,
                NOW + 60_001,
            )
            .unwrap();
        assert!(current.is_none());
        let rows = persist
            .list_since(&app_catalog_projection_topic("node-01").unwrap(), None)
            .unwrap();
        let retraction: AdmittedFlatpakCatalogProjection =
            serde_json::from_str(rows.last().unwrap().body.as_deref().unwrap()).unwrap();
        assert!(retraction.entries.is_empty());
        assert_eq!(
            latest_status(&persist).reason,
            AppCatalogStatusReason::RetainedCatalogExpired
        );
    }

    #[test]
    fn exact_digest_replay_is_idempotent_and_revision_rules_retain_last_good() {
        let temp = TempDir::new().unwrap();
        let key = SigningKey::from_bytes(&[7; 32]);
        let worker = worker(&temp, &key);
        let mut persist = Persist::open(temp.path().join("bus")).unwrap();
        let good = signed_catalog(&key, "flatpak-production", 7);
        import(&persist, &good);
        import(&persist, &good);
        import(&persist, &signed_catalog(&key, "flatpak-production", 6));
        let mut conflict = signed_catalog(&key, "flatpak-production", 7);
        conflict.payload.entries[1].search.weight = 600;
        conflict =
            SignedFlatpakAppCatalog::sign("flatpak-release-v1", conflict.payload, &key).unwrap();
        import(&persist, &conflict);
        import(&persist, &signed_catalog(&key, "other-catalog", 8));
        let mut cursor = None;
        let mut current = None;
        let mut watermark = None;
        let mut last_status = None;
        let tally = worker
            .process_once(
                &mut persist,
                &mut cursor,
                &mut current,
                &mut watermark,
                &mut last_status,
                NOW,
            )
            .unwrap();
        assert_eq!(
            tally,
            ImportTally {
                admitted: 1,
                replayed: 1,
                refused: 3
            }
        );
        assert_eq!(current.as_ref().unwrap().payload.revision, 7);
        assert_eq!(
            persist
                .list_since(&app_catalog_projection_topic("node-01").unwrap(), None)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            recover_last_good(worker.config.as_ref().unwrap(), NOW)
                .unwrap()
                .unwrap()
                .watermark
                .revision,
            7
        );
    }

    #[test]
    fn hostile_imports_publish_payload_free_status_and_preserve_last_good() {
        let temp = TempDir::new().unwrap();
        let key = SigningKey::from_bytes(&[7; 32]);
        let hostile_key = SigningKey::from_bytes(&[8; 32]);
        let worker = worker(&temp, &key);
        let good = signed_catalog(&key, "flatpak-production", 7);
        store_last_good(worker.config.as_ref().unwrap(), &good).unwrap();
        let mut persist = Persist::open(temp.path().join("bus")).unwrap();
        import(
            &persist,
            &signed_catalog(&hostile_key, "flatpak-production", 8),
        );
        persist
            .write(
                &app_catalog_import_topic("node-01").unwrap(),
                Priority::Default,
                None,
                Some("{\"token=super-secret\":true}"),
            )
            .unwrap();
        persist
            .write(
                &app_catalog_import_topic("node-01").unwrap(),
                Priority::Default,
                None,
                None,
            )
            .unwrap();
        let mut cursor = None;
        let mut current = Some(good.clone());
        let mut watermark = Some(CatalogWatermark::from_catalog(&good).unwrap());
        let mut last_status = None;
        let tally = worker
            .process_once(
                &mut persist,
                &mut cursor,
                &mut current,
                &mut watermark,
                &mut last_status,
                NOW,
            )
            .unwrap();
        assert_eq!(tally.refused, 3);
        assert_eq!(current, Some(good.clone()));
        let status_rows = persist
            .list_since(&app_catalog_status_topic("node-01").unwrap(), None)
            .unwrap();
        for row in status_rows {
            let body = row.body.unwrap();
            assert!(!body.contains("super-secret"));
            assert!(!body.contains("signature"));
            assert!(!body.contains("path"));
            assert!(!body.contains("command"));
        }
        assert_eq!(
            recover_last_good(worker.config.as_ref().unwrap(), NOW)
                .unwrap()
                .unwrap()
                .catalog,
            good
        );
    }

    #[test]
    fn trust_and_state_files_enforce_owner_mode_regular_bounds_and_nofollow() {
        let temp = TempDir::new().unwrap();
        let key = SigningKey::from_bytes(&[7; 32]);
        let key_path = temp.path().join("trust.hex");
        fs::write(&key_path, hex(&key.verifying_key().to_bytes())).unwrap();
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            load_verifying_key(&key_path, owner_uid()).unwrap(),
            key.verifying_key()
        );
        assert!(load_verifying_key(&key_path, owner_uid().saturating_add(1)).is_err());
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o620)).unwrap();
        assert!(load_verifying_key(&key_path, owner_uid()).is_err());
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600)).unwrap();

        let symlink = temp.path().join("trust-link.hex");
        std::os::unix::fs::symlink(&key_path, &symlink).unwrap();
        assert!(load_verifying_key(&symlink, owner_uid()).is_err());
        assert!(validate_secure_metadata(
            true,
            MAX_TRUST_KEY_BYTES + 1,
            owner_uid(),
            0o100600,
            MAX_TRUST_KEY_BYTES,
            owner_uid()
        )
        .is_err());

        let real_parent = temp.path().join("real-state");
        fs::create_dir(&real_parent).unwrap();
        fs::set_permissions(&real_parent, fs::Permissions::from_mode(0o700)).unwrap();
        let linked_parent = temp.path().join("linked-state");
        std::os::unix::fs::symlink(&real_parent, &linked_parent).unwrap();
        let linked_config = CatalogConfig {
            signer_id: "flatpak-release-v1".into(),
            verifying_key: key.verifying_key(),
            last_good_file: linked_parent.join("catalog.json"),
            required_owner_uid: owner_uid(),
        };
        assert!(store_last_good(
            &linked_config,
            &signed_catalog(&key, "flatpak-production", 7)
        )
        .is_err());
        assert!(!real_parent.join("catalog.json").exists());
        assert!(validate_secure_metadata(
            false,
            64,
            owner_uid(),
            0o040700,
            MAX_TRUST_KEY_BYTES,
            owner_uid()
        )
        .is_err());
    }

    #[test]
    fn startup_recovery_readmits_exact_trust_and_refuses_stale_or_tampered_cache() {
        let temp = TempDir::new().unwrap();
        let key = SigningKey::from_bytes(&[7; 32]);
        let worker = worker(&temp, &key);
        let catalog = signed_catalog(&key, "flatpak-production", 7);
        store_last_good(worker.config.as_ref().unwrap(), &catalog).unwrap();
        let fresh = recover_last_good(worker.config.as_ref().unwrap(), NOW)
            .unwrap()
            .unwrap();
        assert!(fresh.is_fresh);
        assert_eq!(fresh.catalog, catalog.clone());
        assert_eq!(fresh.watermark.revision, 7);

        let expired = recover_last_good(worker.config.as_ref().unwrap(), NOW + 60_001)
            .unwrap()
            .unwrap();
        assert!(!expired.is_fresh);
        assert_eq!(expired.watermark.catalog_id, "flatpak-production");
        assert_eq!(expired.watermark.revision, 7);

        const RESTART_NOW: u64 = NOW + 70_000;
        let mut persist = Persist::open(temp.path().join("bus")).unwrap();
        publish_empty_projection(&mut persist, "node-01", &expired.catalog).unwrap();
        import(
            &persist,
            &signed_catalog_window(
                &key,
                "other-catalog",
                8,
                RESTART_NOW - 1_000,
                RESTART_NOW + 60_000,
            ),
        );
        import(
            &persist,
            &signed_catalog_window(
                &key,
                "flatpak-production",
                6,
                RESTART_NOW - 1_000,
                RESTART_NOW + 60_000,
            ),
        );
        let mut cursor = None;
        let mut current = None;
        let mut watermark = Some(expired.watermark);
        let mut last_status = None;
        let tally = worker
            .process_once(
                &mut persist,
                &mut cursor,
                &mut current,
                &mut watermark,
                &mut last_status,
                RESTART_NOW,
            )
            .unwrap();
        assert_eq!(tally.refused, 2);
        assert!(current.is_none());
        let projection_rows = persist
            .list_since(&app_catalog_projection_topic("node-01").unwrap(), None)
            .unwrap();
        assert_eq!(projection_rows.len(), 1);
        let retraction: AdmittedFlatpakCatalogProjection =
            serde_json::from_str(projection_rows[0].body.as_deref().unwrap()).unwrap();
        assert!(retraction.entries.is_empty());

        let mut body = fs::read(&worker.config.as_ref().unwrap().last_good_file).unwrap();
        let index = body.iter().position(|byte| *byte == b'E').unwrap();
        body[index] = b'X';
        fs::write(&worker.config.as_ref().unwrap().last_good_file, body).unwrap();
        fs::set_permissions(
            &worker.config.as_ref().unwrap().last_good_file,
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert!(recover_last_good(worker.config.as_ref().unwrap(), NOW).is_err());
    }

    #[test]
    fn unchanged_unavailable_and_refusal_statuses_are_edge_triggered() {
        let temp = TempDir::new().unwrap();
        let mut persist = Persist::open(temp.path().join("bus")).unwrap();
        let unavailable_worker = AppCatalogWorker {
            host: "node-01".into(),
            bus_root: Some(temp.path().join("bus")),
            config: None,
        };
        let mut cursor = None;
        let mut current = None;
        let mut watermark = None;
        let mut last_status = None;
        unavailable_worker
            .process_once(
                &mut persist,
                &mut cursor,
                &mut current,
                &mut watermark,
                &mut last_status,
                NOW,
            )
            .unwrap();
        unavailable_worker
            .process_once(
                &mut persist,
                &mut cursor,
                &mut current,
                &mut watermark,
                &mut last_status,
                NOW + 1_000,
            )
            .unwrap();
        assert_eq!(
            persist
                .list_since(&app_catalog_status_topic("node-01").unwrap(), None)
                .unwrap()
                .len(),
            1
        );

        let key = SigningKey::from_bytes(&[7; 32]);
        let configured = worker(&temp, &key);
        for _ in 0..2 {
            persist
                .write(
                    &app_catalog_import_topic("node-01").unwrap(),
                    Priority::Default,
                    None,
                    Some("{\"secret=must-not-echo\":true}"),
                )
                .unwrap();
        }
        let tally = configured
            .process_once(
                &mut persist,
                &mut cursor,
                &mut current,
                &mut watermark,
                &mut last_status,
                NOW + 2_000,
            )
            .unwrap();
        assert_eq!(tally.refused, 2);
        let statuses = persist
            .list_since(&app_catalog_status_topic("node-01").unwrap(), None)
            .unwrap();
        assert_eq!(statuses.len(), 2);
        assert!(!statuses
            .last()
            .unwrap()
            .body
            .as_deref()
            .unwrap()
            .contains("must-not-echo"));
    }

    #[tokio::test]
    async fn unconfigured_worker_quiesces_without_creating_bus_state() {
        let temp = TempDir::new().unwrap();
        let bus_root = temp.path().join("bus");
        let mut worker = AppCatalogWorker {
            host: "node-01".into(),
            bus_root: Some(bus_root.clone()),
            config: None,
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

    fn hex(bytes: &[u8]) -> String {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut value = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            value.push(char::from(DIGITS[usize::from(byte >> 4)]));
            value.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
        }
        value
    }
}
