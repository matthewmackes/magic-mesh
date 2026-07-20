//! U16 — the **Service VM** delivery view: headless VMs running a service exposed
//! on the mesh. The roster leads with mesh reachability (does the workload's
//! `*.mesh` name resolve + is its overlay path up) alongside live status · drift ·
//! metrics, then the VM lifecycle verbs. Headless by design — there is no seat to
//! attach, so this view omits console and foregrounds service health instead.

use mackes_mesh_types::cloud::{DriftFlag, WorkloadRow};
use mde_egui::egui::{self, Color32, RichText};
use mde_egui::{carbon_icon, card, field, muted_note, status_dot, Style};

use super::super::{row_button, DeliveryView, Panel, WorkloadsState};

/// The Service VM view's own state (U16 owns its fields).
#[derive(Debug, Default)]
pub(in crate::iac) struct State;

/// Render the Service VM view — the headless-service roster + per-VM lifecycle.
pub(super) fn view(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    heading(
        ui,
        "Service VM",
        "Headless VMs running a service exposed on the mesh, placed on their nodes.",
    );
    provision_cta(ui, state, "Provision a service VM");

    let rows: Vec<WorkloadRow> = state
        .workloads_of(DeliveryView::ServiceVm)
        .cloned()
        .collect();
    if rows.is_empty() {
        crate::session::empty_state(
            ui,
            "No service VMs yet",
            "A service VM appears here once a placement node reports a service_vm workload in its \
             state/cloud mirror.",
        );
        return;
    }
    for row in &rows {
        service_card(ui, state, row);
    }
    muted_note(
        ui,
        "Reachability is folded from the overlay keepalive lease + *.mesh resolution. A headless \
         service has no graphics head, so console-attach is deliberately absent here.",
    );
}

/// One service card — name · reachability · live status · drift, the metrics, then
/// the VM lifecycle verbs (destructive ones typed-armed). No console (headless).
fn service_card(ui: &mut egui::Ui, state: &mut WorkloadsState, row: &WorkloadRow) {
    card().show(ui, |ui| {
        header_row(ui, row);
        metrics_line(ui, row);
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            if row_button(ui, "Start", false).clicked() {
                state.issue_lifecycle_direct("instance-start", &row.name, &row.name);
            }
            if row_button(ui, "Stop", false).clicked() {
                state.issue_lifecycle_direct("instance-stop", &row.name, &row.name);
            }
            if row_button(ui, "Reboot\u{2026}", true).clicked() {
                state.arm_lifecycle("instance-reboot", &row.name, &row.name);
            }
            if row_button(ui, "Destroy\u{2026}", true).clicked() {
                state.arm_lifecycle("instance-delete", &row.name, &row.name);
            }
        });
    });
    ui.add_space(Style::SP_S);
}

// ─────────────────────────── shared row grammar ─────────────────────────────

/// The card's identity row: name (strong), the mesh reachability chip (this view's
/// lead signal), the live-status dot + word, the drift chip, then the node.
fn header_row(ui: &mut egui::Ui, row: &WorkloadRow) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&row.name)
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);
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

/// The mesh-reachability chip — a service that isn't on the overlay is a real
/// problem (a warning), never fabricated as up.
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
            carbon_icon(ui, DeliveryView::ServiceVm.icon(), Style::BODY + 2.0);
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

/// The "provision a workload of this type" affordance — jumps to the Provision
/// lens (U14 placement + U15 form).
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
            state.set_panel(Panel::Provision);
        }
    });
    ui.add_space(Style::SP_S);
}

/// The Style tone a live domain status paints.
fn status_tone(status: &str) -> Color32 {
    match status.trim().to_ascii_lowercase().as_str() {
        "running" | "active" => Style::SUPPORT_SUCCESS,
        "paused" | "pmsuspended" => Style::WARN,
        s if s.contains("error") || s.contains("fail") || s.contains("crash") => Style::DANGER,
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
