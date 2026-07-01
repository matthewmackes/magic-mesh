//! The egui rendering of the Voice surface (E12-11).
//!
//! Every widget reads the render-agnostic [`VoiceState`] and draws through the
//! shared [`Style`] — no raw colours or literal metrics (governance §4). The view
//! never mutates the model mid-render: a frame collects the user's intents as
//! [`Command`]s and the caller forwards them to the worker once the frame is
//! done. Status text reuses the SIP state machine's own `RegistrationState`/
//! `CallState` labels (§6 — no re-worded copy).

use mde_egui::egui::{self, Align, Color32, Layout, RichText};
use mde_egui::Style;

use mde_voice_hud::sip::CallState;

use crate::model::{call_tone, dial_ready, registration_tone, Command, Tone, VoiceState};
use crate::VoiceApp;

/// The header strip rendered into `ui`: the surface title, the account identity,
/// and the live registration status (dot + the shipped `RegistrationState` label)
/// with a Retry affordance shown only for a **registrar-backed** failure — a
/// registrar-less P2P node has no registrar to re-register against, so Retry there
/// would be a dead-end.
///
/// This is the standalone binary's chrome, framed by [`VoiceApp`] in the window's
/// top panel. The embedded shell (E12-3b) supplies its own chrome and renders only
/// [`voice_panel`], so the header stays here with the standalone app rather than in
/// the shared panel body.
pub fn header(ui: &mut egui::Ui, app: &VoiceApp) {
    let mut reregister = false;
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
            RichText::new(&app.identity)
                .size(Style::SMALL)
                .color(Style::TEXT_DIM),
        );

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(Style::SP_S);
            // Re-register is meaningless for a registrar-less P2P node (no
            // registrar to retry, and after an overlay-bind failure the agent
            // has already exited), so Retry appears only for a registrar-backed
            // failure — never as a dead-end button.
            if app.registrar_backed
                && matches!(registration_tone(&app.state.registration), Tone::Bad)
                && ui.button("Retry").clicked()
            {
                reregister = true;
            }
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new(app.state.registration.label())
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
            ui.add_space(Style::SP_XS);
            status_dot(ui, tone_color(registration_tone(&app.state.registration)));
        });
    });
    ui.add_space(Style::SP_XS);
    if reregister {
        app.send(Command::Reregister);
    }
}

/// Render the Voice surface's central content into the given `ui`.
///
/// Draws the transient error banner, then one of the three call-lifecycle faces —
/// the incoming-call card, the active-call card, or the dialer — reading `app`'s
/// existing [`VoiceState`] and dial buffer. The user's intents (Answer / Decline /
/// Hang up / Dial) flow straight to the worker through `app`'s command channel,
/// exactly as the standalone binary drives them.
///
/// This is the one body shared by the standalone binary's `CentralPanel` and the
/// embedded shell panel (E12-3b), so the surface renders identically whether it
/// owns a window or is a panel inside the one shell — the EMBED model of E12
/// "Quasar" §5 (surfaces are panels in the shell, not separate clients). It draws
/// only through the shared [`Style`] and reuses `app`'s state (no parallel state).
/// The registration header and the per-frame worker-update drain stay with the
/// standalone app's chrome ([`VoiceApp`]) — the shell owns those.
pub fn voice_panel(ui: &mut egui::Ui, app: &mut VoiceApp) {
    let mut cmds = Vec::new();
    if let Some(error) = &app.state.error {
        ui.add_space(Style::SP_XS);
        ui.colored_label(Style::DANGER, error.as_str());
        ui.add_space(Style::SP_XS);
    }
    ui.add_space(Style::SP_S);
    if app.state.ringing_in() {
        incoming_card(ui, &app.state, &mut cmds);
    } else if app.state.show_dialer() {
        dialer(ui, &app.state, &mut app.dial, &mut cmds);
    } else {
        active_card(ui, &app.state, &mut cmds);
    }
    for cmd in cmds {
        app.send(cmd);
    }
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

#[cfg(test)]
mod tests {
    use super::{header, voice_panel};
    use crate::model::{Command, Update, VoiceState};
    use crate::VoiceApp;
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;
    use mde_voice_hud::sip::{CallState, RegistrationState};
    use std::sync::mpsc;

    /// Build a `VoiceApp` around a given `state` with a dead command channel and no
    /// worker — the embedded case a shell would drive, minus the SIP agent. Neither
    /// `voice_panel` nor `header` needs a live worker: `send` on a hung-up channel is
    /// a silent no-op, and the update channel is never read here.
    fn app_with(state: VoiceState, identity: &str, registrar_backed: bool) -> VoiceApp {
        let (commands, _cmd_rx) = mpsc::channel::<Command>();
        let (_upd_tx, updates) = mpsc::channel::<Update>();
        VoiceApp {
            state,
            dial: String::new(),
            commands,
            updates,
            identity: identity.to_string(),
            registrar_backed,
        }
    }

    /// Drive one headless egui frame that shows the header + `voice_panel`, then
    /// tessellate the result on the CPU so any paint-path fault (bad shape/text/
    /// geometry) surfaces as a test failure. This is the same `Context::run` →
    /// `tessellate` path the DRM runner drives, minus the GPU — no window, no wgpu,
    /// no socket, no sound device — so the embeddable panel is proven runtime-
    /// reachable in `cargo test`.
    fn render(app: &mut VoiceApp) {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 360.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::TopBottomPanel::top("voice-header").show(ctx, |ui| header(ui, app));
            egui::CentralPanel::default().show(ctx, |ui| voice_panel(ui, app));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "frame produced no draw primitives");
    }

    #[test]
    fn idle_dialer_renders_without_a_worker() {
        // The state an unconfigured embed opens to: an idle call ⇒ the dialer, and a
        // registrar-less P2P identity ⇒ no Retry in the header. Rendered end-to-end,
        // no worker spawned — the path a fresh shell panel would open to.
        render(&mut app_with(
            VoiceState::new(),
            "this node · P2P overlay",
            false,
        ));
    }

    #[test]
    fn registration_failure_shows_retry_and_error_banner() {
        // A registrar-backed REGISTER failure paints the header's working Retry, and
        // a transient media error paints the banner above the dialer — both honest,
        // neither swallowed (§7).
        let mut state = VoiceState::new();
        state.registration = RegistrationState::Failed("timeout".to_string());
        state.error = Some("no audio device".to_string());
        render(&mut app_with(state, "alice@sip.example.com", true));
    }

    #[test]
    fn call_lifecycle_faces_render() {
        // A ringing inbound call ⇒ the Answer/Decline card.
        let mut incoming = VoiceState::new();
        incoming.registration = RegistrationState::Registered {
            server: "sip.example.com:5060".to_string(),
            expires: 3600,
        };
        incoming.call = CallState::Incoming {
            from: "Bob".to_string(),
        };
        render(&mut app_with(incoming, "alice@sip.example.com", true));

        // A connected call ⇒ the status label + Hang up card.
        let mut active = VoiceState::new();
        active.call = CallState::InCall {
            peer: "pine".to_string(),
        };
        render(&mut app_with(active, "alice@sip.example.com", true));

        // A failed attempt drops back to the dialer, carrying its honest hint.
        let mut failed = VoiceState::new();
        failed.call = CallState::Failed("1009: busy".to_string());
        render(&mut app_with(failed, "alice@sip.example.com", true));
    }
}
