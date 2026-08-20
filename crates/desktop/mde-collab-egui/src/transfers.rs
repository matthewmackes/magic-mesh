//! Transfers mode — the shared transfer jobs and the recurring sync-pair editor
//! (WL-FUNC-011 / WL-FUNC-028 / WL-FUNC-032).
//!
//! This mode renders the [`TransferJobs`](mde_collab_types::TransferJobs)
//! projection — the read-side **mirror of the WL-FUNC-006 progress ledger**. It
//! is emphatically **not** a second progress authority: byte progress
//! (`moved`/`total`) is *mirrored* from that ledger and rendered honestly
//! (`0 / 0` while the ledger has not reported a size yet — never faked to 100%),
//! and the only job-lifecycle writes this mode makes are typed
//! [`ControlTransfer`](mde_collab_types::CollabCommand::ControlTransfer) commands
//! (pause / resume / cancel).
//!
//! Recurring rsync mirrors reuse the daemon's existing `SyncPairStore` +
//! `schedule_sync_pairs_at` worker. This crate is a pure UI: it never opens that
//! store and never schedules. The editor publishes [`SyncPairCommand`]s into a
//! local [`SyncPairSink`] the shell later drains onto
//! `TransferVerb::{SaveSyncPair, RemoveSyncPair}`. Pair rows (next-run,
//! last-result, unreachable peers) come from [`SyncPairSource`], whose default
//! is an honest empty projection.

use std::sync::atomic::{AtomicU8, Ordering};

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{
    TransferControl, TransferDirection, TransferJobView, TransferMethod, TransferState,
};

use crate::files::{fmt_bytes, transfer_state_color, transfer_state_label};
use crate::icons::CommsHoverExt;
use crate::{icons, relative_age, CommunicationsSurface};

const INTENT_NONE: u8 = 0;
const INTENT_OPEN: u8 = 1;
const INTENT_NEW: u8 = 2;

/// Process-local Transfers hotkey latch so the shell can open this mode (and the
/// in-mode New Transfer editor) without the Communications mount owning a Bus
/// client. Consumed once on the next [`CommunicationsSurface::ui`] frame.
static TRANSFERS_HOTKEY_INTENT: AtomicU8 = AtomicU8::new(0);

/// Ask Communications to land on Transfers mode (Ctrl+J).
pub fn request_open_transfers() {
    TRANSFERS_HOTKEY_INTENT.store(INTENT_OPEN, Ordering::SeqCst);
}

/// Ask Transfers mode to open the New Transfer / sync-pair editor (Ctrl+N).
pub fn request_new_transfer() {
    TRANSFERS_HOTKEY_INTENT.store(INTENT_NEW, Ordering::SeqCst);
}

/// Drop a pending Transfers hotkey intent (tests that must not leak the latch).
pub fn clear_transfers_hotkey_intent() {
    TRANSFERS_HOTKEY_INTENT.store(INTENT_NONE, Ordering::SeqCst);
}

/// Drain one pending Transfers hotkey intent, if any.
pub(crate) fn take_transfers_hotkey_intent() -> Option<TransfersHotkey> {
    match TRANSFERS_HOTKEY_INTENT.swap(INTENT_NONE, Ordering::SeqCst) {
        INTENT_OPEN => Some(TransfersHotkey::Open),
        INTENT_NEW => Some(TransfersHotkey::New),
        _ => None,
    }
}

/// A Transfers-mode chord the shell (or a test) asked the surface to honour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransfersHotkey {
    /// Open Communications in Transfers mode.
    Open,
    /// Open the in-mode New Transfer editor.
    New,
}

/// Typed write intents the Transfers editor emits. The shell later drains these
/// onto `TransferVerb::{SaveSyncPair, RemoveSyncPair}`. Kept local so this pure
/// UI crate does not depend on mackesd or grow a collab-types verb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncPairCommand {
    /// Create or replace a recurring rsync pair.
    Save {
        /// Operator-facing pair id (the store filename stem).
        id: String,
        /// rsync source spec.
        source: String,
        /// rsync destination spec.
        dest: String,
        /// Recurrence interval in seconds (already parsed; never zero).
        every_secs: u64,
        /// Optional rsync `--bwlimit` token.
        bwlimit: Option<String>,
    },
    /// Delete a saved pair by id.
    Remove {
        /// The pair id to drop.
        id: String,
    },
}

/// The sink the editor pushes [`SyncPairCommand`]s into. Tests assert on
/// [`queued`](Self::queued); the shell drains onto the existing TransferVerb Bus.
#[derive(Debug, Default, Clone)]
pub struct SyncPairSink {
    queued: Vec<SyncPairCommand>,
}

impl SyncPairSink {
    /// A fresh, empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `command` as intent for the caller to route.
    pub fn emit(&mut self, command: SyncPairCommand) {
        self.queued.push(command);
    }

    /// Take every queued command, leaving the sink empty.
    #[must_use]
    pub fn drain(&mut self) -> Vec<SyncPairCommand> {
        std::mem::take(&mut self.queued)
    }

    /// The queued commands without draining (test assertions read this).
    #[must_use]
    pub fn queued(&self) -> &[SyncPairCommand] {
        &self.queued
    }

    /// Whether nothing is queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }
}

/// One mirrored sync-pair row from the daemon store/scheduler (next-run and
/// last-result are worker facts; this crate never computes a schedule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPairView {
    /// Stable operator-facing id.
    pub id: String,
    /// rsync source spec.
    pub source: String,
    /// rsync destination spec.
    pub dest: String,
    /// Recurrence interval in seconds.
    pub every_secs: u64,
    /// Optional bandwidth cap token.
    pub bwlimit: Option<String>,
    /// Worker-projected next fire time (epoch ms). `None` until the scheduler
    /// has a stamp.
    pub next_run_unix_ms: Option<i64>,
    /// Worker-projected last outcome (`ok`, a failure reason, …). `None` if
    /// the pair has never fired.
    pub last_result: Option<String>,
    /// `false` when the destination peer is unreachable — the row stays visible
    /// and is painted degraded. Never hidden.
    pub peer_reachable: bool,
}

/// Read-side access to saved sync pairs. Defaults to empty so a source that has
/// not bound the worker projection still renders the honest empty editor — this
/// lives here, not on [`CollabData`](crate::CollabData), because Communications
/// must not grow a second transfer authority on the collab read model.
pub trait SyncPairSource {
    /// The current store projection. Empty until the shell (or a test) binds it.
    #[must_use]
    fn sync_pairs(&self) -> &[SyncPairView] {
        &[]
    }
}

impl SyncPairSource for () {}

impl SyncPairSource for Vec<SyncPairView> {
    fn sync_pairs(&self) -> &[SyncPairView] {
        self
    }
}

/// Transfers-mode editor view state (create/edit drafts). Not a pair store.
#[derive(Debug, Clone, Default)]
pub(crate) struct TransfersUi {
    editor_open: bool,
    edit_id: Option<String>,
    draft_id: String,
    draft_source: String,
    draft_dest: String,
    draft_interval: String,
    draft_bwlimit: String,
    notice: Option<String>,
}

impl TransfersUi {
    /// Open a blank create form (the in-mode New Transfer accelerator).
    pub(crate) fn begin_new(&mut self) {
        self.editor_open = true;
        self.edit_id = None;
        self.draft_id.clear();
        self.draft_source.clear();
        self.draft_dest.clear();
        self.draft_interval = "1h".to_owned();
        self.draft_bwlimit.clear();
        self.notice = None;
    }

    fn begin_edit(&mut self, pair: &SyncPairView) {
        self.editor_open = true;
        self.edit_id = Some(pair.id.clone());
        self.draft_id = pair.id.clone();
        self.draft_source = pair.source.clone();
        self.draft_dest = pair.dest.clone();
        self.draft_interval = format_interval_draft(pair.every_secs);
        self.draft_bwlimit = pair.bwlimit.clone().unwrap_or_default();
        self.notice = None;
    }

    fn close(&mut self) {
        self.editor_open = false;
        self.edit_id = None;
        self.notice = None;
    }
}

impl CommunicationsSurface {
    /// Render Transfers mode: the sync-pair editor + the shared job list.
    pub(crate) fn transfers_body(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
    ) {
        self.consume_in_mode_new_transfer(ui);

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Transfers")
                    .strong()
                    .color(Style::TEXT_STRONG),
            );
            ui.label(
                egui::RichText::new("shared ledger mirror · recurring sync pairs")
                    .small()
                    .color(Style::TEXT_DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::icon_button(
                    ui,
                    icons::FILE_LINK,
                    Style::SP_M,
                    Style::ACCENT,
                    "New transfer (Ctrl+N)",
                )
                .clicked()
                {
                    self.transfers_ui.begin_new();
                }
            });
        });
        ui.separator();

        self.sync_pairs_section(ui, data);
        ui.add_space(Style::SP_S);
        self.jobs_section(ui, data, sink);
    }

    /// Ctrl+N is the in-mode New Transfer accelerator. It must not steal keystrokes
    /// from a focused text field (Documents / this editor's own drafts).
    fn consume_in_mode_new_transfer(&mut self, ui: &mut egui::Ui) {
        if ui.ctx().wants_keyboard_input() {
            return;
        }
        if ui
            .ctx()
            .input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::N))
        {
            self.transfers_ui.begin_new();
        }
    }

    fn sync_pairs_section(&mut self, ui: &mut egui::Ui, data: &dyn crate::CollabData) {
        ui.label(
            egui::RichText::new("Sync pairs")
                .strong()
                .color(Style::TEXT_STRONG),
        );
        ui.label(
            egui::RichText::new("next-run and last-result come from the transfers worker")
                .small()
                .color(Style::TEXT_DIM),
        );

        if self.transfers_ui.editor_open {
            self.sync_pair_editor(ui);
        }

        let pairs = self.sync_pair_views.clone();
        if pairs.is_empty() && !self.transfers_ui.editor_open {
            ui.label(
                egui::RichText::new("No sync pairs saved.")
                    .small()
                    .color(Style::TEXT_DIM),
            );
            ui.label(
                egui::RichText::new("Create one here or with `mackesd transfer sync-pair add`.")
                    .small()
                    .color(Style::TEXT_DIM),
            );
        } else {
            for pair in &pairs {
                self.sync_pair_row(ui, data, pair);
                ui.add_space(Style::SP_XS);
            }
        }
    }

    fn sync_pair_editor(&mut self, ui: &mut egui::Ui) {
        mde_egui::card().show(ui, |ui| {
            let title = if self.transfers_ui.edit_id.is_some() {
                "Edit sync pair"
            } else {
                "New transfer"
            };
            ui.label(
                egui::RichText::new(title)
                    .strong()
                    .color(Style::TEXT_STRONG),
            );
            ui.add_space(Style::SP_XS);
            labeled_edit(ui, "Id", &mut self.transfers_ui.draft_id);
            labeled_edit(ui, "Interval", &mut self.transfers_ui.draft_interval);
            labeled_edit(ui, "Source", &mut self.transfers_ui.draft_source);
            labeled_edit(ui, "Destination", &mut self.transfers_ui.draft_dest);
            labeled_edit(ui, "Bwlimit", &mut self.transfers_ui.draft_bwlimit);
            if let Some(notice) = &self.transfers_ui.notice {
                ui.label(egui::RichText::new(notice).small().color(Style::DANGER));
            }
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    self.save_sync_pair_draft();
                }
                if ui.button("Cancel").clicked() {
                    self.transfers_ui.close();
                }
            });
        });
    }

    fn sync_pair_row(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        pair: &SyncPairView,
    ) {
        let degraded = !pair.peer_reachable;
        let title_tone = if degraded {
            Style::WARN
        } else {
            Style::TEXT_STRONG
        };
        let meta_tone = if degraded {
            Style::WARN
        } else {
            Style::TEXT_DIM
        };
        mde_egui::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::icon(ui, icons::XFER_ROW, Style::SP_M, title_tone);
                ui.label(egui::RichText::new(&pair.id).strong().color(title_tone));
                if degraded {
                    ui.label(
                        egui::RichText::new("unreachable")
                            .small()
                            .strong()
                            .color(Style::WARN),
                    );
                }
                ui.label(
                    egui::RichText::new(format!(
                        "every {}",
                        format_interval_draft(pair.every_secs)
                    ))
                    .small()
                    .color(meta_tone),
                );
            });
            ui.label(
                egui::RichText::new(format!("{} → {}", pair.source, pair.dest))
                    .small()
                    .color(meta_tone),
            );
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format_next_run(data.now_unix_ms(), pair.next_run_unix_ms))
                        .small()
                        .color(meta_tone),
                );
                ui.label(
                    egui::RichText::new(format_last_result(pair.last_result.as_deref()))
                        .small()
                        .color(meta_tone),
                );
                if let Some(limit) = &pair.bwlimit {
                    ui.label(
                        egui::RichText::new(format!("bwlimit {limit}"))
                            .small()
                            .color(meta_tone),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::icon_button(
                        ui,
                        icons::FILE_UNLINK,
                        Style::SP_M,
                        Style::DANGER,
                        "Remove sync pair",
                    )
                    .clicked()
                    {
                        self.sync_pair_sink.emit(SyncPairCommand::Remove {
                            id: pair.id.clone(),
                        });
                    }
                    if icons::icon_button(
                        ui,
                        icons::DOC_ROW,
                        Style::SP_M,
                        Style::TEXT_DIM,
                        "Edit sync pair",
                    )
                    .clicked()
                    {
                        self.transfers_ui.begin_edit(pair);
                    }
                });
            });
        });
    }

    fn save_sync_pair_draft(&mut self) {
        let id = self.transfers_ui.draft_id.trim();
        let source = self.transfers_ui.draft_source.trim();
        let dest = self.transfers_ui.draft_dest.trim();
        let id = if id.is_empty() {
            slug_pair_id(source, dest)
        } else {
            id.to_owned()
        };
        if !valid_pair_id(&id) {
            self.transfers_ui.notice = Some("malformed pair id".to_owned());
            return;
        }
        if source.is_empty() || dest.is_empty() {
            self.transfers_ui.notice = Some("sync pair requires source and destination".to_owned());
            return;
        }
        let Some(every_secs) = parse_interval_secs(&self.transfers_ui.draft_interval) else {
            self.transfers_ui.notice =
                Some("malformed interval (use 30s, 5m, 1h, or a positive second count)".to_owned());
            return;
        };
        let bwlimit = {
            let raw = self.transfers_ui.draft_bwlimit.trim();
            if raw.is_empty() {
                None
            } else {
                Some(raw.to_owned())
            }
        };
        self.sync_pair_sink.emit(SyncPairCommand::Save {
            id,
            source: source.to_owned(),
            dest: dest.to_owned(),
            every_secs,
            bwlimit,
        });
        self.transfers_ui.close();
    }

    #[cfg(test)]
    pub(crate) fn save_sync_pair_draft_for_test(
        &mut self,
        id: &str,
        interval: &str,
        source: &str,
        dest: &str,
        bwlimit: Option<&str>,
    ) {
        self.transfers_ui.editor_open = true;
        self.transfers_ui.draft_id = id.to_owned();
        self.transfers_ui.draft_interval = interval.to_owned();
        self.transfers_ui.draft_source = source.to_owned();
        self.transfers_ui.draft_dest = dest.to_owned();
        self.transfers_ui.draft_bwlimit = bwlimit.unwrap_or("").to_owned();
        self.save_sync_pair_draft();
    }

    #[cfg(test)]
    pub(crate) fn sync_pair_notice_for_test(&self) -> Option<&str> {
        self.transfers_ui.notice.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn editor_open_for_test(&self) -> bool {
        self.transfers_ui.editor_open
    }

    fn jobs_section(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut crate::CommandSink,
    ) {
        ui.label(
            egui::RichText::new("Jobs")
                .strong()
                .color(Style::TEXT_STRONG),
        );
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

fn labeled_edit(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).small().color(Style::TEXT_DIM));
        ui.text_edit_singleline(value);
    });
}

/// Parse a sync-pair interval. Accepts a positive second count or a unit suffix
/// (`s`/`m`/`h`/`d`). Zero, empty, negative, and unknown units refuse.
#[must_use]
pub(crate) fn parse_interval_secs(raw: &str) -> Option<u64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        let n: u64 = s.parse().ok()?;
        return (n >= 1).then_some(n);
    }
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    if split == 0 {
        return None;
    }
    let (num, unit) = s.split_at(split);
    let n: u64 = num.parse().ok()?;
    if n == 0 {
        return None;
    }
    let mult: u64 = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return None,
    };
    n.checked_mul(mult).filter(|v| *v >= 1)
}

fn format_interval_draft(every_secs: u64) -> String {
    if every_secs > 0 && every_secs.is_multiple_of(86_400) {
        format!("{}d", every_secs / 86_400)
    } else if every_secs > 0 && every_secs.is_multiple_of(3600) {
        format!("{}h", every_secs / 3600)
    } else if every_secs > 0 && every_secs.is_multiple_of(60) {
        format!("{}m", every_secs / 60)
    } else {
        format!("{every_secs}s")
    }
}

fn format_next_run(now_unix_ms: i64, next_run_unix_ms: Option<i64>) -> String {
    match next_run_unix_ms {
        None => "next-run pending".to_owned(),
        Some(ts) if ts <= now_unix_ms => "due now".to_owned(),
        Some(ts) => {
            let until = relative_age(ts, now_unix_ms);
            if until == "now" {
                "due soon".to_owned()
            } else {
                format!("next in {until}")
            }
        }
    }
}

fn format_last_result(last: Option<&str>) -> String {
    match last {
        Some(result) if !result.is_empty() => format!("last: {result}"),
        _ => "never run".to_owned(),
    }
}

fn valid_pair_id(id: &str) -> bool {
    let id = id.trim();
    !id.is_empty()
        && id != "."
        && id != ".."
        && id.len() <= 120
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn slug_pair_id(source: &str, dest: &str) -> String {
    let raw = format!("{source}-{dest}");
    let slug: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(80)
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "sync-pair".to_owned()
    } else {
        slug.to_owned()
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
