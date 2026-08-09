//! Pure governed Android model and renderer for the Workloads surface.
//!
//! Bus reads, lifecycle publication, and remote-session routing stay in the
//! parent IAC controller. This module consumes immutable typed projections and
//! returns typed UI intents; rendering performs no clock, disk, Bus, network,
//! or backend I/O.

use mackes_mesh_types::android_apps::{
    AndroidAppCapability, AndroidAppInventory, AndroidAppPermission, AndroidCatalogAppPolicy,
    AndroidSignedCatalog, AospStarterApp, MAX_ANDROID_OBSERVATION_AGE_MS,
};
use mackes_mesh_types::android_provider::{
    AndroidProviderAdmission, AndroidProviderRefusal, AndroidVdiProtocol, AndroidVdiSource,
};
use mde_egui::egui::{self, RichText};
use mde_egui::{card, muted_note, Style};
use serde::Deserialize;

const MAX_CATALOG_WIRE_BYTES: usize = 2 * 1024 * 1024;
const ADMITTED_CACHE_SCHEMA_VERSION: u16 = 1;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmittedCatalogCache {
    schema_version: u16,
    catalog: AndroidSignedCatalog,
}

#[derive(Debug, Clone)]
pub(super) struct CatalogSnapshot {
    pub(super) node: String,
    pub(super) catalog: AndroidSignedCatalog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LifecycleOperation {
    Start,
    Stop,
    Cancel,
    Retry,
}

impl LifecycleOperation {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Cancel => "Cancel",
            Self::Retry => "Retry",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LifecyclePhase {
    Stopped,
    Starting,
    CheckingGuest,
    Running,
    Stopping,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LifecycleReceipt {
    pub(super) schema_version: u16,
    pub(super) workload_id: String,
    pub(super) generation: u64,
    pub(super) phase: LifecyclePhase,
    pub(super) app: Option<AospStarterApp>,
    pub(super) last_request_id: Option<String>,
    pub(super) last_operation: Option<LifecycleOperation>,
    pub(super) last_ok: bool,
    pub(super) failure: Option<String>,
}

impl LifecycleReceipt {
    pub(super) fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() || raw.len() > 32 * 1024 {
            return None;
        }
        let receipt: Self = serde_json::from_str(raw).ok()?;
        let request_valid = receipt
            .last_request_id
            .as_ref()
            .is_some_and(|request_id| !request_id.is_empty() && request_id.len() <= 128);
        let operation_valid = match receipt.last_operation {
            Some(LifecycleOperation::Start | LifecycleOperation::Retry) => {
                receipt.app.is_some() || receipt.phase == LifecyclePhase::Failed
            }
            Some(LifecycleOperation::Stop | LifecycleOperation::Cancel) => receipt.app.is_none(),
            None => false,
        };
        let outcome_valid = if receipt.last_ok {
            receipt.failure.is_none()
                && matches!(
                    receipt.phase,
                    LifecyclePhase::Running | LifecyclePhase::Stopped | LifecyclePhase::Cancelled
                )
        } else {
            receipt.failure.is_some() || receipt.phase == LifecyclePhase::Failed
        };
        (receipt.schema_version == 1
            && !receipt.workload_id.is_empty()
            && receipt.workload_id.len() <= 128
            && receipt.generation > 0
            && request_valid
            && operation_valid
            && outcome_valid)
            .then_some(receipt)
    }
}

#[derive(Debug, Clone)]
pub(super) struct PendingLifecycle {
    pub(super) workload_id: String,
    pub(super) operation: LifecycleOperation,
    pub(super) generation: u64,
    pub(super) app: Option<AospStarterApp>,
}

#[derive(Debug, Clone)]
pub(super) struct WorkloadInput {
    pub(super) node: String,
    pub(super) workload_id: String,
    pub(super) runtime_status: String,
}

#[derive(Debug, Clone)]
pub(super) struct ModelInput<'a> {
    pub(super) workload: &'a WorkloadInput,
    pub(super) catalog: Option<&'a CatalogSnapshot>,
    /// Digest read from the daemon's privileged admitted-catalog cache during
    /// polling. The writable Bus catalog topic alone is never catalog authority.
    pub(super) admitted_cache_digest: Option<&'a str>,
    pub(super) admission: Option<&'a AndroidProviderAdmission>,
    pub(super) inventory: Option<&'a AndroidAppInventory>,
    pub(super) vdi_source: Option<&'a AndroidVdiSource>,
    pub(super) receipt: Option<&'a LifecycleReceipt>,
    pub(super) pending: Option<&'a PendingLifecycle>,
    pub(super) now_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WorkloadAvailability {
    Ready,
    Starting,
    CheckingGuest,
    Running,
    Stopping,
    Cancelled,
    Failed(String),
    Unavailable(String),
}

impl WorkloadAvailability {
    fn label(&self) -> String {
        match self {
            Self::Ready => "Ready to start".to_owned(),
            Self::Starting => "Starting outer VM…".to_owned(),
            Self::CheckingGuest => "Checking package and launcher readiness…".to_owned(),
            Self::Running => "Running · WebRTC session ready".to_owned(),
            Self::Stopping => "Stopping and cleaning up…".to_owned(),
            Self::Cancelled => "Cancelled · cleaned up".to_owned(),
            Self::Failed(reason) => format!("Failed · {reason}"),
            Self::Unavailable(reason) => format!("Unavailable · {reason}"),
        }
    }

    fn tone(&self) -> egui::Color32 {
        match self {
            Self::Ready | Self::Running => Style::OK,
            Self::Starting | Self::CheckingGuest | Self::Stopping => Style::WARN,
            Self::Cancelled => Style::TEXT_DIM,
            Self::Failed(_) | Self::Unavailable(_) => Style::DANGER,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct AppCard {
    pub(super) app: AospStarterApp,
    pub(super) package_id: &'static str,
    pub(super) permissions: String,
    pub(super) capabilities: String,
    pub(super) approval: String,
    pub(super) evidence: String,
    pub(super) can_start: bool,
    pub(super) can_retry: bool,
}

#[derive(Debug, Clone)]
pub(super) struct WorkloadProjection {
    pub(super) node: String,
    pub(super) workload_id: String,
    pub(super) signed_identity: String,
    pub(super) availability: WorkloadAvailability,
    pub(super) expected_generation: u64,
    pub(super) cards: Vec<AppCard>,
    pub(super) vdi_source: Option<AndroidVdiSource>,
    pub(super) can_stop: bool,
    pub(super) can_cancel: bool,
}

#[derive(Debug, Clone)]
pub(super) enum UiAction {
    Lifecycle {
        node: String,
        workload_id: String,
        operation: LifecycleOperation,
        app: Option<AospStarterApp>,
        expected_generation: u64,
        approval: String,
    },
    Attach {
        node: String,
        source: AndroidVdiSource,
    },
}

pub(super) fn decode_catalog_snapshot(topic: &str, body: &str) -> Option<CatalogSnapshot> {
    use mackes_mesh_types::android_apps::ANDROID_CATALOG_STATE_TOPIC_PREFIX;

    if body.is_empty() || body.len() > MAX_CATALOG_WIRE_BYTES {
        return None;
    }
    let node = topic
        .strip_prefix(ANDROID_CATALOG_STATE_TOPIC_PREFIX)?
        .to_owned();
    if node.is_empty()
        || node.len() > 128
        || !node
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return None;
    }
    let catalog: AndroidSignedCatalog = serde_json::from_str(body).ok()?;
    catalog.payload.validate().ok()?;
    valid_signature_envelope_shape(&catalog).then_some(CatalogSnapshot { node, catalog })
}

/// Read only the digest from a daemon-owned admitted cache. Production passes
/// UID 0. The explicit UID parameter permits an unprivileged test fixture to
/// exercise the identical no-follow, ownership, mode, type, and size checks.
#[cfg(unix)]
pub(super) fn read_admitted_catalog_digest(
    path: &std::path::Path,
    required_uid: u32,
) -> Option<String> {
    use std::io::Read as _;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};

    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    options.custom_flags(0o400000); // O_NOFOLLOW
    #[cfg(not(target_os = "linux"))]
    if std::fs::symlink_metadata(path)
        .ok()?
        .file_type()
        .is_symlink()
    {
        return None;
    }
    let file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file()
        || metadata.uid() != required_uid
        || metadata.mode() & 0o022 != 0
        || metadata.len() > MAX_CATALOG_WIRE_BYTES as u64
    {
        return None;
    }
    let mut body =
        String::with_capacity(MAX_CATALOG_WIRE_BYTES.min(usize::try_from(metadata.len()).ok()?));
    file.take((MAX_CATALOG_WIRE_BYTES + 1) as u64)
        .read_to_string(&mut body)
        .ok()?;
    if body.is_empty() || body.len() > MAX_CATALOG_WIRE_BYTES {
        return None;
    }
    let cache: AdmittedCatalogCache = serde_json::from_str(&body).ok()?;
    if cache.schema_version != ADMITTED_CACHE_SCHEMA_VERSION
        || cache.catalog.payload.validate().is_err()
        || !valid_signature_envelope_shape(&cache.catalog)
    {
        return None;
    }
    cache.catalog.payload.content_digest().ok()
}

#[cfg(not(unix))]
pub(super) fn read_admitted_catalog_digest(
    _path: &std::path::Path,
    _required_uid: u32,
) -> Option<String> {
    None
}

/// Syntactic decode only. This does not verify Ed25519 and callers must bind the
/// payload digest to daemon-owned admitted evidence before displaying policy.
fn valid_signature_envelope_shape(catalog: &AndroidSignedCatalog) -> bool {
    !catalog.signer_id.is_empty()
        && catalog.signer_id.len() <= 128
        && catalog.signature.len() == 128
        && catalog
            .signature
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub(super) fn project(input: ModelInput<'_>) -> WorkloadProjection {
    let generation = input
        .pending
        .filter(|pending| pending.workload_id == input.workload.workload_id)
        .map(|pending| pending.generation)
        .or_else(|| {
            input
                .receipt
                .filter(|receipt| receipt.workload_id == input.workload.workload_id)
                .map(|receipt| receipt.generation)
        })
        .or_else(|| input.vdi_source.map(|source| source.generation))
        .unwrap_or(0);

    let catalog = input.catalog.map(|snapshot| &snapshot.catalog);
    let catalog_bound = catalog
        .and_then(|catalog| catalog.payload.content_digest().ok())
        .is_some_and(|digest| input.admitted_cache_digest == Some(digest.as_str()));
    let unavailable = projection_refusal(&input);
    let inventory = input.inventory;
    let lifecycle = lifecycle_availability(&input, unavailable);
    let ready = lifecycle == WorkloadAvailability::Ready;
    let retry = matches!(
        lifecycle,
        WorkloadAvailability::Failed(_) | WorkloadAvailability::Cancelled
    );
    let cards = catalog
        .filter(|_| catalog_bound)
        .map_or_else(Vec::new, |catalog| {
            catalog
                .payload
                .app_policies
                .iter()
                .map(|policy| app_card(policy, catalog, inventory, ready, retry))
                .collect()
        });
    let vdi_source = matches!(lifecycle, WorkloadAvailability::Running)
        .then(|| input.vdi_source.cloned())
        .flatten();
    let can_stop = input
        .receipt
        .is_some_and(|receipt| receipt.phase == LifecyclePhase::Running)
        || vdi_source.is_some();
    let can_cancel = input.pending.is_some_and(|pending| {
        pending.workload_id == input.workload.workload_id
            && matches!(
                pending.operation,
                LifecycleOperation::Start | LifecycleOperation::Retry
            )
            && pending.app.is_some()
    });
    let signed_identity = catalog.filter(|_| catalog_bound).map_or_else(
        || "No admitted signed catalog".to_owned(),
        |catalog| {
            let digest = catalog
                .payload
                .content_digest()
                .unwrap_or_else(|_| "invalid digest".to_owned());
            format!(
                "Daemon-admitted signed catalog · signer {} · revision {} · {}",
                catalog.signer_id, catalog.payload.revision, digest
            )
        },
    );
    WorkloadProjection {
        node: input.workload.node.clone(),
        workload_id: input.workload.workload_id.clone(),
        signed_identity,
        availability: lifecycle,
        expected_generation: generation,
        cards,
        vdi_source,
        can_stop,
        can_cancel,
    }
}

fn projection_refusal(input: &ModelInput<'_>) -> Option<String> {
    let Some(catalog) = input
        .catalog
        .filter(|snapshot| snapshot.node == input.workload.node)
    else {
        return Some("no admitted signed catalog for this placement".to_owned());
    };
    if catalog.catalog.payload.validate().is_err()
        || input.now_unix_ms < catalog.catalog.payload.issued_at_unix_ms
        || input.now_unix_ms > catalog.catalog.payload.expires_at_unix_ms
    {
        return Some("the signed catalog is invalid or outside its validity window".to_owned());
    }
    let digest = match catalog.catalog.payload.content_digest() {
        Ok(digest) => digest,
        Err(_) => return Some("the signed catalog payload digest is invalid".to_owned()),
    };
    if input.admitted_cache_digest != Some(digest.as_str()) {
        return Some(
            "daemon-admitted catalog cache is absent or mismatched; the writable Bus envelope is not signature authority"
                .to_owned(),
        );
    }
    let provenance = &catalog.catalog.payload.package_manifest.image_provenance;
    let admission = match input.admission {
        Some(admission) if admission.workload_id == input.workload.workload_id => admission,
        _ => return Some("provider preflight has not published for this workload".to_owned()),
    };
    if !admission.is_ready() {
        return Some(
            admission
                .refusal
                .map(provider_refusal_text)
                .unwrap_or("provider preflight evidence is invalid")
                .to_owned(),
        );
    }
    if !admission.image_provenance.as_ref().is_some_and(|admitted| {
        admitted.image_id == provenance.image_id
            && admitted.image_digest == provenance.image_digest
            && admitted.source_revision == provenance.source_revision
            && admitted.catalog_revision == provenance.catalog_revision
    }) {
        return Some("provider image identity does not match the signed catalog".to_owned());
    }
    let inventory = match input.inventory {
        Some(inventory) if inventory.workload_id == input.workload.workload_id => inventory,
        _ => return Some("guest package inventory has not published".to_owned()),
    };
    let fresh_inventory = inventory.validate_at(input.now_unix_ms).is_ok()
        && inventory.observed_at_unix_ms.is_some_and(|observed| {
            input.now_unix_ms.saturating_sub(observed) <= MAX_ANDROID_OBSERVATION_AGE_MS
        });
    if !fresh_inventory {
        return Some("guest package inventory is invalid or stale".to_owned());
    }
    if inventory.image_provenance.as_ref() != Some(provenance) {
        return Some("guest inventory image identity does not match the signed catalog".to_owned());
    }
    None
}

fn lifecycle_availability(
    input: &ModelInput<'_>,
    unavailable: Option<String>,
) -> WorkloadAvailability {
    if let Some(pending) = input
        .pending
        .filter(|pending| pending.workload_id == input.workload.workload_id)
    {
        return match pending.operation {
            LifecycleOperation::Start | LifecycleOperation::Retry => WorkloadAvailability::Starting,
            LifecycleOperation::Stop | LifecycleOperation::Cancel => WorkloadAvailability::Stopping,
        };
    }
    if let Some(receipt) = input
        .receipt
        .filter(|receipt| receipt.workload_id == input.workload.workload_id)
    {
        match receipt.phase {
            LifecyclePhase::Starting => return WorkloadAvailability::Starting,
            LifecyclePhase::CheckingGuest => return WorkloadAvailability::CheckingGuest,
            LifecyclePhase::Stopping => return WorkloadAvailability::Stopping,
            LifecyclePhase::Cancelled => return WorkloadAvailability::Cancelled,
            LifecyclePhase::Failed => {
                return WorkloadAvailability::Failed(
                    receipt
                        .failure
                        .clone()
                        .unwrap_or_else(|| "the lifecycle worker reported failure".to_owned()),
                )
            }
            LifecyclePhase::Running | LifecyclePhase::Stopped => {}
        }
    }
    if let Some(reason) = unavailable {
        return WorkloadAvailability::Unavailable(reason);
    }
    if input
        .receipt
        .is_some_and(|receipt| receipt.phase == LifecyclePhase::Running)
        || input.vdi_source.is_some()
    {
        let Some(source) = input.vdi_source else {
            return WorkloadAvailability::Unavailable(
                "Android is running but no typed WebRTC source was published".to_owned(),
            );
        };
        let catalog_digest = input
            .catalog
            .and_then(|snapshot| snapshot.catalog.payload.content_digest().ok());
        let exact_source = source.validate().is_ok()
            && source.workload_id == input.workload.workload_id
            && source.protocol == AndroidVdiProtocol::WebRtc
            && source.observed_at_unix_ms <= input.now_unix_ms
            && source.expires_at_unix_ms > input.now_unix_ms
            && catalog_digest.as_deref() == Some(source.catalog_digest.as_str())
            && input
                .inventory
                .and_then(|inventory| inventory.image_provenance.as_ref())
                .is_some_and(|provenance| {
                    source.image_provenance.image_id == provenance.image_id
                        && source.image_provenance.image_digest == provenance.image_digest
                        && source.image_provenance.source_revision == provenance.source_revision
                        && source.image_provenance.catalog_revision == provenance.catalog_revision
                });
        return if exact_source {
            WorkloadAvailability::Running
        } else {
            WorkloadAvailability::Unavailable(
                "the WebRTC source is stale or does not match catalog/image/generation identity"
                    .to_owned(),
            )
        };
    }
    match input
        .workload
        .runtime_status
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "starting" | "creating" | "booting" => WorkloadAvailability::Starting,
        "stopping" | "shutdown" => WorkloadAvailability::Stopping,
        "running" | "active" => WorkloadAvailability::Unavailable(
            "outer VM is active but guest lifecycle/WebRTC readiness is not proven".to_owned(),
        ),
        status if status.contains("fail") || status.contains("error") => {
            WorkloadAvailability::Failed(format!("outer VM status is {status}"))
        }
        _ => WorkloadAvailability::Ready,
    }
}

fn app_card(
    policy: &AndroidCatalogAppPolicy,
    catalog: &AndroidSignedCatalog,
    inventory: Option<&AndroidAppInventory>,
    ready: bool,
    retry: bool,
) -> AppCard {
    let entry = inventory.and_then(|inventory| {
        inventory
            .entries
            .iter()
            .find(|entry| entry.descriptor.app == policy.app)
    });
    let launchable = entry.is_some_and(|entry| entry.is_launchable());
    let evidence = entry.map_or_else(
        || "No exact guest package evidence".to_owned(),
        |entry| {
            let version = entry.package_version.as_ref().map_or_else(
                || "version unavailable".to_owned(),
                |version| format!("{} ({})", version.version_name, version.version_code),
            );
            let reason = entry
                .unavailable_reason
                .map(|reason| format!(" · {}", reason.label()))
                .unwrap_or_default();
            format!(
                "Package {version} · {} · {}{reason}",
                entry.availability.label(),
                entry.launch_readiness.label()
            )
        },
    );
    AppCard {
        app: policy.app,
        package_id: policy.app.package_id().as_str(),
        permissions: joined_permissions(&policy.permissions),
        capabilities: joined_capabilities(&policy.capabilities),
        approval: format!(
            "Explicit approval · signer {} · catalog revision {}",
            catalog.signer_id, catalog.payload.revision
        ),
        evidence,
        can_start: ready && launchable,
        can_retry: retry && launchable,
    }
}

fn joined_permissions(values: &[AndroidAppPermission]) -> String {
    if values.is_empty() {
        return "None".to_owned();
    }
    values
        .iter()
        .map(|value| match value {
            AndroidAppPermission::Camera => "Camera",
            AndroidAppPermission::Microphone => "Microphone",
            AndroidAppPermission::Location => "Location",
            AndroidAppPermission::Contacts => "Contacts",
            AndroidAppPermission::Calendar => "Calendar",
            AndroidAppPermission::FilesRead => "Files read",
            AndroidAppPermission::FilesWrite => "Files write",
            AndroidAppPermission::Network => "Network",
            AndroidAppPermission::Notifications => "Notifications",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn joined_capabilities(values: &[AndroidAppCapability]) -> String {
    if values.is_empty() {
        return "None".to_owned();
    }
    values
        .iter()
        .map(|value| match value {
            AndroidAppCapability::VdiDisplay => "VDI display",
            AndroidAppCapability::AudioPlayback => "Audio playback",
            AndroidAppCapability::AudioCapture => "Audio capture",
            AndroidAppCapability::CameraInput => "Camera input",
            AndroidAppCapability::LocationInput => "Location input",
            AndroidAppCapability::FilePicker => "File picker",
            AndroidAppCapability::NotificationBridge => "Notification bridge",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn provider_refusal_text(reason: AndroidProviderRefusal) -> &'static str {
    match reason {
        AndroidProviderRefusal::CatalogUnavailable => "signed catalog unavailable",
        AndroidProviderRefusal::CatalogExpired => "signed catalog expired",
        AndroidProviderRefusal::CatalogImageMismatch => "catalog image identity mismatch",
        AndroidProviderRefusal::PackageManifestUnavailable => "package manifest unavailable",
        AndroidProviderRefusal::PackageManifestMismatch => "package manifest mismatch",
        AndroidProviderRefusal::DesiredImageMismatch => "desired image identity mismatch",
        AndroidProviderRefusal::ImageArtifactUnavailable => "immutable image artifact unavailable",
        AndroidProviderRefusal::ImageDigestMismatch => "image digest mismatch",
        AndroidProviderRefusal::KvmUnavailable => "KVM unavailable on the placement node",
        AndroidProviderRefusal::NestedVirtualizationUnavailable => {
            "nested virtualization unavailable on the placement node"
        }
        AndroidProviderRefusal::InsufficientVcpu => "insufficient virtual CPU capacity",
        AndroidProviderRefusal::InsufficientMemory => "insufficient memory capacity",
        AndroidProviderRefusal::InsufficientDisk => "insufficient image disk capacity",
        AndroidProviderRefusal::ProviderUnavailable => "libvirt provider unavailable",
    }
}

pub(super) fn panel(
    ui: &mut egui::Ui,
    projections: &[WorkloadProjection],
    interactive: bool,
) -> Option<UiAction> {
    let mut action = None;
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Governed Android apps")
            .size(Style::BODY)
            .strong()
            .color(Style::TEXT),
    );
    muted_note(
        ui,
        "Cards below come only from an admitted signed catalog plus exact provider, package, launcher, lifecycle, and VDI projections.",
    );
    if projections.is_empty() {
        muted_note(
            ui,
            "No Android workload is present in the current Workloads mirror.",
        );
        return None;
    }
    for projection in projections {
        ui.add_space(Style::SP_XS);
        card().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(&projection.workload_id)
                        .strong()
                        .color(Style::TEXT),
                );
                ui.label(RichText::new(format!("on {}", projection.node)).color(Style::TEXT_DIM));
                ui.colored_label(
                    projection.availability.tone(),
                    projection.availability.label(),
                );
            });
            ui.label(
                RichText::new(&projection.signed_identity)
                    .small()
                    .monospace()
                    .color(Style::TEXT_DIM),
            );
            ui.horizontal_wrapped(|ui| {
                if let Some(source) = &projection.vdi_source {
                    if ui
                        .add_enabled(interactive, egui::Button::new("Open session"))
                        .clicked()
                    {
                        action = Some(UiAction::Attach {
                            node: projection.node.clone(),
                            source: source.clone(),
                        });
                    }
                }
                if ui
                    .add_enabled(
                        interactive && projection.can_stop,
                        egui::Button::new("Stop"),
                    )
                    .clicked()
                {
                    action = Some(UiAction::Lifecycle {
                        node: projection.node.clone(),
                        workload_id: projection.workload_id.clone(),
                        operation: LifecycleOperation::Stop,
                        app: None,
                        expected_generation: projection.expected_generation,
                        approval:
                            "Stop the exact generation and revoke its guest session/VDI source"
                                .to_owned(),
                    });
                }
                if ui
                    .add_enabled(
                        interactive && projection.can_cancel,
                        egui::Button::new("Cancel"),
                    )
                    .clicked()
                {
                    action = Some(UiAction::Lifecycle {
                        node: projection.node.clone(),
                        workload_id: projection.workload_id.clone(),
                        operation: LifecycleOperation::Cancel,
                        app: None,
                        expected_generation: projection.expected_generation,
                        approval: "Cancel the in-flight generation and clean up guest and outer VM"
                            .to_owned(),
                    });
                }
            });
            if projection.cards.is_empty() {
                ui.colored_label(
                    Style::DANGER,
                    "No signed app policy cards are available; start and retry are disabled.",
                );
            }
            for card_model in &projection.cards {
                ui.separator();
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(card_model.app.display_name())
                                .strong()
                                .color(Style::TEXT),
                        );
                        ui.label(
                            RichText::new(card_model.package_id)
                                .monospace()
                                .color(Style::TEXT_DIM),
                        );
                    });
                    ui.label(format!("Permissions: {}", card_model.permissions));
                    ui.label(format!("Capabilities: {}", card_model.capabilities));
                    ui.label(
                        RichText::new(&card_model.approval)
                            .small()
                            .color(Style::TEXT_DIM),
                    );
                    ui.label(
                        RichText::new(&card_model.evidence)
                            .small()
                            .color(Style::TEXT_DIM),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .add_enabled(
                                interactive && card_model.can_start,
                                egui::Button::new("Review start"),
                            )
                            .clicked()
                        {
                            action = Some(UiAction::Lifecycle {
                                node: projection.node.clone(),
                                workload_id: projection.workload_id.clone(),
                                operation: LifecycleOperation::Start,
                                app: Some(card_model.app),
                                expected_generation: projection.expected_generation,
                                approval: format!(
                                    "{} · Permissions: {} · Capabilities: {}",
                                    card_model.approval,
                                    card_model.permissions,
                                    card_model.capabilities
                                ),
                            });
                        }
                        if ui
                            .add_enabled(
                                interactive && card_model.can_retry,
                                egui::Button::new("Review retry"),
                            )
                            .clicked()
                        {
                            action = Some(UiAction::Lifecycle {
                                node: projection.node.clone(),
                                workload_id: projection.workload_id.clone(),
                                operation: LifecycleOperation::Retry,
                                app: Some(card_model.app),
                                expected_generation: projection.expected_generation,
                                approval: format!(
                                    "{} · Permissions: {} · Capabilities: {}",
                                    card_model.approval,
                                    card_model.permissions,
                                    card_model.capabilities
                                ),
                            });
                        }
                    });
                });
            }
        });
    }
    action
}
