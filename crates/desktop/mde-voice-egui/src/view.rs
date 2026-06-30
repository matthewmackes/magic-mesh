//! The egui rendering of the Voice surface (E12-11).
//!
//! Every widget reads the render-agnostic [`VoiceState`] and draws through the
//! shared [`Style`] — no raw colours or literal metrics (governance §4). The view
//! never mutates the model mid-render: a frame collects the user's intents as
//! [`Command`]s and the caller forwards them to the worker once the frame is
//! done. Status text reuses the SIP state machine's own `RegistrationState`/
//! `CallState` labels (§6 — no re-worded copy).

use mde_egui::egui::{self, Align, Color32, Context, Layout, RichText};
use mde_egui::Style;

use mde_voice_hud::sip::CallState;

use crate::model::{call_tone, dial_ready, registration_tone, Command, Tone, VoiceState};

/// Render the whole Voice surface for one frame, returning the user's intents.
pub fn show(ctx: &Context, state: &VoiceState, dial: &mut String, identity: &str) -> Vec<Command> {
    let mut cmds = Vec::new();
    top_panel(ctx, state, identity, &mut cmds);
    central(ctx, state, dial, &mut cmds);
    cmds
}

/// The header strip: the surface title, the account identity, and the live
/// registration status (dot + the shipped `RegistrationState::label`) with a
/// Retry affordance shown only when registration has actually failed.
fn top_panel(ctx: &Context, state: &VoiceState, identity: &str, cmds: &mut Vec<Command>) {
    egui::TopBottomPanel::top("voice-header").show(ctx, |ui| {
        ui.add_space(Style::SP_XS);
        ui.horizontal(|ui| {
            ui.add_space(Style::SP_S);
            ui.heading(
                RichText::new("Voice")
                    .size(Style::HEADING)
                    .color(Style::TEXT),
            );
            ui.add_space(Style::SP_M);
            ui.label(
                RichText::new(identity)
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(Style::SP_S);
                // Re-register is a no-op for a registrar-less P2P node, so the
                // Retry button only appears when a real registration failed.
                if matches!(registration_tone(&state.registration), Tone::Bad)
                    && ui.button("Retry").clicked()
                {
                    cmds.push(Command::Reregister);
                }
                ui.add_space(Style::SP_S);
                ui.label(
                    RichText::new(state.registration.label())
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.add_space(Style::SP_XS);
                status_dot(ui, tone_color(registration_tone(&state.registration)));
            });
        });
        ui.add_space(Style::SP_XS);
    });
}

/// The body: a transient error banner, then one of three call-lifecycle faces —
/// an incoming-call card, the active-call card, or the dialer.
fn central(ctx: &Context, state: &VoiceState, dial: &mut String, cmds: &mut Vec<Command>) {
    egui::CentralPanel::default().show(ctx, |ui| {
        if let Some(error) = &state.error {
            ui.add_space(Style::SP_XS);
            ui.colored_label(Style::DANGER, error.as_str());
            ui.add_space(Style::SP_XS);
        }
        ui.add_space(Style::SP_S);
        if state.ringing_in() {
            incoming_card(ui, state, cmds);
        } else if state.show_dialer() {
            dialer(ui, state, dial, cmds);
        } else {
            active_card(ui, state, cmds);
        }
    });
}

/// A ringing inbound call: the caller, plus Answer / Decline.
fn incoming_card(ui: &mut egui::Ui, state: &VoiceState, cmds: &mut Vec<Command>) {
    ui.vertical_centered(|ui| {
        ui.add_space(Style::SP_L);
        ui.label(
            RichText::new("Incoming call")
                .size(Style::HEADING)
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_XS);
        if let CallState::Incoming { from } = &state.call {
            ui.label(RichText::new(from).size(Style::BODY).color(Style::ACCENT));
        }
        ui.add_space(Style::SP_M);
        ui.horizontal(|ui| {
            let answer = egui::Button::new(RichText::new("Answer").color(Style::BG).strong())
                .fill(Style::OK);
            if ui.add(answer).clicked() {
                cmds.push(Command::Answer);
            }
            ui.add_space(Style::SP_S);
            if ui.button("Decline").clicked() {
                cmds.push(Command::Decline);
            }
        });
    });
}

/// An active (dialing / connected) call: its shipped status label + Hang up.
fn active_card(ui: &mut egui::Ui, state: &VoiceState, cmds: &mut Vec<Command>) {
    ui.vertical_centered(|ui| {
        ui.add_space(Style::SP_L);
        ui.label(
            RichText::new(state.call.label())
                .size(Style::HEADING)
                .color(tone_color(call_tone(&state.call))),
        );
        ui.add_space(Style::SP_M);
        let hang = egui::Button::new(RichText::new("Hang up").color(Style::BG).strong())
            .fill(Style::DANGER);
        if ui.add(hang).clicked() {
            cmds.push(Command::HangUp);
        }
    });
}

/// The dialer: a free-form target field + Call, over an honest status/guidance
/// line that surfaces the previous call's outcome.
fn dialer(ui: &mut egui::Ui, state: &VoiceState, dial: &mut String, cmds: &mut Vec<Command>) {
    ui.label(
        RichText::new("Place a call")
            .size(Style::BODY)
            .strong()
            .color(Style::TEXT),
    );
    ui.add_space(Style::SP_XS);
    ui.separator();
    ui.add_space(Style::SP_S);

    let mut submit = false;
    ui.horizontal(|ui| {
        let field = ui.add(
            egui::TextEdit::singleline(dial)
                .hint_text("mesh peer name, or a number")
                .desired_width(Style::SP_XL * 8.0),
        );
        submit = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.add_space(Style::SP_S);
        let call =
            egui::Button::new(RichText::new("Call").color(Style::BG).strong()).fill(Style::ACCENT);
        if ui.add_enabled(dial_ready(dial), call).clicked() {
            submit = true;
        }
    });
    if submit && dial_ready(dial) {
        cmds.push(Command::Dial(dial.clone()));
    }

    ui.add_space(Style::SP_S);
    match &state.call {
        CallState::Ended => {
            ui.colored_label(Style::TEXT_DIM, "Call ended.");
        }
        CallState::Failed(why) => {
            ui.colored_label(Style::DANGER, format!("Call failed: {why}"));
        }
        _ => {
            ui.colored_label(
                Style::TEXT_DIM,
                "A mesh peer name dials directly over the overlay; a number dials via the registrar.",
            );
        }
    }
}

// ── Small render helpers ────────────────────────────────────────────────────

/// A small filled circle used as the registration status indicator.
fn status_dot(ui: &mut egui::Ui, color: Color32) {
    let diameter = Style::SP_S;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(diameter, diameter), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), diameter * 0.28, color);
}

/// Map a render-agnostic [`Tone`] to its shared `Style` colour.
const fn tone_color(tone: Tone) -> Color32 {
    match tone {
        Tone::Ok => Style::OK,
        Tone::Busy => Style::ACCENT,
        Tone::Bad => Style::DANGER,
        Tone::Neutral => Style::TEXT_DIM,
    }
}
