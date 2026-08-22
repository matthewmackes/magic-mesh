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
//! # Media plane: mute/DTMF and camera/screen follow [`MediaSessionV1`]
//!
//! Mute and DTMF are live-leg verbs. The renderer still only emits typed
//! [`CollabCommand`]s; the mackesd P2P worker (`call_media`) applies them to the
//! bound seat audio. When a [`MediaSessionV1`] projection is present they emit
//! only if that session binds the matching audio/DTMF sender; otherwise the
//! control is an honest unavailable state, never a view-only intent bit.
//!
//! Camera and screen follow the same projection: an offered live track renders
//! as attached, and every other case is unavailable. Device selectors stay
//! disabled because [`MediaSessionV1`] does not carry device names. Group SFU
//! (S3) and SIP gateway PSTN (S4) remain worker-owned. There is deliberately
//! **no recording and no transcription** anywhere — not in this UI, not in the
//! commands, not in the worker or its state.

use mde_egui::egui;
use mde_egui::Style;

use mde_collab_types::{
    ActorId, CallId, CallKind, CallParticipantState, CallParticipantView, CallView, CollabCommand,
    MediaFailureReasonV1, MediaSessionStateV1, MediaSessionV1, MediaTrackKind, SpaceDirectory,
    SpaceId, SpaceKind,
};

use crate::frame::call_kind_label;
use crate::icons::CommsHoverExt;
use crate::{icons, relative_age, CommandSink, CommunicationsSurface};

/// The honest label while [`MediaSessionV1`] carries tracks but not device
/// names. Selectors stay disabled rather than inventing a hardware list.
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

/// The local seat's Calls-mode media preferences — selected device labels,
/// outgoing camera / screen bits mirrored from a live session, and any
/// published [`MediaSessionV1`] documents.
///
/// **Seat-level view state only.** This pure UI crate never touches a real capture
/// device: live microphone bind and mute/DTMF injection are owned by the mackesd
/// P2P media worker, driven by [`SetCallMuted`](CollabCommand::SetCallMuted) and
/// [`SendDtmf`](CollabCommand::SendDtmf) when a session binds those senders.
#[derive(Debug, Clone)]
pub(crate) struct CallMediaPrefs {
    /// The chosen microphone device (default: the system default).
    pub(crate) mic: String,
    /// The chosen camera device (default: the system default).
    pub(crate) camera: String,
    /// The chosen screen-capture source (default: the system default).
    pub(crate) screen: String,
    /// Whether a published session has attached an outgoing camera track.
    pub(crate) camera_on: bool,
    /// Whether a published session has attached an outgoing screen track.
    pub(crate) screen_sharing: bool,
    /// Retained live-media projections for active calls. Empty until a mount
    /// publishes [`MediaSessionV1`] documents; this never invents a connected call.
    pub(crate) sessions: Vec<MediaSessionV1>,
}

impl Default for CallMediaPrefs {
    fn default() -> Self {
        Self {
            mic: DEFAULT_DEVICE.to_owned(),
            camera: DEFAULT_DEVICE.to_owned(),
            screen: DEFAULT_DEVICE.to_owned(),
            camera_on: false,
            screen_sharing: false,
            sessions: Vec::new(),
        }
    }
}

impl CallMediaPrefs {
    fn session_for(&self, call: CallId) -> Option<&MediaSessionV1> {
        self.sessions.iter().find(|session| session.session == call)
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

        // The media device row (mic / camera / screen). Device names are not
        // on MediaSessionV1, so the selectors stay disabled; a published
        // session can still refine the unavailable reason.
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

    /// The media device row: visible mic / camera / screen selectors, disabled
    /// because [`MediaSessionV1`] does not publish device names. A present
    /// session can name a typed device-absent or permission-denied reason.
    pub(crate) fn call_device_row(&mut self, ui: &mut egui::Ui) {
        let session = self.call_media.sessions.first();
        let row_reason = device_row_reason(session);
        let mic_reason = CallDevice::Microphone.unavailable_reason_on_plane(session);
        let camera_reason = CallDevice::Camera.unavailable_reason_on_plane(session);
        let screen_reason = CallDevice::Screen.unavailable_reason_on_plane(session);
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
                CallDevice::Microphone,
                &mut self.call_media.mic,
                mic_reason,
            );
            device_combo(
                ui,
                CallDevice::Camera,
                &mut self.call_media.camera,
                camera_reason,
            );
            device_combo(
                ui,
                CallDevice::Screen,
                &mut self.call_media.screen,
                screen_reason,
            );
        });
        ui.label(
            egui::RichText::new(row_reason)
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
                    let session = self.call_media.session_for(call.call).cloned();
                    let muted = session
                        .as_ref()
                        .map(|plane| plane.local_muted)
                        .unwrap_or_else(|| mine.as_ref().is_some_and(|p| p.muted));
                    self.connected_controls(ui, sink, call.call, muted, session.as_ref());
                    if self.dtmf_pad == Some(call.call) {
                        self.dtmf_keypad(ui, sink, call.call, session.as_ref());
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
    /// keypad toggle, and hang up. Mute + DTMF emit only when the optional
    /// [`MediaSessionV1`] binds those senders (or when no session is present,
    /// so the worker can apply the signaling command once a leg binds). Camera
    /// and screen render attached or unavailable from that same projection.
    fn connected_controls(
        &mut self,
        ui: &mut egui::Ui,
        sink: &mut CommandSink,
        call: CallId,
        muted: bool,
        session: Option<&MediaSessionV1>,
    ) {
        ui.horizontal(|ui| {
            let mute_effect = live_audio_effect(session, LiveAudioKind::Mute);
            let (mic_glyph, mic_hint) = if muted {
                (icons::CALL_UNMUTE, "Unmute microphone")
            } else {
                (icons::CALL_MUTE, "Mute microphone")
            };
            let mic_hint = match mute_effect {
                LiveAudioEffect::Unavailable => {
                    live_audio_unavailable_reason(session, LiveAudioKind::Mute)
                }
                LiveAudioEffect::Live | LiveAudioEffect::Signaling => mic_hint,
            };
            ui.add_enabled_ui(mute_effect.can_emit(), |ui| {
                if icons::icon_button(ui, mic_glyph, Style::SP_M, Style::TEXT_DIM, mic_hint)
                    .clicked()
                {
                    self.set_call_muted_with_session(sink, call, !muted, session);
                }
            });

            outgoing_track_status(
                ui,
                icons::CALL_CAMERA,
                "camera",
                outgoing_track_effect(session, MediaTrackKind::Video),
                outgoing_track_unavailable_reason(session, MediaTrackKind::Video),
            );
            outgoing_track_status(
                ui,
                icons::CALL_SHARE_SCREEN,
                "screen share",
                outgoing_track_effect(session, MediaTrackKind::Screen),
                outgoing_track_unavailable_reason(session, MediaTrackKind::Screen),
            );

            let dtmf_effect = live_audio_effect(session, LiveAudioKind::Dtmf);
            let dtmf_open = self.dtmf_pad == Some(call);
            let dtmf_tint = if dtmf_open {
                Style::ACCENT
            } else {
                Style::TEXT_DIM
            };
            let dtmf_hint = match dtmf_effect {
                LiveAudioEffect::Unavailable => {
                    live_audio_unavailable_reason(session, LiveAudioKind::Dtmf)
                }
                LiveAudioEffect::Live | LiveAudioEffect::Signaling => "DTMF keypad",
            };
            ui.add_enabled_ui(dtmf_effect.can_emit(), |ui| {
                if icons::icon_button(ui, icons::CALL_DTMF, Style::SP_M, dtmf_tint, dtmf_hint)
                    .clicked()
                {
                    self.dtmf_pad = if dtmf_open { None } else { Some(call) };
                }
            });

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
    /// a [`SendDtmf`](CollabCommand::SendDtmf) for `call` when the media session
    /// binds a DTMF sender (or when no session is present).
    fn dtmf_keypad(
        &self,
        ui: &mut egui::Ui,
        sink: &mut CommandSink,
        call: CallId,
        session: Option<&MediaSessionV1>,
    ) {
        ui.add_space(Style::SP_XS);
        ui.label(
            egui::RichText::new("DTMF")
                .small()
                .strong()
                .color(Style::TEXT_DIM),
        );
        let enabled = live_audio_effect(session, LiveAudioKind::Dtmf).can_emit();
        ui.add_enabled_ui(enabled, |ui| {
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
                            self.send_dtmf_with_session(sink, call, digit, session);
                        }
                    }
                });
            }
        });
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
    /// sender owned by the mackesd P2P media worker. No session means the
    /// signaling path; a present session must bind audio or this is a no-op.
    pub(crate) fn set_call_muted(&self, sink: &mut CommandSink, call: CallId, muted: bool) {
        sink.emit(CollabCommand::SetCallMuted { call, muted });
    }

    /// Session-aware mute: refuse to record view-only intent when a
    /// [`MediaSessionV1`] is present without a bound audio sender.
    pub(crate) fn set_call_muted_with_session(
        &self,
        sink: &mut CommandSink,
        call: CallId,
        muted: bool,
        session: Option<&MediaSessionV1>,
    ) {
        if live_audio_effect(session, LiveAudioKind::Mute).can_emit() {
            self.set_call_muted(sink, call, muted);
        }
    }

    /// Emit [`SendDtmf`](CollabCommand::SendDtmf) for the live audio sender
    /// owned by the mackesd P2P media worker. No session means the signaling
    /// path; a present session must bind DTMF or this is a no-op.
    pub(crate) fn send_dtmf(&self, sink: &mut CommandSink, call: CallId, digit: char) {
        sink.emit(CollabCommand::SendDtmf { call, digit });
    }

    /// Session-aware DTMF: refuse to record view-only intent when a
    /// [`MediaSessionV1`] is present without a bound DTMF sender.
    pub(crate) fn send_dtmf_with_session(
        &self,
        sink: &mut CommandSink,
        call: CallId,
        digit: char,
        session: Option<&MediaSessionV1>,
    ) {
        if live_audio_effect(session, LiveAudioKind::Dtmf).can_emit() {
            self.send_dtmf(sink, call, digit);
        }
    }

    /// Publish retained [`MediaSessionV1`] documents into this seat's Calls view.
    /// Empty input is the honest no-projection state — never a faked connected call.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn apply_media_sessions(&mut self, sessions: Vec<MediaSessionV1>) {
        self.call_media.sessions = sessions;
    }

    /// Reconcile outgoing camera/screen bits with the converged call projection
    /// and any published media session. Device preferences remain seat-level
    /// labels. When a session is present, the bits mirror offered live tracks;
    /// otherwise they only survive while this seat has a connected signaling leg.
    pub(crate) fn reconcile_media_intent(&mut self, calls: &[CallView], me: &ActorId) {
        let connected = calls.iter().any(|call| {
            call.participants.iter().any(|participant| {
                &participant.actor == me && participant.state == CallParticipantState::Connected
            })
        });
        if !connected {
            self.call_media.camera_on = false;
            self.call_media.screen_sharing = false;
            return;
        }
        let (camera_on, screen_sharing) = {
            let session = calls.iter().find_map(|call| {
                let local_connected = call.participants.iter().any(|participant| {
                    &participant.actor == me && participant.state == CallParticipantState::Connected
                });
                if local_connected {
                    self.call_media.session_for(call.call)
                } else {
                    None
                }
            });
            match session {
                Some(session) => (
                    outgoing_track_effect(Some(session), MediaTrackKind::Video)
                        == OutgoingTrackEffect::Attached,
                    outgoing_track_effect(Some(session), MediaTrackKind::Screen)
                        == OutgoingTrackEffect::Attached,
                ),
                None => return,
            }
        };
        self.call_media.camera_on = camera_on;
        self.call_media.screen_sharing = screen_sharing;
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

    fn accessible_label_for(self, reason: &str) -> String {
        if reason == self.unavailable_reason() {
            self.accessible_label()
        } else {
            format!("{} device selector, disabled. {}", self.label(), reason)
        }
    }

    fn unavailable_reason_on_plane(self, session: Option<&MediaSessionV1>) -> &'static str {
        match (session.map(|plane| &plane.state), self) {
            (
                Some(MediaSessionStateV1::DeviceAbsent {
                    track: MediaTrackKind::Audio,
                }),
                Self::Microphone,
            ) => "Unavailable: no microphone is present on the live media session",
            (
                Some(MediaSessionStateV1::DeviceAbsent {
                    track: MediaTrackKind::Video,
                }),
                Self::Camera,
            ) => "Unavailable: no camera is present on the live media session",
            (
                Some(MediaSessionStateV1::DeviceAbsent {
                    track: MediaTrackKind::Screen,
                }),
                Self::Screen,
            ) => "Unavailable: no screen-capture source is present on the live media session",
            (
                Some(MediaSessionStateV1::PermissionDenied {
                    track: MediaTrackKind::Audio,
                }),
                Self::Microphone,
            ) => "Unavailable: microphone permission denied on the live media session",
            (
                Some(MediaSessionStateV1::PermissionDenied {
                    track: MediaTrackKind::Video,
                }),
                Self::Camera,
            ) => "Unavailable: camera permission denied on the live media session",
            (
                Some(MediaSessionStateV1::PermissionDenied {
                    track: MediaTrackKind::Screen,
                }),
                Self::Screen,
            ) => "Unavailable: screen-capture permission denied on the live media session",
            _ => self.unavailable_reason(),
        }
    }
}

/// A labeled media device combo — the glyph, then a disabled egui combo showing
/// the honest system default. [`MediaSessionV1`] does not carry device names, so
/// this stays visible but non-actionable, with a device-specific disabled reason
/// for hover and assistive technology.
fn device_combo(ui: &mut egui::Ui, device: CallDevice, value: &mut String, reason: &str) {
    icons::icon(ui, device.glyph(), Style::SP_M, Style::TEXT_DIM).comms_hover_text(device.label());
    let accessible_label = device.accessible_label_for(reason);
    let response = ui
        .add_enabled_ui(false, |ui| {
            egui::ComboBox::new(("mde-collab-call-device", device.label()), device.label())
                .selected_text(bounded_display_text(value, MAX_CALL_LABEL_CHARS))
                .show_ui(ui, |ui| {
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

/// Whether mute or DTMF can act on a live (or still-signaling) audio leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveAudioEffect {
    /// No [`MediaSessionV1`]: emit the typed command; the worker applies it when bound.
    Signaling,
    /// Session proves the matching audio/DTMF bind is present.
    Live,
    /// Session is present but this control has no live sender.
    Unavailable,
}

impl LiveAudioEffect {
    #[must_use]
    pub(crate) const fn can_emit(self) -> bool {
        matches!(self, Self::Live | Self::Signaling)
    }
}

/// Which live-audio control is being classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveAudioKind {
    Mute,
    Dtmf,
}

/// Classify mute/DTMF against an optional live-media projection.
#[must_use]
pub(crate) fn live_audio_effect(
    session: Option<&MediaSessionV1>,
    kind: LiveAudioKind,
) -> LiveAudioEffect {
    let Some(session) = session else {
        return LiveAudioEffect::Signaling;
    };
    let bound = match kind {
        LiveAudioKind::Mute => session.audio_bound,
        LiveAudioKind::Dtmf => session.dtmf_bound,
    };
    if bound
        && !matches!(
            session.state,
            MediaSessionStateV1::Failed { .. }
                | MediaSessionStateV1::DeviceAbsent {
                    track: MediaTrackKind::Audio
                }
                | MediaSessionStateV1::PermissionDenied {
                    track: MediaTrackKind::Audio
                }
        )
    {
        LiveAudioEffect::Live
    } else {
        LiveAudioEffect::Unavailable
    }
}

#[must_use]
fn live_audio_unavailable_reason(
    session: Option<&MediaSessionV1>,
    kind: LiveAudioKind,
) -> &'static str {
    match (session.map(|plane| &plane.state), kind) {
        (
            Some(MediaSessionStateV1::DeviceAbsent {
                track: MediaTrackKind::Audio,
            }),
            _,
        ) => "Unavailable: no microphone is bound on the live media session",
        (
            Some(MediaSessionStateV1::PermissionDenied {
                track: MediaTrackKind::Audio,
            }),
            _,
        ) => "Unavailable: microphone permission denied on the live media session",
        (Some(MediaSessionStateV1::Failed { reason }), _) => match reason {
            MediaFailureReasonV1::TransportUnavailable => {
                "Unavailable: no media transport is bound on this seat"
            }
            MediaFailureReasonV1::InvalidSignaling => "Unavailable: media signaling failed",
            MediaFailureReasonV1::PeerDropped => "Unavailable: the remote peer dropped",
            MediaFailureReasonV1::NegotiationTimeout => "Unavailable: media negotiation timed out",
            MediaFailureReasonV1::SfuUnreachable => {
                "Unavailable: the group media host is unreachable"
            }
            MediaFailureReasonV1::DeviceUnplugged => "Unavailable: the media device was unplugged",
            MediaFailureReasonV1::PermissionRevoked => "Unavailable: media permission was revoked",
        },
        (Some(MediaSessionStateV1::Reconnecting { .. }), _) => {
            "Unavailable: the media session is reconnecting"
        }
        (_, LiveAudioKind::Mute) => {
            "Unavailable: mute has no bound audio sender on this media session"
        }
        (_, LiveAudioKind::Dtmf) => {
            "Unavailable: DTMF has no bound audio sender on this media session"
        }
    }
}

/// Whether an outgoing camera or screen track is attached on the live plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutgoingTrackEffect {
    /// The published session offered this track on a live connected leg.
    Attached,
    /// No session, no offered track, or a typed unavailable media state.
    Unavailable,
}

/// Derive camera/screen attach from typed session state, never a local toggle.
#[must_use]
pub(crate) fn outgoing_track_effect(
    session: Option<&MediaSessionV1>,
    track: MediaTrackKind,
) -> OutgoingTrackEffect {
    let Some(session) = session else {
        return OutgoingTrackEffect::Unavailable;
    };
    if session.offered_tracks.contains(&track) && session.state.claims_live_media() {
        OutgoingTrackEffect::Attached
    } else {
        OutgoingTrackEffect::Unavailable
    }
}

#[must_use]
fn outgoing_track_unavailable_reason(
    session: Option<&MediaSessionV1>,
    track: MediaTrackKind,
) -> &'static str {
    match (session.map(|plane| &plane.state), track) {
        (Some(MediaSessionStateV1::DeviceAbsent { track: absent }), asked) if *absent == asked => {
            match asked {
                MediaTrackKind::Video => {
                    "Unavailable: no camera is present on the live media session"
                }
                MediaTrackKind::Screen => {
                    "Unavailable: no screen-capture source is present on the live media session"
                }
                MediaTrackKind::Audio => {
                    "Unavailable: no microphone is present on the live media session"
                }
            }
        }
        (Some(MediaSessionStateV1::PermissionDenied { track: denied }), asked)
            if *denied == asked =>
        {
            match asked {
                MediaTrackKind::Video => {
                    "Unavailable: camera permission denied on the live media session"
                }
                MediaTrackKind::Screen => {
                    "Unavailable: screen-capture permission denied on the live media session"
                }
                MediaTrackKind::Audio => {
                    "Unavailable: microphone permission denied on the live media session"
                }
            }
        }
        (_, MediaTrackKind::Video) => {
            "Unavailable: no MediaSessionV1 projection has attached a camera track"
        }
        (_, MediaTrackKind::Screen) => {
            "Unavailable: no MediaSessionV1 projection has attached a screen track"
        }
        (_, MediaTrackKind::Audio) => {
            "Unavailable: no MediaSessionV1 projection has attached an audio track"
        }
    }
}

/// Camera / screen status from typed media state. Not a local intent toggle.
fn outgoing_track_status(
    ui: &mut egui::Ui,
    glyph: &str,
    what: &str,
    effect: OutgoingTrackEffect,
    unavailable: &str,
) {
    let (tint, hint) = match effect {
        OutgoingTrackEffect::Attached => (
            Style::ACCENT,
            format!("{what} attached on the live media session"),
        ),
        OutgoingTrackEffect::Unavailable => (Style::TEXT_DIM, unavailable.to_owned()),
    };
    ui.add_enabled_ui(false, |ui| {
        let _ = icons::icon_button(ui, glyph, Style::SP_M, tint, &hint);
    });
}

fn device_row_reason(session: Option<&MediaSessionV1>) -> &'static str {
    match session.map(|plane| &plane.state) {
        Some(MediaSessionStateV1::DeviceAbsent { track }) => match track {
            MediaTrackKind::Audio => {
                "Provider devices unavailable: the live media session has no microphone."
            }
            MediaTrackKind::Video => {
                "Provider devices unavailable: the live media session has no camera."
            }
            MediaTrackKind::Screen => {
                "Provider devices unavailable: the live media session has no screen-capture source."
            }
        },
        Some(MediaSessionStateV1::PermissionDenied { track }) => match track {
            MediaTrackKind::Audio => {
                "Provider devices unavailable: microphone permission denied on the live media session."
            }
            MediaTrackKind::Video => {
                "Provider devices unavailable: camera permission denied on the live media session."
            }
            MediaTrackKind::Screen => {
                "Provider devices unavailable: screen-capture permission denied on the live media session."
            }
        },
        _ => {
            "Provider devices unavailable: no live media provider has published device \
             inventory to this Calls surface yet, so these selectors remain disabled."
        }
    }
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
    use super::{
        bounded_display_text, call_start_enabled, call_start_hint, live_audio_effect,
        outgoing_track_effect, CallDevice, LiveAudioEffect, LiveAudioKind, OutgoingTrackEffect,
    };
    use crate::{CommandSink, CommunicationsSurface};
    use mde_collab_types::{
        ActorClock, ActorId, CallId, CallKind, CallMediaAdapter, CollabCommand, MediaDescriptionV1,
        MediaSessionStateV1, MediaSessionV1, MediaSignalingRoleV1, MediaTrackKind, SpaceDirectory,
        SpaceId, SpaceKind, SpaceRole, SpaceSummary,
    };

    fn plane_session(
        call: CallId,
        state: MediaSessionStateV1,
        tracks: Vec<MediaTrackKind>,
        local_muted: bool,
        dtmf_bound: bool,
        audio_bound: bool,
        frames_observed: u64,
        descriptions: bool,
    ) -> MediaSessionV1 {
        let space = SpaceId::new();
        let local = ActorId::new("eagle");
        let remote = ActorId::new("falcon");
        let (local_description, remote_description) = if descriptions {
            (
                Some(
                    MediaDescriptionV1::new(
                        call,
                        local.clone(),
                        remote.clone(),
                        MediaSignalingRoleV1::Offer,
                        tracks.clone(),
                    )
                    .expect("offer"),
                ),
                Some(
                    MediaDescriptionV1::new(
                        call,
                        remote.clone(),
                        local.clone(),
                        MediaSignalingRoleV1::Answer,
                        tracks.clone(),
                    )
                    .expect("answer"),
                ),
            )
        } else {
            (None, None)
        };
        MediaSessionV1::new(
            call,
            space,
            local,
            remote,
            CallMediaAdapter::WebRtcP2p,
            state,
            tracks,
            local_muted,
            dtmf_bound,
            audio_bound,
            frames_observed,
            local_description,
            remote_description,
        )
        .expect("valid media session")
    }

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

    #[test]
    fn mute_and_dtmf_follow_media_session_live_leg() {
        let call = CallId::new();
        let surface = CommunicationsSurface::new();

        let mut sink = CommandSink::new();
        surface.set_call_muted_with_session(&mut sink, call, true, None);
        assert!(
            matches!(
                sink.queued().first(),
                Some(CollabCommand::SetCallMuted { call: c, muted: true }) if *c == call
            ),
            "no MediaSessionV1 keeps mute on the signaling path"
        );

        let absent = plane_session(
            call,
            MediaSessionStateV1::DeviceAbsent {
                track: MediaTrackKind::Audio,
            },
            vec![MediaTrackKind::Audio],
            false,
            false,
            false,
            0,
            false,
        );
        assert_eq!(
            live_audio_effect(Some(&absent), LiveAudioKind::Mute),
            LiveAudioEffect::Unavailable
        );
        let mut sink = CommandSink::new();
        surface.set_call_muted_with_session(&mut sink, call, true, Some(&absent));
        surface.send_dtmf_with_session(&mut sink, call, '5', Some(&absent));
        assert!(
            sink.is_empty(),
            "a present MediaSessionV1 without a bound sender must not keep mute/DTMF as view-only intent"
        );

        let live = plane_session(
            call,
            MediaSessionStateV1::Connected,
            vec![MediaTrackKind::Audio],
            false,
            true,
            true,
            4,
            true,
        );
        assert_eq!(
            live_audio_effect(Some(&live), LiveAudioKind::Mute),
            LiveAudioEffect::Live
        );
        assert_eq!(
            live_audio_effect(Some(&live), LiveAudioKind::Dtmf),
            LiveAudioEffect::Live
        );
        assert_eq!(
            outgoing_track_effect(Some(&live), MediaTrackKind::Video),
            OutgoingTrackEffect::Unavailable
        );
        assert_eq!(
            outgoing_track_effect(None, MediaTrackKind::Screen),
            OutgoingTrackEffect::Unavailable
        );

        let mut sink = CommandSink::new();
        surface.set_call_muted_with_session(&mut sink, call, true, Some(&live));
        surface.send_dtmf_with_session(&mut sink, call, '9', Some(&live));
        assert!(
            matches!(
                sink.queued(),
                [
                    CollabCommand::SetCallMuted { call: c, muted: true },
                    CollabCommand::SendDtmf { digit: '9', .. }
                ] if *c == call
            ),
            "a bound MediaSessionV1 must route mute/DTMF to the live sender"
        );

        let video = plane_session(
            call,
            MediaSessionStateV1::Connected,
            vec![MediaTrackKind::Audio, MediaTrackKind::Video],
            false,
            true,
            true,
            4,
            true,
        );
        assert_eq!(
            outgoing_track_effect(Some(&video), MediaTrackKind::Video),
            OutgoingTrackEffect::Attached
        );
        assert_eq!(
            outgoing_track_effect(Some(&video), MediaTrackKind::Screen),
            OutgoingTrackEffect::Unavailable
        );
        assert_eq!(
            CallDevice::Microphone.unavailable_reason_on_plane(Some(&absent)),
            "Unavailable: no microphone is present on the live media session"
        );

        let mut surface = CommunicationsSurface::new();
        surface.apply_media_sessions(vec![live.clone()]);
        assert_eq!(
            surface
                .call_media
                .session_for(call)
                .map(|session| session.audio_bound),
            Some(true)
        );
    }
}
