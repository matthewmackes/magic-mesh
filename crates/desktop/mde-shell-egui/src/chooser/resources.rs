//! Universal catalog cards rendered beside discovered desktops.
//!
//! The shell consumes the versioned `state/resources/catalog` contract together
//! with its bound `state/resources/discovery` projection and is deliberately
//! adapter-agnostic: a new service kind receives the same card, lifecycle
//! actions, and Local Service Stack placement without a UI code path.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::cloud::{decode_cloud_arm_credential, CloudArmSigner, CLOUD_ARM_CREDENTIAL};
use mackes_mesh_types::resources::{
    ActionAvailabilityStatus, AuthMethod, AuthStatus, HealthStatus, ResourceActionTarget,
    ResourceActionVerb, ResourceCard, ResourceCatalog, ResourceClass, ResourceDiscoveryEntry,
    ResourceDiscoveryProjection, ServiceCategory, ServiceConfigurationFieldKind,
    ServiceLifecycleStatus, ServiceStackTier, TransportEndpoint, TransportProtocol,
    RESOURCE_CATALOG_TOPIC, RESOURCE_DISCOVERY_TOPIC,
};
use mde_bus::persist::Persist;
use mde_egui::egui::{self, Color32, FontId, RichText, Sense, Stroke, StrokeKind};
use mde_egui::{Style, TypographyRole};
use serde::{Deserialize, Serialize};

const RESOURCE_ACTION_TOPIC: &str = "action/resources/invoke";
const VDI_SESSION_ACTION_TOPIC: &str = "action/vdi/session";
const RESOURCE_ACTION_TTL_MS: u64 = 20_000;
const ACTION_REPLY_POLL_MS: u64 = 25;
static RESOURCE_ACTION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ApprovedCatalogDesktop {
    pub resource_id: String,
    pub display_name: String,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcceptedVdiOpen {
    invocation: ResourceActionInvocation,
    receipt: VdiAuthorityCompletionReply,
    binding: VdiConnectBinding,
    handoff: ApprovedCatalogDesktop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VdiConnectBinding {
    catalog_revision: String,
    catalog_content_digest: String,
    resource_id: String,
    canonical_key: String,
    display_name: String,
    action_id: String,
    target: ResourceActionTarget,
    host: String,
    port: u16,
    card_expires_at_ms: u64,
    action_expires_at_ms: u64,
    approved_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalApprovalBinding {
    catalog_revision: String,
    catalog_content_digest: String,
    resource_id: String,
    action_id: String,
    target: ResourceActionTarget,
    approved_at_ms: u64,
    expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum StrictSessionRequest {
    Open {
        id: String,
        serving_peer: String,
        vm_id: String,
        client_peer: String,
        profile: Option<mackes_mesh_types::vdi_session::DesktopSessionProfile>,
    },
    Close {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "authority",
    content = "request",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum TypedAuthorityRequest {
    Vdi(StrictSessionRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceActionInvocation {
    schema_version: u16,
    request_id: String,
    catalog_revision: String,
    catalog_content_digest: String,
    resource_id: String,
    action_id: String,
    verb: ResourceActionVerb,
    target: ResourceActionTarget,
    expected_generation: u64,
    cancellation_id: String,
    cancels_request_id: Option<String>,
    issued_at_ms: u64,
    deadline_at_ms: u64,
    authority_request: TypedAuthorityRequest,
    vdi_open_receipt: Option<VdiAuthorityCompletionReply>,
    local_approval: Option<LocalApprovalBinding>,
    armed_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceActionReplyBinding {
    catalog_revision: String,
    catalog_content_digest: String,
    resource_id: String,
    action_id: String,
    verb: ResourceActionVerb,
    target: ResourceActionTarget,
    expected_generation: u64,
    cancellation_id: String,
    cancels_request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DownstreamReplyKind {
    VdiAuthorityCompletion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceActionReply {
    schema_version: u16,
    request_id: String,
    accepted: bool,
    downstream_topic: Option<String>,
    downstream_reply_topic: Option<String>,
    downstream_reply_kind: Option<DownstreamReplyKind>,
    binding: Option<ResourceActionReplyBinding>,
    cancellation_completion: Option<serde_json::Value>,
    refusal: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VdiCompletionOutcome {
    DispatchAccepted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VdiAuthorityCompletionReply {
    schema_version: u16,
    request_id: String,
    session_id: String,
    serving_peer: String,
    outcome: VdiCompletionOutcome,
    completed_at_ms: u64,
    downstream_message_id: String,
    downstream_request_digest: String,
    authority_verb: String,
    authority_node: String,
    authority_target: String,
    binding: ResourceActionReplyBinding,
    authority_signature: String,
}

impl VdiAuthorityCompletionReply {
    fn signing_payload(&self) -> Result<String, String> {
        let mut unsigned = self.clone();
        unsigned.authority_signature.clear();
        serde_json::to_string(&unsigned)
            .map_err(|error| format!("encode RDP authority completion: {error}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum CatalogFilter {
    #[default]
    All,
    Desktops,
    Applications,
    MeshServices,
    Media,
    Communications,
    External,
}

impl CatalogFilter {
    const ALL: [Self; 7] = [
        Self::All,
        Self::Desktops,
        Self::Applications,
        Self::MeshServices,
        Self::Media,
        Self::Communications,
        Self::External,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Desktops => "Desktops",
            Self::Applications => "Apps",
            Self::MeshServices => "Mesh services",
            Self::Media => "Media",
            Self::Communications => "Communications",
            Self::External => "External providers",
        }
    }
}

#[derive(Debug, Clone)]
struct AdmittedResourceSnapshot {
    catalog: ResourceCatalog,
    discovery: ResourceDiscoveryProjection,
}

const MAX_SYSTEMD_CREDENTIAL_BYTES: usize = 4 * 1024;

/// Load the same host-bound HMAC authority used by mackesd for resource-action
/// completions. The root shell already receives this credential to mint its
/// short-lived action authorization, so verification introduces no new secret
/// distribution boundary. Missing authority material fails the RDP handoff.
fn vdi_authority_signer_from_systemd_credentials(
    directory: Option<&Path>,
) -> Result<CloudArmSigner, String> {
    if !rustix::process::geteuid().is_root() {
        return Err("RDP authority verification is available only in the root shell".to_owned());
    }
    let directory = directory
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "RDP authority credential directory is unavailable".to_owned())?;
    let raw = read_systemd_credential(&directory.join(CLOUD_ARM_CREDENTIAL))?;
    let key = decode_cloud_arm_credential(&raw)
        .map_err(|error| format!("decode RDP authority credential: {error}"))?;
    CloudArmSigner::new(key).map_err(str::to_owned)
}

/// Bounded, final-leaf-non-following read for a systemd credential. The caller
/// supplies only the fixed leaf above; no arbitrary environment value can
/// select a secret file.
fn read_systemd_credential(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read as _;

    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(0o400000); // O_NOFOLLOW
    }
    #[cfg(not(target_os = "linux"))]
    if !std::fs::symlink_metadata(path)
        .map_err(|_| "systemd credential is unavailable".to_owned())?
        .file_type()
        .is_file()
    {
        return Err("systemd credential is not a regular file".to_owned());
    }

    let file = options
        .open(path)
        .map_err(|_| "systemd credential is unavailable".to_owned())?;
    let metadata = file
        .metadata()
        .map_err(|_| "systemd credential cannot be inspected".to_owned())?;
    if !metadata.file_type().is_file() {
        return Err("systemd credential is not a regular file".to_owned());
    }
    if metadata.len() > MAX_SYSTEMD_CREDENTIAL_BYTES as u64 {
        return Err("systemd credential is oversized".to_owned());
    }

    let mut raw = Vec::with_capacity(
        MAX_SYSTEMD_CREDENTIAL_BYTES
            .min(usize::try_from(metadata.len()).unwrap_or(MAX_SYSTEMD_CREDENTIAL_BYTES)),
    );
    file.take((MAX_SYSTEMD_CREDENTIAL_BYTES + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|_| "systemd credential cannot be read".to_owned())?;
    if raw.len() > MAX_SYSTEMD_CREDENTIAL_BYTES {
        return Err("systemd credential is oversized".to_owned());
    }
    Ok(raw)
}

/// Live universal-catalog view state owned by the chooser.
pub(super) struct ResourceBrowserState {
    bus_root: Option<PathBuf>,
    catalog: Option<ResourceCatalog>,
    /// Separately published, safe browser projection bound to the admitted
    /// catalog. The card remains the action source; this projection owns
    /// discovery/filter facets so the shell never infers health or capability
    /// from display strings.
    discovery: Option<ResourceDiscoveryProjection>,
    filter: CatalogFilter,
    stack_expanded: bool,
    selected_resource: Option<String>,
    configuring_resource: Option<String>,
    configuration_draft: BTreeMap<String, String>,
    action_pending: Option<Receiver<Result<String, String>>>,
    vdi_pending: Option<Receiver<Result<AcceptedVdiOpen, String>>>,
    vdi_close_pending: Option<Receiver<Result<String, String>>>,
    vdi_approval: Option<VdiConnectBinding>,
    vdi_active: Option<AcceptedVdiOpen>,
    vdi_handoff: Option<ApprovedCatalogDesktop>,
    vdi_cancel_requested: bool,
    action_feedback: Option<String>,
    error: Option<String>,
}

impl ResourceBrowserState {
    pub(super) fn show_desktops(&self) -> bool {
        matches!(self.filter, CatalogFilter::All | CatalogFilter::Desktops)
    }

    pub(super) fn new(bus_root: Option<PathBuf>) -> Self {
        Self {
            bus_root,
            catalog: None,
            discovery: None,
            filter: CatalogFilter::All,
            stack_expanded: false,
            selected_resource: None,
            configuring_resource: None,
            configuration_draft: BTreeMap::new(),
            action_pending: None,
            vdi_pending: None,
            vdi_close_pending: None,
            vdi_approval: None,
            vdi_active: None,
            vdi_handoff: None,
            vdi_cancel_requested: false,
            action_feedback: None,
            error: None,
        }
    }

    pub(super) fn refresh(&mut self) {
        let Some(root) = self.bus_root.as_ref() else {
            return;
        };
        let result = Persist::open(root.clone())
            .map_err(|error| error.to_string())
            .and_then(|persist| {
                let catalog_body = persist
                    .read_latest(RESOURCE_CATALOG_TOPIC)
                    .map_err(|error| error.to_string())?
                    .and_then(|message| message.body)
                    .ok_or_else(|| "resource catalog has not published yet".to_owned())?;
                let discovery_body = persist
                    .read_latest(RESOURCE_DISCOVERY_TOPIC)
                    .map_err(|error| error.to_string())?
                    .and_then(|message| message.body)
                    .ok_or_else(|| {
                        "resource discovery projection has not published yet".to_owned()
                    })?;
                let catalog =
                    ResourceCatalog::from_json(&catalog_body).map_err(|error| error.to_string())?;
                let discovery: ResourceDiscoveryProjection = serde_json::from_str(&discovery_body)
                    .map_err(|error| format!("decode resource discovery projection: {error}"))?;
                let discovery = discovery
                    .admitted()
                    .map_err(|error| format!("admit resource discovery projection: {error}"))?;
                let expected = catalog
                    .discovery_projection()
                    .map_err(|error| format!("derive resource discovery projection: {error}"))?;
                if discovery != expected {
                    return Err(
                        "resource discovery projection does not match the retained catalog"
                            .to_owned(),
                    );
                }
                admit_resource_snapshot(&catalog_body, &discovery_body)
            });
        self.apply_refresh_result(result);
    }

    fn apply_refresh_result(&mut self, result: Result<AdmittedResourceSnapshot, String>) {
        match result {
            Ok(snapshot) => {
                self.catalog = Some(snapshot.catalog);
                self.discovery = Some(snapshot.discovery);
                self.error = None;
                if self.vdi_approval.as_ref().is_some_and(|approval| {
                    self.catalog.as_ref().is_none_or(|catalog| {
                        vdi_connect_binding(catalog, &approval.resource_id, current_unix_millis())
                            .is_none_or(|current| !same_vdi_binding(&current, approval))
                    })
                }) || self.vdi_active.as_ref().is_some_and(|active| {
                    self.catalog.as_ref().is_none_or(|catalog| {
                        vdi_connect_binding(
                            catalog,
                            &active.binding.resource_id,
                            current_unix_millis(),
                        )
                        .is_none_or(|current| !same_vdi_binding(&current, &active.binding))
                    })
                }) {
                    self.cancel_vdi_handoff();
                }
            }
            Err(error) => {
                // A retained card may remain useful for inspection, but a failed
                // newer read can represent an equivocated catalog/projection pair.
                // A mismatched catalog/projection pair cannot drive actions.
                self.cancel_vdi_handoff();
                self.error = Some(error);
            }
        }
    }

    /// Render catalog content. Returns `true` when a validated catalog exists.
    pub(super) fn show(&mut self, ui: &mut egui::Ui) -> bool {
        self.poll_service_action(ui.ctx());
        self.poll_vdi_action(ui.ctx());
        self.poll_vdi_close(ui.ctx());
        let Some(catalog) = self.catalog.clone() else {
            return false;
        };
        let Some(discovery) = self.discovery.clone() else {
            return false;
        };
        let cards: Vec<_> = catalog
            .cards
            .iter()
            .filter(|card| self.matches_filter(card))
            .cloned()
            .collect();

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("RESOURCE CATALOG")
                    .font(FontId::monospace(Style::SMALL))
                    .color(Style::TEXT_DIM),
            );
            ui.label(
                RichText::new(format!(
                    "REV {} · {} CARDS",
                    catalog.revision,
                    catalog.cards.len()
                ))
                .font(FontId::monospace(Style::SMALL))
                .color(Style::ACCENT),
            );
            ui.label(
                RichText::new(format!(
                    "DISCOVERY {} · TYPED HEALTH/AUTH/PROTOCOL FACETS",
                    discovery.entries.len()
                ))
                .font(FontId::monospace(Style::SMALL))
                .color(Style::ACCENT_COMMS),
            );
            ui.label(
                RichText::new("MESH CATALOG · CONTENT VALIDATED")
                    .font(Style::typography_font_with_size(
                        TypographyRole::Mono,
                        Style::TYPE_CAPTION,
                    ))
                    .color(Style::OK),
            );
        });
        ui.add_space(Style::SP_XS);
        self.stack_hero(ui, &cards);
        ui.add_space(Style::SP_S);
        ui.horizontal_wrapped(|ui| {
            for filter in CatalogFilter::ALL {
                if ui
                    .selectable_label(self.filter == filter, filter.label())
                    .clicked()
                {
                    self.filter = filter;
                }
            }
        });
        ui.add_space(Style::SP_S);

        if cards.is_empty() {
            ui.label(RichText::new("No resources match this filter.").color(Style::TEXT_DIM));
        } else {
            let columns: usize = if ui.available_width() >= 680.0 { 2 } else { 1 };
            let gutters = Style::SP_S * (columns.saturating_sub(1) as f32);
            let width = ((ui.available_width() - gutters) / columns as f32).max(240.0);
            for row in cards.chunks(columns) {
                ui.horizontal(|ui| {
                    for card in row {
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, 300.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                self.resource_card(
                                    ui,
                                    &catalog,
                                    card,
                                    discovery_entry(&discovery, card.resource_id()),
                                )
                            },
                        );
                    }
                });
                ui.add_space(Style::SP_S);
            }
        }
        self.selected_detail(ui, &catalog, &discovery);
        true
    }

    pub(super) fn take_vdi_handoff(&mut self) -> Option<ApprovedCatalogDesktop> {
        self.vdi_handoff.take()
    }

    pub(super) fn vdi_card_is_current(&self, resource_id: &str) -> bool {
        self.catalog.as_ref().is_some_and(|catalog| {
            vdi_connect_binding(catalog, resource_id, current_unix_millis()).is_some()
        })
    }

    pub(super) fn cancel_active_vdi(&mut self) {
        self.cancel_vdi_handoff();
    }

    fn matches_filter(&self, card: &ResourceCard) -> bool {
        match self.filter {
            CatalogFilter::All => true,
            CatalogFilter::Desktops => card.identity.class == ResourceClass::Desktop,
            CatalogFilter::Applications => card.identity.class == ResourceClass::Application,
            CatalogFilter::MeshServices => card
                .service
                .as_ref()
                .is_some_and(|service| !service.stack.external),
            CatalogFilter::Media => card
                .service
                .as_ref()
                .is_some_and(|service| service.category == ServiceCategory::Media),
            CatalogFilter::Communications => card
                .service
                .as_ref()
                .is_some_and(|service| service.category == ServiceCategory::Communications),
            CatalogFilter::External => card
                .service
                .as_ref()
                .is_some_and(|service| service.stack.external),
        }
    }

    fn stack_hero(&mut self, ui: &mut egui::Ui, cards: &[ResourceCard]) {
        let height = if self.stack_expanded { 250.0 } else { 132.0 };
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, Style::RADIUS_L, Style::SURFACE);
        painter.rect_stroke(
            rect,
            Style::RADIUS_L,
            Stroke::new(1.0, Style::ACCENT_COMMS),
            StrokeKind::Inside,
        );
        let title = if self.stack_expanded {
            "LOCAL SERVICE STACK / LIVE TOPOLOGY · SELECT TO NEST"
        } else {
            "LOCAL SERVICE STACK / LIVE TOPOLOGY · SELECT TO UNNEST"
        };
        painter.text(
            rect.left_top() + egui::vec2(Style::SP_M, Style::SP_S),
            egui::Align2::LEFT_TOP,
            title,
            Style::typography_font_with_size(TypographyRole::Mono, Style::TYPE_SUBHEADLINE),
            Style::ACCENT_COMMS,
        );

        let tiers = [
            (ServiceStackTier::DesktopShell, "01  DESKTOP SHELL"),
            (ServiceStackTier::PlatformServices, "02  PLATFORM SERVICES"),
            (ServiceStackTier::MeshSubstrate, "03  MESH SUBSTRATE"),
        ];
        let lane_top = rect.top() + 43.0;
        let lane_height = (height - 56.0) / 3.0;
        for (index, (tier, label)) in tiers.iter().enumerate() {
            let lane = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 16.0, lane_top + index as f32 * lane_height),
                egui::vec2(rect.width() - 2.0 * Style::SP_M, lane_height - Style::SP_XS),
            );
            painter.rect_stroke(
                lane,
                5.0,
                Stroke::new(0.75, Style::BORDER),
                StrokeKind::Inside,
            );
            painter.text(
                lane.left_center() + egui::vec2(Style::SP_S, 0.0),
                egui::Align2::LEFT_CENTER,
                *label,
                Style::typography_font(TypographyRole::Caption),
                Style::TEXT_DIM,
            );
            let mut x = lane.left() + 155.0;
            for card in cards.iter().filter(|card| {
                card.service
                    .as_ref()
                    .is_some_and(|service| service.stack.tier == *tier)
            }) {
                let service = card.service.as_ref().expect("filtered service");
                let node = egui::Rect::from_min_size(
                    egui::pos2(x, lane.top() + 7.0),
                    egui::vec2(92.0, lane.height() - 14.0),
                );
                painter.rect_filled(
                    node,
                    4.0,
                    lifecycle_color(service.lifecycle).gamma_multiply(0.16),
                );
                painter.rect_stroke(
                    node,
                    4.0,
                    Stroke::new(1.0, lifecycle_color(service.lifecycle)),
                    StrokeKind::Inside,
                );
                painter.text(
                    node.center(),
                    egui::Align2::CENTER_CENTER,
                    truncate(&card.display_name, 13),
                    Style::typography_font(TypographyRole::Caption),
                    Style::TEXT_STRONG,
                );
                x += 100.0;
                if x + 92.0 > lane.right() {
                    break;
                }
            }
        }
        if response.clicked() {
            self.stack_expanded = !self.stack_expanded;
        }
    }

    fn resource_card(
        &mut self,
        ui: &mut egui::Ui,
        catalog: &ResourceCatalog,
        card: &ResourceCard,
        discovery: Option<&ResourceDiscoveryEntry>,
    ) {
        let Some(service) = card.service.as_ref() else {
            self.plain_resource_card(ui, catalog, card, discovery);
            return;
        };
        let selected = self.selected_resource.as_deref() == Some(card.resource_id());
        let frame = egui::Frame::new()
            .fill(Style::SURFACE)
            .stroke(Stroke::new(
                if selected { 1.8 } else { 1.0 },
                lifecycle_color(service.lifecycle),
            ))
            .corner_radius(Style::RADIUS_M)
            .inner_margin(Style::SP_M);
        let response = frame
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(service.service_kind.to_ascii_uppercase())
                            .font(Style::typography_font(TypographyRole::Mono))
                            .color(Style::ACCENT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{:?}", service.lifecycle).to_ascii_uppercase())
                                .font(Style::typography_font(TypographyRole::Caption))
                                .color(lifecycle_color(service.lifecycle)),
                        );
                    });
                });
                ui.label(RichText::new(&card.display_name).strong().color(Style::TEXT_STRONG));
                if let Some(summary) = &card.summary {
                    ui.label(RichText::new(summary).small().color(Style::TEXT_DIM));
                }
                if let Some(discovery) = discovery {
                    discovery_rows(ui, discovery);
                }
                ui.add_space(Style::SP_XS);
                mono_row(ui, "TIER", &format!("{:?}", service.stack.tier));
                mono_row(ui, "PLANE", &format!("{:?}", service.stack.plane));
                mono_row(
                    ui,
                    "WORKER",
                    service.stack.adapter_worker.as_deref().unwrap_or("direct/native"),
                );
                mono_row(
                    ui,
                    "HOSTS",
                    &if service.stack.external {
                        "EXTERNAL · LOCAL ADAPTER".to_owned()
                    } else if service.stack.hosting_nodes.is_empty() {
                        "DISCOVERING".to_owned()
                    } else {
                        service.stack.hosting_nodes.join(", ")
                    },
                );
                ui.add_space(Style::SP_XS);
                ui.horizontal_wrapped(|ui| {
                    let action_now_ms = current_unix_millis();
                    for action in &card.actions {
                        let locally_handled = matches!(
                            action.verb,
                            ResourceActionVerb::Inspect
                                | ResourceActionVerb::Configure
                                | ResourceActionVerb::Test
                                | ResourceActionVerb::Enable
                                | ResourceActionVerb::Disable
                                | ResourceActionVerb::Remove
                        );
                        let button = ui.add_enabled(
                            service_action_is_admitted(action, action_now_ms)
                                && locally_handled
                                && self.action_pending.is_none(),
                            egui::Button::new(
                                RichText::new(format!("{:?}", action.verb).to_ascii_uppercase())
                                    .font(Style::typography_font(TypographyRole::Caption)),
                            ),
                        );
                        if button.clicked() {
                            self.selected_resource = Some(card.resource_id().to_owned());
                            if action.verb == ResourceActionVerb::Configure {
                                self.configuration_draft.clear();
                                self.configuring_resource = Some(card.resource_id().to_owned());
                            } else if !matches!(action.verb, ResourceActionVerb::Inspect) {
                                self.start_service_action(
                                    action.verb,
                                    &service.service_kind,
                                    None,
                                );
                            }
                        }
                        if !locally_handled {
                            let _ = mde_egui::disabled_hover_text(button,
                                "This action remains disabled until its typed daemon provider publishes readiness.",
                            );
                        }
                    }
                });
            })
            .response;
        if response.clicked() {
            self.selected_resource = Some(card.resource_id().to_owned());
        }
    }

    fn plain_resource_card(
        &mut self,
        ui: &mut egui::Ui,
        catalog: &ResourceCatalog,
        card: &ResourceCard,
        discovery: Option<&ResourceDiscoveryEntry>,
    ) {
        let selected = self.selected_resource.as_deref() == Some(card.resource_id());
        let color = match card.health.status {
            mackes_mesh_types::resources::HealthStatus::Available => Style::OK,
            mackes_mesh_types::resources::HealthStatus::Unavailable => Style::DANGER,
            _ => Style::TEXT_DIM,
        };
        let response = egui::Frame::new()
            .fill(Style::SURFACE)
            .stroke(Stroke::new(if selected { 1.8 } else { 1.0 }, color))
            .corner_radius(Style::RADIUS_M)
            .inner_margin(Style::SP_M)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!("{:?}", card.identity.class).to_ascii_uppercase())
                        .font(Style::typography_font(TypographyRole::Mono))
                        .color(Style::ACCENT),
                );
                ui.label(
                    RichText::new(&card.display_name)
                        .strong()
                        .color(Style::TEXT_STRONG),
                );
                if let Some(summary) = &card.summary {
                    ui.label(RichText::new(summary).small().color(Style::TEXT_DIM));
                }
                ui.add_space(Style::SP_XS);
                if let Some(discovery) = discovery {
                    discovery_rows(ui, discovery);
                }
                mono_row(ui, "IDENTITY", card.resource_id());
                mono_row(ui, "HEALTH", &format!("{:?}", card.health.status));
                mono_row(
                    ui,
                    "ROLES",
                    &card
                        .operating_roles
                        .iter()
                        .map(|role| format!("{role:?}"))
                        .collect::<Vec<_>>()
                        .join(" · "),
                );
                if ui.button("INSPECT").clicked() {
                    self.selected_resource = Some(card.resource_id().to_owned());
                }
                if let Some(binding) =
                    vdi_connect_binding(catalog, card.resource_id(), current_unix_millis())
                {
                    let approved = self
                        .vdi_approval
                        .as_ref()
                        .is_some_and(|approval| same_vdi_binding(approval, &binding));
                    let enabled = self.vdi_pending.is_none()
                        && self.vdi_close_pending.is_none()
                        && self.vdi_active.is_none();
                    let label = if approved {
                        "CONNECT RDP"
                    } else {
                        "APPROVE RDP"
                    };
                    let button = ui.add_enabled(enabled, egui::Button::new(label));
                    if button.clicked() {
                        if approved {
                            self.start_vdi_action(
                                self.vdi_approval.clone().expect("approved binding present"),
                            );
                        } else {
                            self.vdi_approval = Some(binding);
                            self.vdi_cancel_requested = false;
                            self.action_feedback = Some(
                                "RDP APPROVED LOCALLY · CONNECT AGAIN TO INVOKE AUTHORITY"
                                    .to_owned(),
                            );
                        }
                    }
                    if (approved || self.vdi_pending.is_some() || self.vdi_active.is_some())
                        && ui.button("CANCEL RDP HANDOFF").clicked()
                    {
                        self.cancel_vdi_handoff();
                    }
                }
            })
            .response;
        if response.clicked() {
            self.selected_resource = Some(card.resource_id().to_owned());
        }
    }

    fn selected_detail(
        &mut self,
        ui: &mut egui::Ui,
        catalog: &ResourceCatalog,
        discovery: &ResourceDiscoveryProjection,
    ) {
        let Some(selected_id) = self.selected_resource.clone() else {
            return;
        };
        let Some(card) = catalog
            .cards
            .iter()
            .find(|candidate| candidate.resource_id() == selected_id)
        else {
            self.selected_resource = None;
            self.configuring_resource = None;
            self.configuration_draft.clear();
            return;
        };
        let Some(service) = card.service.as_ref() else {
            ui.add_space(Style::SP_M);
            egui::Frame::new()
                .fill(Style::SURFACE)
                .stroke(Stroke::new(1.0, Style::BORDER))
                .corner_radius(14.0)
                .inner_margin(16.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{} / RESOURCE BLUEPRINT", card.display_name))
                                .font(FontId::monospace(13.0))
                                .color(Style::TEXT_STRONG),
                        );
                        if ui.button("Close").clicked() {
                            self.selected_resource = None;
                        }
                    });
                    mono_row(ui, "CLASS", &format!("{:?}", card.identity.class));
                    mono_row(ui, "IDENTITY", card.resource_id());
                    mono_row(ui, "HEALTH", &format!("{:?}", card.health.status));
                    mono_row(ui, "TRUST", "Nebula mesh catalog · content validated");
                    if let Some(entry) = discovery_entry(discovery, card.resource_id()) {
                        discovery_rows(ui, entry);
                    }
                    mono_row(
                        ui,
                        "ROLES",
                        &card
                            .operating_roles
                            .iter()
                            .map(|role| format!("{role:?}"))
                            .collect::<Vec<_>>()
                            .join(" · "),
                    );
                });
            return;
        };

        ui.add_space(Style::SP_M);
        egui::Frame::new()
            .fill(Style::SURFACE)
            .stroke(Stroke::new(1.0, lifecycle_color(service.lifecycle)))
            .corner_radius(14.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{} / LOCAL SERVICE STACK", card.display_name))
                            .font(FontId::monospace(13.0))
                            .color(Style::TEXT_STRONG),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            self.selected_resource = None;
                            self.configuring_resource = None;
                            self.configuration_draft.clear();
                        }
                    });
                });
                ui.add_space(Style::SP_XS);
                mono_row(ui, "TIER", &format!("{:?}", service.stack.tier));
                mono_row(ui, "PLANE", &format!("{:?}", service.stack.plane));
                mono_row(
                    ui,
                    "WORKER",
                    service.stack.adapter_worker.as_deref().unwrap_or("direct/native"),
                );

                if let Some(feedback) = &self.action_feedback {
                    ui.add_space(Style::SP_XS);
                    ui.label(
                        RichText::new(feedback)
                            .font(FontId::monospace(10.0))
                            .color(if feedback.contains("FAILED") {
                                Style::DANGER
                            } else {
                                Style::ACCENT
                            }),
                    );
                }
                mono_row(
                    ui,
                    "TRANSPORT",
                    service.stack.transport.as_deref().unwrap_or("not advertised"),
                );
                mono_row(
                    ui,
                    "CREDENTIAL",
                    service.stack.credential_ref.as_deref().unwrap_or("not required"),
                );
                mono_row(
                    ui,
                    "HOSTING",
                    &if service.stack.hosting_nodes.is_empty() {
                        "not placed".to_owned()
                    } else {
                        service.stack.hosting_nodes.join(", ")
                    },
                );
                mono_row(
                    ui,
                    "BUS",
                    &if service.stack.bus_topics.is_empty() {
                        "none".to_owned()
                    } else {
                        service.stack.bus_topics.join(" · ")
                    },
                );
                if let Some(entry) = discovery_entry(discovery, card.resource_id()) {
                    discovery_rows(ui, entry);
                }

                if !service.stack.dependencies.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("DEPENDS ")
                                .font(FontId::monospace(9.0))
                                .color(Style::TEXT_DIM),
                        );
                        for dependency in &service.stack.dependencies {
                            if ui.link(dependency).clicked() {
                                self.selected_resource = Some(dependency.clone());
                                self.configuring_resource = None;
                                self.configuration_draft.clear();
                            }
                        }
                    });
                }

                if self.configuring_resource.as_deref() == Some(selected_id.as_str()) {
                    ui.add_space(Style::SP_S);
                    ui.separator();
                    ui.add_space(Style::SP_S);
                    ui.label(
                        RichText::new("CONFIGURATION / SEALED VALUES")
                            .font(FontId::monospace(11.0))
                            .color(Style::ACCENT),
                    );
                    for field in &service.configuration_fields {
                        let key = format!("{selected_id}/{}", field.key);
                        let value = self.configuration_draft.entry(key).or_default();
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(&field.label)
                                    .font(FontId::monospace(10.0))
                                    .color(Style::TEXT_DIM),
                            );
                            let mut edit = egui::TextEdit::singleline(value)
                                .desired_width((ui.available_width() - 12.0).max(120.0));
                            if field.kind == ServiceConfigurationFieldKind::Secret {
                                edit = edit.password(true);
                            }
                            ui.add(edit);
                        });
                    }
                    ui.label(
                        RichText::new(
                            "Secret values are masked and are never read back from their credential reference.",
                        )
                        .small()
                        .color(Style::TEXT_DIM),
                    );
                    if ui
                        .add_enabled(
                            current_resource_action_count(
                                card,
                                ResourceActionVerb::Configure,
                                current_unix_millis(),
                            ) == 1
                                && self.action_pending.is_none(),
                            egui::Button::new("SAVE SEALED CONFIGURATION"),
                        )
                        .clicked()
                    {
                        let values = service
                            .configuration_fields
                            .iter()
                            .filter_map(|field| {
                                let key = format!("{selected_id}/{}", field.key);
                                self.configuration_draft
                                    .get(&key)
                                    .cloned()
                                    .map(|value| (field.key.clone(), value))
                            })
                            .collect::<BTreeMap<_, _>>();
                        let submission = serde_json::json!({
                            "service_kind": service.service_kind,
                            "values": values,
                        })
                        .to_string();
                        self.start_service_action(
                            ResourceActionVerb::Configure,
                            &service.service_kind,
                            Some(&submission),
                        );
                        // Secret inputs leave UI memory as soon as the bounded
                        // stdin payload has been handed to the worker thread.
                        self.configuration_draft.clear();
                        self.configuring_resource = None;
                    }
                }
            });
    }

    fn start_service_action(
        &mut self,
        verb: ResourceActionVerb,
        service_kind: &str,
        submission: Option<&str>,
    ) {
        if self.action_pending.is_some() {
            return;
        }
        let service_kind = service_kind.to_owned();
        let submission = submission.map(str::to_owned);
        let (sender, receiver) = mpsc::channel();
        self.action_feedback = Some("ACTION RUNNING · WAITING FOR ADAPTER".to_owned());
        self.action_pending = Some(receiver);
        thread::spawn(move || {
            let result = run_service_card_command(verb, &service_kind, submission.as_deref());
            let _ = sender.send(result);
        });
    }

    fn start_vdi_action(&mut self, binding: VdiConnectBinding) {
        if self.vdi_pending.is_some() {
            return;
        }
        let client_peer = crate::discovery::local_peer();
        let (sender, receiver) = mpsc::channel();
        self.vdi_cancel_requested = false;
        self.action_feedback = Some("RDP AUTHORITY INVOCATION PENDING".to_owned());
        self.vdi_pending = Some(receiver);
        thread::spawn(move || {
            let result = publish_vdi_connect(binding, &client_peer);
            let _ = sender.send(result);
        });
    }

    fn cancel_vdi_handoff(&mut self) {
        self.vdi_approval = None;
        self.vdi_handoff = None;
        if self.vdi_pending.is_some() {
            self.vdi_cancel_requested = true;
            self.action_feedback =
                Some("RDP CANCELLATION QUEUED · WAITING FOR OPEN RECEIPT".to_owned());
        } else if let Some(original) = self.vdi_active.clone() {
            self.vdi_cancel_requested = true;
            self.start_vdi_close(original);
        } else {
            self.vdi_cancel_requested = false;
            self.action_feedback = Some("RDP HANDOFF CANCELLED BEFORE DISPATCH".to_owned());
        }
    }

    fn poll_vdi_action(&mut self, ctx: &egui::Context) {
        let Some(receiver) = self.vdi_pending.as_ref() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(handoff)) => {
                self.vdi_pending = None;
                self.vdi_approval = None;
                self.vdi_active = Some(handoff.clone());
                if self.vdi_cancel_requested {
                    self.start_vdi_close(handoff);
                } else {
                    self.vdi_handoff = Some(handoff.handoff);
                    self.action_feedback =
                        Some("RDP AUTHORITY ACCEPTED · WINDOWS LOGIN REQUIRED".to_owned());
                }
                ctx.request_repaint();
            }
            Ok(Err(error)) => {
                self.action_feedback = Some(format!("RDP AUTHORITY REFUSED · {error}"));
                self.vdi_pending = None;
                self.vdi_approval = None;
                self.vdi_cancel_requested = false;
                ctx.request_repaint();
            }
            Err(TryRecvError::Disconnected) => {
                self.action_feedback = Some("RDP AUTHORITY INVOCATION EXITED".to_owned());
                self.vdi_pending = None;
                self.vdi_approval = None;
                self.vdi_cancel_requested = false;
                ctx.request_repaint();
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    fn start_vdi_close(&mut self, original: AcceptedVdiOpen) {
        if self.vdi_close_pending.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        self.action_feedback = Some("RDP EXACT CANCELLATION PENDING".to_owned());
        self.vdi_close_pending = Some(receiver);
        thread::spawn(move || {
            let result = publish_vdi_close(&original);
            let _ = sender.send(result);
        });
    }

    fn poll_vdi_close(&mut self, ctx: &egui::Context) {
        let Some(receiver) = self.vdi_close_pending.as_ref() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(request_id)) => {
                self.vdi_close_pending = None;
                self.vdi_active = None;
                self.vdi_handoff = None;
                self.vdi_cancel_requested = false;
                self.action_feedback = Some(format!("RDP CANCELLATION ACCEPTED · {request_id}"));
                ctx.request_repaint();
            }
            Ok(Err(error)) => {
                self.vdi_close_pending = None;
                self.action_feedback = Some(format!(
                    "RDP CANCELLATION FAILED · LIVE REQUEST RETAINED FOR RETRY · {error}"
                ));
                ctx.request_repaint();
            }
            Err(TryRecvError::Disconnected) => {
                self.vdi_close_pending = None;
                self.action_feedback =
                    Some("RDP CANCELLATION FAILED · LIVE REQUEST RETAINED FOR RETRY".to_owned());
                ctx.request_repaint();
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
    }

    fn poll_service_action(&mut self, ctx: &egui::Context) {
        let Some(receiver) = self.action_pending.as_ref() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Ok(detail)) => {
                self.action_feedback = Some(detail);
                self.action_pending = None;
                self.refresh();
                ctx.request_repaint();
            }
            Ok(Err(error)) => {
                self.action_feedback = Some(format!("ACTION FAILED · {error}"));
                self.action_pending = None;
                self.refresh();
                ctx.request_repaint();
            }
            Err(TryRecvError::Disconnected) => {
                self.action_feedback = Some("ACTION FAILED · ADAPTER EXITED".to_owned());
                self.action_pending = None;
                ctx.request_repaint();
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100))
            }
        }
    }
}

fn vdi_connect_binding(
    catalog: &ResourceCatalog,
    resource_id: &str,
    approved_at_ms: u64,
) -> Option<VdiConnectBinding> {
    let card = catalog
        .cards
        .iter()
        .find(|card| card.resource_id() == resource_id)?;
    if card.identity.class != ResourceClass::Desktop
        || card.health.status != HealthStatus::Available
        || card.auth.status != AuthStatus::Required
        || card.auth.accepted_methods != [AuthMethod::LocalApproval]
        || card.auth.active_method.is_some()
        || card.expires_at_ms <= approved_at_ms
    {
        return None;
    }
    let mut matches = card.actions.iter().filter_map(|action| {
        if action.verb != ResourceActionVerb::Connect
            || action.availability.status != ActionAvailabilityStatus::RequiresApproval
            || action.expires_at_ms <= approved_at_ms
        {
            return None;
        }
        let ResourceActionTarget::TransportClient {
            transport_fingerprint,
            capability_fingerprint,
        } = &action.target
        else {
            return None;
        };
        let transport = card.transports.iter().find(|transport| {
            &transport.fingerprint == transport_fingerprint
                && transport.protocol == TransportProtocol::Rdp
                && transport.client_capability_fingerprint.as_ref() == Some(capability_fingerprint)
                && transport.expires_at_ms > approved_at_ms
                && transport.health.status == HealthStatus::Available
        })?;
        let capability = card.client_capabilities.iter().find(|capability| {
            &capability.fingerprint == capability_fingerprint
                && capability.protocol == TransportProtocol::Rdp
                && capability.auth_methods.contains(&AuthMethod::LocalApproval)
                && capability
                    .safe_actions
                    .contains(&ResourceActionVerb::Connect)
        })?;
        let _ = capability;
        let TransportEndpoint::Network {
            host,
            port,
            base_path: None,
        } = &transport.endpoint
        else {
            return None;
        };
        Some(VdiConnectBinding {
            catalog_revision: catalog.revision.clone(),
            catalog_content_digest: catalog.computed_content_digest(),
            resource_id: card.resource_id().to_owned(),
            canonical_key: card.identity.canonical_key.clone(),
            display_name: card.display_name.clone(),
            action_id: action.action_id.clone(),
            target: action.target.clone(),
            host: host.clone(),
            port: *port,
            card_expires_at_ms: card.expires_at_ms,
            action_expires_at_ms: action.expires_at_ms,
            approved_at_ms,
        })
    });
    let binding = matches.next()?;
    matches.next().is_none().then_some(binding)
}

fn service_action_is_admitted(
    action: &mackes_mesh_types::resources::ResourceAction,
    now_ms: u64,
) -> bool {
    action.availability.status == ActionAvailabilityStatus::Ready
        && action.target == ResourceActionTarget::Resource
        && action.issued_at_ms <= now_ms
        && action.expires_at_ms > now_ms
}

fn current_resource_action_count(
    card: &ResourceCard,
    verb: ResourceActionVerb,
    now_ms: u64,
) -> usize {
    card.actions
        .iter()
        .filter(|action| action.verb == verb && service_action_is_admitted(action, now_ms))
        .count()
}

fn same_vdi_binding(left: &VdiConnectBinding, right: &VdiConnectBinding) -> bool {
    left.catalog_revision == right.catalog_revision
        && left.catalog_content_digest == right.catalog_content_digest
        && left.resource_id == right.resource_id
        && left.canonical_key == right.canonical_key
        && left.action_id == right.action_id
        && left.target == right.target
        && left.host == right.host
        && left.port == right.port
        && left.card_expires_at_ms == right.card_expires_at_ms
        && left.action_expires_at_ms == right.action_expires_at_ms
}

fn publish_vdi_connect(
    binding: VdiConnectBinding,
    client_peer: &str,
) -> Result<AcceptedVdiOpen, String> {
    let credentials_directory = std::env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from);
    let authority_verifier =
        vdi_authority_signer_from_systemd_credentials(credentials_directory.as_deref())?;
    let now_ms = current_unix_millis();
    let deadline_at_ms = now_ms
        .saturating_add(RESOURCE_ACTION_TTL_MS)
        .min(binding.card_expires_at_ms)
        .min(binding.action_expires_at_ms);
    if binding.approved_at_ms == 0 || binding.approved_at_ms > now_ms || deadline_at_ms <= now_ms {
        return Err("local approval is stale".to_owned());
    }
    let sequence = RESOURCE_ACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let request_id = format!("resource-rdp-connect-{now_ms}-{sequence}");
    let invocation = ResourceActionInvocation {
        schema_version: 1,
        request_id: request_id.clone(),
        catalog_revision: binding.catalog_revision.clone(),
        catalog_content_digest: binding.catalog_content_digest.clone(),
        resource_id: binding.resource_id.clone(),
        action_id: binding.action_id.clone(),
        verb: ResourceActionVerb::Connect,
        target: binding.target.clone(),
        expected_generation: 0,
        cancellation_id: format!("cancel-{request_id}"),
        cancels_request_id: None,
        issued_at_ms: now_ms,
        deadline_at_ms,
        authority_request: TypedAuthorityRequest::Vdi(StrictSessionRequest::Open {
            id: request_id.clone(),
            serving_peer: binding.host.clone(),
            vm_id: binding.canonical_key.clone(),
            client_peer: client_peer.to_owned(),
            profile: None,
        }),
        vdi_open_receipt: None,
        local_approval: Some(LocalApprovalBinding {
            catalog_revision: binding.catalog_revision.clone(),
            catalog_content_digest: binding.catalog_content_digest.clone(),
            resource_id: binding.resource_id.clone(),
            action_id: binding.action_id.clone(),
            target: binding.target.clone(),
            approved_at_ms: binding.approved_at_ms,
            expires_at_ms: binding.card_expires_at_ms.min(binding.action_expires_at_ms),
        }),
        armed_token: None,
    };
    let unsigned = serde_json::to_string(&invocation)
        .map_err(|error| format!("encode RDP resource action: {error}"))?;
    let target = format!("{}:{}", invocation.resource_id, invocation.action_id);
    let body = crate::iac::authorize_root_mutation_body(
        &unsigned,
        "resource-action-connect",
        "resource-authority",
        &target,
    )?;
    let root = mde_bus::client_data_dir()
        .ok_or_else(|| "the local mesh Bus directory is unavailable".to_owned())?;
    let persist = Persist::open(root).map_err(|error| format!("open local mesh Bus: {error}"))?;
    let ingress_id = mde_bus::rpc::publish_request(
        &persist,
        RESOURCE_ACTION_TOPIC,
        mde_bus::hooks::config::Priority::Default,
        None,
        Some(&body),
    )
    .map_err(|error| format!("publish RDP resource action: {error}"))?;
    let reply_topic = mde_bus::rpc::reply_topic(&ingress_id);
    loop {
        let now_ms = current_unix_millis();
        if now_ms > invocation.deadline_at_ms {
            return Err("resource authority reply timed out".to_owned());
        }
        let replies = persist
            .list_since_limit(&reply_topic, None, 2)
            .map_err(|error| format!("read RDP resource reply: {error}"))?;
        if replies.len() > 1 {
            return Err("resource authority reply lane contains a replay".to_owned());
        }
        if let Some(message) = replies.first() {
            let reply: ResourceActionReply = serde_json::from_str(
                message
                    .body
                    .as_deref()
                    .ok_or_else(|| "resource authority reply has no body".to_owned())?,
            )
            .map_err(|error| format!("decode RDP resource reply: {error}"))?;
            validate_vdi_reply(&invocation, &reply)?;
            let completion_topic = reply
                .downstream_reply_topic
                .as_deref()
                .ok_or_else(|| "RDP authority omitted its completion topic".to_owned())?;
            let receipt = read_vdi_open_completion(
                &persist,
                completion_topic,
                &invocation,
                &authority_verifier,
                invocation.deadline_at_ms,
            )?;
            return Ok(AcceptedVdiOpen {
                invocation,
                receipt,
                binding: binding.clone(),
                handoff: ApprovedCatalogDesktop {
                    resource_id: binding.resource_id,
                    display_name: binding.display_name,
                    host: binding.host,
                    port: binding.port,
                },
            });
        }
        thread::sleep(Duration::from_millis(ACTION_REPLY_POLL_MS));
    }
}

fn publish_vdi_close(original: &AcceptedVdiOpen) -> Result<String, String> {
    let invocation = vdi_close_invocation(original, current_unix_millis())?;
    publish_vdi_invocation(invocation).map(|accepted| accepted.request_id)
}

fn read_vdi_open_completion(
    persist: &Persist,
    topic: &str,
    invocation: &ResourceActionInvocation,
    authority_verifier: &CloudArmSigner,
    deadline_at_ms: u64,
) -> Result<VdiAuthorityCompletionReply, String> {
    let message_id = topic
        .strip_prefix("reply/")
        .filter(|id| !id.is_empty() && !id.contains(['/', '\\']))
        .ok_or_else(|| "RDP completion topic is malformed".to_owned())?;
    loop {
        let now_ms = current_unix_millis();
        if now_ms > deadline_at_ms {
            return Err("RDP authority completion timed out".to_owned());
        }
        let replies = persist
            .list_since_limit(topic, None, 2)
            .map_err(|error| format!("read signed RDP completion: {error}"))?;
        if replies.len() > 1 {
            return Err("RDP completion lane contains a replay".to_owned());
        }
        if let Some(message) = replies.first() {
            let receipt: VdiAuthorityCompletionReply = serde_json::from_str(
                message
                    .body
                    .as_deref()
                    .ok_or_else(|| "RDP completion has no body".to_owned())?,
            )
            .map_err(|error| format!("decode signed RDP completion: {error}"))?;
            validate_vdi_open_completion(invocation, &receipt, message_id, authority_verifier)?;
            return Ok(receipt);
        }
        thread::sleep(Duration::from_millis(ACTION_REPLY_POLL_MS));
    }
}

fn validate_vdi_open_completion(
    invocation: &ResourceActionInvocation,
    receipt: &VdiAuthorityCompletionReply,
    downstream_message_id: &str,
    authority_verifier: &CloudArmSigner,
) -> Result<(), String> {
    let TypedAuthorityRequest::Vdi(StrictSessionRequest::Open { serving_peer, .. }) =
        &invocation.authority_request
    else {
        return Err("RDP completion is not bound to an Open".to_owned());
    };
    let expected = ResourceActionReplyBinding {
        catalog_revision: invocation.catalog_revision.clone(),
        catalog_content_digest: invocation.catalog_content_digest.clone(),
        resource_id: invocation.resource_id.clone(),
        action_id: invocation.action_id.clone(),
        verb: invocation.verb,
        target: invocation.target.clone(),
        expected_generation: invocation.expected_generation,
        cancellation_id: invocation.cancellation_id.clone(),
        cancels_request_id: None,
    };
    if receipt.schema_version != 1
        || receipt.request_id != invocation.request_id
        || receipt.session_id != invocation.request_id
        || receipt.serving_peer != *serving_peer
        || receipt.outcome != VdiCompletionOutcome::DispatchAccepted
        || receipt.completed_at_ms < invocation.issued_at_ms
        || receipt.completed_at_ms > invocation.deadline_at_ms
        || receipt.downstream_message_id != downstream_message_id
        || receipt.downstream_request_digest.len() != 64
        || !receipt
            .downstream_request_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || receipt.authority_verb != "vdi-session-open"
        || receipt.authority_node != "vdi-session"
        || receipt.authority_target != format!("session:{}", invocation.request_id)
        || receipt.binding != expected
        || !authority_verifier
            .verify_payload(&receipt.signing_payload()?, &receipt.authority_signature)
    {
        return Err(
            "RDP completion signature does not preserve the exact Open identity".to_owned(),
        );
    }
    Ok(())
}

fn vdi_close_invocation(
    original: &AcceptedVdiOpen,
    now_ms: u64,
) -> Result<ResourceActionInvocation, String> {
    let open = &original.invocation;
    if open.cancels_request_id.is_some()
        || !matches!(
            &open.authority_request,
            TypedAuthorityRequest::Vdi(StrictSessionRequest::Open { id, .. })
                if id == &open.request_id
        )
        || open.local_approval.is_none()
        || !same_vdi_binding(
            &original.binding,
            &VdiConnectBinding {
                catalog_revision: open.catalog_revision.clone(),
                catalog_content_digest: open.catalog_content_digest.clone(),
                resource_id: open.resource_id.clone(),
                canonical_key: original.binding.canonical_key.clone(),
                display_name: original.binding.display_name.clone(),
                action_id: open.action_id.clone(),
                target: open.target.clone(),
                host: original.binding.host.clone(),
                port: original.binding.port,
                card_expires_at_ms: original.binding.card_expires_at_ms,
                action_expires_at_ms: original.binding.action_expires_at_ms,
                approved_at_ms: original.binding.approved_at_ms,
            },
        )
    {
        return Err("accepted RDP Open identity is inconsistent".to_owned());
    }
    let deadline_at_ms = now_ms.saturating_add(RESOURCE_ACTION_TTL_MS);
    if now_ms == 0 || deadline_at_ms <= now_ms {
        return Err("RDP cancellation deadline is unavailable".to_owned());
    }
    let sequence = RESOURCE_ACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let request_id = format!("resource-rdp-close-{now_ms}-{sequence}");
    Ok(ResourceActionInvocation {
        schema_version: 1,
        request_id: request_id.clone(),
        catalog_revision: open.catalog_revision.clone(),
        catalog_content_digest: open.catalog_content_digest.clone(),
        resource_id: open.resource_id.clone(),
        action_id: open.action_id.clone(),
        verb: open.verb,
        target: open.target.clone(),
        expected_generation: open.expected_generation,
        cancellation_id: format!("cancel-{request_id}"),
        cancels_request_id: Some(open.request_id.clone()),
        issued_at_ms: now_ms,
        deadline_at_ms,
        authority_request: TypedAuthorityRequest::Vdi(StrictSessionRequest::Close {
            id: open.request_id.clone(),
        }),
        vdi_open_receipt: Some(original.receipt.clone()),
        local_approval: None,
        armed_token: None,
    })
}

fn publish_vdi_invocation(
    invocation: ResourceActionInvocation,
) -> Result<ResourceActionInvocation, String> {
    let unsigned = serde_json::to_string(&invocation)
        .map_err(|error| format!("encode RDP resource action: {error}"))?;
    let target = format!("{}:{}", invocation.resource_id, invocation.action_id);
    let auth_verb = if invocation.cancels_request_id.is_some() {
        "resource-action-cancel"
    } else {
        "resource-action-connect"
    };
    let body = crate::iac::authorize_root_mutation_body(
        &unsigned,
        auth_verb,
        "resource-authority",
        &target,
    )?;
    let root = mde_bus::client_data_dir()
        .ok_or_else(|| "the local mesh Bus directory is unavailable".to_owned())?;
    let persist = Persist::open(root).map_err(|error| format!("open local mesh Bus: {error}"))?;
    let ingress_id = mde_bus::rpc::publish_request(
        &persist,
        RESOURCE_ACTION_TOPIC,
        mde_bus::hooks::config::Priority::Default,
        None,
        Some(&body),
    )
    .map_err(|error| format!("publish RDP resource action: {error}"))?;
    let reply_topic = mde_bus::rpc::reply_topic(&ingress_id);
    loop {
        let now_ms = current_unix_millis();
        if now_ms > invocation.deadline_at_ms {
            return Err("resource authority reply timed out".to_owned());
        }
        let replies = persist
            .list_since_limit(&reply_topic, None, 2)
            .map_err(|error| format!("read RDP resource reply: {error}"))?;
        if replies.len() > 1 {
            return Err("resource authority reply lane contains a replay".to_owned());
        }
        if let Some(message) = replies.first() {
            let reply: ResourceActionReply = serde_json::from_str(
                message
                    .body
                    .as_deref()
                    .ok_or_else(|| "resource authority reply has no body".to_owned())?,
            )
            .map_err(|error| format!("decode RDP resource reply: {error}"))?;
            validate_vdi_reply(&invocation, &reply)?;
            return Ok(invocation);
        }
        thread::sleep(Duration::from_millis(ACTION_REPLY_POLL_MS));
    }
}

fn validate_vdi_reply(
    invocation: &ResourceActionInvocation,
    reply: &ResourceActionReply,
) -> Result<(), String> {
    let expected = ResourceActionReplyBinding {
        catalog_revision: invocation.catalog_revision.clone(),
        catalog_content_digest: invocation.catalog_content_digest.clone(),
        resource_id: invocation.resource_id.clone(),
        action_id: invocation.action_id.clone(),
        verb: invocation.verb,
        target: invocation.target.clone(),
        expected_generation: invocation.expected_generation,
        cancellation_id: invocation.cancellation_id.clone(),
        cancels_request_id: invocation.cancels_request_id.clone(),
    };
    let valid_reply_topic = reply
        .downstream_reply_topic
        .as_deref()
        .is_some_and(|topic| {
            topic.strip_prefix("reply/").is_some_and(|id| {
                !id.is_empty()
                    && id.len() <= 255
                    && id.is_ascii()
                    && !id.contains(['/', '\\'])
                    && !id.contains("..")
            })
        });
    if reply.schema_version != 1
        || reply.request_id != invocation.request_id
        || !reply.accepted
        || reply.refusal.is_some()
        || reply.cancellation_completion.is_some()
        || reply.binding.as_ref() != Some(&expected)
        || reply.downstream_topic.as_deref() != Some(VDI_SESSION_ACTION_TOPIC)
        || reply.downstream_reply_kind != Some(DownstreamReplyKind::VdiAuthorityCompletion)
        || !valid_reply_topic
    {
        return Err("resource authority receipt did not preserve the exact RDP binding".to_owned());
    }
    Ok(())
}

/// Admit a retained catalog/projection pair after structural and digest checks.
fn admit_resource_snapshot(
    catalog_body: &str,
    discovery_body: &str,
) -> Result<AdmittedResourceSnapshot, String> {
    let catalog = ResourceCatalog::from_json(catalog_body).map_err(|error| error.to_string())?;
    let discovery: ResourceDiscoveryProjection = serde_json::from_str(discovery_body)
        .map_err(|error| format!("decode resource discovery projection: {error}"))?;
    let discovery = discovery
        .admitted()
        .map_err(|error| format!("admit resource discovery projection: {error}"))?;
    let expected = catalog
        .discovery_projection()
        .map_err(|error| format!("derive resource discovery projection: {error}"))?;
    if discovery != expected {
        return Err("resource discovery projection does not match the retained catalog".to_owned());
    }

    Ok(AdmittedResourceSnapshot { catalog, discovery })
}

fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(1)
        .max(1)
}

/// Dest identity and join-token env must not leak into service-card children.
/// Login leftover (2): only the dest-env runner sources those vars.
const LIFECYCLE_CHILD_ENV_STRIP: &[&str] = &[
    "MACKESD_BOOTSTRAP_SSH_KEY",
    "MACKESD_BOOTSTRAP_KNOWN_HOSTS",
    "JOIN_TOKEN",
];

fn strip_lifecycle_child_env(command: &mut Command) {
    for name in LIFECYCLE_CHILD_ENV_STRIP {
        command.env_remove(*name);
    }
}

fn run_service_card_command(
    verb: ResourceActionVerb,
    service_kind: &str,
    submission: Option<&str>,
) -> Result<String, String> {
    let mut command = Command::new("/usr/bin/mackesd");
    strip_lifecycle_child_env(&mut command);
    command.arg("service-card");
    match verb {
        ResourceActionVerb::Configure => {
            command.arg("save");
        }
        ResourceActionVerb::Test => {
            command.args(["test", service_kind]);
        }
        ResourceActionVerb::Enable => {
            command.args(["enable", service_kind]);
        }
        ResourceActionVerb::Disable => {
            command.args(["disable", service_kind]);
        }
        ResourceActionVerb::Remove => {
            command.args(["remove", service_kind]);
        }
        _ => return Err("service action has no privileged adapter".into()),
    }
    command
        .stdin(if submission.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("start service-card responder: {error}"))?;
    if let Some(body) = submission {
        child
            .stdin
            .take()
            .ok_or_else(|| "service-card responder stdin was unavailable".to_owned())?
            .write_all(body.as_bytes())
            .map_err(|error| format!("write bounded service configuration: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("wait for service-card responder: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let body: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "service-card responder returned malformed status".to_owned())?;
    let detail = body
        .get("detail")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("service action completed");
    Ok(detail.to_ascii_uppercase())
}

fn mono_row(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{key:<7}"))
                .font(FontId::monospace(9.0))
                .color(Style::TEXT_DIM),
        );
        ui.label(
            RichText::new(value)
                .font(FontId::monospace(9.0))
                .color(Style::TEXT_STRONG),
        );
    });
}

fn discovery_entry<'a>(
    projection: &'a ResourceDiscoveryProjection,
    resource_id: &str,
) -> Option<&'a ResourceDiscoveryEntry> {
    projection
        .entries
        .iter()
        .find(|entry| entry.resource_id == resource_id)
}

/// Render only the bounded, non-secret discovery projection. In particular,
/// endpoints and credential references stay on the typed card/action path.
fn discovery_rows(ui: &mut egui::Ui, entry: &ResourceDiscoveryEntry) {
    mono_row(ui, "DISCOVER", &format!("{:?}", entry.class));
    mono_row(ui, "HEALTH", &format!("{:?}", entry.health_status));
    mono_row(ui, "AUTH", &format!("{:?}", entry.auth_status));
    mono_row(
        ui,
        "SOURCES",
        &entry
            .discovery_sources
            .iter()
            .map(|source| format!("{source:?}"))
            .collect::<Vec<_>>()
            .join(" · "),
    );
    mono_row(
        ui,
        "PROTOCOLS",
        &entry
            .transport_protocols
            .iter()
            .map(|protocol| format!("{protocol:?}"))
            .collect::<Vec<_>>()
            .join(" · "),
    );
    mono_row(
        ui,
        "READY",
        &entry
            .ready_actions
            .iter()
            .map(|action| format!("{action:?}"))
            .collect::<Vec<_>>()
            .join(" · "),
    );
}

fn lifecycle_color(status: ServiceLifecycleStatus) -> Color32 {
    match status {
        ServiceLifecycleStatus::Unconfigured => Style::ACCENT_COMMS,
        ServiceLifecycleStatus::Connecting => Style::ACCENT,
        ServiceLifecycleStatus::Healthy => Style::OK,
        ServiceLifecycleStatus::Degraded => Style::WARN,
        ServiceLifecycleStatus::Offline => Style::DANGER,
        ServiceLifecycleStatus::Disabled => Style::TEXT_DIM,
    }
}

fn truncate(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_card_strips_bootstrap_dest_env() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "printf %s \"$MACKESD_BOOTSTRAP_SSH_KEY$MACKESD_BOOTSTRAP_KNOWN_HOSTS$JOIN_TOKEN\"",
        ]);
        command.env("MACKESD_BOOTSTRAP_SSH_KEY", "/tmp/must-not-leak");
        command.env("MACKESD_BOOTSTRAP_KNOWN_HOSTS", "/tmp/must-not-leak-hosts");
        command.env("JOIN_TOKEN", "must-not-leak-token");
        strip_lifecycle_child_env(&mut command);
        let output = command.output().expect("run stripped child");
        assert!(output.status.success());
        assert!(
            output.stdout.is_empty(),
            "service-card child inherited dest env: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    const NOW: u64 = 1_700_000_000_000;

    fn catalog() -> ResourceCatalog {
        ResourceCatalog {
            schema_version: mackes_mesh_types::resources::RESOURCE_CONTRACT_VERSION,
            revision: "resource-rev".to_owned(),
            publisher: "seat-15".to_owned(),
            generated_at_ms: NOW,
            content_digest: None,
            cards: Vec::new(),
        }
    }

    fn snapshot_bodies() -> (String, String) {
        let catalog = catalog();
        let discovery = catalog
            .discovery_projection()
            .expect("empty discovery projection");
        (
            serde_json::to_string(&catalog).expect("catalog JSON"),
            serde_json::to_string(&discovery).expect("discovery JSON"),
        )
    }

    #[test]
    fn expired_ready_service_action_is_not_admitted() {
        let action = mackes_mesh_types::resources::ResourceAction {
            schema_version: 1,
            action_id: "configure-service".into(),
            verb: ResourceActionVerb::Configure,
            target: ResourceActionTarget::Resource,
            availability: mackes_mesh_types::resources::ActionAvailability {
                status: ActionAvailabilityStatus::Ready,
                failure: None,
            },
            issued_at_ms: NOW - 1_000,
            expires_at_ms: NOW,
        };

        assert!(!service_action_is_admitted(&action, NOW));
        assert!(service_action_is_admitted(&action, NOW - 1));
    }

    #[test]
    fn consumer_admits_matching_catalog_and_discovery_content() {
        let (catalog_body, discovery_body) = snapshot_bodies();
        let admitted =
            admit_resource_snapshot(&catalog_body, &discovery_body).expect("validated snapshot");
        assert_eq!(admitted.catalog, catalog());
    }

    #[test]
    fn rdp_handoff_requires_an_exact_accepted_authority_receipt() {
        let target = ResourceActionTarget::TransportClient {
            transport_fingerprint:
                "transport:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            capability_fingerprint:
                "capability:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        };
        let invocation = ResourceActionInvocation {
            schema_version: 1,
            request_id: "resource-rdp-connect-1".into(),
            catalog_revision: "revision-1".into(),
            catalog_content_digest: "digest-1".into(),
            resource_id:
                "resource:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".into(),
            action_id: "connect-rdp-0".into(),
            verb: ResourceActionVerb::Connect,
            target: target.clone(),
            expected_generation: 0,
            cancellation_id: "cancel-resource-rdp-connect-1".into(),
            cancels_request_id: None,
            issued_at_ms: NOW,
            deadline_at_ms: NOW + 20_000,
            authority_request: TypedAuthorityRequest::Vdi(StrictSessionRequest::Open {
                id: "resource-rdp-connect-1".into(),
                serving_peer: "172.20.146.54".into(),
                vm_id: "mdns:172.20.146.54:3389:rdp".into(),
                client_peer: "seat-15".into(),
                profile: None,
            }),
            vdi_open_receipt: None,
            local_approval: None,
            armed_token: None,
        };
        let binding = ResourceActionReplyBinding {
            catalog_revision: invocation.catalog_revision.clone(),
            catalog_content_digest: invocation.catalog_content_digest.clone(),
            resource_id: invocation.resource_id.clone(),
            action_id: invocation.action_id.clone(),
            verb: invocation.verb,
            target,
            expected_generation: 0,
            cancellation_id: invocation.cancellation_id.clone(),
            cancels_request_id: None,
        };
        let reply = ResourceActionReply {
            schema_version: 1,
            request_id: invocation.request_id.clone(),
            accepted: true,
            downstream_topic: Some(VDI_SESSION_ACTION_TOPIC.into()),
            downstream_reply_topic: Some("reply/01AUTHORITYRECEIPT".into()),
            downstream_reply_kind: Some(DownstreamReplyKind::VdiAuthorityCompletion),
            binding: Some(binding),
            cancellation_completion: None,
            refusal: None,
        };
        validate_vdi_reply(&invocation, &reply).expect("exact accepted receipt");

        let mut substituted = reply.clone();
        substituted.binding.as_mut().expect("binding").action_id = "other-action".into();
        assert!(validate_vdi_reply(&invocation, &substituted).is_err());

        let mut refused = reply;
        refused.accepted = false;
        assert!(validate_vdi_reply(&invocation, &refused).is_err());

        let approved_binding = VdiConnectBinding {
            catalog_revision: invocation.catalog_revision.clone(),
            catalog_content_digest: invocation.catalog_content_digest.clone(),
            resource_id: invocation.resource_id.clone(),
            canonical_key: "mdns:172.20.146.54:3389:rdp".into(),
            display_name: "Windows 10".into(),
            action_id: invocation.action_id.clone(),
            target: invocation.target.clone(),
            host: "172.20.146.54".into(),
            port: 3_389,
            card_expires_at_ms: NOW + 60_000,
            action_expires_at_ms: NOW + 60_000,
            approved_at_ms: NOW,
        };
        let authority_signer = CloudArmSigner::new(b"chooser-authority-test-key".to_vec())
            .expect("test authority signer");
        let mut open_receipt = VdiAuthorityCompletionReply {
            schema_version: 1,
            request_id: invocation.request_id.clone(),
            session_id: invocation.request_id.clone(),
            serving_peer: "172.20.146.54".into(),
            outcome: VdiCompletionOutcome::DispatchAccepted,
            completed_at_ms: NOW,
            downstream_message_id: "01DOWNSTREAM".into(),
            downstream_request_digest: "a".repeat(64),
            authority_verb: "vdi-session-open".into(),
            authority_node: "vdi-session".into(),
            authority_target: format!("session:{}", invocation.request_id),
            binding: ResourceActionReplyBinding {
                catalog_revision: invocation.catalog_revision.clone(),
                catalog_content_digest: invocation.catalog_content_digest.clone(),
                resource_id: invocation.resource_id.clone(),
                action_id: invocation.action_id.clone(),
                verb: invocation.verb,
                target: invocation.target.clone(),
                expected_generation: invocation.expected_generation,
                cancellation_id: invocation.cancellation_id.clone(),
                cancels_request_id: None,
            },
            authority_signature: String::new(),
        };
        open_receipt.authority_signature =
            authority_signer.sign_payload(&open_receipt.signing_payload().expect("payload"));
        validate_vdi_open_completion(
            &invocation,
            &open_receipt,
            "01DOWNSTREAM",
            &authority_signer,
        )
        .expect("cryptographically verified Open completion");
        let mut forged_open_receipt = open_receipt.clone();
        forged_open_receipt.authority_signature = "nonempty-forgery".into();
        assert!(validate_vdi_open_completion(
            &invocation,
            &forged_open_receipt,
            "01DOWNSTREAM",
            &authority_signer,
        )
        .is_err());
        let accepted = AcceptedVdiOpen {
            invocation: ResourceActionInvocation {
                local_approval: Some(LocalApprovalBinding {
                    catalog_revision: invocation.catalog_revision.clone(),
                    catalog_content_digest: invocation.catalog_content_digest.clone(),
                    resource_id: invocation.resource_id.clone(),
                    action_id: invocation.action_id.clone(),
                    target: invocation.target.clone(),
                    approved_at_ms: NOW,
                    expires_at_ms: NOW + 60_000,
                }),
                ..invocation.clone()
            },
            receipt: open_receipt,
            binding: approved_binding,
            handoff: ApprovedCatalogDesktop {
                resource_id: invocation.resource_id.clone(),
                display_name: "Windows 10".into(),
                host: "172.20.146.54".into(),
                port: 3_389,
            },
        };
        let close = vdi_close_invocation(&accepted, NOW + 1).expect("exact VDI Close");
        assert_ne!(close.request_id, accepted.invocation.request_id);
        assert_eq!(
            close.cancels_request_id.as_deref(),
            Some(accepted.invocation.request_id.as_str())
        );
        assert!(matches!(
            close.authority_request,
            TypedAuthorityRequest::Vdi(StrictSessionRequest::Close { ref id })
                if id == &accepted.invocation.request_id
        ));
        assert_eq!(close.catalog_revision, accepted.invocation.catalog_revision);
        assert_eq!(close.resource_id, accepted.invocation.resource_id);
        assert_eq!(close.action_id, accepted.invocation.action_id);
        assert_eq!(close.target, accepted.invocation.target);
        assert!(close.local_approval.is_none());
        assert_eq!(close.vdi_open_receipt, Some(accepted.receipt.clone()));
        let close_reply = ResourceActionReply {
            schema_version: 1,
            request_id: close.request_id.clone(),
            accepted: true,
            downstream_topic: Some(VDI_SESSION_ACTION_TOPIC.into()),
            downstream_reply_topic: Some("reply/01CLOSERECEIPT".into()),
            downstream_reply_kind: Some(DownstreamReplyKind::VdiAuthorityCompletion),
            binding: Some(ResourceActionReplyBinding {
                catalog_revision: close.catalog_revision.clone(),
                catalog_content_digest: close.catalog_content_digest.clone(),
                resource_id: close.resource_id.clone(),
                action_id: close.action_id.clone(),
                verb: close.verb,
                target: close.target.clone(),
                expected_generation: close.expected_generation,
                cancellation_id: close.cancellation_id.clone(),
                cancels_request_id: close.cancels_request_id.clone(),
            }),
            cancellation_completion: None,
            refusal: None,
        };
        validate_vdi_reply(&close, &close_reply).expect("exact Close receipt");

        let mut substituted = accepted.clone();
        substituted.binding.action_id = "other-action".into();
        assert!(vdi_close_invocation(&substituted, NOW + 1).is_err());

        let mut state = ResourceBrowserState::new(None);
        let (_sender, receiver) = mpsc::channel();
        state.vdi_pending = Some(receiver);
        state.vdi_handoff = Some(ApprovedCatalogDesktop {
            resource_id: invocation.resource_id,
            display_name: "Windows 10".into(),
            host: "172.20.146.54".into(),
            port: 3_389,
        });
        state.cancel_vdi_handoff();
        assert!(state.take_vdi_handoff().is_none());
        assert!(state.vdi_cancel_requested);
        assert!(state
            .action_feedback
            .as_deref()
            .is_some_and(|feedback| feedback.contains("CANCELLATION QUEUED")));

        let live_now = current_unix_millis();
        let mut revoked = accepted;
        revoked.binding.card_expires_at_ms = live_now + 60_000;
        revoked.binding.action_expires_at_ms = live_now + 60_000;
        revoked
            .invocation
            .local_approval
            .as_mut()
            .expect("accepted approval")
            .expires_at_ms = live_now + 60_000;
        let mut revoked_state = ResourceBrowserState::new(None);
        revoked_state.vdi_active = Some(revoked);
        revoked_state.vdi_handoff = Some(ApprovedCatalogDesktop {
            resource_id: "resource:revoked".into(),
            display_name: "Windows 10".into(),
            host: "172.20.146.54".into(),
            port: 3_389,
        });
        revoked_state.cancel_active_vdi();
        assert!(revoked_state.vdi_handoff.is_none());
        assert!(revoked_state.vdi_active.is_some());
        assert!(
            revoked_state.vdi_close_pending.is_some(),
            "post-accepted revocation must dispatch exact Close, not only hide the handoff"
        );
    }
}
