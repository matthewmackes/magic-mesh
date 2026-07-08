//! The VDI **Desktop** surface — a remote VM desktop rendered egui-native.
//!
//! E12 "Quasar" brokers VM desktops *into* the one shell (§5 EMBED, lock 21):
//! there is no external viewer. The remote framebuffer is decoded by
//! `mde-vdi-rdp` (RDP-primary) or `mde-vdi-vnc` (VNC / XAPI-console fallback) into
//! an [`egui::ColorImage`]; this panel uploads that image to a `TextureHandle` and
//! paints it as the shell body, and forwards the frame's egui input straight back
//! to the session's input mapper.
//!
//! ```text
//!   session.frame() ─▶ ColorImage ─▶ TextureHandle ─▶ ui paints the body
//!   ui.input events ─────────────────────────────────▶ session.send_input()
//! ```
//!
//! This unit is the **first caller** of the two decoder crates — it gives their
//! `frame()`/`send_input()` surface a home. Until a session is attached (the live
//! wire transport is the gated E12-4 layer) the panel shows an honest "no desktop"
//! EmptyState, never a placeholder render of a fake desktop (§7).

use mde_egui::egui::{self, Sense, TextureHandle, TextureOptions};

use mde_vdi_rdp::RdpSession;
use mde_vdi_vnc::VncSession;

use crate::auth::DesktopAuth;

use std::path::PathBuf;

#[cfg(feature = "live-vdi")]
use {
    mde_vdi_rdp::{PumpOutcome, RdpConfig, RdpConnection},
    std::sync::mpsc,
    std::thread,
    std::time::Duration,
};

/// A live VDI desktop the shell drives — RDP-primary, VNC the console fallback.
/// Both decoder crates expose the *identical* egui-facing surface
/// (`frame()` → [`egui::ColorImage`], `send_input(&egui::Event)`), so the panel
/// drives whichever is attached through one match.
///
/// The variants are matched + driven here, but a session is *constructed* only
/// once the gated live wire transport (E12-4) attaches one — until then the panel
/// runs on the no-session EmptyState, so a non-test build sees no constructor
/// (the tests build both variants to prove the decode → paint path end to end).
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "a session is constructed by the gated E12-4 wire transport; the render panel here is its first caller"
    )
)]
enum Session {
    /// An RDP desktop (`mde-vdi-rdp`, the primary protocol).
    Rdp(RdpSession),
    /// A VNC/RFB desktop (`mde-vdi-vnc`, the universal console fallback).
    Vnc(VncSession),
}

impl Session {
    /// The latest decoded desktop, or `None` if nothing changed since last frame.
    fn frame(&mut self) -> Option<egui::ColorImage> {
        match self {
            Session::Rdp(s) => s.frame(),
            Session::Vnc(s) => s.frame(),
        }
    }

    /// Forward one egui input event to the guest — the session maps it to the
    /// protocol's pointer / key / wheel / text intents internally.
    fn send_input(&mut self, event: &egui::Event) {
        match self {
            Session::Rdp(s) => s.send_input(event),
            Session::Vnc(s) => s.send_input(event),
        }
    }
}

/// A dialable endpoint for a direct desktop transport. Mesh-brokered connects may
/// omit it while the broker resolves the overlay route; manual/mDNS/external rows
/// carry it so the live VDI transport can attach without re-parsing UI text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DesktopEndpoint {
    /// TCP host/address to dial. For mesh rows this should be the Nebula overlay
    /// address or name once the registry publishes it.
    pub host: String,
    /// TCP port for the chosen protocol.
    pub port: u16,
}

impl DesktopEndpoint {
    /// A non-empty host plus non-zero port.
    pub(crate) fn new(host: impl Into<String>, port: u16) -> Option<Self> {
        let host = host.into();
        if host.trim().is_empty() || port == 0 {
            return None;
        }
        Some(Self { host, port })
    }

    /// Log/UI-safe dial address.
    fn label(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// A desktop target the Chooser (CHOOSER-2, née the E12-5b picker) handed to the
/// surface: the desktop the operator chose, plus the direct endpoint if discovery
/// published one. Recorded so the surface reflects the pending connect by name
/// until the live transport attaches the decoder `session` — an honest
/// "connecting" caption, never a fake desktop (§7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestedTarget {
    /// The peer serving the VM (a scheduler node id).
    pub serving_peer: String,
    /// The VM's display name — the surface caption.
    pub name: String,
    /// Direct dial target for manual/mDNS/external endpoints, or for mesh rows once
    /// the registry has published an overlay address + port.
    pub endpoint: Option<DesktopEndpoint>,
}

impl RequestedTarget {
    /// A target from the peer serving the VM and the VM's name.
    pub(crate) fn new(serving_peer: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            serving_peer: serving_peer.into(),
            name: name.into(),
            endpoint: None,
        }
    }

    /// Attach a direct dial endpoint to the target.
    pub(crate) fn with_endpoint(mut self, endpoint: Option<DesktopEndpoint>) -> Self {
        self.endpoint = endpoint;
        self
    }
}

/// Broker lifecycle metadata attached to a mesh-brokered desktop connect. The
/// Chooser mints this with the broker `Open`; the live transport publishes
/// `active` / `disconnect` / `close` against the same id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrokerSessionLifecycle {
    /// The broker roster key.
    pub(crate) id: String,
    /// The local Bus root that accepts `action/vdi/session` lifecycle writes.
    pub(crate) bus_root: Option<PathBuf>,
}

impl BrokerSessionLifecycle {
    /// Attach the minted broker id to the Bus root used for its `Open`.
    pub(crate) fn new(id: impl Into<String>, bus_root: Option<PathBuf>) -> Self {
        Self {
            id: id.into(),
            bus_root,
        }
    }
}

/// The desktop protocol a connect routes to — the VDI tier's *routable* set. The
/// Chooser's wire [`crate::chooser::Protocol`] additionally carries an `Unknown`
/// badge for a tag this build can't render; only a routable protocol reaches a
/// [`ConnectRequest`], so this enum has no unknown arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VdiProtocol {
    /// Remote Desktop Protocol — `mde-vdi-rdp` (the primary).
    Rdp,
    /// VNC / RFB — `mde-vdi-vnc` (the universal console fallback).
    Vnc,
    /// Spice — `mde-vdi-spice` (CHOOSER-5; honest-gated until the crate lands).
    Spice,
}

impl VdiProtocol {
    /// The decoder crate this protocol renders through.
    pub(crate) const fn client_crate(self) -> &'static str {
        match self {
            Self::Rdp => "mde-vdi-rdp",
            Self::Vnc => "mde-vdi-vnc",
            Self::Spice => "mde-vdi-spice",
        }
    }

    /// Whether a decoder crate exists to render this protocol today. Spice's
    /// client is CHOOSER-5 — a Spice request is built honestly but never faked
    /// into a live session (§7).
    pub(crate) const fn has_client(self) -> bool {
        matches!(self, Self::Rdp | Self::Vnc)
    }

    /// The short picker / caption label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Rdp => "RDP",
            Self::Vnc => "VNC",
            Self::Spice => "Spice",
        }
    }
}

/// Fullscreen under the thin chrome bar (the E12 VDI idiom) or a windowed desktop
/// — a per-connection choice (design lock 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayMode {
    /// The desktop fills the shell body under the thin chrome bar.
    Fullscreen,
    /// The desktop runs in a window inside the shell.
    Windowed,
}

impl DisplayMode {
    /// The picker / caption label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Fullscreen => "fullscreen",
            Self::Windowed => "windowed",
        }
    }
}

/// Span the guest across every local display or confine it to a single one — a
/// per-connection choice (design lock 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MonitorSpan {
    /// A single display.
    Single,
    /// Span all local displays.
    All,
}

impl MonitorSpan {
    /// The picker / caption label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Single => "single display",
            Self::All => "span all displays",
        }
    }
}

/// A fully-specified desktop connect the Chooser's always-ask picker produces
/// (CHOOSER-4): the chosen [`VdiProtocol`] (always-asked when a source offered
/// several — lock 6), the [`DisplayMode`] (lock 9), the [`MonitorSpan`] (lock
/// 12), and the [`RequestedTarget`] the session attaches to. The Desktop surface
/// routes it to the matching decoder crate ([`VdiProtocol::client_crate`]); the
/// live wire transport that constructs the session is the gated E12-4 layer, and
/// a Spice route is honest-gated on CHOOSER-5 until `mde-vdi-spice` lands — the
/// request is still built truthfully, but no session is ever faked (§7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectRequest {
    /// The desktop the session attaches to (serving peer + VM/host name).
    pub target: RequestedTarget,
    /// The protocol the operator chose.
    pub protocol: VdiProtocol,
    /// Fullscreen vs windowed (lock 9).
    pub display: DisplayMode,
    /// Single vs span-all (lock 12).
    pub monitors: MonitorSpan,
    /// CHOOSER-6 — how the connect authenticates: mesh-identity SSO for a
    /// mesh-brokered source, or a sealed credential for an external endpoint. The
    /// gated live transport (E12-4) feeds a sealed credential's secret into the
    /// protocol config's password field; a mesh identity needs no prompt. The
    /// secret is redacted from `Debug` ([`DesktopAuth`]), so this request is
    /// log-safe.
    pub auth: DesktopAuth,
    /// Optional broker lifecycle handle for mesh-rostered sessions. Direct
    /// off-mesh endpoints leave this empty.
    pub broker_session: Option<BrokerSessionLifecycle>,
}

impl ConnectRequest {
    /// Assemble a request from the picked target + the three display choices + the
    /// resolved auth (CHOOSER-6).
    pub(crate) const fn new(
        target: RequestedTarget,
        protocol: VdiProtocol,
        display: DisplayMode,
        monitors: MonitorSpan,
        auth: DesktopAuth,
    ) -> Self {
        Self {
            target,
            protocol,
            display,
            monitors,
            auth,
            broker_session: None,
        }
    }

    /// Attach the broker session lifecycle id minted by discovery.
    pub(crate) fn with_broker_session(mut self, broker: BrokerSessionLifecycle) -> Self {
        self.broker_session = Some(broker);
        self
    }
}

#[cfg(feature = "live-vdi")]
enum LiveRdpEvent {
    Connected(String),
    Frame(egui::ColorImage),
    Error(String),
    Ended(String),
}

#[cfg(feature = "live-vdi")]
struct LiveRdpHandle {
    input_tx: mpsc::Sender<egui::Event>,
    stop_tx: mpsc::Sender<()>,
    event_rx: mpsc::Receiver<LiveRdpEvent>,
}

#[cfg(feature = "live-vdi")]
impl LiveRdpHandle {
    fn spawn(request: &ConnectRequest) -> Result<Self, String> {
        let Some(endpoint) = request.target.endpoint.clone() else {
            return Err("discovery has not published a dialable endpoint for this desktop".into());
        };
        let DesktopAuth::Sealed { credential, .. } = &request.auth else {
            return Err("mesh-identity desktop SSO is broker-gated; direct RDP needs a sealed guest credential".into());
        };
        if credential.username.trim().is_empty() {
            return Err("RDP requires a username in the sealed desktop credential".into());
        }

        let config = RdpConfig::new(
            endpoint.host.clone(),
            credential.username.clone(),
            credential.secret.expose().to_owned(),
        )
        .with_port(endpoint.port)
        .with_resolution(1024, 768);
        let (input_tx, input_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        thread::Builder::new()
            .name(format!("mde-live-rdp-{}", request.target.name))
            .spawn(move || run_live_rdp(config, input_rx, stop_rx, event_tx))
            .map_err(|e| format!("failed to spawn live RDP worker: {e}"))?;

        Ok(Self {
            input_tx,
            stop_tx,
            event_rx,
        })
    }

    fn send_input(&self, event: egui::Event) {
        let _ = self.input_tx.send(event);
    }

    fn stop(&self) {
        let _ = self.stop_tx.send(());
    }
}

#[cfg(feature = "live-vdi")]
impl Drop for LiveRdpHandle {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
    }
}

#[cfg(feature = "live-vdi")]
fn run_live_rdp(
    config: RdpConfig,
    input_rx: mpsc::Receiver<egui::Event>,
    stop_rx: mpsc::Receiver<()>,
    event_tx: mpsc::Sender<LiveRdpEvent>,
) {
    let target = format!("{}:{}", config.host, config.port);
    let mut session = match RdpSession::new(config) {
        Ok(session) => session,
        Err(e) => {
            let _ = event_tx.send(LiveRdpEvent::Error(format!("RDP config rejected: {e}")));
            return;
        }
    };
    let mut conn = match RdpConnection::connect(&mut session) {
        Ok(conn) => conn,
        Err(e) => {
            let _ = event_tx.send(LiveRdpEvent::Error(format!("RDP connect failed: {e}")));
            return;
        }
    };
    let _ = event_tx.send(LiveRdpEvent::Connected(target));
    if let Some(frame) = session.frame() {
        let _ = event_tx.send(LiveRdpEvent::Frame(frame));
    }

    loop {
        if stop_rx.try_recv().is_ok() {
            let _ = conn.shutdown(&mut session);
            return;
        }

        let mut had_input = false;
        while let Ok(event) = input_rx.try_recv() {
            session.send_input(&event);
            had_input = true;
        }
        if had_input {
            if let Err(e) = conn.flush_input(&mut session) {
                let _ = event_tx.send(LiveRdpEvent::Error(format!("RDP input failed: {e}")));
                return;
            }
        }

        match conn.pump_once(&mut session, Duration::from_millis(50)) {
            Ok(PumpOutcome::Processed { painted_rects }) => {
                if painted_rects > 0 {
                    if let Some(frame) = session.frame() {
                        let _ = event_tx.send(LiveRdpEvent::Frame(frame));
                    }
                }
            }
            Ok(PumpOutcome::TimedOut) => {}
            Ok(PumpOutcome::Terminated { reason }) => {
                let _ = event_tx.send(LiveRdpEvent::Ended(reason));
                return;
            }
            Err(e) => {
                let _ = event_tx.send(LiveRdpEvent::Error(format!("RDP pump failed: {e}")));
                return;
            }
        }
    }
}

/// The Desktop surface's state: the active session (if any), the desktop texture
/// the framebuffer is uploaded into, the decode → upload hand-off slot, and the
/// picked target the discovery picker requested before a live session attaches.
#[derive(Default)]
pub(crate) struct VdiState {
    /// The connected desktop, or `None` when nothing is attached (the EmptyState).
    session: Option<Session>,
    /// The GPU texture the desktop framebuffer lives in — allocated on the first
    /// frame, then updated in place with [`TextureHandle::set`] every frame after
    /// (egui reuses the allocation, so a live desktop is not a per-frame upload
    /// churn).
    texture: Option<TextureHandle>,
    /// A decoded frame awaiting upload on the next paint. The decode side (the
    /// live session's `frame()`, or a synthetic frame in tests) writes it;
    /// `vdi_panel` drains it into `texture`. This is the single-threaded shape of
    /// the decode → UI hand-off the gated wire transport fills off-thread.
    incoming: Option<egui::ColorImage>,
    /// Raised when the operator presses the reserved Esc chord over the desktop —
    /// the shell reads it to release the fullscreen desktop back to the chrome.
    return_to_chrome: bool,
    /// The connect the Chooser's picker chose (CHOOSER-4 — protocol + display +
    /// monitors + target), held until the gated live transport attaches a
    /// `session`. Drives the honest "connecting" caption (which names the chosen
    /// protocol + display) and tells the shell to show the Desktop surface.
    requested: Option<ConnectRequest>,
    /// Live in-shell RDP transport for a direct endpoint. Kept separate from
    /// `session`, which remains the single-threaded decoder used by tests and VNC.
    #[cfg(feature = "live-vdi")]
    live_rdp: Option<LiveRdpHandle>,
    /// Log-safe live transport status/error shown under the empty backdrop until a
    /// frame arrives.
    #[cfg(feature = "live-vdi")]
    live_status: Option<String>,
    /// Broker lifecycle currently marked active by the live transport.
    #[cfg(feature = "live-vdi")]
    active_broker_session: Option<BrokerSessionLifecycle>,
}

impl VdiState {
    /// Take (and clear) the "return to chrome" request raised by the Esc chord.
    /// The shell calls this after mounting the panel to leave the surface.
    pub(crate) fn take_return_to_chrome(&mut self) -> bool {
        std::mem::take(&mut self.return_to_chrome)
    }

    /// Raise the "return to chrome" request from a control other than the Esc chord
    /// (MENUBAR-ALL — the Desktop bar's **Session → Return to Mesh Control**). It is
    /// the SAME seam [`forward_input`] sets, drained by [`Self::take_return_to_chrome`]
    /// after the panel mounts, so the menu path adds no new behaviour (§6).
    pub(crate) const fn request_return_to_chrome(&mut self) {
        self.return_to_chrome = true;
    }

    /// A log-safe summary of the pending connect for the Desktop bar's status
    /// cluster: the VM/host name plus the chosen protocol's label, or `None` when no
    /// connect is pending (the surface is on the Chooser). Carries no secret.
    pub(crate) fn requested_summary(&self) -> Option<(&str, &'static str)> {
        self.requested
            .as_ref()
            .map(|r| (r.target.name.as_str(), r.protocol.label()))
    }

    /// Record the connect the Chooser's picker chose (CHOOSER-4). The surface then
    /// shows a "connecting" state naming the target + chosen protocol until the
    /// gated wire transport attaches the live decoder session.
    pub(crate) fn request_connect(&mut self, request: ConnectRequest) {
        #[cfg(feature = "live-vdi")]
        {
            self.live_status = None;
            self.publish_broker_close_if_active();
            if let Some(live) = self.live_rdp.take() {
                live.stop();
            }
            self.texture = None;
            self.incoming = None;
            if request.protocol == VdiProtocol::Rdp {
                match LiveRdpHandle::spawn(&request) {
                    Ok(handle) => {
                        self.live_status = Some("Opening live RDP transport".to_string());
                        self.live_rdp = Some(handle);
                    }
                    Err(reason) => {
                        self.live_status = Some(format!("Live RDP gated: {reason}"));
                    }
                }
            }
        }
        self.requested = Some(request);
    }

    /// The picked target, if any — the shell reads it to decide whether the Desktop
    /// surface shows the Chooser (none) or the connecting/desktop state.
    pub(crate) fn requested_target(&self) -> Option<&RequestedTarget> {
        self.requested.as_ref().map(|r| &r.target)
    }

    /// Clear the pending connect — the operator backed out before a live session
    /// attached, so the Desktop surface falls back to the Chooser.
    pub(crate) fn clear_target(&mut self) {
        #[cfg(feature = "live-vdi")]
        {
            if let Some(live) = self.live_rdp.take() {
                live.stop();
            }
            self.publish_broker_close_if_active();
            self.live_status = None;
            self.texture = None;
            self.incoming = None;
        }
        self.requested = None;
    }

    #[cfg(feature = "live-vdi")]
    fn poll_live_rdp(&mut self) {
        let Some(live) = self.live_rdp.as_ref() else {
            return;
        };
        let mut publish_active = false;
        let mut publish_disconnect = false;
        while let Ok(event) = live.event_rx.try_recv() {
            match event {
                LiveRdpEvent::Connected(target) => {
                    self.live_status = Some(format!("Live RDP connected to {target}"));
                    publish_active = true;
                }
                LiveRdpEvent::Frame(frame) => {
                    self.incoming = Some(frame);
                }
                LiveRdpEvent::Error(reason) => {
                    self.live_status = Some(reason);
                    publish_disconnect = true;
                }
                LiveRdpEvent::Ended(reason) => {
                    self.live_status = Some(format!("RDP session ended: {reason}"));
                    publish_disconnect = true;
                }
            }
        }
        if publish_active {
            self.publish_broker_active();
        }
        if publish_disconnect {
            self.publish_broker_disconnect_if_active();
        }
    }

    #[cfg(feature = "live-vdi")]
    fn publish_broker_active(&mut self) {
        if self.active_broker_session.is_some() {
            return;
        }
        let Some(broker) = self
            .requested
            .as_ref()
            .and_then(|request| request.broker_session.clone())
        else {
            return;
        };
        let mut last_error = None;
        crate::discovery::publish_active(
            broker.bus_root.as_deref(),
            &mut last_error,
            broker.id.as_str(),
        );
        if let Some(reason) = last_error {
            self.live_status = Some(format!("Broker lifecycle gated: {reason}"));
        } else {
            self.active_broker_session = Some(broker);
        }
    }

    #[cfg(feature = "live-vdi")]
    fn publish_broker_disconnect_if_active(&mut self) {
        let Some(broker) = self.active_broker_session.take() else {
            return;
        };
        let mut last_error = None;
        crate::discovery::publish_disconnect(
            broker.bus_root.as_deref(),
            &mut last_error,
            broker.id.as_str(),
        );
        if let Some(reason) = last_error {
            self.live_status = Some(format!("Broker lifecycle gated: {reason}"));
        }
    }

    #[cfg(feature = "live-vdi")]
    fn publish_broker_close_if_active(&mut self) {
        let Some(broker) = self.active_broker_session.take() else {
            return;
        };
        let mut last_error = None;
        crate::discovery::publish_close(
            broker.bus_root.as_deref(),
            &mut last_error,
            broker.id.as_str(),
        );
        if let Some(reason) = last_error {
            self.live_status = Some(format!("Broker lifecycle gated: {reason}"));
        }
    }
}

/// A remote desktop is scaled to fill the shell body, so sample it linearly —
/// crisper than nearest when the negotiated desktop size doesn't match the panel.
const DESKTOP_TEX: TextureOptions = TextureOptions::LINEAR;

/// Render the Desktop surface into `ui`: upload any new framebuffer, paint it to
/// fill the body, and forward this frame's egui input to the guest. With no
/// session attached it draws the honest "no desktop" EmptyState instead.
pub(crate) fn vdi_panel(ui: &mut egui::Ui, state: &mut VdiState) {
    #[cfg(feature = "live-vdi")]
    state.poll_live_rdp();

    // 1. Pull the newest decoded frame off the live session into the upload slot.
    if let Some(session) = state.session.as_mut() {
        if let Some(img) = session.frame() {
            state.incoming = Some(img);
        }
    }

    // 2. Upload a pending frame: allocate the texture on the first one, then set
    //    it in place on every frame after.
    if let Some(img) = state.incoming.take() {
        match state.texture.as_mut() {
            Some(handle) => handle.set(img, DESKTOP_TEX),
            None => {
                state.texture = Some(ui.ctx().load_texture("vdi-desktop", img, DESKTOP_TEX));
            }
        }
    }

    // 3. Paint the desktop (or the EmptyState) and drive input.
    match state.texture.as_ref() {
        Some(texture) => {
            let tex_id = texture.id();
            // Allocate the interactive body rect first, then paint the texture over
            // it, so the desktop both fills the panel and captures pointer input.
            let size = ui.available_size();
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
            egui::Image::new(egui::load::SizedTexture::new(tex_id, rect.size())).paint_at(ui, rect);
            // Clicking the desktop focuses it so keystrokes route to the guest.
            if resp.clicked() {
                resp.request_focus();
            }
            forward_input(ui, state);
        }
        None => {
            // No live desktop texture: the empty Desktop surface paints the BRAND-1
            // backdrop — the centered logo lockup (full opacity, breathing while
            // idle) with any honest status relocated to a small line BELOW the image
            // (lock 2), never over it. The backdrop owns the crossfade/breathe motion
            // (lock 10), so there is no bespoke caption ease here.
            match state.requested.as_ref() {
                // The Chooser's picker chose a connect but no live decoder is
                // attached yet (the wire transport is gated) — the status honestly
                // names the desktop + the chosen protocol/display below the logo,
                // never a placeholder render (§7). A Spice route says its client is
                // CHOOSER-5, so it's plain no session was faked.
                Some(req) => {
                    let title = format!(
                        "Connecting to {} via {}",
                        req.target.name,
                        req.protocol.label()
                    );
                    // CHOOSER-6 — name the auth mode honestly (SSO vs sealed cred);
                    // `auth.summary()` is log-safe and never carries the secret.
                    let auth = req.auth.summary();
                    let detail = if req.protocol.has_client() {
                        let endpoint = req.target.endpoint.as_ref().map_or_else(
                            || req.target.serving_peer.clone(),
                            DesktopEndpoint::label,
                        );
                        let live_status = {
                            #[cfg(feature = "live-vdi")]
                            {
                                state
                                    .live_status
                                    .as_deref()
                                    .unwrap_or("Waiting for the live transport")
                                    .to_string()
                            }
                            #[cfg(not(feature = "live-vdi"))]
                            {
                                "the live transport is not compiled into this shell build"
                                    .to_string()
                            }
                        };
                        format!(
                            "Brokering the {} desktop from {} ({} \u{00B7} {} \u{00B7} {auth}) — {live_status}.",
                            req.protocol.client_crate(),
                            endpoint,
                            req.display.label(),
                            req.monitors.label(),
                        )
                    } else {
                        format!(
                            "Spice desktop from {} ({} \u{00B7} {} \u{00B7} {auth}) — the Spice client lands in CHOOSER-5; no session is faked.",
                            req.target.serving_peer,
                            req.display.label(),
                            req.monitors.label(),
                        )
                    };
                    crate::backdrop::show(
                        ui,
                        crate::backdrop::Coverage::Empty,
                        Some((title.as_str(), detail.as_str())),
                    );
                }
                None => crate::backdrop::show(
                    ui,
                    crate::backdrop::Coverage::Empty,
                    Some((
                        "No desktop connected",
                        "Broker a VM desktop (RDP / VNC) — it renders here in the shell.",
                    )),
                ),
            }
        }
    }
}

/// Forward this frame's egui input to the attached guest, reserving the Esc chord.
///
/// Esc releases the desktop back to the mesh-control chrome instead of reaching
/// the guest, so the operator is never trapped in a fullscreen session. Every
/// other event is handed to the session, which maps the ones it understands
/// (pointer / button / wheel / key / text) and drops the rest.
fn forward_input(ui: &egui::Ui, state: &mut VdiState) {
    let has_live = {
        #[cfg(feature = "live-vdi")]
        {
            state.live_rdp.is_some()
        }
        #[cfg(not(feature = "live-vdi"))]
        {
            false
        }
    };
    if state.session.is_none() && !has_live {
        return;
    }
    for event in ui.input(|i| i.events.clone()) {
        if matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Escape,
                pressed: true,
                ..
            }
        ) {
            state.return_to_chrome = true;
            continue;
        }
        if let Some(session) = state.session.as_mut() {
            session.send_input(&event);
        }
        #[cfg(feature = "live-vdi")]
        if let Some(live) = state.live_rdp.as_ref() {
            live.send_input(event);
        }
    }
}

// ─────────────────────────── MENUBAR-ALL (Desktop) ──────────────────────────

/// One action the Desktop menu bar dispatches — each routes to a real seam the
/// shell already owns (§6, no new behaviour).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DesktopMenuAction {
    /// Release the desktop back to the mesh-control chrome — the SAME seam the Esc
    /// chord raises ([`VdiState::request_return_to_chrome`]).
    ReturnToChrome,
    /// Force the Chooser to re-read its published desktop-source roster now
    /// ([`crate::chooser::ChooserState::refresh_now`]).
    RefreshSources,
}

/// Render the Desktop surface's shared top bar (DESKTOP) and return the action the
/// operator picked this frame, if any (MENUBAR-ALL). The Desktop surface has two
/// honest faces — the **Chooser** (no session) and the brokered **desktop** (a
/// pending / live connect) — so its two menus are gated to the face that owns the
/// seam (§7 — a context-gated item disables, never a silent no-op):
///
/// * **Session → Return to Mesh Control** — the Esc-chord twin, live only while a
///   desktop connect is pending / attached.
/// * **View → Refresh Sources** — re-enumerate the Chooser's roster now, live only
///   while the Chooser (no session) is showing.
///
/// The status cluster names the pending desktop + protocol, or the Chooser's live
/// source count. The bar is decoupled from both states (it takes plain readouts) so
/// the shell can mount it above whichever face renders below.
pub(crate) fn desktop_menubar(
    ui: &mut egui::Ui,
    pending: Option<(&str, &str)>,
    source_count: usize,
) -> Option<DesktopMenuAction> {
    use mde_egui::menubar::{MenuBar, MenuBarModel};
    use mde_egui::Style;

    let menus = build_desktop_menus(pending.is_some());
    let status = build_desktop_status(pending, source_count);
    let model = MenuBarModel {
        // The dock tints the Show-Desktop cell with the brand accent (its
        // system-quad cell carries no group hue), so the title matches (lock 2).
        title: "Desktop",
        accent: Style::ACCENT,
        menus: &menus,
        status: &status,
    };
    MenuBar::show(ui, &model)
}

/// The two Desktop menus, each gated to the face that owns its seam (§7): Session →
/// Return to Mesh Control is live only while a connect is pending; View → Refresh
/// Sources only on the Chooser. Every item is present, one is disabled — never a
/// dead/omitted entry.
fn build_desktop_menus(connected: bool) -> Vec<mde_egui::menubar::Menu<DesktopMenuAction>> {
    use mde_egui::menubar::{Entry, Item, Menu};
    vec![
        Menu::new(
            "Session",
            vec![Entry::Item(
                Item::new(DesktopMenuAction::ReturnToChrome, "Return to Mesh Control")
                    .shortcut("Esc")
                    .enabled(connected),
            )],
        ),
        Menu::new(
            "View",
            vec![Entry::Item(
                Item::new(DesktopMenuAction::RefreshSources, "Refresh Sources").enabled(!connected),
            )],
        ),
    ]
}

/// The Desktop status cluster: the pending desktop + protocol, or the Chooser's
/// live source count — real state either way (§7).
fn build_desktop_status(
    pending: Option<(&str, &str)>,
    source_count: usize,
) -> Vec<mde_egui::StatusChip> {
    use mde_egui::{ChipTone, StatusChip};
    match pending {
        Some((name, protocol)) => vec![StatusChip::with_icon(
            "\u{25B6}",
            format!("{name} \u{00B7} {protocol}"),
            ChipTone::Info,
        )],
        None => vec![
            StatusChip::new("No desktop", ChipTone::Neutral),
            StatusChip::new(
                format!(
                    "{source_count} source{}",
                    if source_count == 1 { "" } else { "s" }
                ),
                ChipTone::Neutral,
            ),
        ],
    }
}

/// A small deterministic RGBA gradient standing in for a decoded desktop frame —
/// the render test drives the upload + paint path without a live server.
#[cfg(test)]
pub(crate) fn mock_frame() -> egui::ColorImage {
    const W: usize = 16;
    const H: usize = 12;
    let mut rgba = Vec::with_capacity(W * H * 4);
    for y in 0..H {
        let g = u8::try_from(y * 255 / (H - 1)).expect("gradient byte in 0..=255");
        for x in 0..W {
            let r = u8::try_from(x * 255 / (W - 1)).expect("gradient byte in 0..=255");
            rgba.extend_from_slice(&[r, g, 128, 255]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([W, H], &rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{Credential, DesktopAuth};
    use mde_egui::egui::{pos2, vec2, Rect};
    use mde_egui::Style;
    use mde_vdi_rdp::RdpConfig;
    use mde_vdi_vnc::VncConfig;

    /// A headless 960×640 shell body, mirroring the E12-3b render test.
    fn body_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        }
    }

    /// Drive one headless frame of `vdi_panel` and tessellate it on the CPU, the
    /// same `Context::run` → `tessellate` path the DRM runner drives minus the GPU.
    fn run_panel(state: &mut VdiState, input: egui::RawInput) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| vdi_panel(ui, state));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        !prims.is_empty()
    }

    #[test]
    fn no_session_paints_the_empty_state_not_a_blank_panel() {
        let mut state = VdiState::default();
        let drew = run_panel(&mut state, body_input());
        assert!(state.texture.is_none(), "no frame attached, so no texture");
        assert!(
            drew,
            "the no-desktop BRAND-1 logo backdrop produced no draw primitives"
        );
    }

    #[test]
    fn a_requested_connect_paints_the_connecting_caption() {
        // The Chooser's picker handed a connect but no live decoder is attached
        // yet (the wire transport is gated): the surface shows the connecting
        // caption, still with no texture and no fake desktop.
        let mut state = VdiState::default();
        state.request_connect(ConnectRequest::new(
            RequestedTarget::new("node-a", "web1"),
            VdiProtocol::Rdp,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity("node-a"),
        ));
        assert_eq!(
            state.requested_target().map(|t| t.name.as_str()),
            Some("web1")
        );
        let drew = run_panel(&mut state, body_input());
        assert!(state.texture.is_none(), "no frame attached, so no texture");
        assert!(
            drew,
            "the connecting backdrop (logo + status below) produced no draw primitives"
        );

        // Backing out clears the connect so the surface returns to the picker.
        state.clear_target();
        assert!(state.requested_target().is_none());
    }

    #[test]
    fn a_gated_spice_connect_paints_without_faking_a_session() {
        // A Spice request is constructed honestly, but no client crate exists
        // (CHOOSER-5), so the surface stays on the connecting caption — it never
        // constructs a `Session::Spice` (there is none) nor a fake desktop (§7).
        let mut state = VdiState::default();
        state.request_connect(ConnectRequest::new(
            RequestedTarget::new("oak", "win11"),
            VdiProtocol::Spice,
            DisplayMode::Windowed,
            MonitorSpan::All,
            DesktopAuth::mesh_identity("oak"),
        ));
        let drew = run_panel(&mut state, body_input());
        assert!(state.session.is_none(), "no Spice session is faked");
        assert!(state.texture.is_none(), "no fake desktop texture");
        assert!(
            drew,
            "the gated-Spice connecting caption produced no draw primitives"
        );
    }

    #[test]
    fn the_vdi_protocol_routes_map_to_the_right_client_crate() {
        assert_eq!(VdiProtocol::Rdp.client_crate(), "mde-vdi-rdp");
        assert_eq!(VdiProtocol::Vnc.client_crate(), "mde-vdi-vnc");
        assert_eq!(VdiProtocol::Spice.client_crate(), "mde-vdi-spice");
        // RDP/VNC render today; Spice is CHOOSER-5-gated.
        assert!(VdiProtocol::Rdp.has_client());
        assert!(VdiProtocol::Vnc.has_client());
        assert!(!VdiProtocol::Spice.has_client());
    }

    #[test]
    fn a_connect_request_carries_the_three_display_choices() {
        // The request-construction fold: the picked target + the three choices
        // land on the request verbatim.
        let req = ConnectRequest::new(
            RequestedTarget::new("oak", "web1")
                .with_endpoint(DesktopEndpoint::new("10.42.0.9", 5900)),
            VdiProtocol::Vnc,
            DisplayMode::Windowed,
            MonitorSpan::All,
            DesktopAuth::Sealed {
                store_ref: "desktop/oak/vnc".to_string(),
                credential: Credential::new("admin", "rfb-secret"),
            },
        );
        assert_eq!(req.target.serving_peer, "oak");
        assert_eq!(req.target.name, "web1");
        assert_eq!(
            req.target.endpoint.as_ref().map(DesktopEndpoint::label),
            Some("10.42.0.9:5900".to_string())
        );
        assert_eq!(req.protocol, VdiProtocol::Vnc);
        assert_eq!(req.display, DisplayMode::Windowed);
        assert_eq!(req.monitors, MonitorSpan::All);
        assert_eq!(req.display.label(), "windowed");
        assert_eq!(req.monitors.label(), "span all displays");
        // CHOOSER-6 — the resolved auth rides the request; its secret is redacted
        // from Debug so the request stays log-safe.
        assert_eq!(req.auth.summary(), "sealed credential (admin)");
        assert!(!format!("{req:?}").contains("rfb-secret"));
    }

    #[test]
    fn invalid_desktop_endpoints_are_rejected_before_the_live_transport() {
        assert!(DesktopEndpoint::new("", 3389).is_none());
        assert!(DesktopEndpoint::new("10.42.0.9", 0).is_none());
        assert_eq!(
            DesktopEndpoint::new("10.42.0.9", 3389).map(|endpoint| endpoint.label()),
            Some("10.42.0.9:3389".to_string())
        );
    }

    #[test]
    fn an_attached_frame_is_uploaded_to_a_texture_and_painted() {
        // The decode side hands the panel a frame; the panel uploads + paints it.
        let mut state = VdiState {
            incoming: Some(mock_frame()),
            ..Default::default()
        };
        let drew = run_panel(&mut state, body_input());
        assert!(
            state.texture.is_some(),
            "the attached frame was not uploaded to a texture"
        );
        assert!(drew, "the desktop image produced no draw primitives");
    }

    #[test]
    fn a_live_rdp_session_frame_flows_to_the_texture() {
        // Proves the shell is a real caller of `mde-vdi-rdp`: a fresh session marks
        // its framebuffer dirty, so the panel pulls a `frame()` and uploads it with
        // no server in the loop.
        let session =
            RdpSession::new(RdpConfig::new("host", "user", "pw").with_resolution(640, 480))
                .expect("valid RDP config");
        let mut state = VdiState {
            session: Some(Session::Rdp(session)),
            ..Default::default()
        };
        run_panel(&mut state, body_input());
        assert!(
            state.texture.is_some(),
            "the RDP session's first frame was not pulled and uploaded"
        );
    }

    #[test]
    fn the_menu_return_seam_matches_the_esc_chord() {
        // MENUBAR-ALL: the Session → Return to Mesh Control menu path raises the SAME
        // `return_to_chrome` the Esc chord does, drained by `take_return_to_chrome`.
        let mut state = VdiState::default();
        assert!(!state.take_return_to_chrome());
        state.request_return_to_chrome();
        assert!(
            state.take_return_to_chrome(),
            "the menu return seam raises the chrome-return request"
        );
        assert!(
            !state.take_return_to_chrome(),
            "and it is one-shot, like Esc"
        );
    }

    #[test]
    fn requested_summary_names_the_pending_desktop_and_protocol() {
        let mut state = VdiState::default();
        assert!(
            state.requested_summary().is_none(),
            "no connect ⇒ no summary"
        );
        state.request_connect(ConnectRequest::new(
            RequestedTarget::new("node-a", "win11"),
            VdiProtocol::Rdp,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity("node-a"),
        ));
        assert_eq!(state.requested_summary(), Some(("win11", "RDP")));
    }

    #[test]
    fn the_desktop_menus_gate_each_face_to_its_own_seam() {
        use super::{build_desktop_menus, build_desktop_status};
        use mde_egui::menubar::Entry;
        use mde_egui::ChipTone;

        // The enable state of each menu item is a pure function of `connected`:
        // Return-to-Chrome is live only while connected, Refresh only on the Chooser.
        // Every item is present in both faces — one is disabled, never omitted (§7).
        for connected in [false, true] {
            let menus = build_desktop_menus(connected);
            let item_enabled = |title: &str| -> bool {
                menus
                    .iter()
                    .find(|m| m.title == title)
                    .and_then(|m| {
                        m.entries.iter().find_map(|e| match e {
                            Entry::Item(i) => Some(i.enabled),
                            _ => None,
                        })
                    })
                    .expect("the menu's item is present")
            };
            assert_eq!(
                item_enabled("Session"),
                connected,
                "Return is live iff connected"
            );
            assert_eq!(
                item_enabled("View"),
                !connected,
                "Refresh is live iff on the Chooser"
            );
        }

        // The status cluster reflects the live face.
        let chooser = build_desktop_status(None, 3);
        assert!(chooser.iter().any(|c| c.text == "3 sources"));
        assert!(chooser.iter().any(|c| c.text == "No desktop"));
        let connected = build_desktop_status(Some(("win11", "RDP")), 3);
        assert!(connected.iter().any(|c| c.text.contains("win11")
            && c.text.contains("RDP")
            && c.tone == ChipTone::Info));
    }

    #[test]
    fn the_desktop_bar_renders_headless_in_both_faces() {
        use mde_egui::egui::{pos2, vec2, Rect};
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 640.0))),
            ..Default::default()
        };
        for pending in [None, Some(("win11", "RDP"))] {
            let out = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = desktop_menubar(ui, pending, 3);
                });
            });
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            assert!(!prims.is_empty(), "a Desktop bar face drew nothing");
        }
    }

    #[test]
    fn input_forwards_to_a_vnc_session_and_esc_returns_to_chrome() {
        // The VNC console fallback receives forwarded pointer input, and the
        // reserved Esc chord raises `return_to_chrome` rather than reaching guest.
        let session = VncSession::new(VncConfig::new("host")).expect("valid VNC config");
        let mut state = VdiState {
            session: Some(Session::Vnc(session)),
            ..Default::default()
        };
        let mut input = body_input();
        input.events = vec![
            egui::Event::PointerMoved(pos2(120.0, 90.0)),
            egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            },
        ];
        run_panel(&mut state, input);

        assert!(
            state.return_to_chrome,
            "Esc did not raise the return-to-chrome chord"
        );
        assert!(
            matches!(
                &state.session,
                Some(Session::Vnc(s)) if s.pointer_position() != (0, 0)
            ),
            "the pointer event was not forwarded to the guest"
        );
    }
}
