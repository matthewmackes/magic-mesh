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
//!   (a real convergent command applied to the live P2P audio leg by the
//!   mackesd media worker when a seat device is bound; the projection carries
//!   each participant's muted bit).
//! * **DTMF** — an in-call keypad whose every press emits
//!   [`SendDtmf`](CollabCommand::SendDtmf), which the same worker injects into
//!   the bound live leg.
//! * **Hang up** → [`HangUpCall`](CollabCommand::HangUpCall).
//! * Device selection (mic / camera / screen), reusing the egui combo shape the
//!   `mde-voice-egui` dialer controls take.
//!
//! # Media plane: mute/DTMF are live; camera/screen remain marked follow-ups
//!
//! Mute and DTMF are the live-leg verbs. The renderer still only emits typed
//! [`CollabCommand`]s; the mackesd P2P worker (`call_media`) applies them to the
//! bound seat audio (or publishes a typed unavailable media session when no
//! device or permission exists). What remains marked in-code with
//! `// WL-FUNC-011 media:` is capture that this surface does not yet bind:
//!
//! * **camera / screen-share** track attach (S5);
//! * **elected LiveKit SFU** for group calls (S3);
//! * **LiveKit SIP gateway** PSTN legs (S4).
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
use crate::icons::CommsHoverExt;
use crate::{icons, relative_age, CommandSink, CommunicationsSurface};

/// The honest label for the one media device offered today. Live device
/// enumeration is a marked media-plane follow-up (never a faked device list).
const DEFAULT_DEVICE: &str = "System default";

/// Keep data-backed labels readable inside the fixed Calls cards and rows. The
/// projection may contain values supplied by another seat, so these limits are
/// a rendering boundary rather than validation of the underlying data.
const MAX_CALL_LABEL_CHARS: usize = 48;
const MAX_CALL_STATUS_CHARS: usize = 24;

/// Normalize and bound text before it enters a Calls widget.
///
/// egui will happily measure an arbitrarily long single-line label, which can
/// push the roster and device controls off the available width. Control and
/// bidi-isolation characters are also not useful in a seat label and can make
/// the rendered value misleading. Preserve the source value in the projection;
/// this helper only creates a safe, single-line display copy.
fn bounded_display_text(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let mut normalized = String::with_capacity(value.len().min(max_chars));
    let mut pending_space = false;
    for character in value.chars() {
        if is_calls_invisible(character) || character.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !normalized.is_empty() {
            normalized.push(' ');
        }
        pending_space = false;
        normalized.push(character);
    }

    if normalized.is_empty() {
        return "—".to_owned();
    }

    if normalized.chars().count() <= max_chars {
        return normalized;
    }

    let keep_chars = max_chars.saturating_sub(1);
    let mut bounded: String = normalized.chars().take(keep_chars).collect();
    while bounded.ends_with(' ') {
        bounded.pop();
    }
    bounded.push('…');
    bounded
}

/// Characters that can alter the visual structure or direction of a Calls
/// label. Newlines/tabs are handled as spaces so adjacent words remain legible.
fn is_calls_invisible(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{200B}'
                | '\u{200C}'
                | '\u{2060}'
                | '\u{2066}'..='\u{2069}'
                | '\u{202A}'..='\u{202E}'
                | '\u{FEFF}'
        )
}

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
/// device: live microphone bind and mute/DTMF injection are owned by the mackesd
/// P2P media worker, driven by [`SetCallMuted`](CollabCommand::SetCallMuted) and
/// [`SendDtmf`](CollabCommand::SendDtmf). Camera and screen enumeration plus
/// attaching those tracks remain marked media-plane follow-ups.
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
        // A membership update can remove the selected space between frames. Do
        // not turn that stale view id into a new call target; the frame's call
        // bar applies the same directory boundary.
        let directory = data.space_directory().clone();
        let Some(space) =
            crate::frame::selected_space_in_directory(self.selected_space(), &directory)
        else {
            ui.label(
                egui::RichText::new("Select a current space to place or join a call.")
                    .color(Style::TEXT_DIM),
            );
            return;
        };

        // Everything the body needs, read up front so no `data` borrow is held
        // across the `&mut self` render calls below.
        let me = data.me().clone();
        let now = data.now_unix_ms();
        let calls = data.call_state().active.clone();
        self.reconcile_media_intent(&calls, &me);
        let (space_name, direct) = space_context(&directory, space);
        let can_start = call_start_enabled(&directory, space);

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
                self.call_start_cluster(ui, sink, space, can_start);
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
                egui::RichText::new(if can_start {
                    "Start an audio, video, or screen-share call above — it appears here and in \
                     the call bar for everyone in the space."
                } else {
                    "No other members are available in this space. Call controls stay disabled \
                     until a peer joins."
                })
                .small()
                .color(Style::TEXT_DIM),
            );
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("collab-calls")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let visible = crate::car_glance_limit(ui, calls.len());
                for call in calls.iter().take(visible) {
                    self.call_card(ui, sink, &me, &directory, now, call);
                    ui.add_space(Style::SP_XS);
                }
                if visible < calls.len() {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} more active call{} available when stopped",
                            calls.len() - visible,
                            if calls.len() - visible == 1 {
                                " is"
                            } else {
                                "s are"
                            }
                        ))
                        .small()
                        .color(Style::TEXT_DIM),
                    );
                }
            });
    }

    /// The start cluster (screen-share · video · audio), laid out right-to-left so
    /// audio reads first. Each button emits [`StartCall`](CollabCommand::StartCall)
    /// with its [`CallKind`] for the selected space.
    fn call_start_cluster(
        &self,
        ui: &mut egui::Ui,
        sink: &mut CommandSink,
        space: SpaceId,
        can_start: bool,
    ) {
        let hint = if can_start {
            None
        } else {
            Some("Unavailable: this space has no other current members")
        };
        let tint = if can_start {
            None
        } else {
            Some(Style::DISABLED)
        };

        // Disable the whole cluster at the egui level as well as changing the
        // tint. This makes the no-peer state non-activatable for pointer and
        // keyboard input, rather than relying on a colour-only affordance.
        ui.add_enabled_ui(can_start, |ui| {
            let screen = icons::icon_button(
                ui,
                icons::CALL_SCREEN,
                Style::SP_M,
                tint.unwrap_or(Style::TEXT_DIM),
                call_start_hint(CallKind::Screen, can_start),
            );
            screen.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    call_start_hint(CallKind::Screen, can_start),
                )
            });
            if screen.clicked() {
                self.start_call(sink, space, CallKind::Screen);
            }
            let video = icons::icon_button(
                ui,
                icons::CALL_VIDEO,
                Style::SP_M,
                tint.unwrap_or(Style::ACCENT),
                call_start_hint(CallKind::Video, can_start),
            );
            video.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    call_start_hint(CallKind::Video, can_start),
                )
            });
            if video.clicked() {
                self.start_call(sink, space, CallKind::Video);
            }
            let audio = icons::icon_button(
                ui,
                icons::CALL_AUDIO,
                Style::SP_M,
                tint.unwrap_or(Style::OK),
                call_start_hint(CallKind::Audio, can_start),
            );
            audio.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    call_start_hint(CallKind::Audio, can_start),
                )
            });
            if audio.clicked() {
                self.start_call(sink, space, CallKind::Audio);
            }
        });
        if let Some(hint) = hint {
            ui.label(
                egui::RichText::new("Call actions unavailable: no other current members")
                    .small()
                    .color(Style::TEXT_DIM),
            )
            .comms_hover_text(hint);
        }
    }

    /// The media device row: visible mic / camera / screen selectors, deliberately
    /// disabled until a real provider enumerates devices. The values are local
    /// seat state; the live device list + binding is a marked media-plane follow-up.
    pub(crate) fn call_device_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("Devices")
                    .small()
                    .strong()
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_S);
            device_combo(ui, CallDevice::Microphone, &mut self.call_media.mic);
            device_combo(ui, CallDevice::Camera, &mut self.call_media.camera);
            device_combo(ui, CallDevice::Screen, &mut self.call_media.screen);
        });
        ui.label(
            egui::RichText::new(
                "Provider devices unavailable: no live media provider has published device \
                 inventory to this Calls surface yet, so these selectors remain disabled.",
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

        mde_egui::card().show(ui, |ui| {
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
    /// keypad toggle, and hang up. Mute + DTMF emit the live-leg commands the
    /// P2P media worker applies to a bound audio sender; the camera / screen
    /// toggles record the seat's outgoing-media intent (a marked media-plane
    /// follow-up).
    fn connected_controls(
        &mut self,
        ui: &mut egui::Ui,
        sink: &mut CommandSink,
        call: CallId,
        muted: bool,
    ) {
        ui.horizontal(|ui| {
            // Microphone mute — live-leg command. The mackesd P2P worker applies
            // this to the bound seat audio sender (or refuses with a typed
            // unavailable media session when no device is bound).
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

    /// Emit [`SetCallMuted`](CollabCommand::SetCallMuted) for the live audio
    /// sender owned by the mackesd P2P media worker.
    pub(crate) fn set_call_muted(&self, sink: &mut CommandSink, call: CallId, muted: bool) {
        sink.emit(CollabCommand::SetCallMuted { call, muted });
    }

    /// Emit [`SendDtmf`](CollabCommand::SendDtmf) for the live audio sender
    /// owned by the mackesd P2P media worker.
    pub(crate) fn send_dtmf(&self, sink: &mut CommandSink, call: CallId, digit: char) {
        sink.emit(CollabCommand::SendDtmf { call, digit });
    }

    /// Reconcile recorded camera/screen intent with the converged call
    /// projection. Device preferences remain seat-level state, but outgoing
    /// track intent is meaningful only while this seat has a connected leg.
    /// This keeps the marked capture follow-up honest after a call ends or the
    /// local participant leaves, without inventing a media-plane event.
    pub(crate) fn reconcile_media_intent(&mut self, calls: &[CallView], me: &ActorId) {
        let connected = calls.iter().any(|call| {
            call.participants.iter().any(|participant| {
                &participant.actor == me && participant.state == CallParticipantState::Connected
            })
        });
        if !connected {
            self.call_media.camera_on = false;
            self.call_media.screen_sharing = false;
        }
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

/// The media device selectors remain visible while the provider/device read-model
/// contract is absent. Keeping the selector present makes the missing capability
/// actionable to an operator without inventing hardware or a live media route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallDevice {
    Microphone,
    Camera,
    Screen,
}

impl CallDevice {
    const fn label(self) -> &'static str {
        match self {
            Self::Microphone => "Microphone",
            Self::Camera => "Camera",
            Self::Screen => "Screen",
        }
    }

    const fn glyph(self) -> &'static str {
        match self {
            Self::Microphone => icons::CALL_AUDIO,
            Self::Camera => icons::CALL_CAMERA,
            Self::Screen => icons::CALL_SHARE_SCREEN,
        }
    }

    const fn unavailable_reason(self) -> &'static str {
        match self {
            Self::Microphone => {
                "Unavailable: no live media provider has published microphone devices to this Calls surface yet"
            }
            Self::Camera => {
                "Unavailable: no live media provider has published camera devices to this Calls surface yet"
            }
            Self::Screen => {
                "Unavailable: no live media provider has published screen-capture sources to this Calls surface yet"
            }
        }
    }

    fn accessible_label(self) -> String {
        format!(
            "{} device selector, disabled. {}",
            self.label(),
            self.unavailable_reason()
        )
    }
}

/// A labeled media device combo — the glyph, then a disabled egui combo showing
/// the honest system default. The real device list comes from the media plane;
/// until that provider enumerates, this is visible but non-actionable and has a
/// device-specific disabled reason for hover and assistive technology.
fn device_combo(ui: &mut egui::Ui, device: CallDevice, value: &mut String) {
    icons::icon(ui, device.glyph(), Style::SP_M, Style::TEXT_DIM).comms_hover_text(device.label());
    let accessible_label = device.accessible_label();
    let response = ui
        .add_enabled_ui(false, |ui| {
            egui::ComboBox::new(("mde-collab-call-device", device.label()), device.label())
                .selected_text(bounded_display_text(value, MAX_CALL_LABEL_CHARS))
                .show_ui(ui, |ui| {
                    // WL-FUNC-011 media: only the honest system default is shown today;
                    // the real enumerated device list comes from WebRTC getUserMedia /
                    // the LiveKit device registry, never a faked list.
                    ui.label(DEFAULT_DEVICE);
                })
        })
        .inner
        .response;
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, false, accessible_label.as_str())
    });
    response.comms_hover_text(accessible_label);
    ui.add_space(Style::SP_S);
}

/// The accessible/hover name for each peer-gated start action. The typed command
/// seam remains unchanged; this is only presentation metadata for the no-peer
/// state.
const fn call_start_hint(kind: CallKind, can_start: bool) -> &'static str {
    if can_start {
        match kind {
            CallKind::Audio => "Start an audio call",
            CallKind::Video => "Start a video call",
            CallKind::Screen => "Start a screen share",
            CallKind::CoEdit => "Start a co-edit session",
            CallKind::RemoteDesktop => "Start a remote desktop session",
        }
    } else {
        match kind {
            CallKind::Audio => {
                "Start an audio call — unavailable: this space has no other current members"
            }
            CallKind::Video => {
                "Start a video call — unavailable: this space has no other current members"
            }
            CallKind::Screen => {
                "Start a screen share — unavailable: this space has no other current members"
            }
            CallKind::CoEdit => {
                "Start a co-edit session — unavailable: this space has no other current members"
            }
            CallKind::RemoteDesktop => {
                "Start a remote desktop session — unavailable: this space has no other current members"
            }
        }
    }
}

/// Whether the selected directory row has at least one other member to receive
/// a new call. The count is a retained membership fact, not an online-presence
/// claim: a peer may still be offline or partitioned, in which case the worker
/// remains responsible for the honest ringing/queued state.
#[must_use]
fn call_start_enabled(directory: &SpaceDirectory, space: SpaceId) -> bool {
    directory
        .spaces
        .iter()
        .find(|summary| summary.id == space)
        .is_some_and(|summary| summary.members > 1)
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
        let suffix = if &p.actor == me { " (you)" } else { "" };
        let name_limit = MAX_CALL_LABEL_CHARS.saturating_sub(suffix.chars().count());
        let mut name = bounded_display_text(p.actor.as_str(), name_limit);
        name.push_str(suffix);
        ui.label(egui::RichText::new(name).color(Style::TEXT));
        ui.label(
            egui::RichText::new(bounded_display_text(label, MAX_CALL_STATUS_CHARS))
                .small()
                .color(color),
        );
        if p.muted {
            icons::icon(ui, icons::CALL_MUTE, Style::SP_M, Style::TEXT_DIM)
                .comms_hover_text("Muted");
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
        |s| {
            (
                bounded_display_text(&s.name, MAX_CALL_LABEL_CHARS),
                s.kind == SpaceKind::Direct,
            )
        },
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

#[cfg(test)]
mod tests {
    use super::{bounded_display_text, call_start_enabled, call_start_hint, CallDevice};
    use mde_collab_types::{
        ActorClock, CallKind, SpaceDirectory, SpaceId, SpaceKind, SpaceRole, SpaceSummary,
    };

    #[test]
    fn bounded_display_text_is_single_line_and_hides_direction_controls() {
        assert_eq!(
            bounded_display_text("alice\n\t\u{202e}ops", 48),
            "alice ops"
        );
    }

    #[test]
    fn bounded_display_text_truncates_by_unicode_scalar_without_overflow() {
        assert_eq!(bounded_display_text("🚗 fleet participant", 8), "🚗 fleet…");
    }

    #[test]
    fn bounded_display_text_uses_a_placeholder_for_empty_input() {
        assert_eq!(bounded_display_text("\u{200b}\n", 48), "—");
        assert_eq!(bounded_display_text("anything", 0), "");
    }

    #[test]
    fn call_start_requires_another_current_space_member() {
        let space = SpaceId::new();
        let mut directory = SpaceDirectory::default();
        assert!(!call_start_enabled(&directory, space));

        directory.spaces.push(SpaceSummary {
            id: space,
            kind: SpaceKind::Team,
            name: "Operations".to_owned(),
            role: SpaceRole::Owner,
            unread: 0,
            members: 1,
            last_activity: ActorClock::at(0, 0),
        });
        assert!(
            !call_start_enabled(&directory, space),
            "a solo space must not expose a call action"
        );

        directory.spaces[0].members = 2;
        assert!(
            call_start_enabled(&directory, space),
            "a space with another member may expose a call action"
        );
        assert!(
            !call_start_enabled(&directory, SpaceId::new()),
            "a stale or unknown space id must never become a call target"
        );
    }

    #[test]
    fn unavailable_devices_have_specific_reasons_and_accessible_labels() {
        let devices = [
            CallDevice::Microphone,
            CallDevice::Camera,
            CallDevice::Screen,
        ];
        let reasons: Vec<_> = devices
            .iter()
            .map(|device| device.unavailable_reason())
            .collect();

        assert_eq!(reasons.len(), 3);
        assert!(reasons.iter().all(|reason| {
            reason.starts_with("Unavailable: no live media provider has published")
        }));
        assert!(reasons.iter().all(|reason| reason.ends_with("yet")));
        assert_ne!(reasons[0], reasons[1]);
        assert_ne!(reasons[1], reasons[2]);
        for device in devices {
            let label = device.accessible_label();
            assert!(label.starts_with(device.label()));
            assert!(label.contains("device selector, disabled."));
            assert!(label.contains(device.unavailable_reason()));
        }
    }

    #[test]
    fn peer_gated_call_actions_keep_the_action_name_in_the_disabled_label() {
        for (kind, action) in [
            (CallKind::Audio, "audio call"),
            (CallKind::Video, "video call"),
            (CallKind::Screen, "screen share"),
        ] {
            let hint = call_start_hint(kind, false);
            assert!(hint.starts_with("Start "));
            assert!(hint.contains(action));
            assert!(hint.contains("unavailable: this space has no other current members"));
            assert!(!call_start_hint(kind, true).contains("unavailable"));
        }
    }
}
