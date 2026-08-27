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
#[cfg(test)]
use std::sync::{Mutex, OnceLock};
#[cfg(test)]
use std::thread::ThreadId;

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{
    TransferControl, TransferDirection, TransferJobView, TransferMethod, TransferState,
};

use crate::files::{fmt_bytes, transfer_state_color, transfer_state_label};
use crate::icons::CommsHoverExt;
use crate::{icons, CommunicationsSurface};

const INTENT_NONE: u8 = 0;
const INTENT_OPEN: u8 = 1;
const INTENT_NEW: u8 = 2;

/// Process-local Transfers hotkey latch so the shell can open this mode (and the
/// in-mode New Transfer editor) without the Communications mount owning a Bus
/// client. Consumed once on the next [`CommunicationsSurface::ui`] frame.
static TRANSFERS_HOTKEY_INTENT: AtomicU8 = AtomicU8::new(0);

#[cfg(test)]
static TRANSFERS_HOTKEY_OWNER: OnceLock<Mutex<Option<ThreadId>>> = OnceLock::new();

#[cfg(test)]
fn hotkey_owner() -> &'static Mutex<Option<ThreadId>> {
    TRANSFERS_HOTKEY_OWNER.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
fn remember_hotkey_owner() {
    *hotkey_owner()
        .lock()
        .expect("Transfers hotkey owner lock must not be poisoned") =
        Some(std::thread::current().id());
}

#[cfg(not(test))]
fn remember_hotkey_owner() {}

#[cfg(test)]
fn clear_hotkey_owner() {
    *hotkey_owner()
        .lock()
        .expect("Transfers hotkey owner lock must not be poisoned") = None;
}

#[cfg(not(test))]
fn clear_hotkey_owner() {}

#[cfg(test)]
fn hotkey_belongs_to_current_thread() -> bool {
    *hotkey_owner()
        .lock()
        .expect("Transfers hotkey owner lock must not be poisoned")
        == Some(std::thread::current().id())
}

#[cfg(not(test))]
fn hotkey_belongs_to_current_thread() -> bool {
    true
}

fn theme_color(ui: &egui::Ui, color: egui::Color32) -> egui::Color32 {
    Style::resolve_color(ui.ctx(), color)
}

/// Ask Communications to land on Transfers mode (Ctrl+J).
pub fn request_open_transfers() {
    remember_hotkey_owner();
    TRANSFERS_HOTKEY_INTENT.store(INTENT_OPEN, Ordering::SeqCst);
}

/// Ask Transfers mode to open the New Transfer / sync-pair editor (Ctrl+N).
pub fn request_new_transfer() {
    remember_hotkey_owner();
    TRANSFERS_HOTKEY_INTENT.store(INTENT_NEW, Ordering::SeqCst);
}

/// Drop a pending Transfers hotkey intent (tests that must not leak the latch).
pub fn clear_transfers_hotkey_intent() {
    TRANSFERS_HOTKEY_INTENT.store(INTENT_NONE, Ordering::SeqCst);
    clear_hotkey_owner();
}

/// Drain one pending Transfers hotkey intent, if any.
pub(crate) fn take_transfers_hotkey_intent() -> Option<TransfersHotkey> {
    if TRANSFERS_HOTKEY_INTENT.load(Ordering::SeqCst) != INTENT_NONE
        && !hotkey_belongs_to_current_thread()
    {
        return None;
    }
    match TRANSFERS_HOTKEY_INTENT.swap(INTENT_NONE, Ordering::SeqCst) {
        INTENT_OPEN => {
            clear_hotkey_owner();
            Some(TransfersHotkey::Open)
        }
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
    /// Worker-projected destination reachability. `Some(false)` means the row
    /// is unreachable and is painted degraded; `None` means no probe result has
    /// been published yet. Never infer reachability from a missing result.
    pub peer_reachable: Option<bool>,
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

/// Tone for the Transfers sync-pair notice line. Refuse stays danger; queued
/// producer status matches the CLI stdout (not a second store).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SyncPairNoticeKind {
    #[default]
    Refuse,
    Queued,
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
    notice_kind: SyncPairNoticeKind,
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
        self.clear_notice();
    }

    fn begin_edit(&mut self, pair: &SyncPairView) {
        self.editor_open = true;
        self.edit_id = Some(pair.id.clone());
        self.draft_id = pair.id.clone();
        self.draft_source = pair.source.clone();
        self.draft_dest = pair.dest.clone();
        self.draft_interval = format_interval_draft(pair.every_secs);
        self.draft_bwlimit = pair.bwlimit.clone().unwrap_or_default();
        self.clear_notice();
    }

    fn close(&mut self) {
        self.editor_open = false;
        self.edit_id = None;
        self.clear_notice();
    }

    fn clear_notice(&mut self) {
        self.notice = None;
        self.notice_kind = SyncPairNoticeKind::Refuse;
    }

    fn set_refuse(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
        self.notice_kind = SyncPairNoticeKind::Refuse;
    }

    fn set_queued(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
        self.notice_kind = SyncPairNoticeKind::Queued;
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
                    .color(theme_color(ui, Style::TEXT_STRONG)),
            );
            ui.label(
                egui::RichText::new("shared ledger mirror · recurring sync pairs")
                    .small()
                    .color(theme_color(ui, Style::TEXT_DIM)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::icon_button(
                    ui,
                    icons::FILE_LINK,
                    Style::SP_M,
                    theme_color(ui, Style::ACCENT),
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
                .color(theme_color(ui, Style::TEXT_STRONG)),
        );
        ui.label(
            egui::RichText::new("next-run and last-result come from the transfers worker")
                .small()
                .color(theme_color(ui, Style::TEXT_DIM)),
        );
        self.sync_pair_notice(ui);

        if self.transfers_ui.editor_open {
            self.sync_pair_editor(ui);
        }

        let pairs = self.sync_pair_views.clone();
        if pairs.is_empty() && !self.transfers_ui.editor_open {
            ui.label(
                egui::RichText::new("No sync pairs saved.")
                    .small()
                    .color(theme_color(ui, Style::TEXT_DIM)),
            );
            ui.label(
                egui::RichText::new("Create one here or with `mackesd transfer sync-pair add`.")
                    .small()
                    .color(theme_color(ui, Style::TEXT_DIM)),
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
                    .color(theme_color(ui, Style::TEXT_STRONG)),
            );
            ui.add_space(Style::SP_XS);
            // Consume before the text fields so Enter/Escape cannot be eaten by
            // a focused draft. Same Save/Remove path as the buttons.
            if ui
                .ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            {
                self.transfers_ui.close();
                return;
            }
            let submit = ui
                .ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            if self.transfers_ui.edit_id.is_some() {
                // Replace-by-ID, matching `mackesd transfer sync-pair add --id`.
                // A mutable id here would mint a second row and orphan the original.
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Id")
                            .small()
                            .color(theme_color(ui, Style::TEXT_DIM)),
                    );
                    ui.label(
                        egui::RichText::new(&self.transfers_ui.draft_id)
                            .strong()
                            .color(theme_color(ui, Style::TEXT_STRONG)),
                    );
                });
            } else {
                labeled_edit(ui, "Id", &mut self.transfers_ui.draft_id);
            }
            labeled_edit(ui, "Interval", &mut self.transfers_ui.draft_interval);
            ui.label(
                egui::RichText::new("positive duration such as 30s, 5m, 1h, or seconds")
                    .small()
                    .color(theme_color(ui, Style::TEXT_DIM)),
            );
            labeled_edit(ui, "Source", &mut self.transfers_ui.draft_source);
            labeled_edit(ui, "Destination", &mut self.transfers_ui.draft_dest);
            labeled_edit(ui, "Bwlimit", &mut self.transfers_ui.draft_bwlimit);
            ui.horizontal(|ui| {
                if submit || ui.button("Save").clicked() {
                    self.save_sync_pair_draft();
                }
                if ui.button("Cancel").clicked() {
                    self.transfers_ui.close();
                }
            });
        });
    }

    fn sync_pair_notice(&self, ui: &mut egui::Ui) {
        let Some(notice) = &self.transfers_ui.notice else {
            return;
        };
        let tone = match self.transfers_ui.notice_kind {
            SyncPairNoticeKind::Refuse => Style::DANGER,
            SyncPairNoticeKind::Queued => Style::TEXT_DIM,
        };
        ui.label(
            egui::RichText::new(notice)
                .small()
                .color(theme_color(ui, tone)),
        );
    }

    fn sync_pair_row(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        pair: &SyncPairView,
    ) {
        let degraded = pair.peer_reachable == Some(false);
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
                icons::icon(
                    ui,
                    icons::XFER_ROW,
                    Style::SP_M,
                    theme_color(ui, title_tone),
                );
                ui.label(
                    egui::RichText::new(&pair.id)
                        .strong()
                        .color(theme_color(ui, title_tone)),
                );
                if degraded {
                    ui.label(
                        egui::RichText::new("unreachable")
                            .small()
                            .strong()
                            .color(theme_color(ui, Style::WARN)),
                    );
                }
                ui.label(
                    egui::RichText::new(format!(
                        "every {}",
                        format_interval_draft(pair.every_secs)
                    ))
                    .small()
                    .color(theme_color(ui, meta_tone)),
                );
            });
            ui.label(
                egui::RichText::new(format!("{} → {}", pair.source, pair.dest))
                    .small()
                    .color(theme_color(ui, meta_tone)),
            );
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format_next_run(data.now_unix_ms(), pair.next_run_unix_ms))
                        .small()
                        .color(theme_color(ui, meta_tone)),
                );
                ui.label(
                    egui::RichText::new(format_last_result(pair.last_result.as_deref()))
                        .small()
                        .color(theme_color(ui, meta_tone)),
                );
                if let Some(limit) = &pair.bwlimit {
                    ui.label(
                        egui::RichText::new(format!("bwlimit {limit}"))
                            .small()
                            .color(theme_color(ui, meta_tone)),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::icon_button(
                        ui,
                        icons::FILE_UNLINK,
                        Style::SP_M,
                        theme_color(ui, Style::DANGER),
                        "Remove sync pair",
                    )
                    .clicked()
                    {
                        self.remove_sync_pair(&pair.id);
                    }
                    if icons::icon_button(
                        ui,
                        icons::DOC_ROW,
                        Style::SP_M,
                        theme_color(ui, Style::TEXT_DIM),
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
        // Same order as `mackesd transfer sync-pair add`: trim, parse interval,
        // validate source/destination/NUL/bwlimit/id, then slug. Identical
        // requests must refuse with the same copy and must not mint a second
        // store row on edit (replace-by-ID, matching `add --id`).
        let source = self.transfers_ui.draft_source.trim().to_owned();
        let dest = self.transfers_ui.draft_dest.trim().to_owned();
        let bwlimit_raw = self.transfers_ui.draft_bwlimit.trim().to_owned();
        let interval_raw = self.transfers_ui.draft_interval.trim().to_owned();
        let Some(every_secs) = parse_interval_secs(&interval_raw) else {
            // Same refusal text as `mackesd transfer sync-pair add` so the
            // editor cannot look queued while the CLI would have failed fast.
            self.transfers_ui.set_refuse(format!(
                "malformed interval `{interval_raw}` (expected a positive duration such as 30s, 5m, 1h, or seconds)"
            ));
            return;
        };
        if source.is_empty() || dest.is_empty() {
            self.transfers_ui
                .set_refuse("sync pair requires non-empty source and destination");
            return;
        }
        if source.as_bytes().contains(&0) || dest.as_bytes().contains(&0) {
            self.transfers_ui
                .set_refuse("sync pair source and destination must not contain NUL bytes");
            return;
        }
        let bwlimit = if bwlimit_raw.is_empty() {
            None
        } else if !valid_sync_pair_bwlimit(&bwlimit_raw) {
            self.transfers_ui
                .set_refuse(format!("invalid sync pair bwlimit `{bwlimit_raw}`"));
            return;
        } else {
            Some(bwlimit_raw)
        };
        let id = if let Some(edit_id) = self.transfers_ui.edit_id.clone() {
            edit_id
        } else {
            let id = self.transfers_ui.draft_id.trim();
            if id.is_empty() {
                slug_pair_id(&source, &dest)
            } else {
                id.to_owned()
            }
        };
        if !valid_pair_id(&id) {
            self.transfers_ui
                .set_refuse(format!("invalid sync pair id `{id}`"));
            return;
        }
        let queued = format!(
            "transfer sync-pair add: queued {id} every {every_secs}s (the daemon saves it on its next tick)"
        );
        self.sync_pair_sink.emit(SyncPairCommand::Save {
            id,
            source,
            dest,
            every_secs,
            bwlimit,
        });
        self.transfers_ui.close();
        self.transfers_ui.set_queued(queued);
    }

    /// Remove a mirrored pair by id. Unknown and malformed ids refuse — same
    /// early check as `mackesd transfer sync-pair remove` — against the bound
    /// worker projection, never a second store.
    fn remove_sync_pair(&mut self, id: &str) {
        let id = id.trim();
        if !valid_pair_id(id) {
            self.transfers_ui
                .set_refuse(format!("invalid sync pair id `{id}`"));
            return;
        }
        if !self.sync_pair_views.iter().any(|pair| pair.id == id) {
            self.transfers_ui.set_refuse(format!(
                "no sync pair `{id}` in the store (see `mackesd transfer sync-pair list`)"
            ));
            return;
        }
        let queued = format!(
            "transfer sync-pair remove: requested for {id} (the daemon applies it on its next tick)"
        );
        self.sync_pair_sink
            .emit(SyncPairCommand::Remove { id: id.to_owned() });
        if self.transfers_ui.edit_id.as_deref() == Some(id) {
            self.transfers_ui.close();
        }
        self.transfers_ui.set_queued(queued);
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

    #[cfg(test)]
    pub(crate) fn remove_sync_pair_for_test(&mut self, id: &str) {
        self.remove_sync_pair(id);
    }

    #[cfg(test)]
    pub(crate) fn begin_edit_sync_pair_for_test(&mut self, pair: &SyncPairView) {
        self.transfers_ui.begin_edit(pair);
    }

    /// Fill editor drafts without publishing. Preserves [`TransfersUi::edit_id`]
    /// so create vs replace-by-ID stays under the caller's control.
    #[cfg(test)]
    pub(crate) fn fill_sync_pair_draft_for_test(
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
    }

    #[cfg(test)]
    pub(crate) fn save_open_sync_pair_draft_for_test(&mut self) {
        self.save_sync_pair_draft();
    }

    #[cfg(test)]
    pub(crate) fn sync_pair_drafts_for_test(&self) -> (String, String, String, String, String) {
        (
            self.transfers_ui.draft_id.clone(),
            self.transfers_ui.draft_interval.clone(),
            self.transfers_ui.draft_source.clone(),
            self.transfers_ui.draft_dest.clone(),
            self.transfers_ui.draft_bwlimit.clone(),
        )
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
                .color(theme_color(ui, Style::TEXT_STRONG)),
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
                ui.label(
                    egui::RichText::new("No transfers in flight.")
                        .color(theme_color(ui, Style::TEXT_DIM)),
                );
                ui.label(
                    egui::RichText::new(
                        "Share a file from the Files mode to start one — its progress mirrors here.",
                    )
                    .small()
                    .color(theme_color(ui, Style::TEXT_DIM)),
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
                icons::icon(
                    ui,
                    icons::XFER_ROW,
                    Style::SP_M,
                    theme_color(ui, Style::ACCENT),
                );
                icons::icon(ui, glyph, Style::SP_M, theme_color(ui, Style::TEXT_DIM))
                    .comms_hover_text(dir_hint);
                ui.label(
                    egui::RichText::new(short_file(job))
                        .strong()
                        .color(theme_color(ui, Style::TEXT_STRONG)),
                );
                ui.label(
                    egui::RichText::new(format!("· {}", method_label(job.method)))
                        .small()
                        .color(theme_color(ui, Style::TEXT_DIM)),
                );
            });

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(transfer_state_label(job.state))
                        .small()
                        .strong()
                        .color(theme_color(ui, transfer_state_color(job.state))),
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
                        .color(theme_color(ui, Style::TEXT_DIM)),
                    );
                } else if job.moved > 0 {
                    ui.label(
                        egui::RichText::new(fmt_bytes(job.moved))
                            .small()
                            .color(theme_color(ui, Style::TEXT_DIM)),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("progress pending")
                            .small()
                            .color(theme_color(ui, Style::TEXT_DIM)),
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
                    theme_color(ui, Style::DANGER),
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
                    theme_color(ui, Style::TEXT_DIM),
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
                    theme_color(ui, Style::DANGER),
                    "Cancel",
                )
                .clicked()
                {
                    self.control_transfer(sink, job.transfer, TransferControl::Cancel);
                }
                if icons::icon_button(
                    ui,
                    icons::TRANSFER_RESUME,
                    Style::SP_M,
                    theme_color(ui, Style::OK),
                    "Resume",
                )
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
                    theme_color(ui, Style::DANGER),
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
        ui.label(
            egui::RichText::new(label)
                .small()
                .color(theme_color(ui, Style::TEXT_DIM)),
        );
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

/// Future duration using the same buckets as Communications `relative_age` and
/// the CLI `sync-pair list` next-run column. Kept local so next-run copy cannot
/// drift if `relative_age`'s argument order is used as an "until" trick.
fn relative_until(now_ms: i64, then_ms: i64) -> String {
    let secs = then_ms.saturating_sub(now_ms).max(0) / 1_000;
    if secs < 45 {
        return "now".to_owned();
    }
    let mins = secs / 60;
    if mins < 1 {
        return "1m".to_owned();
    }
    if mins < 60 {
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h");
    }
    format!("{}d", hours / 24)
}

fn format_next_run(now_unix_ms: i64, next_run_unix_ms: Option<i64>) -> String {
    match next_run_unix_ms {
        None => "next-run pending".to_owned(),
        Some(ts) if ts <= now_unix_ms => "due now".to_owned(),
        Some(ts) => {
            let until = relative_until(now_unix_ms, ts);
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

fn valid_sync_pair_bwlimit(raw: &str) -> bool {
    !raw.is_empty()
        && raw.len() <= 32
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
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

#[cfg(test)]
mod tests {
    use super::{
        format_interval_draft, format_last_result, format_next_run, parse_interval_secs,
        slug_pair_id, SyncPairCommand, SyncPairView,
    };
    use crate::CommunicationsSurface;
    use mde_egui::egui;

    fn projected_pair(id: &str) -> SyncPairView {
        SyncPairView {
            id: id.to_owned(),
            source: "/src".into(),
            dest: "/dst".into(),
            every_secs: 900,
            bwlimit: Some("2m".into()),
            next_run_unix_ms: Some(1_060_000),
            last_result: Some("ok".into()),
            peer_reachable: Some(true),
        }
    }

    #[test]
    fn parse_interval_secs_refuses_malformed_zero_and_unknown_units() {
        for raw in [
            "", "abc", "nope", "0", "0s", "-5m", "10y", "1.5h", "h", "15x",
        ] {
            assert_eq!(
                parse_interval_secs(raw),
                None,
                "interval `{raw}` must refuse like the CLI"
            );
        }
        assert_eq!(parse_interval_secs("30s"), Some(30));
        assert_eq!(parse_interval_secs("5m"), Some(300));
        assert_eq!(parse_interval_secs("15m"), Some(900));
        assert_eq!(parse_interval_secs("1h"), Some(3600));
        assert_eq!(parse_interval_secs("2d"), Some(172_800));
        assert_eq!(parse_interval_secs("90"), Some(90));
        assert_eq!(parse_interval_secs(" 15m "), Some(900));
    }

    #[test]
    fn editor_refuses_unknown_pair_id_and_malformed_interval_without_publishing() {
        let mut surface = CommunicationsSurface::new();
        surface.set_sync_pair_views(vec![projected_pair("docs")]);

        surface.save_sync_pair_draft_for_test("docs", "nope", "/src", "/dst", None);
        assert!(
            surface.drain_sync_pair_commands().is_empty(),
            "malformed interval must not publish a Save verb"
        );
        assert!(
            surface.editor_open_for_test(),
            "malformed interval must keep the editor open"
        );
        let interval_notice = surface
            .sync_pair_notice_for_test()
            .expect("malformed interval must refuse visibly");
        assert!(
            interval_notice.contains("malformed interval") && interval_notice.contains("nope"),
            "notice must name the refused interval, matching the CLI: {interval_notice}"
        );

        surface.remove_sync_pair_for_test("ghost");
        assert!(
            surface.drain_sync_pair_commands().is_empty(),
            "unknown pair id must not publish a Remove verb"
        );
        assert!(
            surface.sync_pair_notice_for_test().is_some_and(|n| {
                n.contains("no sync pair `ghost`") && n.contains("sync-pair list")
            }),
            "unknown pair id must refuse with the CLI remove text"
        );

        // Edit is replace-by-ID (CLI add --id). A vanished projection still
        // publishes Save; the worker upserts. A renamed draft id must not
        // orphan the original row.
        surface.begin_edit_sync_pair_for_test(&projected_pair("docs"));
        surface.set_sync_pair_views(vec![]);
        surface.save_sync_pair_draft_for_test("renamed", "15m", "/src", "/dst", Some("2m"));
        assert_eq!(
            surface.drain_sync_pair_commands(),
            vec![SyncPairCommand::Save {
                id: "docs".into(),
                source: "/src".into(),
                dest: "/dst".into(),
                every_secs: 900,
                bwlimit: Some("2m".into()),
            }]
        );
        assert!(
            !surface.editor_open_for_test(),
            "successful replace-by-ID save must close the editor"
        );
        assert!(
            surface
                .sync_pair_notice_for_test()
                .is_some_and(|n| n.contains("queued docs every 900s") && n.contains("next tick")),
            "save must keep the CLI queued-next-tick notice after close"
        );
    }

    #[test]
    fn remove_refuses_unknown_and_malformed_pair_ids() {
        let mut surface = CommunicationsSurface::new();
        surface.set_sync_pair_views(vec![projected_pair("docs")]);

        surface.remove_sync_pair_for_test("ghost");
        assert!(
            surface.drain_sync_pair_commands().is_empty(),
            "unknown pair id must not publish a verb"
        );
        assert!(
            surface
                .sync_pair_notice_for_test()
                .is_some_and(|n| n.contains("no sync pair `ghost`")),
            "unknown pair id must refuse with the CLI remove text"
        );

        surface.remove_sync_pair_for_test("../etc");
        assert!(
            surface.drain_sync_pair_commands().is_empty(),
            "malformed pair id must not publish a verb"
        );
        assert!(
            surface
                .sync_pair_notice_for_test()
                .is_some_and(|n| n.contains("invalid sync pair id") && n.contains("../etc")),
            "malformed pair id must refuse with the CLI id text"
        );

        surface.remove_sync_pair_for_test("docs");
        assert_eq!(
            surface.drain_sync_pair_commands(),
            vec![SyncPairCommand::Remove { id: "docs".into() }]
        );
        assert!(
            surface.sync_pair_notice_for_test().is_some_and(|n| {
                n.contains("remove: requested for docs") && n.contains("next tick")
            }),
            "remove must keep the CLI queued-next-tick notice"
        );
    }

    #[test]
    fn editor_save_refuses_match_cli_and_trims_like_cli() {
        let mut surface = CommunicationsSurface::new();

        surface.save_sync_pair_draft_for_test("../escape", "15m", "/src", "/dst", None);
        assert!(surface.drain_sync_pair_commands().is_empty());
        assert!(
            surface
                .sync_pair_notice_for_test()
                .is_some_and(|n| n.contains("invalid sync pair id") && n.contains("../escape")),
            "invalid id must refuse with the CLI text"
        );
        assert!(
            surface.editor_open_for_test(),
            "refused save must keep the editor open"
        );

        surface.save_sync_pair_draft_for_test("docs", "15m", "", "/dst", None);
        assert!(surface.drain_sync_pair_commands().is_empty());
        assert!(
            surface
                .sync_pair_notice_for_test()
                .is_some_and(|n| n.contains("non-empty source")),
            "empty source must refuse with the CLI text"
        );

        surface.save_sync_pair_draft_for_test("docs", "15m", "/src", "", None);
        assert!(surface.drain_sync_pair_commands().is_empty());
        assert!(
            surface
                .sync_pair_notice_for_test()
                .is_some_and(|n| n.contains("non-empty source and destination")),
            "empty destination must refuse with the CLI text"
        );

        surface.save_sync_pair_draft_for_test("docs", "15m", "/src\0", "/dst", None);
        assert!(surface.drain_sync_pair_commands().is_empty());
        assert!(
            surface
                .sync_pair_notice_for_test()
                .is_some_and(|n| n.contains("NUL bytes")),
            "NUL source must refuse with the CLI text"
        );

        surface.save_sync_pair_draft_for_test("docs", "15m", "/src", "/dst\0", None);
        assert!(surface.drain_sync_pair_commands().is_empty());
        assert!(
            surface
                .sync_pair_notice_for_test()
                .is_some_and(|n| n.contains("NUL bytes")),
            "NUL destination must refuse with the CLI text"
        );

        for bad_id in [".", ".."] {
            surface.save_sync_pair_draft_for_test(bad_id, "15m", "/src", "/dst", None);
            assert!(
                surface.drain_sync_pair_commands().is_empty(),
                "id `{bad_id}` must not publish"
            );
            assert!(
                surface
                    .sync_pair_notice_for_test()
                    .is_some_and(|n| { n.contains("invalid sync pair id") && n.contains(bad_id) }),
                "id `{bad_id}` must refuse with the CLI text"
            );
        }

        // CLI parses interval before id: combined hostility must name the interval.
        surface.save_sync_pair_draft_for_test("../escape", "nope", "/src", "/dst", None);
        assert!(surface.drain_sync_pair_commands().is_empty());
        assert!(
            surface
                .sync_pair_notice_for_test()
                .is_some_and(|n| n.contains("malformed interval") && n.contains("nope")),
            "malformed interval must win over invalid id, matching CLI add order"
        );

        surface.save_sync_pair_draft_for_test("docs", "15m", "/src", "/dst", Some("1m;rm"));
        assert!(surface.drain_sync_pair_commands().is_empty());
        assert!(
            surface
                .sync_pair_notice_for_test()
                .is_some_and(|n| n.contains("bwlimit") && n.contains("1m;rm")),
            "hostile bwlimit must refuse with the CLI text"
        );

        surface.save_sync_pair_draft_for_test("", "15m", "/src", "/dst", None);
        assert_eq!(
            surface.drain_sync_pair_commands(),
            vec![SyncPairCommand::Save {
                id: slug_pair_id("/src", "/dst"),
                source: "/src".into(),
                dest: "/dst".into(),
                every_secs: 900,
                bwlimit: None,
            }]
        );

        let mut surface = CommunicationsSurface::new();
        surface.save_sync_pair_draft_for_test(
            "  docs  ",
            "15m",
            " /src ",
            " /dst ",
            Some("  2m  "),
        );
        assert_eq!(
            surface.drain_sync_pair_commands(),
            vec![SyncPairCommand::Save {
                id: "docs".into(),
                source: "/src".into(),
                dest: "/dst".into(),
                every_secs: 900,
                bwlimit: Some("2m".into()),
            }]
        );
    }

    #[test]
    fn next_run_last_result_copy_matches_cli_list() {
        assert_eq!(format_next_run(1_000, None), "next-run pending");
        assert_eq!(format_next_run(1_000_000, Some(500_000)), "due now");
        assert_eq!(
            format_next_run(1_000_000, Some(1_000_000 + 60_000)),
            "next in 1m"
        );
        assert_eq!(
            format_next_run(1_000_000, Some(1_000_000 + 10_000)),
            "due soon"
        );
        assert_eq!(format_last_result(None), "never run");
        assert_eq!(format_last_result(Some("")), "never run");
        assert_eq!(format_last_result(Some("done")), "last: done");
        assert_eq!(format_interval_draft(900), "15m");
        assert_eq!(slug_pair_id("/src", "/dst"), "src--dst");
    }

    #[test]
    fn editor_edit_round_trips_interval_source_dest_bwlimit_replace_by_id() {
        let mut surface = CommunicationsSurface::new();
        let pair = projected_pair("docs");
        surface.set_sync_pair_views(vec![pair.clone()]);
        surface.begin_edit_sync_pair_for_test(&pair);
        assert_eq!(
            surface.sync_pair_drafts_for_test(),
            (
                "docs".into(),
                "15m".into(),
                "/src".into(),
                "/dst".into(),
                "2m".into()
            )
        );

        surface.save_open_sync_pair_draft_for_test();
        assert_eq!(
            surface.drain_sync_pair_commands(),
            vec![SyncPairCommand::Save {
                id: "docs".into(),
                source: "/src".into(),
                dest: "/dst".into(),
                every_secs: 900,
                bwlimit: Some("2m".into()),
            }]
        );

        surface.begin_edit_sync_pair_for_test(&pair);
        surface.fill_sync_pair_draft_for_test("renamed", "1h", "/a", "/b", Some("1m"));
        surface.save_open_sync_pair_draft_for_test();
        assert_eq!(
            surface.drain_sync_pair_commands(),
            vec![SyncPairCommand::Save {
                id: "docs".into(),
                source: "/a".into(),
                dest: "/b".into(),
                every_secs: 3600,
                bwlimit: Some("1m".into()),
            }],
            "edit must replace-by-ID and publish the edited interval/source/dest/bwlimit"
        );
    }

    #[test]
    fn remove_trims_id_like_the_editor_fields() {
        let mut surface = CommunicationsSurface::new();
        surface.set_sync_pair_views(vec![projected_pair("docs")]);
        surface.remove_sync_pair_for_test("  docs  ");
        assert_eq!(
            surface.drain_sync_pair_commands(),
            vec![SyncPairCommand::Remove { id: "docs".into() }]
        );
    }

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn painted_labels(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
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

    fn render_transfers_editor(
        surface: &mut CommunicationsSurface,
        events: Vec<egui::Event>,
    ) -> Vec<String> {
        use crate::fixture::FixtureData;
        use crate::CommandSink;
        use mde_egui::Style;

        let data = FixtureData::new("eagle", 1_000_000);
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut sink = CommandSink::new();
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 700.0),
                )),
                events,
                time: Some(0.0),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    surface.transfers_body(ui, &data, &mut sink);
                });
            },
        );
        painted_labels(&out.shapes)
    }

    #[test]
    fn editor_paints_create_and_edit_fields_and_honours_enter_escape() {
        let mut surface = CommunicationsSurface::new();
        surface.begin_new_transfer();
        surface.fill_sync_pair_draft_for_test("docs", "15m", "/src", "/dst", Some("2m"));
        let texts = render_transfers_editor(&mut surface, Vec::new());
        for label in [
            "New transfer",
            "Interval",
            "Source",
            "Destination",
            "Bwlimit",
            "30s, 5m, 1h",
        ] {
            assert!(
                texts.iter().any(|t| t.contains(label)),
                "create editor must paint `{label}`: {texts:?}"
            );
        }

        let _ = render_transfers_editor(&mut surface, vec![key(egui::Key::Enter)]);
        assert_eq!(
            surface.drain_sync_pair_commands(),
            vec![SyncPairCommand::Save {
                id: "docs".into(),
                source: "/src".into(),
                dest: "/dst".into(),
                every_secs: 900,
                bwlimit: Some("2m".into()),
            }],
            "Enter must publish Save through TransferVerb, not a second store"
        );
        assert!(
            !surface.editor_open_for_test(),
            "Enter save must close the editor"
        );

        let pair = projected_pair("docs");
        surface.set_sync_pair_views(vec![pair.clone()]);
        surface.begin_edit_sync_pair_for_test(&pair);
        let texts = render_transfers_editor(&mut surface, Vec::new());
        for label in [
            "Edit sync pair",
            "Interval",
            "Source",
            "Destination",
            "Bwlimit",
        ] {
            assert!(
                texts.iter().any(|t| t.contains(label)),
                "edit editor must paint `{label}`: {texts:?}"
            );
        }
        assert!(
            texts.iter().any(|t| t == "docs"),
            "edit must show the locked id: {texts:?}"
        );

        surface.begin_new_transfer();
        let _ = render_transfers_editor(&mut surface, vec![key(egui::Key::Escape)]);
        assert!(
            !surface.editor_open_for_test(),
            "Escape must cancel without publishing"
        );
        assert!(
            surface.drain_sync_pair_commands().is_empty(),
            "Escape must not publish a verb"
        );
    }
}
