//! U16 — the **App VM** delivery view: VMs whose individual apps are forwarded
//! into the MDE desktop (VDI **app-mode** via `session_broker`). Rather than a
//! full seat, the operator runs single apps from the VM windowed into their own
//! desktop. The roster carries the same live status · drift · metrics, a lead
//! `app-mode` tag, whole-VM console + lifecycle verbs, and an honest note on the
//! per-app forwarding leg.

use mackes_mesh_types::cloud::{DriftFlag, WorkloadRow};
use mde_egui::egui::{self, Color32, RichText};
use mde_egui::{carbon_icon, card, field, inset, muted_note, status_dot, Style};

use super::super::{row_button, DeliveryView, Panel, WorkloadsState};

/// The App VM view's own state (U16 owns its fields).
#[derive(Debug, Default)]
pub(in crate::iac) struct State;

/// Render the App VM view — the app-mode roster + per-VM console/lifecycle.
pub(super) fn view(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    heading(
        ui,
        "App VM",
        "VMs whose individual apps are forwarded into your desktop (VDI app-mode).",
    );
    provision_cta(ui, state, "Provision an app VM");

    let rows: Vec<WorkloadRow> = state.workloads_of(DeliveryView::AppVm).cloned().collect();
    if rows.is_empty() {
        crate::empty_state::show(
            ui,
            "No app VMs yet",
            "An app VM appears here once a placement node reports an app_vm workload in its \
             state/cloud mirror.",
        );
    } else {
        for row in &rows {
            app_card(ui, state, row);
        }
    }
    muted_note(
        ui,
        "Console attaches the whole app VM's display; forwarding an individual app into your \
         desktop rides session_broker (VDI app-mode) and is not yet a distinct cloud verb — that \
         per-app launch leg lands with the app-mode forwarding unit.",
    );
    super::super::console_section(ui, state);
}

/// One app-VM card — name · `app-mode` tag · status · drift, the metrics, then the
/// whole-VM verbs (console-attach + lifecycle, destructive ones typed-armed).
fn app_card(ui: &mut egui::Ui, state: &mut WorkloadsState, row: &WorkloadRow) {
    card().show(ui, |ui| {
        header_row(ui, row);
        metrics_line(ui, row);
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            if row_button(ui, "Console", false).clicked() {
                state.issue_console_attach(&row.node, &row.name, &row.name);
            }
            if row_button(ui, "Start", false).clicked() {
                state.issue_lifecycle_direct("instance-start", &row.node, &row.name, &row.name);
            }
            if row_button(ui, "Stop", false).clicked() {
                state.issue_lifecycle_direct("instance-stop", &row.node, &row.name, &row.name);
            }
            if row_button(ui, "Reboot\u{2026}", true).clicked() {
                state.arm_lifecycle("instance-reboot", &row.node, &row.name, &row.name);
            }
            if row_button(ui, "Destroy\u{2026}", true).clicked() {
                state.arm_lifecycle("instance-delete", &row.node, &row.name, &row.name);
            }
        });
    });
    ui.add_space(Style::SP_S);
}

// ─────────────────────────── shared row grammar ─────────────────────────────

/// The card's identity row: name (strong), the `app-mode` delivery tag, the
/// live-status dot + word, the drift chip, then the node.
fn header_row(ui: &mut egui::Ui, row: &WorkloadRow) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&row.name)
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);
        tag(ui, "app-mode");
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

/// A small recessed delivery tag — the `inset` well around a dim caption, so the
/// app-mode nature reads at a glance without a coloured chip.
fn tag(ui: &mut egui::Ui, label: &str) {
    inset().show(ui, |ui| {
        ui.label(
            RichText::new(label)
                .size(Style::SMALL)
                .color(Style::ACCENT_WORKLOADS),
        );
    });
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
            carbon_icon(ui, DeliveryView::AppVm.icon(), Style::ICON_S);
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
