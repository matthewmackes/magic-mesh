//! Files mode — the files **linked into a space** (their references) and the
//! shared transfers that move them.
//!
//! A space owns **references**, not a private folder (spec §WL-FUNC-011): each
//! row renders a [`FileReferenceView`](mde_collab_types::FileReferenceView) — the
//! file's name, who linked it, its size, and its content address (the SHA-256 that
//! *is* the version identity in this content-addressed model) — alongside the
//! shared transfer's state, read from the WL-FUNC-006 ledger mirror
//! ([`TransferJobs`](mde_collab_types::TransferJobs)). This surface never owns a
//! second progress authority: byte progress (`moved`/`total`) is *mirrored* from
//! that ledger, never recomputed here.
//!
//! # Actions (all emit typed [`CollabCommand`]s into the sink)
//!
//! * **Link a file** — opens a picker that REUSES the file-manager's
//!   render-agnostic listing ([`mde_files::LocalFsBackend::list_dir`] + the
//!   [`FileRow`](mde_files::FileRow) model, the same core `mde-files-egui` renders)
//!   to choose a canonical file; picking one reads + hashes it into a
//!   [`FileRef`](mde_collab_types::FileRef) and emits
//!   [`LinkFile`](CollabCommand::LinkFile).
//! * **Remove from space** — a single-click
//!   [`UnlinkFile`](CollabCommand::UnlinkFile): it removes only *this space's
//!   reference*; the canonical file (and any other space's reference to it) is
//!   untouched.
//! * **Delete permanently** — a *distinct*, danger-tinted affordance gated behind
//!   typing the file's exact name (spec: a separate typed-confirm, not undoable).
//!   In this content-addressed, reference-counted model the collab-level primitive
//!   is still `UnlinkFile` — removing the reference — after which the canonical
//!   payload is garbage-collected by the core's purge gate once no space
//!   references it. The affordance's typed gate + copy is what distinguishes the
//!   not-undoable permanent removal from the safe "remove from space".
//! * **Share / transfer control** — every member may control a shared transfer:
//!   [`StartTransfer`](CollabCommand::StartTransfer) to begin sharing a linked file
//!   to members, and [`ControlTransfer`](CollabCommand::ControlTransfer)
//!   (pause / resume / cancel) once it is running — the state read from the shared
//!   ledger mirror.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{
    CollabCommand, FileRef, FileRefId, FileReferenceView, SpaceId, TransferControl,
    TransferDirection, TransferId, TransferJobView, TransferMethod, TransferState,
};
use mde_files::{LocalFsBackend, Mime};

use crate::icons::CommsHoverExt;
use crate::{icons, relative_age, CommandSink, CommunicationsSurface};

// Files-mode values are read from the collab read model or the local filesystem.
// Keep the complete values in the model, but put a bounded, single-line surface
// around anything that can be long or operator-controlled. The tooltip is also
// bounded so an adversarial name cannot create an unbounded popup.
const FILE_DISPLAY_MAX_CHARS: usize = 160;
const FILE_TOOLTIP_MAX_CHARS: usize = 512;
const FILE_ROW_ACTIONS_RESERVE: f32 = Style::SP_XL * 4.0;
const PICKER_SIZE_RESERVE: f32 = Style::SP_XL * 2.0;
/// The largest local payload this paint-thread file-reference path will hash.
/// Larger files belong on the transfer path rather than being materialized in
/// one GUI allocation.
const MAX_FILE_REF_BYTES: u64 = 100 * 1024 * 1024;

fn is_unsafe_display_char(ch: char) -> bool {
    ch.is_control()
        || matches!(
            ch,
            '\u{00ad}'
                | '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206f}'
                | '\u{feff}'
        )
}

/// Make untrusted text safe for a bounded Files-mode presentation without
/// changing the underlying value used by commands or exact-name confirmation.
/// Control and bidi-format characters become visible replacement glyphs, and a
/// Unicode-safe ellipsis marks truncation at the requested character limit.
fn safe_display_text(value: &str, max_chars: usize) -> String {
    let normalized: String = value
        .chars()
        .map(|ch| {
            if is_unsafe_display_char(ch) {
                '\u{fffd}'
            } else {
                ch
            }
        })
        .collect();
    let count = normalized.chars().count();
    if count <= max_chars {
        return normalized;
    }

    let keep = max_chars.saturating_sub(1);
    let mut clipped: String = normalized.chars().take(keep).collect();
    clipped.push('\u{2026}');
    clipped
}

fn safe_tooltip_text(value: &str) -> String {
    safe_display_text(value, FILE_TOOLTIP_MAX_CHARS)
}

/// A pending **permanent-delete** typed-confirm: the file being deleted, its exact
/// name (the string that must be typed to arm the delete), and the working buffer.
#[derive(Debug, Clone)]
pub(crate) struct PendingDelete {
    /// The file reference being permanently removed.
    pub(crate) file: FileRefId,
    /// The file's exact name — the delete arms only once this is typed verbatim.
    pub(crate) name: String,
    /// The confirm text field's working buffer.
    pub(crate) typed: String,
}

impl CommunicationsSurface {
    /// Render Files mode for the selected space: the "link a file" action, the
    /// linked-reference list with per-file transfer controls, and — when open — the
    /// link picker or the permanent-delete typed-confirm.
    pub(crate) fn files_body(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut CommandSink,
    ) {
        let Some(space) = self.selected_space() else {
            ui.label(
                egui::RichText::new("Select a space to see its shared files.")
                    .color(Style::TEXT_DIM),
            );
            return;
        };

        // Header — the mode title + the "Link a file" affordance (a space owns
        // references, so this adds a reference, it does not copy into a folder).
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Shared files")
                    .strong()
                    .color(Style::TEXT_STRONG),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::icon_button(
                    ui,
                    icons::FILE_LINK,
                    Style::SP_M,
                    Style::ACCENT,
                    "Link a file into this space",
                )
                .clicked()
                {
                    self.file_picker = Some(picker_start_dir());
                    self.files_notice = None;
                }
            });
        });
        ui.separator();

        // A transient, honest notice (e.g. an unreadable pick) — never silent.
        if let Some(notice) = self.files_notice.clone() {
            let display = safe_display_text(&notice, FILE_DISPLAY_MAX_CHARS);
            let response = ui.add(
                egui::Label::new(egui::RichText::new(display).small().color(Style::DANGER))
                    .truncate(),
            );
            let _ = response.comms_hover_text(safe_tooltip_text(&notice));
        }

        // The link picker takes over the body while open.
        if self.file_picker.is_some() {
            self.link_picker_ui(ui, sink, space);
            return;
        }

        // The permanent-delete typed-confirm interrupts the list while pending.
        if self.files_confirm_delete.is_some() {
            self.permanent_delete_confirm_ui(ui, sink, space);
            return;
        }

        // The linked-reference list.
        let refs = data.file_references(space);
        let jobs = data.transfer_jobs();
        match refs {
            Some(refs) if !refs.files.is_empty() => {
                egui::ScrollArea::vertical()
                    .id_salt("collab-files")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for view in &refs.files {
                            let job =
                                jobs.and_then(|j| j.jobs.iter().find(|job| job.file == view.file));
                            self.file_row(ui, sink, space, view, job, data.now_unix_ms());
                            ui.add_space(Style::SP_XS);
                        }
                    });
            }
            _ => {
                ui.label(
                    egui::RichText::new("No files linked into this space yet.")
                        .color(Style::TEXT_DIM),
                );
                ui.label(
                    egui::RichText::new(
                        "Link a file to share a reference — joining members backfill the current set.",
                    )
                    .small()
                    .color(Style::TEXT_DIM),
                );
            }
        }
    }

    /// One linked-file row: the header (glyph · name · owner · age), the honest
    /// facts (size + content address), the shared-transfer state + controls, and
    /// the reference-remove vs. permanent-delete affordances.
    fn file_row(
        &mut self,
        ui: &mut egui::Ui,
        sink: &mut CommandSink,
        space: SpaceId,
        view: &FileReferenceView,
        job: Option<&TransferJobView>,
        now_unix_ms: i64,
    ) {
        mde_egui::card()
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    icons::icon(ui, icons::FILE_ROW, Style::SP_M, Style::ACCENT);
                    let display_name =
                        safe_display_text(&view.reference.name, FILE_DISPLAY_MAX_CHARS);
                    let response = ui.add(
                        egui::Label::new(
                            egui::RichText::new(display_name)
                                .strong()
                                .color(Style::TEXT_STRONG),
                        )
                        .truncate(),
                    );
                    let _ = response.comms_hover_text(safe_tooltip_text(&view.reference.name));
                });

                // Keep owner/age metadata and actions in a separate bounded row
                // so a long name cannot push either action off the card.
                ui.horizontal(|ui| {
                    let metadata = format!(
                        "· {} · {}",
                        view.linked_by.as_str(),
                        relative_age(now_unix_ms, view.linked_unix_ms)
                    );
                    let metadata_width =
                        (ui.available_width() - FILE_ROW_ACTIONS_RESERVE).max(Style::SP_S);
                    let display_metadata =
                        safe_display_text(&metadata, FILE_DISPLAY_MAX_CHARS);
                    let response = ui.add_sized(
                        [metadata_width, Style::SP_L],
                        egui::Label::new(
                            egui::RichText::new(display_metadata)
                                .small()
                                .color(Style::TEXT_DIM),
                        )
                        .truncate(),
                    );
                    let _ = response.comms_hover_text(safe_tooltip_text(&metadata));

                    // Reference-remove (safe) + permanent-delete (danger, gated),
                    // right-aligned in the reserved action area.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::icon_button(
                            ui,
                            icons::FILE_DELETE_PERMANENT,
                            Style::SP_M,
                            Style::DANGER,
                            "Delete permanently — removes the canonical file for everyone (not undoable)",
                        )
                        .clicked()
                        {
                            self.request_permanent_delete(view.file, view.reference.name.clone());
                        }
                        if icons::icon_button(
                            ui,
                            icons::FILE_UNLINK,
                            Style::SP_M,
                            Style::TEXT_DIM,
                            "Remove from space — unlinks this space's reference; the file itself is kept",
                        )
                        .clicked()
                        {
                            self.remove_reference(sink, space, view.file);
                        }
                    });
                });

                // Honest facts: size + the content address (the version identity in
                // a content-addressed model).
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(fmt_bytes(view.reference.size))
                            .small()
                            .color(Style::TEXT_DIM),
                    );
                    if let Some(mime) = view.reference.mime.as_deref() {
                        let display_mime = safe_display_text(mime, FILE_DISPLAY_MAX_CHARS);
                        let response = ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("· type {display_mime}"))
                                    .small()
                                    .color(Style::TEXT_DIM),
                            )
                            .truncate(),
                        );
                    let _ = response.comms_hover_text(safe_tooltip_text(mime));
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "· content {}",
                            short_hash(&view.reference.sha256_hex),
                        ))
                        .small()
                        .color(Style::TEXT_DIM),
                    );
                });

                self.transfer_controls(ui, sink, space, view.file, job);
            });
    }

    /// The shared-transfer row for a file: its state read from the ledger mirror,
    /// plus the control every member may drive. No job yet → a "Share to members"
    /// [`StartTransfer`](CollabCommand::StartTransfer); a live job → its state, byte
    /// progress (mirrored, never recomputed), and the pause/resume/cancel controls.
    fn transfer_controls(
        &self,
        ui: &mut egui::Ui,
        sink: &mut CommandSink,
        space: SpaceId,
        file: FileRefId,
        job: Option<&TransferJobView>,
    ) {
        ui.horizontal(|ui| {
            let Some(job) = job else {
                if icons::icon_button(
                    ui,
                    icons::TRANSFER_SEND,
                    Style::SP_M,
                    Style::TEXT_DIM,
                    "Share to members — start a transfer",
                )
                .clicked()
                {
                    self.start_transfer_to_members(sink, space, file);
                }
                ui.label(
                    egui::RichText::new("Not shared yet")
                        .small()
                        .color(Style::TEXT_DIM),
                );
                return;
            };

            ui.label(
                egui::RichText::new(transfer_state_label(job.state))
                    .small()
                    .strong()
                    .color(transfer_state_color(job.state)),
            );
            // Mirrored byte progress (WL-FUNC-006). `total == 0` means the ledger
            // has not reported a size yet — shown honestly, never faked to 100%.
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
            }

            // The controls appropriate to the state (terminal states carry none).
            match job.state {
                TransferState::Active => {
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
                    self.cancel_button(ui, sink, job.transfer);
                }
                TransferState::Paused => {
                    if icons::icon_button(
                        ui,
                        icons::TRANSFER_RESUME,
                        Style::SP_M,
                        Style::OK,
                        "Resume",
                    )
                    .clicked()
                    {
                        self.control_transfer(sink, job.transfer, TransferControl::Resume);
                    }
                    self.cancel_button(ui, sink, job.transfer);
                }
                TransferState::Queued => self.cancel_button(ui, sink, job.transfer),
                TransferState::Completed | TransferState::Failed | TransferState::Canceled => {}
            }
        });
    }

    /// The cancel control shared by the queued/active/paused states.
    fn cancel_button(&self, ui: &mut egui::Ui, sink: &mut CommandSink, transfer: TransferId) {
        if icons::icon_button(
            ui,
            icons::TRANSFER_CANCEL,
            Style::SP_M,
            Style::DANGER,
            "Cancel",
        )
        .clicked()
        {
            self.control_transfer(sink, transfer, TransferControl::Cancel);
        }
    }

    /// The "link a file" picker: reuses the file-manager's [`LocalFsBackend`]
    /// listing (the same render-agnostic core `mde-files-egui` renders) to browse
    /// directories; a folder click descends, a file click links it into the space.
    fn link_picker_ui(&mut self, ui: &mut egui::Ui, sink: &mut CommandSink, space: SpaceId) {
        let Some(dir) = self.file_picker.clone() else {
            return;
        };
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Link a file")
                    .strong()
                    .color(Style::TEXT_STRONG),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::icon_button(ui, "window-close", Style::SP_M, Style::TEXT_DIM, "Cancel")
                    .clicked()
                {
                    self.file_picker = None;
                }
            });
        });
        let directory = dir.display().to_string();
        let directory_display = safe_display_text(&directory, FILE_DISPLAY_MAX_CHARS);
        let response = ui.add(
            egui::Label::new(
                egui::RichText::new(directory_display)
                    .small()
                    .color(Style::TEXT_DIM),
            )
            .truncate(),
        );
        let _ = response.comms_hover_text(safe_tooltip_text(&directory));
        ui.separator();

        // Collected inside the closure, applied after (so `self` is not borrowed
        // across the `ScrollArea`'s `&mut self`-free body).
        let mut navigate: Option<PathBuf> = None;
        let mut pick: Option<PathBuf> = None;
        egui::ScrollArea::vertical()
            .id_salt("collab-file-picker")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(parent) = dir.parent() {
                    ui.horizontal(|ui| {
                        icons::icon(ui, icons::PICKER_UP, Style::SP_M, Style::TEXT_DIM);
                        if ui
                            .selectable_label(false, egui::RichText::new("..").color(Style::TEXT))
                            .clicked()
                        {
                            navigate = Some(parent.to_path_buf());
                        }
                    });
                }
                for row in LocalFsBackend::list_dir(&dir) {
                    let Some(path) = row.path.clone() else {
                        continue; // virtual (mesh/peer) rows carry no local path
                    };
                    let is_dir = row.mime == Mime::Folder;
                    let glyph = if is_dir {
                        icons::PICKER_FOLDER
                    } else {
                        icons::FILE_ROW
                    };
                    ui.horizontal(|ui| {
                        icons::icon(ui, glyph, Style::SP_M, Style::TEXT_DIM);
                        let name_width =
                            (ui.available_width() - PICKER_SIZE_RESERVE).max(Style::SP_S);
                        let display_name = safe_display_text(&row.name, FILE_DISPLAY_MAX_CHARS);
                        let response = ui.add_sized(
                            [name_width, Style::SP_L],
                            egui::Label::new(egui::RichText::new(display_name).color(Style::TEXT))
                                .truncate()
                                .sense(egui::Sense::click()),
                        );
                        let clicked = response.clicked();
                        let _ = response.comms_hover_text(safe_tooltip_text(&row.name));
                        if clicked {
                            if is_dir {
                                navigate = Some(PathBuf::from(path));
                            } else {
                                pick = Some(PathBuf::from(path));
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let display_size = safe_display_text(&row.size, FILE_DISPLAY_MAX_CHARS);
                            let response = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(display_size)
                                        .small()
                                        .color(Style::TEXT_DIM),
                                )
                                .truncate(),
                            );
                            let _ = response.comms_hover_text(safe_tooltip_text(&row.size));
                        });
                    });
                }
            });

        if let Some(dir) = navigate {
            self.file_picker = Some(dir);
        }
        if let Some(path) = pick {
            match self.link_file_from_path(sink, space, &path) {
                Ok(()) => self.files_notice = None,
                Err(e) => {
                    // A real, visible failure (permission/IO) — never a silent
                    // swallow, and never a faked link (§7). The picker stays open.
                    self.files_notice = Some(format!("Couldn't link {}: {e}", path.display()));
                }
            }
        }
    }

    /// The permanent-delete typed-confirm: the danger copy + a text field that must
    /// match the file's exact name to arm the (not-undoable) delete.
    fn permanent_delete_confirm_ui(
        &mut self,
        ui: &mut egui::Ui,
        sink: &mut CommandSink,
        space: SpaceId,
    ) {
        let Some(pending) = self.files_confirm_delete.as_mut() else {
            return;
        };
        let name = pending.name.clone();
        let display_name = safe_display_text(&name, FILE_DISPLAY_MAX_CHARS);
        let mut confirm = false;
        let mut cancel = false;
        mde_egui::card().show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::icon(ui, icons::FILE_DELETE_PERMANENT, Style::SP_M, Style::DANGER);
                ui.label(
                    egui::RichText::new("Delete permanently")
                        .strong()
                        .color(Style::DANGER),
                );
            });
            ui.label(
                egui::RichText::new(format!(
                    "Permanently deletes \u{201c}{display_name}\u{201d} for everyone. This removes the \
                         reference and lets the canonical file be purged once no space keeps it. \
                         This cannot be undone.",
                ))
                .color(Style::TEXT),
            );
            ui.label(
                egui::RichText::new(format!(
                    "Type the file name to confirm: {display_name}"
                ))
                    .small()
                    .color(Style::TEXT_DIM),
            );
            ui.add(
                egui::TextEdit::singleline(&mut pending.typed)
                    .desired_width(f32::INFINITY)
                    .hint_text("Exact file name"),
            );
            let armed = pending.typed.trim() == name;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(armed, egui::Button::new("Delete permanently"))
                    .clicked()
                {
                    confirm = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if confirm {
            self.confirm_permanent_delete(sink, space);
        } else if cancel {
            self.cancel_permanent_delete();
        }
    }

    // ── testable command seams (the UI above drives these same methods) ──────

    /// Read + hash `path` into a [`FileRef`] and emit
    /// [`LinkFile`](CollabCommand::LinkFile) for `space`, closing the picker on
    /// success. The one place a canonical file becomes a space reference.
    pub(crate) fn link_file_from_path(
        &mut self,
        sink: &mut CommandSink,
        space: SpaceId,
        path: &Path,
    ) -> std::io::Result<()> {
        let (file, reference) = file_ref_of_path(path)?;
        sink.emit(CollabCommand::LinkFile {
            space,
            file,
            reference,
        });
        self.file_picker = None;
        Ok(())
    }

    /// Emit [`UnlinkFile`](CollabCommand::UnlinkFile) — remove only *this space's
    /// reference* to `file`. The canonical file and other references are untouched.
    pub(crate) fn remove_reference(&self, sink: &mut CommandSink, space: SpaceId, file: FileRefId) {
        sink.emit(CollabCommand::UnlinkFile { space, file });
    }

    /// Emit [`StartTransfer`](CollabCommand::StartTransfer) to share `file` to the
    /// space's members over the default mesh transport (outbound from this seat).
    /// The byte-moving engine + progress are WL-FUNC-006's; this only mints the
    /// control handle the ledger mirror is then keyed by.
    pub(crate) fn start_transfer_to_members(
        &self,
        sink: &mut CommandSink,
        space: SpaceId,
        file: FileRefId,
    ) {
        sink.emit(CollabCommand::StartTransfer {
            space,
            transfer: TransferId::new(),
            file,
            method: TransferMethod::Node,
            direction: TransferDirection::Outbound,
        });
    }

    /// Emit [`ControlTransfer`](CollabCommand::ControlTransfer) — pause/resume/
    /// cancel a live transfer (every member may control a shared transfer).
    pub(crate) fn control_transfer(
        &self,
        sink: &mut CommandSink,
        transfer: TransferId,
        control: TransferControl,
    ) {
        sink.emit(CollabCommand::ControlTransfer { transfer, control });
    }

    /// Open the permanent-delete typed-confirm for `file` (armed only once `name`
    /// is typed verbatim).
    pub(crate) fn request_permanent_delete(&mut self, file: FileRefId, name: impl Into<String>) {
        self.files_confirm_delete = Some(PendingDelete {
            file,
            name: name.into(),
            typed: String::new(),
        });
    }

    /// Fire the pending permanent delete **iff** the typed name matches exactly.
    /// The collab-level primitive is [`UnlinkFile`](CollabCommand::UnlinkFile);
    /// the canonical payload is then purged by the core's purge gate once no space
    /// references it. Returns `true` when it fired.
    pub(crate) fn confirm_permanent_delete(
        &mut self,
        sink: &mut CommandSink,
        space: SpaceId,
    ) -> bool {
        let Some(pending) = self.files_confirm_delete.as_ref() else {
            return false;
        };
        if pending.typed.trim() != pending.name {
            return false;
        }
        let file = pending.file;
        sink.emit(CollabCommand::UnlinkFile { space, file });
        self.files_confirm_delete = None;
        true
    }

    /// Dismiss the pending permanent-delete confirm without deleting.
    pub(crate) fn cancel_permanent_delete(&mut self) {
        self.files_confirm_delete = None;
    }

    /// Whether the link picker is open (test/inspection accessor).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn file_picker_open(&self) -> bool {
        self.file_picker.is_some()
    }

    /// Set the picker's browse directory (used by tests to drive it deterministically
    /// at a known tempdir).
    #[cfg(test)]
    pub(crate) fn open_file_picker_at(&mut self, dir: PathBuf) {
        self.file_picker = Some(dir);
    }

    /// The pending permanent-delete's working buffer, set to `text` (test seam for
    /// the typed-confirm gate).
    #[cfg(test)]
    pub(crate) fn set_permanent_delete_typed(&mut self, text: impl Into<String>) {
        if let Some(pending) = self.files_confirm_delete.as_mut() {
            pending.typed = text.into();
        }
    }
}

/// Read `path`'s bytes and build a stable [`FileRef`] for it: the file name, its
/// exact byte size, and the SHA-256 content address (the platform content-address
/// convention). Mints a fresh [`FileRefId`] — the opaque, stable handle the space
/// keys the reference by.
///
/// Reads the bounded regular file to hash it; acceptable for an operator-picked
/// file, and a fleet-scale version would stream the hash off the paint thread.
pub fn file_ref_of_path(path: &Path) -> io::Result<(FileRefId, FileRef)> {
    let bytes = read_regular_file_bounded(path)?;
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let reference = FileRef {
        name,
        size: bytes.len() as u64,
        sha256_hex: mde_collab_types::value::sha256_hex(&bytes),
        mime: mime_hint(path),
    };
    Ok((FileRefId::new(), reference))
}

/// Read a local link candidate without following an untrusted final symlink or
/// materializing more than the file-reference ceiling. The pre-open metadata
/// check gives callers a useful error, while the descriptor check protects the
/// read from a path being replaced between inspection and open.
fn read_regular_file_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let inspected = fs::symlink_metadata(path)?;
    if inspected.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing symlinked file reference {}", path.display()),
        ));
    }
    if !inspected.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("file reference is not a regular file {}", path.display()),
        ));
    }
    reject_oversized_file_ref(path, inspected.len())?;

    let file = open_regular_file_no_follow(path)?;
    let opened = file.metadata()?;
    if !opened.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("file reference is not a regular file {}", path.display()),
        ));
    }
    reject_oversized_file_ref(path, opened.len())?;

    let mut bytes = Vec::with_capacity(opened.len() as usize);
    file.take(MAX_FILE_REF_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_REF_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "file reference {} exceeds {MAX_FILE_REF_BYTES}-byte limit",
                path.display()
            ),
        ));
    }
    Ok(bytes)
}

fn reject_oversized_file_ref(path: &Path, len: u64) -> io::Result<()> {
    if len > MAX_FILE_REF_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "file reference {} exceeds {MAX_FILE_REF_BYTES}-byte limit",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// Open the final leaf without following a symlink. The current desktop target
/// is Linux; its stable std extension accepts the kernel's O_NOFOLLOW flag
/// without requiring another direct dependency in this pure UI crate.
#[cfg(target_os = "linux")]
fn open_regular_file_no_follow(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    // Linux fcntl.h: O_NOFOLLOW == 00400000 (octal).
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(0o400000)
        .open(path)
}

/// On non-Linux targets, retain the explicit symlink rejection before opening;
/// Linux uses the race-resistant O_NOFOLLOW path above.
#[cfg(not(target_os = "linux"))]
fn open_regular_file_no_follow(path: &Path) -> io::Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing symlinked file reference {}", path.display()),
        ));
    }
    File::open(path)
}

/// A conservative content-type hint from the file extension — only the few
/// types the platform commonly moves, `None` otherwise (an honest
/// "unknown", never a guessed/faked MIME).
fn mime_hint(path: &Path) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)?;
    let ty = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        "json" => "application/json",
        "zip" => "application/zip",
        _ => return None,
    };
    Some(ty.to_owned())
}

/// The picker's initial directory: `$HOME`, falling back to `/`.
fn picker_start_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// A short display form of a 64-char lower-hex SHA-256 (`abcd1234…`). The content
/// address is the file's version identity; the row shows a recognisable prefix.
/// Shared with the Clipboard mode (a clip carries the same content-hash identity).
pub(crate) fn short_hash(sha256_hex: &str) -> String {
    let head: String = sha256_hex.chars().take(12).collect();
    if sha256_hex.len() > 12 {
        format!("{head}\u{2026}")
    } else {
        head
    }
}

/// The honest label for a transfer state. Shared with the Transfers mode.
pub(crate) const fn transfer_state_label(state: TransferState) -> &'static str {
    match state {
        TransferState::Queued => "Queued",
        TransferState::Active => "Transferring",
        TransferState::Paused => "Paused",
        TransferState::Completed => "Completed",
        TransferState::Failed => "Failed",
        TransferState::Canceled => "Canceled",
    }
}

/// The Carbon tint for a transfer state. Shared with the Transfers mode.
pub(crate) const fn transfer_state_color(state: TransferState) -> egui::Color32 {
    match state {
        TransferState::Queued | TransferState::Paused => Style::WARN,
        TransferState::Active => Style::ACCENT,
        TransferState::Completed => Style::OK,
        TransferState::Failed => Style::DANGER,
        TransferState::Canceled => Style::TEXT_DIM,
    }
}

/// A compact byte-size label (`512 B`, `2 KB`, `5.0 MB`, `3.0 GB`) — the surface's
/// own small formatter (the file-manager's `fmt_bytes` is `pub(crate)` to its own
/// crate), so a linked file's size + a transfer's mirrored progress read alike.
/// Shared with the Transfers + Clipboard modes.
pub(crate) fn fmt_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if n >= GB {
        format!("{:.1} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{} KB", n / KB)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::{file_ref_of_path, safe_display_text, MAX_FILE_REF_BYTES};

    #[test]
    fn safe_display_text_replaces_control_and_bidi_format_characters() {
        assert_eq!(
            safe_display_text("report\n\u{202e}txt", 64),
            "report\u{fffd}\u{fffd}txt"
        );
    }

    #[test]
    fn safe_display_text_truncates_on_unicode_character_boundaries() {
        let display = safe_display_text("é📦-a-very-long-file-name", 8);
        assert_eq!(display, "é📦-a-ve…");
        assert_eq!(display.chars().count(), 8);
    }

    #[test]
    fn safe_display_text_preserves_truthful_mime_values_when_bounded() {
        assert_eq!(safe_display_text("application/pdf", 64), "application/pdf");
    }

    #[test]
    fn file_ref_of_path_rejects_oversized_input_before_materialization() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("too-large.bin");
        let file = std::fs::File::create(&path).expect("create sparse file");
        file.set_len(MAX_FILE_REF_BYTES + 1)
            .expect("extend sparse file");

        let error = file_ref_of_path(&path).expect_err("oversized file reference");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("exceeds"));
    }

    #[cfg(unix)]
    #[test]
    fn file_ref_of_path_rejects_a_final_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("outside.txt");
        let link = dir.path().join("picked.txt");
        std::fs::write(&target, b"must not be linked").expect("write target");
        symlink(&target, &link).expect("create symlink");

        let error = file_ref_of_path(&link).expect_err("symlinked file reference");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("symlink"));
    }
}
