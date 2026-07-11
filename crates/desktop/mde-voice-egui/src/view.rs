//! The egui rendering of the Voice surface (E12-11).
//!
//! Every widget reads the render-agnostic [`VoiceState`] and draws through the
//! shared [`Style`] — no raw colours or literal metrics (governance §4). The view
//! never mutates the model mid-render: a frame collects the user's intents as
//! [`Command`]s and the caller forwards them to the worker once the frame is
//! done. Status text reuses the SIP state machine's own `RegistrationState`/
//! `CallState` labels (§6 — no re-worded copy).

use std::f32::consts::TAU;

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::{Motion, Style};

use mde_voice_hud::sip::CallState;

use crate::model::{call_tone, dial_ready, Command, Tab, Tone, VoiceState};
use crate::VoiceApp;

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
/// The MENUBAR-ALL top bar ([`crate::voice_menubar`], carrying the account
/// identity + live registration in its status cluster) and the per-frame
/// worker-update drain frame this body — both host paths render the bar above it.
pub fn voice_panel(ui: &mut egui::Ui, app: &mut VoiceApp) {
    // A ringing inbound call takes precedence over tab browsing — Answer/Decline
    // is urgent and must surface whichever tab is open, so the dialer stays
    // reachable while the fleet board is up.
    if app.state.ringing_in() {
        let mut cmds = Vec::new();
        ui.add_space(Style::SP_S);
        incoming_card(ui, &app.state, &mut cmds);
        for cmd in cmds {
            app.send(cmd);
        }
        return;
    }

    // The section toggle: the local dialer, or the VOIP-GW-5 fleet board. The
    // dialer keeps working; the fleet board is an added tab (design lock 5/16).
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.selectable_value(&mut app.tab, Tab::Dialer, "Dialer");
        ui.add_space(Style::SP_XS);
        ui.selectable_value(&mut app.tab, Tab::Fleet, "Fleet");
    });
    ui.add_space(Style::SP_XS);
    ui.separator();

    match app.tab {
        Tab::Dialer => dialer_tab(ui, app),
        Tab::Fleet => {
            app.fleet.poll(ui.ctx());
            app.fleet.show(ui);
        }
    }
}

/// The local dialer face: the transient error banner, then the dialer or the
/// active-call card. Its intents flow to the SIP worker through `app`.
fn dialer_tab(ui: &mut egui::Ui, app: &mut VoiceApp) {
    let mut cmds = Vec::new();
    if let Some(error) = &app.state.error {
        ui.add_space(Style::SP_XS);
        ui.colored_label(Style::DANGER, error.as_str());
        ui.add_space(Style::SP_XS);
    }
    ui.add_space(Style::SP_S);
    if app.state.show_dialer() {
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
        // The breathing ringing emblem — a live pulse on the shared Motion cadence
        // so a ringing call reads as urgent, not a frozen card (§4 micro-interaction).
        ringing_emblem(ui);
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new("Incoming call")
                .size(Style::HEADING)
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_XS);
        if let CallState::Incoming { from } = &state.call {
            // The caller identity is data — mono, one rung up, so it reads as the
            // card's key fact rather than a caption (mono-first, lock #3).
            ui.label(
                RichText::new(from)
                    .monospace()
                    .size(Style::TITLE)
                    .color(Style::ACCENT),
            );
        }
        ui.add_space(Style::SP_L);
        ui.horizontal(|ui| {
            let answer = egui::Button::new(RichText::new("Answer").color(Style::BG).strong())
                .fill(Style::OK)
                .min_size(CALL_ACTION_MIN);
            if ui.add(answer).clicked() {
                cmds.push(Command::Answer);
            }
            ui.add_space(Style::SP_S);
            let decline = egui::Button::new("Decline").min_size(CALL_ACTION_MIN);
            if ui.add(decline).clicked() {
                cmds.push(Command::Decline);
            }
        });
    });
}

/// An active (dialing / connected) call: its shipped status label + Hang up.
fn active_card(ui: &mut egui::Ui, state: &VoiceState, cmds: &mut Vec<Command>) {
    ui.vertical_centered(|ui| {
        ui.add_space(Style::SP_L);
        // The live call state is a status metric — mono, so it reads as a readout
        // rather than prose (mono-first, lock #3).
        ui.label(
            RichText::new(state.call.label())
                .monospace()
                .size(Style::HEADING)
                .color(tone_color(call_tone(&state.call))),
        );
        ui.add_space(Style::SP_M);
        let hang = egui::Button::new(RichText::new("Hang up").color(Style::BG).strong())
            .fill(Style::DANGER)
            .min_size(CALL_ACTION_MIN);
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
                // The dialed target is data (a number / peer id) — mono, so digits
                // and ids line up and read as an entry field (mono-first, lock #3).
                .font(egui::TextStyle::Monospace)
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

/// The comfortable minimum footprint of a primary call-lifecycle action button
/// (Answer / Decline / Hang up) — a wide, ≥`SP_XL`-tall target on the spacing
/// grid so the urgent verbs are easy to hit. egui floors a button's height at the
/// density's `interact_size` *before* applying this min, so it only ever grows a
/// mouse target and never shrinks a larger touch one (a11y hit-target axis).
const CALL_ACTION_MIN: egui::Vec2 = egui::Vec2::new(Style::SP_XL * 3.0, Style::SP_XL);

/// One full ringing-pulse heartbeat, in seconds, derived from the shared
/// [`Motion`] table so the ring cadence stays on the harness timing scale rather
/// than a bespoke literal (§4). A ringing call is urgent-but-calm — a slow
/// breathing ripple, deliberately *not* the D/F-grade [`Motion::blink`] alarm.
const RING_PULSE_SECS: f64 = Motion::SLOW as f64 * 3.0;

/// The steady radius of the ringing emblem's accent core dot (spacing-grid-shaped).
const RING_CORE_R: f32 = Style::SP_S;
/// How far the emblem's ripple travels outward from the core at a pulse's peak.
const RING_RIPPLE_TRAVEL: f32 = Style::SP_M;

/// A breathing **ringing emblem** — an accent core dot with an outward ripple that
/// expands and fades on the shared [`Motion`] cadence — so a ringing call *reads*
/// as live rather than a static card. The ripple phase comes from the egui clock
/// through [`RING_PULSE_SECS`] (derived from [`Motion::SLOW`], no literal), and the
/// frame repaints only while the emblem is on screen (i.e. only while ringing), so
/// an idle DRM seat never spins on it (§4 / CRAFT repaint hygiene).
fn ringing_emblem(ui: &mut egui::Ui) {
    // A smooth 0→1→0 breath (cosine ease) — the same shape mde-mesh-view's leader
    // heartbeat and mde-panel's pip pulse ride, so the platform breathes one way.
    let phase = (ui.input(|i| i.time) / RING_PULSE_SECS).fract() as f32;
    let breath = 0.5 - 0.5 * (phase * TAU).cos();

    let diameter = (RING_CORE_R + RING_RIPPLE_TRAVEL) * 2.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(diameter, diameter), egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();

    // The ripple grows from the core outward and fades as it goes, so it reads as a
    // wave leaving the dot — a 1 px stroke (geometry discipline), alpha on the
    // inverse breath (bright at the core, gone at the edge).
    let ripple_r = RING_CORE_R + RING_RIPPLE_TRAVEL * breath;
    let ripple = Style::ACCENT.gamma_multiply(1.0 - breath);
    painter.circle_stroke(center, ripple_r, egui::Stroke::new(1.0, ripple));
    // The steady accent core.
    painter.circle_filled(center, RING_CORE_R, Style::ACCENT);

    // Keep the breath alive — but only while the ringing card is on screen.
    ui.ctx().request_repaint();
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
    use super::voice_panel;
    use crate::menubar::{voice_menubar, MenuBarState};
    use crate::model::{Command, Update, VoiceState};
    use crate::VoiceApp;
    use mde_egui::egui::{self, pos2, vec2, Rect};
    use mde_egui::Style;
    use mde_voice_hud::sip::{CallState, RegistrationState};
    use std::sync::mpsc;

    /// Build a `VoiceApp` around a given `state` with a dead command channel and no
    /// worker — the embedded case a shell would drive, minus the SIP agent. Neither
    /// `voice_panel` nor the menu bar needs a live worker: `send` on a hung-up
    /// channel is a silent no-op, and the update channel is never read here.
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
            tab: crate::model::Tab::default(),
            fleet: crate::fleet::FleetState::new(),
            menu: MenuBarState::default(),
        }
    }

    /// Drive one headless egui frame that shows the shared menu bar + `voice_panel`,
    /// then tessellate the result on the CPU so any paint-path fault (bad shape/
    /// text/geometry) surfaces as a test failure. This is the same `Context::run` →
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
            egui::TopBottomPanel::top("voice-menubar").show(ctx, |ui| voice_menubar(ui, app));
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
