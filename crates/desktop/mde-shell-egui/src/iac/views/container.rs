//! U16 — the **Service Container** delivery view: Podman / Quadlet service
//! containers (rootless by default), installed as systemd units by the
//! `container-deploy` verb. Unlike the VM views these are **not** libvirt domains,
//! so the `instance-*` virsh verbs do not apply; per-container lifecycle
//! (restart / logs / destroy) rides the dedicated Podman/systemd action verbs.
//! The roster and its actions are real (live status · drift · metrics), with
//! action outcomes surfaced through the shared Workloads status note.

use mackes_mesh_types::cloud::{DriftFlag, WorkloadRow};
use mde_egui::egui::{self, Color32, RichText};
use mde_egui::{carbon_icon, card, field, inset, muted_note, status_dot, Style};

use super::super::{row_button, DeliveryView, WorkloadsRoute, WorkloadsState};

/// The Service Container view's own state (U16 owns its fields).
#[derive(Debug, Default)]
pub(in crate::iac) struct State;

/// Render the Service Container view — the Podman/Quadlet roster + per-container
/// lifecycle affordances.
pub(super) fn view(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    heading(
        ui,
        "Container",
        "Podman / Quadlet service containers (rootless), installed as systemd units.",
    );
    provision_cta(ui, state, "Deploy a container");

    let rows: Vec<WorkloadRow> = state
        .workloads_of(DeliveryView::ServiceContainer)
        .cloned()
        .collect();
    if rows.is_empty() {
        crate::empty_state::show(
            ui,
            "No containers yet",
            "A container appears here once a placement node reports a service_container workload in \
             its state/cloud mirror.",
        );
        return;
    }
    for row in &rows {
        container_card(ui, state, row);
    }
    muted_note(
        ui,
        "Restart / Logs / Destroy ride the placement node's Podman + systemd (Quadlet) \
         lifecycle actions. Requests retain this row's node and container identity; outcomes \
         appear in the shared action status above.",
    );
}

/// One container card — name · `rootless` tag · reachability · status · drift, the
/// metrics, then the Podman/systemd lifecycle verbs (destructive ones typed-armed).
fn container_card(ui: &mut egui::Ui, state: &mut WorkloadsState, row: &WorkloadRow) {
    card().show(ui, |ui| {
        header_row(ui, row);
        metrics_line(ui, row);
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            if row_button(ui, "Restart", false).clicked() {
                state.issue_lifecycle_direct("container-restart", &row.node, &row.name, &row.name);
            }
            if row_button(ui, "Logs", false).clicked() {
                state.issue_lifecycle_direct("container-logs", &row.node, &row.name, &row.name);
            }
            if row_button(ui, "Destroy\u{2026}", true).clicked() {
                state.issue_lifecycle_direct("container-destroy", &row.node, &row.name, &row.name);
            }
        });
    });
    ui.add_space(Style::SP_S);
}

// ─────────────────────────── shared row grammar ─────────────────────────────

/// The card's identity row: name (strong), the `rootless` tag, the mesh
/// reachability chip, the live-status dot + word, the drift chip, then the node.
fn header_row(ui: &mut egui::Ui, row: &WorkloadRow) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&row.name)
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);
        tag(ui, "rootless");
        ui.add_space(Style::SP_M);
        reach_chip(ui, row.reachable);
        ui.add_space(Style::SP_M);
        let tone = status_tone(&row.status);
        status_dot(ui, tone);
        ui.colored_label(tone, RichText::new(&row.status).size(Style::SMALL));
        ui.add_space(Style::SP_M);
        drift_chip(ui, row.drift);
        ui.add_space(Style::SP_M);
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(format!("on {}", row.node)).size(Style::SMALL),
        );
    });
}

/// A small recessed delivery tag — the `inset` well around a dim caption.
fn tag(ui: &mut egui::Ui, label: &str) {
    inset().show(ui, |ui| {
        ui.label(
            RichText::new(label)
                .size(Style::SMALL)
                .color(Style::ACCENT_WORKLOADS),
        );
    });
}

/// The mesh-reachability chip — an exposed container that isn't on the overlay is
/// a real problem (a warning), never fabricated as up.
fn reach_chip(ui: &mut egui::Ui, reachable: bool) {
    let (tone, word) = if reachable {
        (Style::SUPPORT_SUCCESS, "on mesh")
    } else {
        (Style::WARN, "off mesh")
    };
    status_dot(ui, tone);
    ui.colored_label(tone, RichText::new(word).size(Style::SMALL));
}

/// The live cpu / mem / disk metrics row (cpu toned by load).
fn metrics_line(ui: &mut egui::Ui, row: &WorkloadRow) {
    ui.horizontal(|ui| {
        field(
            ui,
            "cpu",
            &format!("{}%", row.cpu_pct),
            load_tone(row.cpu_pct),
        );
        ui.add_space(Style::SP_M);
        field(ui, "mem", &mem_label(row.mem_mb), Style::TEXT);
        ui.add_space(Style::SP_M);
        field(ui, "disk", &format!("{} GiB", row.disk_gb), Style::TEXT);
    });
}

/// A drift chip — a Style SUPPORT_* dot + word for desired-vs-actual state.
fn drift_chip(ui: &mut egui::Ui, drift: DriftFlag) {
    let tone = drift_tone(drift);
    status_dot(ui, tone);
    ui.colored_label(tone, RichText::new(drift_word(drift)).size(Style::SMALL));
}

/// The view heading — the Workloads-accent glyph + title + a one-line blurb.
fn heading(ui: &mut egui::Ui, title: &str, blurb: &str) {
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            ui.visuals_mut().override_text_color = Some(Style::ACCENT_WORKLOADS);
            carbon_icon(ui, DeliveryView::ServiceContainer.icon(), Style::ICON_S);
        });
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(title)
                .size(Style::TITLE)
                .strong()
                .color(Style::ACCENT_WORKLOADS),
        );
    });
    muted_note(ui, blurb);
    ui.add_space(Style::SP_S);
}

/// The "deploy a container" affordance — jumps to the Provision route (U14
/// placement + U15 form, which renders the container-deploy path).
fn provision_cta(ui: &mut egui::Ui, state: &mut WorkloadsState, label: &str) {
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            ui.visuals_mut().override_text_color = Some(Style::ACCENT_WORKLOADS);
            carbon_icon(ui, "list-add", Style::BODY);
        });
        ui.add_space(Style::SP_XS);
        if ui
            .add(egui::Button::new(
                RichText::new(label)
                    .size(Style::SMALL)
                    .color(Style::ACCENT_WORKLOADS),
            ))
            .clicked()
        {
            state.set_route(WorkloadsRoute::Provision);
        }
    });
    ui.add_space(Style::SP_S);
}

/// The Style tone a live container status paints.
fn status_tone(status: &str) -> Color32 {
    match status.trim().to_ascii_lowercase().as_str() {
        "running" | "active" => Style::SUPPORT_SUCCESS,
        "paused" | "created" => Style::WARN,
        s if s.contains("error") || s.contains("fail") || s.contains("dead") => Style::DANGER,
        _ => Style::TEXT_DIM,
    }
}

/// The Style tone a drift flag paints (drift chips use the SUPPORT_* tokens).
const fn drift_tone(drift: DriftFlag) -> Color32 {
    match drift {
        DriftFlag::InSync => Style::SUPPORT_SUCCESS,
        DriftFlag::Drift => Style::SUPPORT_WARNING,
        DriftFlag::Unknown => Style::TEXT_DIM,
    }
}

/// The drift chip's word.
const fn drift_word(drift: DriftFlag) -> &'static str {
    match drift {
        DriftFlag::InSync => "in sync",
        DriftFlag::Drift => "drift",
        DriftFlag::Unknown => "unplanned",
    }
}

/// The Style tone a cpu-load percentage paints (amber past 70, red past 90).
const fn load_tone(pct: u16) -> Color32 {
    if pct >= 90 {
        Style::DANGER
    } else if pct >= 70 {
        Style::WARN
    } else {
        Style::TEXT
    }
}

/// A memory figure as MiB, or one-decimal GiB past a gibibyte — integer-only so
/// clippy's cast lints stay quiet.
fn mem_label(mb: u32) -> String {
    if mb >= 1024 {
        format!("{}.{} GiB", mb / 1024, (mb % 1024) * 10 / 1024)
    } else {
        format!("{mb} MiB")
    }
}
