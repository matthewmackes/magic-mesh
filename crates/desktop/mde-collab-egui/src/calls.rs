//! Calls mode — the full **roster + controls** view of the persistent call bar's
//! [`CallState`](mde_collab_types::CallState) projection (WL-FUNC-011 "Calls and
//! media", the last of the six Communications modes).
//!
//! The persistent call bar (see [`frame`](crate::frame)) is the compact strip that
//! survives every mode/space switch; this mode is the same read model rendered in
//! full — every active/ringing call, each call's participants and their state, the
//! per-space-vs-direct context, and the whole control set:
//!
//! * **Start** — audio / video / screen-share, each emitting
//!   [`StartCall`](CollabCommand::StartCall) with the matching
//!   [`CallKind`](mde_collab_types::CallKind) for the selected space (a Direct
//!   space's call reads as a direct call, a Team/Incident/Project space's as a
//!   space call).
//! * **Answer / Decline** a ringing invitation →
//!   [`AnswerCall`](CollabCommand::AnswerCall) /
//!   [`DeclineCall`](CollabCommand::DeclineCall).
//! * **Mute** the local microphone → [`SetCallMuted`](CollabCommand::SetCallMuted)
//!   (a real convergent command; the projection carries each participant's muted
//!   bit).
//! * **DTMF** — an in-call keypad whose every press emits
//!   [`SendDtmf`](CollabCommand::SendDtmf).
//! * **Hang up** → [`HangUpCall`](CollabCommand::HangUpCall).
//! * Device selection (mic / camera / screen), reusing the egui combo shape the
//!   `mde-voice-egui` dialer controls take.
//!
//! # Media transport is an explicit, marked follow-up — never faked (spec §7)
//!
//! Every control above emits a **real** typed command and the call **state** flows
//! through the worker's event log into the projection today. What lands later is
//! the media plane that actually carries the audio/video frames, marked in-code
//! with `// WL-FUNC-011 media:`:
//!
//! * **WebRTC P2P** for direct (two-party) calls;
//! * an **elected LiveKit SFU** for group calls + P2P failover;
//! * the existing **SIP** account / DID / G.711 path (from `mde-voice-config` /
//!   `mde-voice-hud`, reused unchanged) behind a **LiveKit SIP gateway** so a mesh
//!   call can bridge to the PSTN.
//!
//! The camera / screen-source toggles record the seat's outgoing-media intent as
//! local view state; binding a real device to a live sender is part of that same
//! follow-up. There is deliberately **no recording and no transcription** anywhere
//! — not in this UI, not in the commands, not in the worker or its state.

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{
    ActorId, CallId, CallKind, CallParticipantState, CallParticipantView, CallView, CollabCommand,
    SpaceDirectory, SpaceId, SpaceKind,
};

use crate::frame::call_kind_label;
use crate::{icons, relative_age, CommandSink, CommunicationsSurface};

/// The honest label for the one media device offered today. Live device
/// enumeration is a marked media-plane follow-up (never a faked device list).
const DEFAULT_DEVICE: &str = "System default";

/// The in-call DTMF keypad layout (telephone order), driving
/// [`SendDtmf`](CollabCommand::SendDtmf).
const DTMF_ROWS: [[char; 3]; 4] = [
    ['1', '2', '3'],
    ['4', '5', '6'],
    ['7', '8', '9'],
    ['*', '0', '#'],
];

/// The local seat's Calls-mode media preferences — the selected mic / camera /
/// screen device and the seat's outgoing camera / screen-share intents.
///
/// **Seat-level view state only.** This pure UI crate never touches a real capture
/// device: the actual device enumeration and the act of binding a device to the
/// live media sender (WebRTC / LiveKit) are the marked media-plane follow-up. The
/// *audio* mute is not here — that is a real convergent
/// [`SetCallMuted`](CollabCommand::SetCallMuted) command reflected in the
/// projection, not a local preference.
#[derive(Debug, Clone)]
pub(crate) struct CallMediaPrefs {
    /// The chosen microphone device (default: the system default).
    pub(crate) mic: String,
    /// The chosen camera device (default: the system default).
    pub(crate) camera: String,
    /// The chosen screen-capture source (default: the system default).
    pub(crate) screen: String,
    /// Whether the seat intends to send camera video. The outgoing video track is
    /// the media-plane follow-up; this records the intent honestly today.
    pub(crate) camera_on: bool,
    /// Whether the seat intends to share its screen. The outgoing screen track is
    /// the media-plane follow-up; this records the intent honestly today.
    pub(crate) screen_sharing: bool,
}

impl Default for CallMediaPrefs {
    fn default() -> Self {
        Self {
            mic: DEFAULT_DEVICE.to_owned(),
            camera: DEFAULT_DEVICE.to_owned(),
            screen: DEFAULT_DEVICE.to_owned(),
            camera_on: false,
            screen_sharing: false,
        }
    }
}

impl CommunicationsSurface {
    /// Render Calls mode for the selected space: the start cluster (audio / video /
    /// screen-share), the media device row, and the roster of active calls with
    /// their participants and per-call controls.
    pub(crate) fn calls_body(
        &mut self,
        ui: &mut egui::Ui,
        data: &dyn crate::CollabData,
        sink: &mut CommandSink,
    ) {
        let Some(space) = self.selected_space() else {
            ui.label(
                egui::RichText::new("Select a space to place or join a call.")
                    .color(Style::TEXT_DIM),
            );
            return;
        };

        // Everything the body needs, read up front so no `data` borrow is held
        // across the `&mut self` render calls below.
        let me = data.me().clone();
        let now = data.now_unix_ms();
        let calls = data.call_state().active.clone();
        let directory = data.space_directory().clone();
        let (space_name, direct) = space_context(&directory, space);

        // Header — the mode title + the start cluster for the selected space.
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Calls")
                    .strong()
                    .color(Style::TEXT_STRONG),
            );
            ui.label(
                egui::RichText::new(if direct {
                    format!("· {space_name} (direct)")
                } else {
                    format!("· {space_name}")
                })
                .small()
                .color(Style::TEXT_DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.call_start_cluster(ui, sink, space);
            });
        });
        ui.separator();

        // The media device row (mic / camera / screen).
        self.call_device_row(ui);
        ui.separator();

        // The roster of active calls.
        if calls.is_empty() {
            ui.label(egui::RichText::new("No active calls.").color(Style::TEXT_DIM));
            ui.label(
                egui::RichText::new(
                    "Start an audio, video, or screen-share call above — it appears here and in \
                     the call bar for everyone in the space.",
                )
                .small()
                .color(Style::TEXT_DIM),
            );
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("collab-calls")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for call in &calls {
                    self.call_card(ui, sink, &me, &directory, now, call);
                    ui.add_space(Style::SP_XS);
                }
            });
    }

    /// The start cluster (screen-share · video · audio), laid out right-to-left so
    /// audio reads first. Each button emits [`StartCall`](CollabCommand::StartCall)
    /// with its [`CallKind`] for the selected space.
    fn call_start_cluster(&self, ui: &mut egui::Ui, sink: &mut CommandSink, space: SpaceId) {
        if icons::icon_button(
            ui,
            icons::CALL_SCREEN,
            Style::SP_M,
            Style::TEXT_DIM,
            "Start a screen share",
        )
        .clicked()
        {
            self.start_call(sink, space, CallKind::Screen);
        }
        if icons::icon_button(
            ui,
            icons::CALL_VIDEO,
            Style::SP_M,
            Style::ACCENT,
            "Start a video call",
        )
        .clicked()
        {
            self.start_call(sink, space, CallKind::Video);
        }
        if icons::icon_button(
            ui,
            icons::CALL_AUDIO,
            Style::SP_M,
            Style::OK,
            "Start an audio call",
        )
        .clicked()
        {
            self.start_call(sink, space, CallKind::Audio);
        }
    }

    /// The media device row: a labeled mic / camera / screen combo apiece, reusing
    /// the egui combo shape the voice dialer controls take. The selection is local
    /// seat state; the live device list + binding is a marked media-plane follow-up.
    fn call_device_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("Devices")
                    .small()
                    .strong()
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_S);
            device_combo(
                ui,
                "Microphone",
                icons::CALL_AUDIO,
                &mut self.call_media.mic,
            );
            device_combo(
                ui,
                "Camera",
                icons::CALL_CAMERA,
                &mut self.call_media.camera,
            );
            device_combo(
                ui,
                "Screen",
                icons::CALL_SHARE_SCREEN,
                &mut self.call_media.screen,
            );
        });
        ui.label(
            egui::RichText::new(
                "Showing the system default — live device enumeration and binding a device to \
                 the call's media sender arrive with the media plane.",
            )
            .small()
            .color(Style::TEXT_DIM),
        );
    }

    /// One active-call card: the kind + space context + age header, the full
    /// participant roster, and this seat's controls (answer / decline while
    /// ringing; the connected cluster + DTMF keypad while connected).
    fn call_card(
        &mut self,
        ui: &mut egui::Ui,
        sink: &mut CommandSink,
        me: &ActorId,
        directory: &SpaceDirectory,
        now_unix_ms: i64,
        call: &CallView,
    ) {
        let mine = call.participants.iter().find(|p| &p.actor == me).cloned();
        let connected = call
            .participants
            .iter()
            .filter(|p| p.state == CallParticipantState::Connected)
            .count();

        egui::Frame::NONE
            .fill(Style::LAYER_01)
            .inner_margin(Style::SP_S)
            .show(ui, |ui| {
                call_card_header(ui, directory, now_unix_ms, call, connected);
                for p in &call.participants {
                    call_roster_row(ui, me, p);
                }
                match mine.as_ref().map(|p| p.state) {
                    Some(CallParticipantState::Ringing) => {
                        self.ringing_controls(ui, sink, call.call);
                    }
                    Some(CallParticipantState::Connected) => {
                        let muted = mine.as_ref().is_some_and(|p| p.muted);
                        self.connected_controls(ui, sink, call.call, muted);
                        if self.dtmf_pad == Some(call.call) {
                            self.dtmf_keypad(ui, sink, call.call);
                        }
                    }
                    // Declined/Left this call, or only watching it (not a
                    // participant): the roster is shown read-only — no faked
                    // "rejoin" control, because there is no such command today.
                    _ => {}
                }
            });
    }

    /// The ringing-invitation controls: Answer / Decline.
    fn ringing_controls(&self, ui: &mut egui::Ui, sink: &mut CommandSink, call: CallId) {
        ui.horizontal(|ui| {
            if icons::icon_button(ui, icons::CALL_ANSWER, Style::SP_M, Style::OK, "Answer")
                .clicked()
            {
                self.answer_call(sink, call);
            }
            if icons::icon_button(
                ui,
                icons::CALL_DECLINE,
                Style::SP_M,
                Style::DANGER,
                "Decline",
            )
            .clicked()
            {
                self.decline_call(sink, call);
            }
        });
    }

    /// The connected-seat control cluster: mute, camera, screen-share, the DTMF
    /// keypad toggle, and hang up. Mute + DTMF emit real commands; the camera /
    /// screen toggles record the seat's outgoing-media intent (a marked media-plane
    /// follow-up).
    fn connected_controls(
        &mut self,
        ui: &mut egui::Ui,
        sink: &mut CommandSink,
        call: CallId,
        muted: bool,
    ) {
        ui.horizontal(|ui| {
            // Microphone mute — a real convergent command.
            let (mic_glyph, mic_hint) = if muted {
                (icons::CALL_UNMUTE, "Unmute microphone")
            } else {
                (icons::CALL_MUTE, "Mute microphone")
            };
            if icons::icon_button(ui, mic_glyph, Style::SP_M, Style::TEXT_DIM, mic_hint).clicked() {
                self.set_call_muted(sink, call, !muted);
            }

            // WL-FUNC-011 media: the camera toggle records the seat's outgoing-video
            // intent; capturing the camera + attaching the WebRTC/LiveKit video track
            // is the media-plane follow-up (no convergent command carries a video bit
            // today, so this stays honest local state, never a faked live stream).
            if media_toggle(ui, icons::CALL_CAMERA, self.call_media.camera_on, "camera") {
                self.call_media.camera_on = !self.call_media.camera_on;
            }

            // WL-FUNC-011 media: the screen-share toggle records the seat's intent;
            // the actual screen capture + outgoing track is the media-plane follow-up.
            if media_toggle(
                ui,
                icons::CALL_SHARE_SCREEN,
                self.call_media.screen_sharing,
                "screen share",
            ) {
                self.call_media.screen_sharing = !self.call_media.screen_sharing;
            }

            // The DTMF keypad toggle — a real per-press command once open.
            let dtmf_open = self.dtmf_pad == Some(call);
            let dtmf_tint = if dtmf_open {
                Style::ACCENT
            } else {
                Style::TEXT_DIM
            };
            if icons::icon_button(ui, icons::CALL_DTMF, Style::SP_M, dtmf_tint, "DTMF keypad")
                .clicked()
            {
                self.dtmf_pad = if dtmf_open { None } else { Some(call) };
            }

            // Hang up — leaves the call (ends it when no one else remains).
            if icons::icon_button(
                ui,
                icons::CALL_HANGUP,
                Style::SP_M,
                Style::DANGER,
                "Hang up",
            )
            .clicked()
            {
                self.hang_up_call(sink, call);
            }
        });
    }

    /// The in-call DTMF keypad: a telephone-order 3×4 grid whose every press emits
    /// a [`SendDtmf`](CollabCommand::SendDtmf) for `call`.
    fn dtmf_keypad(&self, ui: &mut egui::Ui, sink: &mut CommandSink, call: CallId) {
        ui.add_space(Style::SP_XS);
        ui.label(
            egui::RichText::new("DTMF")
                .small()
                .strong()
                .color(Style::TEXT_DIM),
        );
        for row in DTMF_ROWS {
            ui.horizontal(|ui| {
                for digit in row {
                    let button = egui::Button::new(
                        egui::RichText::new(digit.to_string())
                            .monospace()
                            .color(Style::TEXT),
                    )
                    .min_size(egui::vec2(Style::SP_XL, Style::SP_XL));
                    if ui.add(button).clicked() {
                        self.send_dtmf(sink, call, digit);
                    }
                }
            });
        }
    }

    // ── testable command seams (the UI above drives these same methods) ──────

    /// Emit [`StartCall`](CollabCommand::StartCall) for `space` with `kind` — mints
    /// a fresh [`CallId`] (the control handle the worker's `CallStarted` event and
    /// the [`CallState`](mde_collab_types::CallState) projection are keyed by).
    pub(crate) fn start_call(&self, sink: &mut CommandSink, space: SpaceId, kind: CallKind) {
        sink.emit(CollabCommand::StartCall {
            space,
            call: CallId::new(),
            kind,
        });
    }

    /// Emit [`AnswerCall`](CollabCommand::AnswerCall) — accept a ringing invitation.
    pub(crate) fn answer_call(&self, sink: &mut CommandSink, call: CallId) {
        sink.emit(CollabCommand::AnswerCall { call });
    }

    /// Emit [`DeclineCall`](CollabCommand::DeclineCall) — decline a ringing call.
    pub(crate) fn decline_call(&self, sink: &mut CommandSink, call: CallId) {
        sink.emit(CollabCommand::DeclineCall { call });
    }

    /// Emit [`HangUpCall`](CollabCommand::HangUpCall) — leave the call (the worker
    /// ends it when no other participant remains connected).
    pub(crate) fn hang_up_call(&self, sink: &mut CommandSink, call: CallId) {
        sink.emit(CollabCommand::HangUpCall { call });
    }

    /// Emit [`SetCallMuted`](CollabCommand::SetCallMuted) — toggle the local mic.
    pub(crate) fn set_call_muted(&self, sink: &mut CommandSink, call: CallId, muted: bool) {
        sink.emit(CollabCommand::SetCallMuted { call, muted });
    }

    /// Emit [`SendDtmf`](CollabCommand::SendDtmf) — one in-call keypad tone.
    pub(crate) fn send_dtmf(&self, sink: &mut CommandSink, call: CallId, digit: char) {
        sink.emit(CollabCommand::SendDtmf { call, digit });
    }

    /// Open the in-call DTMF keypad for `call` (test seam for the keypad gate).
    #[cfg(test)]
    pub(crate) fn open_dtmf_pad(&mut self, call: CallId) {
        self.dtmf_pad = Some(call);
    }

    /// The call whose DTMF keypad is open, if any (test/inspection accessor).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn dtmf_pad_target(&self) -> Option<CallId> {
        self.dtmf_pad
    }
}

/// A labeled media device combo — the glyph, then an egui combo of the offered
/// devices (today only the honest system default). The picked value is written
/// back into `value`.
fn device_combo(ui: &mut egui::Ui, label: &str, glyph: &str, value: &mut String) {
    icons::icon(ui, glyph, Style::SP_M, Style::TEXT_DIM).on_hover_text(label);
    egui::ComboBox::from_id_salt(("mde-collab-call-device", label))
        .selected_text(value.clone())
        .show_ui(ui, |ui| {
            // WL-FUNC-011 media: only the honest system default is offered today; the
            // real enumerated device list comes from the media plane (WebRTC
            // getUserMedia / the LiveKit device registry), never a faked list.
            ui.selectable_value(value, DEFAULT_DEVICE.to_owned(), DEFAULT_DEVICE);
        });
    ui.add_space(Style::SP_S);
}

/// An outgoing-media intent toggle (camera / screen-share). Records the seat's
/// intent as local view state; the actual capture + track is the marked media-plane
/// follow-up. Returns `true` when clicked (the caller flips the stored intent).
fn media_toggle(ui: &mut egui::Ui, glyph: &str, on: bool, what: &str) -> bool {
    let tint = if on { Style::ACCENT } else { Style::TEXT_DIM };
    let hint = if on {
        format!("Turn {what} off (media plane pending)")
    } else {
        format!("Turn {what} on (media plane pending)")
    };
    icons::icon_button(ui, glyph, Style::SP_M, tint, &hint).clicked()
}

/// The call card's header row: the kind glyph + label, the per-space (or direct)
/// context, the call's age, and the connected count.
fn call_card_header(
    ui: &mut egui::Ui,
    directory: &SpaceDirectory,
    now_unix_ms: i64,
    call: &CallView,
    connected: usize,
) {
    let (space_name, direct) = space_context(directory, call.space);
    ui.horizontal(|ui| {
        icons::icon(ui, call_kind_icon(call.kind), Style::SP_M, Style::ACCENT);
        ui.label(
            egui::RichText::new(call_kind_label(call.kind))
                .strong()
                .color(Style::TEXT_STRONG),
        );
        ui.label(
            egui::RichText::new(if direct {
                format!("· direct · {space_name}")
            } else {
                format!("· {space_name}")
            })
            .small()
            .color(Style::TEXT_DIM),
        );
        ui.label(
            egui::RichText::new(relative_age(now_unix_ms, call.started_unix_ms))
                .small()
                .color(Style::TEXT_DIM),
        );
        ui.label(
            egui::RichText::new(format!("· {connected} connected"))
                .small()
                .color(Style::TEXT_DIM),
        );
    });
}

/// One participant roster row: the state glyph + name (marking the local seat) +
/// state label, plus a muted indicator when the projection says so.
fn call_roster_row(ui: &mut egui::Ui, me: &ActorId, p: &CallParticipantView) {
    let (glyph, label, color) = participant_view(p.state);
    ui.horizontal(|ui| {
        icons::icon(ui, glyph, Style::SP_M, color);
        let mut name = p.actor.as_str().to_owned();
        if &p.actor == me {
            name.push_str(" (you)");
        }
        ui.label(egui::RichText::new(name).color(Style::TEXT));
        ui.label(egui::RichText::new(label).small().color(color));
        if p.muted {
            icons::icon(ui, icons::CALL_MUTE, Style::SP_M, Style::TEXT_DIM).on_hover_text("Muted");
        }
    });
}

/// The name + direct-ness of the space a call lives in, for the roster's per-space
/// vs direct context label. A [`Direct`](SpaceKind::Direct) space's call reads as a
/// direct call; anything else as a space call. An unknown space (not in the seat's
/// directory) falls back to a short id handle, honestly.
fn space_context(directory: &SpaceDirectory, space: SpaceId) -> (String, bool) {
    directory.spaces.iter().find(|s| s.id == space).map_or_else(
        || {
            let id = space.to_string();
            let head: String = id.chars().take(8).collect();
            (format!("space {head}\u{2026}"), false)
        },
        |s| (s.name.clone(), s.kind == SpaceKind::Direct),
    )
}

/// The Carbon glyph for a call's [`CallKind`] (the roster header + call-bar glyph).
const fn call_kind_icon(kind: CallKind) -> &'static str {
    match kind {
        CallKind::Audio => icons::CALL_AUDIO,
        CallKind::Video => icons::CALL_VIDEO,
        CallKind::Screen => icons::CALL_SCREEN,
        CallKind::CoEdit => "document-edit",
        CallKind::RemoteDesktop => "system-lock-screen",
    }
}

/// The glyph, label, and tint for a participant's
/// [`CallParticipantState`](mde_collab_types::CallParticipantState) roster row.
const fn participant_view(
    state: CallParticipantState,
) -> (&'static str, &'static str, egui::Color32) {
    match state {
        CallParticipantState::Ringing => (icons::CALL_PARTICIPANT_RINGING, "ringing", Style::WARN),
        CallParticipantState::Connected => {
            (icons::CALL_PARTICIPANT_CONNECTED, "connected", Style::OK)
        }
        CallParticipantState::Declined => (
            icons::CALL_PARTICIPANT_DECLINED,
            "declined",
            Style::TEXT_DIM,
        ),
        CallParticipantState::Left => (icons::CALL_PARTICIPANT_LEFT, "left", Style::TEXT_DIM),
    }
}
