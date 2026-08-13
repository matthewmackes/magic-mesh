//! Provider views mounted as leaf destinations by the Workers catalog.
//!
//! The historical Workbench chrome, plane rail, and embedded Action Console
//! have been retired. This module retains the provider projection and typed
//! Action Console implementation while Workers remains the sole navigation
//! authority.
//!
//! WL-ARCH-006 — the mesh cloud left the Workbench: the old **Cloud** plane
//! retired into the first-class **Workloads** surface (`Surface::InfraCode`),
//! reached directly from the dock. The Workbench is now node/network/fleet
//! control only.

use mde_egui::egui;
use mde_egui::Style;

/// One of the four top-level control planes of the Workbench, ordered by blast
/// radius — from the local host outward to the whole fleet.
///
/// WL-ARCH-006 — the old Cloud plane was retired here: the mesh cloud is now its
/// own first-class **Workloads** surface (`Surface::InfraCode`), reached straight
/// from the dock, not folded into the Workbench.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Plane {
    /// This host — hardware, the local desktop seat, node-local services.
    #[default]
    ThisNode,
    /// Network fabric — the Nebula overlay, lighthouses, routes, reachability.
    Network,
    /// The fleet — every peer and the VM desktops they serve.
    Fleet,
    /// Provisioning — golden images, enrollment, bringing new nodes online.
    Provisioning,
}

impl Plane {
    /// Stable provider label used by typed workflow actions and audit text.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::ThisNode => "This Node",
            Self::Network => "Network",
            Self::Fleet => "Fleet",
            Self::Provisioning => "Provisioning",
        }
    }
}

/// Render one Workbench plane as a Workers catalog leaf. This intentionally
/// omits the Workbench menu, plane rail, and Action Console: those controls are
/// owned by the flat Workers catalog now.
#[allow(clippy::too_many_arguments)]
pub(crate) fn show_catalog_plane(
    ui: &mut egui::Ui,
    plane: Plane,
    datacenter: &mut crate::datacenter::DatacenterState,
    thisnode: &mut crate::thisnode::ThisNodeState,
    system: &mut crate::system::SystemState,
    surface_card: &mut crate::surface_card::SurfaceCardState,
    network: &crate::network::NetworkState,
    provisioning: &crate::provisioning::ProvisioningState,
    spawn_lighthouse: &mut crate::spawn_lighthouse_flow::SpawnLighthouseFlowState,
) -> Option<crate::surface_card::SurfaceCardHandoff> {
    match plane {
        Plane::ThisNode => {
            thisnode.show_with_system(ui, Some(system));
            surface_card
                .is_surface()
                .then(|| surface_card.show(ui))
                .flatten()
        }
        Plane::Network => {
            network.show(ui);
            None
        }
        Plane::Fleet => {
            datacenter.show(ui);
            None
        }
        Plane::Provisioning => {
            provisioning.show(ui);
            spawn_lighthouse.show(ui);
            None
        }
    }
}

/// The Action Console is a first-class Workers destination, not a child of a
/// Workbench plane. Its typed preview/authorization/commit state remains in the
/// same egui-owned state slot and therefore survives catalog selection.
pub(crate) fn show_action_console(ui: &mut egui::Ui) {
    action_console::show(ui);
}

/// WL-ARCH-009 S5 — the production Workers Action Console slice. State lives in
/// egui's context store so the already-mounted Workbench remains the sole route;
/// no shell-root field or duplicate surface is introduced.
mod action_console {
    use std::path::PathBuf;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use mackes_mesh_types::worker_runtime::{
        worker_change_set_action_topic, worker_change_set_digest, worker_change_set_result_topic,
        WorkerAction, WorkerActionDescriptor, WorkerArmingRequirement, WorkerChangeSetItem,
        WorkerChangeSetOperation, WorkerChangeSetOutcome, WorkerChangeSetRequest,
        WorkerChangeSetResult, WorkerChangeSetTarget, WorkerContract, WorkerGroup,
        WorkerRelationEndpoint, WorkerRuntimeSnapshot, MAX_WORKER_CHANGE_SET_TTL_MS,
        WORKER_CHANGE_SET_AUTH_VERB, WORKER_RUNTIME_SCHEMA_VERSION,
    };
    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;
    use mde_egui::egui::{self, RichText};
    use mde_egui::Style;
    use serde::{Deserialize, Serialize};

    const STATE_REFRESH: Duration = Duration::from_secs(2);
    const CONSOLE_STATE_ID: &str = "workers-action-console-state-v1";
    const MAX_NODE_WORKERS: usize = 256;
    const MAX_NODE_STATUS_WIRE_BYTES: usize = 4 * 1024 * 1024;
    const IMPACT: &str = "Apply the selected typed worker lifecycle action.";
    const RECOVERY: &str = "Cancel before commit or issue a typed inverse action.";

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct WorkerStatusRow {
        contract: WorkerContract,
        snapshot: WorkerRuntimeSnapshot,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct NodeStatusWire {
        schema_version: u16,
        node_id: String,
        observed_at_ms: u64,
        workers: Vec<WorkerStatusRow>,
    }

    impl NodeStatusWire {
        fn from_body(body: &str, expected_node: &str, now_ms: u64) -> Result<Self, String> {
            if body.len() > MAX_NODE_STATUS_WIRE_BYTES {
                return Err("Workers state exceeded the bounded wire size.".to_string());
            }
            let state: Self = serde_json::from_str(body)
                .map_err(|_| "Workers state failed closed-contract admission.".to_string())?;
            if state.schema_version != WORKER_RUNTIME_SCHEMA_VERSION
                || state.node_id != expected_node
                || state.observed_at_ms == 0
                || state.observed_at_ms > now_ms
                || state.workers.len() > MAX_NODE_WORKERS
            {
                return Err("Workers state identity, clock, or capacity was invalid.".to_string());
            }
            let mut previous: Option<(mackes_mesh_types::worker_runtime::WorkerGroup, &str)> = None;
            for row in &state.workers {
                row.contract
                    .validate()
                    .map_err(|error| format!("Worker contract refused: {error}"))?;
                row.snapshot
                    .validate_at(now_ms)
                    .map_err(|error| format!("Worker snapshot refused: {error}"))?;
                if row.snapshot.node_id != state.node_id
                    || row.snapshot.worker_id != row.contract.worker_id
                    || row.snapshot.group != row.contract.group
                {
                    return Err("Worker contract and snapshot identities diverged.".to_string());
                }
                let current = (row.contract.group, row.contract.worker_id.as_str());
                if previous.is_some_and(|prior| prior >= current) {
                    return Err(
                        "Workers state was duplicated or not deterministically ordered."
                            .to_string(),
                    );
                }
                previous = Some(current);
            }
            Ok(state)
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct StagedChange {
        request_id: String,
        target: WorkerChangeSetTarget,
        expected_generation: u64,
        items: Vec<WorkerChangeSetItem>,
        arming: WorkerArmingRequirement,
        digest: String,
        staged_at_ms: u64,
        preview_admitted: bool,
        terminal: bool,
        awaiting: Option<PendingOperation>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct PendingOperation {
        operation: WorkerChangeSetOperation,
        published_at_ms: u64,
    }

    #[derive(Debug, Clone)]
    struct ActionConsoleState {
        bus_root: Option<PathBuf>,
        node_id: String,
        workers: Vec<WorkerStatusRow>,
        selected_worker: Option<String>,
        selected_action: Option<WorkerAction>,
        staged: Option<StagedChange>,
        result: Option<WorkerChangeSetResult>,
        last_error: Option<String>,
        last_poll: Option<Instant>,
    }

    impl Default for ActionConsoleState {
        fn default() -> Self {
            Self {
                bus_root: mde_bus::client_data_dir(),
                node_id: local_node_id(),
                workers: Vec::new(),
                selected_worker: None,
                selected_action: None,
                staged: None,
                result: None,
                last_error: None,
                last_poll: None,
            }
        }
    }

    impl ActionConsoleState {
        fn poll(&mut self, now_ms: u64) {
            if self
                .last_poll
                .is_some_and(|last| last.elapsed() < STATE_REFRESH)
            {
                return;
            }
            self.last_poll = Some(Instant::now());
            let Some(root) = self.bus_root.clone() else {
                self.last_error = Some("Mesh Bus unavailable; worker actions are disabled.".into());
                return;
            };
            let Ok(persist) = Persist::open(root) else {
                self.last_error =
                    Some("Mesh Bus could not be opened; worker actions are disabled.".into());
                return;
            };
            let state_topic = format!("state/mackesd/{}", self.node_id);
            if let Ok(Some(message)) = persist.read_latest(&state_topic) {
                if let Some(body) = message.body {
                    match NodeStatusWire::from_body(&body, &self.node_id, now_ms) {
                        Ok(state) => {
                            self.workers = state.workers;
                            self.reconcile_selection();
                            self.reconcile_generation();
                            self.last_error = None;
                        }
                        Err(error) => self.last_error = Some(error),
                    }
                }
            }
            self.poll_result(&persist);
        }

        fn reconcile_selection(&mut self) {
            let selected_exists = self.selected_worker.as_ref().is_some_and(|selected| {
                self.workers
                    .iter()
                    .any(|row| row.contract.worker_id == *selected)
            });
            if !selected_exists {
                self.selected_worker = self
                    .workers
                    .first()
                    .map(|row| row.contract.worker_id.clone());
            }
            let allowed = self
                .selected_row()
                .map_or(&[][..], |row| row.contract.actions.as_slice());
            if !self.selected_action.is_some_and(|selected| {
                allowed
                    .iter()
                    .any(|descriptor| descriptor.action == selected)
            }) {
                self.selected_action = allowed.first().map(|descriptor| descriptor.action);
            }
        }

        fn reconcile_generation(&mut self) {
            let stale = self.staged.as_ref().is_some_and(|staged| {
                self.workers
                    .iter()
                    .find(|row| {
                        staged.target.worker_id.as_deref() == Some(row.contract.worker_id.as_str())
                    })
                    .is_none_or(|row| row.snapshot.generation != staged.expected_generation)
            });
            if stale {
                self.staged = None;
                self.result = None;
                self.last_error = Some(
                    "Worker generation changed; the staged preview was discarded.".to_string(),
                );
            }
        }

        fn selected_row(&self) -> Option<&WorkerStatusRow> {
            let worker_id = self.selected_worker.as_deref()?;
            self.workers
                .iter()
                .find(|row| row.contract.worker_id == worker_id)
        }

        fn selected_descriptor(&self) -> Option<&WorkerActionDescriptor> {
            let action = self.selected_action?;
            self.selected_row()?
                .contract
                .actions
                .iter()
                .find(|descriptor| descriptor.action == action)
        }

        fn stage_preview(&mut self, now_ms: u64) -> Result<(), String> {
            let row = self
                .selected_row()
                .cloned()
                .ok_or_else(|| "Select a live worker with an admitted action.".to_string())?;
            let descriptor = self
                .selected_descriptor()
                .cloned()
                .ok_or_else(|| "Select an action admitted by that worker.".to_string())?;
            let target = WorkerChangeSetTarget {
                node_id: row.snapshot.node_id.clone(),
                worker_id: Some(row.contract.worker_id.clone()),
            };
            let request_id = format!("workers-{}", uuid::Uuid::new_v4().simple());
            let items = vec![WorkerChangeSetItem {
                item_id: format!("item-{}", uuid::Uuid::new_v4().simple()),
                worker_id: row.contract.worker_id,
                action: descriptor.action,
            }];
            let digest = worker_change_set_digest(
                &target,
                row.snapshot.generation,
                &items,
                IMPACT,
                RECOVERY,
                descriptor.arming,
            )
            .map_err(|error| error.to_string())?;
            self.staged = Some(StagedChange {
                request_id,
                target,
                expected_generation: row.snapshot.generation,
                items,
                arming: descriptor.arming,
                digest,
                staged_at_ms: now_ms,
                preview_admitted: false,
                terminal: false,
                awaiting: None,
            });
            self.result = None;
            if let Err(error) = self.publish_operation(WorkerChangeSetOperation::Preview, now_ms) {
                self.staged = None;
                return Err(error);
            }
            Ok(())
        }

        fn publish_operation(
            &mut self,
            operation: WorkerChangeSetOperation,
            now_ms: u64,
        ) -> Result<(), String> {
            let staged = self
                .staged
                .as_ref()
                .ok_or_else(|| "No staged worker change exists.".to_string())?;
            if now_ms.saturating_sub(staged.staged_at_ms) > MAX_WORKER_CHANGE_SET_TTL_MS {
                return Err("The staged preview expired; stage it again.".to_string());
            }
            if staged.terminal {
                return Err("That staged worker change already reached a terminal result.".into());
            }
            if operation == WorkerChangeSetOperation::Commit && !staged.preview_admitted {
                return Err("Commit requires an admitted preview result.".to_string());
            }
            let request = WorkerChangeSetRequest::new(
                staged.request_id.clone(),
                operation,
                staged.target.clone(),
                staged.expected_generation,
                staged.items.clone(),
                IMPACT,
                RECOVERY,
                staged.arming,
                staged.digest.clone(),
                now_ms,
                now_ms.saturating_add(MAX_WORKER_CHANGE_SET_TTL_MS),
            )
            .map_err(|error| error.to_string())?;
            let unsigned = request.to_json().map_err(|error| error.to_string())?;
            let capability_target = format!("change-set:{}", staged.request_id);
            let body = crate::iac::authorize_root_mutation_body(
                &unsigned,
                WORKER_CHANGE_SET_AUTH_VERB,
                &staged.target.node_id,
                &capability_target,
            )?;
            WorkerChangeSetRequest::from_json(&body).map_err(|error| {
                format!("Authorized request failed contract admission: {error}")
            })?;
            let topic = worker_change_set_action_topic(&staged.target.node_id)
                .map_err(|error| error.to_string())?;
            let root = self
                .bus_root
                .clone()
                .ok_or_else(|| "Mesh Bus unavailable; worker actions are disabled.".to_string())?;
            Persist::open(root)
                .and_then(|persist| persist.write(&topic, Priority::Default, None, Some(&body)))
                .map_err(|error| format!("Worker action publication failed: {error}"))?;
            self.staged
                .as_mut()
                .expect("staged change remains present through publication")
                .awaiting = Some(PendingOperation {
                operation,
                published_at_ms: now_ms,
            });
            self.last_error = None;
            self.last_poll = None;
            Ok(())
        }

        fn poll_result(&mut self, persist: &Persist) {
            let Some(staged) = self.staged.as_mut() else {
                return;
            };
            let Ok(topic) = worker_change_set_result_topic(&staged.target.node_id) else {
                return;
            };
            let Ok(Some(message)) = persist.read_latest(&topic) else {
                return;
            };
            let Some(body) = message.body else {
                return;
            };
            let Ok(result) = WorkerChangeSetResult::from_json(&body) else {
                self.last_error = Some("Worker result failed closed-contract admission.".into());
                return;
            };
            let Some(awaiting) = staged.awaiting else {
                return;
            };
            if result.request_id != staged.request_id
                || result.target != staged.target
                || result.expected_generation != staged.expected_generation
                || result.operation != awaiting.operation
                || result.completed_at_ms < awaiting.published_at_ms
            {
                return;
            }
            let expected_items = staged
                .items
                .iter()
                .map(|item| item.item_id.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            if result
                .items
                .iter()
                .any(|item| !expected_items.contains(item.item_id.as_str()))
            {
                self.last_error =
                    Some("Worker result named an item outside the staged change.".into());
                return;
            }
            staged.preview_admitted = result.operation == WorkerChangeSetOperation::Preview
                && result.outcome == WorkerChangeSetOutcome::Previewed
                && result.actual_generation == staged.expected_generation;
            staged.terminal = result.operation != WorkerChangeSetOperation::Preview
                || result.outcome != WorkerChangeSetOutcome::Previewed;
            staged.awaiting = None;
            self.result = Some(result);
            self.last_error = None;
        }

        fn show(&mut self, ui: &mut egui::Ui, now_ms: u64) {
            self.poll(now_ms);
            ui.colored_label(
                Style::TEXT_DIM,
                "One live worker selection drives the tree, typed graph, inspector, history, and staged actions.",
            );
            if self.workers.is_empty() {
                ui.colored_label(
                    Style::TEXT_DIM,
                    "No current worker runtime snapshot is available on this node.",
                );
            } else if ui.available_width() < 760.0 {
                self.show_worker_tree(ui, now_ms);
                ui.separator();
                self.show_inspector(ui, now_ms);
            } else {
                ui.columns(2, |columns| {
                    self.show_worker_tree(&mut columns[0], now_ms);
                    self.show_inspector(&mut columns[1], now_ms);
                });
            }
            if let Some(error) = &self.last_error {
                ui.colored_label(Style::DANGER, error);
            }
            self.show_result(ui);
        }

        fn show_worker_tree(&mut self, ui: &mut egui::Ui, now_ms: u64) {
            ui.label(RichText::new("Workers").strong());
            let prior_worker = self.selected_worker.clone();
            for group in WorkerGroup::ALL {
                let count = self
                    .workers
                    .iter()
                    .filter(|row| row.contract.group == group)
                    .count();
                if count == 0 {
                    continue;
                }
                egui::CollapsingHeader::new(format!("{} ({count})", group.as_str()))
                    .default_open(true)
                    .show(ui, |ui| {
                        for row in self
                            .workers
                            .iter()
                            .filter(|row| row.contract.group == group)
                        {
                            let state = row.snapshot.effective_state(now_ms);
                            ui.selectable_value(
                                &mut self.selected_worker,
                                Some(row.contract.worker_id.clone()),
                                format!("{}  ·  {}", row.contract.display_name, state.as_str()),
                            );
                        }
                    });
            }
            if self.selected_worker != prior_worker {
                self.selected_action = None;
                self.reconcile_selection();
                self.staged = None;
                self.result = None;
            }
        }

        fn show_inspector(&mut self, ui: &mut egui::Ui, now_ms: u64) {
            let Some(row) = self.selected_row().cloned() else {
                ui.colored_label(Style::TEXT_DIM, "Select a worker to inspect it.");
                return;
            };
            ui.label(RichText::new(&row.contract.display_name).strong());
            ui.monospace(format!(
                "{} · {} · generation {}",
                row.contract.worker_id,
                row.snapshot.effective_state(now_ms).as_str(),
                row.snapshot.generation
            ));
            if !row.contract.description.is_empty() {
                ui.label(&row.contract.description);
            }
            ui.small(format!(
                "Restarts {} · observed {} ms · fresh until {} ms",
                row.snapshot.restart_count,
                row.snapshot.observed_at_ms,
                row.snapshot.fresh_until_ms
            ));

            egui::CollapsingHeader::new(format!("Typed graph ({})", row.snapshot.relations.len()))
                .default_open(true)
                .show(ui, |ui| {
                    if row.snapshot.relations.is_empty() {
                        ui.colored_label(Style::TEXT_DIM, "No typed relations published.");
                    }
                    for relation in &row.snapshot.relations {
                        ui.horizontal_wrapped(|ui| {
                            ui.monospace(relation_endpoint(&relation.source));
                            ui.label(format!("{:?}", relation.relation));
                            ui.monospace(relation_endpoint(&relation.target));
                            if let Some(label) = &relation.label {
                                ui.colored_label(Style::TEXT_DIM, label);
                            }
                        });
                    }
                });

            egui::CollapsingHeader::new(format!("History ({})", row.snapshot.timeline.len()))
                .default_open(true)
                .show(ui, |ui| {
                    if row.snapshot.timeline.is_empty() {
                        ui.colored_label(Style::TEXT_DIM, "No bounded history published.");
                    }
                    for event in row.snapshot.timeline.iter().rev() {
                        ui.horizontal_wrapped(|ui| {
                            ui.monospace(format!("#{}", event.sequence));
                            ui.label(format!("{:?}", event.kind));
                            ui.label(&event.summary);
                            if let Some(detail) = &event.detail {
                                ui.colored_label(Style::TEXT_DIM, detail);
                            }
                        });
                    }
                });

            ui.separator();
            ui.label(RichText::new("Action Console").strong());
            if row.contract.actions.is_empty() {
                ui.colored_label(
                    Style::TEXT_DIM,
                    "This worker publishes no admitted lifecycle actions.",
                );
                return;
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Action");
                let actions = self
                    .selected_row()
                    .map(|row| row.contract.actions.clone())
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt("workers-action-kind")
                    .selected_text(
                        self.selected_descriptor()
                            .map_or("Unavailable", |descriptor| descriptor.label.as_str()),
                    )
                    .show_ui(ui, |ui| {
                        for descriptor in actions {
                            ui.selectable_value(
                                &mut self.selected_action,
                                Some(descriptor.action),
                                descriptor.label,
                            );
                        }
                    });
            });
            self.show_stage(ui, now_ms);
        }

        fn show_stage(&mut self, ui: &mut egui::Ui, now_ms: u64) {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.selected_descriptor().is_some(),
                        egui::Button::new("Preview"),
                    )
                    .clicked()
                {
                    if let Err(error) = self.stage_preview(now_ms) {
                        self.last_error = Some(error);
                    }
                }
                let can_commit = self
                    .staged
                    .as_ref()
                    .is_some_and(|staged| staged.preview_admitted);
                if ui
                    .add_enabled(can_commit, egui::Button::new("Commit"))
                    .clicked()
                {
                    if let Err(error) =
                        self.publish_operation(WorkerChangeSetOperation::Commit, now_ms)
                    {
                        self.last_error = Some(error);
                    }
                }
                let can_cancel = self.staged.as_ref().is_some_and(|staged| !staged.terminal);
                if ui
                    .add_enabled(can_cancel, egui::Button::new("Cancel"))
                    .clicked()
                {
                    if let Err(error) =
                        self.publish_operation(WorkerChangeSetOperation::Cancel, now_ms)
                    {
                        self.last_error = Some(error);
                    }
                }
            });
            if let Some(staged) = &self.staged {
                ui.monospace(format!(
                    "{} · generation {} · {}",
                    staged.request_id, staged.expected_generation, staged.digest
                ));
            }
        }

        fn show_result(&self, ui: &mut egui::Ui) {
            let Some(result) = &self.result else {
                return;
            };
            ui.separator();
            ui.label(RichText::new(format!("Result: {:?}", result.outcome)).strong());
            if let Some(audit_id) = &result.audit_id {
                ui.monospace(format!("Audit: {audit_id}"));
            }
            if let Some(detail) = &result.detail {
                ui.label(detail);
            }
            for item in &result.items {
                ui.horizontal_wrapped(|ui| {
                    ui.monospace(&item.item_id);
                    ui.label(format!("{:?}", item.outcome));
                    if let Some(detail) = &item.detail {
                        ui.colored_label(Style::TEXT_DIM, detail);
                    }
                });
            }
        }
    }

    fn relation_endpoint(endpoint: &WorkerRelationEndpoint) -> String {
        match endpoint {
            WorkerRelationEndpoint::Worker { worker_id } => format!("worker:{worker_id}"),
            WorkerRelationEndpoint::Node { node_id } => format!("node:{node_id}"),
            WorkerRelationEndpoint::Output {
                worker_id,
                output_kind,
            } => format!("output:{worker_id}/{output_kind}"),
            WorkerRelationEndpoint::Topic { topic } => format!("topic:{topic}"),
        }
    }

    fn unix_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .unwrap_or(1)
    }

    fn local_node_id() -> String {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| std::env::var("HOSTNAME").ok())
            .unwrap_or_else(|| "unknown-node".to_string())
    }

    pub(super) fn show(ui: &mut egui::Ui) {
        let id = egui::Id::new(CONSOLE_STATE_ID);
        let mut state = ui
            .ctx()
            .data_mut(|data| data.get_temp::<ActionConsoleState>(id))
            .unwrap_or_default();
        state.show(ui, unix_ms());
        ui.ctx().data_mut(|data| data.insert_temp(id, state));
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use mackes_mesh_types::worker_runtime::{
            WorkerChangeSetItemOutcome, WorkerChangeSetItemResult, WorkerGroup, WorkerRuntimeState,
        };

        fn worker(generation: u64) -> WorkerStatusRow {
            let mut contract =
                WorkerContract::new("host-state", WorkerGroup::Observation, "Host state")
                    .expect("worker contract");
            contract.actions = vec![WorkerActionDescriptor {
                action: WorkerAction::Refresh,
                label: "Refresh".to_string(),
                arming: WorkerArmingRequirement::Confirmation,
            }];
            contract.validate().expect("action remains admitted");
            let snapshot = WorkerRuntimeSnapshot::new(
                format!("snapshot-{generation}"),
                "node-a",
                "host-state",
                WorkerGroup::Observation,
                generation,
                WorkerRuntimeState::Running,
                1_000,
                2_000,
                2_000,
                12_000,
            )
            .expect("snapshot");
            WorkerStatusRow { contract, snapshot }
        }

        fn state(root: PathBuf) -> ActionConsoleState {
            ActionConsoleState {
                bus_root: Some(root),
                node_id: "node-a".to_string(),
                workers: vec![worker(7)],
                selected_worker: Some("host-state".to_string()),
                selected_action: Some(WorkerAction::Refresh),
                staged: None,
                result: None,
                last_error: None,
                last_poll: None,
            }
        }

        fn publish_result(
            persist: &Persist,
            staged: &StagedChange,
            operation: WorkerChangeSetOperation,
            outcome: WorkerChangeSetOutcome,
            completed_at_ms: u64,
        ) {
            let result = WorkerChangeSetResult {
                schema_version: WORKER_RUNTIME_SCHEMA_VERSION,
                request_id: staged.request_id.clone(),
                operation,
                outcome,
                target: staged.target.clone(),
                expected_generation: staged.expected_generation,
                actual_generation: staged.expected_generation,
                items: vec![],
                audit_id: Some(format!("audit-{:?}", operation).to_ascii_lowercase()),
                completed_at_ms,
                detail: None,
            };
            persist
                .write(
                    "state/workers/change-set/node-a",
                    Priority::Default,
                    None,
                    Some(&result.to_json().expect("result body")),
                )
                .expect("publish result");
        }

        #[test]
        fn preview_publication_is_typed_authenticated_and_generation_bound() {
            let temp = tempfile::tempdir().expect("bus root");
            let mut console = state(temp.path().to_path_buf());
            console.stage_preview(3_000).expect("publish preview");

            let persist = Persist::open(temp.path().to_path_buf()).expect("open bus");
            let message = persist
                .read_latest("action/workers/change-set/node-a")
                .expect("read request")
                .expect("published request");
            let request =
                WorkerChangeSetRequest::from_json(message.body.as_deref().expect("request body"))
                    .expect("admitted authorized request");
            assert_eq!(request.operation, WorkerChangeSetOperation::Preview);
            assert_eq!(request.expected_generation, 7);
            assert_eq!(request.items.len(), 1);
            assert_eq!(request.items[0].action, WorkerAction::Refresh);
            assert!(
                request
                    .armed_token
                    .as_deref()
                    .is_some_and(|token| !token.is_empty()),
                "the existing action authority must mint the exact-body capability"
            );
            assert!(!message.body.as_deref().unwrap().contains("command"));
            assert!(!message.body.as_deref().unwrap().contains("path"));
        }

        #[test]
        fn generation_change_discards_the_staged_preview_before_commit() {
            let temp = tempfile::tempdir().expect("bus root");
            let mut console = state(temp.path().to_path_buf());
            console.stage_preview(3_000).expect("publish preview");
            console.workers = vec![worker(8)];
            console.reconcile_generation();
            assert!(console.staged.is_none());
            assert!(console.result.is_none());
            assert!(console
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("generation changed")));
        }

        #[test]
        fn admitted_partial_result_preserves_audit_and_per_item_failure() {
            let temp = tempfile::tempdir().expect("bus root");
            let mut console = state(temp.path().to_path_buf());
            console.stage_preview(3_000).expect("publish preview");
            let persist = Persist::open(temp.path().to_path_buf()).expect("open bus");
            let staged = console.staged.as_ref().expect("staged").clone();
            publish_result(
                &persist,
                &staged,
                WorkerChangeSetOperation::Preview,
                WorkerChangeSetOutcome::Previewed,
                3_100,
            );
            console.poll_result(&persist);
            console
                .publish_operation(WorkerChangeSetOperation::Commit, 3_200)
                .expect("publish commit");
            let staged = console.staged.as_ref().expect("staged").clone();
            let result = WorkerChangeSetResult {
                schema_version: WORKER_RUNTIME_SCHEMA_VERSION,
                request_id: staged.request_id,
                operation: WorkerChangeSetOperation::Commit,
                outcome: WorkerChangeSetOutcome::Partial,
                target: staged.target,
                expected_generation: staged.expected_generation,
                actual_generation: staged.expected_generation + 1,
                items: vec![WorkerChangeSetItemResult {
                    item_id: staged.items[0].item_id.clone(),
                    outcome: WorkerChangeSetItemOutcome::Failed,
                    detail: Some("worker refused the transition".to_string()),
                }],
                audit_id: Some("audit-workers-1".to_string()),
                completed_at_ms: 3_500,
                detail: Some("one typed action failed".to_string()),
            };
            result.validate().expect("typed partial result");
            persist
                .write(
                    "state/workers/change-set/node-a",
                    Priority::Default,
                    None,
                    Some(&result.to_json().expect("result body")),
                )
                .expect("publish result");
            console.poll_result(&persist);
            let admitted = console.result.as_ref().expect("projected result");
            assert_eq!(admitted.outcome, WorkerChangeSetOutcome::Partial);
            assert_eq!(admitted.audit_id.as_deref(), Some("audit-workers-1"));
            assert_eq!(
                admitted.items[0].outcome,
                WorkerChangeSetItemOutcome::Failed
            );
        }

        #[test]
        fn delayed_preview_result_cannot_answer_commit_or_cancel() {
            let temp = tempfile::tempdir().expect("bus root");
            let persist = Persist::open(temp.path().to_path_buf()).expect("open bus");
            let mut console = state(temp.path().to_path_buf());
            console.stage_preview(3_000).expect("publish preview");
            let staged = console.staged.as_ref().expect("staged").clone();
            publish_result(
                &persist,
                &staged,
                WorkerChangeSetOperation::Preview,
                WorkerChangeSetOutcome::Previewed,
                3_100,
            );
            console.poll_result(&persist);
            assert!(console
                .staged
                .as_ref()
                .is_some_and(|staged| staged.preview_admitted));

            console
                .publish_operation(WorkerChangeSetOperation::Commit, 3_200)
                .expect("publish commit");
            publish_result(
                &persist,
                &staged,
                WorkerChangeSetOperation::Preview,
                WorkerChangeSetOutcome::Previewed,
                3_300,
            );
            console.poll_result(&persist);

            let staged = console.staged.as_ref().expect("commit remains pending");
            assert_eq!(
                staged.awaiting,
                Some(PendingOperation {
                    operation: WorkerChangeSetOperation::Commit,
                    published_at_ms: 3_200,
                })
            );
            assert!(!staged.terminal);
            assert_eq!(
                console.result.as_ref().map(|result| result.operation),
                Some(WorkerChangeSetOperation::Preview),
                "the delayed preview must not masquerade as the commit result"
            );
        }

        #[test]
        fn inspector_selection_is_not_limited_to_actionable_workers() {
            let temp = tempfile::tempdir().expect("bus root");
            let mut observer = worker(7);
            observer.contract.worker_id = "metrics-collector".to_string();
            observer.contract.display_name = "Metrics collector".to_string();
            observer.contract.actions.clear();
            observer.snapshot.worker_id = observer.contract.worker_id.clone();
            observer.contract.validate().expect("observer contract");
            observer.snapshot.validate().expect("observer snapshot");

            let mut console = state(temp.path().to_path_buf());
            console.workers = vec![observer, worker(7)];
            console.selected_worker = Some("metrics-collector".to_string());
            console.selected_action = Some(WorkerAction::Refresh);
            console.reconcile_selection();

            assert_eq!(
                console
                    .selected_row()
                    .map(|row| row.contract.worker_id.as_str()),
                Some("metrics-collector")
            );
            assert_eq!(console.selected_action, None);
            assert!(console.stage_preview(3_000).is_err());
        }
    }
}
