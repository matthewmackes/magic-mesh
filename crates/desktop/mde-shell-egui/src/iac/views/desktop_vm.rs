//! U16 — the **Desktop VM** delivery view: full VM desktops delivered as native
//! VDI seats. Renders the seat roster from the folded `state/cloud` mirror (live
//! status · drift · cpu/mem/disk), an honest empty state, and per-seat lifecycle
//! typed Workload verbs (open / start / stop, reboot + destroy) over the
//! landed backend seams.

use mackes_mesh_types::cloud::{DriftFlag, WorkloadRow};
use mde_egui::egui::{self, Color32, RichText};
use mde_egui::{carbon_icon, card, field, muted_note, status_dot, Style, TypographyRole};

use super::super::{row_button, DeliveryView, WorkloadsRoute, WorkloadsState};

/// The Desktop VM view's own state (U16 owns its fields).
#[derive(Debug, Default)]
pub(in crate::iac) struct State;

/// Render the Desktop VM view — the native-VDI-seat roster + per-seat lifecycle.
pub(super) fn view(ui: &mut egui::Ui, state: &mut WorkloadsState) {
    heading(
        ui,
        "Desktop VM",
        "Full VM desktops delivered as native VDI seats, placed on their mesh nodes.",
    );
    provision_cta(ui, state, "Provision a desktop VM");

    let rows: Vec<WorkloadRow> = state
        .workloads_of(DeliveryView::DesktopVm)
        .cloned()
        .collect();
    if rows.is_empty() {
        crate::empty_state::show(
            ui,
            "No desktop seats yet",
            "A desktop seat appears here once a placement node reports a desktop_vm workload in \
             its state/cloud mirror.",
        );
    } else {
        for row in &rows {
            seat_card(ui, state, row);
        }
    }
}

/// One seat card — name · placement · live status · drift, the live metrics, then
/// the seat's lifecycle verbs (typed Open first, destructive ones confirmed).
fn seat_card(ui: &mut egui::Ui, state: &mut WorkloadsState, row: &WorkloadRow) {
    card().show(ui, |ui| {
        header_row(ui, row);
        metrics_line(ui, row);
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            if row_button(ui, "Console", false).clicked() {
                state.issue_console_attach(&row.node, &row.name, &row.name);
            }
            if row_button(ui, "Start", false).clicked() {
                state.issue_workload_direct(
                    "instance-start",
                    &row.node,
                    &row.name,
                    row.delivery_type,
                    &row.name,
                );
            }
            if row_button(ui, "Stop", false).clicked() {
                state.issue_workload_direct(
                    "instance-stop",
                    &row.node,
                    &row.name,
                    row.delivery_type,
                    &row.name,
                );
            }
            if row_button(ui, "Reboot\u{2026}", true).clicked() {
                state.issue_workload_direct(
                    "instance-reboot",
                    &row.node,
                    &row.name,
                    row.delivery_type,
                    &row.name,
                );
            }
            if row_button(ui, "Destroy\u{2026}", true).clicked() {
                state.issue_workload_direct(
                    "instance-delete",
                    &row.node,
                    &row.name,
                    row.delivery_type,
                    &row.name,
                );
            }
        });
    });
    ui.add_space(Style::SP_S);
}

// ─────────────────────────── shared row grammar ─────────────────────────────

/// The card's identity row: name (strong), a live-status dot + word, the drift
/// chip, then the placement node — all in reading order.
fn header_row(ui: &mut egui::Ui, row: &WorkloadRow) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&row.name)
                .font(Style::typography_font_with_size(
                    TypographyRole::Body,
                    Style::BODY,
                ))
                .strong()
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);
        let tone = status_tone(&row.status);
        status_dot(ui, tone);
        ui.colored_label(
            tone,
            RichText::new(&row.status).font(Style::typography_font_with_size(
                TypographyRole::Label,
                Style::SMALL,
            )),
        );
        ui.add_space(Style::SP_M);
        drift_chip(ui, row.drift);
        ui.add_space(Style::SP_M);
        ui.colored_label(
            Style::TEXT_DIM,
            RichText::new(format!("on {}", row.node)).font(Style::typography_font_with_size(
                TypographyRole::Caption,
                Style::SMALL,
            )),
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
    ui.colored_label(
        tone,
        RichText::new(drift_word(drift)).font(Style::typography_font_with_size(
            TypographyRole::Label,
            Style::SMALL,
        )),
    );
}

/// The view heading — the Workloads-accent glyph + title + a one-line blurb.
fn heading(ui: &mut egui::Ui, title: &str, blurb: &str) {
    ui.horizontal(|ui| {
        ui.scope(|ui| {
            ui.visuals_mut().override_text_color = Some(Style::ACCENT_WORKLOADS);
            carbon_icon(ui, DeliveryView::DesktopVm.icon(), Style::ICON_M);
        });
        ui.add_space(Style::SP_XS);
        ui.label(
            RichText::new(title)
                .font(Style::typography_font_with_size(
                    TypographyRole::Title,
                    Style::TITLE,
                ))
                .strong()
                .color(Style::ACCENT_WORKLOADS),
        );
    });
    muted_note(ui, blurb);
    ui.add_space(Style::SP_S);
}

/// The "provision a workload of this type" affordance — jumps to the Provision
/// route (U14 placement + U15 form).
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
                    .font(Style::typography_font_with_size(
                        TypographyRole::Label,
                        Style::SMALL,
                    ))
                    .color(Style::ACCENT_WORKLOADS),
            ))
            .clicked()
        {
            state.set_route(WorkloadsRoute::Provision);
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
