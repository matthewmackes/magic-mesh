//! `mde-voice-egui` — the MCNF **E12 "Quasar"** egui Voice/SIP surface (E12-11).
//!
//! A standalone eframe surface on the shared [`mde_egui`] harness that REUSES the
//! shipped pure-Rust SIP stack in `mde-voice-hud` (governance §6 — glue, not
//! reimplementation):
//!
//! * the [`mde_voice_hud::sip`] register/call state machine REGISTERs the loaded
//!   account and serves inbound INVITE/BYE on its agent thread, and places
//!   outbound calls (`place_call` / `place_call_direct`),
//! * the [`mde_voice_hud::media`] engine carries RTP/G.711 audio on a connected
//!   call,
//! * the dialed-string routing helpers (`looks_like_peer` / `peer_host_for`)
//!   split a mesh-peer name from a registrar number.
//!
//! Everything renders through the shared [`mde_egui::Style`]. The blocking, !Send
//! SIP work lives on [`worker`] threads so the egui UI thread never blocks; the
//! render-agnostic view-model in [`model`] is unit-tested without a socket or a
//! sound device. The retired Cosmic-era HUD is never pulled (`mde-voice-hud` is
//! consumed with its `gui` feature off).
//!
//! Under E12 "Quasar" the mesh-control surfaces are **panels inside the one shell**
//! (`mde-shell-egui`), not separate clients (§5, the EMBED model — there is no
//! compositor). So the central view is factored into the public [`voice_panel`]
//! function: the standalone [`VoiceApp`] renders it into its own `CentralPanel`
//! (framing it with the registration header and the per-frame worker-update drain),
//! and the shell renders the *same* function into a panel of its egui context, so
//! the surface looks and behaves identically either way.
//!
//! Tier (§6): desktop-shell — it depends only on the harness and the voice
//! service (both inward edges), pulling in no mesh-substrate crate.

pub mod fleet;
pub mod menubar;
pub mod model;

mod view;
mod worker;

use std::sync::mpsc::{self, Receiver, Sender};

use mde_egui::eframe::{self, App, CreationContext};
use mde_egui::egui::{self, Context};

use mde_voice_hud::sip::{CallState, SipAccount};

use crate::fleet::FleetState;
use crate::menubar::MenuBarState;
use crate::model::{Command, Tab, Update, VoiceState};

pub use menubar::voice_menubar;
pub use view::voice_panel;

/// The voice surface: the view-model, the dial buffer, and the channel to its
/// worker threads.
pub struct VoiceApp {
    /// The render-agnostic state the view draws.
    state: VoiceState,
    /// The dialer's free-form target buffer (view-local input).
    dial: String,
    /// Outbound intents to the worker.
    commands: Sender<Command>,
    /// Inbound results from the worker, drained at the top of each frame.
    updates: Receiver<Update>,
    /// The account identity shown in the header (the AOR, or a P2P-overlay note).
    identity: String,
    /// Whether this is a registrar-backed account (vs. a registrar-less P2P
    /// identity) — gates the header's Retry affordance, which is meaningless
    /// (a dead-end) for a P2P node with no registrar to re-register against.
    registrar_backed: bool,
    /// Which face is showing — the local dialer or the fleet board.
    tab: Tab,
    /// The VOIP-GW-5 fleet config board: the live per-node reg-state read off the
    /// Bus + the shared-account config, with the provision/inbound/nickname
    /// verbs. Its own Bus handle (read `state/voice/*`, publish `action/voice/*`).
    fleet: FleetState,
    /// The MENUBAR-ALL top-bar state (only the shortcuts-reference toggle; every
    /// other menu item reads/drives the fields above).
    menu: MenuBarState,
}

impl VoiceApp {
    /// Build the surface: load the shared SIP account and spawn the worker (the
    /// SIP agent registers + listens immediately). With no `account.toml`, the
    /// shipped loader synthesizes a registrar-less local overlay identity, so the
    /// surface always has a real agent to drive — the registration status then
    /// reflects reality (P2P-registered, or a failure) rather than faking it.
    #[must_use]
    pub fn new(cc: &CreationContext<'_>) -> Self {
        Self::new_with_ctx(&cc.egui_ctx)
    }

    /// Build over an egui [`egui::Context`] directly — the DRM-seat shell path has
    /// no eframe `CreationContext`, only the bare `Context` the DRM runner drives.
    /// Both entry points converge here so the SIP worker gets a repaint handle.
    #[must_use]
    pub fn new_with_ctx(ctx: &egui::Context) -> Self {
        let (update_tx, update_rx) = mpsc::channel::<Update>();
        let account = SipAccount::load();
        let identity = match &account {
            Some(a) if !a.server_host.is_empty() => format!("{}@{}", a.username, a.server_host),
            _ => "this node · P2P overlay".to_string(),
        };
        let account = account.unwrap_or_else(SipAccount::local_identity);
        // Whether Retry (re-register) is meaningful — a registrar-backed account,
        // not a registrar-less P2P node. Read from the resolved account before it
        // is moved into the worker.
        let registrar_backed = model::is_registrar_backed(&account);
        let commands = worker::spawn(account, ctx.clone(), &update_tx);
        Self {
            state: VoiceState::new(),
            dial: String::new(),
            commands,
            updates: update_rx,
            identity,
            registrar_backed,
            tab: Tab::default(),
            fleet: FleetState::new(),
            menu: MenuBarState::default(),
        }
    }

    /// Send an intent to the worker (a no-op if the worker has hung up).
    fn send(&self, cmd: Command) {
        let _ = self.commands.send(cmd);
    }

    /// WIN7-4 — the current call lifecycle, the SAME `self.state.call` field
    /// the dialer's own status row already renders via [`CallState::label`]
    /// (no second read, no new formatting, §7). `mde-shell-egui`'s embedding
    /// shell holds this `VoiceApp` directly and reuses this SAME `label()`
    /// (gated on its own `Idle` → `String::new()` convention, rather than a
    /// second `is_active()`-derived subset) for the Start Menu Voice tile's
    /// live fact.
    #[must_use]
    pub const fn call_state(&self) -> &CallState {
        &self.state.call
    }
}

/// Drain the worker's updates into the surface state — the per-frame **state pump**.
///
/// The standalone [`VoiceApp`]'s `update` calls this at the top of every
/// frame; the E12 shell (E12-3b) calls it for the mounted surface each frame too,
/// because the shell owns the one frame loop and never calls the surface's
/// `App::update`. Non-blocking (`try_recv`) and a no-op when the SIP agent has
/// sent nothing since the last frame.
pub fn voice_pump(app: &mut VoiceApp) {
    while let Ok(update) = app.updates.try_recv() {
        app.state.apply(update);
    }
}

impl App for VoiceApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Drain everything the worker has sent since the last frame.
        voice_pump(self);

        // The MENUBAR-ALL shared top bar (VOICE title · menus · live status
        // cluster) — the surface's discoverable control surface, replacing the
        // old registration header (its identity + reg status now ride the bar's
        // status chips, its Retry the honest File → Re-register item).
        egui::TopBottomPanel::top("voice-menubar").show(ctx, |ui| menubar::voice_menubar(ui, self));

        // The central content is the shared `voice_panel` body, so the standalone
        // window and the embedded shell panel (E12-3b) render identically.
        egui::CentralPanel::default().show(ctx, |ui| view::voice_panel(ui, self));
    }
}
