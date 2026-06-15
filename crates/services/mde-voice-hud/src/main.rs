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
                    .args(["-a", "Magic Mesh Voice", "Incoming call", &from])
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
    }
    Task::none()
}

/// iced view — renders the HUD overlay surface.
pub fn view(state: &VoiceHud, _id: window::Id) -> Element<'_, Message> {
    container(
        column![
            build_topbar(state),
            build_display(state),
            build_keypad(),
            build_call_bar(state),
        ]
        .spacing(12),
    )
    .padding(Padding::from([16, 16]))
    .width(Length::Fill)
    .height(Length::Fill)
    .sty(|_: &Theme| cosmic::iced::widget::container::Style {
        background: Some(cosmic::iced::Background::Color(theme::SURF)),
        ..Default::default()
    })
    .into()
}

fn subscription(_state: &VoiceHud) -> cosmic::iced::Subscription<Message> {
    cosmic::iced::Subscription::batch([
        keyboard_subscription(),
        agent_subscription(),
        dial_subscription(),
    ])
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
fn build_topbar(state: &VoiceHud) -> Element<'_, Message> {
    let peer_name = local_peer_name();
    let pip_color = if state.registration.is_online() {
        theme::PRESENCE_AVAILABLE
    } else {
        theme::PRESENCE_OFFLINE
    };
    let registration_status = state.registration.label();
    let account_dot = container(
        text(account_initials(&peer_name))
            .size(13.0)
            .colr(theme::ON_PRIMARY),
    )
    .sty(|_: &Theme| cosmic::iced::widget::container::Style {
        background: Some(cosmic::iced::Background::Color(theme::PRIMARY)),
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
        text(peer_name).size(14.0).colr(theme::ON_SURF),
        row![
            presence_pip,
            cosmic::iced::widget::space().width(Length::Fixed(6.0)),
            text(registration_status)
                .size(11.0)
                .colr(theme::ON_SURF_VAR),
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
/// input.
fn build_keypad<'a>() -> Element<'a, Message> {
    let rows: [[char; 3]; 4] = [
        ['1', '2', '3'],
        ['4', '5', '6'],
        ['7', '8', '9'],
        ['*', '0', '#'],
    ];
    let mut col: Vec<Element<'a, Message>> = Vec::with_capacity(4);
    for line in rows {
        let mut row_buf: Vec<Element<'a, Message>> = Vec::with_capacity(3);
        for c in line {
            row_buf.push(keypad_button(c));
        }
        col.push(row(row_buf).spacing(8).into());
    }
    column(col).spacing(8).into()
}

/// One 3 × 4 keypad button. Renders the digit/symbol on a
/// surface-container background; click fires
/// `Message::KeypadPressed(c)`.
fn keypad_button<'a>(c: char) -> Element<'a, Message> {
    button(
        container(text(c.to_string()).size(22.0).colr(theme::ON_SURF))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(cosmic::iced::alignment::Horizontal::Center)
            .align_y(cosmic::iced::alignment::Vertical::Center),
    )
    .on_press(Message::KeypadPressed(c))
    .width(Length::Fill)
    .height(Length::Fixed(56.0))
    .sty(|_: &Theme, _status| cosmic::iced::widget::button::Style {
        background: Some(cosmic::iced::Background::Color(theme::SURF_C)),
        text_color: theme::ON_SURF,
        border: cosmic::iced::Border {
            radius: cosmic::iced::border::Radius::from(8.0),
            ..Default::default()
        },
        ..Default::default()
    })
    .into()
}

/// A full-width call-action pill in `fill` with a white label.
fn call_pill<'a>(label: &'a str, fill: Color, msg: Message) -> Element<'a, Message> {
    button(
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
    )
    .into()
}

/// The call-action row (Answer/Decline · Hang-up · Call) + live call status.
fn build_call_bar(state: &VoiceHud) -> Element<'_, Message> {
    let action: Element<Message> = if matches!(state.call, sip::CallState::Incoming { .. }) {
        row![
            call_pill("Answer", theme::SUCCESS, Message::Answer),
            call_pill("Decline", theme::ERROR, Message::Decline),
        ]
        .spacing(8)
        .into()
    } else if state.call.is_active() {
        button(
            container(text("Hang up").size(16.0).colr(theme::SURF))
                .width(Length::Fill)
                .align_x(cosmic::iced::alignment::Horizontal::Center),
        )
        .on_press(Message::HangUp)
        .width(Length::Fill)
        .height(Length::Fixed(48.0))
        .sty(|_: &Theme, _status| cosmic::iced::widget::button::Style {
            background: Some(cosmic::iced::Background::Color(theme::ERROR)),
            text_color: theme::SURF,
            border: cosmic::iced::Border {
                radius: cosmic::iced::border::Radius::from(8.0),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
    } else {
        let enabled = !state.dialer_input.trim().is_empty();
        let mut b = button(
            container(text("Call").size(16.0).colr(if enabled {
                theme::SURF
            } else {
                theme::ON_SURF_MUTED
            }))
            .width(Length::Fill)
            .align_x(cosmic::iced::alignment::Horizontal::Center),
        )
        .width(Length::Fill)
        .height(Length::Fixed(48.0))
        .sty(
            move |_: &Theme, _status| cosmic::iced::widget::button::Style {
                background: Some(cosmic::iced::Background::Color(if enabled {
                    theme::SUCCESS
                } else {
                    theme::SURF_C
                })),
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
        b.into()
    };
    column![
        action,
        text(state.call.label()).size(11.0).colr(theme::ON_SURF_VAR),
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
    let Some(acct) = sip::SipAccount::load() else {
        tracing::info!("voice agent: no account.toml — publishing unregistered heartbeat");
        loop {
            sip::publish_voice_status(&sip::RegistrationState::NoAccount, false);
            std::thread::sleep(std::time::Duration::from_secs(sip::STATUS_HEARTBEAT_SECS));
        }
    };
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
            let registration = match account.clone() {
                Some(acct) => {
                    tracing::info!(server = %acct.server_host, "voice-hud: starting SIP agent");
                    // Spawn the persistent SIP agent (registration + re-register
                    // + inbound INVITE/BYE) on its own thread, bridged to the UI
                    // via the AGENT_* channels (events ← agent, commands → agent).
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
                        .spawn(move || sip::run_agent(&acct, &event_tx, &cmd_rx));
                    sip::RegistrationState::Registering
                }
                None => sip::RegistrationState::NoAccount,
            };
            (
                VoiceHud {
                    dialer_input: String::new(),
                    roster: peers,
                    registration,
                    account,
                    call: sip::CallState::Idle,
                    session: None,
                    media: None,
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
}
