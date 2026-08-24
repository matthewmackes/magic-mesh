//! Centered System and Mesh Health modal.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::health::{
    action_result_topic, format_health_duration_ms, HealthAction, HealthActionOutcome,
    HealthActionRequest, HealthActionResult, HealthComponent, HealthCondition, HealthScope,
    HealthSeverity, NodeGrade, SystemMeshHealthSnapshot, ACTION_TOPIC, HEALTH_SCHEMA_VERSION,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_egui::{egui, Style, TypographyRole};

use crate::construct::ConstructChrome;

const SEATS: [(&str, &[&str]); 5] = [
    ("Seat 15", &["seat15", "seat-15", "basement"]),
    ("Dell", &["dell"]),
    ("Eagle", &["eagle", "t470"]),
    ("T480", &["t480"]),
    ("Surface", &["surface"]),
];
const MESH_SELECTION: &str = "__mesh_wide__";
const HISTORY_PAGE_SIZE: usize = 8;
const HISTORY_MAX_IDENTITIES: usize = 256;
const HISTORY_WINDOW_MS: u64 = 24 * 60 * 60 * 1_000;
const ACTION_ERROR_STATE_ID: &str = "health-action-publication-error";
const SUPPORT_EXPORT_STATE_ID: &str = "health-support-export-outcome";
const SUPPORT_BUNDLE_SCHEMA: &str = "mde.health.support-bundle.v1";
const SUPPORT_BUNDLE_MAX_BYTES: usize = 64 * 1_024;
const SUPPORT_BUNDLE_MAX_NODES: usize = 32;
const SUPPORT_BUNDLE_MAX_ACTIVE: usize = 32;
const SUPPORT_BUNDLE_MAX_RESOLVED: usize = 32;
const SUPPORT_BUNDLE_MAX_FACTS: usize = 8;
const SUPPORT_BUNDLE_MAX_TEXT_BYTES: usize = 192;
const MAX_REDACTION_SCAN_BYTES: usize = 16 * 1024;
const SUPPORT_BUNDLE_MAX_FILENAME_BYTES: usize = 128;
const HISTORY_FILTER_STATE_ID: &str = "health-history-severity-filter";
const HISTORY_COMPONENT_FILTER_STATE_ID: &str = "health-history-component-filter";
const HISTORY_SOURCE_FILTER_STATE_ID: &str = "health-history-source-filter";
const HISTORY_PROVIDER_FILTER_STATE_ID: &str = "health-history-provider-filter";
const HISTORY_PAGE_STATE_ID: &str = "health-history-page";
const HISTORY_SELECTION_STATE_ID: &str = "health-history-selection";
const HISTORY_ORIGIN_FILTER_LIMIT: usize = 32;
const SNAPSHOT_AUTHORITY_STATE_ID: &str = "health-snapshot-authority";
const ACTION_PROGRESS_STATE_ID: &str = "health-action-result-progress";
const ACTION_RESULT_TAIL_BOUND: usize = 8;
const ACTION_RESULT_POLL_INTERVAL: Duration = Duration::from_millis(500);
static ACTION_REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum HistorySeverityFilter {
    #[default]
    All,
    Warning,
    Critical,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HistoryPageState {
    node: String,
    filter: HistorySeverityFilter,
    component: HistoryComponentFilter,
    source: Option<String>,
    provider: Option<String>,
    page: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HistorySelection {
    node: String,
    incident_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SupportExportAuthority {
    snapshot_generation: u64,
    snapshot_generated_at_ms: u64,
    node_scope: String,
    severity: HistorySeverityFilter,
    component: HistoryComponentFilter,
    source: Option<String>,
    provider: Option<String>,
    selected_incident_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum HistoryComponentFilter {
    #[default]
    All,
    Component(HealthComponent),
}

impl HistorySeverityFilter {
    const ALL: [Self; 3] = [Self::All, Self::Warning, Self::Critical];

    const fn label(self) -> &'static str {
        match self {
            Self::All => "All severities",
            Self::Warning => "Warning",
            Self::Critical => "Critical",
        }
    }

    const fn admits(self, severity: HealthSeverity) -> bool {
        match self {
            Self::All => true,
            Self::Warning => matches!(severity, HealthSeverity::Warning),
            Self::Critical => matches!(severity, HealthSeverity::Critical),
        }
    }
}

impl HistoryComponentFilter {
    const ALL: [Self; 8] = [
        Self::All,
        Self::Component(HealthComponent::System),
        Self::Component(HealthComponent::Mesh),
        Self::Component(HealthComponent::Resources),
        Self::Component(HealthComponent::Devices),
        Self::Component(HealthComponent::Audio),
        Self::Component(HealthComponent::Firmware),
        Self::Component(HealthComponent::Evidence),
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::All => "All components",
            Self::Component(HealthComponent::System) => "System",
            Self::Component(HealthComponent::Mesh) => "Mesh",
            Self::Component(HealthComponent::Resources) => "Resources",
            Self::Component(HealthComponent::Devices) => "Devices",
            Self::Component(HealthComponent::Audio) => "Audio",
            Self::Component(HealthComponent::Firmware) => "Firmware",
            Self::Component(HealthComponent::Evidence) => "Evidence",
        }
    }

    fn admits(self, component: HealthComponent) -> bool {
        matches!(self, Self::All)
            || matches!(self, Self::Component(expected) if expected == component)
    }
}

pub(crate) fn mount(
    ctx: &egui::Context,
    chrome: &mut ConstructChrome,
    snapshot: Option<&SystemMeshHealthSnapshot>,
) {
    if !chrome.health_modal_open {
        chrome.health_pending_action = None;
        clear_action_error(ctx);
        clear_support_export_outcome(ctx);
        return;
    }
    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        chrome.health_modal_open = false;
        chrome.health_pending_action = None;
        clear_action_error(ctx);
        clear_support_export_outcome(ctx);
        return;
    }

    let screen = ctx.screen_rect();
    let width = (screen.width() - Style::SP_L * 2.0).clamp(360.0, 1_080.0);
    let height = (screen.height() - Style::SP_L * 2.0).clamp(420.0, 760.0);
    let id = egui::Id::new("system-and-mesh-health-modal");
    let shown = egui::Modal::new(id)
        .backdrop_color(Style::SCRIM_THIN)
        .show(ctx, |ui| {
            ui.set_min_size(egui::vec2(width, height));
            ui.set_max_size(egui::vec2(width, height));
            show(ui, chrome, snapshot);
        });
    if shown.should_close() {
        chrome.health_modal_open = false;
        chrome.health_pending_action = None;
        clear_action_error(ctx);
        clear_support_export_outcome(ctx);
    }
}

fn show(
    ui: &mut egui::Ui,
    chrome: &mut ConstructChrome,
    snapshot: Option<&SystemMeshHealthSnapshot>,
) {
    let admitted_snapshot = admit_modal_snapshot(ui.ctx(), snapshot);
    let snapshot = admitted_snapshot.as_ref();
    stabilize_selection(chrome, snapshot);
    let active = active_condition_count(snapshot);
    let issue_text = format!(
        "{active} active {}",
        if active == 1 { "issue" } else { "issues" }
    );
    let compact_header = ui.available_width() < 700.0;
    ui.horizontal(|ui| {
        ui.heading("System and Mesh Health");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Close").clicked() {
                chrome.health_modal_open = false;
                chrome.health_pending_action = None;
                clear_action_error(ui.ctx());
            }
            if !compact_header {
                if let Some(snapshot) = snapshot {
                    ui.label(format!("Overall {}", snapshot.mesh_summary.grade.as_str()));
                }
            }
        });
    });
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(issue_text).color(if active == 0 {
            Style::SUPPORT_SUCCESS
        } else {
            Style::SUPPORT_WARNING
        }));
        if compact_header {
            if let Some(snapshot) = snapshot {
                ui.label(format!("Overall {}", snapshot.mesh_summary.grade.as_str()));
            }
        }
        let export = ui.add_enabled(
            snapshot.is_some(),
            egui::Button::new("Export redacted support bundle"),
        );
        if export.clicked() {
            let snapshot = snapshot.expect("enabled export requires a current snapshot");
            apply_support_export_result(
                ui.ctx(),
                capture_support_export_authority(ui.ctx(), chrome, snapshot).and_then(
                    |authority| export_support_bundle(ui.ctx(), chrome, snapshot, &authority),
                ),
            );
        }
    });
    if let Some(outcome) = support_export_outcome(ui.ctx()) {
        match outcome {
            SupportExportOutcome::Exported(path) => {
                ui.colored_label(
                    Style::SUPPORT_SUCCESS,
                    format!("Support bundle exported to {}", path.display()),
                );
            }
            SupportExportOutcome::Failed(error) => {
                ui.colored_label(
                    Style::SUPPORT_ERROR,
                    format!("Support bundle export failed: {error}"),
                );
            }
        }
    }
    ui.separator();
    if let Some(error) = action_error(ui.ctx()) {
        ui.colored_label(Style::SUPPORT_ERROR, error.presentable());
        ui.add_space(Style::SP_XS);
    }
    if let Some(progress) = refresh_action_progress(ui.ctx(), snapshot) {
        render_action_progress(ui, &progress);
        ui.add_space(Style::SP_XS);
    }

    let narrow = ui.available_width() < 760.0;
    if narrow {
        egui::ScrollArea::vertical()
            .id_salt("health-content-narrow")
            .show(ui, |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("health-matrix-narrow")
                    .show(ui, |ui| {
                        matrix(ui, chrome, snapshot);
                    });
                ui.separator();
                detail(ui, chrome, snapshot);
            });
    } else {
        let matrix_width = (ui.available_width() * 0.56).clamp(520.0, 620.0);
        ui.horizontal_top(|ui| {
            let height = ui.available_height();
            ui.allocate_ui_with_layout(
                egui::vec2(matrix_width, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::both()
                        .id_salt("health-matrix")
                        .show(ui, |ui| {
                            matrix(ui, chrome, snapshot);
                        });
                },
            );
            ui.separator();
            let detail_width = ui.available_width();
            ui.allocate_ui_with_layout(
                egui::vec2(detail_width, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("health-detail")
                        .show(ui, |ui| {
                            detail(ui, chrome, snapshot);
                        });
                },
            );
        });
    }
}

/// Retain the newest snapshot rendered by this shell so a delayed or replaced
/// projection cannot regain modal authority merely by carrying fresh wall-clock
/// timestamps. Equal generations are immutable; forward generations must also
/// advance publication time. A rejected live update leaves the last admitted
/// snapshot visible until its own freshness expires.
fn admit_modal_snapshot(
    ctx: &egui::Context,
    candidate: Option<&SystemMeshHealthSnapshot>,
) -> Option<SystemMeshHealthSnapshot> {
    let candidate = candidate?;
    let id = egui::Id::new(SNAPSHOT_AUTHORITY_STATE_ID);
    let retained = ctx.data(|data| data.get_temp::<SystemMeshHealthSnapshot>(id));
    let admitted = match retained {
        None => candidate.clone(),
        Some(retained) if retained == *candidate => retained,
        Some(retained)
            if retained.observer == candidate.observer
                && retained.generation < candidate.generation
                && retained.generated_at_ms < candidate.generated_at_ms =>
        {
            candidate.clone()
        }
        Some(retained) => return Some(retained),
    };
    ctx.data_mut(|data| data.insert_temp(id, admitted.clone()));
    Some(admitted)
}

/// Establish the default detail target once, then preserve it across live
/// snapshot reorder/removal.  Selection belongs to the operator-facing modal
/// model; deriving it from `current_node_grades.first()` on every paint lets an
/// asynchronously refreshed roster silently move the evidence pane.
fn stabilize_selection(chrome: &mut ConstructChrome, snapshot: Option<&SystemMeshHealthSnapshot>) {
    if chrome.health_selected_node.is_none() {
        chrome.health_selected_node = snapshot.and_then(|snapshot| {
            snapshot
                .current_node_grades
                .first()
                .map(|grade| grade.node.clone())
        });
    }
}

fn matrix(
    ui: &mut egui::Ui,
    chrome: &mut ConstructChrome,
    snapshot: Option<&SystemMeshHealthSnapshot>,
) {
    egui::Grid::new("system-mesh-health-matrix")
        .striped(true)
        .min_col_width(58.0)
        .show(ui, |ui| {
            for heading in [
                "Node",
                "Grade",
                "System",
                "Mesh",
                "Resources",
                "Devices",
                "Freshness",
            ] {
                ui.strong(heading);
            }
            ui.end_row();
            for (label, aliases) in SEATS {
                let grade = snapshot.and_then(|snapshot| find_grade(snapshot, aliases));
                let node = grade.map(|grade| grade.node.as_str());
                let selected =
                    node.is_some_and(|node| chrome.health_selected_node.as_deref() == Some(node));
                if ui.selectable_label(selected, label).clicked() {
                    chrome.health_selected_node = node.map(str::to_string);
                }
                ui.label(grade.map_or("—", |grade| grade.grade.as_str()));
                if node.is_some() {
                    status_cell(
                        ui,
                        snapshot,
                        node,
                        &[
                            HealthComponent::System,
                            HealthComponent::Audio,
                            HealthComponent::Firmware,
                        ],
                    );
                    status_cell(ui, snapshot, node, &[HealthComponent::Mesh]);
                    status_cell(ui, snapshot, node, &[HealthComponent::Resources]);
                    status_cell(ui, snapshot, node, &[HealthComponent::Devices]);
                } else {
                    for _ in 0..4 {
                        ui.colored_label(Style::TEXT_DIM, "—");
                    }
                }
                let fresh =
                    grade.is_some() && snapshot.is_some_and(|snapshot| snapshot.is_fresh(now_ms()));
                ui.colored_label(
                    if fresh {
                        Style::SUPPORT_SUCCESS
                    } else {
                        Style::TEXT_DIM
                    },
                    if fresh { "Current" } else { "Stale" },
                );
                ui.end_row();
            }
            if ui
                .selectable_label(
                    chrome.health_selected_node.as_deref() == Some(MESH_SELECTION),
                    "Mesh-wide",
                )
                .clicked()
            {
                chrome.health_selected_node = Some(MESH_SELECTION.into());
            }
            ui.label(snapshot.map_or("—", |snapshot| snapshot.mesh_summary.grade.as_str()));
            ui.label("—");
            status_cell(
                ui,
                snapshot,
                None,
                &[HealthComponent::Mesh, HealthComponent::Evidence],
            );
            ui.label("—");
            ui.label("—");
            ui.label(snapshot.map_or("Stale", |snapshot| {
                if snapshot.is_fresh(now_ms()) {
                    "Current"
                } else {
                    "Stale"
                }
            }));
            ui.end_row();
        });
}

fn find_grade<'a>(
    snapshot: &'a SystemMeshHealthSnapshot,
    aliases: &[&str],
) -> Option<&'a NodeGrade> {
    snapshot.current_node_grades.iter().find(|grade| {
        let name = grade.node.to_ascii_lowercase();
        aliases.iter().any(|alias| name.contains(alias))
    })
}

fn status_cell(
    ui: &mut egui::Ui,
    snapshot: Option<&SystemMeshHealthSnapshot>,
    node: Option<&str>,
    components: &[HealthComponent],
) {
    let (text, color) = match status_cell_state(snapshot, node, components, now_ms()) {
        StatusCellState::Critical => ("Critical", Style::SUPPORT_ERROR),
        StatusCellState::Warning => ("Warning", Style::SUPPORT_WARNING),
        StatusCellState::Ok => ("OK", Style::SUPPORT_SUCCESS),
        StatusCellState::Stale => ("Stale", Style::TEXT_DIM),
        StatusCellState::Unavailable => ("—", Style::TEXT_DIM),
    };
    ui.colored_label(color, text);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusCellState {
    Critical,
    Warning,
    Ok,
    Stale,
    Unavailable,
}

/// Derive a cell only from current provenance. Expected-absence observations
/// are informational, so a fresh one remains non-outage; once the containing
/// projection expires, however, its lack of an outage condition is no longer
/// evidence for `OK`.
fn status_cell_state(
    snapshot: Option<&SystemMeshHealthSnapshot>,
    node: Option<&str>,
    components: &[HealthComponent],
    now_ms: u64,
) -> StatusCellState {
    let Some(snapshot) = snapshot else {
        return StatusCellState::Unavailable;
    };
    if !snapshot.is_fresh(now_ms) {
        return StatusCellState::Stale;
    }
    let worst = snapshot
        .active_conditions
        .iter()
        .filter(|condition| {
            condition.is_active()
                && condition.requirement == mackes_mesh_types::health::RequirementClass::Required
                && components.contains(&condition.component)
                && match (&condition.scope, node) {
                    (HealthScope::Node { node: target }, Some(node)) => target == node,
                    (HealthScope::Mesh, None) => true,
                    _ => false,
                }
        })
        .map(|condition| condition.severity)
        .max();
    match worst {
        Some(HealthSeverity::Critical) => StatusCellState::Critical,
        Some(HealthSeverity::Warning) => StatusCellState::Warning,
        None => StatusCellState::Ok,
    }
}

fn active_condition_count(snapshot: Option<&SystemMeshHealthSnapshot>) -> usize {
    snapshot.map_or(0, |snapshot| {
        snapshot
            .active_conditions
            .iter()
            .filter(|condition| {
                condition.is_active()
                    && condition.requirement
                        == mackes_mesh_types::health::RequirementClass::Required
            })
            .count()
    })
}

fn detail(
    ui: &mut egui::Ui,
    chrome: &mut ConstructChrome,
    snapshot: Option<&SystemMeshHealthSnapshot>,
) {
    let selected = chrome.health_selected_node.clone();
    let Some(node) = selected else {
        ui.heading("No current node evidence");
        ui.label("The health provider has not published a current seat row.");
        return;
    };
    if node == MESH_SELECTION {
        ui.heading("Mesh-wide");
        if let Some(snapshot) = snapshot {
            ui.label(format!(
                "Grade {} · {} of {} node publications current · {} lighthouse(s) reachable",
                snapshot.mesh_summary.grade.as_str(),
                snapshot.mesh_summary.fresh_nodes,
                snapshot.mesh_summary.canonical_nodes,
                snapshot.mesh_summary.reachable_lighthouses,
            ));
            let conditions: Vec<_> = snapshot
                .active_conditions
                .iter()
                .filter(|condition| {
                    condition.scope == HealthScope::Mesh
                        && condition.requirement
                            == mackes_mesh_types::health::RequirementClass::Required
                })
                .collect();
            if conditions.is_empty() {
                ui.colored_label(Style::SUPPORT_SUCCESS, "0 active issues");
            }
            for condition in conditions {
                condition_card(ui, chrome, snapshot, condition, MESH_SELECTION);
            }
        } else {
            ui.label("The health provider has not published a current mesh summary.");
        }
        return;
    }
    ui.heading(redact_support_text(&node));
    let grade = snapshot.and_then(|snapshot| {
        snapshot
            .current_node_grades
            .iter()
            .find(|grade| grade.node == node)
    });
    if let Some(grade) = grade {
        ui.label(format!(
            "Grade {} · capability {}",
            grade.grade.as_str(),
            grade.capability_score
        ));
        ui.label(format!(
            "Evaluated {}",
            format_timestamp(grade.evaluated_at_ms)
        ));
    }
    ui.add_space(Style::SP_S);
    let conditions: Vec<_> = snapshot
        .into_iter()
        .flat_map(|snapshot| snapshot.active_conditions.iter())
        .filter(|condition| {
            condition.requirement == mackes_mesh_types::health::RequirementClass::Required
                && matches!(&condition.scope, HealthScope::Node { node: target } if target == &node)
        })
        .collect();
    ui.strong("Active Issues");
    if conditions.is_empty() {
        ui.colored_label(Style::SUPPORT_SUCCESS, "0 active issues");
        ui.label("All required providers are current and within policy.");
    }
    for condition in conditions {
        condition_card(
            ui,
            chrome,
            snapshot.expect("condition requires snapshot"),
            condition,
            &node,
        );
    }

    let information: Vec<_> = snapshot
        .into_iter()
        .flat_map(|snapshot| snapshot.active_conditions.iter())
        .filter(|condition| {
            condition.requirement != mackes_mesh_types::health::RequirementClass::Required
                && matches!(&condition.scope, HealthScope::Node { node: target } if target == &node)
        })
        .collect();
    if !information.is_empty() {
        ui.separator();
        ui.strong("Information");
        for condition in information {
            ui.label(redact_support_text(&condition.evidence.summary));
        }
    }

    if let Some(snapshot) = snapshot {
        let all_resolved = recurrence_history(
            &snapshot.resolved_conditions,
            &node,
            snapshot.generated_at_ms,
        );
        if !all_resolved.is_empty() {
            ui.separator();
            ui.strong("Recent History");
            let mut filter = history_filter(ui.ctx());
            egui::ComboBox::from_id_salt("health-history-severity-filter-combo")
                .selected_text(filter.label())
                .show_ui(ui, |ui| {
                    for choice in HistorySeverityFilter::ALL {
                        ui.selectable_value(&mut filter, choice, choice.label());
                    }
                });
            set_history_filter(ui.ctx(), filter);
            let mut component = history_component_filter(ui.ctx());
            egui::ComboBox::from_id_salt("health-history-component-filter-combo")
                .selected_text(component.label())
                .show_ui(ui, |ui| {
                    for choice in HistoryComponentFilter::ALL {
                        ui.selectable_value(&mut component, choice, choice.label());
                    }
                });
            set_history_component_filter(ui.ctx(), component);
            let sources = history_origin_choices(
                &snapshot.resolved_conditions,
                &node,
                snapshot.generated_at_ms,
                |condition| &condition.source,
            );
            let providers = history_origin_choices(
                &snapshot.resolved_conditions,
                &node,
                snapshot.generated_at_ms,
                |condition| &condition.evidence.provider,
            );
            let mut source = history_source_filter(ui.ctx());
            if source
                .as_ref()
                .is_some_and(|selected| !sources.contains(selected))
            {
                source = None;
            }
            egui::ComboBox::from_id_salt("health-history-source-filter-combo")
                .selected_text(
                    source
                        .as_deref()
                        .map_or("All sources".into(), redact_support_text),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut source, None, "All sources");
                    for choice in &sources {
                        ui.selectable_value(
                            &mut source,
                            Some(choice.clone()),
                            redact_support_text(choice),
                        );
                    }
                });
            set_history_source_filter(ui.ctx(), source.clone());
            let mut provider = history_provider_filter(ui.ctx());
            if provider
                .as_ref()
                .is_some_and(|selected| !providers.contains(selected))
            {
                provider = None;
            }
            egui::ComboBox::from_id_salt("health-history-provider-filter-combo")
                .selected_text(
                    provider
                        .as_deref()
                        .map_or("All providers".into(), redact_support_text),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut provider, None, "All providers");
                    for choice in &providers {
                        ui.selectable_value(
                            &mut provider,
                            Some(choice.clone()),
                            redact_support_text(choice),
                        );
                    }
                });
            set_history_provider_filter(ui.ctx(), provider.clone());
            let mut page_state = history_page_state(
                ui.ctx(),
                &node,
                filter,
                component,
                source.as_deref(),
                provider.as_deref(),
            );
            let mut history = paged_recurrence_history(
                &snapshot.resolved_conditions,
                &node,
                snapshot.generated_at_ms,
                filter,
                component,
                source.as_deref(),
                provider.as_deref(),
                page_state.page,
            );
            let page_count = history.total.div_ceil(HISTORY_PAGE_SIZE).max(1);
            if page_state.page >= page_count {
                page_state.page = page_count - 1;
                history = paged_recurrence_history(
                    &snapshot.resolved_conditions,
                    &node,
                    snapshot.generated_at_ms,
                    filter,
                    component,
                    source.as_deref(),
                    provider.as_deref(),
                    page_state.page,
                );
            }
            if history.rows.is_empty() {
                ui.colored_label(Style::TEXT_DIM, "No history matches this filter.");
            }
            let selected_incident = history_selection(ui.ctx(), &node);
            for recurrence in history.rows {
                let condition = recurrence.condition;
                let recurrence_copy = if recurrence.occurrences == 1 {
                    "once".to_string()
                } else {
                    format!("{} times", recurrence.occurrences)
                };
                let row_text = format!(
                    "{} · occurred {recurrence_copy} · resolved {} · duration {}",
                    redact_support_text(&condition.evidence.summary),
                    condition
                        .resolved_at_ms
                        .map_or_else(|| "—".into(), format_timestamp),
                    resolution_duration_ms(condition)
                        .map_or_else(|| "unknown".to_string(), format_health_duration_ms),
                );
                if ui
                    .selectable_label(
                        selected_incident.as_deref() == Some(condition.id.as_str()),
                        row_text,
                    )
                    .clicked()
                {
                    set_history_selection(ui.ctx(), &node, &condition.id);
                }
            }
            if history.total > HISTORY_PAGE_SIZE {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(page_state.page > 0, egui::Button::new("Previous").small())
                        .clicked()
                    {
                        page_state.page -= 1;
                    }
                    ui.label(format!("Page {} of {page_count}", page_state.page + 1));
                    if ui
                        .add_enabled(
                            page_state.page + 1 < page_count,
                            egui::Button::new("Next").small(),
                        )
                        .clicked()
                    {
                        page_state.page += 1;
                    }
                });
            }
            set_history_page_state(ui.ctx(), page_state);
            if let Some(incident_id) = history_selection(ui.ctx(), &node) {
                ui.add_space(Style::SP_S);
                ui.strong("Resolved issue detail");
                if let Some(condition) = selected_history_condition(
                    &snapshot.resolved_conditions,
                    &node,
                    snapshot.generated_at_ms,
                    &incident_id,
                ) {
                    resolved_condition_detail(ui, condition);
                } else {
                    ui.colored_label(
                        Style::TEXT_DIM,
                        "This resolved issue is no longer in the retained history.",
                    );
                }
            }
        }
    }
}

fn resolved_condition_detail(ui: &mut egui::Ui, condition: &HealthCondition) {
    ui.label(redact_support_text(&condition.evidence.summary));
    ui.label(format!(
        "{:?} · {:?} · source {} · provider {}",
        condition.severity,
        condition.component,
        redact_support_text(&condition.source),
        redact_support_text(&condition.evidence.provider),
    ));
    ui.label(format!(
        "Observed {} · resolved {} · duration {}",
        format_timestamp(condition.evidence.observed_at_ms),
        condition
            .resolved_at_ms
            .map_or_else(|| "—".into(), format_timestamp),
        resolution_duration_ms(condition)
            .map_or_else(|| "unknown".to_string(), format_health_duration_ms),
    ));
    for (key, value) in condition
        .evidence
        .facts
        .iter()
        .filter(|(key, _)| !credential_shaped(key))
        .take(SUPPORT_BUNDLE_MAX_FACTS)
    {
        ui.label(format!(
            "{}: {}",
            redact_support_text(key),
            redact_support_text(value)
        ));
    }
}

fn condition_card(
    ui: &mut egui::Ui,
    chrome: &mut ConstructChrome,
    snapshot: &SystemMeshHealthSnapshot,
    condition: &HealthCondition,
    node: &str,
) {
    let tone = if condition.severity == HealthSeverity::Critical {
        Style::SUPPORT_ERROR
    } else {
        Style::SUPPORT_WARNING
    };
    mde_egui::card().show(ui, |ui| {
        ui.colored_label(tone, format!("{:?}", condition.severity));
        ui.strong(redact_support_text(&condition.evidence.summary));
        ui.label(Style::typography_text(
            format!(
                "{} · observed {}",
                redact_support_text(&condition.evidence.provider),
                format_timestamp(condition.last_observed_ms)
            ),
            TypographyRole::Caption,
        ));
        let local = crate::explorer::local_hostname();
        let actionable_here =
            matches!(&condition.scope, HealthScope::Node { node: target } if target == &local);
        let recovery_in_flight = action_progress_is_pending(ui.ctx(), snapshot);
        ui.horizontal_wrapped(|ui| {
            if actionable_here
                && ui
                    .add_enabled(
                        !recovery_in_flight,
                        egui::Button::new("Acknowledge").small(),
                    )
                    .clicked()
            {
                publish_action_for_ui(
                    ui.ctx(),
                    chrome,
                    snapshot,
                    condition,
                    node,
                    HealthAction::Acknowledge,
                    false,
                );
            }
            if actionable_here
                && ui
                    .add_enabled(
                        !recovery_in_flight,
                        egui::Button::new("Snooze 1 hour").small(),
                    )
                    .clicked()
            {
                publish_action_for_ui(
                    ui.ctx(),
                    chrome,
                    snapshot,
                    condition,
                    node,
                    HealthAction::SnoozeOneHour,
                    false,
                );
            }
            if let Some(route) = condition
                .remediation
                .iter()
                .find_map(|action| action.workspace_route.as_deref())
            {
                if let Some(key) = device_route_key(route) {
                    if ui.small_button("Open device inventory").clicked() {
                        chrome.health_inventory_target = Some((node.to_string(), key.to_string()));
                    }
                }
            }
        });
        if !actionable_here && matches!(condition.scope, HealthScope::Node { .. }) {
            ui.label(Style::typography_text(
                "Guided actions are available when this modal is opened on the target seat.",
                TypographyRole::Caption,
            ));
        }
        for action in &condition.remediation {
            if !actionable_here {
                continue;
            }
            ui.separator();
            ui.label(redact_support_text(&action.impact));
            let label = action_label(action.action);
            if action.confirmation_required {
                if ui
                    .add_enabled(!recovery_in_flight, egui::Button::new(label))
                    .clicked()
                {
                    chrome.health_pending_action = Some((condition.id.clone(), action.action));
                }
            } else if ui
                .add_enabled(!recovery_in_flight, egui::Button::new(label))
                .clicked()
            {
                publish_action_for_ui(
                    ui.ctx(),
                    chrome,
                    snapshot,
                    condition,
                    node,
                    action.action,
                    false,
                );
            }
            if chrome
                .health_pending_action
                .as_ref()
                .is_some_and(|pending| pending.0 == condition.id && pending.1 == action.action)
            {
                ui.colored_label(
                    Style::SUPPORT_WARNING,
                    "Confirm this guided action after reviewing its expected impact.",
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!recovery_in_flight, egui::Button::new("Confirm action"))
                        .clicked()
                    {
                        publish_action_for_ui(
                            ui.ctx(),
                            chrome,
                            snapshot,
                            condition,
                            node,
                            action.action,
                            true,
                        );
                    }
                    if ui.button("Cancel").clicked() {
                        chrome.health_pending_action = None;
                    }
                });
            }
        }
    });
    ui.add_space(Style::SP_XS);
}

fn device_route_key(route: &str) -> Option<&str> {
    route
        .strip_prefix("device-manager?device=")
        .filter(|key| !key.is_empty())
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SupportExportOutcome {
    Exported(PathBuf),
    Failed(String),
}

fn support_export_state_id() -> egui::Id {
    egui::Id::new(SUPPORT_EXPORT_STATE_ID)
}

fn support_export_outcome(ctx: &egui::Context) -> Option<SupportExportOutcome> {
    ctx.data(|data| {
        data.get_temp::<Option<SupportExportOutcome>>(support_export_state_id())
            .flatten()
    })
}

fn set_support_export_outcome(ctx: &egui::Context, outcome: SupportExportOutcome) {
    ctx.data_mut(|data| data.insert_temp(support_export_state_id(), Some(outcome)));
}

fn apply_support_export_result(ctx: &egui::Context, result: std::io::Result<PathBuf>) {
    let outcome = result
        .map(SupportExportOutcome::Exported)
        .unwrap_or_else(|error| SupportExportOutcome::Failed(bounded_error(&error)));
    set_support_export_outcome(ctx, outcome);
}

fn clear_support_export_outcome(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        data.remove_temp::<Option<SupportExportOutcome>>(support_export_state_id());
    });
}

fn bounded_error(error: &std::io::Error) -> String {
    bound_support_text(&error.to_string())
}

/// Resolve the one support-export location from the current UID's operating
/// system account record. Environment-controlled XDG/HOME roots and temporary
/// fallbacks are intentionally excluded from this security boundary.
fn support_export_dir() -> std::io::Result<PathBuf> {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    const MAX_PASSWD_BYTES: u64 = 1024 * 1024;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).custom_flags(0o400000); // Linux O_NOFOLLOW.
    let passwd = options.open("/etc/passwd")?;
    if !passwd.metadata()?.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "account database is not a regular file",
        ));
    }
    let mut contents = String::new();
    passwd
        .take(MAX_PASSWD_BYTES.saturating_add(1))
        .read_to_string(&mut contents)?;
    if contents.len() as u64 > MAX_PASSWD_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "account database exceeds its read bound",
        ));
    }
    let uid = rustix::process::getuid().as_raw();
    let home = contents.lines().find_map(|line| {
        let mut fields = line.split(':');
        let _name = fields.next()?;
        let _password = fields.next()?;
        let account_uid = fields.next()?.parse::<u32>().ok()?;
        let _gid = fields.next()?;
        let _gecos = fields.next()?;
        let home = fields.next()?;
        (account_uid == uid).then(|| PathBuf::from(home))
    });
    let home = home.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "current UID has no account home",
        )
    })?;
    if !home.is_absolute()
        || home == Path::new("/")
        || home.components().any(|component| {
            matches!(
                component,
                Component::CurDir | Component::ParentDir | Component::Prefix(_)
            )
        })
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "account home is not a safe absolute path",
        ));
    }
    Ok(home
        .join(".local")
        .join("share")
        .join("mde")
        .join("health-support"))
}

fn support_bundle_filename(snapshot: &SystemMeshHealthSnapshot) -> String {
    sanitize_support_filename(&format!(
        "health-support-{}-{}.json",
        snapshot.generated_at_ms, snapshot.generation
    ))
}

fn sanitize_support_filename(name: &str) -> String {
    let mut safe = String::with_capacity(name.len().min(SUPPORT_BUNDLE_MAX_FILENAME_BYTES));
    for character in name.chars() {
        let mapped = if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            character
        } else {
            '_'
        };
        if safe.len().saturating_add(mapped.len_utf8()) > SUPPORT_BUNDLE_MAX_FILENAME_BYTES {
            break;
        }
        safe.push(mapped);
    }
    if safe.is_empty() || safe == "." || safe == ".." {
        "health-support.json".into()
    } else {
        safe
    }
}

fn capture_support_export_authority(
    ctx: &egui::Context,
    chrome: &ConstructChrome,
    snapshot: &SystemMeshHealthSnapshot,
) -> std::io::Result<SupportExportAuthority> {
    let node_scope = chrome.health_selected_node.clone().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "support export has no admitted Health selection",
        )
    })?;
    let selected_incident_id = history_selection(ctx, &node_scope);
    let authority = SupportExportAuthority {
        snapshot_generation: snapshot.generation,
        snapshot_generated_at_ms: snapshot.generated_at_ms,
        node_scope,
        severity: history_filter(ctx),
        component: history_component_filter(ctx),
        source: history_source_filter(ctx),
        provider: history_provider_filter(ctx),
        selected_incident_id,
    };
    validate_support_export_authority(snapshot, &authority)?;
    Ok(authority)
}

fn validate_support_export_authority(
    snapshot: &SystemMeshHealthSnapshot,
    authority: &SupportExportAuthority,
) -> std::io::Result<()> {
    let snapshot_matches = authority.snapshot_generation == snapshot.generation
        && authority.snapshot_generated_at_ms == snapshot.generated_at_ms;
    let scope_matches = authority.node_scope == MESH_SELECTION
        || snapshot
            .current_node_grades
            .iter()
            .any(|grade| grade.node == authority.node_scope);
    let incident_matches = authority
        .selected_incident_id
        .as_ref()
        .is_none_or(|incident_id| {
            selected_history_condition(
                &snapshot.resolved_conditions,
                &authority.node_scope,
                snapshot.generated_at_ms,
                incident_id,
            )
            .is_some()
        });
    if !snapshot_matches || !scope_matches || !incident_matches {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Health changed before the support export became durable; review the current selection and try again",
        ));
    }
    Ok(())
}

fn export_support_bundle(
    ctx: &egui::Context,
    chrome: &ConstructChrome,
    snapshot: &SystemMeshHealthSnapshot,
    authority: &SupportExportAuthority,
) -> std::io::Result<PathBuf> {
    let current = capture_support_export_authority(ctx, chrome, snapshot)?;
    export_support_bundle_to(&support_export_dir()?, snapshot, authority, &current)
}

fn export_support_bundle_to(
    directory: &Path,
    snapshot: &SystemMeshHealthSnapshot,
    authority: &SupportExportAuthority,
    current: &SupportExportAuthority,
) -> std::io::Result<PathBuf> {
    if authority != current {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Health selection or filters changed before the support export became durable; review the current view and try again",
        ));
    }
    validate_support_export_authority(snapshot, authority)?;
    let encoded = support_bundle_json(snapshot, authority)?;
    write_support_bundle(directory, &support_bundle_filename(snapshot), &encoded)
}

/// Write a complete sibling and rename it into place. The filename seam is
/// deliberately defensive for tests and future callers: exactly one sanitized
/// normal component is accepted, so traversal cannot escape `directory`.
fn write_support_bundle(
    directory: &Path,
    filename: &str,
    encoded: &[u8],
) -> std::io::Result<PathBuf> {
    use rand::RngCore as _;

    let mut nonce = [0_u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    write_support_bundle_with_nonce(directory, filename, encoded, nonce)
}

fn write_support_bundle_with_nonce(
    directory: &Path,
    filename: &str,
    encoded: &[u8],
    nonce: [u8; 16],
) -> std::io::Result<PathBuf> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags};
    use std::io::Write as _;
    let mut components = Path::new(filename).components();
    let one_safe_component = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && filename == sanitize_support_filename(filename)
        && filename.len() <= SUPPORT_BUNDLE_MAX_FILENAME_BYTES;
    if !one_safe_component {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "support bundle filename is not contained",
        ));
    }
    if encoded.len() > SUPPORT_BUNDLE_MAX_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "support bundle exceeds the byte limit",
        ));
    }
    if !directory.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "support export directory must be absolute",
        ));
    }
    let path_components = directory
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            Component::RootDir => None,
            _ => Some(std::ffi::OsString::new()),
        })
        .collect::<Vec<_>>();
    if path_components.iter().any(|component| component.is_empty()) || path_components.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "support export directory contains an unsafe component",
        ));
    }

    let directory_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory_fd =
        rustix::fs::open("/", directory_flags, Mode::empty()).map_err(rustix_io_error)?;
    for component in path_components {
        directory_fd =
            match rustix::fs::openat(&directory_fd, &component, directory_flags, Mode::empty()) {
                Ok(opened) => opened,
                Err(rustix::io::Errno::NOENT) => {
                    match rustix::fs::mkdirat(
                        &directory_fd,
                        &component,
                        Mode::RUSR | Mode::WUSR | Mode::XUSR,
                    ) {
                        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                        Err(error) => return Err(rustix_io_error(error)),
                    }
                    let parent: std::fs::File = directory_fd.try_clone()?.into();
                    parent.sync_all()?;
                    rustix::fs::openat(&directory_fd, &component, directory_flags, Mode::empty())
                        .map_err(rustix_io_error)?
                }
                Err(error) => return Err(rustix_io_error(error)),
            };
    }
    let directory_metadata = rustix::fs::fstat(&directory_fd).map_err(rustix_io_error)?;
    if directory_metadata.st_uid != rustix::process::getuid().as_raw()
        || directory_metadata.st_mode & 0o077 != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "support export directory is not private to the current user",
        ));
    }

    match rustix::fs::statat(&directory_fd, filename, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata) if FileType::from_raw_mode(metadata.st_mode) == FileType::Symlink => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "support bundle destination is a symlink",
            ));
        }
        Ok(metadata) if FileType::from_raw_mode(metadata.st_mode) != FileType::RegularFile => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "support bundle destination is not a regular file",
            ));
        }
        Ok(_) | Err(rustix::io::Errno::NOENT) => {}
        Err(error) => return Err(rustix_io_error(error)),
    }

    let nonce = nonce
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let temporary = format!(".health-support-{nonce}.tmp");
    let temporary_flags =
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let temporary_fd = rustix::fs::openat(
        &directory_fd,
        temporary.as_str(),
        temporary_flags,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(rustix_io_error)?;
    let mut temporary_file: std::fs::File = temporary_fd.into();
    if let Err(error) = temporary_file
        .write_all(encoded)
        .and_then(|()| temporary_file.sync_all())
    {
        drop(temporary_file);
        let _ = rustix::fs::unlinkat(&directory_fd, temporary.as_str(), AtFlags::empty());
        let directory_file: std::fs::File = directory_fd.into();
        let _ = directory_file.sync_all();
        return Err(error);
    }
    drop(temporary_file);
    if let Err(error) =
        rustix::fs::renameat(&directory_fd, temporary.as_str(), &directory_fd, filename)
    {
        let _ = rustix::fs::unlinkat(&directory_fd, temporary.as_str(), AtFlags::empty());
        let directory_file: std::fs::File = directory_fd.into();
        let _ = directory_file.sync_all();
        return Err(rustix_io_error(error));
    }
    let directory_file: std::fs::File = directory_fd.into();
    directory_file.sync_all()?;
    Ok(directory.join(filename))
}

fn rustix_io_error(error: rustix::io::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(error.raw_os_error())
}

fn bounded_sorted_clones<'a, T: Clone + 'a>(
    candidates: impl Iterator<Item = &'a T>,
    maximum: usize,
    compare: impl Fn(&T, &T) -> std::cmp::Ordering,
) -> Vec<T> {
    let mut selected = Vec::with_capacity(maximum);
    for candidate in candidates {
        let position = selected
            .binary_search_by(|existing| compare(existing, candidate))
            .unwrap_or_else(|position| position);
        if position < maximum {
            selected.insert(position, candidate.clone());
            if selected.len() > maximum {
                selected.pop();
            }
        }
    }
    selected
}

fn support_bundle_json(
    snapshot: &SystemMeshHealthSnapshot,
    authority: &SupportExportAuthority,
) -> std::io::Result<Vec<u8>> {
    validate_support_export_authority(snapshot, authority)?;
    let mut nodes = bounded_sorted_clones(
        snapshot.current_node_grades.iter().filter(|node| {
            authority.node_scope == MESH_SELECTION || node.node == authority.node_scope
        }),
        SUPPORT_BUNDLE_MAX_NODES,
        |left, right| {
            left.node
                .cmp(&right.node)
                .then_with(|| left.evaluated_at_ms.cmp(&right.evaluated_at_ms))
        },
    );

    let mut active = bounded_sorted_clones(
        snapshot.active_conditions.iter().filter(|condition| {
            condition.is_active() && support_scope_admits(authority, condition)
        }),
        SUPPORT_BUNDLE_MAX_ACTIVE,
        support_condition_order,
    );

    let window_start = snapshot.generated_at_ms.saturating_sub(HISTORY_WINDOW_MS);
    let mut resolved = bounded_sorted_clones(
        snapshot.resolved_conditions.iter().filter(|condition| {
            support_scope_admits(authority, condition)
                && authority.severity.admits(condition.severity)
                && authority.component.admits(condition.component)
                && authority
                    .source
                    .as_deref()
                    .is_none_or(|source| condition.source == source)
                && authority
                    .provider
                    .as_deref()
                    .is_none_or(|provider| condition.evidence.provider == provider)
                && condition.resolved_at_ms.is_some_and(|resolved_at| {
                    (window_start..=snapshot.generated_at_ms).contains(&resolved_at)
                        && resolved_at >= condition.last_observed_ms
                })
        }),
        SUPPORT_BUNDLE_MAX_RESOLVED,
        support_condition_order,
    );

    loop {
        let value = support_bundle_value(snapshot, authority, &nodes, &active, &resolved);
        let encoded = serde_json::to_vec_pretty(&value).map_err(std::io::Error::other)?;
        if encoded.len() <= SUPPORT_BUNDLE_MAX_BYTES {
            return Ok(encoded);
        }
        if resolved.pop().is_some() || active.pop().is_some() || nodes.pop().is_some() {
            continue;
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "support bundle metadata exceeds the byte limit",
        ));
    }
}

fn support_scope_admits(authority: &SupportExportAuthority, condition: &HealthCondition) -> bool {
    authority.node_scope == MESH_SELECTION
        || matches!(
            &condition.scope,
            HealthScope::Node { node } if node == &authority.node_scope
        )
}

fn support_condition_order(left: &HealthCondition, right: &HealthCondition) -> std::cmp::Ordering {
    right
        .severity
        .cmp(&left.severity)
        .then_with(|| right.resolved_at_ms.cmp(&left.resolved_at_ms))
        .then_with(|| left.id.cmp(&right.id))
        .then_with(|| left.source.cmp(&right.source))
}

fn support_bundle_value(
    snapshot: &SystemMeshHealthSnapshot,
    authority: &SupportExportAuthority,
    nodes: &[NodeGrade],
    active: &[HealthCondition],
    resolved: &[HealthCondition],
) -> serde_json::Value {
    serde_json::json!({
        "schema": SUPPORT_BUNDLE_SCHEMA,
        "generated_at_ms": snapshot.generated_at_ms,
        "snapshot": {
            "schema_version": snapshot.schema_version,
            "generation": snapshot.generation,
            "fresh_until_ms": snapshot.fresh_until_ms,
            "observer": redact_support_text(&snapshot.observer),
            "roster_revision": redact_support_text(&snapshot.roster_revision),
        },
        "export_authority": {
            "snapshot_generation": authority.snapshot_generation,
            "snapshot_generated_at_ms": authority.snapshot_generated_at_ms,
            "node_scope": redact_support_text(&authority.node_scope),
            "severity_filter": authority.severity.label(),
            "component_filter": authority.component.label(),
            "source_filter": authority.source.as_deref().map(redact_support_text),
            "provider_filter": authority.provider.as_deref().map(redact_support_text),
            "selected_incident_id": authority.selected_incident_id.as_deref().map(redact_support_text),
        },
        "mesh_summary": {
            "grade": snapshot.mesh_summary.grade.as_str(),
            "canonical_nodes": snapshot.mesh_summary.canonical_nodes,
            "fresh_nodes": snapshot.mesh_summary.fresh_nodes,
            "reachable_lighthouses": snapshot.mesh_summary.reachable_lighthouses,
            "active_warnings": snapshot.mesh_summary.active_warnings,
            "active_critical": snapshot.mesh_summary.active_critical,
            "unacknowledged_actionable": snapshot.mesh_summary.unacknowledged_actionable,
        },
        "limits": {
            "maximum_bytes": SUPPORT_BUNDLE_MAX_BYTES,
            "maximum_text_bytes": SUPPORT_BUNDLE_MAX_TEXT_BYTES,
            "history_window_ms": HISTORY_WINDOW_MS,
            "nodes_in_snapshot": snapshot.current_node_grades.len(),
            "nodes_exported": nodes.len(),
            "active_in_snapshot": snapshot.active_conditions.len(),
            "active_exported": active.len(),
            "resolved_in_snapshot": snapshot.resolved_conditions.len(),
            "resolved_exported": resolved.len(),
        },
        "nodes": nodes.iter().map(support_node_value).collect::<Vec<_>>(),
        "active_conditions": active.iter().map(support_condition_value).collect::<Vec<_>>(),
        "resolved_history": resolved.iter().map(support_condition_value).collect::<Vec<_>>(),
        "redaction": "credential-shaped values, authorization material, and paths are omitted or replaced",
    })
}

fn support_node_value(node: &NodeGrade) -> serde_json::Value {
    serde_json::json!({
        "node": redact_support_text(&node.node),
        "grade": node.grade.as_str(),
        "capability_score": node.capability_score,
        "evaluated_at_ms": node.evaluated_at_ms,
        "factors": {
            "cpu": node.factors.cpu,
            "memory": node.factors.memory,
            "disk": node.factors.disk,
            "system": node.factors.system,
            "mesh": node.factors.mesh,
            "devices": node.factors.devices,
        },
    })
}

fn support_condition_value(condition: &HealthCondition) -> serde_json::Value {
    let scope = match &condition.scope {
        HealthScope::Node { node } => serde_json::json!({
            "scope": "node",
            "node": redact_support_text(node),
        }),
        HealthScope::Mesh => serde_json::json!({ "scope": "mesh" }),
    };
    let facts = condition
        .evidence
        .facts
        .iter()
        .filter(|(key, _)| !credential_shaped(key))
        .take(SUPPORT_BUNDLE_MAX_FACTS)
        .map(|(key, value)| (redact_support_text(key), redact_support_text(value)))
        .collect::<std::collections::BTreeMap<_, _>>();
    serde_json::json!({
        "id": redact_support_text(&condition.id),
        "scope": scope,
        "component": format!("{:?}", condition.component).to_ascii_lowercase(),
        "source": redact_support_text(&condition.source),
        "severity": format!("{:?}", condition.severity).to_ascii_lowercase(),
        "requirement": format!("{:?}", condition.requirement).to_ascii_lowercase(),
        "evidence": {
            "provider": redact_support_text(&condition.evidence.provider),
            "summary": redact_support_text(&condition.evidence.summary),
            "facts": facts,
            "observed_at_ms": condition.evidence.observed_at_ms,
        },
        "active_since_ms": condition.active_since_ms,
        "last_observed_ms": condition.last_observed_ms,
        "resolved_at_ms": condition.resolved_at_ms,
        "acknowledged_at_ms": condition.acknowledged_at_ms,
        "snoozed_until_ms": condition.snoozed_until_ms,
    })
}

fn redact_support_text(value: &str) -> String {
    if value.len() > MAX_REDACTION_SCAN_BYTES
        || credential_shaped(value)
        || unsafe_path_shaped(value)
    {
        "[redacted]".into()
    } else {
        bound_support_text(value)
    }
}

fn credential_shaped(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let normalized: String = lower
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    [
        "authorization",
        "bearer",
        "cookie",
        "credential",
        "password",
        "privatekey",
        "secret",
        "token",
        "-----begin",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
}

fn unsafe_path_shaped(value: &str) -> bool {
    value.contains("../")
        || value.contains("..\\")
        || value.contains("file://")
        || value.split_ascii_whitespace().any(|part| {
            part.starts_with('/')
                || part.starts_with('\\')
                || part.as_bytes().get(1) == Some(&b':')
                    && part
                        .as_bytes()
                        .get(2)
                        .is_some_and(|byte| matches!(byte, b'/' | b'\\'))
        })
}

fn bound_support_text(value: &str) -> String {
    if value.len() <= SUPPORT_BUNDLE_MAX_TEXT_BYTES {
        return value.to_string();
    }
    let mut bounded = String::with_capacity(SUPPORT_BUNDLE_MAX_TEXT_BYTES);
    for character in value.chars() {
        if bounded.len().saturating_add(character.len_utf8()) > SUPPORT_BUNDLE_MAX_TEXT_BYTES - 3 {
            break;
        }
        bounded.push(character);
    }
    bounded.push_str("...");
    bounded
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActionPublishFailure {
    StaleSnapshot,
    ConditionNotCurrent,
    TargetMismatch,
    ActionNotAuthorized,
    ConfirmationRequired,
    BusRootUnavailable,
    PersistOpen,
    Serialization,
    PersistWrite,
}

impl ActionPublishFailure {
    const fn presentable(self) -> &'static str {
        match self {
            Self::StaleSnapshot => {
                "Couldn’t send the health action because this health snapshot is stale. Refresh Health and try again."
            }
            Self::ConditionNotCurrent => {
                "Couldn’t send the health action because this issue is no longer current. Refresh Health and review the latest state."
            }
            Self::TargetMismatch => {
                "Couldn’t send the health action because its target no longer matches this issue. Refresh Health before retrying."
            }
            Self::ActionNotAuthorized => {
                "Couldn’t send the health action because the current issue does not authorize it. Refresh Health and review the offered actions."
            }
            Self::ConfirmationRequired => {
                "Couldn’t send the health action because the current recovery authority requires explicit confirmation."
            }
            Self::BusRootUnavailable => {
                "Couldn’t send the health action because the local Mesh Bus is unavailable. Retry when it returns."
            }
            Self::PersistOpen => {
                "Couldn’t open the local Mesh Bus to send the health action. Retry after the service recovers."
            }
            Self::Serialization => {
                "Couldn’t prepare the health action for publication. The action was not sent."
            }
            Self::PersistWrite => {
                "Couldn’t write the health action to the local Mesh Bus. The action was not sent; retry when storage is writable."
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ActionPublishOutcome {
    Published(HealthActionRequest),
    Failed(ActionPublishFailure),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingHealthAction {
    request: HealthActionRequest,
    result: Option<HealthActionResult>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActionResultPollIssue {
    BusUnavailable,
    UnverifiedResult,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ActionResultPoll {
    Waiting,
    Matched(HealthActionResult),
    Blocked(ActionResultPollIssue),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActionProgressTone {
    Neutral,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ActionProgressPresentation {
    tone: ActionProgressTone,
    title: String,
    detail: String,
}

fn action_error_id() -> egui::Id {
    egui::Id::new(ACTION_ERROR_STATE_ID)
}

fn action_progress_id() -> egui::Id {
    egui::Id::new(ACTION_PROGRESS_STATE_ID)
}

fn action_error(ctx: &egui::Context) -> Option<ActionPublishFailure> {
    ctx.data(|data| {
        data.get_temp::<Option<ActionPublishFailure>>(action_error_id())
            .flatten()
    })
}

fn clear_action_error(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        data.remove_temp::<Option<ActionPublishFailure>>(action_error_id());
    });
}

fn pending_health_action(ctx: &egui::Context) -> Option<PendingHealthAction> {
    ctx.data(|data| data.get_temp::<PendingHealthAction>(action_progress_id()))
}

fn set_pending_health_action(ctx: &egui::Context, pending: PendingHealthAction) {
    ctx.data_mut(|data| data.insert_temp(action_progress_id(), pending));
}

fn apply_action_outcome(
    ctx: &egui::Context,
    chrome: &mut ConstructChrome,
    clear_confirmation_on_success: bool,
    outcome: ActionPublishOutcome,
) {
    match outcome {
        ActionPublishOutcome::Published(request) => {
            clear_action_error(ctx);
            set_pending_health_action(
                ctx,
                PendingHealthAction {
                    request,
                    result: None,
                },
            );
            if clear_confirmation_on_success {
                chrome.health_pending_action = None;
            }
        }
        ActionPublishOutcome::Failed(error) => {
            ctx.data_mut(|data| data.insert_temp(action_error_id(), Some(error)));
        }
    }
}

fn action_progress_is_pending(ctx: &egui::Context, snapshot: &SystemMeshHealthSnapshot) -> bool {
    pending_health_action(ctx).is_some_and(|pending| match pending.result {
        None => true,
        Some(result) => {
            result.outcome == HealthActionOutcome::Applied
                && snapshot.generation < result.snapshot_generation
        }
    })
}

fn refresh_action_progress(
    ctx: &egui::Context,
    snapshot: Option<&SystemMeshHealthSnapshot>,
) -> Option<ActionProgressPresentation> {
    let mut pending = pending_health_action(ctx)?;
    let mut poll_issue = None;
    if pending.result.is_none() {
        match poll_action_result(mde_bus::client_data_dir(), &pending.request) {
            ActionResultPoll::Waiting => {}
            ActionResultPoll::Matched(result) => {
                pending.result = Some(result);
                set_pending_health_action(ctx, pending.clone());
            }
            ActionResultPoll::Blocked(issue) => poll_issue = Some(issue),
        }
    }
    let presentation = action_progress_presentation(&pending, snapshot, poll_issue);
    if pending.result.is_none()
        || pending.result.as_ref().is_some_and(|result| {
            result.outcome == HealthActionOutcome::Applied
                && snapshot.is_none_or(|snapshot| snapshot.generation < result.snapshot_generation)
        })
    {
        ctx.request_repaint_after(ACTION_RESULT_POLL_INTERVAL);
    }
    Some(presentation)
}

fn poll_action_result(root: Option<PathBuf>, request: &HealthActionRequest) -> ActionResultPoll {
    let Some(root) = root else {
        return ActionResultPoll::Blocked(ActionResultPollIssue::BusUnavailable);
    };
    let Ok(persist) = Persist::open(root) else {
        return ActionResultPoll::Blocked(ActionResultPollIssue::BusUnavailable);
    };
    let topic = action_result_topic(&request.request_id);
    let Ok(rows) = persist.read_tail(&topic, ACTION_RESULT_TAIL_BOUND) else {
        return ActionResultPoll::Blocked(ActionResultPollIssue::BusUnavailable);
    };
    if rows.is_empty() {
        return ActionResultPoll::Waiting;
    }

    let now = now_ms();
    let mut matched: Option<HealthActionResult> = None;
    for row in rows {
        let Some(body) = row.body.as_deref() else {
            continue;
        };
        let Ok(result) = serde_json::from_str::<HealthActionResult>(body) else {
            continue;
        };
        if result.validate_at(now).is_err() {
            continue;
        }
        if !result_is_bound_to_request(&result, request, now) {
            continue;
        }
        if matched.as_ref().is_some_and(|prior| prior != &result) {
            return ActionResultPoll::Blocked(ActionResultPollIssue::UnverifiedResult);
        }
        matched = Some(result);
    }
    matched.map_or(
        ActionResultPoll::Blocked(ActionResultPollIssue::UnverifiedResult),
        ActionResultPoll::Matched,
    )
}

/// The result contract does not repeat `target`; bind it transitively to the
/// exact request using the worker's node-qualified audit identity. Node actions
/// additionally require the local requester to be the target; mesh actions bind
/// the publisher to that local requester. A guessed result topic therefore
/// cannot complete another node's request.
fn result_is_bound_to_request(
    result: &HealthActionResult,
    request: &HealthActionRequest,
    now_ms: u64,
) -> bool {
    let result_publisher = match &request.target {
        HealthScope::Node { node } if node == &request.requester => node,
        HealthScope::Node { .. } => return false,
        HealthScope::Mesh => &request.requester,
    };
    let audit_prefix = format!("health:{result_publisher}:");
    let source = result.audit_id.strip_prefix(&audit_prefix);
    result.schema_version == HEALTH_SCHEMA_VERSION
        && result.request_id == request.request_id
        && result.condition_id == request.condition_id
        && result.action == request.action
        && result.snapshot_generation >= request.expected_snapshot_generation
        && result.completed_at_ms >= request.requested_at_ms
        && result.completed_at_ms <= now_ms
        && source.is_some_and(|source| {
            source.len() == 26
                && source
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        })
}

fn action_progress_presentation(
    pending: &PendingHealthAction,
    snapshot: Option<&SystemMeshHealthSnapshot>,
    poll_issue: Option<ActionResultPollIssue>,
) -> ActionProgressPresentation {
    let action = action_label(pending.request.action);
    let target = match &pending.request.target {
        HealthScope::Node { node } => node.as_str(),
        HealthScope::Mesh => "the mesh",
    };
    let base = format!("{action} · {target}");
    let Some(result) = pending.result.as_ref() else {
        return match poll_issue {
            None => ActionProgressPresentation {
                tone: ActionProgressTone::Neutral,
                title: format!("{base} requested"),
                detail: "Waiting for the governed health worker to report a result.".into(),
            },
            Some(ActionResultPollIssue::BusUnavailable) => ActionProgressPresentation {
                tone: ActionProgressTone::Warning,
                title: format!("{base} result unavailable"),
                detail: "The local Mesh Bus cannot currently be read. The action remains pending; no outcome is being inferred.".into(),
            },
            Some(ActionResultPollIssue::UnverifiedResult) => ActionProgressPresentation {
                tone: ActionProgressTone::Warning,
                title: format!("{base} result not verified"),
                detail: "A result row did not match this request’s identity, generation, action, and target. The action remains pending.".into(),
            },
        };
    };

    let result_detail = bound_support_text(&result.detail);
    match result.outcome {
        HealthActionOutcome::Applied => {
            let Some(snapshot) = snapshot.filter(|snapshot| {
                snapshot.is_fresh(now_ms()) && snapshot.generation >= result.snapshot_generation
            }) else {
                return ActionProgressPresentation {
                    tone: ActionProgressTone::Neutral,
                    title: format!("{base} ran; refreshing health"),
                    detail: format!("{result_detail} Current health evidence has not reached result generation {} yet.", result.snapshot_generation),
                };
            };
            let still_active = snapshot.active_conditions.iter().find(|condition| {
                condition.is_active()
                    && condition.id == pending.request.condition_id
                    && condition.scope == pending.request.target
            });
            if let Some(condition) = still_active {
                ActionProgressPresentation {
                    tone: ActionProgressTone::Warning,
                    title: format!("{base} completed with the issue still active"),
                    detail: format!(
                        "{result_detail} Current evidence: {}",
                        bound_support_text(&condition.evidence.summary)
                    ),
                }
            } else if snapshot.active_conditions.iter().any(|condition| {
                condition.is_active() && condition.id == pending.request.condition_id
            }) {
                ActionProgressPresentation {
                    tone: ActionProgressTone::Warning,
                    title: format!("{base} result needs refreshed target evidence"),
                    detail: "The condition identity now belongs to a different target, so this result is not being used to report recovery.".into(),
                }
            } else {
                ActionProgressPresentation {
                    tone: ActionProgressTone::Success,
                    title: format!("{base} completed"),
                    detail: format!("{result_detail} Current health no longer reports this issue."),
                }
            }
        }
        HealthActionOutcome::Refused => ActionProgressPresentation {
            tone: ActionProgressTone::Error,
            title: format!("{base} was refused"),
            detail: result_detail,
        },
        HealthActionOutcome::StaleGeneration => ActionProgressPresentation {
            tone: ActionProgressTone::Warning,
            title: format!("{base} was not run against stale health"),
            detail: result_detail,
        },
        HealthActionOutcome::NotApplicable => ActionProgressPresentation {
            tone: ActionProgressTone::Warning,
            title: format!("{base} was not applicable"),
            detail: result_detail,
        },
        HealthActionOutcome::Failed => ActionProgressPresentation {
            tone: ActionProgressTone::Error,
            title: format!("{base} failed"),
            detail: result_detail,
        },
    }
}

fn render_action_progress(ui: &mut egui::Ui, progress: &ActionProgressPresentation) {
    let tone = match progress.tone {
        ActionProgressTone::Neutral => Style::TEXT_DIM,
        ActionProgressTone::Success => Style::SUPPORT_SUCCESS,
        ActionProgressTone::Warning => Style::SUPPORT_WARNING,
        ActionProgressTone::Error => Style::SUPPORT_ERROR,
    };
    mde_egui::card().show(ui, |ui| {
        ui.colored_label(tone, &progress.title);
        ui.label(&progress.detail);
    });
}

fn publish_action_for_ui(
    ctx: &egui::Context,
    chrome: &mut ConstructChrome,
    snapshot: &SystemMeshHealthSnapshot,
    condition: &HealthCondition,
    node: &str,
    action: HealthAction,
    confirmed: bool,
) {
    if action == HealthAction::OpenOnboarding {
        let outcome =
            match authorize_modal_action(snapshot, condition, node, action, confirmed, now_ms()) {
                Err(error) => ActionPublishOutcome::Failed(error),
                Ok(_) => match launch_onboarding() {
                    Ok(()) => {
                        chrome.health_pending_action = None;
                        return;
                    }
                    Err(_) => ActionPublishOutcome::Failed(ActionPublishFailure::PersistWrite),
                },
            };
        apply_action_outcome(ctx, chrome, confirmed, outcome);
        return;
    }
    let outcome = publish_action(snapshot, condition, node, action, confirmed);
    apply_action_outcome(ctx, chrome, confirmed, outcome);
}

fn launch_onboarding() -> Result<(), String> {
    let path = Path::new("/usr/bin/magic-setup");
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("onboarding executable is missing or unsafe".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err("onboarding executable is not runnable".into());
        }
    }
    Command::new(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn publish_action(
    snapshot: &SystemMeshHealthSnapshot,
    condition: &HealthCondition,
    node: &str,
    action: HealthAction,
    confirmed: bool,
) -> ActionPublishOutcome {
    publish_action_to(
        mde_bus::client_data_dir(),
        snapshot,
        condition,
        node,
        action,
        confirmed,
    )
}

fn publish_action_to(
    root: Option<std::path::PathBuf>,
    snapshot: &SystemMeshHealthSnapshot,
    condition: &HealthCondition,
    node: &str,
    action: HealthAction,
    confirmed: bool,
) -> ActionPublishOutcome {
    let now = now_ms();
    let target = match authorize_modal_action(snapshot, condition, node, action, confirmed, now) {
        Ok(target) => target,
        Err(error) => return ActionPublishOutcome::Failed(error),
    };
    let request = HealthActionRequest {
        schema_version: HEALTH_SCHEMA_VERSION,
        request_id: format!(
            "health-{now:016x}-{:016x}",
            ACTION_REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ),
        condition_id: condition.id.clone(),
        action,
        target,
        expected_snapshot_generation: snapshot.generation,
        requester: crate::explorer::local_hostname(),
        authorization: "local-seat".into(),
        confirmation: confirmed.then(|| "CONFIRM".into()),
        requested_at_ms: now,
    };
    let Some(root) = root else {
        return ActionPublishOutcome::Failed(ActionPublishFailure::BusRootUnavailable);
    };
    let Ok(persist) = Persist::open(root) else {
        return ActionPublishOutcome::Failed(ActionPublishFailure::PersistOpen);
    };
    let Ok(body) = serde_json::to_string(&request) else {
        return ActionPublishOutcome::Failed(ActionPublishFailure::Serialization);
    };
    match persist.write(ACTION_TOPIC, Priority::Default, None, Some(&body)) {
        Ok(_) => ActionPublishOutcome::Published(request),
        Err(_) => ActionPublishOutcome::Failed(ActionPublishFailure::PersistWrite),
    }
}

/// Re-check the complete recovery authority at the final UI-to-Bus boundary.
/// Paint-time visibility is not authorization: the snapshot may have expired,
/// the selected row may have changed, or a stale callback may carry a condition
/// that is no longer in the canonical active lane. Mutation actions additionally
/// bind to exactly one generation- and target-matched remediation descriptor.
fn authorize_modal_action(
    snapshot: &SystemMeshHealthSnapshot,
    condition: &HealthCondition,
    node: &str,
    action: HealthAction,
    confirmed: bool,
    now_ms: u64,
) -> Result<HealthScope, ActionPublishFailure> {
    if !snapshot.is_fresh(now_ms) {
        return Err(ActionPublishFailure::StaleSnapshot);
    }
    if !condition.is_active()
        || !snapshot
            .active_conditions
            .iter()
            .any(|current| current == condition)
    {
        return Err(ActionPublishFailure::ConditionNotCurrent);
    }

    let target = condition.scope.clone();
    let caller_matches_scope = match &target {
        HealthScope::Node { node: target_node } => target_node == node,
        HealthScope::Mesh => node == MESH_SELECTION,
    };
    if !caller_matches_scope {
        return Err(ActionPublishFailure::TargetMismatch);
    }

    let mut offered = condition
        .remediation
        .iter()
        .filter(|remediation| remediation.action == action);
    let descriptor = offered.next();
    if offered.next().is_some() {
        return Err(ActionPublishFailure::ActionNotAuthorized);
    }
    if let Some(descriptor) = descriptor {
        if descriptor.target != target
            || descriptor.expected_snapshot_generation != snapshot.generation
        {
            return Err(ActionPublishFailure::ActionNotAuthorized);
        }
        if descriptor.confirmation_required && !confirmed {
            return Err(ActionPublishFailure::ConfirmationRequired);
        }
    } else if !matches!(
        action,
        HealthAction::Acknowledge | HealthAction::SnoozeOneHour
    ) {
        return Err(ActionPublishFailure::ActionNotAuthorized);
    }

    Ok(target)
}

const fn action_label(action: HealthAction) -> &'static str {
    match action {
        HealthAction::Acknowledge => "Acknowledge",
        HealthAction::SnoozeOneHour => "Snooze 1 hour",
        HealthAction::RefreshProvider => "Refresh provider",
        HealthAction::RestoreWorkstationAudio => "Restore workstation audio",
        HealthAction::RefreshFirmwareMetadata => "Refresh firmware metadata",
        HealthAction::RestartMackesd => "Restart mackesd",
        HealthAction::RestartMeshBus => "Restart Mesh Bus",
        HealthAction::RestartNebula => "Restart Nebula",
        HealthAction::RestartSyncthing => "Restart Syncthing",
        HealthAction::RestartDns => "Restart DNS provider",
        HealthAction::RestartKdc => "Restart KDC provider",
        HealthAction::RestartShell => "Restart Construct shell",
        HealthAction::ExpandSeat15Root => "Expand seat 15 root to 30 GiB",
        HealthAction::PublishOverlayIp => "Publish overlay IP",
        HealthAction::SetupEtcdClient => "Configure etcd client",
        HealthAction::RecoverXdgBinds => "Restore mesh Downloads binds",
        HealthAction::RunLifecycleFirstboot => "Retry first-boot convergence",
        HealthAction::OpenOnboarding => "Open Onboarding",
        HealthAction::StartNodeVirt => "Start node virt stack",
        HealthAction::StartBrowserVm => "Start Browser VM",
        HealthAction::SetupSyncthing => "Configure Syncthing file plane",
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn format_timestamp(timestamp_ms: u64) -> String {
    format!("{} s", timestamp_ms / 1_000)
}

fn resolution_duration_ms(condition: &HealthCondition) -> Option<u64> {
    condition
        .resolved_at_ms
        .map(|resolved| resolved.saturating_sub(condition.active_since_ms))
}

fn history_filter(ctx: &egui::Context) -> HistorySeverityFilter {
    ctx.data(|data| {
        data.get_temp::<HistorySeverityFilter>(egui::Id::new(HISTORY_FILTER_STATE_ID))
            .unwrap_or_default()
    })
}

fn set_history_filter(ctx: &egui::Context, filter: HistorySeverityFilter) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(HISTORY_FILTER_STATE_ID), filter));
}

fn history_page_state(
    ctx: &egui::Context,
    node: &str,
    filter: HistorySeverityFilter,
    component: HistoryComponentFilter,
    source: Option<&str>,
    provider: Option<&str>,
) -> HistoryPageState {
    ctx.data(|data| data.get_temp::<HistoryPageState>(egui::Id::new(HISTORY_PAGE_STATE_ID)))
        .filter(|state| {
            state.node == node
                && state.filter == filter
                && state.component == component
                && state.source.as_deref() == source
                && state.provider.as_deref() == provider
        })
        .unwrap_or_else(|| HistoryPageState {
            node: node.to_string(),
            filter,
            component,
            source: source.map(str::to_owned),
            provider: provider.map(str::to_owned),
            page: 0,
        })
}

fn history_component_filter(ctx: &egui::Context) -> HistoryComponentFilter {
    ctx.data(|data| {
        data.get_temp::<HistoryComponentFilter>(egui::Id::new(HISTORY_COMPONENT_FILTER_STATE_ID))
            .unwrap_or_default()
    })
}

fn set_history_component_filter(ctx: &egui::Context, filter: HistoryComponentFilter) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(HISTORY_COMPONENT_FILTER_STATE_ID), filter));
}

fn history_source_filter(ctx: &egui::Context) -> Option<String> {
    ctx.data(|data| data.get_temp(egui::Id::new(HISTORY_SOURCE_FILTER_STATE_ID)))
}

fn set_history_source_filter(ctx: &egui::Context, filter: Option<String>) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(HISTORY_SOURCE_FILTER_STATE_ID), filter));
}

fn history_provider_filter(ctx: &egui::Context) -> Option<String> {
    ctx.data(|data| data.get_temp(egui::Id::new(HISTORY_PROVIDER_FILTER_STATE_ID)))
}

fn set_history_provider_filter(ctx: &egui::Context, filter: Option<String>) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(HISTORY_PROVIDER_FILTER_STATE_ID), filter));
}

fn set_history_page_state(ctx: &egui::Context, state: HistoryPageState) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(HISTORY_PAGE_STATE_ID), state));
}

fn history_selection(ctx: &egui::Context, node: &str) -> Option<String> {
    ctx.data(|data| data.get_temp::<HistorySelection>(egui::Id::new(HISTORY_SELECTION_STATE_ID)))
        .filter(|selection| selection.node == node)
        .map(|selection| selection.incident_id)
}

fn set_history_selection(ctx: &egui::Context, node: &str, incident_id: &str) {
    ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new(HISTORY_SELECTION_STATE_ID),
            HistorySelection {
                node: node.to_owned(),
                incident_id: incident_id.to_owned(),
            },
        );
    });
}

struct HistoryRecurrence<'a> {
    condition: &'a HealthCondition,
    occurrences: usize,
}

struct HistoryPage<'a> {
    rows: Vec<HistoryRecurrence<'a>>,
    total: usize,
}

fn selected_history_condition<'a>(
    conditions: &'a [HealthCondition],
    node: &str,
    as_of_ms: u64,
    incident_id: &str,
) -> Option<&'a HealthCondition> {
    let window_start_ms = as_of_ms.saturating_sub(HISTORY_WINDOW_MS);
    conditions
        .iter()
        .filter(|condition| {
            condition.id == incident_id
                && matches!(&condition.scope, HealthScope::Node { node: target } if target == node)
                && !condition.is_active()
                && condition.resolved_at_ms.is_some_and(|resolved_at_ms| {
                    (window_start_ms..=as_of_ms).contains(&resolved_at_ms)
                        && resolved_at_ms >= condition.last_observed_ms
                })
        })
        .min_by(|left, right| history_order(left, right))
}

/// Aggregate stable lifecycle identities without materializing an unbounded
/// history in the modal. Only genuinely resolved records in the snapshot's
/// inclusive 24-hour window participate. The first pass retains a fixed maximum
/// of 256 identities; the second pass counts recurrences for those retained
/// rows, and paint materializes only the requested eight-row page. This keeps
/// paint-time memory fixed even if an untrusted caller bypasses the snapshot's
/// wire-level collection bound.
fn recurrence_history<'a>(
    conditions: &'a [HealthCondition],
    node: &str,
    as_of_ms: u64,
) -> Vec<HistoryRecurrence<'a>> {
    filtered_recurrence_history(conditions, node, as_of_ms, HistorySeverityFilter::All)
}

fn filtered_recurrence_history<'a>(
    conditions: &'a [HealthCondition],
    node: &str,
    as_of_ms: u64,
    filter: HistorySeverityFilter,
) -> Vec<HistoryRecurrence<'a>> {
    paged_recurrence_history(
        conditions,
        node,
        as_of_ms,
        filter,
        HistoryComponentFilter::All,
        None,
        None,
        0,
    )
    .rows
}

fn paged_recurrence_history<'a>(
    conditions: &'a [HealthCondition],
    node: &str,
    as_of_ms: u64,
    filter: HistorySeverityFilter,
    component: HistoryComponentFilter,
    source: Option<&str>,
    provider: Option<&str>,
    page: usize,
) -> HistoryPage<'a> {
    let window_start_ms = as_of_ms.saturating_sub(HISTORY_WINDOW_MS);
    let applies_to_page = |condition: &HealthCondition| {
        matches!(&condition.scope, HealthScope::Node { node: target } if target.as_str() == node)
            && filter.admits(condition.severity)
            && component.admits(condition.component)
            && source.is_none_or(|source| condition.source == source)
            && provider.is_none_or(|provider| condition.evidence.provider == provider)
            && !condition.is_active()
            && condition.resolved_at_ms.is_some_and(|resolved_at_ms| {
                (window_start_ms..=as_of_ms).contains(&resolved_at_ms)
                    && resolved_at_ms >= condition.last_observed_ms
            })
    };
    let mut resolved: Vec<HistoryRecurrence<'a>> = Vec::with_capacity(HISTORY_MAX_IDENTITIES);
    for condition in conditions
        .iter()
        .filter(|condition| applies_to_page(condition))
    {
        if let Some(recurrence) = resolved
            .iter_mut()
            .find(|recurrence| recurrence.condition.id == condition.id)
        {
            if history_order(condition, recurrence.condition) == std::cmp::Ordering::Less {
                recurrence.condition = condition;
                resolved.sort_by(|left, right| history_order(left.condition, right.condition));
            }
            continue;
        }
        let insert_at = resolved.partition_point(|existing| {
            history_order(existing.condition, condition) != std::cmp::Ordering::Greater
        });
        if insert_at >= HISTORY_MAX_IDENTITIES {
            continue;
        }
        resolved.insert(
            insert_at,
            HistoryRecurrence {
                condition,
                occurrences: 0,
            },
        );
        if resolved.len() > HISTORY_MAX_IDENTITIES {
            resolved.pop();
        }
    }
    for condition in conditions
        .iter()
        .filter(|condition| applies_to_page(condition))
    {
        if let Some(recurrence) = resolved
            .iter_mut()
            .find(|recurrence| recurrence.condition.id == condition.id)
        {
            recurrence.occurrences = recurrence.occurrences.saturating_add(1);
        }
    }
    let total = resolved.len();
    let offset = page.saturating_mul(HISTORY_PAGE_SIZE).min(total);
    let rows = resolved
        .into_iter()
        .skip(offset)
        .take(HISTORY_PAGE_SIZE)
        .collect();
    HistoryPage { rows, total }
}

fn history_origin_choices(
    conditions: &[HealthCondition],
    node: &str,
    as_of_ms: u64,
    value: impl for<'a> Fn(&'a HealthCondition) -> &'a str,
) -> Vec<String> {
    let window_start_ms = as_of_ms.saturating_sub(HISTORY_WINDOW_MS);
    let mut choices = std::collections::BTreeSet::new();
    for condition in conditions.iter().filter(|condition| {
        matches!(&condition.scope, HealthScope::Node { node: target } if target == node)
            && !condition.is_active()
            && condition.resolved_at_ms.is_some_and(|resolved_at_ms| {
                (window_start_ms..=as_of_ms).contains(&resolved_at_ms)
                    && resolved_at_ms >= condition.last_observed_ms
            })
    }) {
        choices.insert(value(condition).to_owned());
        if choices.len() > HISTORY_ORIGIN_FILTER_LIMIT {
            choices.pop_last();
        }
    }
    choices.into_iter().collect()
}

fn history_order(left: &HealthCondition, right: &HealthCondition) -> std::cmp::Ordering {
    right
        .severity
        .cmp(&left.severity)
        .then_with(|| resolution_duration_ms(right).cmp(&resolution_duration_ms(left)))
        .then_with(|| left.id.cmp(&right.id))
        .then_with(|| right.resolved_at_ms.cmp(&left.resolved_at_ms))
        .then_with(|| right.last_observed_ms.cmp(&left.last_observed_ms))
        .then_with(|| left.source.cmp(&right.source))
        .then_with(|| left.evidence.provider.cmp(&right.evidence.provider))
        .then_with(|| left.evidence.summary.cmp(&right.evidence.summary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::Path;

    use mackes_mesh_types::health::{
        GradeFactors, GradeLetter, HealthEvidence, HealthRemediation, MeshHealthSummary,
        RequirementClass,
    };

    fn condition(
        id: &str,
        node: &str,
        severity: HealthSeverity,
        component: HealthComponent,
    ) -> HealthCondition {
        HealthCondition {
            id: id.into(),
            scope: HealthScope::Node { node: node.into() },
            component,
            source: "render-proof".into(),
            severity,
            requirement: RequirementClass::Required,
            evidence: HealthEvidence {
                provider: "direct seat poll".into(),
                summary: format!(
                    "Evidence for {node} has a deliberately long summary that remains readable."
                ),
                facts: BTreeMap::from([
                    ("threshold".into(), "95% for three observations".into()),
                    ("observed".into(), "96%, 97%, 96%".into()),
                ]),
                observed_at_ms: 1_000,
            },
            active_since_ms: 900,
            last_observed_ms: 1_000,
            resolved_at_ms: None,
            acknowledged_at_ms: None,
            snoozed_until_ms: None,
            remediation: vec![HealthRemediation {
                action: HealthAction::RefreshProvider,
                target: HealthScope::Node { node: node.into() },
                expected_snapshot_generation: 42,
                impact: "Refreshes this allowlisted provider and republishes its evidence.".into(),
                confirmation_required: true,
                workspace_route: Some("device-manager?device=pci-0000:00:1f.3".into()),
            }],
        }
    }

    fn fixture_snapshot(with_issues: bool, fresh: bool) -> SystemMeshHealthSnapshot {
        let now = now_ms();
        let mut conditions = if with_issues {
            vec![
                condition(
                    "Dell-operations-workstation:cpu-pressure",
                    "Dell-operations-workstation",
                    HealthSeverity::Critical,
                    HealthComponent::Resources,
                ),
                condition(
                    "Basement:firmware-refresh",
                    "Basement",
                    HealthSeverity::Warning,
                    HealthComponent::Firmware,
                ),
                condition(
                    "Surface-Pro-6-conference-room:audio",
                    "Surface-Pro-6-conference-room",
                    HealthSeverity::Warning,
                    HealthComponent::Audio,
                ),
            ]
        } else {
            Vec::new()
        };
        if with_issues {
            for index in 0..8 {
                conditions.push(condition(
                    &format!("Dell-operations-workstation:provider-{index}"),
                    "Dell-operations-workstation",
                    HealthSeverity::Warning,
                    HealthComponent::System,
                ));
            }
        }
        let nodes = [
            "Basement",
            "Dell-operations-workstation",
            "Eagle",
            "T480",
            "Surface-Pro-6-conference-room",
        ];
        let current_node_grades = nodes
            .iter()
            .map(|node| NodeGrade::evaluate(*node, 92, GradeFactors::default(), &conditions, now))
            .collect();
        let critical = conditions
            .iter()
            .filter(|condition| condition.severity == HealthSeverity::Critical)
            .count();
        let warnings = conditions.len() - critical;
        let resolved_conditions = if with_issues {
            let mut resolved = condition(
                "Dell-operations-workstation:firmware-history",
                "Dell-operations-workstation",
                HealthSeverity::Warning,
                HealthComponent::Firmware,
            );
            resolved.resolved_at_ms = Some(now.saturating_sub(30_000));
            vec![resolved]
        } else {
            Vec::new()
        };
        SystemMeshHealthSnapshot {
            schema_version: HEALTH_SCHEMA_VERSION,
            observer: "render-seat".into(),
            roster_revision: "render-roster-42".into(),
            generation: 42,
            generated_at_ms: now,
            fresh_until_ms: if fresh { now.saturating_add(60_000) } else { 0 },
            current_node_grades,
            active_conditions: conditions,
            resolved_conditions,
            mesh_summary: MeshHealthSummary {
                grade: if critical > 0 {
                    GradeLetter::F
                } else if warnings > 0 {
                    GradeLetter::D
                } else {
                    GradeLetter::A
                },
                canonical_nodes: 5,
                fresh_nodes: 5,
                reachable_lighthouses: 3,
                active_warnings: warnings,
                active_critical: critical,
                unacknowledged_actionable: warnings + critical,
            },
        }
    }

    fn fixture_export_authority(snapshot: &SystemMeshHealthSnapshot) -> SupportExportAuthority {
        SupportExportAuthority {
            snapshot_generation: snapshot.generation,
            snapshot_generated_at_ms: snapshot.generated_at_ms,
            node_scope: MESH_SELECTION.into(),
            severity: HistorySeverityFilter::All,
            component: HistoryComponentFilter::All,
            source: None,
            provider: None,
            selected_incident_id: None,
        }
    }

    fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn render_frame(
        ctx: &egui::Context,
        chrome: &mut ConstructChrome,
        snapshot: &SystemMeshHealthSnapshot,
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ctx| mount(ctx, chrome, Some(snapshot)),
        )
    }

    fn write_proof(
        name: &str,
        size: egui::Vec2,
        scheme: mde_egui::StyleColorScheme,
        snapshot: &SystemMeshHealthSnapshot,
        large_text: bool,
        confirm: bool,
    ) {
        let mut snapshot = snapshot.clone();
        let selected = if confirm {
            let local = crate::explorer::local_hostname();
            let id = format!("{local}:guided-confirmation");
            let guided = condition(
                &id,
                &local,
                HealthSeverity::Warning,
                HealthComponent::System,
            );
            snapshot.active_conditions.push(guided);
            snapshot.current_node_grades.push(NodeGrade::evaluate(
                &local,
                90,
                GradeFactors::default(),
                &snapshot.active_conditions,
                now_ms(),
            ));
            snapshot.mesh_summary.active_warnings += 1;
            snapshot.mesh_summary.unacknowledged_actionable += 1;
            local
        } else {
            "Dell-operations-workstation".into()
        };
        let ctx = egui::Context::default();
        Style::install_color_scheme_with_density(&ctx, scheme, mde_egui::Density::Mouse);
        if large_text {
            ctx.style_mut(|style| {
                for font in style.text_styles.values_mut() {
                    font.size *= 1.35;
                }
            });
        }
        let mut chrome = ConstructChrome::default();
        chrome.health_modal_open = true;
        chrome.health_selected_node = Some(selected.clone());
        if confirm {
            chrome.health_pending_action = Some((
                format!("{selected}:guided-confirmation"),
                HealthAction::RefreshProvider,
            ));
        }
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let mut capture = crate::screenshot::Capture::new();
        let _settle = capture.frame(&ctx, input(), |ctx| {
            mount(ctx, &mut chrome, Some(&snapshot));
        });
        let canvas = capture.frame(&ctx, input(), |ctx| {
            mount(ctx, &mut chrome, Some(&snapshot));
        });
        assert!(!canvas.is_blank(), "health proof {name} must paint pixels");
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("screenshots")
            .join(name);
        canvas.write_png(&path).expect("write health modal proof");
        println!("Health modal rendered proof written to {}", path.display());
    }

    #[test]
    fn matrix_order_is_the_locked_five_seat_order() {
        assert_eq!(
            SEATS.map(|seat| seat.0),
            ["Seat 15", "Dell", "Eagle", "T480", "Surface"]
        );
    }

    #[test]
    fn every_action_has_functional_copy() {
        for action in [
            HealthAction::Acknowledge,
            HealthAction::SnoozeOneHour,
            HealthAction::RefreshProvider,
            HealthAction::RestoreWorkstationAudio,
            HealthAction::RefreshFirmwareMetadata,
            HealthAction::RestartMackesd,
            HealthAction::RestartMeshBus,
            HealthAction::RestartNebula,
            HealthAction::RestartSyncthing,
            HealthAction::RestartDns,
            HealthAction::RestartKdc,
            HealthAction::RestartShell,
            HealthAction::ExpandSeat15Root,
            HealthAction::PublishOverlayIp,
            HealthAction::SetupEtcdClient,
            HealthAction::RecoverXdgBinds,
            HealthAction::RunLifecycleFirstboot,
            HealthAction::OpenOnboarding,
            HealthAction::StartNodeVirt,
            HealthAction::StartBrowserVm,
            HealthAction::SetupSyncthing,
        ] {
            assert!(!action_label(action).is_empty());
        }
    }

    #[test]
    fn active_copy_and_badge_have_distinct_acknowledgement_semantics() {
        let mut snapshot = fixture_snapshot(true, true);
        snapshot.active_conditions.truncate(1);
        snapshot.active_conditions[0].acknowledged_at_ms = Some(now_ms());
        assert_eq!(active_condition_count(Some(&snapshot)), 1);
        assert_eq!(snapshot.active_issue_count(now_ms()), 0);

        snapshot.active_conditions[0].acknowledged_at_ms = None;
        snapshot.active_conditions[0].snoozed_until_ms = Some(u64::MAX);
        assert_eq!(active_condition_count(Some(&snapshot)), 1);
        assert_eq!(snapshot.active_issue_count(now_ms()), 0);
    }

    #[test]
    fn status_cells_keep_expected_absence_non_outage_but_refuse_stale_ok() {
        let now = now_ms();
        let mut snapshot = fixture_snapshot(false, true);
        let mut expected_absence = condition(
            "Dell-operations-workstation:expected-absence",
            "Dell-operations-workstation",
            HealthSeverity::Warning,
            HealthComponent::System,
        );
        expected_absence.requirement = RequirementClass::Informational;
        expected_absence.evidence.summary =
            "Dell is sleeping and expected to return within its declared window.".into();
        snapshot.active_conditions.push(expected_absence);
        snapshot.fresh_until_ms = now;

        assert_eq!(
            status_cell_state(
                Some(&snapshot),
                Some("Dell-operations-workstation"),
                &[HealthComponent::System],
                now,
            ),
            StatusCellState::Ok,
            "a current expected absence is information, not an outage"
        );

        assert_eq!(
            status_cell_state(
                Some(&snapshot),
                Some("Dell-operations-workstation"),
                &[HealthComponent::System],
                now.saturating_add(1),
            ),
            StatusCellState::Stale,
            "expired provenance cannot substantiate an OK status"
        );
        assert_eq!(
            status_cell_state(
                None,
                Some("Dell-operations-workstation"),
                &[HealthComponent::System],
                now,
            ),
            StatusCellState::Unavailable,
        );
    }

    #[test]
    fn zero_state_and_escape_are_rendered_and_functional() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let snapshot = fixture_snapshot(false, true);
        let mut chrome = ConstructChrome::default();
        chrome.health_modal_open = true;
        let _ = render_frame(
            &ctx,
            &mut chrome,
            &snapshot,
            egui::vec2(1_200.0, 800.0),
            Vec::new(),
        );
        let output = render_frame(
            &ctx,
            &mut chrome,
            &snapshot,
            egui::vec2(1_200.0, 800.0),
            Vec::new(),
        );
        let text = painted_text(&output.shapes).join(" | ");
        assert!(text.contains("System and Mesh Health"), "{text}");
        assert!(text.contains("0 active issues"), "{text}");
        assert!(
            text.contains("Export redacted support bundle"),
            "the current snapshot exposes its support action: {text}"
        );

        let _ = render_frame(
            &ctx,
            &mut chrome,
            &snapshot,
            egui::vec2(1_200.0, 800.0),
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert!(!chrome.health_modal_open, "Escape closes the modal");
    }

    #[test]
    fn modal_traps_keyboard_focus_away_from_background_content() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let snapshot = fixture_snapshot(true, true);
        let mut chrome = ConstructChrome::default();
        chrome.health_modal_open = true;
        let size = egui::vec2(1_200.0, 800.0);
        let mut outside_id = None;
        for events in [
            Vec::new(),
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
        ] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        outside_id = Some(ui.button("Background action").id);
                    });
                    mount(ctx, &mut chrome, Some(&snapshot));
                },
            );
        }
        let focused = ctx.memory(|memory| memory.focused());
        assert!(focused.is_some(), "Tab establishes focus inside the modal");
        assert_ne!(
            focused, outside_id,
            "modal focus cannot escape to background content"
        );
    }

    #[test]
    fn device_deep_links_are_narrowly_allowlisted() {
        assert_eq!(
            device_route_key("device-manager?device=pci-0000:00:1f.3"),
            Some("pci-0000:00:1f.3")
        );
        assert_eq!(device_route_key("device-manager?device="), None);
        assert_eq!(device_route_key("diagnostics?device=pci-0"), None);
    }

    #[test]
    fn critical_auto_open_queues_and_reopens_only_after_recurrence() {
        let critical = fixture_snapshot(true, true);
        let healthy = fixture_snapshot(false, true);
        let now = now_ms();
        let mut chrome = ConstructChrome::default();

        chrome.observe_health(Some(&critical), now, true);
        assert!(
            !chrome.health_modal_open,
            "blocked critical opening is queued"
        );
        chrome.observe_health(Some(&critical), now, false);
        assert!(
            chrome.health_modal_open,
            "queued critical opens when the block clears"
        );

        chrome.health_modal_open = false;
        chrome.observe_health(Some(&critical), now, false);
        assert!(
            !chrome.health_modal_open,
            "the same occurrence never reopens"
        );

        chrome.observe_health(Some(&healthy), now, false);
        chrome.observe_health(Some(&critical), now, false);
        assert!(
            chrome.health_modal_open,
            "a resolved and recurring critical opens once again"
        );
    }

    #[test]
    fn resolution_durations_use_elapsed_boundaries_not_wall_clock_time() {
        assert_eq!(format_health_duration_ms(0), "0s");
        assert_eq!(format_health_duration_ms(59_999), "59s");
        assert_eq!(format_health_duration_ms(3_599_000), "59m 59s");
        assert_eq!(format_health_duration_ms(3_600_000), "01:00:00");
        assert_eq!(format_health_duration_ms(86_400_000), "24:00:00");
        assert_eq!(format_health_duration_ms(172_801_000), "2d 00:00:01");
    }

    #[test]
    fn history_sorts_critical_before_warning_then_longest_duration() {
        let mut short_critical = condition(
            "critical-short",
            "node",
            HealthSeverity::Critical,
            HealthComponent::System,
        );
        short_critical.resolved_at_ms = Some(2_000);
        let mut long_critical = condition(
            "critical-long",
            "node",
            HealthSeverity::Critical,
            HealthComponent::System,
        );
        long_critical.active_since_ms = 0;
        long_critical.resolved_at_ms = Some(10_000);
        let mut long_warning = condition(
            "warning-long",
            "node",
            HealthSeverity::Warning,
            HealthComponent::System,
        );
        long_warning.active_since_ms = 0;
        long_warning.resolved_at_ms = Some(20_000);
        let conditions = vec![long_warning, short_critical, long_critical];

        let ordered = recurrence_history(&conditions, "node", 20_000);
        assert_eq!(
            ordered
                .iter()
                .map(|recurrence| recurrence.condition.id.as_str())
                .collect::<Vec<_>>(),
            ["critical-long", "critical-short", "warning-long"]
        );
    }

    #[test]
    fn live_snapshot_reorder_or_removal_never_moves_selection() {
        let mut initial = fixture_snapshot(false, true);
        let selected = initial.current_node_grades[0].node.clone();
        let mut chrome = ConstructChrome::default();

        stabilize_selection(&mut chrome, Some(&initial));
        assert_eq!(
            chrome.health_selected_node.as_deref(),
            Some(selected.as_str())
        );

        initial.current_node_grades.reverse();
        assert_ne!(initial.current_node_grades[0].node, selected);
        stabilize_selection(&mut chrome, Some(&initial));
        assert_eq!(
            chrome.health_selected_node.as_deref(),
            Some(selected.as_str()),
            "a live reorder must not silently switch the detail pane"
        );

        initial
            .current_node_grades
            .retain(|grade| grade.node != selected);
        stabilize_selection(&mut chrome, Some(&initial));
        assert_eq!(
            chrome.health_selected_node.as_deref(),
            Some(selected.as_str()),
            "temporary node disappearance must retain the operator's selection"
        );
    }

    #[test]
    fn lower_generation_live_update_cannot_replace_admitted_health_authority() {
        let ctx = egui::Context::default();
        let trusted = fixture_snapshot(true, true);
        assert_eq!(
            admit_modal_snapshot(&ctx, Some(&trusted)),
            Some(trusted.clone())
        );

        let mut rollback = trusted.clone();
        rollback.generation = rollback.generation.saturating_sub(1);
        rollback.generated_at_ms = trusted.generated_at_ms.saturating_add(1);
        rollback.fresh_until_ms = rollback.generated_at_ms.saturating_add(60_000);
        rollback.active_conditions.clear();
        rollback.mesh_summary.active_warnings = 0;
        rollback.mesh_summary.active_critical = 0;
        assert_eq!(
            admit_modal_snapshot(&ctx, Some(&rollback)),
            Some(trusted.clone()),
            "a fresh-looking rollback cannot erase the last admitted outage"
        );

        let mut forward = trusted;
        forward.generation = forward.generation.saturating_add(1);
        forward.generated_at_ms = forward.generated_at_ms.saturating_add(1);
        forward.fresh_until_ms = forward.generated_at_ms.saturating_add(60_000);
        assert_eq!(
            admit_modal_snapshot(&ctx, Some(&forward)),
            Some(forward),
            "only a timestamp-advancing generation can replace modal authority"
        );
    }

    #[test]
    fn history_materialization_is_node_scoped_and_hard_bounded() {
        let mut conditions = Vec::new();
        for index in 0..64 {
            let mut resolved = condition(
                &format!("node:resolved-{index}"),
                "node",
                HealthSeverity::Warning,
                HealthComponent::System,
            );
            resolved.resolved_at_ms = Some(2_000 + index);
            conditions.push(resolved);
        }
        let mut short_critical = condition(
            "node:critical-short",
            "node",
            HealthSeverity::Critical,
            HealthComponent::System,
        );
        short_critical.resolved_at_ms = Some(2_000);
        conditions.push(short_critical);
        let mut long_critical = condition(
            "node:critical-long",
            "node",
            HealthSeverity::Critical,
            HealthComponent::System,
        );
        long_critical.active_since_ms = 0;
        long_critical.resolved_at_ms = Some(10_000);
        conditions.push(long_critical);
        let mut other = condition(
            "other:critical",
            "other",
            HealthSeverity::Critical,
            HealthComponent::System,
        );
        other.resolved_at_ms = Some(9_999);
        conditions.push(other);

        let page = recurrence_history(&conditions, "node", 100_000);
        assert_eq!(page.len(), 8, "one paint materializes at most eight rows");
        assert_eq!(
            page.iter()
                .map(|recurrence| recurrence.condition.id.as_str())
                .collect::<Vec<_>>(),
            [
                "node:critical-long",
                "node:critical-short",
                "node:resolved-63",
                "node:resolved-62",
                "node:resolved-61",
                "node:resolved-60",
                "node:resolved-59",
                "node:resolved-58",
            ],
            "bounded insertion retains the same severity/duration/id ordering"
        );
        assert!(page.iter().all(|recurrence| {
            matches!(&recurrence.condition.scope, HealthScope::Node { node } if node == "node")
        }));
    }

    #[test]
    fn history_severity_filters_apply_before_the_bounded_recurrence_page() {
        let mut conditions = Vec::new();
        for index in 0..32 {
            let severity = if index % 2 == 0 {
                HealthSeverity::Warning
            } else {
                HealthSeverity::Critical
            };
            let mut resolved = condition(
                &format!("node:resolved-{index:02}"),
                "node",
                severity,
                HealthComponent::System,
            );
            resolved.resolved_at_ms = Some(2_000 + index);
            conditions.push(resolved);
        }

        for (filter, expected) in [
            (HistorySeverityFilter::Warning, HealthSeverity::Warning),
            (HistorySeverityFilter::Critical, HealthSeverity::Critical),
        ] {
            let page = filtered_recurrence_history(&conditions, "node", 100_000, filter);
            assert_eq!(
                page.len(),
                HISTORY_PAGE_SIZE,
                "each filter fills one bounded page from matching records"
            );
            assert!(page
                .iter()
                .all(|recurrence| recurrence.condition.severity == expected));
        }

        assert_eq!(
            filtered_recurrence_history(
                &conditions,
                "missing-node",
                100_000,
                HistorySeverityFilter::Critical,
            )
            .len(),
            0,
            "a filter never widens node scope"
        );
    }

    #[test]
    fn history_pages_expose_later_rows_and_clamp_live_shrink() {
        let mut conditions = Vec::new();
        for index in 0..19 {
            let mut resolved = condition(
                &format!("node:resolved-{index:02}"),
                "node",
                HealthSeverity::Warning,
                HealthComponent::System,
            );
            resolved.resolved_at_ms = Some(2_000 + index);
            conditions.push(resolved);
        }

        let second = paged_recurrence_history(
            &conditions,
            "node",
            100_000,
            HistorySeverityFilter::All,
            HistoryComponentFilter::All,
            None,
            None,
            1,
        );
        assert_eq!(second.total, 19);
        assert_eq!(second.rows.len(), HISTORY_PAGE_SIZE);
        assert_eq!(second.rows[0].condition.id, "node:resolved-10");

        let stale_page = paged_recurrence_history(
            &conditions[..3],
            "node",
            100_000,
            HistorySeverityFilter::All,
            HistoryComponentFilter::All,
            None,
            None,
            2,
        );
        assert_eq!(stale_page.total, 3);
        assert!(stale_page.rows.is_empty());
        let clamped = paged_recurrence_history(
            &conditions[..3],
            "node",
            100_000,
            HistorySeverityFilter::All,
            HistoryComponentFilter::All,
            None,
            None,
            0,
        );
        assert_eq!(clamped.rows.len(), 3);
    }

    #[test]
    fn history_component_and_severity_filters_compose_before_paging() {
        let mut conditions = Vec::new();
        for index in 0..32 {
            let mut resolved = condition(
                &format!("node:resolved-{index:02}"),
                "node",
                if index % 2 == 0 {
                    HealthSeverity::Critical
                } else {
                    HealthSeverity::Warning
                },
                if index % 4 < 2 {
                    HealthComponent::Devices
                } else {
                    HealthComponent::Audio
                },
            );
            resolved.resolved_at_ms = Some(2_000 + index);
            conditions.push(resolved);
        }

        let page = paged_recurrence_history(
            &conditions,
            "node",
            100_000,
            HistorySeverityFilter::Critical,
            HistoryComponentFilter::Component(HealthComponent::Devices),
            None,
            None,
            0,
        );
        assert_eq!(page.total, 8, "the intersection is counted before paging");
        assert_eq!(page.rows.len(), HISTORY_PAGE_SIZE);
        assert!(page.rows.iter().all(|recurrence| {
            recurrence.condition.severity == HealthSeverity::Critical
                && recurrence.condition.component == HealthComponent::Devices
        }));

        let ctx = egui::Context::default();
        let stale = HistoryPageState {
            node: "node".into(),
            filter: HistorySeverityFilter::Critical,
            component: HistoryComponentFilter::Component(HealthComponent::Audio),
            source: Some("old-source".into()),
            provider: Some("old-provider".into()),
            page: 3,
        };
        set_history_page_state(&ctx, stale);
        assert_eq!(
            history_page_state(
                &ctx,
                "node",
                HistorySeverityFilter::Critical,
                HistoryComponentFilter::Component(HealthComponent::Devices),
                Some("new-source"),
                Some("new-provider"),
            )
            .page,
            0,
            "changing the component dimension resets stale page authority"
        );
    }

    #[test]
    fn history_source_and_provider_filters_compose_before_recurrence_and_paging() {
        let mut conditions = Vec::new();
        for index in 0..40 {
            let mut resolved = condition(
                &format!("node:resolved-{:02}", index % 10),
                "node",
                if index % 2 == 0 {
                    HealthSeverity::Critical
                } else {
                    HealthSeverity::Warning
                },
                if index % 4 < 2 {
                    HealthComponent::Devices
                } else {
                    HealthComponent::Audio
                },
            );
            resolved.source = if index % 5 == 0 {
                "remote-probe".into()
            } else {
                "local-probe".into()
            };
            resolved.evidence.provider = if index % 10 == 0 {
                "provider-a".into()
            } else {
                "provider-b".into()
            };
            resolved.resolved_at_ms = Some(2_000 + index);
            conditions.push(resolved);
        }

        let page = paged_recurrence_history(
            &conditions,
            "node",
            100_000,
            HistorySeverityFilter::Critical,
            HistoryComponentFilter::Component(HealthComponent::Devices),
            Some("remote-probe"),
            Some("provider-a"),
            0,
        );
        assert_eq!(
            page.total, 1,
            "all four dimensions precede identity aggregation"
        );
        assert_eq!(
            page.rows[0].occurrences, 2,
            "matching recurrences remain complete"
        );
        assert!(page.rows.iter().all(|recurrence| {
            recurrence.condition.source == "remote-probe"
                && recurrence.condition.evidence.provider == "provider-a"
                && recurrence.condition.severity == HealthSeverity::Critical
                && recurrence.condition.component == HealthComponent::Devices
        }));

        let choices = history_origin_choices(&conditions, "node", 100_000, |condition| {
            &condition.evidence.provider
        });
        assert_eq!(choices, ["provider-a", "provider-b"]);

        let mut active = condition(
            "node:active-provider-a",
            "node",
            HealthSeverity::Critical,
            HealthComponent::Devices,
        );
        active.source = "remote-probe".into();
        active.evidence.provider = "provider-a".into();
        conditions.push(active);
        assert_eq!(
            paged_recurrence_history(
                &conditions,
                "node",
                100_000,
                HistorySeverityFilter::All,
                HistoryComponentFilter::All,
                Some("remote-probe"),
                Some("provider-a"),
                0,
            )
            .total,
            1,
            "history filtering cannot absorb or duplicate active conditions"
        );
    }

    #[test]
    fn resolved_history_selection_keeps_exact_identity_across_reorder_filter_and_page_changes() {
        let ctx = egui::Context::default();
        let mut selected = condition(
            "node:selected-incident",
            "node",
            HealthSeverity::Warning,
            HealthComponent::Devices,
        );
        selected.source = "selected-source".into();
        selected.resolved_at_ms = Some(9_000);
        selected.last_observed_ms = 9_000;
        let mut substitute = condition(
            "node:substitute-incident",
            "node",
            HealthSeverity::Critical,
            HealthComponent::System,
        );
        substitute.resolved_at_ms = Some(10_000);
        substitute.last_observed_ms = 10_000;
        let mut conditions = vec![selected.clone(), substitute.clone()];

        set_history_selection(&ctx, "node", &selected.id);
        conditions.reverse();
        assert_eq!(history_selection(&ctx, "node"), Some(selected.id.clone()));
        assert_eq!(
            selected_history_condition(&conditions, "node", 10_000, &selected.id)
                .map(|condition| condition.id.as_str()),
            Some(selected.id.as_str()),
            "live reorder must resolve detail by stable incident identity"
        );

        let filtered_page = paged_recurrence_history(
            &conditions,
            "node",
            10_000,
            HistorySeverityFilter::Critical,
            HistoryComponentFilter::Component(HealthComponent::System),
            None,
            None,
            7,
        );
        assert!(filtered_page.rows.is_empty());
        assert_eq!(history_selection(&ctx, "node"), Some(selected.id.clone()));
        assert_eq!(
            selected_history_condition(&conditions, "node", 10_000, &selected.id)
                .map(|condition| condition.source.as_str()),
            Some("selected-source"),
            "filter and page state cannot replace selected detail"
        );

        conditions.retain(|condition| condition.id != selected.id);
        assert!(selected_history_condition(&conditions, "node", 10_000, &selected.id).is_none());
        assert_eq!(history_selection(&ctx, "node"), Some(selected.id));
        assert_ne!(
            selected_history_condition(&conditions, "node", 10_000, "node:selected-incident")
                .map(|condition| condition.id.as_str()),
            Some(substitute.id.as_str()),
            "a vanished incident must not substitute the next visible row"
        );
        assert_eq!(history_selection(&ctx, "other-node"), None);
    }

    #[test]
    fn history_excludes_active_rows_from_the_resolved_page() {
        let active = condition(
            "node:active-with-resolution",
            "node",
            HealthSeverity::Critical,
            HealthComponent::System,
        );

        let mut resolved = active.clone();
        resolved.id = "node:resolved".into();
        resolved.resolved_at_ms = Some(2_000);
        resolved.last_observed_ms = 2_000;

        let conditions = [active, resolved];
        let page = recurrence_history(&conditions, "node", 2_000);
        assert_eq!(
            page.iter()
                .map(|recurrence| recurrence.condition.id.as_str())
                .collect::<Vec<_>>(),
            ["node:resolved"],
            "history must contain only inactive lifecycle rows"
        );
    }

    #[test]
    fn history_excludes_resolutions_before_the_last_observation() {
        let mut contradictory = condition(
            "node:contradictory-resolution",
            "node",
            HealthSeverity::Critical,
            HealthComponent::System,
        );
        contradictory.last_observed_ms = 3_000;
        contradictory.resolved_at_ms = Some(2_000);

        assert!(
            recurrence_history(&[contradictory], "node", 3_000).is_empty(),
            "a resolution cannot precede the condition's final observation"
        );
    }

    #[test]
    fn recurrence_aggregation_is_bounded_complete_and_order_independent() {
        let mut conditions = Vec::new();
        for index in 0..32 {
            let mut resolved = condition(
                &format!("node:warning-{index:02}"),
                "node",
                HealthSeverity::Warning,
                HealthComponent::System,
            );
            resolved.resolved_at_ms = Some(2_000 + index);
            conditions.push(resolved);
        }
        for duration in [9_000, 3_000, 7_000] {
            let mut recurrence = condition(
                "node:recurring-critical",
                "node",
                HealthSeverity::Critical,
                HealthComponent::Resources,
            );
            recurrence.active_since_ms = 1_000;
            recurrence.resolved_at_ms = Some(1_000 + duration);
            conditions.push(recurrence);
        }
        let mut equal_duration_later = condition(
            "node:recurring-critical",
            "node",
            HealthSeverity::Critical,
            HealthComponent::Resources,
        );
        equal_duration_later.active_since_ms = 2_000;
        equal_duration_later.resolved_at_ms = Some(11_000);
        conditions.push(equal_duration_later);
        let mut wrong_node = condition(
            "node:recurring-critical",
            "other",
            HealthSeverity::Critical,
            HealthComponent::Resources,
        );
        wrong_node.resolved_at_ms = Some(u64::MAX);
        conditions.push(wrong_node);

        let mut reversed_conditions = conditions.clone();
        reversed_conditions.reverse();
        let forward = recurrence_history(&conditions, "node", 20_000);
        let reversed = recurrence_history(&reversed_conditions, "node", 20_000);
        let summarize = |page: &[HistoryRecurrence<'_>]| {
            page.iter()
                .map(|recurrence| {
                    (
                        recurrence.condition.id.clone(),
                        recurrence.occurrences,
                        resolution_duration_ms(recurrence.condition),
                        recurrence.condition.resolved_at_ms,
                    )
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(forward.len(), HISTORY_PAGE_SIZE);
        assert_eq!(summarize(&forward), summarize(&reversed));
        assert_eq!(
            summarize(&forward)[0],
            (
                "node:recurring-critical".to_string(),
                4,
                Some(9_000),
                Some(11_000)
            ),
            "all same-node occurrences are counted and equal durations choose the latest resolution"
        );
        assert_eq!(
            forward
                .iter()
                .filter(|recurrence| recurrence.condition.id == "node:recurring-critical")
                .count(),
            1,
            "a stable lifecycle identity occupies exactly one bounded row"
        );
    }

    #[test]
    fn history_filter_rejects_out_of_window_future_and_unresolved_records() {
        let as_of_ms = HISTORY_WINDOW_MS + 10_000;
        let mut at_boundary = condition(
            "node:at-boundary",
            "node",
            HealthSeverity::Warning,
            HealthComponent::System,
        );
        at_boundary.resolved_at_ms = Some(as_of_ms - HISTORY_WINDOW_MS);

        let mut current = condition(
            "node:current",
            "node",
            HealthSeverity::Warning,
            HealthComponent::System,
        );
        current.resolved_at_ms = Some(as_of_ms);

        let mut too_old = condition(
            "node:too-old-critical",
            "node",
            HealthSeverity::Critical,
            HealthComponent::System,
        );
        too_old.resolved_at_ms = Some(as_of_ms - HISTORY_WINDOW_MS - 1);

        let mut future = condition(
            "node:future-critical",
            "node",
            HealthSeverity::Critical,
            HealthComponent::System,
        );
        future.resolved_at_ms = Some(as_of_ms + 1);

        let unresolved = condition(
            "node:unresolved-critical",
            "node",
            HealthSeverity::Critical,
            HealthComponent::System,
        );

        let conditions = vec![too_old, future, unresolved, at_boundary, current];
        let page = recurrence_history(&conditions, "node", as_of_ms);
        assert_eq!(
            page.iter()
                .map(|recurrence| recurrence.condition.id.as_str())
                .collect::<Vec<_>>(),
            ["node:current", "node:at-boundary"],
            "only genuinely resolved records inside the inclusive 24-hour window belong on the page"
        );
    }

    #[test]
    fn hostile_health_projection_cannot_render_secret_or_path_material() {
        let local = crate::explorer::local_hostname();
        let mut snapshot = fixture_snapshot(false, true);
        snapshot.current_node_grades.clear();

        let mut active = condition(
            "hostile-active",
            &local,
            HealthSeverity::Warning,
            HealthComponent::System,
        );
        active.evidence.summary = "token=active-secret".into();
        active.evidence.provider = "/etc/shadow".into();
        active.remediation[0].impact = "Authorization: Bearer recovery-secret".into();

        let mut expected_absence = condition(
            "hostile-expected-absence",
            &local,
            HealthSeverity::Warning,
            HealthComponent::System,
        );
        expected_absence.requirement = RequirementClass::Informational;
        expected_absence.evidence.summary = "password=expected-state-secret".into();
        expected_absence.remediation.clear();

        let mut history = condition(
            "hostile-history",
            &local,
            HealthSeverity::Critical,
            HealthComponent::System,
        );
        history.evidence.summary = "private_key=history-secret".into();
        history.active_since_ms = snapshot.generated_at_ms.saturating_sub(2_000);
        history.last_observed_ms = snapshot.generated_at_ms.saturating_sub(1_000);
        history.evidence.observed_at_ms = history.last_observed_ms;
        history.resolved_at_ms = Some(snapshot.generated_at_ms);
        history.remediation.clear();

        snapshot.active_conditions = vec![active, expected_absence];
        snapshot.resolved_conditions = vec![history];

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut chrome = ConstructChrome::default();
        chrome.health_selected_node = Some(local);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                detail(ui, &mut chrome, Some(&snapshot));
            });
        });
        let text = painted_text(&output.shapes).join(" | ");

        for forbidden in [
            "active-secret",
            "/etc/shadow",
            "recovery-secret",
            "expected-state-secret",
            "history-secret",
        ] {
            assert!(
                !text.contains(forbidden),
                "render leaked {forbidden:?}: {text}"
            );
        }
        assert!(
            text.matches("[redacted]").count() >= 5,
            "every hostile active, expected-state, history, provider, and recovery string fails closed: {text}"
        );
    }

    #[test]
    fn action_publication_reports_hostile_failures_and_preserves_bound_success() {
        let snapshot = fixture_snapshot(true, true);
        let condition = &snapshot.active_conditions[0];
        let HealthScope::Node { node } = &condition.scope else {
            panic!("fixture condition must be node-scoped")
        };
        let pending = (condition.id.clone(), HealthAction::RefreshProvider);
        let ctx = egui::Context::default();
        let mut chrome = ConstructChrome::default();
        chrome.health_pending_action = Some(pending.clone());

        let unavailable = publish_action_to(
            None,
            &snapshot,
            condition,
            node,
            HealthAction::RefreshProvider,
            true,
        );
        assert_eq!(
            unavailable,
            ActionPublishOutcome::Failed(ActionPublishFailure::BusRootUnavailable)
        );
        apply_action_outcome(&ctx, &mut chrome, true, unavailable);
        assert_eq!(chrome.health_pending_action, Some(pending.clone()));
        assert_eq!(
            action_error(&ctx),
            Some(ActionPublishFailure::BusRootUnavailable)
        );

        let blocked_root = tempfile::tempdir().expect("blocked Bus fixture");
        Persist::open(blocked_root.path().to_path_buf()).expect("initialize Bus index");
        std::fs::write(blocked_root.path().join("action"), b"not a directory")
            .expect("block action topic directory");
        let write_failed = publish_action_to(
            Some(blocked_root.path().to_path_buf()),
            &snapshot,
            condition,
            node,
            HealthAction::RefreshProvider,
            true,
        );
        assert_eq!(
            write_failed,
            ActionPublishOutcome::Failed(ActionPublishFailure::PersistWrite)
        );
        apply_action_outcome(&ctx, &mut chrome, true, write_failed);
        assert_eq!(chrome.health_pending_action, Some(pending.clone()));
        assert_eq!(action_error(&ctx), Some(ActionPublishFailure::PersistWrite));

        let live_root = tempfile::tempdir().expect("writable Bus fixture");
        let published = publish_action_to(
            Some(live_root.path().to_path_buf()),
            &snapshot,
            condition,
            node,
            HealthAction::RefreshProvider,
            true,
        );
        assert!(matches!(&published, ActionPublishOutcome::Published(_)));
        let persist = Persist::open(live_root.path().to_path_buf()).expect("reopen Bus");
        let messages = persist
            .list_since(ACTION_TOPIC, None)
            .expect("read published action");
        assert_eq!(messages.len(), 1);
        let request: HealthActionRequest =
            serde_json::from_str(messages[0].body.as_deref().expect("action request body"))
                .expect("decode action request");
        assert_eq!(request.target, HealthScope::Node { node: node.clone() });
        assert_eq!(request.expected_snapshot_generation, snapshot.generation);
        assert_eq!(request.condition_id, condition.id);
        assert_eq!(request.confirmation.as_deref(), Some("CONFIRM"));

        apply_action_outcome(&ctx, &mut chrome, true, published);
        assert_eq!(chrome.health_pending_action, None);
        assert_eq!(action_error(&ctx), None);
    }

    #[test]
    fn governed_action_publication_requires_current_exact_generation_bound_authority() {
        let mut snapshot = fixture_snapshot(true, true);
        let mut mesh_condition = snapshot.active_conditions[0].clone();
        mesh_condition.id = "mesh:canonical-recovery".into();
        mesh_condition.scope = HealthScope::Mesh;
        mesh_condition.remediation[0].target = HealthScope::Mesh;
        snapshot.active_conditions.push(mesh_condition);
        let condition = &snapshot.active_conditions[0];
        let HealthScope::Node { node } = &condition.scope else {
            panic!("fixture condition must be node-scoped")
        };
        let root = tempfile::tempdir().expect("governed action Bus fixture");
        let persist = Persist::open(root.path().to_path_buf()).expect("initialize Bus fixture");

        assert_eq!(
            publish_action_to(
                Some(root.path().to_path_buf()),
                &snapshot,
                condition,
                "different-node",
                HealthAction::RefreshProvider,
                true,
            ),
            ActionPublishOutcome::Failed(ActionPublishFailure::TargetMismatch),
            "a caller-selected node cannot replace canonical condition scope"
        );

        let mut forged = condition.clone();
        forged.source = "forged-source".into();
        assert_eq!(
            publish_action_to(
                Some(root.path().to_path_buf()),
                &snapshot,
                &forged,
                node,
                HealthAction::RefreshProvider,
                true,
            ),
            ActionPublishOutcome::Failed(ActionPublishFailure::ConditionNotCurrent),
            "authority must come from the exact canonical active record"
        );

        let mut stale = snapshot.clone();
        stale.fresh_until_ms = 0;
        assert_eq!(
            publish_action_to(
                Some(root.path().to_path_buf()),
                &stale,
                &stale.active_conditions[0],
                node,
                HealthAction::RefreshProvider,
                true,
            ),
            ActionPublishOutcome::Failed(ActionPublishFailure::StaleSnapshot),
            "expired modal state cannot authorize a recovery"
        );

        assert_eq!(
            publish_action_to(
                Some(root.path().to_path_buf()),
                &snapshot,
                condition,
                node,
                HealthAction::RefreshProvider,
                false,
            ),
            ActionPublishOutcome::Failed(ActionPublishFailure::ConfirmationRequired),
            "a generation-bound descriptor retains its confirmation policy"
        );
        assert_eq!(
            publish_action_to(
                Some(root.path().to_path_buf()),
                &snapshot,
                condition,
                node,
                HealthAction::RestartNebula,
                true,
            ),
            ActionPublishOutcome::Failed(ActionPublishFailure::ActionNotAuthorized),
            "an allowlisted enum is not authority unless this condition offers it"
        );
        assert!(
            persist
                .list_since(ACTION_TOPIC, None)
                .expect("inspect refused publication lane")
                .is_empty(),
            "every refused request must fail before a Bus side effect"
        );

        assert!(matches!(
            publish_action_to(
                Some(root.path().to_path_buf()),
                &snapshot,
                condition,
                node,
                HealthAction::RefreshProvider,
                true,
            ),
            ActionPublishOutcome::Published(_)
        ));
        let messages = persist
            .list_since(ACTION_TOPIC, None)
            .expect("read authorized publication");
        assert_eq!(messages.len(), 1);
        let request: HealthActionRequest = serde_json::from_str(
            messages[0]
                .body
                .as_deref()
                .expect("authorized action has a body"),
        )
        .expect("decode authorized action");
        assert_eq!(request.target, condition.scope);
        assert_eq!(request.expected_snapshot_generation, snapshot.generation);
        assert_eq!(request.confirmation.as_deref(), Some("CONFIRM"));

        let mesh_condition = snapshot
            .active_conditions
            .last()
            .expect("mesh authority fixture");
        assert_eq!(mesh_condition.scope, HealthScope::Mesh);
        assert!(matches!(
            publish_action_to(
                Some(root.path().to_path_buf()),
                &snapshot,
                mesh_condition,
                MESH_SELECTION,
                HealthAction::RefreshProvider,
                true,
            ),
            ActionPublishOutcome::Published(_)
        ), "mesh authority derives a Mesh target only through the explicit mesh selection convention");
        let messages = persist
            .list_since(ACTION_TOPIC, None)
            .expect("read node and mesh publications");
        assert_eq!(messages.len(), 2);
        let mesh_request: HealthActionRequest =
            serde_json::from_str(messages[1].body.as_deref().expect("mesh action has a body"))
                .expect("decode mesh action");
        assert_eq!(mesh_request.target, HealthScope::Mesh);
    }

    #[test]
    fn action_result_progress_binds_identity_generation_target_and_reports_partial_failure() {
        let mut snapshot = fixture_snapshot(true, true);
        let local = crate::explorer::local_hostname();
        let mut condition = snapshot.active_conditions[0].clone();
        condition.id = format!("{local}:result-progress");
        condition.scope = HealthScope::Node {
            node: local.clone(),
        };
        condition.remediation[0].target = condition.scope.clone();
        snapshot.active_conditions[0] = condition.clone();
        let HealthScope::Node { node } = &condition.scope else {
            panic!("fixture condition must be node-scoped")
        };
        let root = tempfile::tempdir().expect("action-result Bus fixture");
        let outcome = publish_action_to(
            Some(root.path().to_path_buf()),
            &snapshot,
            &condition,
            node,
            HealthAction::RefreshProvider,
            true,
        );
        let ActionPublishOutcome::Published(request) = outcome.clone() else {
            panic!("governed request must publish")
        };
        let ctx = egui::Context::default();
        let mut chrome = ConstructChrome::default();
        chrome.health_pending_action = Some((condition.id.clone(), request.action));
        apply_action_outcome(&ctx, &mut chrome, true, outcome);
        assert_eq!(chrome.health_pending_action, None);
        assert_eq!(
            pending_health_action(&ctx),
            Some(PendingHealthAction {
                request: request.clone(),
                result: None,
            }),
            "durable publication starts an exact result-tracked request"
        );

        let persist = Persist::open(root.path().to_path_buf()).expect("open result fixture");
        let topic = action_result_topic(&request.request_id);
        let write_result = |result: &HealthActionResult| {
            persist
                .write(
                    &topic,
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(result).expect("encode result fixture")),
                )
                .expect("publish result fixture");
        };
        let valid_shape = HealthActionResult {
            schema_version: HEALTH_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            condition_id: request.condition_id.clone(),
            action: request.action,
            outcome: HealthActionOutcome::Failed,
            detail: "governed result detail".into(),
            audit_id: format!("health:{node}:01J00000000000000000000009"),
            completed_at_ms: now_ms(),
            snapshot_generation: request.expected_snapshot_generation,
            refreshed_evidence: None,
        };
        let oversized = HealthActionResult {
            detail: "x".repeat(mackes_mesh_types::health::MAX_HEALTH_TEXT_BYTES + 1),
            ..valid_shape.clone()
        };
        write_result(&oversized);
        let mut unknown = serde_json::to_value(&valid_shape).expect("encode hostile result");
        unknown
            .as_object_mut()
            .expect("result object")
            .insert("untrusted_extension".into(), serde_json::json!(true));
        persist
            .write(
                &topic,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&unknown).expect("encode unknown-field result")),
            )
            .expect("publish unknown-field fixture");
        persist
            .write(
                &topic,
                Priority::Default,
                None,
                Some("{\"schema_version\":1,\"request_id\":false}"),
            )
            .expect("publish malformed fixture");
        assert_eq!(
            poll_action_result(Some(root.path().to_path_buf()), &request),
            ActionResultPoll::Blocked(ActionResultPollIssue::UnverifiedResult),
            "malformed, oversized, and unknown-field rows cannot be presented"
        );
        let completed_at_ms = now_ms();
        let unrelated_target = HealthActionResult {
            schema_version: HEALTH_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            condition_id: request.condition_id.clone(),
            action: request.action,
            outcome: HealthActionOutcome::Failed,
            detail: "unrelated target failed".into(),
            audit_id: "health:different-node:01J00000000000000000000000".into(),
            completed_at_ms,
            snapshot_generation: request.expected_snapshot_generation,
            refreshed_evidence: None,
        };
        write_result(&unrelated_target);
        let stale_generation = HealthActionResult {
            audit_id: format!("health:{node}:01J00000000000000000000000"),
            snapshot_generation: request.expected_snapshot_generation.saturating_sub(1),
            detail: "stale result".into(),
            ..unrelated_target.clone()
        };
        write_result(&stale_generation);
        assert_eq!(
            poll_action_result(Some(root.path().to_path_buf()), &request),
            ActionResultPoll::Blocked(ActionResultPollIssue::UnverifiedResult),
            "wrong-target and stale-generation rows cannot complete the pending request"
        );
        assert_eq!(
            pending_health_action(&ctx)
                .expect("request remains tracked")
                .result,
            None
        );

        let applied = HealthActionResult {
            schema_version: HEALTH_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            condition_id: request.condition_id.clone(),
            action: request.action,
            outcome: HealthActionOutcome::Applied,
            detail: "provider refresh completed".into(),
            audit_id: format!("health:{node}:01J00000000000000000000001"),
            completed_at_ms: now_ms(),
            snapshot_generation: request.expected_snapshot_generation + 1,
            refreshed_evidence: Some(condition.evidence.clone()),
        };
        write_result(&applied);
        assert_eq!(
            poll_action_result(Some(root.path().to_path_buf()), &request),
            ActionResultPoll::Matched(applied.clone()),
            "only the exact identity-, generation-, action-, and target-bound result is admitted"
        );

        let mut pending = pending_health_action(&ctx).expect("pending request state");
        pending.result = Some(applied.clone());
        set_pending_health_action(&ctx, pending.clone());
        snapshot.generation = applied.snapshot_generation;
        snapshot.generated_at_ms = now_ms();
        snapshot.fresh_until_ms = snapshot.generated_at_ms.saturating_add(60_000);
        let presentation = action_progress_presentation(&pending, Some(&snapshot), None);
        assert_eq!(presentation.tone, ActionProgressTone::Warning);
        assert!(presentation.title.contains("issue still active"));
        assert!(presentation.detail.contains("Current evidence:"));
        assert!(
            !action_progress_is_pending(&ctx, &snapshot),
            "an exact terminal result ends progress without erasing its truthful partial-failure presentation"
        );
        assert_eq!(
            pending_health_action(&ctx)
                .expect("terminal presentation remains available")
                .result,
            Some(applied.clone()),
            "unrelated rows never clear or replace the admitted result"
        );

        let mut cross_node_request = request.clone();
        cross_node_request.requester = "different-requester".into();
        assert!(
            !result_is_bound_to_request(&applied, &cross_node_request, now_ms()),
            "a node result requires the local requester and target to be the same node"
        );

        let mut mesh_snapshot = fixture_snapshot(true, true);
        let mut mesh_condition = mesh_snapshot.active_conditions[0].clone();
        mesh_condition.id = "mesh:genuine-result".into();
        mesh_condition.scope = HealthScope::Mesh;
        mesh_condition.remediation[0].target = HealthScope::Mesh;
        mesh_snapshot.active_conditions.push(mesh_condition.clone());
        let mesh_outcome = publish_action_to(
            Some(root.path().to_path_buf()),
            &mesh_snapshot,
            &mesh_condition,
            MESH_SELECTION,
            HealthAction::RefreshProvider,
            true,
        );
        let ActionPublishOutcome::Published(mesh_request) = mesh_outcome else {
            panic!("governed mesh request must publish")
        };
        let false_mesh_publisher = HealthActionResult {
            schema_version: HEALTH_SCHEMA_VERSION,
            request_id: mesh_request.request_id.clone(),
            condition_id: mesh_request.condition_id.clone(),
            action: mesh_request.action,
            outcome: HealthActionOutcome::Refused,
            detail: "mesh action was refused".into(),
            audit_id: "health:mesh:01J00000000000000000000002".into(),
            completed_at_ms: now_ms(),
            snapshot_generation: mesh_request.expected_snapshot_generation,
            refreshed_evidence: None,
        };
        assert!(
            !result_is_bound_to_request(&false_mesh_publisher, &mesh_request, now_ms()),
            "mesh is a scope, not a result publisher identity"
        );
        let genuine_mesh_result = HealthActionResult {
            audit_id: format!(
                "health:{}:01J00000000000000000000003",
                mesh_request.requester
            ),
            ..false_mesh_publisher
        };
        persist
            .write(
                &action_result_topic(&mesh_request.request_id),
                Priority::Default,
                None,
                Some(
                    &serde_json::to_string(&genuine_mesh_result)
                        .expect("encode genuine mesh result"),
                ),
            )
            .expect("publish genuine mesh result");
        assert_eq!(
            poll_action_result(Some(root.path().to_path_buf()), &mesh_request),
            ActionResultPoll::Matched(genuine_mesh_result),
            "a genuine mesh-scoped result binds to the local requester's node-qualified worker audit"
        );
    }

    #[test]
    fn support_bundle_is_deterministic_byte_bounded_and_redacts_hostile_material() {
        let mut snapshot = fixture_snapshot(true, true);
        snapshot.observer = "Authorization: Bearer observer-secret".into();
        snapshot.roster_revision = "/etc/shadow".into();
        snapshot.current_node_grades.clear();
        snapshot.active_conditions.clear();
        snapshot.resolved_conditions.clear();
        for index in 0..256 {
            let node = format!("../../unsafe-node-{index}-{}", "n".repeat(512));
            snapshot.current_node_grades.push(NodeGrade::evaluate(
                node.clone(),
                90,
                GradeFactors::default(),
                &[],
                snapshot.generated_at_ms,
            ));
            let mut active = condition(
                &format!("condition-{index}-{}", "i".repeat(512)),
                &node,
                HealthSeverity::Warning,
                HealthComponent::System,
            );
            active.source = r"C:\Users\operator\private.txt".into();
            active.evidence.provider = "password=hunter2".into();
            active.evidence.summary = format!("token=top-secret-{index}-{}", "s".repeat(512));
            active.evidence.facts = BTreeMap::from([
                ("api_token".into(), "fact-secret".into()),
                ("safe".into(), "private_key=key-secret".into()),
                ("location".into(), "../../escape".into()),
                ("bounded".into(), "v".repeat(1_024)),
            ]);
            snapshot.active_conditions.push(active.clone());
            active.resolved_at_ms = Some(snapshot.generated_at_ms.saturating_sub(index as u64));
            snapshot.resolved_conditions.push(active);
        }

        let authority = fixture_export_authority(&snapshot);
        let encoded = support_bundle_json(&snapshot, &authority)
            .expect("hostile snapshot remains exportable");
        assert_eq!(
            encoded,
            support_bundle_json(&snapshot, &authority).expect("same snapshot encodes identically"),
            "the bundle is deterministic"
        );
        assert!(encoded.len() <= SUPPORT_BUNDLE_MAX_BYTES);
        let text = String::from_utf8(encoded.clone()).expect("JSON is UTF-8");
        for forbidden in [
            "observer-secret",
            "hunter2",
            "top-secret",
            "fact-secret",
            "key-secret",
            "/etc/shadow",
            "C:\\Users",
            "../../",
        ] {
            assert!(!text.contains(forbidden), "leaked {forbidden:?}: {text}");
        }
        let parsed: serde_json::Value = serde_json::from_slice(&encoded).expect("valid JSON");
        assert_eq!(parsed["schema"], SUPPORT_BUNDLE_SCHEMA);
        assert_eq!(parsed["generated_at_ms"], snapshot.generated_at_ms);
        assert!(parsed["nodes"].as_array().expect("nodes array").len() <= SUPPORT_BUNDLE_MAX_NODES);
        assert!(
            parsed["active_conditions"]
                .as_array()
                .expect("active array")
                .len()
                <= SUPPORT_BUNDLE_MAX_ACTIVE
        );
        assert!(
            parsed["resolved_history"]
                .as_array()
                .expect("resolved array")
                .len()
                <= SUPPORT_BUNDLE_MAX_RESOLVED
        );
        assert_eq!(parsed["snapshot"]["observer"], "[redacted]");
        assert_eq!(parsed["snapshot"]["roster_revision"], "[redacted]");
        assert_eq!(
            parsed["export_authority"]["snapshot_generation"],
            snapshot.generation
        );
    }

    #[test]
    fn support_bundle_materializes_only_the_captured_health_view() {
        let mut snapshot = fixture_snapshot(false, true);
        snapshot.active_conditions.clear();
        snapshot.resolved_conditions.clear();

        let mut active = condition(
            "dell:active",
            "Dell-operations-workstation",
            HealthSeverity::Warning,
            HealthComponent::System,
        );
        active.evidence.summary = "admitted active condition".into();
        snapshot.active_conditions.push(active);

        let mut foreign_active = condition(
            "surface:active-secret",
            "Surface",
            HealthSeverity::Critical,
            HealthComponent::System,
        );
        foreign_active.evidence.summary = "foreign-active-marker".into();
        snapshot.active_conditions.push(foreign_active);

        let mut matching = condition(
            "dell:matching-history",
            "Dell-operations-workstation",
            HealthSeverity::Warning,
            HealthComponent::Firmware,
        );
        matching.source = "firmware-monitor".into();
        matching.evidence.provider = "fwupd".into();
        matching.evidence.summary = "admitted-history-marker".into();
        matching.last_observed_ms = snapshot.generated_at_ms.saturating_sub(2_000);
        matching.resolved_at_ms = Some(snapshot.generated_at_ms.saturating_sub(1_000));
        snapshot.resolved_conditions.push(matching);

        for index in 0..64 {
            let mut hostile = condition(
                &format!("surface:critical-{index}"),
                "Surface",
                HealthSeverity::Critical,
                HealthComponent::Firmware,
            );
            hostile.source = "firmware-monitor".into();
            hostile.evidence.provider = "fwupd".into();
            hostile.evidence.summary = format!("foreign-history-marker-{index}");
            hostile.last_observed_ms = snapshot.generated_at_ms.saturating_sub(2_000);
            hostile.resolved_at_ms = Some(snapshot.generated_at_ms.saturating_sub(1_000));
            snapshot.resolved_conditions.push(hostile);
        }

        let authority = SupportExportAuthority {
            snapshot_generation: snapshot.generation,
            snapshot_generated_at_ms: snapshot.generated_at_ms,
            node_scope: "Dell-operations-workstation".into(),
            severity: HistorySeverityFilter::Warning,
            component: HistoryComponentFilter::Component(HealthComponent::Firmware),
            source: Some("firmware-monitor".into()),
            provider: Some("fwupd".into()),
            selected_incident_id: None,
        };
        let encoded = support_bundle_json(&snapshot, &authority)
            .expect("the captured filtered Health view is exportable");
        let text = String::from_utf8(encoded.clone()).expect("support bundle is UTF-8");
        assert!(text.contains("admitted active condition"));
        assert!(text.contains("admitted-history-marker"));
        assert!(!text.contains("foreign-active-marker"));
        assert!(!text.contains("foreign-history-marker"));

        let parsed: serde_json::Value = serde_json::from_slice(&encoded).expect("valid JSON");
        assert_eq!(parsed["nodes"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            parsed["active_conditions"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(parsed["resolved_history"].as_array().map(Vec::len), Some(1));
        assert_eq!(parsed["limits"]["resolved_in_snapshot"], 65);
        assert_eq!(parsed["limits"]["resolved_exported"], 1);
    }

    #[test]
    fn support_bundle_writer_rejects_escape_and_filename_is_sanitized() {
        let root = tempfile::tempdir().expect("support export fixture");
        let directory = root.path().join("exports");
        let outside = root.path().join("escaped.json");
        let error = write_support_bundle(&directory, "../../escaped.json", b"{}")
            .expect_err("traversal is rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(!outside.exists());

        let safe = sanitize_support_filename("../../host/health?.json");
        assert!(!safe.contains('/') && !safe.contains('\\'));
        let written = write_support_bundle(&directory, &safe, b"{}")
            .expect("sanitized filename remains writable");
        assert_eq!(written.parent(), Some(directory.as_path()));
        assert!(written.starts_with(&directory));
    }

    #[test]
    fn support_bundle_rejects_symlinked_directory_and_destination() {
        use std::os::unix::fs::symlink;
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().expect("support export fixture");
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).expect("outside fixture directory");
        let linked_directory = root.path().join("linked-export");
        symlink(&outside, &linked_directory).expect("preplant export-directory symlink");
        let error = write_support_bundle(&linked_directory, "bundle.json", b"{}")
            .expect_err("symlinked export directory is rejected");
        assert!(
            matches!(
                error.raw_os_error(),
                Some(code) if code == rustix::io::Errno::LOOP.raw_os_error()
                    || code == rustix::io::Errno::NOTDIR.raw_os_error()
            ),
            "unexpected symlink rejection: {error}"
        );
        assert!(!outside.join("bundle.json").exists());

        let directory = root.path().join("real-export");
        std::fs::create_dir(&directory).expect("real export directory");
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .expect("make export directory private");
        let victim = root.path().join("victim");
        std::fs::write(&victim, b"untouched").expect("victim fixture");
        symlink(&victim, directory.join("bundle.json")).expect("preplant destination symlink");
        let error = write_support_bundle(&directory, "bundle.json", b"replacement")
            .expect_err("symlinked destination is rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(std::fs::read(&victim).expect("read victim"), b"untouched");
    }

    #[test]
    fn support_bundle_exclusive_temp_collision_is_not_followed_or_removed() {
        use std::os::unix::fs::symlink;
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().expect("support export fixture");
        let directory = root.path().join("exports");
        std::fs::create_dir(&directory).expect("export fixture directory");
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .expect("make export directory private");
        let victim = root.path().join("victim");
        std::fs::write(&victim, b"untouched").expect("victim fixture");
        let nonce = [0xabu8; 16];
        let temporary = directory.join(format!(
            ".health-support-{}.tmp",
            nonce
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ));
        symlink(&victim, &temporary).expect("preplant temporary symlink");

        let error =
            write_support_bundle_with_nonce(&directory, "bundle.json", b"replacement", nonce)
                .expect_err("exclusive temporary collision is rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&victim).expect("read victim"), b"untouched");
        assert!(
            std::fs::symlink_metadata(&temporary)
                .expect("hostile temporary remains owned by its creator")
                .file_type()
                .is_symlink(),
            "a failed create must not unlink an entry it did not create"
        );
        assert!(!directory.join("bundle.json").exists());
    }

    #[test]
    fn support_export_rejects_replaced_generation_scope_filters_and_incident() {
        let root = tempfile::tempdir().expect("support export authority fixture");
        let snapshot = fixture_snapshot(true, true);
        let incident_id = snapshot.resolved_conditions[0].id.clone();
        let captured = SupportExportAuthority {
            snapshot_generation: snapshot.generation,
            snapshot_generated_at_ms: snapshot.generated_at_ms,
            node_scope: "Dell-operations-workstation".into(),
            severity: HistorySeverityFilter::Warning,
            component: HistoryComponentFilter::Component(HealthComponent::Firmware),
            source: Some("render-proof".into()),
            provider: Some("direct seat poll".into()),
            selected_incident_id: Some(incident_id),
        };
        validate_support_export_authority(&snapshot, &captured)
            .expect("the initiating Health view is admitted");

        let mut replacements = Vec::new();
        let mut generation = captured.clone();
        generation.snapshot_generation += 1;
        replacements.push(("generation", generation));
        let mut scope = captured.clone();
        scope.node_scope = "Basement".into();
        replacements.push(("scope", scope));
        let mut severity = captured.clone();
        severity.severity = HistorySeverityFilter::Critical;
        replacements.push(("severity", severity));
        let mut component = captured.clone();
        component.component = HistoryComponentFilter::Component(HealthComponent::Audio);
        replacements.push(("component", component));
        let mut source = captured.clone();
        source.source = Some("replacement-source".into());
        replacements.push(("source", source));
        let mut provider = captured.clone();
        provider.provider = Some("replacement-provider".into());
        replacements.push(("provider", provider));
        let mut incident = captured.clone();
        incident.selected_incident_id = Some("Dell-operations-workstation:replacement".into());
        replacements.push(("incident", incident));

        for (label, current) in replacements {
            let directory = root.path().join(label);
            let error = export_support_bundle_to(&directory, &snapshot, &captured, &current)
                .expect_err("replaced live Health authority must fail closed");
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            assert!(
                !directory.exists(),
                "{label} replacement reached the durable write boundary"
            );
        }
    }

    #[test]
    fn support_bundle_export_writes_atomic_round_trip_json() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().expect("support export fixture");
        let directory = root.path().join("exports");
        let snapshot = fixture_snapshot(true, true);
        let authority = fixture_export_authority(&snapshot);
        let path = export_support_bundle_to(&directory, &snapshot, &authority, &authority)
            .expect("real export succeeds");
        assert_eq!(path.parent(), Some(directory.as_path()));
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(support_bundle_filename(&snapshot).as_str())
        );
        let encoded = std::fs::read(&path).expect("read completed export");
        let parsed: serde_json::Value = serde_json::from_slice(&encoded).expect("round-trip JSON");
        assert_eq!(parsed["schema"], SUPPORT_BUNDLE_SCHEMA);
        assert_eq!(parsed["snapshot"]["generation"], snapshot.generation);
        assert_eq!(parsed["mesh_summary"]["canonical_nodes"], 5);
        assert!(
            std::fs::read_dir(&directory)
                .expect("read export directory")
                .all(|entry| !entry
                    .expect("directory entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".health-support-")),
            "successful persistence leaves no temporary sibling"
        );
        assert_eq!(
            std::fs::metadata(&path)
                .expect("export metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "durable bundle remains private to its user"
        );
        std::fs::File::open(&path)
            .expect("reopen durable bundle")
            .sync_all()
            .expect("completed bundle supports a persistence barrier");
        std::fs::File::open(&directory)
            .expect("reopen durable parent")
            .sync_all()
            .expect("parent directory supports a persistence barrier");
    }

    #[test]
    fn support_bundle_write_failure_is_preserved_in_modal_state() {
        let root = tempfile::tempdir().expect("support export fixture");
        let blocker = root.path().join("not-a-directory");
        std::fs::write(&blocker, b"block directory creation").expect("write blocker");
        let snapshot = fixture_snapshot(false, true);
        let authority = fixture_export_authority(&snapshot);
        let failed =
            export_support_bundle_to(&blocker.join("exports"), &snapshot, &authority, &authority);
        assert!(failed.is_err(), "the hostile write must fail");

        let ctx = egui::Context::default();
        apply_support_export_result(&ctx, failed);
        let Some(SupportExportOutcome::Failed(message)) = support_export_outcome(&ctx) else {
            panic!("the modal must preserve an honest export failure")
        };
        assert!(!message.is_empty());
        assert!(message.len() <= SUPPORT_BUNDLE_MAX_TEXT_BYTES);
    }

    #[test]
    fn redaction_fails_closed_before_scanning_oversized_evidence() {
        let oversized = "safe-looking ".repeat(MAX_REDACTION_SCAN_BYTES);
        assert_eq!(redact_support_text(&oversized), "[redacted]");
        assert_eq!(redact_support_text("safe summary"), "safe summary");
    }

    #[test]
    fn rendered_proof_covers_theme_width_density_and_condition_states() {
        let zero = fixture_snapshot(false, true);
        let issues = fixture_snapshot(true, true);
        let stale = fixture_snapshot(true, false);
        write_proof(
            "health-dark-zero-desktop.png",
            egui::vec2(1_440.0, 900.0),
            mde_egui::StyleColorScheme::Dark,
            &zero,
            false,
            false,
        );
        write_proof(
            "health-light-zero-desktop.png",
            egui::vec2(1_440.0, 900.0),
            mde_egui::StyleColorScheme::Light,
            &zero,
            false,
            false,
        );
        write_proof(
            "health-dark-many-guided-desktop.png",
            egui::vec2(1_440.0, 900.0),
            mde_egui::StyleColorScheme::Dark,
            &issues,
            false,
            true,
        );
        write_proof(
            "health-dark-many-narrow.png",
            egui::vec2(480.0, 760.0),
            mde_egui::StyleColorScheme::Dark,
            &issues,
            false,
            false,
        );
        write_proof(
            "health-light-stale-large-text.png",
            egui::vec2(1_024.0, 768.0),
            mde_egui::StyleColorScheme::Light,
            &stale,
            true,
            false,
        );
    }
}
