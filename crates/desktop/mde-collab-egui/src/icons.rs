//! The Communications surface's **Mackes-Carbon** icon standard.
//!
//! Carbon is the canonical platform icon set (see [`mde_egui::carbon`]); this
//! surface paints *every* glyph through the shared loader — no glyph text, no
//! hand-stroked vectors. This module is the single place each surface concept
//! (a space kind, a mode tab, a delivery state, a call control) is mapped to a
//! registered Carbon glyph name, so a test can iterate [`ALL_COLLAB_ICONS`] and
//! assert the whole set is embedded + rasterizes (mirroring the browser chrome's
//! `every_chrome_icon_maps_to_a_registered_carbon_glyph`).

use mde_egui::egui;

use mde_collab_types::{ClipItemKind, DeliveryState, Severity, SpaceKind};

use crate::{ActivityFilter, Mode};

/// The Carbon glyph for a space kind's rail row.
#[must_use]
pub const fn space_kind_icon(kind: SpaceKind) -> &'static str {
    match kind {
        SpaceKind::Direct => "share",
        SpaceKind::Team => "view-grid",
        SpaceKind::Incident => "dialog-warning",
        SpaceKind::Project => "text-x-generic",
    }
}

/// The Carbon glyph for a mode tab.
#[must_use]
pub const fn mode_icon(mode: Mode) -> &'static str {
    match mode {
        Mode::Activity => "view",
        Mode::Messages => "share",
        Mode::Calls => "audio-volume-high",
        Mode::Files => "download",
        Mode::Transfers => "view-refresh",
        Mode::Documents => "document-edit",
        Mode::Alerts => "notification",
        Mode::Clipboard => "text-x-generic",
    }
}

/// The Carbon glyph for an alert's severity band (the leading row glyph). Info
/// rings quietly (a plain bell); Warning + Critical carry the warning glyph and
/// are told apart by tint at the call site.
#[must_use]
pub const fn severity_icon(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "notification",
        Severity::Warning | Severity::Critical => "dialog-warning",
    }
}

/// The Carbon glyph for a clipboard item's kind (text vs. a shared URI/link).
#[must_use]
pub const fn clip_kind_icon(kind: ClipItemKind) -> &'static str {
    match kind {
        ClipItemKind::Text => "text-x-generic",
        ClipItemKind::Uri => "globe",
    }
}

/// The Carbon glyph for a message's honest delivery state.
#[must_use]
pub const fn delivery_icon(delivery: DeliveryState) -> &'static str {
    match delivery {
        DeliveryState::Sent => "share",
        DeliveryState::Delivered => "emblem-ok",
        DeliveryState::Queued => "document-open-recent",
    }
}

/// The Carbon glyph for an Activity filter chip. `None` keeps a filter chip a
/// text-only control (the surface does not force a mismatched glyph on a filter
/// band that has no faithful Carbon match, e.g. "People").
#[must_use]
pub const fn activity_filter_icon(filter: ActivityFilter) -> Option<&'static str> {
    match filter {
        ActivityFilter::All => Some("view-grid"),
        ActivityFilter::Messages => Some("share"),
        ActivityFilter::Alerts => Some("notification"),
        ActivityFilter::Calls => Some("audio-volume-high"),
        ActivityFilter::Files => Some("download"),
        ActivityFilter::People => None,
    }
}

/// Send a composed message.
pub const SEND: &str = "share";
/// Edit one's own message.
pub const EDIT: &str = "document-edit";
/// Delete one's own message.
pub const DELETE: &str = "list-remove";
/// Open / anchor a reply thread.
pub const THREAD: &str = "go-next";

/// Start a call in the selected space.
pub const CALL_START: &str = "media-playback-start";
/// Hang up / leave an active call.
pub const CALL_HANGUP: &str = "process-stop";
/// Answer a ringing call.
pub const CALL_ANSWER: &str = "emblem-ok";
/// Decline a ringing call.
pub const CALL_DECLINE: &str = "window-close";
/// Mute the local microphone in a call.
pub const CALL_MUTE: &str = "audio-volume-muted";
/// Unmute the local microphone in a call.
pub const CALL_UNMUTE: &str = "audio-volume-high";

// ---- Calls mode (WL-FUNC-011) --------------------------------------------
/// The Calls-mode roster leading glyph + the "start audio call" affordance.
pub const CALL_AUDIO: &str = "audio-volume-high";
/// Start a **video** call (audio + camera) in the selected space.
pub const CALL_VIDEO: &str = "camera-photo";
/// Start a **screen-share** session in the selected space.
pub const CALL_SCREEN: &str = "system-lock-screen";
/// Toggle the local **camera** in a call (the outgoing video track is a marked
/// media-plane follow-up; the toggle records the seat's intent).
pub const CALL_CAMERA: &str = "camera-photo";
/// Toggle local **screen sharing** in a call (the outgoing screen track is a
/// marked media-plane follow-up; the toggle records the seat's intent).
pub const CALL_SHARE_SCREEN: &str = "system-lock-screen";
/// Open the in-call **DTMF keypad** (each press emits a `SendDtmf` command).
pub const CALL_DTMF: &str = "view-grid";
/// A **ringing** call participant (invited, not yet connected).
pub const CALL_PARTICIPANT_RINGING: &str = "notification";
/// A **connected** call participant.
pub const CALL_PARTICIPANT_CONNECTED: &str = "emblem-ok";
/// A participant who **declined** the call.
pub const CALL_PARTICIPANT_DECLINED: &str = "window-close";
/// A participant who **left** the call.
pub const CALL_PARTICIPANT_LEFT: &str = "process-stop";

// ---- Files mode ----------------------------------------------------------
/// Link a canonical file into the space (open the picker / add a reference).
pub const FILE_LINK: &str = "list-add";
/// Remove a file **reference** from the space (unlink — the canonical file is
/// untouched; distinct from a permanent delete).
pub const FILE_UNLINK: &str = "list-remove";
/// Permanently delete the canonical file (danger, typed-confirm gated).
pub const FILE_DELETE_PERMANENT: &str = "dialog-warning";
/// A linked-file row's leading glyph.
pub const FILE_ROW: &str = "text-x-generic";
/// Start sharing a linked file to members (start a transfer through the ledger).
pub const TRANSFER_SEND: &str = "download";
/// Pause a running transfer.
pub const TRANSFER_PAUSE: &str = "media-playback-pause";
/// Resume a paused transfer.
pub const TRANSFER_RESUME: &str = "media-playback-start";
/// Cancel a transfer.
pub const TRANSFER_CANCEL: &str = "media-playback-stop";
/// A folder row in the link picker.
pub const PICKER_FOLDER: &str = "view-grid";
/// Ascend to the parent directory in the link picker.
pub const PICKER_UP: &str = "go-up";

// ---- Alerts mode ---------------------------------------------------------
/// Acknowledge an alert (mark handled for the seat).
pub const ALERT_ACK: &str = "emblem-ok";
/// Snooze an alert until a time.
pub const ALERT_SNOOZE: &str = "document-open-recent";
/// Run a (safe) typed inline alert action.
pub const ALERT_RUN: &str = "go-next";
/// A destructive typed inline alert action (danger, arm-then-confirm gated).
pub const ALERT_DESTRUCTIVE: &str = "dialog-warning";
/// Mute an alert source for the local seat (block it from ringing).
pub const ALERT_MUTE: &str = "changes-prevent";
/// Toggle fleet Do-Not-Disturb (only Critical breaks through).
pub const ALERT_DND: &str = "weather-clear-night";

// ---- Clipboard mode ------------------------------------------------------
/// Pin a clipboard item (survives the cap + clear).
pub const CLIP_PIN: &str = "star";
/// Attach / re-share a clipboard item into the space.
pub const CLIP_ATTACH: &str = "share";
/// Delete a single clipboard item.
pub const CLIP_DELETE: &str = "list-remove";
/// Publish a new clipboard item into the lane.
pub const CLIP_PUBLISH: &str = "list-add";

// ---- Documents mode (WL-FUNC-011) ----------------------------------------
/// The Document sub-mode tab (the one-pane Markdown editor).
pub const DOC_SUBMODE_DOCUMENT: &str = "document-edit";
/// The Project sub-mode tab (the embedded full "Construct" IDE editor).
pub const DOC_SUBMODE_PROJECT: &str = "view-grid";
/// The Source view (edit the raw Markdown rope).
pub const DOC_VIEW_SOURCE: &str = "text-x-generic";
/// The Visual view (the rendered Markdown, via the editor's own render).
pub const DOC_VIEW_VISUAL: &str = "view";
/// Start a new document (opens the ops-oriented template picker).
pub const DOC_NEW: &str = "list-add";
/// Save the document — emits `UpdateDocument` with the canonical Markdown.
pub const DOC_SAVE: &str = "emblem-ok";
/// Export the canonical Markdown (the only export; print/preview live in the
/// embedded editor's File menu, off this default toolbar).
pub const DOC_EXPORT: &str = "go-up";
/// A document row in the session picker.
pub const DOC_ROW: &str = "document-edit";

// ---- Transfers mode ------------------------------------------------------
/// A transfer row's leading glyph.
pub const XFER_ROW: &str = "download";
/// Bytes arriving at this seat (inbound direction).
pub const XFER_INBOUND: &str = "go-down";
/// Bytes leaving this seat (outbound direction).
pub const XFER_OUTBOUND: &str = "go-up";

/// Every Carbon glyph name this surface can paint — the icon-standard set a test
/// asserts is registered in the shared loader and rasterizes to a non-blank
/// tinted mask. Keep this in sync with the mappings above.
pub const ALL_COLLAB_ICONS: &[&str] = &[
    // space kinds
    "share",
    "view-grid",
    "dialog-warning",
    "text-x-generic",
    // mode tabs
    "view",
    "download",
    "document-edit",
    "notification",
    // delivery states
    "emblem-ok",
    "document-open-recent",
    // message + thread actions
    "list-remove",
    "go-next",
    // call bar + Calls mode (start audio/video/screen, roster states, DTMF keypad)
    "media-playback-start",
    "process-stop",
    "window-close",
    "audio-volume-muted",
    "audio-volume-high",
    "camera-photo",
    "system-lock-screen",
    // Files mode (link / unlink / permanent-delete + transfer controls + picker)
    "list-add",
    "media-playback-pause",
    "media-playback-stop",
    "go-up",
    // Transfers/Alerts/Clipboard modes (WL-FUNC-011)
    "view-refresh",
    "go-down",
    "changes-prevent",
    "weather-clear-night",
    "globe",
    "star",
];

/// Paint a Carbon glyph `name` at `size` logical points tinted `color`, sensing
/// hover, and return its [`Response`](egui::Response). The one icon entry point
/// the surface draws through, so every glyph is a real rasterized Carbon image
/// mesh (never glyph text). An unknown/failed glyph still allocates its space so
/// layout stays stable.
pub fn icon(ui: &mut egui::Ui, name: &str, size: f32, color: egui::Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let _ = mde_egui::carbon::paint_carbon(ui.painter(), rect, name, color);
    }
    response
}

/// A clickable Carbon icon button: paints glyph `name` at `size`, tinted `color`
/// (brightened on hover), senses click, and shows `hint` on hover. Returns the
/// [`Response`](egui::Response) so the caller reads `.clicked()`. The one command
/// affordance the call bar + message actions draw through, so every control is a
/// real Carbon image mesh.
pub fn icon_button(
    ui: &mut egui::Ui,
    name: &str,
    size: f32,
    color: egui::Color32,
    hint: &str,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let tint = if response.hovered() {
            mde_egui::Style::TEXT_STRONG
        } else {
            color
        };
        let _ = mde_egui::carbon::paint_carbon(ui.painter(), rect, name, tint);
    }
    response.on_hover_text(hint)
}
