//! Transfers mode — the shared transfer jobs and the controls every member may
//! drive (WL-FUNC-011).
//!
//! This mode renders the [`TransferJobs`](mde_collab_types::TransferJobs)
//! projection — the read-side **mirror of the WL-FUNC-006 progress ledger**. It
//! is emphatically **not** a second progress authority: byte progress
//! (`moved`/`total`) is *mirrored* from that ledger and rendered honestly
//! (`0 / 0` while the ledger has not reported a size yet — never faked to 100%),
//! and the only writes this mode makes are typed
//! [`ControlTransfer`](mde_collab_types::CollabCommand::ControlTransfer) commands
//! (pause / resume / cancel) — the same control the Files mode drives per-file,
//! surfaced here as a fleet-wide job list.

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{
    TransferControl, TransferDirection, TransferJobView, TransferMethod, TransferState,
};

use crate::files::{fmt_bytes, transfer_state_color, transfer_state_label};
use crate::icons::CommsHoverExt;
use crate::{icons, CommunicationsSurface};

impl CommunicationsSurface {
    /// Render Transfers mode: the whole shared transfer-job list, newest ledger
    /// order, each row showing its mirrored state + byte progress and carrying the
    /// controls appropriate to its state.
    pub(crate) fn transfers_body(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
    ) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Transfers")
                    .strong()
                    .color(Style::TEXT_STRONG),
            );
            ui.label(
                egui::RichText::new("shared ledger mirror")
                    .small()
                    .color(Style::TEXT_DIM),
            );
        });
        ui.separator();

        let jobs = data.transfer_jobs();
        match jobs {
            Some(jobs) if !jobs.jobs.is_empty() => {
                egui::ScrollArea::vertical()
                    .id_salt("collab-transfers")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for job in &jobs.jobs {
                            self.transfer_job_row(ui, sink, job);
                            ui.add_space(Style::SP_XS);
                        }
                    });
            }
            _ => {
                ui.label(egui::RichText::new("No transfers in flight.").color(Style::TEXT_DIM));
                ui.label(
                    egui::RichText::new(
                        "Share a file from the Files mode to start one — its progress mirrors here.",
                    )
                    .small()
                    .color(Style::TEXT_DIM),
                );
            }
        }
    }

    /// One transfer-job row: the direction glyph, the file it moves + transport,
    /// its mirrored state + byte progress, and the pause/resume/cancel controls.
    fn transfer_job_row(
        &self,
        ui: &mut egui::Ui,
        sink: &mut crate::CommandSink,
        job: &TransferJobView,
    ) {
        mde_egui::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                let (glyph, dir_hint) = match job.direction {
                    TransferDirection::Inbound => (icons::XFER_INBOUND, "Inbound"),
                    TransferDirection::Outbound => (icons::XFER_OUTBOUND, "Outbound"),
                };
                icons::icon(ui, icons::XFER_ROW, Style::SP_M, Style::ACCENT);
                icons::icon(ui, glyph, Style::SP_M, Style::TEXT_DIM).comms_hover_text(dir_hint);
                ui.label(
                    egui::RichText::new(short_file(job))
                        .strong()
                        .color(Style::TEXT_STRONG),
                );
                ui.label(
                    egui::RichText::new(format!("· {}", method_label(job.method)))
                        .small()
                        .color(Style::TEXT_DIM),
                );
            });

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(transfer_state_label(job.state))
                        .small()
                        .strong()
                        .color(transfer_state_color(job.state)),
                );
                // Mirrored byte progress (WL-FUNC-006). `total == 0` means the
                // ledger has not reported a size yet — shown honestly, never
                // faked to a full bar.
                if job.total > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} / {}",
                            fmt_bytes(job.moved),
                            fmt_bytes(job.total)
                        ))
                        .small()
                        .color(Style::TEXT_DIM),
                    );
                } else if job.moved > 0 {
                    ui.label(
                        egui::RichText::new(fmt_bytes(job.moved))
                            .small()
                            .color(Style::TEXT_DIM),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("progress pending")
                            .small()
                            .color(Style::TEXT_DIM),
                    );
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.transfer_controls_row(ui, sink, job);
                });
            });
        });
    }

    /// The controls appropriate to a job's state (terminal states carry none) —
    /// each emits a typed
    /// [`ControlTransfer`](mde_collab_types::CollabCommand::ControlTransfer).
    fn transfer_controls_row(
        &self,
        ui: &mut egui::Ui,
        sink: &mut crate::CommandSink,
        job: &TransferJobView,
    ) {
        match job.state {
            TransferState::Active => {
                if icons::icon_button(
                    ui,
                    icons::TRANSFER_CANCEL,
                    Style::SP_M,
                    Style::DANGER,
                    "Cancel",
                )
                .clicked()
                {
                    self.control_transfer(sink, job.transfer, TransferControl::Cancel);
                }
                if icons::icon_button(
                    ui,
                    icons::TRANSFER_PAUSE,
                    Style::SP_M,
                    Style::TEXT_DIM,
                    "Pause",
                )
                .clicked()
                {
                    self.control_transfer(sink, job.transfer, TransferControl::Pause);
                }
            }
            TransferState::Paused => {
                if icons::icon_button(
                    ui,
                    icons::TRANSFER_CANCEL,
                    Style::SP_M,
                    Style::DANGER,
                    "Cancel",
                )
                .clicked()
                {
                    self.control_transfer(sink, job.transfer, TransferControl::Cancel);
                }
                if icons::icon_button(ui, icons::TRANSFER_RESUME, Style::SP_M, Style::OK, "Resume")
                    .clicked()
                {
                    self.control_transfer(sink, job.transfer, TransferControl::Resume);
                }
            }
            TransferState::Queued => {
                if icons::icon_button(
                    ui,
                    icons::TRANSFER_CANCEL,
                    Style::SP_M,
                    Style::DANGER,
                    "Cancel",
                )
                .clicked()
                {
                    self.control_transfer(sink, job.transfer, TransferControl::Cancel);
                }
            }
            // Terminal states carry no control (the ledger owns their finality).
            TransferState::Completed | TransferState::Failed | TransferState::Canceled => {}
        }
    }
}

/// A short display handle for the file a job moves (the content-address model
/// keys transfers by an opaque `FileRefId`; the row shows a recognisable prefix).
fn short_file(job: &TransferJobView) -> String {
    let id = job.file.to_string();
    let head: String = id.chars().take(8).collect();
    format!("file {head}\u{2026}")
}

/// The honest transport label for a transfer method.
const fn method_label(method: TransferMethod) -> &'static str {
    match method {
        TransferMethod::Node => "mesh",
        TransferMethod::Sftp => "SFTP",
        TransferMethod::Http => "HTTP",
        TransferMethod::Rsync => "rsync",
        TransferMethod::BrowserDownload => "browser",
        TransferMethod::MusicLibrary => "music",
    }
}
