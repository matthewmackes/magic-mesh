//! The background workers (E12-11): the threads that own the blocking, !Send SIP
//! work so the egui UI thread never blocks on a socket or a sound device.
//!
//! `mde-voice-hud`'s SIP agent ([`sip::run_agent`]) is itself a blocking loop
//! that owns its UDP socket and its own command channel, so the worker is three
//! cooperating threads rather than music's single one:
//!
//! * the **agent** thread runs [`sip::run_agent`] — REGISTER + inbound
//!   INVITE/BYE + Answer/Decline/HangUp, emitting [`AgentEvent`]s;
//! * the **bridge** thread translates each [`AgentEvent`] into a model [`Update`]
//!   and wakes the UI with [`Context::request_repaint`];
//! * the **command** thread services UI [`Command`]s — forwarding
//!   Answer/Decline/HangUp/Reregister to the agent, and placing outbound calls
//!   (the blocking [`sip::place_call`]/[`sip::place_call_direct`] + its
//!   [`MediaSession`], both !Send, stay on this thread).
//!
//! The UI sends [`Command`]s in and drains [`Update`]s out; nothing !Send ever
//! crosses back to the UI thread.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use mde_egui::egui::Context;
use mde_voice_hud::media::{self, MediaSession};
use mde_voice_hud::sip::{self, AgentCommand, AgentEvent, CallSession, SipAccount};

use crate::model::{is_registrar_backed, Command, Update};

/// How long an outbound INVITE waits for an answer before giving up. Mirrors the
/// shipped HUD's 30 s ring timeout.
const RING_TIMEOUT: Duration = Duration::from_secs(30);

/// Spawn the worker around `account`, returning the [`Command`] sender the UI
/// drives it with. `ctx` is repainted after every [`Update`]; `updates` carries
/// results back.
pub fn spawn(account: SipAccount, ctx: Context, updates: &Sender<Update>) -> Sender<Command> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let (agent_ev_tx, agent_ev_rx) = mpsc::channel::<AgentEvent>();
    let (agent_cmd_tx, agent_cmd_rx) = mpsc::channel::<AgentCommand>();

    // Loading state: a registrar account's initial REGISTER is synchronous
    // (`agent_register` blocks the whole registrar round-trip), and the agent
    // emits its first Registration event only once that returns — so without this
    // the surface would sit on a stale "Not registered" for the round-trip. Show
    // the real in-progress "Registering…" at once; the agent's first event
    // replaces it with the true outcome. A registrar-less P2P node has no REGISTER
    // and reports its real state near-instantly, so it keeps its honest NoAccount.
    // This send precedes any worker thread, so it is drained before the agent's
    // outcome — no flicker back to "Registering…".
    if is_registrar_backed(&account) {
        let _ = updates.send(Update::Registration(sip::RegistrationState::Registering));
    }

    // The SIP agent: registration + inbound calls. Owns the socket; blocks.
    let agent_account = account.clone();
    spawn_named("mde-voice-egui-agent", updates, move || {
        sip::run_agent(&agent_account, &agent_ev_tx, &agent_cmd_rx);
    });

    // The bridge: AgentEvent → Update (+ repaint). Stateful so an inbound
    // `Established` (which carries no caller id) still names the ringing caller.
    // Ends when the agent drops its event sender (agent shutdown).
    let bridge_updates = updates.clone();
    let bridge_ctx = ctx.clone();
    spawn_named("mde-voice-egui-bridge", updates, move || {
        run_bridge(&agent_ev_rx, &bridge_updates, &bridge_ctx);
    });

    // The command handler: UI Command → agent command / outbound call. Ends when
    // the UI drops its command sender; dropping `agent_cmd_tx` then shuts the
    // agent down (its try_recv sees Disconnected), which ends the bridge too.
    let cmd_updates = updates.clone();
    spawn_named("mde-voice-egui-cmd", updates, move || {
        run_commands(&account, &cmd_rx, &agent_cmd_tx, &cmd_updates, &ctx);
    });

    cmd_tx
}

/// Spawn a named worker thread, surfacing a spawn failure as an [`Update::Error`]
/// so the UI shows it rather than silently doing nothing.
fn spawn_named<F>(name: &str, updates: &Sender<Update>, body: F)
where
    F: FnOnce() + Send + 'static,
{
    if let Err(e) = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(body)
    {
        let _ = updates.send(Update::Error(format!("could not start {name}: {e}")));
    }
}

/// Forward each [`AgentEvent`] to the UI as an [`Update`], waking it each time.
fn run_bridge(events: &Receiver<AgentEvent>, updates: &Sender<Update>, ctx: &Context) {
    // The agent's `Established`/`RemoteHangup` events carry no caller id, so we
    // remember the last ringing caller to label the connected/ended state.
    let mut incoming_from: Option<String> = None;
    while let Ok(ev) = events.recv() {
        let update = match ev {
            AgentEvent::Registration(reg) => Update::Registration(reg),
            AgentEvent::Incoming { from, .. } => {
                incoming_from = Some(from.clone());
                Update::Incoming { from }
            }
            AgentEvent::Established => Update::Connected {
                peer: incoming_from.clone().unwrap_or_else(|| "call".to_string()),
            },
            AgentEvent::RemoteHangup => {
                incoming_from = None;
                Update::Ended
            }
        };
        let _ = updates.send(update);
        ctx.request_repaint();
    }
}

/// Service UI commands until the UI hangs up (its command sender drops). The
/// outbound call's dialog + media live here (both !Send); an inbound call's
/// media lives in the agent thread instead.
fn run_commands(
    account: &SipAccount,
    cmd_rx: &Receiver<Command>,
    agent_cmd_tx: &Sender<AgentCommand>,
    updates: &Sender<Update>,
    ctx: &Context,
) {
    let mut outbound: Option<CallSession> = None;
    let mut out_media: Option<MediaSession> = None;

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            Command::Answer => {
                let _ = agent_cmd_tx.send(AgentCommand::Answer);
            }
            Command::Decline => {
                let _ = agent_cmd_tx.send(AgentCommand::Decline);
                let _ = updates.send(Update::Ended);
            }
            Command::Reregister => {
                let _ = agent_cmd_tx.send(AgentCommand::Reregister);
            }
            Command::HangUp => {
                // Tear down whichever call is up: the inbound dialog lives in the
                // agent; an outbound dialog + media live here.
                let _ = agent_cmd_tx.send(AgentCommand::HangUp);
                teardown_outbound(&mut outbound, &mut out_media);
                let _ = updates.send(Update::Ended);
            }
            Command::Dial(target) => {
                place_outbound(account, &target, updates, &mut outbound, &mut out_media);
            }
        }
        ctx.request_repaint();
    }

    // UI hung up — best-effort teardown of a live outbound call before this
    // thread (and then, via the dropped agent sender, the agent) exits.
    teardown_outbound(&mut outbound, &mut out_media);
}

/// Place an outbound call (blocking) and attach its media. A peer NAME dials
/// directly over the overlay (registrar-less); a NUMBER routes via the
/// registrar. Surfaces progress as [`Update::Dialing`] then
/// [`Update::Connected`]/[`Update::Failed`].
fn place_outbound(
    account: &SipAccount,
    target: &str,
    updates: &Sender<Update>,
    outbound: &mut Option<CallSession>,
    out_media: &mut Option<MediaSession>,
) {
    let dialed = target.trim();
    if dialed.is_empty() {
        return;
    }
    let _ = updates.send(Update::Dialing {
        peer: dialed.to_string(),
    });

    let result = if sip::looks_like_peer(dialed) {
        // VOIP-P2P — a mesh-peer name dials DIRECTLY over the overlay, even with
        // no registrar account (a local overlay identity is synthesized).
        let host = sip::peer_host_for(dialed);
        sip::place_call_direct(account, "", &host, sip::P2P_SIP_PORT, RING_TIMEOUT)
    } else if account.server_host.trim().is_empty() {
        Err("no registrar account — dial a mesh peer by name for a direct call".to_string())
    } else {
        sip::place_call(account, dialed, RING_TIMEOUT)
    };

    match result {
        Ok(session) => {
            // Start RTP/G.711 over the negotiated endpoint. A media failure (no
            // audio device on this host) leaves the call up but silent — honest
            // degradation, surfaced in the banner, not a panic.
            match media::start_media(session.rtp_port, &session.remote) {
                Ok(m) => *out_media = Some(m),
                Err(e) => {
                    let _ = updates.send(Update::Error(format!("audio unavailable: {e}")));
                }
            }
            let _ = updates.send(Update::Connected {
                peer: dialed.to_string(),
            });
            *outbound = Some(session);
        }
        Err(why) => {
            let _ = updates.send(Update::Failed(format!("{dialed}: {why}")));
        }
    }
}

/// Stop an outbound call's media and BYE its dialog (best-effort; never panics).
fn teardown_outbound(outbound: &mut Option<CallSession>, out_media: &mut Option<MediaSession>) {
    if let Some(m) = out_media.take() {
        m.stop();
    }
    if let Some(session) = outbound.take() {
        let _ = sip::hang_up(&session);
    }
}
