//! `mde-voice-hud` — the libcosmic + wlr-layer-shell voice HUD.
//!
//! Opens a 420 × 720 layer-shell Overlay surface anchored bottom-right with
//! 16 px right + 56 px bottom clearance per
//! `docs/design/v6.0-pjsip-presence-and-hud.md` §2.5 (CUT-2: now on the
//! libcosmic fork's native layer-shell, not iced_layershell). Renders the
//! topbar (account dot + peer name + live registration status), a dialer
//! display + resolved-chip strip, and a 3 × 4 keypad.
//!
//! **Shipped state** (VOIP-27/28/29, not a scaffold):
//! - The topbar renders the *live* `RegistrationState` (`registration.label()`)
//!   driven by a persistent SIP agent thread (`AGENT_EVENTS`/`AGENT_CMD`), which
//!   REGISTERs on launch and surfaces inbound INVITE/BYE (VOIP-28).
//! - Outbound calls are real: `PlaceCall`/`DialRequested` → INVITE, plus
//!   `Answer`/`Decline`/`HangUp` (VOIP-29). PD-5 `action/voice/dial` lands a
//!   call from the Peers panel.
//! - The resolved chip classifies mesh / PSTN / partial / invalid via
//!   `resolve_target` against the live roster.
//!
//! `--agent` runs the headless SIP agent (no window) for login autostart.
//! The one honest gap is the same NAT-class detail OSS Nebula doesn't expose.

#![forbid(unsafe_code)]

use cosmic::iced::widget::{button, column, container, row, text, text_input};
use cosmic::iced::{window, Color, Length, Padding, Task};
// CUT-2: cosmic::Element/Theme bake in cosmic::Theme (the cosmic_compat
// .sty()/.colr() shims thread through it); the layer surface is created with
// the fork's native wlr-layer-shell commands (iced_layershell is dropped).
use cosmic::iced::platform_specific::runtime::wayland::layer_surface::SctkLayerSurfaceSettings;
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    get_layer_surface, Anchor, KeyboardInteractivity, Layer,
};
use cosmic::{Element, Theme};

use crate::cosmic_compat::prelude::*;

mod cosmic_compat;
mod media;
mod resolve;
mod roster;
mod sip;
mod theme;

use resolve::{resolve_target, Resolved};
use roster::{Peer, RosterLoad};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mde_theme::animation::{Animator, Transition};
use mde_theme::feedback::{ControlFeedback, FeedbackParams};
use mde_theme::motion::{Easing, Motion, PANEL_MOUNT_TRANSLATE_Y_PX};

// ── MOTION-FEEDBACK-1 / MOTION-TRANS — the HUD's shared-motion wiring ─────────
//
// The voice HUD applies the shell motion vocabulary via `mde_theme::{animation,
// feedback}`: a fade+slide-in when the overlay appears, a smooth crossfade when
// the call mode/state changes, and hover-lift + press-depress feedback on the
// interactive controls (the keypad keys + the call-action pills). All of it
// flows through ONE [`Animator`] advanced by a single tick subscription that
// only runs while a tween is in flight (`Animator::is_idle`) — a voice HUD must
// never burn CPU at rest. Reduce-motion is honored by routing every tween's
// duration through the helpers' reduce-motion cap and dropping movement (the
// state still changes, it just doesn't move).

/// Tick cadence while a tween is live (~60 fps). The subscription that emits it
/// is only created while [`Animator::is_idle`] is false, so there are zero idle
/// wakeups (MOTION-PERF-1).
const ANIM_TICK: Duration = Duration::from_millis(16);

/// [`Animator`] key for the HUD's appear transition (fade + slide-in).
const ANIM_APPEAR: &str = "appear";
/// [`Animator`] key for the call-bar's state-change crossfade.
const ANIM_CALLSTATE: &str = "callstate";

/// Build a per-control hover-lift [`Animator`] key (one tween per control id).
fn hover_key(id: &str) -> String {
    format!("hover/{id}")
}

/// VOIP-28 slice 4 — channels bridging the persistent SIP agent thread (inbound
/// calls + registration) to the iced UI. Statics so the agent-event
/// subscription's `'static` closure can drain them; the agent is spawned once
/// at boot. `AGENT_EVENTS` holds the agent→UI receiver (taken by the
/// subscription), `AGENT_CMD` the UI→agent sender (used for answer/decline).
static AGENT_EVENTS: Mutex<Option<mpsc::Receiver<sip::AgentEvent>>> = Mutex::new(None);
static AGENT_CMD: Mutex<Option<mpsc::Sender<sip::AgentCommand>>> = Mutex::new(None);

/// Send a command to the agent thread (no-op if no agent is running).
fn agent_send(cmd: sip::AgentCommand) {
    if let Ok(guard) = AGENT_CMD.lock() {
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(cmd);
        }
    }
}

/// VOIP-27 §2.5 size lock — cozy density default.
const WIDTH: u32 = 420;
/// VOIP-27 §2.5 size lock — cozy density default.
const HEIGHT: u32 = 720;
/// VOIP-27 §2.5 margin lock: right=16 px, bottom=56 px (over
/// dock clearance). Applied via the fork's `IcedMargin`
/// (top/right/bottom/left) in `boot_surface` (CUT-2).
const MARGIN_RIGHT: i32 = 16;
const MARGIN_BOTTOM: i32 = 56;

/// The local peer's display name — the host's own name (`/etc/hostname`, else
/// `$HOSTNAME`, else "MDE"). Real single-node data, not a fabricated label;
/// VOIP-28 swaps in the live mded read.
fn local_peer_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "MDE".to_string())
}

/// Account-dot initials derived from the peer name: up to two leading
/// alphanumerics, uppercased (or "—" when the name has none).
fn account_initials(name: &str) -> String {
    let inits: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(2)
        .collect();
    if inits.is_empty() {
        "—".to_string()
    } else {
        inits.to_uppercase()
    }
}

/// HUD application messages. CUT-2: the surface is a plain
/// `cosmic::iced::daemon` over the fork's native layer-shell, so
/// the old `#[to_layer_message]` macro (and its injected
/// size/anchor/margin action variants, which the HUD never used)
/// are gone — only these hand-written variants remain.
#[derive(Debug, Clone)]
pub enum Message {
    /// Operator typed into the display text-input or clicked a
    /// keypad button. Carries the full new contents (text-input
    /// emits this on every char; keypad clicks build the new
    /// string in the update handler).
    DialerInputChanged(String),
    /// Keypad button pressed. The handler appends the char to
    /// the current display contents.
    KeypadPressed(char),
    /// Operator clicked the backspace key (or pressed Backspace
    /// on the hardware keyboard). Removes the last char.
    Backspace,
    /// Operator pressed Escape. VOIP-27 ships idle-state only;
    /// Escape exits the process (active-call → minimize-to-
    /// dock-pill ships with VOIP-29). The handler invokes
    /// `Task::done(Message::Exit)` which routes through the
    /// runtime to a graceful exit.
    Escape,
    /// Sentinel that the runtime uses to flag exit. Triggers
    /// `std::process::exit(0)` — a layer-shell daemon has no
    /// last-window-closed shutdown signal.
    Exit,
    /// An event from the persistent SIP agent (registration / inbound call).
    Agent(sip::AgentEvent),
    /// Operator answered the ringing inbound call.
    Answer,
    /// Operator declined the ringing inbound call.
    Decline,
    /// Operator pressed Call — place an outbound INVITE to the dialed number.
    PlaceCall,
    /// Result of the outbound INVITE: the established dialog or a failure.
    CallConnected(Result<sip::CallSession, String>),
    /// Operator pressed Hang up — tear the active call down with a BYE.
    HangUp,
    /// The BYE task finished — the call is fully torn down (no-op confirm).
    CallEnded,
    /// PD-5 — a dial request arrived over the Bus (`action/voice/dial`),
    /// carrying the target the Peers panel asked to call (a mesh hostname,
    /// an extension, or a SIP URI). Resolved against the roster then placed.
    DialRequested(String),
    /// MOTION-* — one animation frame: advance + GC the [`Animator`]. Emitted by
    /// the tick subscription only while a tween is in flight.
    AnimTick,
    /// MOTION-FEEDBACK-1 — the pointer entered an interactive control (its id),
    /// arming the hover-lift tween.
    HoverEnter(String),
    /// MOTION-FEEDBACK-1 — the pointer left a control, settling its hover-lift.
    HoverExit(String),
    /// MOTION-FEEDBACK-1 — the pointer pressed *down* on a control. The depress
    /// fires on this down edge (no input delay), so it carries no timestamp.
    ControlPressed(String),
    /// MOTION-FEEDBACK-1 — the pointer released a control (depress lifts).
    ControlReleased(String),
}

/// Top-level HUD state.
pub struct VoiceHud {
    /// Current contents of the dialer display field.
    pub dialer_input: String,
    /// Loaded mesh roster — drives the `Resolved::Mesh` lookup
    /// in the resolved-chip rendering.
    pub roster: Vec<Peer>,
    /// Live SIP registration state (VOIP-28) — drives the topbar status
    /// line + presence pip. `NoAccount` until `account.toml` exists.
    pub registration: sip::RegistrationState,
    /// The loaded SIP account (used to place calls); `None` with no config.
    pub account: Option<sip::SipAccount>,
    /// Live outbound-call state (VOIP-28 slice 2) — drives the call status
    /// row + the Call/Hang-up button.
    pub call: sip::CallState,
    /// The established dialog while a call is up (for the BYE on hang-up).
    pub session: Option<sip::CallSession>,
    /// The running RTP/G.711 media engine while a call is up (slice 3).
    pub media: Option<media::MediaSession>,
    /// MOTION-* — the single shared animator driving every HUD tween (appear,
    /// call-state crossfade, per-control hover lift). Tick-driven only while a
    /// tween is in flight; idle otherwise (no idle CPU).
    pub anim: Animator,
    /// MOTION-* — the user's reduce-motion preference. When set, motion collapses
    /// to a fast crossfade / no movement (the state change is kept).
    pub reduce_motion: bool,
    /// MOTION-FEEDBACK-1 — the control id currently pressed *down*, if any. The
    /// depress is geometric-on-down (no warm-up tween), so it's tracked as a flag
    /// rather than a tween in [`Self::anim`].
    pub pressed: Option<String>,
    /// MOTION-FEEDBACK-1 — the control id currently hovered, if any (so a stale
    /// `HoverExit` after a fast re-enter is ignored).
    pub hovered: Option<String>,
    /// MOTION-TRANS — the discriminant of the last-rendered call state, so the
    /// update handler can start the state-change crossfade only when the mode
    /// actually changes (not on every unrelated message).
    pub call_kind: CallKind,
}

/// MOTION-TRANS — a coarse discriminant of [`sip::CallState`].
///
/// Used only to detect a *mode* change (idle ↔ calling ↔ in-call ↔ incoming ↔
/// ended/failed) so the call-bar crossfade fires once per real transition. Pure
/// mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    /// Idle / no call.
    Idle,
    /// An outbound call is being placed.
    Calling,
    /// A call is up.
    InCall,
    /// An inbound call is ringing.
    Incoming,
    /// The call ended or failed.
    Ended,
}

impl CallKind {
    /// Map a live [`sip::CallState`] to its coarse mode.
    #[must_use]
    pub const fn of(state: &sip::CallState) -> Self {
        match state {
            sip::CallState::Idle => Self::Idle,
            sip::CallState::Calling { .. } | sip::CallState::Ringing { .. } => Self::Calling,
            sip::CallState::InCall { .. } => Self::InCall,
            sip::CallState::Incoming { .. } => Self::Incoming,
            sip::CallState::Ended | sip::CallState::Failed(_) => Self::Ended,
        }
    }
}

impl VoiceHud {
    /// MOTION-* — (re)start a tween under `id` from now, with `preset`'s
    /// duration+easing resolved against the user's reduce-motion preference. GC
    /// first so a re-armed tween replaces a settled one cleanly. Centralizes the
    /// reduce-motion routing so no call site re-implements the contract.
    fn start_anim(&mut self, id: impl Into<String>, preset: Motion) {
        let now = Instant::now();
        self.anim.gc(now);
        self.anim.start(id, now, preset, self.reduce_motion);
    }

    /// MOTION-FEEDBACK-1 — arm the hover-lift tween for control `id` (used on both
    /// enter and leave; the geometry sampler reads `self.hovered` for direction).
    fn start_hover(&mut self, id: &str) {
        self.start_anim(hover_key(id), Motion::hover());
    }

    /// MOTION-TRANS — start the call-bar crossfade iff the call MODE changed since
    /// the last sync. Called once at the end of every `update`. Under reduce-motion
    /// the crossfade collapses to the ≤80 ms cap (state kept, motion minimal).
    fn sync_call_state(&mut self) {
        let kind = CallKind::of(&self.call);
        if kind != self.call_kind {
            self.call_kind = kind;
            self.start_anim(ANIM_CALLSTATE, Motion::tooltip_fade());
        }
    }

    /// MOTION-FEEDBACK-1 — the geometric feedback (`translate_y` lift + press
    /// `scale`) for control `id` at `now`, built from the shared
    /// [`ControlFeedback`] helper so the lift/press math + reduce-motion contract
    /// live in one place. The hover tween's *progress* comes from the animator;
    /// the press is geometric-on-down (`self.pressed`).
    fn control_feedback(&self, id: &str, now: Instant) -> FeedbackParams {
        // The animator already encodes hover progress + reduce-motion; derive the
        // current lift offset from it, then fold the press depress in via the
        // shared helper so the press-scale formula isn't duplicated here.
        let hovered = self.hovered.as_deref() == Some(id);
        let pressed = self.pressed.as_deref() == Some(id);
        // `hover_since` is when this control's hover tween started; the animator
        // holds that tween (keyed by the hover id). Build a ControlFeedback whose
        // hover timestamp is back-derived from the animator's eased value so the
        // lift matches the shared subscription clock exactly.
        let lift = self.hover_lift(id, now, hovered);
        let fb = ControlFeedback::new().pressed(pressed);
        let press = fb.params(now, self.reduce_motion);
        FeedbackParams {
            translate_y: lift,
            scale: press.scale,
        }
    }

    /// MOTION-FEEDBACK-1 — the hover-lift offset (px, negative = up) for control
    /// `id` at `now`, sampled from the shared animator. Rises toward
    /// `-HOVER_LIFT_PX` while hovered, settles back to 0 on leave; the animator's
    /// reduce-motion-resolved tween caps the motion (and the
    /// [`Transition::Lift`] mapping keeps the lift formula single-sourced).
    fn hover_lift(&self, id: &str, now: Instant, hovered: bool) -> f32 {
        if self.reduce_motion {
            // Reduce-motion drops the movement; the hover state is conveyed by the
            // control's color token, not motion.
            return 0.0;
        }
        let key = hover_key(id);
        let t = self.anim.value(&key, now, Easing::EaseOut);
        let full = Transition::Lift(mde_theme::feedback::HOVER_LIFT_PX)
            .params(1.0)
            .translate_y;
        // The tween animates the *transition* (0→1). On enter the lift runs 0→full
        // as `t` grows; on leave it runs full→0 (the same tween, reversed). When
        // no tween is registered the animator returns 1.0, i.e. fully settled.
        if hovered {
            mde_theme::animation::lerp_f32(0.0, full, t)
        } else {
            mde_theme::animation::lerp_f32(full, 0.0, t)
        }
    }

    /// MOTION-* — the HUD-appear render params (alpha + slide-in offset) at `now`,
    /// sampled from the shared animator's `appear` tween. Fades 0→1 while rising
    /// from [`PANEL_MOUNT_TRANSLATE_Y_PX`] below; under reduce-motion the slide is
    /// dropped (pure fast crossfade). `1.0` alpha / `0.0` offset once settled.
    fn appear_params(&self, now: Instant) -> (f32, f32) {
        let t = self.anim.value(ANIM_APPEAR, now, Easing::EaseOut);
        if self.reduce_motion {
            return (t, 0.0);
        }
        let params = Transition::SlideUp(PANEL_MOUNT_TRANSLATE_Y_PX).params(t);
        (params.alpha, params.translate_y)
    }

    /// MOTION-TRANS — the call-bar crossfade alpha at `now` (0→1 as the new mode
    /// fades in). `1.0` once settled, so the bar is fully opaque at rest.
    fn call_state_alpha(&self, now: Instant) -> f32 {
        self.anim.value(ANIM_CALLSTATE, now, Easing::EaseOut)
    }
}

/// MOTION-* — blend `color` toward the surface background by `alpha` (0 = fully
/// background, 1 = `color`). iced 0.13 has no opacity widget, so a fade is
/// rendered by interpolating the visible color toward the surface instead — the
/// MOTION-INFRA-2 color-alpha approach. Pure.
fn fade_color(color: Color, alpha: f32) -> Color {
    let a = alpha.clamp(0.0, 1.0);
    Color {
        r: mde_theme::animation::lerp_f32(theme::SURF.r, color.r, a),
        g: mde_theme::animation::lerp_f32(theme::SURF.g, color.g, a),
        b: mde_theme::animation::lerp_f32(theme::SURF.b, color.b, a),
        a: color.a,
    }
}

// ── cosmic::iced daemon builder functions (CUT-2) ────────────────────────────

fn namespace(_state: &VoiceHud, _id: window::Id) -> String {
    "mde-voice-hud".to_string()
}

/// CUT-2 — the HUD's Carbon Gray-100 dark theme for the cosmic daemon.
fn theme(_state: &VoiceHud, _id: window::Id) -> Theme {
    cosmic::Theme::dark()
}

/// Boot the layer-shell Overlay surface: bottom-right, fixed size, OnDemand
/// keyboard (focus on click), Overlay layer (above normal windows, below the
/// lock screen) — the §2.5 surface lock, now via the fork's native commands.
fn boot_surface() -> Task<Message> {
    get_layer_surface(SctkLayerSurfaceSettings {
        id: window::Id::unique(),
        namespace: "mde-voice-hud".to_string(),
        size: Some((Some(WIDTH), Some(HEIGHT))),
        exclusive_zone: 0,
        // (top, right, bottom, left) — anchored bottom-right with margins.
        margin: cosmic::iced::platform_specific::runtime::wayland::layer_surface::IcedMargin {
            top: 0,
            right: MARGIN_RIGHT,
            bottom: MARGIN_BOTTOM,
            left: 0,
        },
        anchor: Anchor::BOTTOM.union(Anchor::RIGHT),
        layer: Layer::Overlay,
        keyboard_interactivity: KeyboardInteractivity::OnDemand,
        ..Default::default()
    })
}

/// iced update — drives the HUD state machine (calls, registration, roster).
pub fn update(state: &mut VoiceHud, message: Message) -> Task<Message> {
    match message {
        Message::DialerInputChanged(value) => {
            state.dialer_input = filter_dialer_chars(&value);
        }
        Message::KeypadPressed(c) => {
            if is_dialer_char(c) {
                state.dialer_input.push(c);
            }
        }
        Message::Backspace => {
            state.dialer_input.pop();
        }
        Message::Escape => {
            return Task::done(Message::Exit);
        }
        Message::Exit => {
            std::process::exit(0);
        }
        Message::Agent(event) => match event {
            sip::AgentEvent::Registration(reg) => {
                tracing::info!(state = ?reg, "voice-hud: registration result");
                state.registration = reg;
            }
            sip::AgentEvent::Incoming { from, .. } => {
                tracing::info!(%from, "voice-hud: incoming call");
                state.call = sip::CallState::Incoming { from: from.clone() };
                // Document the call as a desktop notification via the FDO
                // Notifications path (Cosmic's notification daemon) — the
                // system notification IS the call record (sweep-3 I1; no
                // stored recents list). Best-effort, never fatal.
                let _ = std::process::Command::new("notify-send")
                    .args(["-a", "MCNF Voice", "Incoming call", &from])
                    .spawn();
            }
            sip::AgentEvent::Established => {
                let peer = match &state.call {
                    sip::CallState::Incoming { from } => from.clone(),
                    _ => String::new(),
                };
                state.call = sip::CallState::InCall { peer };
            }
            sip::AgentEvent::RemoteHangup => {
                state.call = sip::CallState::Ended;
            }
        },
        Message::Answer => {
            agent_send(sip::AgentCommand::Answer);
        }
        Message::Decline => {
            agent_send(sip::AgentCommand::Decline);
            state.call = sip::CallState::Idle;
        }
        Message::DialRequested(target) => {
            // VOIP-P2P — registrar-less model: a mesh peer is dialed DIRECTLY by
            // name over the overlay (PlaceCall routes a peer name to
            // `place_call_direct`), not resolved to an extension on a registrar.
            // A bare number / SIP URI is kept as-is for the registrar path.
            let t = target.trim();
            if t.is_empty() || state.call.is_active() {
                return Task::none();
            }
            state.dialer_input = t.to_string();
            return Task::done(Message::PlaceCall);
        }
        Message::PlaceCall => {
            let dialed = state.dialer_input.trim().to_string();
            if dialed.is_empty() || state.call.is_active() {
                return Task::none();
            }
            // VOIP-P2P — a mesh-peer name dials DIRECTLY over the overlay
            // (registrar-less); a bare number/extension routes via the registrar
            // account. Direct calls work even with no account.toml (a local
            // overlay identity is synthesized).
            if looks_like_peer(&dialed) {
                let acct = state
                    .account
                    .clone()
                    .unwrap_or_else(sip::SipAccount::local_identity);
                let peer_host = peer_host_for(&dialed);
                state.call = sip::CallState::Calling {
                    peer: dialed.clone(),
                };
                let peer = dialed.clone();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            sip::place_call_direct(
                                &acct,
                                "",
                                &peer_host,
                                5060,
                                std::time::Duration::from_secs(30),
                            )
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("call task failed: {e}")))
                    },
                    move |res| Message::CallConnected(res.map_err(|e| format!("{peer}: {e}"))),
                );
            }
            match state.account.clone() {
                Some(acct) => {
                    state.call = sip::CallState::Calling {
                        peer: dialed.clone(),
                    };
                    let peer = dialed.clone();
                    return Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                sip::place_call(&acct, &dialed, std::time::Duration::from_secs(30))
                            })
                            .await
                            .unwrap_or_else(|e| Err(format!("call task failed: {e}")))
                        },
                        move |res| Message::CallConnected(res.map_err(|e| format!("{peer}: {e}"))),
                    );
                }
                None => {
                    state.call = sip::CallState::Failed(
                        "no registrar account — dial a peer by name for a direct call".into(),
                    );
                }
            }
        }
        Message::CallConnected(Ok(session)) => {
            tracing::info!(
                rtp = session.rtp_port,
                "voice-hud: call connected; starting media"
            );
            // Start the RTP/G.711 media path over the negotiated endpoint. A
            // failure (no audio devices) leaves the call up but silent — honest
            // degradation, not a panic.
            match media::start_media(session.rtp_port, &session.remote) {
                Ok(m) => state.media = Some(m),
                Err(e) => tracing::warn!(%e, "voice-hud: media start failed (call up, no audio)"),
            }
            state.call = sip::CallState::InCall {
                peer: state.dialer_input.trim().to_string(),
            };
            state.session = Some(session);
        }
        Message::CallConnected(Err(why)) => {
            tracing::info!(why = %why, "voice-hud: call failed");
            state.call = sip::CallState::Failed(why);
            state.session = None;
        }
        Message::HangUp => {
            state.call = sip::CallState::Ended;
            if let Some(m) = state.media.take() {
                m.stop();
            }
            // An inbound call's media/dialog live in the agent thread.
            agent_send(sip::AgentCommand::HangUp);
            if let Some(session) = state.session.take() {
                return Task::perform(
                    async move {
                        let _ = tokio::task::spawn_blocking(move || sip::hang_up(&session)).await;
                    },
                    |()| Message::CallEnded,
                );
            }
        }
        Message::CallEnded => {
            // BYE delivered; state is already `Ended`.
        }
        Message::AnimTick => {
            // MOTION-* — one frame: drop every settled tween. When the last one
            // completes the animator goes idle and the tick subscription stops
            // (no idle wakeups). No state is read here; `view` samples the
            // animator at `Instant::now()`.
            state.anim.gc(Instant::now());
        }
        Message::HoverEnter(id) => {
            // MOTION-FEEDBACK-1 — lift this control; arm its hover tween.
            if state.hovered.as_deref() != Some(&id) {
                state.hovered = Some(id.clone());
                state.start_hover(&id);
            }
        }
        Message::HoverExit(id) => {
            // Settle back only if it's still the hovered control (ignore a stale
            // exit after a fast re-enter).
            if state.hovered.as_deref() == Some(&id) {
                state.hovered = None;
                state.start_hover(&id);
            }
        }
        Message::ControlPressed(id) => {
            // The depress is geometric-on-down (no warm-up tween): just record it.
            state.pressed = Some(id);
        }
        Message::ControlReleased(id) => {
            if state.pressed.as_deref() == Some(&id) {
                state.pressed = None;
            }
        }
    }
    // MOTION-TRANS — after any message, if the call MODE changed, crossfade the
    // call bar so the mode switch reads as one motion (not a hard cut).
    state.sync_call_state();
    Task::none()
}

/// iced view — renders the HUD overlay surface.
pub fn view(state: &VoiceHud, _id: window::Id) -> Element<'_, Message> {
    let now = Instant::now();
    // MOTION-* — appear transition: the whole HUD fades + rises from a few px
    // below on first paint (Carbon panel-mount). The fade is rendered by blending
    // every surface toward the background (no opacity widget in iced 0.13); the
    // slide is rendered as decaying top padding so the column's height stays put
    // (no reflow). Both settle to rest, after which the animator is idle.
    let (alpha, slide) = state.appear_params(now);
    let column = column![
        build_topbar(state, alpha),
        build_display(state),
        build_keypad(state),
        build_call_bar(state, now),
    ]
    .spacing(12);
    // The slide is a downward offset that decays to 0: reserve it as top padding,
    // shrinking the bottom by the same amount so total height is constant (no
    // neighbour reflow). The fade is applied per-element (the topbar text + the
    // call bar) — iced 0.13 has no opacity widget, so a fade is rendered by
    // blending the visible colors toward the surface instead.
    let top = (16.0 + slide).max(0.0);
    let bottom = (16.0 - slide).max(0.0);
    container(column)
        .padding(Padding {
            top,
            right: 16.0,
            bottom,
            left: 16.0,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .sty(|_: &Theme| cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(theme::SURF)),
            ..Default::default()
        })
        .into()
}

fn subscription(state: &VoiceHud) -> cosmic::iced::Subscription<Message> {
    let mut subs = vec![
        keyboard_subscription(),
        agent_subscription(),
        dial_subscription(),
    ];
    // MOTION-PERF-1 — only arm the ~60 fps animation tick while a tween is in
    // flight. When the animator is idle this subscription isn't created, so the
    // HUD has zero idle wakeups (a voice HUD must not burn CPU at rest).
    if !state.anim.is_idle(Instant::now()) {
        subs.push(
            cosmic::iced::time::every(ANIM_TICK).map(|_| Message::AnimTick),
        );
    }
    cosmic::iced::Subscription::batch(subs)
}

/// PD-5 — the Bus topic the Peers panel publishes a dial request on.
const DIAL_TOPIC: &str = "action/voice/dial";

/// PD-5 — subscribe to `action/voice/dial`; each new request becomes a
/// [`Message::DialRequested`]. Cursor-seeded so it only acts on requests
/// published after the HUD starts (an old request never re-dials).
fn dial_subscription() -> cosmic::iced::Subscription<Message> {
    use cosmic::iced::futures::SinkExt;
    use cosmic::iced::stream;
    cosmic::iced::Subscription::run(|| {
        stream::channel(
            8,
            |mut output: cosmic::iced::futures::channel::mpsc::Sender<Message>| async move {
                let mut cursor = dial_cursor_init().await;
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
                    let (targets, next) = dial_poll(cursor.clone()).await;
                    cursor = next;
                    for t in targets {
                        let _ = output.send(Message::DialRequested(t)).await;
                    }
                }
            },
        )
    })
}

/// Seed the cursor at the latest existing dial request.
async fn dial_cursor_init() -> Option<String> {
    tokio::task::spawn_blocking(|| {
        let dir = mde_bus::default_data_dir()?;
        let persist = mde_bus::persist::Persist::open(dir).ok()?;
        persist
            .list_since(DIAL_TOPIC, None)
            .ok()?
            .last()
            .map(|m| m.ulid.clone())
    })
    .await
    .ok()
    .flatten()
}

/// New dial targets (each request's `target` field) since `cursor`, plus
/// the advanced cursor. Bus unavailable → nothing, cursor unchanged.
async fn dial_poll(cursor: Option<String>) -> (Vec<String>, Option<String>) {
    tokio::task::spawn_blocking(move || {
        let Some(dir) = mde_bus::default_data_dir() else {
            return (Vec::new(), cursor);
        };
        let Ok(persist) = mde_bus::persist::Persist::open(dir) else {
            return (Vec::new(), cursor);
        };
        let msgs = persist
            .list_since(DIAL_TOPIC, cursor.as_deref())
            .unwrap_or_default();
        let next = msgs.last().map(|m| m.ulid.clone()).or(cursor);
        let targets = msgs
            .iter()
            .filter_map(|m| {
                let body = m.body.as_deref()?;
                let v: serde_json::Value = serde_json::from_str(body).ok()?;
                v.get("target").and_then(|t| t.as_str()).map(str::to_string)
            })
            .collect();
        (targets, next)
    })
    .await
    .unwrap_or((Vec::new(), None))
}

fn keyboard_subscription() -> cosmic::iced::Subscription<Message> {
    use cosmic::iced::event;
    use cosmic::iced::keyboard;
    event::listen_with(|event, status, _window| match event {
        cosmic::iced::Event::Keyboard(keyboard::Event::KeyPressed { key, .. })
            if status == event::Status::Ignored =>
        {
            use cosmic::iced::keyboard::{key::Named, Key};
            match key {
                Key::Named(Named::Escape) => Some(Message::Escape),
                Key::Named(Named::Backspace) => Some(Message::Backspace),
                Key::Character(s) => {
                    let c = s.chars().next()?;
                    if is_dialer_char(c) {
                        Some(Message::KeypadPressed(c))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        _ => None,
    })
}

/// Bridge the persistent SIP agent's event channel (set up at boot) into iced
/// `Message::Agent`s. Drains the std-mpsc receiver on a 50 ms poll inside the
/// subscription's async task (mirrors the workbench's `stream::channel` idiom).
fn agent_subscription() -> cosmic::iced::Subscription<Message> {
    use cosmic::iced::futures::SinkExt;
    use cosmic::iced::stream;
    cosmic::iced::Subscription::run(|| {
        stream::channel(
            64,
            |mut output: cosmic::iced::futures::channel::mpsc::Sender<Message>| async move {
                let rx = AGENT_EVENTS.lock().ok().and_then(|mut g| g.take());
                if let Some(rx) = rx {
                    loop {
                        while let Ok(ev) = rx.try_recv() {
                            let _ = output.send(Message::Agent(ev)).await;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    }
                }
                std::future::pending::<()>().await;
            },
        )
    })
}

/// Build the topbar — account dot + peer name + presence pip + registration
/// status. The peer name + initials are the real host identity; the presence
/// pip is offline and the status reads "Not registered" until VOIP-28 wires
/// the live PJSIP registrar state over the Bus (the registered view is the
/// SIP-server bench).
fn build_topbar(state: &VoiceHud, appear: f32) -> Element<'_, Message> {
    let peer_name = local_peer_name();
    let pip_color = fade_color(
        if state.registration.is_online() {
            theme::PRESENCE_AVAILABLE
        } else {
            theme::PRESENCE_OFFLINE
        },
        appear,
    );
    let registration_status = state.registration.label();
    let account_dot = container(
        text(account_initials(&peer_name))
            .size(13.0)
            .colr(fade_color(theme::ON_PRIMARY, appear)),
    )
    .sty(move |_: &Theme| cosmic::iced::widget::container::Style {
        background: Some(cosmic::iced::Background::Color(fade_color(theme::PRIMARY, appear))),
        border: cosmic::iced::Border {
            radius: cosmic::iced::border::Radius::from(16.0),
            ..Default::default()
        },
        ..Default::default()
    })
    .width(Length::Fixed(32.0))
    .height(Length::Fixed(32.0))
    .align_x(cosmic::iced::alignment::Horizontal::Center)
    .align_y(cosmic::iced::alignment::Vertical::Center);

    let presence_pip = container(cosmic::iced::widget::Space::new())
        .sty(move |_: &Theme| cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(pip_color)),
            border: cosmic::iced::Border {
                radius: cosmic::iced::border::Radius::from(4.0),
                ..Default::default()
            },
            ..Default::default()
        })
        .width(Length::Fixed(8.0))
        .height(Length::Fixed(8.0));

    let name_col = column![
        text(peer_name)
            .size(14.0)
            .colr(fade_color(theme::ON_SURF, appear)),
        row![
            presence_pip,
            cosmic::iced::widget::space().width(Length::Fixed(6.0)),
            text(registration_status)
                .size(11.0)
                .colr(fade_color(theme::ON_SURF_VAR, appear)),
        ]
        .align_y(cosmic::iced::Alignment::Center),
    ]
    .spacing(2);

    row![
        account_dot,
        cosmic::iced::widget::space().width(Length::Fixed(12.0)),
        name_col,
    ]
    .align_y(cosmic::iced::Alignment::Center)
    .into()
}

/// Build the display + resolved-chip strip. The text-input
/// receives keypad/keyboard input; the chip to its right
/// renders the `resolve_target` classification.
fn build_display<'a>(state: &VoiceHud) -> Element<'a, Message> {
    let display = text_input(
        "Type 1NNN for mesh, 9 + E.164 for PSTN",
        &state.dialer_input,
    )
    .on_input(Message::DialerInputChanged)
    .size(20.0)
    .padding(Padding::from([10, 12]))
    .width(Length::Fill);

    let resolved = resolve_target(&state.dialer_input, &state.roster);
    let chip = build_resolved_chip(&resolved);

    column![
        container(display).sty(|_: &Theme| cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(theme::SURF_C)),
            border: cosmic::iced::Border {
                radius: cosmic::iced::border::Radius::from(8.0),
                color: theme::OUTLINE_VAR,
                width: 1.0,
            },
            ..Default::default()
        }),
        chip,
    ]
    .spacing(8)
    .into()
}

/// Build the resolved-classification chip for the current
/// display contents. One pill per state, colored by category.
fn build_resolved_chip<'a>(resolved: &Resolved) -> Element<'a, Message> {
    let (label, color) = resolved_chip_label_and_color(resolved);
    container(text(label).size(12.0).colr(Color::WHITE))
        .sty(move |_: &Theme| cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(color)),
            border: cosmic::iced::Border {
                radius: cosmic::iced::border::Radius::from(12.0),
                ..Default::default()
            },
            ..Default::default()
        })
        .padding(Padding::from([4, 10]))
        .into()
}

/// Map a `Resolved` state to its chip label + tint.
#[must_use]
pub fn resolved_chip_label_and_color(resolved: &Resolved) -> (String, Color) {
    match resolved {
        Resolved::Empty => ("type 1NNN or 9+E.164".to_string(), theme::SURF_C_HI),
        Resolved::Mesh { name, .. } => (format!("mesh · {name}"), theme::PRESENCE_AVAILABLE),
        Resolved::MeshUnknown => ("mesh · not in roster".to_string(), theme::ERROR),
        Resolved::MeshPartial { remaining } => (
            format!(
                "{remaining} more digit{}",
                if *remaining == 1 { "" } else { "s" }
            ),
            theme::INFO,
        ),
        Resolved::Pstn { formatted } => (format!("PSTN · {formatted}"), theme::PRIMARY),
        Resolved::PstnPartial { remaining } => (
            format!(
                "{remaining} more digit{} via Vitelity",
                if *remaining == 1 { "" } else { "s" }
            ),
            theme::INFO,
        ),
        Resolved::Invalid => ("invalid prefix".to_string(), theme::ERROR),
    }
}

/// Build the 3 × 4 keypad. Numeric 1-9 + *, 0, # in the standard
/// phone-pad layout. Each button click appends to the dialer
/// input. MOTION-FEEDBACK-1 — every key shares the hover-lift +
/// press-depress feedback (drawn from the shared `mde_theme::feedback`
/// vocabulary).
fn build_keypad(state: &VoiceHud) -> Element<'_, Message> {
    let now = Instant::now();
    let rows: [[char; 3]; 4] = [
        ['1', '2', '3'],
        ['4', '5', '6'],
        ['7', '8', '9'],
        ['*', '0', '#'],
    ];
    let mut col: Vec<Element<Message>> = Vec::with_capacity(4);
    for line in rows {
        let mut row_buf: Vec<Element<Message>> = Vec::with_capacity(3);
        for c in line {
            row_buf.push(keypad_button(state, now, c));
        }
        col.push(row(row_buf).spacing(8).into());
    }
    column(col).spacing(8).into()
}

/// MOTION-FEEDBACK-1 — the control id for one keypad key (`key/<c>`), keying its
/// hover/press feedback in [`VoiceHud`].
fn keypad_id(c: char) -> String {
    format!("key/{c}")
}

/// MOTION-FEEDBACK-1 — render a control's [`FeedbackParams`] (hover-lift +
/// press-depress) as a `(top, bottom)` padding pair around `base_pad`. The control
/// rises on hover and sinks on press, and in BOTH cases the opposite edge absorbs
/// the offset by the same amount, so the cell's total height is constant — the
/// control *translates* without any neighbour reflow. iced 0.13 has no transform
/// widget, so this is the MOTION-INFRA-2 translate-as-padding idiom; the press
/// `scale` (<1.0) is mapped to a small downward shift (its visual proxy).
fn feedback_padding(fb: FeedbackParams, base_pad: f32) -> (f32, f32) {
    // Hover-lift: `translate_y` is negative (up). Shift the control up by `lift` —
    // grow the top padding, shrink the bottom by the same amount.
    let lift = (-fb.translate_y).max(0.0);
    // Press-depress: the scale-down (e.g. 0.96) maps to a small DOWNWARD shift, so
    // the press reads as the control pressing in. Like the lift it's an opposite-
    // edge offset (top shrinks, bottom grows), so the total height is unchanged.
    let sink = press_sink_px(fb.scale);
    // Net vertical offset: lift up (−) and press down (+) compose. The opposite
    // edge mirrors it, keeping `top + bottom == 2 * base_pad`.
    let offset = sink - lift;
    let top = (base_pad + offset).max(0.0);
    let bottom = (base_pad - offset).max(0.0);
    (top, bottom)
}

/// MOTION-FEEDBACK-1 — the downward press shift (px) for a control at depress
/// `scale` (`1.0` = released, `< 1.0` = pressed). A pressed control shifts in by at
/// most [`mde_theme::feedback::HOVER_LIFT_PX`] — the same single-sourced Carbon
/// micro-interaction nudge the hover-lift uses (§4: one token, no scattered metric)
/// — scaled by how deep the press is (`1 - scale`, normalized by the full
/// [`mde_theme::feedback::PRESS_DEPTH`]). Pure.
fn press_sink_px(scale: f32) -> f32 {
    let depth = (1.0 - scale).max(0.0);
    // Normalize the depth to 0..=1 against the full press depth so a full press is
    // exactly HOVER_LIFT_PX of travel (mirroring the hover nudge magnitude).
    let frac = (depth / mde_theme::feedback::PRESS_DEPTH).clamp(0.0, 1.0);
    frac * mde_theme::feedback::HOVER_LIFT_PX
}

/// One 3 × 4 keypad button. Renders the digit/symbol on a
/// surface-container background; click fires
/// `Message::KeypadPressed(c)`. MOTION-FEEDBACK-1 — wrapped in a `mouse_area`
/// that drives the shared hover-lift + press-depress feedback; the hover state
/// also tints the key (the non-motion cue kept under reduce-motion).
fn keypad_button(state: &VoiceHud, now: Instant, c: char) -> Element<'_, Message> {
    let id = keypad_id(c);
    let hovered = state.hovered.as_deref() == Some(&id);
    let fb = state.control_feedback(&id, now);
    // Hover tint is the non-motion cue (kept under reduce-motion, where the lift
    // is dropped): a hovered key brightens to the high-elevation surface step.
    let bg = if hovered { theme::SURF_C_HI } else { theme::SURF_C };
    let key = button(
        container(text(c.to_string()).size(22.0).colr(theme::ON_SURF))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(cosmic::iced::alignment::Horizontal::Center)
            .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .on_press(Message::KeypadPressed(c))
    .width(Length::Fill)
    .height(Length::Fixed(56.0))
    .sty(move |_: &Theme, _status| cosmic::iced::widget::button::Style {
        background: Some(cosmic::iced::Background::Color(bg)),
        text_color: theme::ON_SURF,
        border: cosmic::iced::Border {
            radius: cosmic::iced::border::Radius::from(8.0),
            ..Default::default()
        },
        ..Default::default()
    });
    feedback_wrap(key, fb, id)
}

/// MOTION-FEEDBACK-1 — wrap an interactive control so its hover-lift /
/// press-depress feedback is applied (as padding) and its pointer enter / exit /
/// press / release events drive that feedback's tweens. The single place every
/// control routes through, so the feedback vocabulary is applied identically.
fn feedback_wrap<'a>(
    inner: impl Into<Element<'a, Message>>,
    fb: FeedbackParams,
    id: String,
) -> Element<'a, Message> {
    let (top, bottom) = feedback_padding(fb, 0.0);
    let lifted = container(inner.into()).padding(Padding {
        top,
        right: 0.0,
        bottom,
        left: 0.0,
    });
    cosmic::iced::widget::mouse_area(lifted)
        .on_enter(Message::HoverEnter(id.clone()))
        .on_exit(Message::HoverExit(id.clone()))
        .on_press(Message::ControlPressed(id.clone()))
        .on_release(Message::ControlReleased(id))
        .into()
}

/// A full-width call-action pill in `fill` with a `SURF` label, carrying the
/// shared hover-lift + press-depress feedback (MOTION-FEEDBACK-1). `alpha` is the
/// call-bar state-crossfade opacity (the fill fades in on a mode change).
fn call_pill<'a>(
    state: &VoiceHud,
    now: Instant,
    id: &str,
    label: &'a str,
    fill: Color,
    msg: Message,
) -> Element<'a, Message> {
    let fill = fade_color(fill, state.call_state_alpha(now));
    let fb = state.control_feedback(id, now);
    let pill = button(
        container(text(label).size(16.0).colr(theme::SURF))
            .width(Length::Fill)
            .align_x(cosmic::iced::alignment::Horizontal::Center),
    )
    .on_press(msg)
    .width(Length::Fill)
    .height(Length::Fixed(48.0))
    .sty(
        move |_: &Theme, _status| cosmic::iced::widget::button::Style {
            background: Some(cosmic::iced::Background::Color(fill)),
            text_color: theme::SURF,
            border: cosmic::iced::Border {
                radius: cosmic::iced::border::Radius::from(8.0),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    feedback_wrap(pill, fb, id.to_string())
}

/// The call-action row (Answer/Decline · Hang-up · Call) + live call status.
/// MOTION-TRANS — the action fill + status crossfade in on a call-mode change;
/// MOTION-FEEDBACK-1 — each pill carries the hover-lift + press-depress feedback.
fn build_call_bar(state: &VoiceHud, now: Instant) -> Element<'_, Message> {
    let alpha = state.call_state_alpha(now);
    let action: Element<Message> = if matches!(state.call, sip::CallState::Incoming { .. }) {
        row![
            call_pill(state, now, "call/answer", "Answer", theme::SUCCESS, Message::Answer),
            call_pill(state, now, "call/decline", "Decline", theme::ERROR, Message::Decline),
        ]
        .spacing(8)
        .into()
    } else if state.call.is_active() {
        call_pill(state, now, "call/hangup", "Hang up", theme::ERROR, Message::HangUp)
    } else {
        let enabled = !state.dialer_input.trim().is_empty();
        let fill = fade_color(
            if enabled { theme::SUCCESS } else { theme::SURF_C },
            alpha,
        );
        let label_color = if enabled {
            theme::SURF
        } else {
            theme::ON_SURF_MUTED
        };
        let mut b = button(
            container(text("Call").size(16.0).colr(label_color))
                .width(Length::Fill)
                .align_x(cosmic::iced::alignment::Horizontal::Center),
        )
        .width(Length::Fill)
        .height(Length::Fixed(48.0))
        .sty(
            move |_: &Theme, _status| cosmic::iced::widget::button::Style {
                background: Some(cosmic::iced::Background::Color(fill)),
                text_color: theme::SURF,
                border: cosmic::iced::Border {
                    radius: cosmic::iced::border::Radius::from(8.0),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        if enabled {
            b = b.on_press(Message::PlaceCall);
        }
        // The Call pill only takes feedback when it's actually pressable.
        if enabled {
            feedback_wrap(b, state.control_feedback("call/place", now), "call/place".to_string())
        } else {
            b.into()
        }
    };
    column![
        action,
        text(state.call.label())
            .size(11.0)
            .colr(fade_color(theme::ON_SURF_VAR, alpha)),
    ]
    .spacing(6)
    .into()
}

// ── Pure helpers ────────────────────────────────────────────────────────

/// `true` if `c` is a valid dialer character: ASCII digit, `*`,
/// or `#`. Keypad + keyboard inputs filter through this to keep
/// the display field strictly dialer-shaped.
#[must_use]
pub fn is_dialer_char(c: char) -> bool {
    c.is_ascii_digit() || c == '*' || c == '#'
}

/// VOIP-P2P — a dialed string names a mesh PEER (vs a number/extension) when it
/// contains an alphabetic character (a hostname). Peer names route to a direct
/// registrar-less overlay call; pure numbers route to the registrar. Pure.
#[must_use]
pub fn looks_like_peer(dialed: &str) -> bool {
    dialed.chars().any(|c| c.is_ascii_alphabetic())
}

/// VOIP-P2P — normalize a dialed peer name to a resolvable overlay host. A bare
/// name gets the `.mesh.mde` mesh-DNS suffix (which resolves to the peer's
/// overlay IP); an already-qualified host (contains `.`), or a `sip:`/`user@`
/// form, yields just its host part. Pure + testable.
#[must_use]
pub fn peer_host_for(dialed: &str) -> String {
    let d = dialed.trim();
    let host = d.strip_prefix("sip:").unwrap_or(d);
    // Take the host part after any `user@`.
    let host = host.rsplit('@').next().unwrap_or(host).trim();
    if host.contains('.') {
        host.to_string()
    } else {
        format!("{host}.mesh.mde")
    }
}

/// Strip non-dialer characters from a pasted string. Operator
/// pasting `"(415) 555-1234"` into the field should resolve to
/// `"4155551234"`. Spaces, parens, dashes all drop.
#[must_use]
pub fn filter_dialer_chars(s: &str) -> String {
    s.chars().filter(|c| is_dialer_char(*c)).collect()
}

// ── Main ────────────────────────────────────────────────────────────────

/// Headless persistent-agent mode (`mde-voice-hud --agent`, E7.5). Runs the SIP
/// register + inbound-listen loop with no GUI, publishing `state/voice/status`
/// to the Bus for the birthright commissioning dashboard. Blocks until the
/// process is signalled (SIGTERM from the session teardown). With no
/// `account.toml` it publishes an honest "Not registered" heartbeat rather than
/// exiting, so a reader can still distinguish "agent up, no account" from "agent
/// not running".
fn run_headless_agent() {
    // VOIP-P2P — registrar-less by default: with no account.toml, run as a P2P
    // agent on the overlay (a local identity, no registrar) so peers can still
    // dial this node directly. A configured account.toml uses the registrar.
    let acct = sip::SipAccount::load().unwrap_or_else(sip::SipAccount::local_identity);
    tracing::info!(server = %acct.server_host, "voice agent: starting (headless)");
    let (event_tx, event_rx) = mpsc::channel::<sip::AgentEvent>();
    // Hold the command sender for the agent's lifetime so its loop never sees a
    // disconnected channel; drain events so the channel never backs up (and log
    // inbound rings — the answer/decline HUD hand-off is E5.4's domain).
    let (_cmd_tx, cmd_rx) = mpsc::channel::<sip::AgentCommand>();
    std::thread::spawn(move || {
        while let Ok(ev) = event_rx.recv() {
            if let sip::AgentEvent::Incoming { from, .. } = &ev {
                tracing::info!(%from, "voice agent: inbound call ringing (headless)");
            }
        }
    });
    sip::run_agent(&acct, &event_tx, &cmd_rx);
}

fn main() -> Result<(), cosmic::iced::Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("MDE_VOICE_HUD_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("mde_voice_hud=info,warn")),
        )
        .json()
        .init();

    // E7.5 — headless agent mode. The labwc autostart launches `mde-voice-hud
    // --agent` at login so the persistent SIP agent registers + listens for
    // inbound calls (and publishes `state/voice/status` to the Bus) without a
    // window. Returns only when the process is signalled.
    if std::env::args().skip(1).any(|a| a == "--agent") {
        run_headless_agent();
        return Ok(());
    }

    cosmic::iced::daemon(
        || {
            let RosterLoad { peers, source } = roster::load();
            tracing::info!(
                roster_count = peers.len(),
                ?source,
                "voice-hud: roster loaded"
            );
            // VOIP-28 — kick off a real SIP REGISTER on launch when an account
            // is configured. The blocking socket work runs off the UI thread
            // via spawn_blocking; the result lands as Message::Registered.
            let account = sip::SipAccount::load();
            // VOIP-P2P — the agent ALWAYS runs: a registrar account when
            // configured, else a registrar-less local overlay identity so the
            // node still listens for direct peer INVITEs. `state.account` keeps
            // the loaded Option (None → outbound dials synthesize a local
            // identity in PlaceCall); the agent thread gets the resolved one.
            let agent_account = account
                .clone()
                .unwrap_or_else(sip::SipAccount::local_identity);
            let registration = {
                tracing::info!(server = %agent_account.server_host, "voice-hud: starting SIP agent");
                // Spawn the persistent SIP agent (register/listen or P2P-listen +
                // inbound INVITE/BYE) on its own thread, bridged to the UI via the
                // AGENT_* channels (events ← agent, commands → agent).
                let (event_tx, event_rx) = mpsc::channel::<sip::AgentEvent>();
                let (cmd_tx, cmd_rx) = mpsc::channel::<sip::AgentCommand>();
                if let Ok(mut g) = AGENT_EVENTS.lock() {
                    *g = Some(event_rx);
                }
                if let Ok(mut g) = AGENT_CMD.lock() {
                    *g = Some(cmd_tx);
                }
                let _ = std::thread::Builder::new()
                    .name("mwv-sip-agent".into())
                    .spawn(move || sip::run_agent(&agent_account, &event_tx, &cmd_rx));
                sip::RegistrationState::Registering
            };
            // MOTION-* — honor the user's reduce-motion preference for the HUD's
            // appear / state / feedback motion (env `MDE_REDUCE_MOTION` overrides
            // the file; both read by `Preferences::load`).
            let reduce_motion = mde_theme::prefs::Preferences::load().a11y.reduce_motion;
            // MOTION-* — arm the appear transition (fade + slide-in) so the HUD
            // animates in on first paint. The tick subscription is created
            // because the animator is non-idle; it stops once the tween settles.
            let mut anim = Animator::new();
            anim.start(ANIM_APPEAR, Instant::now(), Motion::panel_mount(), reduce_motion);
            (
                VoiceHud {
                    dialer_input: String::new(),
                    roster: peers,
                    registration,
                    account,
                    call: sip::CallState::Idle,
                    session: None,
                    media: None,
                    anim,
                    reduce_motion,
                    pressed: None,
                    hovered: None,
                    call_kind: CallKind::Idle,
                },
                boot_surface(),
            )
        },
        update,
        view,
    )
    .title(namespace)
    .subscription(subscription)
    .theme(theme)
    .run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::Peer;

    fn sample_roster() -> Vec<Peer> {
        vec![Peer {
            ext: "1003".to_string(),
            name: "alice".to_string(),
            role: "GUI".to_string(),
            presence: "available".to_string(),
            lan: true,
            hint: "Alice's ThinkPad".to_string(),
        }]
    }

    fn make_hud() -> VoiceHud {
        VoiceHud {
            dialer_input: String::new(),
            roster: vec![],
            registration: sip::RegistrationState::NoAccount,
            account: None,
            call: sip::CallState::Idle,
            session: None,
            media: None,
            anim: Animator::new(),
            reduce_motion: false,
            pressed: None,
            hovered: None,
            call_kind: CallKind::Idle,
        }
    }

    #[test]
    fn dial_request_keeps_peer_name_for_direct_p2p() {
        // VOIP-P2P — registrar-less: a Bus dial request for a mesh peer keeps
        // the NAME in the dialer (PlaceCall then routes it to a direct overlay
        // call), instead of resolving it to a registrar extension. A bare number
        // is kept verbatim for the registrar path.
        let mut hud = make_hud();
        hud.roster = vec![Peer {
            ext: "1007".into(),
            name: "pine".into(),
            role: "Host".into(),
            presence: "available".into(),
            lan: true,
            hint: String::new(),
        }];
        let _ = update(&mut hud, Message::DialRequested("pine".into()));
        assert_eq!(
            hud.dialer_input, "pine",
            "peer name kept for direct P2P dial"
        );
        assert!(
            looks_like_peer(&hud.dialer_input),
            "routes to place_call_direct"
        );

        let _ = update(&mut hud, Message::DialRequested("1009".into()));
        assert_eq!(
            hud.dialer_input, "1009",
            "a number is kept for the registrar"
        );
        assert!(!looks_like_peer(&hud.dialer_input));
    }

    #[test]
    fn dial_request_ignores_empty_target() {
        let mut hud = make_hud();
        hud.dialer_input = "keep".into();
        let _ = update(&mut hud, Message::DialRequested("  ".into()));
        assert_eq!(hud.dialer_input, "keep");
    }

    #[test]
    fn is_dialer_char_accepts_digits_star_hash() {
        for c in '0'..='9' {
            assert!(is_dialer_char(c), "digit {c} should be a dialer char");
        }
        assert!(is_dialer_char('*'));
        assert!(is_dialer_char('#'));
        // Letters + whitespace + punctuation are not dialer chars.
        assert!(!is_dialer_char('a'));
        assert!(!is_dialer_char(' '));
        assert!(!is_dialer_char('-'));
        assert!(!is_dialer_char('+'));
    }

    #[test]
    fn filter_dialer_chars_strips_formatting() {
        assert_eq!(filter_dialer_chars("(415) 555-1234"), "4155551234");
        assert_eq!(filter_dialer_chars("9 800 555 0199"), "98005550199");
        assert_eq!(filter_dialer_chars("1003"), "1003");
        assert_eq!(filter_dialer_chars(""), "");
        // Letters dropped, digits preserved.
        assert_eq!(filter_dialer_chars("call 1003 now"), "1003");
    }

    #[test]
    fn looks_like_peer_distinguishes_names_from_numbers() {
        // Mesh peer names (have letters) → direct P2P.
        assert!(looks_like_peer("pine"));
        assert!(looks_like_peer("pine.mesh.mde"));
        assert!(looks_like_peer("UNIT-EAGLE"));
        // Numbers / extensions / SIP digits → registrar.
        assert!(!looks_like_peer("1004"));
        assert!(!looks_like_peer("+15551234567"));
        assert!(!looks_like_peer("*69"));
        assert!(!looks_like_peer(""));
    }

    #[test]
    fn peer_host_for_appends_mesh_suffix_for_bare_names() {
        assert_eq!(peer_host_for("pine"), "pine.mesh.mde");
        // Already-qualified hosts are used as-is.
        assert_eq!(peer_host_for("pine.mesh.mde"), "pine.mesh.mde");
        assert_eq!(peer_host_for("pine.mesh"), "pine.mesh");
        // sip: / user@ forms reduce to the host part.
        assert_eq!(peer_host_for("sip:pine"), "pine.mesh.mde");
        assert_eq!(peer_host_for("sip:matt@birch.mesh.mde"), "birch.mesh.mde");
    }

    #[test]
    fn resolved_chip_empty_state() {
        let (label, _color) = resolved_chip_label_and_color(&Resolved::Empty);
        assert_eq!(label, "type 1NNN or 9+E.164");
    }

    #[test]
    fn resolved_chip_mesh_with_peer_name() {
        let roster = sample_roster();
        let resolved = resolve_target("1003", &roster);
        let (label, _color) = resolved_chip_label_and_color(&resolved);
        assert!(label.starts_with("mesh · "));
        assert!(label.contains("alice"));
    }

    #[test]
    fn resolved_chip_mesh_unknown() {
        let roster = sample_roster();
        // 1999 doesn't exist in the sample roster.
        let resolved = resolve_target("1999", &roster);
        let (label, _color) = resolved_chip_label_and_color(&resolved);
        assert_eq!(label, "mesh · not in roster");
    }

    #[test]
    fn resolved_chip_mesh_partial_singular_and_plural() {
        let roster = sample_roster();
        // 1 char → "3 more digits".
        let (label, _) = resolved_chip_label_and_color(&resolve_target("1", &roster));
        assert_eq!(label, "3 more digits");
        // 3 chars → "1 more digit" (singular).
        let (label, _) = resolved_chip_label_and_color(&resolve_target("100", &roster));
        assert_eq!(label, "1 more digit");
    }

    #[test]
    fn resolved_chip_pstn_formatted() {
        let roster = sample_roster();
        // `9` + 11 digits.
        let resolved = resolve_target("914155551234", &roster);
        let (label, _) = resolved_chip_label_and_color(&resolved);
        assert!(label.starts_with("PSTN · "));
    }

    #[test]
    fn resolved_chip_pstn_partial_singular_and_plural() {
        let roster = sample_roster();
        // 1 digit after 9 → "10 more digits via Vitelity".
        let (label, _) = resolved_chip_label_and_color(&resolve_target("91", &roster));
        assert_eq!(label, "10 more digits via Vitelity");
        // 10 digits after 9 → "1 more digit via Vitelity".
        let (label, _) = resolved_chip_label_and_color(&resolve_target("94155551234", &roster));
        assert_eq!(label, "1 more digit via Vitelity");
    }

    #[test]
    fn resolved_chip_invalid_prefix() {
        let roster = sample_roster();
        // Prefix not in [1, 9] range.
        let resolved = resolve_target("5555", &roster);
        let (label, _) = resolved_chip_label_and_color(&resolved);
        assert_eq!(label, "invalid prefix");
    }

    #[test]
    fn voice_hud_keypad_pressed_appends_to_input() {
        let mut hud = make_hud();
        assert_eq!(hud.dialer_input, "");
        for c in "1003".chars() {
            let _ = update(&mut hud, Message::KeypadPressed(c));
        }
        assert_eq!(hud.dialer_input, "1003");
    }

    #[test]
    fn voice_hud_keypad_rejects_non_dialer_char() {
        let mut hud = make_hud();
        let _ = update(&mut hud, Message::KeypadPressed('a'));
        assert_eq!(hud.dialer_input, "");
    }

    #[test]
    fn voice_hud_backspace_removes_last_char() {
        let mut hud = make_hud();
        for c in "1003".chars() {
            let _ = update(&mut hud, Message::KeypadPressed(c));
        }
        let _ = update(&mut hud, Message::Backspace);
        assert_eq!(hud.dialer_input, "100");
        // Backspace on empty input is a no-op.
        for _ in 0..10 {
            let _ = update(&mut hud, Message::Backspace);
        }
        assert_eq!(hud.dialer_input, "");
    }

    #[test]
    fn voice_hud_dialer_input_changed_filters_input() {
        let mut hud = make_hud();
        // Paste a formatted number — non-dialer chars drop.
        let _ = update(
            &mut hud,
            Message::DialerInputChanged("(415) 555-1234".to_string()),
        );
        assert_eq!(hud.dialer_input, "4155551234");
    }

    #[test]
    fn layer_settings_match_design_lock() {
        // Compile-time + const-eval check that VOIP-27's §2.5
        // values are wired correctly.
        assert_eq!(WIDTH, 420);
        assert_eq!(HEIGHT, 720);
        assert_eq!(MARGIN_RIGHT, 16);
        assert_eq!(MARGIN_BOTTOM, 56);
    }

    #[test]
    fn topbar_shows_honest_unregistered_state_and_derived_initials() {
        // The topbar is honest single-node: with no account.toml the state is
        // NoAccount → "Not registered" (not a fabricated "Registered ·
        // 127.0.0.1:5060"). The live registrar round-trip is the SIP-server
        // bench; VOIP-28 drives the real REGISTER + state transitions.
        assert_eq!(sip::RegistrationState::NoAccount.label(), "Not registered");
        // Initials derive from the real peer name (up to two leading alnums).
        assert_eq!(account_initials("Pixel Workstation"), "PI");
        assert_eq!(account_initials("mde-host-7"), "MD");
        assert_eq!(account_initials("—//—"), "—");
        // The local peer name is the real host identity, never empty.
        assert!(!local_peer_name().is_empty());
    }

    // ── MOTION-FEEDBACK-1 / MOTION-TRANS — the HUD's shared-motion wiring ──────

    #[test]
    fn appear_transition_arms_and_settles_then_goes_idle() {
        // MOTION-* — a fresh HUD arms the appear tween (fade 0→1 + slide-in), so
        // the animator is non-idle (the tick subscription runs); after the
        // panel-mount duration it settles to full opacity + zero offset and goes
        // idle (the tick stops — no idle CPU).
        let now = Instant::now();
        let mut hud = make_hud();
        hud.anim
            .start(ANIM_APPEAR, now, Motion::panel_mount(), false);
        assert!(!hud.anim.is_idle(now), "appear tween in flight ⇒ not idle");
        let (a0, slide0) = hud.appear_params(now);
        assert!(a0 < 1e-3, "starts transparent");
        assert!(slide0 > 0.0, "starts slid down");
        let done = now + Motion::panel_mount().duration;
        let (a1, slide1) = hud.appear_params(done);
        assert!((a1 - 1.0).abs() < 1e-3, "ends opaque");
        assert!(slide1.abs() < 1e-3, "rests at zero offset");
        assert!(hud.anim.is_idle(done), "settled ⇒ idle (tick stops)");
    }

    #[test]
    fn appear_reduce_motion_drops_the_slide() {
        // Reduce-motion keeps the (fast) fade but drops the slide entirely — no
        // positional motion, regardless of progress.
        let now = Instant::now();
        let mut hud = make_hud();
        hud.reduce_motion = true;
        hud.anim.start(ANIM_APPEAR, now, Motion::panel_mount(), true);
        for ms in [0, 20, 40, 80, 240] {
            let (_a, slide) = hud.appear_params(now + Duration::from_millis(ms));
            assert_eq!(slide, 0.0, "no slide under reduce-motion at {ms}ms");
        }
    }

    #[test]
    fn call_state_change_arms_the_crossfade_once_per_mode() {
        // MOTION-TRANS — entering a call mode (Idle → Calling) starts the call-bar
        // crossfade; a same-mode update does NOT re-arm it.
        let now = Instant::now();
        let mut hud = make_hud();
        // No animation pending at rest.
        hud.anim.gc(now);
        hud.call = sip::CallState::Calling { peer: "pine".into() };
        hud.sync_call_state();
        assert_eq!(hud.call_kind, CallKind::Calling);
        assert!(
            hud.anim.is_animating(ANIM_CALLSTATE, now),
            "mode change arms the crossfade"
        );
        // Drain it (well past the tooltip-fade window, measured from the real
        // clock the animator stamped the tween at), then a Ringing update is the
        // SAME coarse mode ⇒ no re-arm.
        let settled = Instant::now() + Motion::tooltip_fade().duration + Duration::from_millis(50);
        hud.anim.gc(settled);
        hud.call = sip::CallState::Ringing { peer: "pine".into() };
        hud.sync_call_state();
        assert!(
            !hud.anim.is_animating(ANIM_CALLSTATE, settled),
            "same coarse mode (Calling) does not re-fire"
        );
    }

    #[test]
    fn hover_lift_rises_with_motion_and_is_static_under_reduce_motion() {
        // MOTION-FEEDBACK-1 — a hovered control lifts (negative translate_y) over
        // the hover motion; reduce-motion drops the movement (the color tint is
        // the kept cue, not the lift).
        let now = Instant::now();
        let id = keypad_id('5');
        let mut hud = make_hud();
        hud.hovered = Some(id.clone());
        hud.anim.start(hover_key(&id), now, Motion::hover(), false);
        // At the end of the hover tween the control is lifted up.
        let end = now + Motion::hover().duration;
        let lifted = hud.control_feedback(&id, end);
        assert!(
            lifted.translate_y < -1e-3,
            "hover lifts up, got {}",
            lifted.translate_y
        );
        // Reduce-motion: no movement at any sampled frame.
        hud.reduce_motion = true;
        for ms in [0, 35, 70, 200] {
            let g = hud.control_feedback(&id, now + Duration::from_millis(ms));
            assert_eq!(g.translate_y, 0.0, "no hover-lift under reduce-motion @{ms}ms");
        }
    }

    #[test]
    fn press_depresses_on_down_with_no_input_delay() {
        // MOTION-FEEDBACK-1 acceptance — the depress is at full depth the instant
        // the control is pressed (no warm-up tween); released ⇒ back to natural
        // size.
        let now = Instant::now();
        let id = keypad_id('7');
        let mut hud = make_hud();
        let _ = update(&mut hud, Message::ControlPressed(id.clone()));
        assert_eq!(hud.pressed.as_deref(), Some(id.as_str()));
        let down = hud.control_feedback(&id, now);
        assert!(
            (down.scale - (1.0 - mde_theme::feedback::PRESS_DEPTH)).abs() < 1e-6,
            "press at full depth on the down edge, got {}",
            down.scale
        );
        // Sinks downward as the visual proxy for the scale-down.
        let (top, _bottom) = feedback_padding(down, 0.0);
        assert!(top > 0.0, "press sinks the control");
        let _ = update(&mut hud, Message::ControlReleased(id.clone()));
        assert_eq!(hud.pressed, None);
        let up = hud.control_feedback(&id, now);
        assert!((up.scale - 1.0).abs() < 1e-6, "release restores natural size");
    }

    #[test]
    fn anim_tick_gcs_settled_tweens_and_goes_idle() {
        // MOTION-PERF-1 — the tick advances/GCs the animator; once everything is
        // settled the animator is idle so the subscription stops (no idle CPU).
        let now = Instant::now();
        let mut hud = make_hud();
        hud.anim.start(ANIM_APPEAR, now, Motion::panel_mount(), false);
        assert!(!hud.anim.is_idle(now));
        // A tick well past the duration settles + GCs it.
        std::thread::sleep(Duration::from_millis(5));
        let _ = update(&mut hud, Message::AnimTick);
        // After the panel-mount window the animator is idle.
        assert!(hud.anim.is_idle(now + Motion::panel_mount().duration));
    }

    #[test]
    fn hover_enter_exit_tracks_the_hovered_control() {
        // MOTION-FEEDBACK-1 — enter sets the hovered control + arms its tween; a
        // stale exit for a different control is ignored.
        let mut hud = make_hud();
        let a = keypad_id('1');
        let b = keypad_id('2');
        let _ = update(&mut hud, Message::HoverEnter(a.clone()));
        assert_eq!(hud.hovered.as_deref(), Some(a.as_str()));
        // A stale exit for a control that isn't hovered is a no-op.
        let _ = update(&mut hud, Message::HoverExit(b));
        assert_eq!(hud.hovered.as_deref(), Some(a.as_str()));
        // Exiting the hovered control clears it.
        let _ = update(&mut hud, Message::HoverExit(a));
        assert_eq!(hud.hovered, None);
    }

    #[test]
    fn feedback_padding_keeps_cell_height_constant() {
        // MOTION-FEEDBACK-1 — the lift/press are rendered as opposite-edge padding
        // so the control TRANSLATES without changing the cell height (no neighbour
        // reflow): top + bottom == 2 * base_pad for any feedback state.
        let base = 8.0;
        for (ty, scale) in [
            (0.0, 1.0),                                          // at rest
            (-mde_theme::feedback::HOVER_LIFT_PX, 1.0),          // hovered
            (0.0, 1.0 - mde_theme::feedback::PRESS_DEPTH),       // pressed
            (-mde_theme::feedback::HOVER_LIFT_PX, 1.0 - mde_theme::feedback::PRESS_DEPTH), // both
        ] {
            let (top, bottom) = feedback_padding(
                FeedbackParams {
                    translate_y: ty,
                    scale,
                },
                base,
            );
            assert!(
                (top + bottom - 2.0 * base).abs() < 1e-4,
                "height must be constant: top {top} + bottom {bottom} != {}",
                2.0 * base
            );
        }
    }

    #[test]
    fn press_sink_is_token_sourced_and_bounded() {
        // §4 — the press shift is single-sourced from the shared HOVER_LIFT_PX
        // token (no scattered metric): released ⇒ no shift; full press ⇒ exactly
        // one HOVER_LIFT_PX of travel.
        assert_eq!(press_sink_px(1.0), 0.0, "released ⇒ no sink");
        let full = press_sink_px(1.0 - mde_theme::feedback::PRESS_DEPTH);
        assert!(
            (full - mde_theme::feedback::HOVER_LIFT_PX).abs() < 1e-4,
            "full press sinks exactly HOVER_LIFT_PX, got {full}"
        );
        // Never exceeds the nudge even past full depth.
        assert!(press_sink_px(0.0) <= mde_theme::feedback::HOVER_LIFT_PX + 1e-6);
    }

    #[test]
    fn fade_color_blends_toward_surface() {
        // MOTION-* — alpha 0 ⇒ fully the surface background (invisible); alpha 1 ⇒
        // the original color (the iced-0.13 no-opacity fade).
        let at0 = fade_color(theme::PRIMARY, 0.0);
        assert!((at0.r - theme::SURF.r).abs() < 1e-6);
        assert!((at0.g - theme::SURF.g).abs() < 1e-6);
        assert!((at0.b - theme::SURF.b).abs() < 1e-6);
        let at1 = fade_color(theme::PRIMARY, 1.0);
        assert!((at1.r - theme::PRIMARY.r).abs() < 1e-6);
        assert!((at1.g - theme::PRIMARY.g).abs() < 1e-6);
    }
}
