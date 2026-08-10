//! Centered System and Mesh Health modal.

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mackes_mesh_types::health::{
    format_health_duration_ms, HealthAction, HealthActionRequest, HealthComponent, HealthCondition,
    HealthScope, HealthSeverity, NodeGrade, SystemMeshHealthSnapshot, ACTION_TOPIC,
    HEALTH_SCHEMA_VERSION,
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
const SUPPORT_BUNDLE_MAX_FILENAME_BYTES: usize = 128;
const HISTORY_FILTER_STATE_ID: &str = "health-history-severity-filter";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum HistorySeverityFilter {
    #[default]
    All,
    Warning,
    Critical,
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
            apply_support_export_result(ui.ctx(), export_support_bundle(snapshot));
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
                condition_card(ui, chrome, snapshot, condition, "");
            }
        } else {
            ui.label("The health provider has not published a current mesh summary.");
        }
        return;
    }
    ui.heading(&node);
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
            ui.label(&condition.evidence.summary);
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
            let resolved = filtered_recurrence_history(
                &snapshot.resolved_conditions,
                &node,
                snapshot.generated_at_ms,
                filter,
            );
            if resolved.is_empty() {
                ui.colored_label(Style::TEXT_DIM, "No history matches this filter.");
            }
            for recurrence in resolved {
                let condition = recurrence.condition;
                let recurrence_copy = if recurrence.occurrences == 1 {
                    "once".to_string()
                } else {
                    format!("{} times", recurrence.occurrences)
                };
                ui.label(format!(
                    "{} · occurred {recurrence_copy} · resolved {} · duration {}",
                    condition.evidence.summary,
                    condition
                        .resolved_at_ms
                        .map_or_else(|| "—".into(), format_timestamp),
                    resolution_duration_ms(condition)
                        .map_or_else(|| "unknown".to_string(), format_health_duration_ms),
                ));
            }
        }
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
        ui.strong(&condition.evidence.summary);
        ui.label(Style::typography_text(
            format!(
                "{} · observed {}",
                condition.evidence.provider,
                format_timestamp(condition.last_observed_ms)
            ),
            TypographyRole::Caption,
        ));
        let local = crate::explorer::local_hostname();
        let actionable_here =
            matches!(&condition.scope, HealthScope::Node { node: target } if target == &local);
        ui.horizontal_wrapped(|ui| {
            if actionable_here && ui.small_button("Acknowledge").clicked() {
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
            if actionable_here && ui.small_button("Snooze 1 hour").clicked() {
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
            ui.label(&action.impact);
            let label = action_label(action.action);
            if action.confirmation_required {
                if ui.button(label).clicked() {
                    chrome.health_pending_action = Some((condition.id.clone(), action.action));
                }
            } else if ui.button(label).clicked() {
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
                    if ui.button("Confirm action").clicked() {
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

fn export_support_bundle(snapshot: &SystemMeshHealthSnapshot) -> std::io::Result<PathBuf> {
    export_support_bundle_to(&support_export_dir()?, snapshot)
}

fn export_support_bundle_to(
    directory: &Path,
    snapshot: &SystemMeshHealthSnapshot,
) -> std::io::Result<PathBuf> {
    let encoded = support_bundle_json(snapshot)?;
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

fn support_bundle_json(snapshot: &SystemMeshHealthSnapshot) -> std::io::Result<Vec<u8>> {
    let mut nodes = bounded_sorted_clones(
        snapshot.current_node_grades.iter(),
        SUPPORT_BUNDLE_MAX_NODES,
        |left, right| {
            left.node
                .cmp(&right.node)
                .then_with(|| left.evaluated_at_ms.cmp(&right.evaluated_at_ms))
        },
    );

    let mut active = bounded_sorted_clones(
        snapshot
            .active_conditions
            .iter()
            .filter(|condition| condition.is_active()),
        SUPPORT_BUNDLE_MAX_ACTIVE,
        support_condition_order,
    );

    let window_start = snapshot.generated_at_ms.saturating_sub(HISTORY_WINDOW_MS);
    let mut resolved = bounded_sorted_clones(
        snapshot.resolved_conditions.iter().filter(|condition| {
            condition.resolved_at_ms.is_some_and(|resolved_at| {
                (window_start..=snapshot.generated_at_ms).contains(&resolved_at)
            })
        }),
        SUPPORT_BUNDLE_MAX_RESOLVED,
        support_condition_order,
    );

    loop {
        let value = support_bundle_value(snapshot, &nodes, &active, &resolved);
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
    if credential_shaped(value) || unsafe_path_shaped(value) {
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
    BusRootUnavailable,
    PersistOpen,
    Serialization,
    PersistWrite,
}

impl ActionPublishFailure {
    const fn presentable(self) -> &'static str {
        match self {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActionPublishOutcome {
    Published,
    Failed(ActionPublishFailure),
}

fn action_error_id() -> egui::Id {
    egui::Id::new(ACTION_ERROR_STATE_ID)
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

fn apply_action_outcome(
    ctx: &egui::Context,
    chrome: &mut ConstructChrome,
    clear_confirmation_on_success: bool,
    outcome: ActionPublishOutcome,
) {
    match outcome {
        ActionPublishOutcome::Published => {
            clear_action_error(ctx);
            if clear_confirmation_on_success {
                chrome.health_pending_action = None;
            }
        }
        ActionPublishOutcome::Failed(error) => {
            ctx.data_mut(|data| data.insert_temp(action_error_id(), Some(error)));
        }
    }
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
    let outcome = publish_action(snapshot, condition, node, action, confirmed);
    apply_action_outcome(ctx, chrome, confirmed, outcome);
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
    let request = HealthActionRequest {
        schema_version: HEALTH_SCHEMA_VERSION,
        request_id: format!("health-{now}-{}", condition.id.replace(':', "-")),
        condition_id: condition.id.clone(),
        action,
        target: HealthScope::Node { node: node.into() },
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
        Ok(_) => ActionPublishOutcome::Published,
        Err(_) => ActionPublishOutcome::Failed(ActionPublishFailure::PersistWrite),
    }
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

struct HistoryRecurrence<'a> {
    condition: &'a HealthCondition,
    occurrences: usize,
}

/// Aggregate stable lifecycle identities without materializing the complete
/// history in the modal. Only genuinely resolved records in the snapshot's
/// inclusive 24-hour window participate. The first pass retains only the
/// strongest eight identities; the second pass counts recurrences for those
/// retained rows. This keeps paint-time memory fixed even if an untrusted
/// caller bypasses the snapshot's wire-level collection bound.
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
    let window_start_ms = as_of_ms.saturating_sub(HISTORY_WINDOW_MS);
    let applies_to_page = |condition: &HealthCondition| {
        matches!(&condition.scope, HealthScope::Node { node: target } if target.as_str() == node)
            && filter.admits(condition.severity)
            && condition.resolved_at_ms.is_some_and(|resolved_at_ms| {
                (window_start_ms..=as_of_ms).contains(&resolved_at_ms)
            })
    };
    let mut resolved: Vec<HistoryRecurrence<'a>> = Vec::with_capacity(HISTORY_PAGE_SIZE);
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
        if insert_at >= HISTORY_PAGE_SIZE {
            continue;
        }
        resolved.insert(
            insert_at,
            HistoryRecurrence {
                condition,
                occurrences: 0,
            },
        );
        if resolved.len() > HISTORY_PAGE_SIZE {
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
    resolved
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
    fn action_publication_reports_hostile_failures_and_preserves_bound_success() {
        let snapshot = fixture_snapshot(true, true);
        let condition = &snapshot.active_conditions[0];
        let pending = (condition.id.clone(), HealthAction::RefreshProvider);
        let ctx = egui::Context::default();
        let mut chrome = ConstructChrome::default();
        chrome.health_pending_action = Some(pending.clone());

        let unavailable = publish_action_to(
            None,
            &snapshot,
            condition,
            "bound-target",
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
            "bound-target",
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
            "bound-target",
            HealthAction::RefreshProvider,
            true,
        );
        assert_eq!(published, ActionPublishOutcome::Published);
        let persist = Persist::open(live_root.path().to_path_buf()).expect("reopen Bus");
        let messages = persist
            .list_since(ACTION_TOPIC, None)
            .expect("read published action");
        assert_eq!(messages.len(), 1);
        let request: HealthActionRequest =
            serde_json::from_str(messages[0].body.as_deref().expect("action request body"))
                .expect("decode action request");
        assert_eq!(
            request.target,
            HealthScope::Node {
                node: "bound-target".into()
            }
        );
        assert_eq!(request.expected_snapshot_generation, snapshot.generation);
        assert_eq!(request.condition_id, condition.id);
        assert_eq!(request.confirmation.as_deref(), Some("CONFIRM"));

        apply_action_outcome(&ctx, &mut chrome, true, published);
        assert_eq!(chrome.health_pending_action, None);
        assert_eq!(action_error(&ctx), None);
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

        let encoded = support_bundle_json(&snapshot).expect("hostile snapshot remains exportable");
        assert_eq!(
            encoded,
            support_bundle_json(&snapshot).expect("same snapshot encodes identically"),
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
    fn support_bundle_export_writes_atomic_round_trip_json() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().expect("support export fixture");
        let directory = root.path().join("exports");
        let snapshot = fixture_snapshot(true, true);
        let path = export_support_bundle_to(&directory, &snapshot).expect("real export succeeds");
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
        let failed = export_support_bundle_to(&blocker.join("exports"), &snapshot);
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
