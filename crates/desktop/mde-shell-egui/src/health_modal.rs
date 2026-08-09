//! Centered System and Mesh Health modal.

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

pub(crate) fn mount(
    ctx: &egui::Context,
    chrome: &mut ConstructChrome,
    snapshot: Option<&SystemMeshHealthSnapshot>,
) {
    if !chrome.health_modal_open {
        chrome.health_pending_action = None;
        return;
    }
    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        chrome.health_modal_open = false;
        chrome.health_pending_action = None;
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
    });
    ui.separator();

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
    let worst = snapshot
        .into_iter()
        .flat_map(|snapshot| snapshot.active_conditions.iter())
        .filter(|condition| {
            condition.requirement == mackes_mesh_types::health::RequirementClass::Required
                && components.contains(&condition.component)
                && match (&condition.scope, node) {
                    (HealthScope::Node { node: target }, Some(node)) => target == node,
                    (HealthScope::Mesh, None) => true,
                    _ => false,
                }
        })
        .map(|condition| condition.severity)
        .max();
    let (text, color) = match worst {
        Some(HealthSeverity::Critical) => ("Critical", Style::SUPPORT_ERROR),
        Some(HealthSeverity::Warning) => ("Warning", Style::SUPPORT_WARNING),
        None if snapshot.is_some() => ("OK", Style::SUPPORT_SUCCESS),
        None => ("—", Style::TEXT_DIM),
    };
    ui.colored_label(color, text);
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
        let resolved = recurrence_history(&snapshot.resolved_conditions, &node);
        if !resolved.is_empty() {
            ui.separator();
            ui.strong("Recent History");
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
                publish_action(snapshot, condition, node, HealthAction::Acknowledge, false);
            }
            if actionable_here && ui.small_button("Snooze 1 hour").clicked() {
                publish_action(
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
                publish_action(snapshot, condition, node, action.action, false);
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
                        publish_action(snapshot, condition, node, action.action, true);
                        chrome.health_pending_action = None;
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

fn publish_action(
    snapshot: &SystemMeshHealthSnapshot,
    condition: &HealthCondition,
    node: &str,
    action: HealthAction,
    confirmed: bool,
) {
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
    let Some(root) = mde_bus::client_data_dir() else {
        return;
    };
    let Ok(persist) = Persist::open(root) else {
        return;
    };
    if let Ok(body) = serde_json::to_string(&request) {
        let _ = persist.write(ACTION_TOPIC, Priority::Default, None, Some(&body));
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

struct HistoryRecurrence<'a> {
    condition: &'a HealthCondition,
    occurrences: usize,
}

/// Aggregate stable lifecycle identities without materializing the complete
/// history in the modal. The first pass retains only the strongest eight
/// identities; the second pass counts recurrences for those retained rows.
/// This keeps paint-time memory fixed even if an untrusted caller bypasses the
/// snapshot's wire-level collection bound.
fn recurrence_history<'a>(
    conditions: &'a [HealthCondition],
    node: &str,
) -> Vec<HistoryRecurrence<'a>> {
    let applies_to_node = |condition: &HealthCondition| matches!(&condition.scope, HealthScope::Node { node: target } if target.as_str() == node);
    let mut resolved: Vec<HistoryRecurrence<'a>> = Vec::with_capacity(HISTORY_PAGE_SIZE);
    for condition in conditions
        .iter()
        .filter(|condition| applies_to_node(condition))
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
        .filter(|condition| applies_to_node(condition))
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

        let ordered = recurrence_history(&conditions, "node");
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

        let page = recurrence_history(&conditions, "node");
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
        let forward = recurrence_history(&conditions, "node");
        let reversed = recurrence_history(&reversed_conditions, "node");
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
