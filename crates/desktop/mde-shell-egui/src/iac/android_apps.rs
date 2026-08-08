//! Active lifecycle-first projection of the governed AOSP starter catalog.
//!
//! The panel consumes the optional Android inventory mirror when one is
//! present, but remains honest about the current provider boundary: the cloud
//! worker publishes only admitted inventory evidence, and the panel exposes
//! the existing typed launcher affordance only for a fresh, launch-ready row.

use std::collections::BTreeMap;

use mackes_mesh_types::android_apps::{
    pending_starter_entries, AndroidAppInventory, AndroidAppInventoryEntry, AndroidGuestBootState,
    AndroidImageProvenance, AndroidLauncherResolvability, AndroidUnavailableReason, AospStarterApp,
    AOSP_STARTER_APP_COUNT, MAX_ANDROID_OBSERVATION_AGE_MS,
};
use mackes_mesh_types::cloud::MAX_ANDROID_INVENTORIES_PER_STATE;
use mde_egui::egui::{self, RichText};
use mde_egui::{card, inset, muted_note, status_dot, Style};

const NO_ANDROID_VM_SCOPE: &str = "No Android VM reported";

/// One outer Workloads Android VM row and its stable inventory lookup key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AndroidVmWorkload {
    /// Existing Workloads identity; this is also `AndroidAppInventory.workload_id`.
    pub(super) workload_id: String,
    /// Path-safe placement node that owns the Android guest provider.
    pub(super) target_host: String,
    /// Display-only placement scope (workload name + node).
    pub(super) vm_scope: String,
}

/// One admitted starter-app selection returned by the catalog UI.
///
/// The caller owns the confirmation and capability gate. This projection only
/// returns the closed app identity plus the workload/node identities already
/// present in the folded Workloads mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AndroidAppLaunchSelection {
    pub(super) target_host: String,
    pub(super) workload_id: String,
    pub(super) app: AospStarterApp,
}

/// The evidence state rendered for one Android VM card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AndroidInventoryStatus {
    /// No guest observation has arrived, including the explicit daemon pending
    /// mirror and the outer-row-only legacy mirror.
    Pending,
    /// The retained provider observation names an observation-stale reason.
    Stale,
    /// The provider reports an unavailable guest or image.
    Unavailable,
    /// A valid guest inventory was observed.
    Observed,
}

/// Progress for one workload-keyed Android inventory mirror.
///
/// This is a shell projection, not a second wire contract. The payloads stay
/// on the closed Android contract enums so the card cannot invent a command,
/// provider state, or launch capability while it describes what is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AndroidInventoryProgress {
    /// The provider has not completed an admitted observation yet.
    Pending(AndroidGuestBootState),
    /// The retained observation crossed the bounded freshness window.
    Stale,
    /// The provider reported a closed unavailable reason. `None` is retained
    /// only as a defensive projection state; canonical input never admits it.
    Unavailable(Option<AndroidUnavailableReason>),
    /// A current, validated guest inventory was observed.
    Observed,
}

impl AndroidInventoryProgress {
    fn label(self) -> String {
        match self {
            Self::Pending(boot_state) => format!("pending · {}", boot_state.label()),
            Self::Stale => "stale · observation stale".to_owned(),
            Self::Unavailable(reason) => reason.map_or_else(
                || "unavailable · reason not supplied".to_owned(),
                |reason| format!("unavailable · {}", reason.label()),
            ),
            Self::Observed => "observed".to_owned(),
        }
    }

    const fn tone(self) -> egui::Color32 {
        match self {
            Self::Pending(_) | Self::Stale => Style::WARN,
            Self::Unavailable(_) => Style::DANGER,
            Self::Observed => Style::OK,
        }
    }
}

/// Read-only retry projection for an Android inventory card.
///
/// `Eligible` means the closed reason is transient and the existing provider
/// poll may safely try again. It does not mean this file emits a retry command;
/// the CloudWorker/provider seam owns that operation. `Blocked` covers facts
/// such as a missing package/image that a repeated observation cannot repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AndroidInventoryRetryState {
    AwaitingObservation,
    Eligible(AndroidUnavailableReason),
    Blocked(Option<AndroidUnavailableReason>),
    NotNeeded,
}

impl AndroidInventoryRetryState {
    fn label(self) -> String {
        match self {
            Self::AwaitingObservation => "awaiting typed observation".to_owned(),
            Self::Eligible(reason) => format!("eligible · {}", reason.label()),
            Self::Blocked(reason) => reason.map_or_else(
                || "blocked · reason not supplied".to_owned(),
                |reason| format!("blocked · {}", reason.label()),
            ),
            Self::NotNeeded => "not needed".to_owned(),
        }
    }

    const fn tone(self) -> egui::Color32 {
        match self {
            Self::AwaitingObservation | Self::Eligible(_) => Style::WARN,
            Self::Blocked(_) => Style::DANGER,
            Self::NotNeeded => Style::TEXT_DIM,
        }
    }
}

/// Whether the current card has workload-scoped, read-only evidence to
/// inspect. This deliberately has no click payload or command string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AndroidInventoryInspectState {
    Available,
    Unscoped,
}

impl AndroidInventoryInspectState {
    const fn label(self) -> &'static str {
        match self {
            Self::Available => "read-only evidence",
            Self::Unscoped => "unscoped catalog",
        }
    }

    const fn tone(self) -> egui::Color32 {
        match self {
            Self::Available => Style::TEXT,
            Self::Unscoped => Style::TEXT_DIM,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AndroidInventoryCardState {
    progress: AndroidInventoryProgress,
    retry: AndroidInventoryRetryState,
    inspect: AndroidInventoryInspectState,
}

impl AndroidInventoryCardState {
    fn for_projection(workload_id: Option<&str>, inventory: Option<&AndroidAppInventory>) -> Self {
        let progress = inventory_progress(inventory);
        let retry = match progress {
            AndroidInventoryProgress::Pending(_) => AndroidInventoryRetryState::AwaitingObservation,
            AndroidInventoryProgress::Stale => {
                AndroidInventoryRetryState::Eligible(AndroidUnavailableReason::ObservationStale)
            }
            AndroidInventoryProgress::Unavailable(reason) => {
                reason.map_or(AndroidInventoryRetryState::Blocked(None), |reason| {
                    if retryable_reason(reason) {
                        AndroidInventoryRetryState::Eligible(reason)
                    } else {
                        AndroidInventoryRetryState::Blocked(Some(reason))
                    }
                })
            }
            AndroidInventoryProgress::Observed => AndroidInventoryRetryState::NotNeeded,
        };
        let inspect = if workload_id.is_some() {
            AndroidInventoryInspectState::Available
        } else {
            AndroidInventoryInspectState::Unscoped
        };
        Self {
            progress,
            retry,
            inspect,
        }
    }
}

impl AndroidInventoryStatus {
    const fn label(self) -> &'static str {
        match self {
            Self::Pending => "inventory pending",
            Self::Stale => "inventory stale",
            Self::Unavailable => "inventory unavailable",
            Self::Observed => "inventory observed",
        }
    }

    const fn tone(self) -> egui::Color32 {
        match self {
            Self::Pending | Self::Stale => Style::WARN,
            Self::Unavailable => Style::DANGER,
            Self::Observed => Style::OK,
        }
    }
}

/// The bounded non-package evidence shown above an Android app list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AndroidInventoryEvidence {
    guest_boot_state: AndroidGuestBootState,
    image_provenance: Option<AndroidImageProvenance>,
    observed_at_unix_ms: Option<u64>,
    observation_age_ms: Option<u64>,
    unavailable_reason: Option<AndroidUnavailableReason>,
}

impl AndroidInventoryEvidence {
    fn pending() -> Self {
        Self {
            guest_boot_state: AndroidGuestBootState::Pending,
            image_provenance: None,
            observed_at_unix_ms: None,
            observation_age_ms: None,
            unavailable_reason: None,
        }
    }

    fn from_inventory(inventory: &AndroidAppInventory) -> Self {
        Self {
            guest_boot_state: inventory.guest_boot_state,
            image_provenance: inventory.image_provenance.clone(),
            observed_at_unix_ms: inventory.observed_at_unix_ms,
            observation_age_ms: inventory.observation_age_ms,
            unavailable_reason: inventory.unavailable_reason,
        }
    }
}

/// The shell-side projection of one Android VM's governed starter catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AndroidVmCatalogProjection {
    workload_id: Option<String>,
    target_host: Option<String>,
    vm_scope: String,
    entries: Vec<AndroidAppInventoryEntry>,
    launch_enabled: Vec<bool>,
    evidence: AndroidInventoryEvidence,
    status: AndroidInventoryStatus,
    card_state: AndroidInventoryCardState,
}

/// Render the governed starter-image expectations and any admitted guest
/// inventory for the Android VM filter.
pub(super) fn catalog_panel(
    ui: &mut egui::Ui,
    vm_workloads: &[AndroidVmWorkload],
    inventories: &[AndroidAppInventory],
) -> Option<AndroidAppLaunchSelection> {
    let projections = vm_catalog(vm_workloads, inventories);
    let mut selection = None;
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("AOSP starter apps")
            .size(Style::BODY)
            .strong()
            .color(Style::TEXT),
    );
    muted_note(
        ui,
        format!(
            "{} The governed {AOSP_STARTER_APP_COUNT}-app identities below are typed starter-image expectations. Guest evidence is shown only from a valid workload-keyed inventory mirror; launch remains gated by admitted freshness and readiness.",
            scope_text(vm_workloads),
        ),
    );
    ui.add_space(Style::SP_XS);

    for (index, projection) in projections.iter().enumerate() {
        card().show(ui, |ui| {
            vm_scope_header(ui, projection);
            ui.separator();
            inventory_evidence(ui, &projection.evidence);
            inventory_card_state(ui, &projection.card_state);
            ui.separator();
            for (entry_index, entry) in projection.entries.iter().enumerate() {
                starter_app_row(
                    ui,
                    entry,
                    projection.workload_id.as_deref(),
                    projection.target_host.as_deref(),
                    projection
                        .launch_enabled
                        .get(entry_index)
                        .copied()
                        .unwrap_or(false),
                    &mut selection,
                );
                if entry_index + 1 < projection.entries.len() {
                    ui.separator();
                }
            }
        });
        if index + 1 < projections.len() {
            ui.add_space(Style::SP_XS);
        }
    }
    ui.add_space(Style::SP_S);
    selection
}

/// Fold outer Android workload rows with valid inventory records. Workload IDs
/// and duplicate records are canonicalized before matching, so mirror order or
/// duplicate retained publications cannot change the rendered card.
fn vm_catalog(
    vm_workloads: &[AndroidVmWorkload],
    inventories: &[AndroidAppInventory],
) -> Vec<AndroidVmCatalogProjection> {
    let workloads = canonical_vm_workloads(vm_workloads);
    let inventories = canonical_inventories(inventories);

    if workloads.is_empty() {
        let entries = pending_starter_entries();
        return vec![AndroidVmCatalogProjection {
            workload_id: None,
            target_host: None,
            vm_scope: NO_ANDROID_VM_SCOPE.to_owned(),
            launch_enabled: vec![false; entries.len()],
            entries,
            evidence: AndroidInventoryEvidence::pending(),
            status: AndroidInventoryStatus::Pending,
            card_state: AndroidInventoryCardState::for_projection(None, None),
        }];
    }

    workloads
        .into_iter()
        .map(|workload| {
            inventories.get(&workload.workload_id).map_or_else(
                || pending_projection(&workload),
                |inventory| observed_projection(&workload, inventory),
            )
        })
        .collect()
}

fn pending_projection(workload: &AndroidVmWorkload) -> AndroidVmCatalogProjection {
    let entries = pending_starter_entries();
    AndroidVmCatalogProjection {
        workload_id: Some(workload.workload_id.clone()),
        target_host: Some(workload.target_host.clone()),
        vm_scope: workload.vm_scope.clone(),
        launch_enabled: vec![false; entries.len()],
        entries,
        evidence: AndroidInventoryEvidence::pending(),
        status: AndroidInventoryStatus::Pending,
        card_state: AndroidInventoryCardState::for_projection(
            Some(workload.workload_id.as_str()),
            None,
        ),
    }
}

fn observed_projection(
    workload: &AndroidVmWorkload,
    inventory: &AndroidAppInventory,
) -> AndroidVmCatalogProjection {
    let launch_enabled = inventory
        .entries
        .iter()
        .map(|entry| launch_is_enabled(inventory, entry))
        .collect();
    AndroidVmCatalogProjection {
        workload_id: Some(workload.workload_id.clone()),
        target_host: Some(workload.target_host.clone()),
        vm_scope: workload.vm_scope.clone(),
        entries: inventory.entries.clone(),
        launch_enabled,
        evidence: AndroidInventoryEvidence::from_inventory(inventory),
        status: inventory_status(inventory),
        card_state: AndroidInventoryCardState::for_projection(
            Some(workload.workload_id.as_str()),
            Some(inventory),
        ),
    }
}

/// Drop malformed records before they reach the shell and select one record per
/// workload ID. A real observed record outranks the daemon's explicit pending
/// mirror; ties use the canonical JSON representation for deterministic output.
fn canonical_inventories<'a>(
    inventories: &'a [AndroidAppInventory],
) -> BTreeMap<String, &'a AndroidAppInventory> {
    let mut candidates = inventories
        .iter()
        .take(MAX_ANDROID_INVENTORIES_PER_STATE)
        .filter(|inventory| inventory.validate().is_ok())
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        inventory_rank(left)
            .cmp(&inventory_rank(right))
            .then_with(|| left.workload_id.cmp(&right.workload_id))
            .then_with(|| inventory_key(left).cmp(&inventory_key(right)))
    });

    let mut canonical = BTreeMap::new();
    for inventory in candidates {
        canonical
            .entry(inventory.workload_id.clone())
            .or_insert(inventory);
    }
    canonical
}

fn canonical_vm_workloads(vm_workloads: &[AndroidVmWorkload]) -> Vec<AndroidVmWorkload> {
    let mut canonical = BTreeMap::<String, (String, String)>::new();
    for workload in vm_workloads {
        let workload_id = workload.workload_id.trim();
        let target_host = workload.target_host.trim();
        if workload_id.is_empty() || target_host.is_empty() {
            continue;
        }
        let vm_scope = {
            let scope = workload.vm_scope.trim();
            if scope.is_empty() {
                workload_id
            } else {
                scope
            }
        };
        let candidate = (target_host.to_owned(), vm_scope.to_owned());
        canonical
            .entry(workload_id.to_owned())
            .and_modify(|existing| {
                if candidate < *existing {
                    *existing = candidate.clone();
                }
            })
            .or_insert(candidate);
    }
    canonical
        .into_iter()
        .map(|(workload_id, (target_host, vm_scope))| AndroidVmWorkload {
            workload_id,
            target_host,
            vm_scope,
        })
        .collect()
}

fn inventory_rank(inventory: &AndroidAppInventory) -> u8 {
    match inventory_status(inventory) {
        AndroidInventoryStatus::Observed => 0,
        AndroidInventoryStatus::Stale => 1,
        AndroidInventoryStatus::Unavailable => 2,
        AndroidInventoryStatus::Pending => 3,
    }
}

fn inventory_status(inventory: &AndroidAppInventory) -> AndroidInventoryStatus {
    if inventory.unavailable_reason == Some(AndroidUnavailableReason::ObservationStale) {
        return AndroidInventoryStatus::Stale;
    }
    match inventory.guest_boot_state {
        AndroidGuestBootState::Pending | AndroidGuestBootState::Booting => {
            AndroidInventoryStatus::Pending
        }
        AndroidGuestBootState::Ready => AndroidInventoryStatus::Observed,
        AndroidGuestBootState::Unavailable => AndroidInventoryStatus::Unavailable,
    }
}

fn inventory_progress(inventory: Option<&AndroidAppInventory>) -> AndroidInventoryProgress {
    let Some(inventory) = inventory else {
        return AndroidInventoryProgress::Pending(AndroidGuestBootState::Pending);
    };
    match inventory_status(inventory) {
        AndroidInventoryStatus::Pending => {
            AndroidInventoryProgress::Pending(inventory.guest_boot_state)
        }
        AndroidInventoryStatus::Stale => AndroidInventoryProgress::Stale,
        AndroidInventoryStatus::Unavailable => {
            AndroidInventoryProgress::Unavailable(inventory.unavailable_reason)
        }
        AndroidInventoryStatus::Observed => AndroidInventoryProgress::Observed,
    }
}

/// Reasons that can plausibly recover on a typed provider poll. Image/package
/// facts remain blocked until the admitted image/catalog changes.
const fn retryable_reason(reason: AndroidUnavailableReason) -> bool {
    matches!(
        reason,
        AndroidUnavailableReason::GuestUnavailable
            | AndroidUnavailableReason::GuestBootFailed
            | AndroidUnavailableReason::PackageManagerUnavailable
            | AndroidUnavailableReason::ProviderUnavailable
            | AndroidUnavailableReason::CapacityUnavailable
            | AndroidUnavailableReason::TransportUnavailable
            | AndroidUnavailableReason::ObservationStale
    )
}

fn inventory_key(inventory: &AndroidAppInventory) -> String {
    // This serializes only the closed, already-validated contract and is used
    // solely as a deterministic duplicate tie-breaker.
    serde_json::to_string(inventory).unwrap_or_default()
}

/// Trim, sort, and deduplicate the VM rows before they reach the rendered model.
fn scope_text(vm_workloads: &[AndroidVmWorkload]) -> String {
    let workloads = canonical_vm_workloads(vm_workloads);
    match workloads.as_slice() {
        [] => "No Android VM workload is reporting; the catalog is shown without a VM scope."
            .to_owned(),
        [workload] => format!(
            "Android VM {} is reported; its workload-keyed guest inventory is rendered as available evidence or remains pending.",
            workload.vm_scope
        ),
        workloads => format!(
            "{} Android VMs are reported ({}); each VM has its own workload-keyed guest inventory mirror.",
            workloads.len(),
            workloads
                .iter()
                .map(|workload| workload.vm_scope.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn vm_scope_header(ui: &mut egui::Ui, projection: &AndroidVmCatalogProjection) {
    let tone = projection.status.tone();
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new("VM scope")
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT_DIM),
        );
        category_tag(ui, "Android VM");
        ui.label(
            RichText::new(&projection.vm_scope)
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT),
        );
        if let Some(workload_id) = projection.workload_id.as_deref() {
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(format!("key: {workload_id}"))
                    .size(Style::SMALL)
                    .monospace(),
            );
        }
        status_dot(ui, tone);
        ui.colored_label(
            tone,
            RichText::new(projection.status.label()).size(Style::SMALL),
        );
    });
}

fn inventory_evidence(ui: &mut egui::Ui, evidence: &AndroidInventoryEvidence) {
    muted_note(ui, evidence_text(evidence));
}

/// Render projection facts only. These labels are not buttons: this file has
/// no typed retry/inspect dispatch seam, so it must not pretend to execute one.
fn inventory_card_state(ui: &mut egui::Ui, state: &AndroidInventoryCardState) {
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(
            state.progress.tone(),
            RichText::new(format!("Progress: {}", state.progress.label())).size(Style::SMALL),
        );
        ui.colored_label(
            state.retry.tone(),
            RichText::new(format!("Retry: {}", state.retry.label())).size(Style::SMALL),
        );
        ui.colored_label(
            state.inspect.tone(),
            RichText::new(format!("Inspect: {}", state.inspect.label())).size(Style::SMALL),
        );
    });
}

fn evidence_text(evidence: &AndroidInventoryEvidence) -> String {
    let observation = match (evidence.observed_at_unix_ms, evidence.observation_age_ms) {
        (Some(observed_at), Some(age)) => format!("at {observed_at} ms, age {age} ms"),
        _ => "not observed".to_owned(),
    };
    let provenance = evidence.image_provenance.as_ref().map_or_else(
        || "not admitted".to_owned(),
        |provenance| {
            format!(
                "{} / {} / {} / {}",
                provenance.image_id,
                provenance.image_digest,
                provenance.source_revision,
                provenance.catalog_revision
            )
        },
    );
    let reason = evidence
        .unavailable_reason
        .map_or("none", AndroidUnavailableReason::label);
    format!(
        "Guest boot: {} · Observation: {observation} · Image provenance: {provenance} · Reason: {reason}",
        evidence.guest_boot_state.label()
    )
}

/// Render one immutable identity plus bounded package/version evidence. Launch
/// is enabled only from the admitted inventory projection; the action remains
/// the shared closed `MAIN` + `LAUNCHER` path, never a host command or ADB
/// fallback.
fn starter_app_row(
    ui: &mut egui::Ui,
    entry: &AndroidAppInventoryEntry,
    workload_id: Option<&str>,
    target_host: Option<&str>,
    launch_enabled: bool,
    selection: &mut Option<AndroidAppLaunchSelection>,
) {
    let tone = if entry.unavailable_reason.is_some() {
        Style::DANGER
    } else if entry.package_version.is_some() {
        Style::OK
    } else {
        Style::WARN
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(entry.descriptor.app.display_name())
                .size(Style::SMALL)
                .strong()
                .color(Style::TEXT),
        );
        category_tag(ui, entry.descriptor.category.label());
        ui.label(
            RichText::new(entry.descriptor.package_id.as_str())
                .size(Style::SMALL)
                .monospace()
                .color(Style::TEXT_DIM),
        );
        status_dot(ui, tone);
        ui.colored_label(
            tone,
            RichText::new(entry.availability.label()).size(Style::SMALL),
        );
        ui.colored_label(
            tone,
            RichText::new(entry.readiness.label()).size(Style::SMALL),
        );
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(entry_evidence_text(entry)).size(Style::SMALL),
        );
        let launch_button = ui.add_enabled(
            launch_enabled,
            egui::Button::new(RichText::new(entry.launch_readiness.label()).size(Style::SMALL)),
        );
        if launch_button.clicked() && launch_enabled {
            if let (Some(workload_id), Some(target_host)) = (workload_id, target_host) {
                *selection = Some(AndroidAppLaunchSelection {
                    target_host: target_host.to_owned(),
                    workload_id: workload_id.to_owned(),
                    app: entry.descriptor.app,
                });
            }
        }
        launch_button.on_hover_text(if launch_enabled {
            "Launch uses only the admitted closed MAIN + LAUNCHER action for this Android app."
        } else {
            "Launch requires a fresh admitted Android inventory entry that is installed, ready, launcher-resolved, and dispatch-ready."
        });
    });
}

fn entry_evidence_text(entry: &AndroidAppInventoryEntry) -> String {
    let version = entry.package_version.as_ref().map_or_else(
        || "version pending".to_owned(),
        |version| {
            format!(
                "version {} (code {})",
                version.version_name, version.version_code
            )
        },
    );
    let reason = entry
        .unavailable_reason
        .map_or("reason none", AndroidUnavailableReason::label);
    format!(
        "package {} · {version} · launcher {} · {reason}",
        entry.descriptor.package_id.as_str(),
        launcher_label(entry.launcher_resolvability),
    )
}

fn launcher_label(resolvability: AndroidLauncherResolvability) -> &'static str {
    match resolvability {
        AndroidLauncherResolvability::Pending => "launcher pending",
        AndroidLauncherResolvability::Resolved => "launcher resolved",
        AndroidLauncherResolvability::Unavailable => "launcher unavailable",
    }
}

/// Gate the UI control on the complete admitted inventory and the shared typed
/// entry action contract. The strict `<` freshness check keeps an observation
/// at the retention boundary disabled until a newer provider observation lands;
/// the provider's stale projection also carries a closed unavailable reason.
fn launch_is_enabled(inventory: &AndroidAppInventory, entry: &AndroidAppInventoryEntry) -> bool {
    inventory.validate().is_ok()
        && inventory.guest_boot_state == AndroidGuestBootState::Ready
        && inventory.image_provenance.is_some()
        && inventory.unavailable_reason.is_none()
        && inventory
            .observation_age_ms
            .is_some_and(|age| age < MAX_ANDROID_OBSERVATION_AGE_MS)
        && inventory.entries.iter().any(|candidate| candidate == entry)
        && entry.is_launchable()
}

fn category_tag(ui: &mut egui::Ui, label: &str) {
    inset().show(ui, |ui| {
        ui.label(
            RichText::new(label)
                .size(Style::SMALL)
                .color(Style::ACCENT_WORKLOADS),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::android_apps::{
        pending_starter_entries, AndroidAppAvailability, AndroidAppCategory,
        AndroidAppContractError, AndroidAppInventory, AndroidAppReadiness, AndroidGuestBootState,
        AndroidImageProvenance, AndroidLaunchReadiness, AndroidLauncherResolvability,
        AndroidPackageVersion, AospStarterApp, AOSP_STARTER_APP_COUNT,
    };

    fn workload(workload_id: &str, vm_scope: &str) -> AndroidVmWorkload {
        AndroidVmWorkload {
            workload_id: workload_id.to_owned(),
            target_host: "test-node".to_owned(),
            vm_scope: vm_scope.to_owned(),
        }
    }

    fn valid_provenance() -> AndroidImageProvenance {
        AndroidImageProvenance::new(
            "aosp-cuttlefish-2026-08",
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "aosp-source-2026-08",
            "starter-catalog-v1",
        )
        .expect("valid Android image provenance")
    }

    fn observed_inventory(workload_id: &str) -> AndroidAppInventory {
        let mut entries = pending_starter_entries();
        for entry in &mut entries {
            entry.availability = AndroidAppAvailability::Installed;
            entry.package_version = Some(AndroidPackageVersion::new("1.0.0", 1).unwrap());
            entry.readiness = AndroidAppReadiness::Ready;
            entry.launcher_resolvability = AndroidLauncherResolvability::Resolved;
            entry.launch_readiness = AndroidLaunchReadiness::IntegrationPending;
        }
        AndroidAppInventory::observed(
            workload_id,
            valid_provenance(),
            AndroidGuestBootState::Ready,
            1_786_000_000_000,
            100,
            entries,
        )
        .expect("valid observed Android inventory")
    }

    fn unavailable_inventory(
        workload_id: &str,
        reason: AndroidUnavailableReason,
    ) -> AndroidAppInventory {
        let mut inventory = AndroidAppInventory::pending(workload_id);
        inventory.guest_boot_state = AndroidGuestBootState::Unavailable;
        inventory.observed_at_unix_ms = Some(1_786_000_000_000);
        inventory.observation_age_ms = Some(100);
        inventory.unavailable_reason = Some(reason);
        for entry in &mut inventory.entries {
            entry.availability = AndroidAppAvailability::ImageUnavailable;
            entry.readiness = AndroidAppReadiness::Unavailable;
            entry.launcher_resolvability = AndroidLauncherResolvability::Unavailable;
            entry.launch_readiness = AndroidLaunchReadiness::Unavailable;
            entry.unavailable_reason = Some(AndroidUnavailableReason::ImageUnavailable);
        }
        assert!(inventory.validate().is_ok());
        inventory
    }

    #[test]
    fn governed_catalog_is_nine_pending_non_launchable_entries() {
        let entries = pending_starter_entries();
        assert_eq!(entries.len(), AOSP_STARTER_APP_COUNT);
        assert!(entries.iter().all(|entry| {
            entry.availability == AndroidAppAvailability::InventoryPending
                && entry.readiness == AndroidAppReadiness::GuestPending
                && entry.launch_readiness == AndroidLaunchReadiness::IntegrationPending
                && !entry.is_launchable()
        }));
    }

    #[test]
    fn scope_copy_names_zero_one_and_multiple_android_vms() {
        assert_eq!(
            scope_text(&[]),
            "No Android VM workload is reporting; the catalog is shown without a VM scope."
        );
        assert!(scope_text(&[workload("android-1", "android-1 on eagle")])
            .contains("Android VM android-1 on eagle is reported"));
        assert!(scope_text(&[
            workload("android-2", "android-2 on falcon"),
            workload("android-1", "android-1 on eagle"),
            workload("android-2", "android-2 on falcon"),
        ])
        .contains("2 Android VMs are reported (android-1 on eagle, android-2 on falcon)"));
    }

    #[test]
    fn projection_matches_by_workload_id_and_sorts_deduplicates_deterministically() {
        let observed = observed_inventory("android-1");
        let projections = vm_catalog(
            &[
                workload("android-2", "android-2 on falcon"),
                workload("android-1", "android-1 on zulu"),
                workload("android-1", "android-1 on eagle"),
                workload("android-2", "android-2 on falcon"),
            ],
            &[
                AndroidAppInventory::pending("android-1"),
                observed.clone(),
                observed,
            ],
        );

        assert_eq!(projections.len(), 2);
        assert_eq!(
            projections
                .iter()
                .map(|projection| projection.workload_id.as_deref())
                .collect::<Vec<_>>(),
            [Some("android-1"), Some("android-2")]
        );
        assert_eq!(projections[0].vm_scope, "android-1 on eagle");
        assert_eq!(projections[0].status, AndroidInventoryStatus::Observed);
        assert_eq!(
            projections[0].entries[0]
                .package_version
                .as_ref()
                .map(|version| version.version_name.as_str()),
            Some("1.0.0")
        );
        assert!(evidence_text(&projections[0].evidence).contains("guest ready"));
        assert!(evidence_text(&projections[0].evidence).contains("age 100 ms"));
        assert!(evidence_text(&projections[0].evidence).contains("aosp-cuttlefish-2026-08"));
        assert!(entry_evidence_text(&projections[0].entries[0]).contains("version 1.0.0 (code 1)"));
        assert!(entry_evidence_text(&projections[0].entries[0]).contains("reason none"));
        assert!(projections
            .iter()
            .all(|projection| projection.launch_enabled.iter().all(|enabled| !*enabled)));
    }

    #[test]
    fn pending_stale_and_unavailable_inventory_keep_honest_copy_and_disabled_launch() {
        let stale =
            unavailable_inventory("android-stale", AndroidUnavailableReason::ObservationStale);
        let unavailable = unavailable_inventory(
            "android-unavailable",
            AndroidUnavailableReason::GuestUnavailable,
        );
        let projections = vm_catalog(
            &[
                workload("android-pending", "android-pending on eagle"),
                workload("android-stale", "android-stale on eagle"),
                workload("android-unavailable", "android-unavailable on eagle"),
            ],
            &[
                AndroidAppInventory::pending("android-pending"),
                stale,
                unavailable,
            ],
        );

        assert_eq!(projections[0].status, AndroidInventoryStatus::Pending);
        assert_eq!(projections[1].status, AndroidInventoryStatus::Stale);
        assert!(evidence_text(&projections[1].evidence).contains("observation stale"));
        assert_eq!(projections[2].status, AndroidInventoryStatus::Unavailable);
        assert!(evidence_text(&projections[2].evidence).contains("guest unavailable"));
        assert!(projections
            .iter()
            .all(|projection| projection.launch_enabled.iter().all(|enabled| !*enabled)));
    }

    #[test]
    fn card_state_uses_typed_progress_retry_and_inspect_for_each_inventory_state() {
        let observed = observed_inventory("android-observed");
        let stale =
            unavailable_inventory("android-stale", AndroidUnavailableReason::ObservationStale);
        let transient = unavailable_inventory(
            "android-transient",
            AndroidUnavailableReason::GuestUnavailable,
        );
        let image_unavailable =
            unavailable_inventory("android-image", AndroidUnavailableReason::ImageUnavailable);
        let projections = vm_catalog(
            &[
                workload("android-image", "android-image on eagle"),
                workload("android-observed", "android-observed on eagle"),
                workload("android-pending", "android-pending on eagle"),
                workload("android-stale", "android-stale on eagle"),
                workload("android-transient", "android-transient on eagle"),
            ],
            &[
                AndroidAppInventory::pending("android-pending"),
                image_unavailable,
                observed,
                stale,
                transient,
            ],
        );
        let state_for = |workload_id: &str| {
            projections
                .iter()
                .find(|projection| projection.workload_id.as_deref() == Some(workload_id))
                .expect("projection for workload")
                .card_state
        };

        let pending = state_for("android-pending");
        assert_eq!(
            pending.progress,
            AndroidInventoryProgress::Pending(AndroidGuestBootState::Pending)
        );
        assert_eq!(
            pending.retry,
            AndroidInventoryRetryState::AwaitingObservation
        );
        assert_eq!(pending.inspect, AndroidInventoryInspectState::Available);

        let stale = state_for("android-stale");
        assert_eq!(stale.progress, AndroidInventoryProgress::Stale);
        assert_eq!(
            stale.retry,
            AndroidInventoryRetryState::Eligible(AndroidUnavailableReason::ObservationStale)
        );
        assert_eq!(stale.inspect, AndroidInventoryInspectState::Available);

        let transient = state_for("android-transient");
        assert_eq!(
            transient.progress,
            AndroidInventoryProgress::Unavailable(Some(AndroidUnavailableReason::GuestUnavailable))
        );
        assert_eq!(
            transient.retry,
            AndroidInventoryRetryState::Eligible(AndroidUnavailableReason::GuestUnavailable)
        );

        let image = state_for("android-image");
        assert_eq!(
            image.progress,
            AndroidInventoryProgress::Unavailable(Some(AndroidUnavailableReason::ImageUnavailable))
        );
        assert_eq!(
            image.retry,
            AndroidInventoryRetryState::Blocked(Some(AndroidUnavailableReason::ImageUnavailable))
        );

        let observed = state_for("android-observed");
        assert_eq!(observed.progress, AndroidInventoryProgress::Observed);
        assert_eq!(observed.retry, AndroidInventoryRetryState::NotNeeded);
        assert_eq!(observed.inspect, AndroidInventoryInspectState::Available);
    }

    #[test]
    fn no_vm_projection_is_explicitly_unscoped_but_still_shows_catalog() {
        let projections = vm_catalog(&[], &[]);

        assert_eq!(projections.len(), 1);
        assert_eq!(projections[0].vm_scope, NO_ANDROID_VM_SCOPE);
        assert_eq!(projections[0].entries.len(), AOSP_STARTER_APP_COUNT);
        assert_eq!(
            projections[0].card_state,
            AndroidInventoryCardState {
                progress: AndroidInventoryProgress::Pending(AndroidGuestBootState::Pending),
                retry: AndroidInventoryRetryState::AwaitingObservation,
                inspect: AndroidInventoryInspectState::Unscoped,
            }
        );
        assert!(projections[0]
            .launch_enabled
            .iter()
            .all(|enabled| !*enabled));
    }

    #[test]
    fn malformed_inventory_is_not_projected() {
        let mut malformed = AndroidAppInventory::pending("android-1");
        malformed.workload_id = "../android-1".to_owned();
        assert_eq!(
            malformed.validate(),
            Err(AndroidAppContractError::InvalidWorkloadId)
        );
        let projections = vm_catalog(&[workload("android-1", "android-1 on eagle")], &[malformed]);
        assert_eq!(projections[0].status, AndroidInventoryStatus::Pending);
    }

    #[test]
    fn lifecycle_gate_is_android_plan_and_run_only() {
        use super::super::{should_show_android_starter_catalog, DeliveryView, ResourceTableMode};

        assert!(should_show_android_starter_catalog(
            ResourceTableMode::Plan,
            DeliveryView::AndroidVm
        ));
        assert!(should_show_android_starter_catalog(
            ResourceTableMode::Run,
            DeliveryView::AndroidVm
        ));
        assert!(!should_show_android_starter_catalog(
            ResourceTableMode::Drift,
            DeliveryView::AndroidVm
        ));
        assert!(!should_show_android_starter_catalog(
            ResourceTableMode::Plan,
            DeliveryView::DesktopVm
        ));
        assert!(!should_show_android_starter_catalog(
            ResourceTableMode::Containers,
            DeliveryView::ServiceContainer
        ));
    }

    #[test]
    fn observed_fixture_uses_the_governed_browser_identity() {
        let inventory = observed_inventory("android-1");
        let browser = inventory
            .entries
            .iter()
            .find(|entry| entry.descriptor.app == AospStarterApp::Browser)
            .expect("governed Browser entry");
        assert_eq!(browser.descriptor.category, AndroidAppCategory::Web);
        assert_eq!(
            browser.descriptor.package_id.as_str(),
            "com.android.browser"
        );
    }

    fn launch_ready_inventory(workload_id: &str) -> AndroidAppInventory {
        let mut inventory = observed_inventory(workload_id);
        inventory.entries[0].launch_readiness = AndroidLaunchReadiness::Ready;
        assert!(inventory.validate().is_ok());
        inventory
    }

    #[test]
    fn launch_is_enabled_only_for_a_fresh_admitted_ready_entry() {
        let inventory = launch_ready_inventory("android-ready");
        assert!(launch_is_enabled(&inventory, &inventory.entries[0]));
        assert!(!launch_is_enabled(&inventory, &inventory.entries[1]));

        let mut boundary_age = inventory.clone();
        boundary_age.observation_age_ms = Some(MAX_ANDROID_OBSERVATION_AGE_MS);
        assert!(boundary_age.validate().is_ok());
        assert!(!launch_is_enabled(&boundary_age, &boundary_age.entries[0]));

        let mut not_admitted = inventory.clone();
        not_admitted.entries[0].descriptor.package_id =
            mackes_mesh_types::android_apps::AospPackageId::Calendar;
        assert!(not_admitted.validate().is_err());
        assert!(!launch_is_enabled(&not_admitted, &not_admitted.entries[0]));
    }

    #[test]
    fn launch_is_enabled_rejects_stale_or_unscoped_entries() {
        let stale =
            unavailable_inventory("android-stale", AndroidUnavailableReason::ObservationStale);
        assert!(!launch_is_enabled(&stale, &stale.entries[0]));

        let pending = AndroidAppInventory::pending("android-pending");
        let ready = launch_ready_inventory("android-ready");
        assert!(!launch_is_enabled(&pending, &ready.entries[0]));
    }
}
