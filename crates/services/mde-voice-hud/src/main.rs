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
//! - In-call controls: a mic Mute toggle, and the keypad sends DTMF — pressing a
//!   digit during an established call transmits an RFC 4733 telephone-event tone
//!   to the peer/IVR (the SDP advertises `telephone-event/8000`) instead of
//!   editing the dial buffer; the display chip flips to a "sends tones" hint.
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
use roster::{Peer, RosterLoad, RosterSource};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mde_theme::animation::{Animator, Transition};
use mde_theme::feedback::{ControlFeedback, FeedbackParams, FocusRing};
use mde_theme::motion::{Easing, Motion, PANEL_MOUNT_TRANSLATE_Y_PX};
use mde_theme::{LoadState, StateTone};

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
/// POLISH-voicehud-focusring — [`Animator`] key for the keyboard focus-ring
/// grow-in. Re-armed each time focus moves to a new control so the tick
/// subscription stays alive while that control's ring animates in (and only
/// then — a settled ring needs no ticks, MOTION-PERF-1).
const ANIM_FOCUS: &str = "focus";

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

/// Send an in-call DTMF digit. Outbound calls own their [`media::MediaSession`]
/// in `state.media`; inbound (answered) calls run their media in the agent
/// thread, so route through [`sip::AgentCommand::Dtmf`]. Either way it's a tone,
/// never a dial-buffer edit. No-op if neither path has a live media session.
fn send_in_call_dtmf(state: &VoiceHud, c: char) {
    if let Some(m) = state.media.as_ref() {
        if m.send_dtmf(c) {
            tracing::info!(digit = %c, "voice-hud: DTMF sent (local media)");
        }
    } else {
        // Inbound/answered call — the media session lives in the agent thread.
        agent_send(sip::AgentCommand::Dtmf(c));
        tracing::info!(digit = %c, "voice-hud: DTMF sent (agent media)");
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
    /// Operator clicked the on-screen Backspace key (or pressed
    /// Backspace on the hardware keyboard). Removes the last char
    /// of the dialer; a no-op while a call is up (the dialer is
    /// frozen, showing the peer rather than an editable target).
    Backspace,
    /// Operator pressed Escape — a keyboard *intent*, routed in
    /// `update` by the call state so a live call is never killed:
    /// an `Incoming` ring → `Decline`, an active call → `HangUp`,
    /// and only an idle/ended HUD → the graceful `Exit`. The
    /// mapping is the pure [`escape_action`]; routing is `Task::done`.
    Escape,
    /// Operator pressed Enter — the confirm/answer keyboard
    /// accelerator. Routed in `update` (pure [`enter_action`]) to
    /// the existing call messages: `Answer` a ringing call, else
    /// `PlaceCall` the dialed target; a no-op while a call is
    /// connecting or up. A keyboard intent, not a new call flow.
    Enter,
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
    /// POLISH-voicehud-focusring — Tab pressed: advance keyboard focus to the
    /// next on-screen control (wraps at the end), arming its focus-ring grow-in.
    /// Parallel to [`HoverEnter`](Self::HoverEnter), but driven by the keyboard
    /// rather than the pointer, so a keyboard user can SEE which control is
    /// focused.
    FocusNext,
    /// POLISH-voicehud-focusring — Shift+Tab: advance keyboard focus to the
    /// previous on-screen control (wraps at the start).
    FocusPrev,
    /// MOTION-FEEDBACK-1 — the pointer pressed *down* on a control. The depress
    /// fires on this down edge (no input delay), so it carries no timestamp.
    ControlPressed(String),
    /// MOTION-FEEDBACK-1 — the pointer released a control (depress lifts).
    ControlReleased(String),
    /// In-call control: toggle the microphone mute on the live media session.
    /// No-op when no call is up. Stops/resumes mic transmission while the peer's
    /// audio keeps playing.
    ToggleMute,
    /// POLISH-voicehud-loadstate — the operator clicked Retry on a recoverable
    /// (failed) registration. Optimistically flips the state to the in-flight
    /// `Registering` and asks the persistent SIP agent to re-REGISTER (its
    /// existing register action); the agent's result lands as a later
    /// `Agent(Registration(..))` event.
    RetryRegistration,
    /// POLISH-voicehud-chips — the operator clicked "Configure account" on the
    /// empty-dialer zero-state (shown only with no `account.toml`). Launches the
    /// `mde-voice-config` companion so they can register an account — best-effort,
    /// mirroring the desktop-notification spawn (a missing binary never crashes
    /// the HUD).
    ConfigureAccount,
}

/// Top-level HUD state.
pub struct VoiceHud {
    /// Current contents of the dialer display field.
    pub dialer_input: String,
    /// Loaded mesh roster — drives the `Resolved::Mesh` lookup
    /// in the resolved-chip rendering.
    pub roster: Vec<Peer>,
    /// POLISH-voicehud-loadstate — where [`roster`](Self::roster) came from
    /// (live mesh vs the embedded compile-time fixture). Drives the topbar's
    /// roster-source caveat so a fixture roster is surfaced honestly instead of
    /// being shown as if it were live mesh data.
    pub roster_source: RosterSource,
    /// Live SIP registration state (VOIP-28) — drives the topbar status
    /// line + state icon. `NoAccount` until `account.toml` exists.
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
    /// POLISH-voicehud-focusring — the control id currently keyboard-focused, if
    /// any. Parallel to [`hovered`](Self::hovered), but advanced by the keyboard
    /// (Tab / Shift+Tab) instead of the pointer; drives the Carbon focus ring so
    /// a keyboard user can see — and reach — each control. Only ever holds an id
    /// that is actually on screen (see [`focus_order`]).
    pub focused: Option<String>,
    /// POLISH-voicehud-focusring — when focus last moved to a new control. Drives
    /// the focus-ring grow-in tween ([`ControlFeedback::focus_ring`]); a single
    /// timestamp suffices because exactly one control is focused at a time.
    pub focus_since: Instant,
    /// MOTION-TRANS — the discriminant of the last-rendered call state, so the
    /// update handler can start the state-change crossfade only when the mode
    /// actually changes (not on every unrelated message).
    pub call_kind: CallKind,
    /// In-call microphone mute. Mirrors the live [`media::MediaSession`] mute
    /// flag so the in-call Mute pill renders the current state; reset to `false`
    /// whenever a call ends (the next call starts un-muted).
    pub muted: bool,
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

    /// POLISH-voicehud-focusring — move keyboard focus to `id` (or clear it),
    /// stamping [`focus_since`](Self::focus_since) and arming the focus-ring
    /// grow-in so the ring animates in on the newly-focused control. A no-op when
    /// the id is unchanged, so holding/re-pressing the same key doesn't restart
    /// the ring's grow-in. The focus tween is only armed when focusing a control
    /// (clearing focus needs no animation — the ring just snaps off).
    fn set_focus(&mut self, id: Option<String>) {
        if self.focused == id {
            return;
        }
        self.focused = id;
        self.focus_since = Instant::now();
        if self.focused.is_some() {
            self.start_anim(ANIM_FOCUS, Motion::focus());
        }
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

    /// POLISH-voicehud-focusring — the animated focus ring for control `id` at
    /// `now`, built from the shared [`ControlFeedback`] helper (§6 — reuse, not a
    /// parallel ring impl). Visible — and growing in over [`Motion::focus`] —
    /// only for the keyboard-[`focused`](Self::focused) control; an invisible
    /// (zero-width) ring for every other control. Under reduce-motion the ring is
    /// present immediately at full width (the helper's contract).
    fn focus_ring(&self, id: &str, now: Instant) -> FocusRing {
        let focused = self.focused.as_deref() == Some(id);
        ControlFeedback::new()
            .focused(focused, self.focus_since)
            .focus_ring(now, self.reduce_motion)
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
            // While a call is up the dialer field is frozen (it shows the peer,
            // not an editable target): a focused text-input would otherwise
            // CAPTURE typed digits before the keypad subscription sees them, so
            // route any newly-typed dialer char to DTMF and leave the buffer
            // unchanged — keyboard and on-screen keypad then behave identically.
            if matches!(state.call, sip::CallState::InCall { .. }) {
                // Compare the RAW new contents to the (possibly letter-bearing,
                // e.g. a peer name) frozen buffer: a single appended dialer char
                // is the keystroke to tone. Filtering first would drop the peer
                // name's letters and break the prefix match.
                if let Some(c) = appended_char(&state.dialer_input, &value) {
                    send_in_call_dtmf(state, c);
                }
            } else {
                state.dialer_input = filter_dialer_chars(&value);
            }
        }
        Message::KeypadPressed(c) => {
            // In an established call the keypad sends DTMF (RFC 4733 tones to the
            // peer/IVR) instead of editing the dial buffer. Otherwise it's the
            // dialer — append the char.
            if matches!(state.call, sip::CallState::InCall { .. }) {
                if is_dialer_char(c) {
                    send_in_call_dtmf(state, c);
                }
            } else if is_dialer_char(c) {
                state.dialer_input.push(c);
            }
        }
        Message::Backspace => {
            // The dialer is frozen while a call is up (it shows the peer, not an
            // editable target); don't let Backspace corrupt that display. Matches
            // the KeypadPressed / DialerInputChanged in-call freeze.
            if !matches!(state.call, sip::CallState::InCall { .. }) {
                state.dialer_input.pop();
            }
        }
        Message::Escape => {
            // A live call must never be killed by Esc: route Esc to the call
            // control that matches the current state (Decline a ring, Hang up an
            // active call) and only exit an idle/ended HUD. Reuses the existing
            // call-state messages — no new flow (§6).
            return Task::done(escape_action(&state.call));
        }
        Message::Enter => {
            // The confirm/answer accelerator: Answer a ring, else place the
            // dialed call (PlaceCall self-guards an empty buffer / an active
            // call); no Enter action while a call is connecting or up.
            if let Some(action) = enter_action(&state.call) {
                return Task::done(action);
            }
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
                state.muted = false;
                // Tear down this call's media so a stale session can't back the
                // mute pill into the next call (the inbound dialog's media also
                // lives in the agent thread, which ends on the BYE).
                if let Some(m) = state.media.take() {
                    m.stop();
                }
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
            // A fresh session always starts un-muted; clear any carried-over UI
            // mute so the pill can never claim "muted" over a live new session.
            state.muted = false;
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
            state.muted = false;
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
        Message::FocusNext => {
            // POLISH-voicehud-focusring — Tab walks focus forward through the
            // controls actually on screen (so the ring never lands on a hidden
            // control); set_focus arms the grow-in.
            let order = focus_order(state);
            let next = advance_focus(&order, state.focused.as_deref(), true);
            state.set_focus(next);
        }
        Message::FocusPrev => {
            // Shift+Tab — the same walk, reversed.
            let order = focus_order(state);
            let prev = advance_focus(&order, state.focused.as_deref(), false);
            state.set_focus(prev);
        }
        Message::ToggleMute => {
            // Toggle the live media session's mic mute. Only meaningful while a
            // call has media up; a no-op otherwise (the pill only renders then).
            if let Some(media) = &state.media {
                let next = !state.muted;
                media.set_muted(next);
                state.muted = next;
                tracing::info!(muted = next, "voice-hud: mic mute toggled");
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
        Message::RetryRegistration => {
            // Gated in the view on `can_retry()`, so this fires only on a
            // recoverable state. Show the in-flight state immediately, then ask
            // the agent to re-REGISTER — reusing its existing register action
            // (the periodic `agent_register` path) rather than a new flow (§6).
            state.registration = sip::RegistrationState::Registering;
            agent_send(sip::AgentCommand::Reregister);
        }
        Message::ConfigureAccount => {
            // Launch the voice-config companion (the account editor). Best-effort,
            // like the notify-send call record — a missing binary must never crash
            // the HUD. The CTA only renders with no account configured, so this is
            // the operator's real next step to register.
            let _ = std::process::Command::new("mde-voice-config").spawn();
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
        subs.push(cosmic::iced::time::every(ANIM_TICK).map(|_| Message::AnimTick));
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
    // Only act on events no widget consumed (`Ignored`) — the same focus contract
    // the dialer text-input relies on, so a focused field still owns its keys.
    event::listen_with(|event, status, _window| match event {
        cosmic::iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
            if status == event::Status::Ignored =>
        {
            key_to_message(&key.as_ref(), modifiers.shift())
        }
        _ => None,
    })
}

/// Pure key → [`Message`] mapping for the HUD's hardware-keyboard accelerators,
/// split out so the routing is unit-testable without an event loop. Escape and
/// Enter are keyboard *intents* the `update` handler resolves against the live
/// call state ([`escape_action`] / [`enter_action`]); `m`/`M` is the in-call mute
/// accelerator ([`Message::ToggleMute`] self-guards when no call is up, so it maps
/// unconditionally); Tab / Shift+Tab walk the keyboard focus ring through the
/// on-screen controls (POLISH-voicehud-focusring); a dialer char types into the
/// pad. Other keys are ignored.
fn key_to_message(key: &cosmic::iced::keyboard::Key<&str>, shift: bool) -> Option<Message> {
    use cosmic::iced::keyboard::{key::Named, Key};
    match key {
        Key::Named(Named::Escape) => Some(Message::Escape),
        Key::Named(Named::Enter) => Some(Message::Enter),
        Key::Named(Named::Backspace) => Some(Message::Backspace),
        // Tab moves keyboard focus through the controls so a keyboard user can
        // SEE which one is focused (the pointer-only mouse_areas drew no ring).
        // Shift+Tab reverses. This is the a11y nav the HUD was missing.
        Key::Named(Named::Tab) => Some(if shift {
            Message::FocusPrev
        } else {
            Message::FocusNext
        }),
        Key::Character(s) => {
            let c = s.chars().next()?;
            if is_dialer_char(c) {
                Some(Message::KeypadPressed(c))
            } else if c.eq_ignore_ascii_case(&'m') {
                Some(Message::ToggleMute)
            } else {
                None
            }
        }
        _ => None,
    }
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

/// Build the topbar — account dot + peer name + the live registration state.
///
/// POLISH-voicehud-loadstate — the registration state rides the shared
/// [`LoadState`] async-state vocabulary: it renders as a distinct icon SHAPE +
/// the registration label, so the state reads by icon+text and never by colour
/// alone (the a11y contract the old colour-only pip broke). The state's
/// [`StateTone`] is a *secondary* colour cue, every hue a `mde-theme` token
/// (§4). A Retry affordance appears whenever the state is recoverable
/// ([`LoadState::can_retry`]), wired to the persistent SIP agent's existing
/// re-REGISTER action. When the roster is the embedded fixture (not live mesh
/// data) a Degraded "fixture roster" caveat is surfaced rather than dropped.
fn build_topbar(state: &VoiceHud, appear: f32) -> Element<'_, Message> {
    let peer_name = local_peer_name();
    // The registration state, mapped onto the shared LoadState vocabulary so its
    // icon (shape) + tone (colour) come from the one source the whole shell uses.
    let reg_ls = registration_load_state(&state.registration);
    let registration_status = state.registration.label();
    let account_dot = container(
        text(account_initials(&peer_name))
            .size(13.0)
            .colr(fade_color(theme::ON_PRIMARY, appear)),
    )
    .sty(move |_: &Theme| cosmic::iced::widget::container::Style {
        background: Some(cosmic::iced::Background::Color(fade_color(
            theme::PRIMARY,
            appear,
        ))),
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

    // The state "pip" is now the LoadState icon glyph — a distinct shape carrying
    // the state, with the tone colour layered on as a secondary (non-sole) cue.
    let mut status_items: Vec<Element<Message>> = vec![
        text(reg_ls.icon().to_string())
            .size(11.0)
            .colr(fade_color(state_tone_color(reg_ls.tone()), appear))
            .into(),
        cosmic::iced::widget::space()
            .width(Length::Fixed(6.0))
            .into(),
        text(registration_status)
            .size(11.0)
            .colr(fade_color(theme::ON_SURF_VAR, appear))
            .into(),
    ];
    // Retry only when the state is recoverable (Failed) — the user can act.
    if reg_ls.can_retry() {
        status_items.push(
            cosmic::iced::widget::space()
                .width(Length::Fixed(8.0))
                .into(),
        );
        status_items.push(retry_chip(appear));
    }
    let status_line = row(status_items).align_y(cosmic::iced::Alignment::Center);

    let mut name_children: Vec<Element<Message>> = vec![
        text(peer_name)
            .size(14.0)
            .colr(fade_color(theme::ON_SURF, appear))
            .into(),
        status_line.into(),
    ];
    // Roster-source honesty: warn when the roster is the embedded fixture rather
    // than live mesh data (live mesh is the silent-good default — no caveat).
    if let Some(indicator) = roster_source_indicator(state.roster_source, appear) {
        name_children.push(indicator);
    }
    let name_col = column(name_children).spacing(2);

    row![
        account_dot,
        cosmic::iced::widget::space().width(Length::Fixed(12.0)),
        name_col,
    ]
    .align_y(cosmic::iced::Alignment::Center)
    .into()
}

/// POLISH-voicehud-loadstate — the topbar Retry affordance, shown only when the
/// registration is recoverable ([`LoadState::can_retry`]). Wired to
/// [`Message::RetryRegistration`], which asks the persistent SIP agent to
/// re-REGISTER (its existing register action). Every hue is a `mde-theme` token.
fn retry_chip<'a>(appear: f32) -> Element<'a, Message> {
    button(
        text("Retry")
            .size(11.0)
            .colr(fade_color(theme::ON_PRIMARY, appear)),
    )
    .on_press(Message::RetryRegistration)
    .padding(Padding::from([4, 10]))
    .sty(
        move |_: &Theme, _status| cosmic::iced::widget::button::Style {
            background: Some(cosmic::iced::Background::Color(fade_color(
                theme::PRIMARY,
                appear,
            ))),
            text_color: fade_color(theme::ON_PRIMARY, appear),
            border: cosmic::iced::Border {
                radius: cosmic::iced::border::Radius::from(12.0),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .into()
}

/// POLISH-voicehud-loadstate — the roster-source caveat indicator. A live mesh
/// source renders nothing (a `Loaded` roster is the silent-good default); the
/// embedded compile-time fixture renders a `Degraded` "fixture roster" chip
/// (icon shape + label + warning tone) so the operator can tell they're not on
/// live mesh data. Returns `None` for the live case so no empty row is added.
fn roster_source_indicator<'a>(source: RosterSource, appear: f32) -> Option<Element<'a, Message>> {
    let ls = roster_load_state(source);
    if ls == LoadState::Loaded {
        return None;
    }
    Some(
        row![
            text(ls.icon().to_string())
                .size(11.0)
                .colr(fade_color(state_tone_color(ls.tone()), appear)),
            cosmic::iced::widget::space().width(Length::Fixed(6.0)),
            text("fixture roster")
                .size(11.0)
                .colr(fade_color(theme::ON_SURF_MUTED, appear)),
        ]
        .align_y(cosmic::iced::Alignment::Center)
        .into(),
    )
}

/// Build the display + status strip. The text-input receives keypad/keyboard
/// input; below it sits the `resolve_target` classification chip — or, when the
/// dialer is empty (the resting state), the shared empty-state block instead of a
/// bare hint pill.
fn build_display<'a>(state: &VoiceHud) -> Element<'a, Message> {
    let display = text_input(
        "Type 1NNN for mesh, 9 + E.164 for PSTN",
        &state.dialer_input,
    )
    .on_input(Message::DialerInputChanged)
    .size(20.0)
    .padding(Padding::from([10, 12]))
    .width(Length::Fill);

    // In an established call the keypad sends DTMF tones rather than editing the
    // dial buffer — surface that so the keypad reads correctly (a touch-tone pad,
    // not a target editor). True for both outbound (local media) and inbound
    // (agent-thread media) calls. Otherwise classify the dialled target: an empty
    // dialer is the resting zero-state (the shared EmptyState), anything else a
    // status chip.
    let in_call = matches!(state.call, sip::CallState::InCall { .. });
    let status: Element<Message> = if in_call {
        chip("keypad · sends tones".to_string(), theme::INFO)
    } else {
        match resolve_target(&state.dialer_input, &state.roster) {
            Resolved::Empty => dialer_empty_state(state.account.is_none()),
            resolved => {
                let (label, fill) = resolved_chip_label_and_color(&resolved);
                chip(label, fill)
            }
        }
    };

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
        status,
    ]
    .spacing(8)
    .into()
}

/// One rounded status pill — `label` over `fill`, the label colour WCAG-picked
/// for that fill ([`theme::label_on`]) so it's never the old low-contrast
/// white-on-Green-40. The single pill builder the resolved-target chip and the
/// in-call DTMF hint (keypad sends DTMF touch-tones per RFC 4733, not dial edits)
/// now share — one chip, not the near-identical pills they were (§6 reuse). §4 —
/// every colour is an `mde-theme` token.
fn chip<'a>(label: String, fill: Color) -> Element<'a, Message> {
    container(text(label).size(12.0).colr(theme::label_on(fill)))
        .sty(move |_: &Theme| cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(fill)),
            border: cosmic::iced::Border {
                radius: cosmic::iced::border::Radius::from(12.0),
                ..Default::default()
            },
            ..Default::default()
        })
        .padding(Padding::from([4, 10]))
        .into()
}

/// The empty-dialer zero-state, built on the shared [`mde_theme::EmptyState`]
/// data shape rather than a bare hint pill (§6 — the one empty-state vocabulary
/// the shell already uses in mde-files / mde-music / the workbench). With no SIP
/// account configured it carries a "Configure account" CTA that launches the
/// `mde-voice-config` companion ([`Message::ConfigureAccount`]); with an account
/// it's an info-only nudge on how to dial.
fn dialer_empty_state<'a>(no_account: bool) -> Element<'a, Message> {
    let data = if no_account {
        mde_theme::EmptyState::with_cta(
            "No SIP account",
            "Dial a mesh peer by name, or configure an account to register for extensions and PSTN.",
            "Configure account",
        )
    } else {
        mde_theme::EmptyState::info(
            "Ready to dial",
            "Type 1NNN for a mesh peer, or 9 + E.164 for PSTN.",
        )
    };
    render_empty_state(&data, Message::ConfigureAccount)
}

/// Render an [`mde_theme::EmptyState`] as the HUD's compact zero-state block:
/// heading + muted body, and — when the data carries a CTA label — a primary
/// pill wired to `cta`. Like mde-files / mde-music, the HUD renders the shared
/// data shape with its own widgets (the workbench widget builder is shell-side,
/// §6). §4 — every colour is an `mde-theme` token; the inter-element gaps are the
/// `EmptyState` component tokens.
fn render_empty_state<'a>(data: &mde_theme::EmptyState, cta: Message) -> Element<'a, Message> {
    let mut col = column![
        text(data.heading.clone()).size(14.0).colr(theme::ON_SURF),
        text(data.body.clone())
            .size(12.0)
            .colr(theme::ON_SURF_MUTED),
    ]
    .spacing(mde_theme::components::HEADING_BODY_GAP)
    .align_x(cosmic::iced::Alignment::Center);
    if let Some(label) = data.cta_label.clone() {
        col = col
            .push(
                cosmic::iced::widget::space()
                    .height(Length::Fixed(mde_theme::components::BODY_CTA_GAP)),
            )
            .push(empty_state_cta(label, cta));
    }
    container(col)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding(Padding::from([8, 0]))
        .into()
}

/// The empty-state CTA — a primary-fill pill wired to `msg`, its label WCAG-
/// picked for the fill ([`theme::label_on`]) like every other chip. §4 tokens.
fn empty_state_cta<'a>(label: String, msg: Message) -> Element<'a, Message> {
    button(text(label).size(12.0).colr(theme::label_on(theme::PRIMARY)))
        .on_press(msg)
        .padding(Padding::from([6, 14]))
        .sty(
            move |_: &Theme, _status| cosmic::iced::widget::button::Style {
                background: Some(cosmic::iced::Background::Color(theme::PRIMARY)),
                text_color: theme::label_on(theme::PRIMARY),
                border: cosmic::iced::Border {
                    radius: cosmic::iced::border::Radius::from(12.0),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
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
    let mut col: Vec<Element<Message>> = Vec::with_capacity(5);
    for line in rows {
        let mut row_buf: Vec<Element<Message>> = Vec::with_capacity(3);
        for c in line {
            row_buf.push(keypad_button(state, now, c));
        }
        col.push(row(row_buf).spacing(8).into());
    }
    // A Backspace key in the bottom-right cell (under '#', the phone-keypad
    // convention): the two leading thirds are left empty so it aligns to the
    // keypad's right column. Gives the touch UI parity with the keyboard
    // Backspace (axis 10 — no keyboard-only erase).
    col.push(
        row![
            cosmic::iced::widget::space().width(Length::Fill),
            cosmic::iced::widget::space().width(Length::Fill),
            backspace_cell(state, now),
        ]
        .spacing(8)
        .into(),
    );
    column(col).spacing(8).into()
}

/// MOTION-FEEDBACK-1 — the control id for one keypad key (`key/<c>`), keying its
/// hover/press feedback in [`VoiceHud`].
fn keypad_id(c: char) -> String {
    format!("key/{c}")
}

/// POLISH-voicehud-focusring — the keyboard tab order: every control id that is
/// actually rendered + pressable in the current call state, in reading order
/// (the keypad, then the live call-bar actions). It mirrors [`build_keypad`] +
/// [`build_call_bar`] exactly, so Tab focus only ever lands on a control that is
/// really on screen and the ring always rings a real target (§7). The single
/// source the focus-advance and the per-control ring render both read.
fn focus_order(state: &VoiceHud) -> Vec<String> {
    let mut ids: Vec<String> = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '*', '0', '#']
        .into_iter()
        .map(keypad_id)
        .collect();
    ids.push("key/backspace".to_string());
    // Mirror build_call_bar's state machine (Incoming checked before is_active).
    if matches!(state.call, sip::CallState::Incoming { .. }) {
        ids.push("call/answer".to_string());
        ids.push("call/decline".to_string());
    } else if state.call.is_active() {
        // The Mute pill only renders once media is up, and sits before Hang up.
        if state.media.is_some() {
            ids.push("call/mute".to_string());
        }
        ids.push("call/hangup".to_string());
    } else if !state.dialer_input.trim().is_empty() {
        // The Call pill is only pressable (and so focusable) with a dialed target.
        ids.push("call/place".to_string());
    }
    ids
}

/// POLISH-voicehud-focusring — pure: the next focus id when tabbing through
/// `order`. Steps one place from `current` in the `forward` direction, wrapping
/// at the ends; a `current` of `None` (or one no longer in `order`, e.g. the
/// focused control just left the screen) restarts at the first entry going
/// forward / the last going backward. An empty order yields `None`. Split out so
/// the focus-advance logic is unit-testable without an event loop.
#[must_use]
fn advance_focus(order: &[String], current: Option<&str>, forward: bool) -> Option<String> {
    if order.is_empty() {
        return None;
    }
    let len = order.len();
    let next = match current.and_then(|c| order.iter().position(|id| id.as_str() == c)) {
        Some(i) if forward => (i + 1) % len,
        Some(i) => (i + len - 1) % len,
        None if forward => 0,
        None => len - 1,
    };
    Some(order[next].clone())
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

/// POLISH-voicehud-focusring — a control's border carrying the animated Carbon
/// focus ring. The keyboard-[`focused`](VoiceHud::focused) control gets the Blue
/// accent ring growing 0 → [`FOCUS_RING_WIDTH_PX`](mde_theme::feedback::FOCUS_RING_WIDTH_PX)
/// (the 2px Carbon weight, single-sourced with the Object Card), its opacity
/// rising with the width; every other control gets a zero-width (invisible)
/// border at `radius` — identical to the prior plain border. §4 — the hue reads
/// the [`theme::PRIMARY`] token and the width comes from the `mde-theme` ring
/// token, no raw literal. The single border builder the keypad keys + the call
/// pills share (§6 — one ring, applied identically).
fn focus_border(ring: FocusRing, radius: f32) -> cosmic::iced::Border {
    cosmic::iced::Border {
        radius: cosmic::iced::border::Radius::from(radius),
        // Alpha-only override of the accent token (carbon-lint sees this as a
        // token-derived spread, not a raw colour): the ring fades in with width.
        color: Color {
            a: ring.alpha,
            ..theme::PRIMARY
        },
        width: ring.width,
    }
}

/// One 3 × 4 keypad button — the digit/symbol cell, click fires
/// `Message::KeypadPressed(c)`. A thin wrapper over [`keypad_cell`] (the shared
/// key builder), so every key — digits and Backspace alike — has one look and one
/// feedback path.
fn keypad_button(state: &VoiceHud, now: Instant, c: char) -> Element<'_, Message> {
    keypad_cell(
        state,
        now,
        keypad_id(c),
        c.to_string(),
        Message::KeypadPressed(c),
    )
}

/// The on-screen Backspace key — the touch twin of the hardware Backspace, wired
/// to the same [`Message::Backspace`] so both erase paths behave identically. Sits
/// in the keypad's bottom-right cell (the phone-keypad convention) and carries the
/// shared key look + hover/press feedback.
fn backspace_cell(state: &VoiceHud, now: Instant) -> Element<'_, Message> {
    keypad_cell(
        state,
        now,
        "key/backspace".to_string(),
        "\u{232b}".to_string(), // ⌫ U+232B ERASE TO THE LEFT
        Message::Backspace,
    )
}

/// One keypad cell: a labelled key on the mid-elevation surface, wired to `msg`,
/// carrying the shared hover-lift + press-depress feedback (MOTION-FEEDBACK-1) and
/// the hover tint that survives reduce-motion. The single builder the digit/symbol
/// keys and the Backspace key share, so every key has one look + one feedback path
/// (§6 — reuse, not per-key reimplementation). Every colour is an `mde-theme`
/// token (§4).
fn keypad_cell(
    state: &VoiceHud,
    now: Instant,
    id: String,
    label: String,
    msg: Message,
) -> Element<'_, Message> {
    let hovered = state.hovered.as_deref() == Some(&id);
    let fb = state.control_feedback(&id, now);
    // POLISH-voicehud-focusring — the Carbon focus ring for this key (visible only
    // while it holds keyboard focus); rendered as the button's border.
    let ring = state.focus_ring(&id, now);
    // Hover tint is the non-motion cue (kept under reduce-motion, where the lift
    // is dropped): a hovered key brightens to the high-elevation surface step.
    let bg = if hovered {
        theme::SURF_C_HI
    } else {
        theme::SURF_C
    };
    let key = button(
        container(text(label).size(22.0).colr(theme::ON_SURF))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(cosmic::iced::alignment::Horizontal::Center)
            .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .on_press(msg)
    .width(Length::Fill)
    .height(Length::Fixed(56.0))
    .sty(
        move |_: &Theme, _status| cosmic::iced::widget::button::Style {
            background: Some(cosmic::iced::Background::Color(bg)),
            text_color: theme::ON_SURF,
            border: focus_border(ring, 8.0),
            ..Default::default()
        },
    );
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

/// A full-width call-action pill in `fill` with a `label_color` label, carrying
/// the shared hover-lift + press-depress feedback (MOTION-FEEDBACK-1). The fill
/// fades in on a call-mode change via the call-bar state crossfade. Every color
/// is an `mde-theme` Carbon token (§4).
fn call_pill<'a>(
    state: &VoiceHud,
    now: Instant,
    id: &str,
    label: &'a str,
    fill: Color,
    label_color: Color,
    msg: Message,
) -> Element<'a, Message> {
    let fill = fade_color(fill, state.call_state_alpha(now));
    let fb = state.control_feedback(id, now);
    // POLISH-voicehud-focusring — the Carbon focus ring for this pill, drawn as
    // its border when it holds keyboard focus.
    let ring = state.focus_ring(id, now);
    let pill = button(
        container(text(label).size(16.0).colr(label_color))
            .width(Length::Fill)
            .align_x(cosmic::iced::alignment::Horizontal::Center),
    )
    .on_press(msg)
    .width(Length::Fill)
    .height(Length::Fixed(48.0))
    .sty(
        move |_: &Theme, _status| cosmic::iced::widget::button::Style {
            background: Some(cosmic::iced::Background::Color(fill)),
            text_color: label_color,
            border: focus_border(ring, 8.0),
            ..Default::default()
        },
    );
    feedback_wrap(pill, fb, id.to_string())
}

/// The in-call mic-mute toggle pill — a [`call_pill`] whose fill/label flip on
/// `state.muted`: Carbon Blue-accent + light label when muted (the "active
/// toggle" affordance), Gray-80 + primary text when live, so the state reads at
/// a glance in dark + light. §4 — every color is an `mde-theme` Carbon token.
fn mute_pill<'a>(state: &VoiceHud, now: Instant) -> Element<'a, Message> {
    let (fill, label_color, label) = if state.muted {
        (theme::PRIMARY, theme::ON_PRIMARY, "Unmute")
    } else {
        (theme::SURF_C, theme::ON_SURF, "Mute")
    };
    call_pill(
        state,
        now,
        "call/mute",
        label,
        fill,
        label_color,
        Message::ToggleMute,
    )
}

/// The call-action row (Answer/Decline · Hang-up · Call) + live call status.
/// MOTION-TRANS — the action fill + status crossfade in on a call-mode change;
/// MOTION-FEEDBACK-1 — each pill carries the hover-lift + press-depress feedback.
fn build_call_bar(state: &VoiceHud, now: Instant) -> Element<'_, Message> {
    let alpha = state.call_state_alpha(now);
    let action: Element<Message> = if matches!(state.call, sip::CallState::Incoming { .. }) {
        row![
            call_pill(
                state,
                now,
                "call/answer",
                "Answer",
                theme::SUCCESS,
                theme::SURF,
                Message::Answer
            ),
            call_pill(
                state,
                now,
                "call/decline",
                "Decline",
                theme::ERROR,
                theme::SURF,
                Message::Decline
            ),
        ]
        .spacing(8)
        .into()
    } else if state.call.is_active() {
        // An active call shows Hang up; once media is up, a Mute toggle sits
        // beside it (muting stops mic transmit, peer audio keeps playing).
        let hangup = call_pill(
            state,
            now,
            "call/hangup",
            "Hang up",
            theme::ERROR,
            theme::SURF,
            Message::HangUp,
        );
        if state.media.is_some() {
            let mute = mute_pill(state, now);
            row![mute, hangup].spacing(8).into()
        } else {
            hangup
        }
    } else {
        let enabled = !state.dialer_input.trim().is_empty();
        let fill = fade_color(
            if enabled {
                theme::SUCCESS
            } else {
                theme::SURF_C
            },
            alpha,
        );
        let label_color = if enabled {
            theme::SURF
        } else {
            theme::ON_SURF_MUTED
        };
        // POLISH-voicehud-focusring — the Call pill's ring (only ever visible
        // when it is both enabled and keyboard-focused; a disabled Call is not in
        // the focus order, so its ring stays invisible).
        let ring = state.focus_ring("call/place", now);
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
                border: focus_border(ring, 8.0),
                ..Default::default()
            },
        );
        if enabled {
            b = b.on_press(Message::PlaceCall);
        }
        // The Call pill only takes feedback when it's actually pressable.
        if enabled {
            feedback_wrap(
                b,
                state.control_feedback("call/place", now),
                "call/place".to_string(),
            )
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

/// If `new` is `old` with exactly one dialer char appended (a single keystroke
/// into the focused dialer field), return that char. Used in-call to route a
/// typed digit to DTMF while keeping the (frozen) buffer unchanged. `None` for
/// any other edit (backspace, paste, multi-char) so only single keystrokes tone.
#[must_use]
pub fn appended_char(old: &str, new: &str) -> Option<char> {
    let extra = new.strip_prefix(old)?;
    let mut chars = extra.chars();
    let c = chars.next()?;
    if chars.next().is_none() && is_dialer_char(c) {
        Some(c)
    } else {
        None
    }
}

/// POLISH-voicehud-loadstate — map the SIP [`sip::RegistrationState`] onto the
/// shared [`LoadState`] async-state vocabulary.
///
/// The topbar then renders the state with `LoadState`'s icon (a non-colour-only
/// shape cue) + tone, and offers a Retry whenever [`LoadState::can_retry`].
/// `NoAccount` is the resting `Idle` (nothing to register → no retry); a REGISTER
/// in flight is `Loading`; a 200 OK is `Loaded`; a terminal failure is `Failed`
/// (retriable). Pure + testable.
#[must_use]
pub const fn registration_load_state(reg: &sip::RegistrationState) -> LoadState {
    match reg {
        sip::RegistrationState::NoAccount => LoadState::Idle,
        sip::RegistrationState::Registering => LoadState::Loading,
        sip::RegistrationState::Registered { .. } => LoadState::Loaded,
        sip::RegistrationState::Failed(_) => LoadState::Failed,
    }
}

/// POLISH-voicehud-loadstate — map the roster [`RosterSource`] onto a
/// [`LoadState`].
///
/// Every live source is `Loaded`; the embedded compile-time fixture is `Degraded`
/// (usable, but NOT live mesh data) so the topbar can warn the operator they're
/// seeing a fixture roster instead of silently dropping the source. Pure +
/// testable.
#[must_use]
pub const fn roster_load_state(source: RosterSource) -> LoadState {
    match source {
        RosterSource::EmbeddedFixture => LoadState::Degraded,
        RosterSource::MeshDirectory
        | RosterSource::EnvOverride
        | RosterSource::MeshStorage
        | RosterSource::LocalFallback => LoadState::Loaded,
    }
}

/// POLISH-voicehud-loadstate — the `mde-theme` Carbon colour token for a
/// [`StateTone`].
///
/// §4 — every hue reads a token, never a raw literal. The tone is the *secondary*
/// cue; the state's icon + label carry it without colour.
#[must_use]
pub const fn state_tone_color(tone: StateTone) -> Color {
    match tone {
        StateTone::Neutral => theme::ON_SURF_MUTED,
        StateTone::Info => theme::INFO,
        StateTone::Warning => theme::WARNING,
        StateTone::Danger => theme::ERROR,
        StateTone::Success => theme::SUCCESS,
    }
}

/// Pure: the action an Escape key press maps to, given the live call state.
///
/// A live call is never killed by Esc — Esc Declines a ringing call and Hangs up
/// an active one (reusing the existing call-state messages); only an idle/ended
/// HUD exits. Split out so the keyboard→message routing is unit-testable.
#[must_use]
pub const fn escape_action(call: &sip::CallState) -> Message {
    match CallKind::of(call) {
        CallKind::Incoming => Message::Decline,
        CallKind::Calling | CallKind::InCall => Message::HangUp,
        CallKind::Idle | CallKind::Ended => Message::Exit,
    }
}

/// Pure: the action the Enter/confirm key maps to.
///
/// Answers a ringing call, otherwise places the dialed call (`PlaceCall`
/// self-guards an empty buffer); `None` while a call is connecting or up (Enter
/// has no in-call action).
#[must_use]
pub const fn enter_action(call: &sip::CallState) -> Option<Message> {
    match CallKind::of(call) {
        CallKind::Incoming => Some(Message::Answer),
        CallKind::Idle | CallKind::Ended => Some(Message::PlaceCall),
        CallKind::Calling | CallKind::InCall => None,
    }
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
                    roster_source: source,
                    registration,
                    account,
                    call: sip::CallState::Idle,
                    session: None,
                    media: None,
                    anim,
                    reduce_motion,
                    pressed: None,
                    hovered: None,
                    focused: None,
                    focus_since: Instant::now(),
                    call_kind: CallKind::Idle,
                    muted: false,
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
            roster_source: roster::RosterSource::EmbeddedFixture,
            registration: sip::RegistrationState::NoAccount,
            account: None,
            call: sip::CallState::Idle,
            session: None,
            media: None,
            anim: Animator::new(),
            reduce_motion: false,
            pressed: None,
            hovered: None,
            focused: None,
            focus_since: Instant::now(),
            call_kind: CallKind::Idle,
            muted: false,
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
    fn toggle_mute_is_noop_without_media() {
        // The Mute pill only renders once media is up; a stray toggle with no
        // session must not flip the flag (the pill isn't shown to click anyway).
        let mut hud = make_hud();
        hud.call = sip::CallState::InCall {
            peer: "pine".into(),
        };
        assert!(hud.media.is_none());
        let _ = update(&mut hud, Message::ToggleMute);
        assert!(!hud.muted, "no media → mute flag stays false");
    }

    #[test]
    fn hangup_clears_mute_so_next_call_starts_unmuted() {
        // A muted call that hangs up must reset the flag — the next call's Mute
        // pill should read "Mute", not inherit the prior call's muted state.
        let mut hud = make_hud();
        hud.call = sip::CallState::InCall {
            peer: "pine".into(),
        };
        hud.muted = true;
        let _ = update(&mut hud, Message::HangUp);
        assert!(!hud.muted, "hang-up clears the mute flag");
        assert!(matches!(hud.call, sip::CallState::Ended));
    }

    #[test]
    fn remote_hangup_clears_mute() {
        // A peer-initiated BYE must also reset mute (same next-call invariant).
        let mut hud = make_hud();
        hud.call = sip::CallState::InCall {
            peer: "pine".into(),
        };
        hud.muted = true;
        let _ = update(&mut hud, Message::Agent(sip::AgentEvent::RemoteHangup));
        assert!(!hud.muted, "remote hang-up clears the mute flag");
        assert!(matches!(hud.call, sip::CallState::Ended));
    }

    #[test]
    fn remote_hangup_tears_down_media_so_no_stale_pill_next_call() {
        // The mute pill is gated on `state.media.is_some()`. If a peer-initiated
        // BYE left a stale MediaSession behind, the next call's Calling phase
        // would render a Mute pill backed by a dead session (a mic-live-but-UI-
        // muted divergence). RemoteHangup must take + stop the media, mirroring
        // the local HangUp path.
        let peer = std::net::UdpSocket::bind(("127.0.0.1", 0)).expect("bind loopback peer");
        let peer_addr = peer.local_addr().expect("peer addr");
        let remote = sip::RemoteMedia {
            addr: peer_addr.ip().to_string(),
            port: peer_addr.port(),
            payload_type: 0,
            telephone_event_pt: Some(101),
        };
        let mut hud = make_hud();
        hud.call = sip::CallState::InCall {
            peer: "pine".into(),
        };
        hud.media = Some(media::start_media(0, &remote).expect("media starts"));
        let _ = update(&mut hud, Message::Agent(sip::AgentEvent::RemoteHangup));
        assert!(
            hud.media.is_none(),
            "remote hang-up tears down the media session"
        );
    }

    #[test]
    fn keypad_sends_dtmf_in_call_not_appending_to_the_dialer() {
        // While a call is up with media, a keypad digit is a DTMF tone — it must
        // NOT mutate the dial buffer (the buffer holds the dialed peer, not the
        // in-call tones). Stand up a real loopback media session so the in-call
        // branch is exercised end-to-end (send_dtmf → the live MediaSession).
        let peer = std::net::UdpSocket::bind(("127.0.0.1", 0)).expect("bind loopback peer");
        let peer_addr = peer.local_addr().expect("peer addr");
        let remote = sip::RemoteMedia {
            addr: peer_addr.ip().to_string(),
            port: peer_addr.port(),
            payload_type: 0,
            telephone_event_pt: Some(101),
        };
        let mut hud = make_hud();
        hud.call = sip::CallState::InCall {
            peer: "pine".into(),
        };
        hud.dialer_input = "pine".into();
        hud.media = Some(media::start_media(0, &remote).expect("media starts"));
        let _ = update(&mut hud, Message::KeypadPressed('7'));
        assert_eq!(
            hud.dialer_input, "pine",
            "an in-call keypad press is DTMF, not a dial-buffer edit"
        );
    }

    #[test]
    fn keypad_appends_to_the_dialer_when_idle() {
        // Idle (no call): the keypad edits the dial buffer as before.
        let mut hud = make_hud();
        assert!(matches!(hud.call, sip::CallState::Idle));
        let _ = update(&mut hud, Message::KeypadPressed('4'));
        let _ = update(&mut hud, Message::KeypadPressed('2'));
        assert_eq!(hud.dialer_input, "42", "idle keypad fills the dialer");
    }

    #[test]
    fn in_call_typed_digit_does_not_edit_the_frozen_dial_buffer() {
        // A focused text-input CAPTURES typed digits, so an in-call typed digit
        // arrives as DialerInputChanged (the keypad subscription never sees it).
        // It must route to DTMF and leave the (frozen) buffer untouched — the
        // same behavior as an on-screen keypad press. (No media here → the agent
        // path is taken; the buffer invariant is what we assert.)
        let mut hud = make_hud();
        hud.call = sip::CallState::InCall {
            peer: "pine".into(),
        };
        hud.dialer_input = "pine".into();
        // The text-input emits the full new contents = old + one digit.
        let _ = update(&mut hud, Message::DialerInputChanged("pine7".into()));
        assert_eq!(
            hud.dialer_input, "pine",
            "in-call typed digit is DTMF, not a frozen-buffer edit"
        );
    }

    #[test]
    fn appended_char_detects_single_keystroke_only() {
        assert_eq!(appended_char("12", "123"), Some('3'));
        assert_eq!(appended_char("", "5"), Some('5'));
        // A letter-bearing frozen buffer (a dialed peer name) + one typed digit
        // — the digit is the appended char (the common in-call case).
        assert_eq!(appended_char("pine", "pine7"), Some('7'));
        assert_eq!(appended_char("pine", "pine#"), Some('#'));
        // Not a single appended dialer char:
        assert_eq!(appended_char("12", "12"), None, "no change");
        assert_eq!(appended_char("123", "12"), None, "backspace");
        assert_eq!(appended_char("1", "199"), None, "two chars (paste)");
        assert_eq!(appended_char("ab", "abc"), None, "non-dialer char");
        assert_eq!(appended_char("12", "1x3"), None, "mid-edit, not a suffix");
        assert_eq!(
            appended_char("pine", "piney"),
            None,
            "appended letter, no tone"
        );
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
        hud.anim
            .start(ANIM_APPEAR, now, Motion::panel_mount(), true);
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
        hud.call = sip::CallState::Calling {
            peer: "pine".into(),
        };
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
        hud.call = sip::CallState::Ringing {
            peer: "pine".into(),
        };
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
            assert_eq!(
                g.translate_y, 0.0,
                "no hover-lift under reduce-motion @{ms}ms"
            );
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
        assert!(
            (up.scale - 1.0).abs() < 1e-6,
            "release restores natural size"
        );
    }

    #[test]
    fn anim_tick_gcs_settled_tweens_and_goes_idle() {
        // MOTION-PERF-1 — the tick advances/GCs the animator; once everything is
        // settled the animator is idle so the subscription stops (no idle CPU).
        let now = Instant::now();
        let mut hud = make_hud();
        hud.anim
            .start(ANIM_APPEAR, now, Motion::panel_mount(), false);
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
            (0.0, 1.0),                                    // at rest
            (-mde_theme::feedback::HOVER_LIFT_PX, 1.0),    // hovered
            (0.0, 1.0 - mde_theme::feedback::PRESS_DEPTH), // pressed
            (
                -mde_theme::feedback::HOVER_LIFT_PX,
                1.0 - mde_theme::feedback::PRESS_DEPTH,
            ), // both
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

    // ── POLISH-voicehud-callkeys — keyboard call control ──────────────────────

    #[test]
    fn escape_never_kills_a_live_call() {
        // The bug fix: Esc must Decline a ring / Hang up an active call, and only
        // exit when there is no live call. The pure mapping IS the routing — the
        // handler is `Task::done(escape_action(..))`.
        assert!(matches!(
            escape_action(&sip::CallState::Incoming {
                from: "pine".into()
            }),
            Message::Decline
        ));
        assert!(matches!(
            escape_action(&sip::CallState::Calling {
                peer: "pine".into()
            }),
            Message::HangUp
        ));
        assert!(matches!(
            escape_action(&sip::CallState::Ringing {
                peer: "pine".into()
            }),
            Message::HangUp
        ));
        assert!(matches!(
            escape_action(&sip::CallState::InCall {
                peer: "pine".into()
            }),
            Message::HangUp
        ));
        // Only an idle / ended / failed HUD exits.
        assert!(matches!(
            escape_action(&sip::CallState::Idle),
            Message::Exit
        ));
        assert!(matches!(
            escape_action(&sip::CallState::Ended),
            Message::Exit
        ));
        assert!(matches!(
            escape_action(&sip::CallState::Failed("x".into())),
            Message::Exit
        ));
    }

    #[test]
    fn enter_answers_a_ring_else_places_the_call() {
        assert!(matches!(
            enter_action(&sip::CallState::Incoming {
                from: "pine".into()
            }),
            Some(Message::Answer)
        ));
        assert!(matches!(
            enter_action(&sip::CallState::Idle),
            Some(Message::PlaceCall)
        ));
        assert!(matches!(
            enter_action(&sip::CallState::Ended),
            Some(Message::PlaceCall)
        ));
        // No confirm action while a call is connecting or up.
        assert!(enter_action(&sip::CallState::Calling { peer: "p".into() }).is_none());
        assert!(enter_action(&sip::CallState::Ringing { peer: "p".into() }).is_none());
        assert!(enter_action(&sip::CallState::InCall { peer: "p".into() }).is_none());
    }

    #[test]
    fn key_to_message_maps_call_control_and_dialer_keys() {
        use cosmic::iced::keyboard::{key::Named, Key};
        // Call-control intents + erase (no modifier).
        assert!(matches!(
            key_to_message(&Key::Named(Named::Escape), false),
            Some(Message::Escape)
        ));
        assert!(matches!(
            key_to_message(&Key::Named(Named::Enter), false),
            Some(Message::Enter)
        ));
        assert!(matches!(
            key_to_message(&Key::Named(Named::Backspace), false),
            Some(Message::Backspace)
        ));
        // 'm' / 'M' is the in-call mute accelerator (case-insensitive).
        assert!(matches!(
            key_to_message(&Key::Character("m"), false),
            Some(Message::ToggleMute)
        ));
        assert!(matches!(
            key_to_message(&Key::Character("M"), false),
            Some(Message::ToggleMute)
        ));
        // Dialer chars still type into the pad.
        assert!(matches!(
            key_to_message(&Key::Character("5"), false),
            Some(Message::KeypadPressed('5'))
        ));
        assert!(matches!(
            key_to_message(&Key::Character("#"), false),
            Some(Message::KeypadPressed('#'))
        ));
        // POLISH-voicehud-focusring — Tab now walks the keyboard focus ring
        // (Shift+Tab reverses); it is no longer an ignored no-op.
        assert!(matches!(
            key_to_message(&Key::Named(Named::Tab), false),
            Some(Message::FocusNext)
        ));
        assert!(matches!(
            key_to_message(&Key::Named(Named::Tab), true),
            Some(Message::FocusPrev)
        ));
        // A non-accelerator letter is still ignored (no dial-buffer pollution).
        assert!(key_to_message(&Key::Character("z"), false).is_none());
    }

    #[test]
    fn backspace_does_not_edit_the_frozen_in_call_buffer() {
        // The dialer shows the peer while a call is up; the new on-screen (and the
        // existing hardware) Backspace must not corrupt it.
        let mut hud = make_hud();
        hud.call = sip::CallState::InCall {
            peer: "pine".into(),
        };
        hud.dialer_input = "pine".into();
        let _ = update(&mut hud, Message::Backspace);
        assert_eq!(hud.dialer_input, "pine", "in-call Backspace is a no-op");
        // Idle, it still erases the last char.
        hud.call = sip::CallState::Idle;
        let _ = update(&mut hud, Message::Backspace);
        assert_eq!(hud.dialer_input, "pin");
    }

    // ── POLISH-voicehud-focusring — keyboard focus ring ───────────────────────

    #[test]
    fn advance_focus_walks_the_order_and_wraps() {
        // The pure focus-id advance logic: forward from None starts at the first,
        // backward from None at the last, and stepping wraps at both ends.
        let order: Vec<String> = ["a", "b", "c"].into_iter().map(String::from).collect();
        // None ⇒ the appropriate end.
        assert_eq!(advance_focus(&order, None, true).as_deref(), Some("a"));
        assert_eq!(advance_focus(&order, None, false).as_deref(), Some("c"));
        // Step forward through the list.
        assert_eq!(advance_focus(&order, Some("a"), true).as_deref(), Some("b"));
        assert_eq!(advance_focus(&order, Some("b"), true).as_deref(), Some("c"));
        // Wrap forward at the end.
        assert_eq!(advance_focus(&order, Some("c"), true).as_deref(), Some("a"));
        // Step + wrap backward.
        assert_eq!(
            advance_focus(&order, Some("b"), false).as_deref(),
            Some("a")
        );
        assert_eq!(
            advance_focus(&order, Some("a"), false).as_deref(),
            Some("c")
        );
        // A current id that left the order restarts at the appropriate end (so a
        // control vanishing — e.g. the call bar changing — never strands focus).
        assert_eq!(
            advance_focus(&order, Some("gone"), true).as_deref(),
            Some("a")
        );
        assert_eq!(
            advance_focus(&order, Some("gone"), false).as_deref(),
            Some("c")
        );
        // An empty order has nothing to focus.
        assert_eq!(advance_focus(&[], Some("a"), true), None);
    }

    #[test]
    fn focus_order_tracks_the_live_call_state() {
        // focus_order lists exactly the controls on screen, so a Tab can only ever
        // ring a real target (§7). The keypad (12 keys + Backspace) is always
        // present; the call-bar tail follows the call state, mirroring
        // build_call_bar.
        let mut hud = make_hud();
        // Idle + empty dialer ⇒ keypad only (the Call pill isn't pressable yet).
        let idle = focus_order(&hud);
        assert_eq!(idle.len(), 13, "12 keys + Backspace");
        assert_eq!(idle.first().map(String::as_str), Some("key/1"));
        assert_eq!(idle.last().map(String::as_str), Some("key/backspace"));
        assert!(!idle.iter().any(|id| id == "call/place"));
        // Idle + a dialed target ⇒ the Call pill joins the order.
        hud.dialer_input = "1003".into();
        assert!(focus_order(&hud).iter().any(|id| id == "call/place"));
        // A ringing inbound call ⇒ Answer + Decline, no Call pill.
        hud.dialer_input.clear();
        hud.call = sip::CallState::Incoming {
            from: "pine".into(),
        };
        let inc = focus_order(&hud);
        assert!(inc.iter().any(|id| id == "call/answer"));
        assert!(inc.iter().any(|id| id == "call/decline"));
        assert!(!inc.iter().any(|id| id == "call/place"));
        // An active call ⇒ Hang up; Mute only joins once media is up (it isn't).
        hud.call = sip::CallState::InCall {
            peer: "pine".into(),
        };
        let active = focus_order(&hud);
        assert!(active.iter().any(|id| id == "call/hangup"));
        assert!(
            !active.iter().any(|id| id == "call/mute"),
            "no Mute pill without a live media session"
        );
    }

    #[test]
    fn tab_advances_keyboard_focus_and_rings_the_focused_control() {
        // POLISH-voicehud-focusring — Tab lands focus on the first control and
        // arms its ring grow-in; the focused control draws a visible 2px Carbon
        // ring (once grown in) and no other does. §7 — the ring tracks REAL focus.
        let mut hud = make_hud();
        assert!(hud.focused.is_none(), "nothing focused at rest");
        assert!(
            !hud.focus_ring("key/1", Instant::now()).is_visible(),
            "no ring before any focus"
        );

        let _ = update(&mut hud, Message::FocusNext);
        assert_eq!(
            hud.focused.as_deref(),
            Some("key/1"),
            "Tab lands on the first control"
        );
        // The focus-ring grow-in tween is armed, so the tick subscription runs
        // while it animates in (and only then — MOTION-PERF-1).
        assert!(
            hud.anim.is_animating(ANIM_FOCUS, hud.focus_since),
            "focus-ring grow-in is armed"
        );
        // Sampled at the end of the grow-in the focused control's ring is fully
        // drawn at the Carbon 2px weight; an unfocused control draws nothing.
        let settled = hud.focus_since + Motion::focus().duration;
        let focused = hud.focused.clone().unwrap();
        let ring = hud.focus_ring(&focused, settled);
        assert!(ring.is_visible(), "the focused control is ringed");
        assert!(
            (ring.width - mde_theme::feedback::FOCUS_RING_WIDTH_PX).abs() < 1e-4,
            "ring grows to the 2px Carbon weight, got {}",
            ring.width
        );
        assert!(
            !hud.focus_ring("key/2", settled).is_visible(),
            "an unfocused control draws no ring"
        );

        // Tab again advances the ring to the next control (and off the first).
        let _ = update(&mut hud, Message::FocusNext);
        assert_eq!(hud.focused.as_deref(), Some("key/2"));
        assert!(
            !hud.focus_ring("key/1", settled).is_visible(),
            "the ring left the previously-focused control"
        );
        // Shift+Tab walks focus back.
        let _ = update(&mut hud, Message::FocusPrev);
        assert_eq!(hud.focused.as_deref(), Some("key/1"));
    }

    #[test]
    fn focus_ring_is_present_immediately_under_reduce_motion() {
        // The helper's reduce-motion contract at the HUD level: the ring is the
        // focus cue, so it is present (at full width) the instant focus arrives —
        // it simply does not animate in.
        let mut hud = make_hud();
        hud.reduce_motion = true;
        let _ = update(&mut hud, Message::FocusNext);
        let focused = hud.focused.clone().unwrap();
        let ring = hud.focus_ring(&focused, hud.focus_since);
        assert!(
            ring.is_visible(),
            "focus ring present immediately under reduce-motion"
        );
        assert!((ring.width - mde_theme::feedback::FOCUS_RING_WIDTH_PX).abs() < 1e-4);
    }

    // ── POLISH-voicehud-loadstate — registration/roster load-state mapping ─────

    #[test]
    fn registration_maps_onto_the_shared_loadstate_vocabulary() {
        use sip::RegistrationState as R;
        assert_eq!(registration_load_state(&R::NoAccount), LoadState::Idle);
        assert_eq!(registration_load_state(&R::Registering), LoadState::Loading);
        assert_eq!(
            registration_load_state(&R::Registered {
                server: "sip.example.com:5060".into(),
                expires: 60,
            }),
            LoadState::Loaded
        );
        assert_eq!(
            registration_load_state(&R::Failed("registrar unreachable".into())),
            LoadState::Failed
        );
    }

    #[test]
    fn only_a_failed_registration_offers_retry() {
        // The Retry affordance is gated on `can_retry()`, so it appears exactly
        // for the recoverable (Failed) state — never mid-register or when live.
        use sip::RegistrationState as R;
        assert!(registration_load_state(&R::Failed("x".into())).can_retry());
        assert!(!registration_load_state(&R::Registering).can_retry());
        assert!(!registration_load_state(&R::NoAccount).can_retry());
        assert!(!registration_load_state(&R::Registered {
            server: "s".into(),
            expires: 1,
        })
        .can_retry());
    }

    #[test]
    fn every_registration_state_reads_as_a_distinct_shape_not_colour() {
        // The a11y fix: each state carries a distinct icon SHAPE (via LoadState),
        // so it is never conveyed by colour alone.
        use sip::RegistrationState as R;
        let states = [
            R::NoAccount,
            R::Registering,
            R::Registered {
                server: "s".into(),
                expires: 1,
            },
            R::Failed("x".into()),
        ];
        let icons: Vec<char> = states
            .iter()
            .map(|s| registration_load_state(s).icon())
            .collect();
        for (i, a) in icons.iter().enumerate() {
            for b in &icons[i + 1..] {
                assert_ne!(a, b, "registration icons must be distinct shapes");
            }
        }
    }

    #[test]
    fn roster_fixture_is_degraded_and_live_sources_are_loaded() {
        assert_eq!(
            roster_load_state(RosterSource::EmbeddedFixture),
            LoadState::Degraded
        );
        for live in [
            RosterSource::MeshDirectory,
            RosterSource::EnvOverride,
            RosterSource::MeshStorage,
            RosterSource::LocalFallback,
        ] {
            assert_eq!(
                roster_load_state(live),
                LoadState::Loaded,
                "{live:?} is live"
            );
        }
        // The fixture caveat reads as a Warning tone (usable-but-not-live), and a
        // live roster is the silent-good default (no indicator).
        assert_eq!(
            roster_load_state(RosterSource::EmbeddedFixture).tone(),
            StateTone::Warning
        );
        assert!(roster_source_indicator(RosterSource::EmbeddedFixture, 1.0).is_some());
        assert!(roster_source_indicator(RosterSource::MeshDirectory, 1.0).is_none());
    }

    #[test]
    fn retry_flips_registration_optimistically_in_flight() {
        // Clicking Retry shows the in-flight (Loading) Registering state at once;
        // the agent's real result then lands as a later Registration event.
        let mut hud = make_hud();
        hud.registration = sip::RegistrationState::Failed("registrar unreachable".into());
        let _ = update(&mut hud, Message::RetryRegistration);
        assert!(matches!(
            hud.registration,
            sip::RegistrationState::Registering
        ));
        assert_eq!(
            registration_load_state(&hud.registration),
            LoadState::Loading
        );
    }

    #[test]
    fn state_tone_colours_are_distinct_carbon_tokens() {
        // §4 — each tone resolves to a distinct mde-theme Carbon token (the
        // secondary colour cue), and danger/success/warning map to the expected
        // status tokens.
        let danger = state_tone_color(StateTone::Danger);
        let success = state_tone_color(StateTone::Success);
        let warning = state_tone_color(StateTone::Warning);
        let triple = |c: Color| (c.r, c.g, c.b);
        assert_ne!(triple(danger), triple(success));
        assert_ne!(triple(warning), triple(success));
        assert_ne!(triple(warning), triple(danger));
        assert_eq!(triple(danger), triple(theme::ERROR));
        assert_eq!(triple(success), triple(theme::SUCCESS));
        assert_eq!(triple(warning), triple(theme::WARNING));
    }
}
