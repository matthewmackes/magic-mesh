//! U18 — the **Status** lens: day-2 per-node backend health (`OpenTofu` / Ansible /
//! libvirt, honestly probed off each node's `state/cloud` mirror), an aggregate
//! desired-vs-actual **drift** rollup, the live **workload metrics** (CPU / mem /
//! disk from [`mackes_mesh_types::cloud::WorkloadRow`]), and the session audit
//! trail (its permanent home). Every state reads honestly (§7) — an off-mesh Bus
//! is an empty roster, an absent tool is `absent` not a fabricated `up`.

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::{carbon_icon, Style};

use mackes_mesh_types::cloud::{CloudState, DriftFlag, HealthState, WorkloadRow};

use super::WorkloadsState;

/// The Status lens's own state (U18 reads entirely from the folded mirror + the
/// preserved audit trail, so it holds no fields of its own).
#[derive(Debug, Default)]
pub(super) struct State;

/// Render the Status lens — backend health, drift, live workload metrics, and the
/// preserved session audit trail. Read-only: it emits no verb (§7).
#[allow(clippy::needless_pass_by_ref_mut)]
pub(super) fn status_panel(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    super::mirror_summary(ui, state);
    ui.add_space(Style::SP_S);

    let nodes = state.states();

    // ── per-node backend health ──
    lens_heading(ui, "emblem-ok", "Backend health");
    if nodes.is_empty() {
        mde_egui::muted_note(
            ui,
            "No node has published a state/cloud mirror yet \u{2014} backend health appears once a \
             placement node reports.",
        );
    } else {
        for node in nodes {
            node_health_card(ui, node);
        }
    }
    ui.add_space(Style::SP_S);

    // ── aggregate desired-state drift ──
    lens_heading(ui, "view-refresh", "Desired-state drift");
    drift_rollup(ui, nodes);
    ui.add_space(Style::SP_S);

    // ── live workload metrics ──
    lens_heading(ui, "view-grid", "Workloads \u{2014} live metrics");
    let mut any = false;
    for node in nodes {
        for workload in &node.workloads {
            workload_metric_row(ui, workload);
            any = true;
        }
    }
    if !any {
        mde_egui::muted_note(
            ui,
            "No workloads reported \u{2014} live metrics fill once a placement node folds its \
             domains into the mirror.",
        );
    }
    ui.add_space(Style::SP_S);

    // ── the preserved session audit trail (its permanent home) ──
    super::render_audit(ui, &state.audit);
}

/// One node's backend-tool health card — its host, the aggregate `backend_ready`
/// verdict, the plan-only vs apply-armed mode, and the three tool rows.
fn node_health_card(ui: &mut egui::Ui, node: &CloudState) {
    mde_egui::card().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(&node.host)
                    .size(Style::BODY)
                    .strong()
                    .color(Style::TEXT),
            );
            ui.add_space(Style::SP_S);
            let (word, tone) = if node.backend_ready() {
                ("backend ready", Style::SUPPORT_SUCCESS)
            } else {
                ("backend not ready", Style::SUPPORT_WARNING)
            };
            ui.label(RichText::new(word).size(Style::SMALL).color(tone).strong());
            ui.add_space(Style::SP_S);
            let (mode, mode_tone) = if node.apply_armed {
                ("apply armed", Style::ACCENT)
            } else {
                ("plan-only", Style::TEXT_DIM)
            };
            ui.label(RichText::new(mode).size(Style::SMALL).color(mode_tone));
        });
        ui.add_space(Style::SP_XS);
        for (svc, label) in [
            ("opentofu", "OpenTofu"),
            ("ansible", "Ansible"),
            ("libvirt", "libvirt"),
        ] {
            tool_health_row(ui, node, svc, label);
        }
    });
    ui.add_space(Style::SP_S);
}

/// One backend-tool health row — a state dot + the tool label + the honest
/// Up/Down/Absent word + the probe latency. A tool the mirror never reported
/// reads "not reported", never a fabricated state (§7).
fn tool_health_row(ui: &mut egui::Ui, node: &CloudState, svc: &str, label: &str) {
    let health = node.tool_health(svc);
    ui.horizontal_wrapped(|ui| {
        let (dot, word, tone) = match health.map(|h| h.state) {
            Some(HealthState::Up) => (Style::SUPPORT_SUCCESS, "up", Style::SUPPORT_SUCCESS),
            Some(HealthState::Down) => (Style::SUPPORT_ERROR, "down", Style::SUPPORT_ERROR),
            Some(HealthState::Absent) => (Style::SUPPORT_WARNING, "absent", Style::SUPPORT_WARNING),
            None => (Style::TEXT_DIM, "not reported", Style::TEXT_DIM),
        };
        mde_egui::status_dot(ui, dot);
        ui.add_space(Style::SP_XS);
        ui.label(RichText::new(label).size(Style::SMALL).color(Style::TEXT));
        ui.add_space(Style::SP_S);
        ui.label(RichText::new(word).size(Style::SMALL).color(tone));
        if let Some(ms) = health.and_then(|h| h.latency_ms) {
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new(format!("{ms} ms"))
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
        }
    });
}

/// The aggregate drift rollup across every node — the drifted-workload count (green
/// when in sync, amber when drifted) and the last-plan recency.
fn drift_rollup(ui: &mut egui::Ui, nodes: &[CloudState]) {
    let drift_count: u32 = nodes.iter().map(|n| n.drift_summary.drift_count).sum();
    let workloads: usize = nodes.iter().map(|n| n.workloads.len()).sum();
    let last_plan = nodes
        .iter()
        .map(|n| n.drift_summary.last_plan_ms)
        .max()
        .unwrap_or(0);
    mde_egui::inset().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let (dot, word, tone) = if workloads == 0 {
                (
                    Style::TEXT_DIM,
                    "no workloads to plan".to_string(),
                    Style::TEXT_DIM,
                )
            } else if drift_count == 0 {
                (
                    Style::SUPPORT_SUCCESS,
                    "in sync \u{2014} 0 workloads drifted from desired".to_string(),
                    Style::SUPPORT_SUCCESS,
                )
            } else {
                (
                    Style::SUPPORT_WARNING,
                    format!("{drift_count} workload(s) drifted from desired"),
                    Style::SUPPORT_WARNING,
                )
            };
            mde_egui::status_dot(ui, dot);
            ui.add_space(Style::SP_XS);
            ui.label(RichText::new(word).size(Style::SMALL).color(tone).strong());
            ui.add_space(Style::SP_M);
            ui.label(
                RichText::new(plan_age_label(last_plan))
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
        });
    });
}

/// One workload's live-metric row — a reachability dot, its identity, status, the
/// CPU / mem / disk figures, and its drift badge.
fn workload_metric_row(ui: &mut egui::Ui, workload: &WorkloadRow) {
    mde_egui::inset().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            mde_egui::status_dot(
                ui,
                if workload.reachable {
                    Style::SUPPORT_SUCCESS
                } else {
                    Style::SUPPORT_ERROR
                },
            );
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(&workload.name)
                    .size(Style::BODY)
                    .strong()
                    .color(Style::TEXT),
            );
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new(workload.delivery_type.label())
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new(format!("on {}", workload.node))
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_S);
            let status_tone = if workload.status.eq_ignore_ascii_case("running") {
                Style::SUPPORT_SUCCESS
            } else {
                Style::TEXT_DIM
            };
            ui.label(
                RichText::new(&workload.status)
                    .size(Style::SMALL)
                    .color(status_tone),
            );
        });
        ui.add_space(Style::SP_XS);
        ui.horizontal_wrapped(|ui| {
            metric(ui, "cpu", &format!("{}%", workload.cpu_pct));
            metric(ui, "mem", &format!("{} MiB", workload.mem_mb));
            metric(ui, "disk", &format!("{} GiB", workload.disk_gb));
            let (word, tone) = drift_badge(workload.drift);
            ui.label(RichText::new(word).size(Style::SMALL).color(tone).strong());
        });
    });
    ui.add_space(Style::SP_XS);
}

/// One inline metric — a dim label + its value, with a trailing gutter.
fn metric(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(
        RichText::new(label)
            .size(Style::SMALL)
            .color(Style::TEXT_DIM),
    );
    ui.add_space(Style::SP_XS);
    ui.label(RichText::new(value).size(Style::SMALL).color(Style::TEXT));
    ui.add_space(Style::SP_M);
}

/// The drift-flag badge word + its status tone (green in-sync · amber drift · dim
/// unplanned).
const fn drift_badge(flag: DriftFlag) -> (&'static str, Color32) {
    match flag {
        DriftFlag::InSync => ("in sync", Style::SUPPORT_SUCCESS),
        DriftFlag::Drift => ("drift", Style::SUPPORT_WARNING),
        DriftFlag::Unknown => ("unplanned", Style::TEXT_DIM),
    }
}

/// An honest recency label for the last drift plan (ms since the Unix epoch);
/// `not yet planned` when the node has never planned.
#[allow(clippy::cast_sign_loss)]
fn plan_age_label(last_plan_ms: i64) -> String {
    if last_plan_ms <= 0 {
        return "not yet planned".to_string();
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let age_s = now_ms.saturating_sub(last_plan_ms.max(0) as u128) / 1000;
    if age_s < 60 {
        format!("last plan {age_s}s ago")
    } else if age_s < 3600 {
        format!("last plan {}m ago", age_s / 60)
    } else {
        format!("last plan {}h ago", age_s / 3600)
    }
}

/// A lens section heading — a Workloads-accent Carbon glyph + a strong label.
fn lens_heading(ui: &mut egui::Ui, icon: &str, label: &str) {
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            ui.visuals_mut().override_text_color = Some(Style::ACCENT_WORKLOADS);
            carbon_icon(ui, icon, Style::ICON_M);
        });
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(label)
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
    });
    ui.add_space(Style::SP_XS);
}
