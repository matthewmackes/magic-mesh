//! Pure in-memory Remote Sessions browser for the universal resource catalog.
//!
//! Snapshot acquisition and admission belong to the shell/controller boundary.
//! This module only projects an already bounded snapshot and paints it; no
//! render path opens Bus, a socket, a file, or a backend connection.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use mackes_mesh_types::android_apps::AospStarterApp;
use mackes_mesh_types::resources::{
    ActionAvailabilityStatus, AuthStatus, DiscoverySource, HealthStatus, ResourceActionTarget,
    ResourceActionVerb, ResourceCatalog, ResourceClass, ResourceDiscoveryEntry,
    ResourceDiscoveryProjection, ResourceScope, TransportProtocol,
};
use mackes_mesh_types::workloads::{
    WorkloadOperationAction, WorkloadOperationRequest, WORKLOAD_OPERATION_TOPIC,
};
use mde_egui::egui::{self, RichText};
use mde_egui::Style;
use serde::{Deserialize, Serialize};

const MAX_SEARCH_CHARS: usize = 128;
const RESOURCE_ACTION_TOPIC: &str = "action/resources/invoke";
const RESOURCE_ACTION_TTL_MS: u64 = 20_000;
// The shell publishes only RESOURCE_ACTION_TOPIC. This value is the daemon
// authority route that an accepted reply must report back; it is never a shell
// publication target.
const ANDROID_PROVIDER_ACTION_TOPIC: &str = "action/cloud/android-lifecycle";
const ACTION_REPLY_POLL_MS: u64 = 25;
static RESOURCE_ACTION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Frozen selection derived from one admitted full card. The UI never accepts
/// a node, workload, package, topic, path, URL, or command from the click.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AndroidStartBinding {
    catalog_revision: String,
    catalog_content_digest: String,
    resource_id: String,
    action_id: String,
    target: ResourceActionTarget,
    node: String,
    workload_id: String,
    app: AospStarterApp,
    card_expires_at_ms: u64,
    action_expires_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AndroidOperation {
    Start,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AndroidLifecycleRequest {
    schema_version: u16,
    node: String,
    workload_id: String,
    request_id: String,
    expected_generation: u64,
    operation: AndroidOperation,
    app: Option<AospStarterApp>,
    armed_token: Option<String>,
    typed_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "authority",
    content = "request",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum TypedAuthorityRequest {
    Workload(WorkloadOperationRequest),
    AndroidProvider(AndroidLifecycleRequest),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cancels_request_id: Option<String>,
    issued_at_ms: u64,
    deadline_at_ms: u64,
    authority_request: TypedAuthorityRequest,
    armed_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ResourceActionRefusal {
    Malformed,
    Unauthorized,
    StaleCatalog,
    StaleCard,
    Unavailable,
    CapabilityMismatch,
    TargetMismatch,
    AuthorityUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DownstreamReplyKind {
    WorkloadOperation,
    VdiAuthorityCompletion,
    ClipboardReceipt,
    CloudOperation,
}

/// Complete immutable identity echoed by the resource router. Matching only a
/// request ID is insufficient: a hostile reply must not substitute another
/// card, action, target, generation, or authorization/cancellation capability.
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
    refusal: Option<ResourceActionRefusal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResourceActionReceipt {
    invocation: ResourceActionInvocation,
    reply: ResourceActionReply,
}

#[derive(Debug, Default)]
struct ResourceReplyLedger {
    consumed_requests: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingActionKind {
    Start,
    Cancel,
}

#[derive(Debug)]
struct PendingResourceAction {
    kind: PendingActionKind,
    receiver: Receiver<Result<ResourceActionReceipt, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FeedState {
    Ready,
    Reconnecting(String),
    Unavailable(String),
    Conflict(String),
}

impl Default for FeedState {
    fn default() -> Self {
        Self::Unavailable("No admitted resource catalog is available yet.".into())
    }
}

#[derive(Debug)]
pub(super) struct RemoteSessionsModel {
    projection: Option<ResourceDiscoveryProjection>,
    android_starts: BTreeMap<String, AndroidStartBinding>,
    cancellable_actions: BTreeMap<String, ResourceActionReceipt>,
    reply_ledger: ResourceReplyLedger,
    feed_state: FeedState,
    query: String,
    class_filter: Option<ResourceClass>,
    action_pending: Option<PendingResourceAction>,
    action_feedback: Option<String>,
}

impl Default for RemoteSessionsModel {
    fn default() -> Self {
        Self {
            projection: None,
            android_starts: BTreeMap::new(),
            cancellable_actions: BTreeMap::new(),
            reply_ledger: ResourceReplyLedger::default(),
            feed_state: FeedState::default(),
            query: String::new(),
            class_filter: None,
            action_pending: None,
            action_feedback: None,
        }
    }
}

impl RemoteSessionsModel {
    pub(super) fn install_catalog(&mut self, catalog: ResourceCatalog) -> Result<(), String> {
        let projection = catalog
            .discovery_projection()
            .map_err(|error| format!("Resource catalog rejected: {error}"))?;
        let android_starts = android_start_bindings(&catalog);
        self.install_projection(projection)?;
        self.android_starts = android_starts;
        Ok(())
    }

    fn install_projection(
        &mut self,
        projection: ResourceDiscoveryProjection,
    ) -> Result<(), String> {
        projection
            .validate()
            .map_err(|error| format!("Resource catalog projection rejected: {error}"))?;

        if let Some(previous) = &self.projection {
            let same_publisher = previous.publisher == projection.publisher;
            let same_generation = previous.publisher == projection.publisher
                && previous.revision == projection.revision;
            let different_content = previous.catalog_content_digest
                != projection.catalog_content_digest
                || previous.entries != projection.entries;
            let publisher_rollback =
                same_publisher && projection.generated_at_ms < previous.generated_at_ms;
            let publisher_equivocation = same_publisher
                && projection.generated_at_ms == previous.generated_at_ms
                && (previous.revision != projection.revision || different_content);
            if (same_generation && different_content)
                || publisher_rollback
                || publisher_equivocation
            {
                self.feed_state = FeedState::Conflict(format!(
                    "Publisher {} supplied a conflicting or non-monotonic catalog at revision {}. The last admitted snapshot remains visible and actionless.",
                    projection.publisher, projection.revision
                ));
                self.android_starts.clear();
                self.cancellable_actions.clear();
                return Err("conflicting resource catalog revision".into());
            }
            if !same_generation
                || previous.catalog_content_digest != projection.catalog_content_digest
            {
                // A cancellation must be issued against the same admitted
                // card/action generation as its original request. A refreshed
                // catalog requires the owning action surface to re-admit a new
                // operation instead of carrying an old cancellation handle.
                self.cancellable_actions.clear();
            }
        }

        self.projection = Some(projection);
        self.feed_state = FeedState::Ready;
        Ok(())
    }

    pub(super) fn mark_reconnecting(&mut self, detail: impl Into<String>) {
        self.feed_state = FeedState::Reconnecting(bounded_detail(detail.into()));
    }

    pub(super) fn mark_unavailable(&mut self, detail: impl Into<String>) {
        self.feed_state = FeedState::Unavailable(bounded_detail(detail.into()));
    }

    fn set_query(&mut self, query: String) {
        self.query = query.chars().take(MAX_SEARCH_CHARS).collect();
    }

    fn visible_entries(&self, _now_ms: u64) -> Vec<&ResourceDiscoveryEntry> {
        let needle = self.query.trim().to_lowercase();
        self.projection
            .iter()
            .flat_map(|projection| projection.entries.iter())
            .filter(|entry| {
                self.class_filter.is_none_or(|class| class == entry.class)
                    && (needle.is_empty()
                        || entry.display_name.to_lowercase().contains(&needle)
                        || entry
                            .summary
                            .as_deref()
                            .is_some_and(|summary| summary.to_lowercase().contains(&needle))
                        || class_label(entry.class).to_lowercase().contains(&needle)
                        || entry
                            .transport_protocols
                            .iter()
                            .any(|protocol| protocol_label(*protocol).contains(&needle)))
            })
            .collect()
    }

    fn begin_android_start(&mut self, resource_id: &str, now_ms: u64) {
        if self.action_pending.is_some() {
            return;
        }
        let Some(binding) = self.android_starts.get(resource_id).cloned() else {
            self.action_feedback = Some(
                "Start refused: this card has no workload-bound Android action identity."
                    .to_owned(),
            );
            return;
        };
        let invocation = match android_start_invocation(&binding, now_ms) {
            Ok(invocation) => invocation,
            Err(error) => {
                self.action_feedback = Some(format!("Start refused: {error}"));
                return;
            }
        };
        let (sender, receiver) = mpsc::channel();
        self.action_feedback = Some(format!(
            "Starting {} through resource authority…",
            binding.app.display_name()
        ));
        self.action_pending = Some(PendingResourceAction {
            kind: PendingActionKind::Start,
            receiver,
        });
        std::thread::spawn(move || {
            let result = publish_resource_action(invocation);
            let _ = sender.send(result);
        });
    }

    fn begin_cancellation(&mut self, resource_id: &str, now_ms: u64) {
        if self.action_pending.is_some() {
            return;
        }
        let Some(prior) = self.cancellable_actions.get(resource_id).cloned() else {
            self.action_feedback = Some(
                "Cancel refused: no fully correlated workload action is active for this card."
                    .to_owned(),
            );
            return;
        };
        let invocation = match workload_cancellation_invocation(&prior, now_ms) {
            Ok(invocation) => invocation,
            Err(error) => {
                self.action_feedback = Some(format!("Cancel refused: {error}"));
                return;
            }
        };
        let (sender, receiver) = mpsc::channel();
        self.action_feedback = Some("Cancelling exact workload action…".to_owned());
        self.action_pending = Some(PendingResourceAction {
            kind: PendingActionKind::Cancel,
            receiver,
        });
        std::thread::spawn(move || {
            let result = publish_resource_action(invocation);
            let _ = sender.send(result);
        });
    }

    fn poll_action(&mut self) {
        let Some(pending) = self.action_pending.as_ref() else {
            return;
        };
        let kind = pending.kind;
        match pending.receiver.try_recv() {
            Ok(Ok(receipt)) => {
                let request_id = receipt.invocation.request_id.clone();
                let resource_id = receipt.invocation.resource_id.clone();
                let cancels_request_id = receipt.invocation.cancels_request_id.clone();
                // The authority may finish after the catalog changed while the
                // request was in flight.  Its downstream effect is owned by
                // the daemon, but this browser must not present that stale
                // result as belonging to the newly admitted generation or
                // retain a cancellation handle for it.
                if !self.invocation_matches_current_catalog(&receipt.invocation) {
                    self.action_feedback = Some(
                        "Action reply refused: the admitted resource generation changed."
                            .to_owned(),
                    );
                    self.action_pending = None;
                    return;
                }
                if matches!(kind, PendingActionKind::Cancel)
                    && self
                        .cancellable_actions
                        .get(&resource_id)
                        .is_none_or(|active| {
                            validate_cancellation_target(&receipt.invocation, active).is_err()
                        })
                {
                    self.action_feedback =
                        Some("Cancellation reply refused: active target changed.".to_owned());
                    self.action_pending = None;
                    return;
                }
                if let Err(error) = self.reply_ledger.admit(&receipt, unix_now_ms()) {
                    self.action_feedback = Some(format!("Action reply refused: {error}"));
                    self.action_pending = None;
                    return;
                }
                if matches!(kind, PendingActionKind::Cancel) {
                    if self
                        .cancellable_actions
                        .get(&resource_id)
                        .is_some_and(|active| {
                            Some(active.invocation.request_id.as_str())
                                == cancels_request_id.as_deref()
                        })
                    {
                        self.cancellable_actions.remove(&resource_id);
                    }
                    self.action_feedback = Some(format!("Cancellation accepted · {request_id}"));
                } else {
                    if matches!(
                        &receipt.invocation.authority_request,
                        TypedAuthorityRequest::Workload(_)
                    ) {
                        self.cancellable_actions.insert(resource_id, receipt);
                    }
                    self.action_feedback = Some(format!("Start accepted · {request_id}"));
                }
                self.action_pending = None;
            }
            Ok(Err(error)) => {
                let label = match kind {
                    PendingActionKind::Start => "Start",
                    PendingActionKind::Cancel => "Cancel",
                };
                self.action_feedback = Some(format!("{label} failed: {error}"));
                self.action_pending = None;
            }
            Err(TryRecvError::Disconnected) => {
                self.action_feedback = Some("Action failed: action publisher exited.".to_owned());
                self.action_pending = None;
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    fn invocation_matches_current_catalog(&self, invocation: &ResourceActionInvocation) -> bool {
        matches!(self.feed_state, FeedState::Ready)
            && self.projection.as_ref().is_some_and(|projection| {
                projection.revision == invocation.catalog_revision
                    && projection.catalog_content_digest.as_deref()
                        == Some(invocation.catalog_content_digest.as_str())
            })
    }
}

pub(super) fn remote_sessions_panel(ui: &mut egui::Ui, model: &mut RemoteSessionsModel) {
    let now_ms = unix_now_ms();
    model.poll_action();
    ui.vertical(|ui| {
        ui.heading("Remote Sessions");
        ui.label(
            RichText::new(
                "One catalog for desktops, applications, machines, services, media, and shares",
            )
            .color(Style::TEXT_DIM),
        );
        ui.add_space(Style::SP_S);

        paint_feed_state(ui, model);
        if let Some(feedback) = &model.action_feedback {
            ui.label(RichText::new(feedback).small().color(Style::TEXT_DIM));
        }
        ui.add_space(Style::SP_S);

        ui.horizontal(|ui| {
            let mut query = model.query.clone();
            let response = ui.add_sized(
                [ui.available_width().min(360.0), 32.0],
                egui::TextEdit::singleline(&mut query).hint_text("Search sessions and resources"),
            );
            if response.changed() {
                model.set_query(query);
            }
            egui::ComboBox::from_id_salt("remote-sessions-class-filter")
                .selected_text(model.class_filter.map_or("All types", class_label))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut model.class_filter, None, "All types");
                    for class in RESOURCE_CLASSES {
                        ui.selectable_value(
                            &mut model.class_filter,
                            Some(class),
                            class_label(class),
                        );
                    }
                });
        });
        ui.add_space(Style::SP_M);

        let visible = model.visible_entries(now_ms);
        if model.projection.is_none() {
            ui.label("No resource cards can be shown until a snapshot is admitted.");
            return;
        }
        if visible.is_empty() {
            ui.label("No resources match this search and filter.");
            return;
        }

        let mut requested_action = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for class in RESOURCE_CLASSES {
                let group: Vec<_> = visible
                    .iter()
                    .copied()
                    .filter(|entry| entry.class == class)
                    .collect();
                if group.is_empty() {
                    continue;
                }
                ui.label(
                    RichText::new(format!("{} · {}", class_label(class), group.len()))
                        .strong()
                        .color(Style::TEXT),
                );
                for entry in group {
                    let can_start_android = model.android_starts.contains_key(&entry.resource_id)
                        && model.action_pending.is_none();
                    let can_cancel = model.cancellable_actions.contains_key(&entry.resource_id)
                        && model.action_pending.is_none();
                    if let Some(intent) = paint_resource_card(
                        ui,
                        entry,
                        now_ms,
                        &model.feed_state,
                        can_start_android,
                        can_cancel,
                    ) {
                        requested_action = Some((entry.resource_id.clone(), intent));
                    }
                }
                ui.add_space(Style::SP_M);
            }
        });
        drop(visible);
        if let Some((resource_id, intent)) = requested_action {
            match intent {
                CardActionIntent::StartAndroid => model.begin_android_start(&resource_id, now_ms),
                CardActionIntent::Cancel => model.begin_cancellation(&resource_id, now_ms),
            }
        }
    });
}

fn paint_feed_state(ui: &mut egui::Ui, model: &RemoteSessionsModel) {
    let (title, detail) = match &model.feed_state {
        FeedState::Ready => {
            let projection = model.projection.as_ref().expect("ready has projection");
            (
                "Catalog current".to_owned(),
                format!(
                    "{} · revision {} · {} resources",
                    projection.publisher,
                    projection.revision,
                    projection.entries.len()
                ),
            )
        }
        FeedState::Reconnecting(detail) => ("Reconnecting to catalog".into(), detail.clone()),
        FeedState::Unavailable(detail) => ("Catalog unavailable".into(), detail.clone()),
        FeedState::Conflict(detail) => ("Catalog conflict".into(), detail.clone()),
    };
    egui::Frame::default()
        .fill(Style::SURFACE)
        .corner_radius(egui::CornerRadius::same(Style::RADIUS_M as u8))
        .inner_margin(egui::Margin::same(Style::SP_S as i8))
        .show(ui, |ui| {
            ui.label(RichText::new(title).strong());
            ui.label(RichText::new(detail).small().color(Style::TEXT_DIM));
        });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardActionIntent {
    StartAndroid,
    Cancel,
}

fn paint_resource_card(
    ui: &mut egui::Ui,
    entry: &ResourceDiscoveryEntry,
    now_ms: u64,
    feed_state: &FeedState,
    can_start_android: bool,
    can_cancel: bool,
) -> Option<CardActionIntent> {
    let mut intent = None;
    egui::Frame::default()
        .fill(Style::SURFACE)
        .corner_radius(egui::CornerRadius::same(Style::RADIUS_M as u8))
        .inner_margin(egui::Margin::same(Style::SP_M as i8))
        .outer_margin(egui::Margin::symmetric(0, Style::SP_XS as i8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&entry.display_name).strong());
                badge(ui, availability_label(entry, now_ms, feed_state));
                badge(ui, auth_label(entry.auth_status));
                for protocol in &entry.transport_protocols {
                    badge(ui, protocol_label(*protocol));
                }
            });
            if let Some(summary) = &entry.summary {
                ui.label(summary);
            }
            ui.label(
                RichText::new(format!(
                    "Freshness: {} · provenance: {} · scope: {}",
                    freshness_label(entry, now_ms),
                    joined_sources(&entry.discovery_sources),
                    joined_scopes(&entry.reachability_scopes),
                ))
                .small()
                .color(Style::TEXT_DIM),
            );
            if !entry.ready_actions.is_empty() {
                if entry.ready_actions.contains(&ResourceActionVerb::Start) {
                    let usable = can_start_android
                        && matches!(feed_state, FeedState::Ready)
                        && entry.expires_at_ms > now_ms
                        && matches!(entry.health_status, HealthStatus::Available | HealthStatus::Degraded)
                        && matches!(entry.auth_status, AuthStatus::NotRequired | AuthStatus::Authorized);
                    let button = ui.add_enabled(usable, egui::Button::new("Start"));
                    if button.clicked() {
                        intent = Some(CardActionIntent::StartAndroid);
                    }
                    if !can_start_android {
                        let _ = button.on_hover_text(
                            "Android Start requires an exact workload-bound identity from the admitted full card.",
                        );
                    }
                } else {
                    ui.label(
                        RichText::new("No supported Remote Sessions action is wired for this card.")
                            .small()
                            .color(Style::TEXT_DIM),
                    );
                }
            }
            if can_cancel {
                let usable = matches!(feed_state, FeedState::Ready)
                    && entry.expires_at_ms > now_ms
                    && matches!(
                        entry.health_status,
                        HealthStatus::Available | HealthStatus::Degraded
                    )
                    && matches!(
                        entry.auth_status,
                        AuthStatus::NotRequired | AuthStatus::Authorized
                    );
                if ui
                    .add_enabled(usable, egui::Button::new("Cancel"))
                    .on_hover_text("Cancel only the exact accepted workload request for this card.")
                    .clicked()
                {
                    intent = Some(CardActionIntent::Cancel);
                }
            }
        });
    intent
}

fn android_start_bindings(catalog: &ResourceCatalog) -> BTreeMap<String, AndroidStartBinding> {
    let digest = catalog.computed_content_digest();
    catalog
        .cards
        .iter()
        .filter_map(|card| {
            if card.identity.class != ResourceClass::Application {
                return None;
            }
            let mut parts = card.identity.canonical_key.split('/');
            let (Some("android-app"), Some(node), Some(workload_id), Some(package), None) = (
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
            ) else {
                return None;
            };
            if !safe_android_segment(node) || !safe_android_segment(workload_id) {
                return None;
            }
            let app = AospStarterApp::ALL
                .into_iter()
                .find(|app| app.package_id().as_str() == package)?;
            let mut starts = card.actions.iter().filter(|action| {
                action.verb == ResourceActionVerb::Start
                    && action.target == ResourceActionTarget::Resource
                    && action.availability.status == ActionAvailabilityStatus::Ready
            });
            let action = starts.next()?;
            if starts.next().is_some() {
                return None;
            }
            Some((
                card.resource_id().to_owned(),
                AndroidStartBinding {
                    catalog_revision: catalog.revision.clone(),
                    catalog_content_digest: digest.clone(),
                    resource_id: card.resource_id().to_owned(),
                    action_id: action.action_id.clone(),
                    target: action.target.clone(),
                    node: node.to_owned(),
                    workload_id: workload_id.to_owned(),
                    app,
                    card_expires_at_ms: card.expires_at_ms,
                    action_expires_at_ms: action.expires_at_ms,
                },
            ))
        })
        .collect()
}

fn safe_android_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
}

fn android_start_invocation(
    binding: &AndroidStartBinding,
    now_ms: u64,
) -> Result<ResourceActionInvocation, String> {
    let deadline_at_ms = now_ms
        .saturating_add(RESOURCE_ACTION_TTL_MS)
        .min(binding.card_expires_at_ms)
        .min(binding.action_expires_at_ms);
    if now_ms == 0 || deadline_at_ms <= now_ms {
        return Err("the workload-bound action is stale".to_owned());
    }
    let sequence = RESOURCE_ACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let request_id = format!("resource-android-start-{now_ms}-{sequence}");
    Ok(ResourceActionInvocation {
        schema_version: 1,
        request_id: request_id.clone(),
        catalog_revision: binding.catalog_revision.clone(),
        catalog_content_digest: binding.catalog_content_digest.clone(),
        resource_id: binding.resource_id.clone(),
        action_id: binding.action_id.clone(),
        verb: ResourceActionVerb::Start,
        target: binding.target.clone(),
        expected_generation: 0,
        cancellation_id: format!("cancel-{request_id}"),
        cancels_request_id: None,
        issued_at_ms: now_ms,
        deadline_at_ms,
        authority_request: TypedAuthorityRequest::AndroidProvider(AndroidLifecycleRequest {
            schema_version: 1,
            node: binding.node.clone(),
            workload_id: binding.workload_id.clone(),
            request_id,
            expected_generation: 0,
            operation: AndroidOperation::Start,
            app: Some(binding.app),
            armed_token: None,
            typed_name: None,
        }),
        armed_token: None,
    })
}

fn workload_cancellation_invocation(
    prior: &ResourceActionReceipt,
    now_ms: u64,
) -> Result<ResourceActionInvocation, String> {
    // Only an admitted ordinary Workload operation can be cancelled. The
    // Android, VDI, and clipboard authorities need their own typed cancellation
    // contracts and must never be coerced into a Workload cancellation.
    let TypedAuthorityRequest::Workload(previous_request) = &prior.invocation.authority_request
    else {
        return Err("the accepted action is not owned by Workloads".to_owned());
    };
    if prior.invocation.cancels_request_id.is_some()
        || previous_request.action == WorkloadOperationAction::Cancel
        || prior.invocation.expected_generation == 0
        || previous_request.expected_generation != prior.invocation.expected_generation
        || previous_request.request_id != prior.invocation.request_id
        || prior.reply.binding.as_ref() != Some(&reply_binding(&prior.invocation))
    {
        return Err("the accepted workload action identity is inconsistent".to_owned());
    }
    let deadline_at_ms = now_ms
        .saturating_add(RESOURCE_ACTION_TTL_MS)
        .min(prior.invocation.deadline_at_ms);
    if now_ms == 0 || deadline_at_ms <= now_ms {
        return Err("the accepted workload action is stale".to_owned());
    }

    let sequence = RESOURCE_ACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let request_id = format!("resource-workload-cancel-{now_ms}-{sequence}");
    let mut request = previous_request.clone();
    request.request_id = request_id.clone();
    request.action = WorkloadOperationAction::Cancel;
    request.target_request_id = Some(prior.invocation.request_id.clone());
    request.deadline_at_ms = deadline_at_ms;
    request.armed_token = None;

    let invocation = ResourceActionInvocation {
        schema_version: 1,
        request_id: request_id.clone(),
        catalog_revision: prior.invocation.catalog_revision.clone(),
        catalog_content_digest: prior.invocation.catalog_content_digest.clone(),
        resource_id: prior.invocation.resource_id.clone(),
        action_id: prior.invocation.action_id.clone(),
        verb: prior.invocation.verb,
        target: prior.invocation.target.clone(),
        expected_generation: prior.invocation.expected_generation,
        cancellation_id: format!("cancel-{request_id}"),
        cancels_request_id: Some(prior.invocation.request_id.clone()),
        issued_at_ms: now_ms,
        deadline_at_ms,
        authority_request: TypedAuthorityRequest::Workload(request),
        armed_token: None,
    };
    validate_cancellation_target(&invocation, prior)?;
    Ok(invocation)
}

fn validate_cancellation_target(
    cancellation: &ResourceActionInvocation,
    prior: &ResourceActionReceipt,
) -> Result<(), String> {
    let TypedAuthorityRequest::Workload(request) = &cancellation.authority_request else {
        return Err("cancellation authority is not Workloads".to_owned());
    };
    if cancellation.request_id == prior.invocation.request_id
        || cancellation.catalog_revision != prior.invocation.catalog_revision
        || cancellation.catalog_content_digest != prior.invocation.catalog_content_digest
        || cancellation.resource_id != prior.invocation.resource_id
        || cancellation.action_id != prior.invocation.action_id
        || cancellation.verb != prior.invocation.verb
        || cancellation.target != prior.invocation.target
        || cancellation.expected_generation != prior.invocation.expected_generation
        || cancellation.cancels_request_id.as_deref() != Some(prior.invocation.request_id.as_str())
        || request.request_id != cancellation.request_id
        || request.target_request_id != cancellation.cancels_request_id
        || request.action != WorkloadOperationAction::Cancel
        || request.expected_generation != prior.invocation.expected_generation
        || request.deadline_at_ms != cancellation.deadline_at_ms
        || request.armed_token.is_some()
    {
        return Err("cancellation target identity does not match the accepted action".to_owned());
    }
    let TypedAuthorityRequest::Workload(prior_request) = &prior.invocation.authority_request else {
        return Err("accepted action authority is not Workloads".to_owned());
    };
    if request.workload_id != prior_request.workload_id
        || request.target_node != prior_request.target_node
        || request.backend != prior_request.backend
        || request.resources != prior_request.resources
        || request.image_ref != prior_request.image_ref
        || request.preferred_attachment != prior_request.preferred_attachment
    {
        return Err("cancellation workload identity was substituted".to_owned());
    }
    Ok(())
}

fn reply_binding(invocation: &ResourceActionInvocation) -> ResourceActionReplyBinding {
    ResourceActionReplyBinding {
        catalog_revision: invocation.catalog_revision.clone(),
        catalog_content_digest: invocation.catalog_content_digest.clone(),
        resource_id: invocation.resource_id.clone(),
        action_id: invocation.action_id.clone(),
        verb: invocation.verb,
        target: invocation.target.clone(),
        expected_generation: invocation.expected_generation,
        cancellation_id: invocation.cancellation_id.clone(),
        cancels_request_id: invocation.cancels_request_id.clone(),
    }
}

fn expected_downstream(
    invocation: &ResourceActionInvocation,
) -> Result<(&'static str, DownstreamReplyKind), String> {
    match &invocation.authority_request {
        TypedAuthorityRequest::Workload(request) => {
            if request.request_id != invocation.request_id
                || request.expected_generation != invocation.expected_generation
                || request.deadline_at_ms != invocation.deadline_at_ms
                || request.target_request_id != invocation.cancels_request_id
                || (request.action == WorkloadOperationAction::Cancel)
                    != invocation.cancels_request_id.is_some()
            {
                return Err("workload authority identity does not match invocation".to_owned());
            }
            Ok((
                WORKLOAD_OPERATION_TOPIC,
                DownstreamReplyKind::WorkloadOperation,
            ))
        }
        TypedAuthorityRequest::AndroidProvider(request) => {
            if request.request_id != invocation.request_id
                || request.expected_generation != invocation.expected_generation
                || invocation.cancels_request_id.is_some()
            {
                return Err("Android authority identity does not match invocation".to_owned());
            }
            Ok((
                ANDROID_PROVIDER_ACTION_TOPIC,
                DownstreamReplyKind::CloudOperation,
            ))
        }
    }
}

fn valid_generated_reply_topic(topic: &str) -> bool {
    let Some(message_id) = topic.strip_prefix("reply/") else {
        return false;
    };
    !message_id.is_empty()
        && message_id.len() <= 255
        && message_id.is_ascii()
        && !message_id.contains(['/', '\\'])
        && !message_id.contains("..")
}

fn validate_resource_action_reply(
    invocation: &ResourceActionInvocation,
    reply: &ResourceActionReply,
    now_ms: u64,
) -> Result<(), String> {
    if now_ms == 0 || now_ms > invocation.deadline_at_ms {
        return Err("resource action reply is stale".to_owned());
    }
    if reply.schema_version != 1 || reply.request_id != invocation.request_id {
        return Err("resource action reply request identity does not match".to_owned());
    }
    if !reply.accepted {
        if reply.binding.is_some()
            || reply.downstream_topic.is_some()
            || reply.downstream_reply_topic.is_some()
            || reply.downstream_reply_kind.is_some()
            || reply.refusal.is_none()
        {
            return Err("malformed resource action refusal".to_owned());
        }
        return Err(format!(
            "resource authority refused the action: {:?}",
            reply.refusal.expect("checked present")
        ));
    }
    if reply.refusal.is_some() || reply.binding.as_ref() != Some(&reply_binding(invocation)) {
        return Err("resource action reply binding does not match".to_owned());
    }
    let (expected_topic, expected_kind) = expected_downstream(invocation)?;
    if reply.downstream_topic.as_deref() != Some(expected_topic)
        || reply.downstream_reply_kind != Some(expected_kind)
        || !reply
            .downstream_reply_topic
            .as_deref()
            .is_some_and(valid_generated_reply_topic)
    {
        return Err("resource action reply substituted its authority route".to_owned());
    }
    Ok(())
}

impl ResourceReplyLedger {
    fn admit(&mut self, receipt: &ResourceActionReceipt, now_ms: u64) -> Result<(), String> {
        validate_resource_action_reply(&receipt.invocation, &receipt.reply, now_ms)?;
        if !self
            .consumed_requests
            .insert(receipt.invocation.request_id.clone())
        {
            return Err("resource action reply was replayed".to_owned());
        }
        Ok(())
    }
}

fn resource_auth_context(
    invocation: &ResourceActionInvocation,
) -> Result<(&'static str, String), String> {
    let target = format!("{}:{}", invocation.resource_id, invocation.action_id);
    if matches!(
        &invocation.authority_request,
        TypedAuthorityRequest::Workload(WorkloadOperationRequest {
            action: WorkloadOperationAction::Cancel,
            ..
        })
    ) {
        return Ok(("resource-action-cancel", target));
    }
    let verb = match invocation.verb {
        ResourceActionVerb::Connect => "resource-action-connect",
        ResourceActionVerb::Launch => "resource-action-launch",
        ResourceActionVerb::Start => "resource-action-start",
        ResourceActionVerb::Resume => "resource-action-resume",
        ResourceActionVerb::Transfer => "resource-action-transfer",
        _ => return Err("unsupported resource action verb".to_owned()),
    };
    Ok((verb, target))
}

fn publish_resource_action(
    invocation: ResourceActionInvocation,
) -> Result<ResourceActionReceipt, String> {
    let unsigned = serde_json::to_string(&invocation)
        .map_err(|error| format!("encode typed resource action: {error}"))?;
    let (auth_verb, auth_target) = resource_auth_context(&invocation)?;
    let body = crate::iac::authorize_root_mutation_body(
        &unsigned,
        auth_verb,
        "resource-authority",
        &auth_target,
    )?;
    let root = mde_bus::client_data_dir()
        .ok_or_else(|| "the local mesh Bus directory is unavailable".to_owned())?;
    let persist = mde_bus::persist::Persist::open(root)
        .map_err(|error| format!("open local mesh Bus: {error}"))?;
    let ingress_id = mde_bus::rpc::publish_request(
        &persist,
        RESOURCE_ACTION_TOPIC,
        mde_bus::hooks::config::Priority::Default,
        None,
        Some(&body),
    )
    .map_err(|error| format!("publish typed resource action: {error}"))?;
    let reply_topic = mde_bus::rpc::reply_topic(&ingress_id);
    loop {
        let now_ms = unix_now_ms();
        if now_ms == 0 || now_ms > invocation.deadline_at_ms {
            return Err("resource authority reply timed out".to_owned());
        }
        let replies = persist
            .list_since_limit(&reply_topic, None, 2)
            .map_err(|error| format!("read typed resource action reply: {error}"))?;
        if replies.len() > 1 {
            return Err("resource authority reply lane contains a replay".to_owned());
        }
        if let Some(reply) = replies.first() {
            let body = reply
                .body
                .as_deref()
                .ok_or_else(|| "resource authority reply has no body".to_owned())?;
            let reply: ResourceActionReply = serde_json::from_str(body)
                .map_err(|error| format!("decode typed resource action reply: {error}"))?;
            validate_resource_action_reply(&invocation, &reply, now_ms)?;
            return Ok(ResourceActionReceipt { invocation, reply });
        }
        std::thread::sleep(Duration::from_millis(ACTION_REPLY_POLL_MS));
    }
}

fn availability_label(
    entry: &ResourceDiscoveryEntry,
    now_ms: u64,
    feed_state: &FeedState,
) -> &'static str {
    if matches!(feed_state, FeedState::Conflict(_)) {
        return "conflict";
    }
    if matches!(feed_state, FeedState::Reconnecting(_)) {
        return "reconnecting";
    }
    if entry.expires_at_ms <= now_ms {
        return "unavailable · stale";
    }
    match entry.health_status {
        HealthStatus::Unknown => "unavailable · unknown",
        HealthStatus::Available => "available",
        HealthStatus::Degraded => "available · degraded",
        HealthStatus::Unavailable => "unavailable",
        HealthStatus::Stale => "unavailable · stale",
    }
}

fn freshness_label(entry: &ResourceDiscoveryEntry, now_ms: u64) -> String {
    if entry.expires_at_ms <= now_ms {
        return format!(
            "expired {} ago",
            duration_label(now_ms - entry.expires_at_ms)
        );
    }
    format!(
        "seen {} ago · valid for {}",
        duration_label(now_ms.saturating_sub(entry.last_seen_at_ms)),
        duration_label(entry.expires_at_ms - now_ms)
    )
}

fn duration_label(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

fn badge(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).small().color(Style::TEXT_DIM));
}

fn bounded_detail(detail: String) -> String {
    const MAX_DETAIL_BYTES: usize = 256;
    if detail.len() <= MAX_DETAIL_BYTES {
        return detail;
    }
    let mut bounded = String::with_capacity(MAX_DETAIL_BYTES);
    for character in detail.chars() {
        if bounded.len().saturating_add(character.len_utf8()) > MAX_DETAIL_BYTES - 3 {
            break;
        }
        bounded.push(character);
    }
    bounded.push_str("...");
    bounded
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

const RESOURCE_CLASSES: [ResourceClass; 10] = [
    ResourceClass::Desktop,
    ResourceClass::Application,
    ResourceClass::VirtualMachine,
    ResourceClass::Container,
    ResourceClass::Node,
    ResourceClass::MediaServer,
    ResourceClass::FileShare,
    ResourceClass::NetworkDevice,
    ResourceClass::CloudWorkload,
    ResourceClass::Service,
];

const fn class_label(class: ResourceClass) -> &'static str {
    match class {
        ResourceClass::Node => "Nodes",
        ResourceClass::Desktop => "Desktops",
        ResourceClass::Application => "Applications",
        ResourceClass::VirtualMachine => "Virtual machines",
        ResourceClass::Container => "Containers",
        ResourceClass::MediaServer => "Media",
        ResourceClass::FileShare => "File shares",
        ResourceClass::NetworkDevice => "Network devices",
        ResourceClass::CloudWorkload => "Cloud workloads",
        ResourceClass::Service => "Services",
    }
}

const fn protocol_label(protocol: TransportProtocol) -> &'static str {
    match protocol {
        TransportProtocol::Rdp => "rdp",
        TransportProtocol::Vnc => "vnc",
        TransportProtocol::Spice => "spice",
        TransportProtocol::Moonlight => "moonlight",
        TransportProtocol::Ssh => "ssh",
        TransportProtocol::SshX11Application => "ssh+x11 app",
        TransportProtocol::X11Desktop => "x11 desktop",
        TransportProtocol::Jellyfin => "jellyfin",
        TransportProtocol::OpenSubsonic => "open-subsonic",
        TransportProtocol::DlnaUpnp => "dlna/upnp",
        TransportProtocol::Mpd => "mpd",
        TransportProtocol::Smb => "smb",
        TransportProtocol::Nfs => "nfs",
        TransportProtocol::WebDav => "webdav",
    }
}

const fn auth_label(status: AuthStatus) -> &'static str {
    match status {
        AuthStatus::NotRequired => "auth not required",
        AuthStatus::Required => "auth required",
        AuthStatus::Pending => "auth pending",
        AuthStatus::Authorized => "authorized",
        AuthStatus::Denied => "auth denied",
        AuthStatus::Revoked => "auth revoked",
        AuthStatus::Unavailable => "auth unavailable",
    }
}

fn joined_sources(sources: &[DiscoverySource]) -> String {
    sources
        .iter()
        .map(|source| match source {
            DiscoverySource::Local => "local",
            DiscoverySource::MeshDirectory => "mesh directory",
            DiscoverySource::MdnsDnsSd => "mDNS/DNS-SD",
            DiscoverySource::SsdpUpnp => "SSDP/UPnP",
            DiscoverySource::GatewayRegistry => "gateway",
            DiscoverySource::ProviderRegistry => "provider",
            DiscoverySource::Manual => "operator",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn joined_scopes(scopes: &[ResourceScope]) -> String {
    scopes
        .iter()
        .map(|scope| match scope {
            ResourceScope::Local => "local",
            ResourceScope::Mesh => "mesh",
            ResourceScope::TrustedLan => "trusted LAN",
            ResourceScope::Gateway => "gateway",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::resources::{
        ActionAvailability, AuthState, HealthState, IdentityAuthority, ProvenanceTrust,
        ResourceAction, ResourceCard, ResourceIdentity, ResourceOperatingRole, SourceProvenance,
        RESOURCE_CONTRACT_VERSION,
    };
    use mackes_mesh_types::workloads::{
        WorkloadBackend, WorkloadId, WorkloadResources, WORKLOAD_CONTRACT_SCHEMA_VERSION,
    };

    const NOW: u64 = 1_900_000_000_000;

    fn entry(
        suffix: char,
        class: ResourceClass,
        name: &str,
        protocol: TransportProtocol,
        expires_at_ms: u64,
    ) -> ResourceDiscoveryEntry {
        ResourceDiscoveryEntry {
            schema_version: RESOURCE_CONTRACT_VERSION,
            resource_id: format!("resource:v1:{}", suffix.to_string().repeat(64)),
            class,
            display_name: name.into(),
            summary: Some(format!("{name} summary")),
            health_status: HealthStatus::Available,
            auth_status: AuthStatus::Authorized,
            last_seen_at_ms: NOW - 1_000,
            expires_at_ms,
            discovery_sources: vec![DiscoverySource::MeshDirectory],
            reachability_scopes: vec![ResourceScope::Mesh],
            transport_protocols: vec![protocol],
            ready_actions: vec![ResourceActionVerb::Connect],
            service_category: None,
        }
    }

    fn projection(revision: &str, digest_char: char) -> ResourceDiscoveryProjection {
        ResourceDiscoveryProjection {
            schema_version: RESOURCE_CONTRACT_VERSION,
            revision: revision.into(),
            catalog_content_digest: Some(format!(
                "catalog:v1:{}",
                digest_char.to_string().repeat(64)
            )),
            publisher: "mesh-seat-193".into(),
            generated_at_ms: NOW,
            entries: vec![
                entry(
                    'a',
                    ResourceClass::Desktop,
                    "Engineering Desktop",
                    TransportProtocol::Rdp,
                    NOW + 60_000,
                ),
                entry(
                    'b',
                    ResourceClass::FileShare,
                    "Project Archive",
                    TransportProtocol::Smb,
                    NOW + 60_000,
                ),
            ],
        }
    }

    fn android_catalog(canonical_key: &str) -> ResourceCatalog {
        let identity = ResourceIdentity::new(
            ResourceClass::Application,
            IdentityAuthority::Mesh,
            canonical_key,
            vec![],
        )
        .expect("Android resource identity");
        let card = ResourceCard {
            schema_version: RESOURCE_CONTRACT_VERSION,
            identity,
            display_name: "Browser".into(),
            summary: Some("Workload-bound Android app".into()),
            first_seen_at_ms: NOW - 1_000,
            last_seen_at_ms: NOW,
            expires_at_ms: NOW + 60_000,
            health: HealthState {
                schema_version: RESOURCE_CONTRACT_VERSION,
                status: HealthStatus::Available,
                observed_at_ms: NOW,
                expires_at_ms: NOW + 60_000,
                latency_ms: None,
                failure: None,
            },
            auth: AuthState {
                schema_version: RESOURCE_CONTRACT_VERSION,
                status: AuthStatus::NotRequired,
                accepted_methods: vec![],
                active_method: None,
                credential_ref: None,
                updated_at_ms: NOW,
                expires_at_ms: None,
                failure: None,
            },
            provenance: vec![SourceProvenance {
                schema_version: RESOURCE_CONTRACT_VERSION,
                source: DiscoverySource::ProviderRegistry,
                source_id: "android-provider-node-a".into(),
                scope: ResourceScope::Mesh,
                trust: ProvenanceTrust::AuthenticatedMesh,
                interface: None,
                observed_at_ms: NOW,
                expires_at_ms: NOW + 60_000,
            }],
            transports: vec![],
            client_capabilities: vec![],
            actions: vec![ResourceAction {
                schema_version: RESOURCE_CONTRACT_VERSION,
                action_id: "start-browser".into(),
                verb: ResourceActionVerb::Start,
                target: ResourceActionTarget::Resource,
                availability: ActionAvailability {
                    status: ActionAvailabilityStatus::Ready,
                    failure: None,
                },
                issued_at_ms: NOW,
                expires_at_ms: NOW + 30_000,
            }],
            operating_roles: vec![ResourceOperatingRole::Loader],
            service: None,
        };
        let mut catalog = ResourceCatalog {
            schema_version: RESOURCE_CONTRACT_VERSION,
            revision: "revision-android-7".into(),
            publisher: "node-a".into(),
            generated_at_ms: NOW,
            content_digest: None,
            cards: vec![card],
        };
        catalog.content_digest = Some(catalog.computed_content_digest());
        catalog.validate().expect("valid Android catalog");
        catalog
    }

    fn workload_invocation() -> ResourceActionInvocation {
        let request_id = "resource-workload-start-1".to_owned();
        let request = WorkloadOperationRequest {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            request_id: request_id.clone(),
            workload_id: WorkloadId::new("vm-a").expect("workload ID"),
            backend: WorkloadBackend::LibvirtVirtqemud,
            resources: WorkloadResources {
                vcpu: 2,
                memory_mb: 4_096,
                disk_gb: 32,
            },
            image_ref: Some("construct:12.1".into()),
            target_node: "node-a".into(),
            expected_generation: 7,
            action: WorkloadOperationAction::Start,
            target_request_id: None,
            deadline_at_ms: NOW + 20_000,
            preferred_attachment: None,
            armed_token: None,
        };
        ResourceActionInvocation {
            schema_version: 1,
            request_id: request_id.clone(),
            catalog_revision: "revision-workload-9".into(),
            catalog_content_digest: format!("catalog:v1:{}", "a".repeat(64)),
            resource_id: format!("resource:v1:{}", "b".repeat(64)),
            action_id: "start-vm-a".into(),
            verb: ResourceActionVerb::Start,
            target: ResourceActionTarget::Resource,
            expected_generation: 7,
            cancellation_id: "cancel-resource-workload-start-1".into(),
            cancels_request_id: None,
            issued_at_ms: NOW,
            deadline_at_ms: NOW + 20_000,
            authority_request: TypedAuthorityRequest::Workload(request),
            armed_token: None,
        }
    }

    fn accepted_reply(invocation: &ResourceActionInvocation) -> ResourceActionReply {
        let (topic, kind) = expected_downstream(invocation).expect("known typed authority");
        ResourceActionReply {
            schema_version: 1,
            request_id: invocation.request_id.clone(),
            accepted: true,
            downstream_topic: Some(topic.into()),
            downstream_reply_topic: Some("reply/01K00000000000000000000000".into()),
            downstream_reply_kind: Some(kind),
            binding: Some(reply_binding(invocation)),
            refusal: None,
        }
    }

    fn accepted_workload_receipt() -> ResourceActionReceipt {
        let invocation = workload_invocation();
        ResourceActionReceipt {
            reply: accepted_reply(&invocation),
            invocation,
        }
    }

    #[test]
    fn remote_sessions_model_search_filter_and_capability_projection_are_deterministic() {
        let mut model = RemoteSessionsModel::default();
        model
            .install_projection(projection("revision-1", 'c'))
            .expect("admitted snapshot");

        model.set_query("RDP".into());
        let visible = model.visible_entries(NOW);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].display_name, "Engineering Desktop");
        assert_eq!(visible[0].transport_protocols, vec![TransportProtocol::Rdp]);

        model.set_query(String::new());
        model.class_filter = Some(ResourceClass::FileShare);
        let visible = model.visible_entries(NOW);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].display_name, "Project Archive");
    }

    #[test]
    fn remote_sessions_model_keeps_last_snapshot_and_exposes_reconnect_and_conflict() {
        let mut model = RemoteSessionsModel::default();
        model
            .install_projection(projection("revision-1", 'c'))
            .expect("first snapshot");
        model.mark_reconnecting("refresh timed out");
        assert!(matches!(model.feed_state, FeedState::Reconnecting(_)));
        assert_eq!(model.visible_entries(NOW).len(), 2);

        let error = model
            .install_projection(projection("revision-1", 'd'))
            .expect_err("same revision with changed content must conflict");
        assert!(error.contains("conflicting"));
        assert!(matches!(model.feed_state, FeedState::Conflict(_)));
        assert_eq!(model.visible_entries(NOW).len(), 2);
    }

    #[test]
    fn reconnect_detail_is_utf8_safe_and_byte_bounded() {
        let detail = bounded_detail("界".repeat(256));
        assert!(detail.len() <= 256);
        assert!(detail.is_char_boundary(detail.len()));
        assert!(detail.ends_with("..."));
    }

    #[test]
    fn remote_sessions_rejects_same_publisher_rollback_and_revokes_action_handles() {
        let mut newer = android_catalog("android-app/node-a/android-vm-a/com.android.browser");
        newer.revision = "revision-android-8".into();
        newer.generated_at_ms = NOW + 1_000;
        newer.content_digest = Some(newer.computed_content_digest());
        newer.validate().expect("newer catalog");

        let mut older = newer.clone();
        older.revision = "revision-android-7".into();
        older.generated_at_ms = NOW;
        older.content_digest = Some(older.computed_content_digest());
        older
            .validate()
            .expect("older catalog remains structurally valid");

        let resource_id = newer.cards[0].resource_id().to_owned();
        let mut model = RemoteSessionsModel::default();
        model
            .install_catalog(newer)
            .expect("newer catalog admitted");
        assert!(model.android_starts.contains_key(&resource_id));
        model
            .cancellable_actions
            .insert(resource_id.clone(), accepted_workload_receipt());

        let error = model
            .install_catalog(older)
            .expect_err("same-publisher rollback must fail closed");
        assert!(error.contains("conflicting"));
        assert!(matches!(model.feed_state, FeedState::Conflict(_)));
        assert_eq!(
            model
                .projection
                .as_ref()
                .expect("last good snapshot")
                .revision,
            "revision-android-8"
        );
        assert!(model.android_starts.is_empty());
        assert!(model.cancellable_actions.is_empty());
    }

    #[test]
    fn delayed_action_reply_after_catalog_replacement_is_not_adopted() {
        let receipt = accepted_workload_receipt();
        let resource_id = receipt.invocation.resource_id.clone();
        let mut model = RemoteSessionsModel::default();
        model
            .install_projection(projection("revision-workload-9", 'a'))
            .expect("request generation admitted");

        let (sender, receiver) = mpsc::channel();
        model.action_pending = Some(PendingResourceAction {
            kind: PendingActionKind::Start,
            receiver,
        });

        let mut replacement = projection("revision-workload-10", 'd');
        replacement.generated_at_ms = NOW + 1;
        model
            .install_projection(replacement)
            .expect("corrected-forward catalog admitted while action is in flight");
        sender
            .send(Ok(receipt))
            .expect("deliver delayed accepted authority reply");

        model.poll_action();

        assert!(model.action_pending.is_none());
        assert!(!model.cancellable_actions.contains_key(&resource_id));
        assert!(model
            .action_feedback
            .as_deref()
            .is_some_and(|message| message.contains("generation changed")));
    }

    #[test]
    fn remote_sessions_model_marks_expired_cards_unavailable_and_has_no_ui_action_mapping() {
        let mut snapshot = projection("revision-2", 'e');
        snapshot.entries[0].expires_at_ms = NOW + 1;
        let mut model = RemoteSessionsModel::default();
        model
            .install_projection(snapshot)
            .expect("bounded snapshot");
        let card = &model.visible_entries(NOW + 2)[0];
        assert_eq!(
            availability_label(card, NOW + 2, &model.feed_state),
            "unavailable · stale"
        );
        assert!(!card.ready_actions.is_empty());
        // Presentation intentionally has no action enum or callback: an
        // advertised action is not executable until a typed shell seam exists.
    }

    #[test]
    fn action_ready_android_card_routes_exact_workload_bound_start_intent() {
        let catalog = android_catalog("android-app/node-a/android-vm-a/com.android.browser");
        let resource_id = catalog.cards[0].resource_id().to_owned();
        let mut model = RemoteSessionsModel::default();
        model.install_catalog(catalog).expect("admitted catalog");
        let binding = model
            .android_starts
            .get(&resource_id)
            .expect("workload-bound Android action");
        assert_eq!(binding.node, "node-a");
        assert_eq!(binding.workload_id, "android-vm-a");
        assert_eq!(binding.app, AospStarterApp::Browser);

        let invocation = android_start_invocation(binding, NOW + 1).expect("typed Start intent");
        let document = serde_json::to_value(invocation).expect("wire document");
        assert_eq!(document["verb"], "start");
        assert_eq!(document["target"]["kind"], "resource");
        assert_eq!(
            document["authority_request"]["authority"],
            "android_provider"
        );
        let request = &document["authority_request"]["request"];
        assert_eq!(request["node"], "node-a");
        assert_eq!(request["workload_id"], "android-vm-a");
        assert_eq!(request["app"], "browser");
        assert_eq!(request["operation"], "start");
        assert_eq!(request["expected_generation"], 0);
        assert!(request["armed_token"].is_null());
        assert!(request["typed_name"].is_null());
        let wire = document.to_string();
        for forbidden in ["topic", "path", "url", "command", "executable"] {
            assert!(!wire.contains(&format!("\"{forbidden}\"")));
        }

        let invocation = android_start_invocation(binding, NOW + 1).expect("fresh Start intent");
        let reply = accepted_reply(&invocation);
        validate_resource_action_reply(&invocation, &reply, NOW + 2)
            .expect("fully bound Android router reply");
        assert_eq!(
            resource_auth_context(&invocation).expect("Android auth context"),
            (
                "resource-action-start",
                format!("{}:{}", invocation.resource_id, invocation.action_id)
            )
        );
    }

    #[test]
    fn workload_cancel_binds_the_exact_accepted_action_and_authorization_identity() {
        let prior = accepted_workload_receipt();
        let cancellation =
            workload_cancellation_invocation(&prior, NOW + 1).expect("exact cancellation");
        validate_cancellation_target(&cancellation, &prior).expect("bound cancellation target");

        assert_ne!(cancellation.request_id, prior.invocation.request_id);
        assert_eq!(
            cancellation.cancels_request_id.as_deref(),
            Some(prior.invocation.request_id.as_str())
        );
        assert_eq!(
            cancellation.catalog_revision,
            prior.invocation.catalog_revision
        );
        assert_eq!(
            cancellation.catalog_content_digest,
            prior.invocation.catalog_content_digest
        );
        assert_eq!(cancellation.resource_id, prior.invocation.resource_id);
        assert_eq!(cancellation.action_id, prior.invocation.action_id);
        assert_eq!(cancellation.verb, prior.invocation.verb);
        assert_eq!(cancellation.target, prior.invocation.target);
        assert_eq!(
            cancellation.expected_generation,
            prior.invocation.expected_generation
        );
        assert_eq!(
            resource_auth_context(&cancellation).expect("cancellation auth context"),
            (
                "resource-action-cancel",
                format!(
                    "{}:{}",
                    prior.invocation.resource_id, prior.invocation.action_id
                )
            )
        );

        let TypedAuthorityRequest::Workload(cancel_request) = &cancellation.authority_request
        else {
            panic!("cancellation changed authority");
        };
        let TypedAuthorityRequest::Workload(prior_request) = &prior.invocation.authority_request
        else {
            panic!("fixture changed authority");
        };
        assert_eq!(cancel_request.action, WorkloadOperationAction::Cancel);
        assert_eq!(
            cancel_request.target_request_id.as_deref(),
            Some(prior_request.request_id.as_str())
        );
        assert_eq!(cancel_request.workload_id, prior_request.workload_id);
        assert_eq!(cancel_request.target_node, prior_request.target_node);
        assert_eq!(cancel_request.backend, prior_request.backend);
        assert_eq!(cancel_request.resources, prior_request.resources);
        assert_eq!(cancel_request.image_ref, prior_request.image_ref);
        assert_eq!(
            cancel_request.preferred_attachment,
            prior_request.preferred_attachment
        );
        assert!(cancel_request.armed_token.is_none());
    }

    #[test]
    fn workload_cancel_rejects_cross_target_generation_action_and_authority_substitution() {
        let prior = accepted_workload_receipt();
        let cancellation =
            workload_cancellation_invocation(&prior, NOW + 1).expect("exact cancellation");

        let mut hostile = cancellation.clone();
        hostile.resource_id = format!("resource:v1:{}", "c".repeat(64));
        assert!(validate_cancellation_target(&hostile, &prior).is_err());

        let mut hostile = cancellation.clone();
        hostile.action_id = "start-another-vm".into();
        assert!(validate_cancellation_target(&hostile, &prior).is_err());

        let mut hostile = cancellation.clone();
        hostile.expected_generation += 1;
        assert!(validate_cancellation_target(&hostile, &prior).is_err());

        let mut hostile = cancellation.clone();
        hostile.cancels_request_id = Some("resource-workload-other".into());
        assert!(validate_cancellation_target(&hostile, &prior).is_err());

        let mut hostile = cancellation.clone();
        let TypedAuthorityRequest::Workload(request) = &mut hostile.authority_request else {
            unreachable!();
        };
        request.target_node = "node-b".into();
        assert!(validate_cancellation_target(&hostile, &prior).is_err());

        let android_catalog =
            android_catalog("android-app/node-a/android-vm-a/com.android.browser");
        let android_binding = android_start_bindings(&android_catalog)
            .into_values()
            .next()
            .expect("Android binding");
        let android = android_start_invocation(&android_binding, NOW + 1).expect("Android Start");
        let android_receipt = ResourceActionReceipt {
            reply: accepted_reply(&android),
            invocation: android,
        };
        assert!(workload_cancellation_invocation(&android_receipt, NOW + 2).is_err());
    }

    #[test]
    fn typed_router_replies_reject_stale_replayed_and_substituted_identity() {
        let prior = accepted_workload_receipt();
        let cancellation =
            workload_cancellation_invocation(&prior, NOW + 1).expect("exact cancellation");
        let receipt = ResourceActionReceipt {
            reply: accepted_reply(&cancellation),
            invocation: cancellation,
        };
        let mut ledger = ResourceReplyLedger::default();
        ledger
            .admit(&receipt, NOW + 2)
            .expect("first exact reply admitted");
        assert!(ledger.admit(&receipt, NOW + 2).is_err(), "replay admitted");
        assert!(validate_resource_action_reply(
            &receipt.invocation,
            &receipt.reply,
            receipt.invocation.deadline_at_ms + 1
        )
        .is_err());

        let assert_refused = |reply: ResourceActionReply| {
            assert!(
                validate_resource_action_reply(&receipt.invocation, &reply, NOW + 2).is_err(),
                "substituted reply was admitted: {reply:?}"
            );
        };

        let mut reply = receipt.reply.clone();
        reply.request_id = "resource-workload-other".into();
        assert_refused(reply);

        let mut reply = receipt.reply.clone();
        reply.binding.as_mut().expect("binding").resource_id =
            format!("resource:v1:{}", "d".repeat(64));
        assert_refused(reply);

        let mut reply = receipt.reply.clone();
        reply.binding.as_mut().expect("binding").action_id = "other-action".into();
        assert_refused(reply);

        let mut reply = receipt.reply.clone();
        reply.binding.as_mut().expect("binding").expected_generation += 1;
        assert_refused(reply);

        let mut reply = receipt.reply.clone();
        reply.binding.as_mut().expect("binding").cancellation_id = "other-capability".into();
        assert_refused(reply);

        let mut reply = receipt.reply.clone();
        reply.binding.as_mut().expect("binding").cancels_request_id =
            Some("resource-workload-other".into());
        assert_refused(reply);

        let mut reply = receipt.reply.clone();
        reply.downstream_topic = Some(ANDROID_PROVIDER_ACTION_TOPIC.into());
        assert_refused(reply);

        let mut reply = receipt.reply.clone();
        reply.downstream_reply_kind = Some(DownstreamReplyKind::CloudOperation);
        assert_refused(reply);

        let mut reply = receipt.reply.clone();
        reply.downstream_reply_topic = Some("reply/other/injected".into());
        assert_refused(reply);
    }

    #[test]
    fn legacy_unknown_and_substituted_android_identities_remain_actionless() {
        for canonical_key in [
            "android-app/node-a/com.android.browser",
            "android-app/node-a/android-vm-a/org.example.unapproved",
            "android-app/node-a/android-vm-a/substitute/com.android.browser",
            "android-app/node-a/android-vm-a/com.android.browser.evil",
        ] {
            let catalog = android_catalog(canonical_key);
            let mut model = RemoteSessionsModel::default();
            model
                .install_catalog(catalog)
                .expect("card remains inspectable");
            assert!(
                model.android_starts.is_empty(),
                "hostile identity became actionable: {canonical_key}"
            );
        }
    }

    #[test]
    fn android_start_selection_cannot_substitute_another_resource_identity() {
        let catalog = android_catalog("android-app/node-a/android-vm-a/com.android.browser");
        let resource_id = catalog.cards[0].resource_id().to_owned();
        let bindings = android_start_bindings(&catalog);
        assert!(bindings.contains_key(&resource_id));
        assert!(!bindings.contains_key(
            "resource:v1:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        ));
        let invocation = android_start_invocation(
            bindings.get(&resource_id).expect("exact card binding"),
            NOW + 1,
        )
        .expect("typed invocation");
        assert_eq!(invocation.resource_id, resource_id);
        let TypedAuthorityRequest::AndroidProvider(request) = invocation.authority_request else {
            panic!("Android binding changed authority");
        };
        assert_eq!(request.workload_id, "android-vm-a");
    }

    #[test]
    fn remote_sessions_model_presentation_renders_admitted_grouped_cards_without_io() {
        let mut model = RemoteSessionsModel::default();
        model
            .install_projection(projection("revision-3", 'f'))
            .expect("admitted snapshot");
        let context = egui::Context::default();
        Style::install(&context);
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 640.0),
                )),
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    remote_sessions_panel(ui, &mut model);
                });
            },
        );
        assert!(!output.shapes.is_empty());
        assert_eq!(model.visible_entries(NOW).len(), 2);
    }
}
