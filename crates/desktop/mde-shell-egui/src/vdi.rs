//! The VDI **Desktop** surface — a remote VM desktop rendered egui-native.
//!
//! E12 "Quasar" brokers VM desktops *into* the one shell (§5 EMBED, lock 21):
//! there is no external viewer. The remote framebuffer is decoded by
//! `mde-vdi-rdp` (RDP-primary), `mde-vdi-vnc` (VNC / XAPI-console fallback), or
//! `mde-vdi-spice` (native QEMU/KVM console) into an [`egui::ColorImage`]; this
//! panel uploads that image to a `TextureHandle` and paints it as the shell body,
//! and forwards the frame's egui input straight back to the session's input
//! mapper.
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
use mde_vdi_spice::SpiceSession;
use mde_vdi_vnc::VncSession;

use crate::auth::DesktopAuth;

use std::path::PathBuf;

#[cfg(feature = "live-vdi")]
use {
    mde_vdi_rdp::{PumpOutcome, RdpConfig, RdpConnection},
    mde_vdi_spice::{BlockingSpiceTransport, SpiceConfig},
    mde_vdi_vnc::{PumpOutcome as VncPumpOutcome, VncConfig, VncConnection},
    std::sync::mpsc,
    std::thread,
    std::time::Duration,
};

/// A live VDI desktop the shell drives — RDP-primary, VNC the console fallback,
/// and SPICE for native QEMU/KVM consoles. The decoder crates expose the same
/// egui-facing surface
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
    /// A SPICE desktop (`mde-vdi-spice`, the native QEMU/KVM console).
    Spice(SpiceSession),
}

impl Session {
    /// The latest decoded desktop, or `None` if nothing changed since last frame.
    fn frame(&mut self) -> Option<egui::ColorImage> {
        match self {
            Session::Rdp(s) => s.frame(),
            Session::Vnc(s) => s.frame(),
            Session::Spice(s) => s.frame(),
        }
    }

    /// Forward one egui input event to the guest — the session maps it to the
    /// protocol's pointer / key / wheel / text intents internally.
    fn send_input(&mut self, event: &egui::Event) {
        match self {
            Session::Rdp(s) => s.send_input(event),
            Session::Vnc(s) => s.send_input(event),
            Session::Spice(s) => s.send_input(event),
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

/// The retained Bus topic the serving peer's `console_broker` (VDI-VM-1) publishes
/// brokered console endpoints to. MUST equal
/// `mackesd::workers::console_broker::CONSOLE_TOPIC` (a cross-check test pins it).
pub(crate) const CONSOLE_TOPIC: &str = "state/vdi/console";

/// The shell's read mirror of `console_broker`'s brokered-console status — only the
/// fields the transport needs. serde ignores the rest (e.g. the record's `protocol`
/// tag: the transport uses the operator's chosen protocol, the record only supplies
/// the dialable `host:port`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum BrokeredConsoleStatus {
    /// A reachable overlay endpoint was brokered.
    Brokered {
        /// The Nebula overlay address the serving peer's relay listens on.
        host: String,
        /// The overlay port.
        port: u16,
    },
    /// No reachable endpoint could be brokered — the honest reason.
    Unbrokerable {
        /// The reason surfaced to the operator.
        reason: String,
    },
}

/// The shell's read mirror of one `console_broker` record on [`CONSOLE_TOPIC`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct BrokeredConsoleRecord {
    /// The session this console serves — the globally-unique correlation key the
    /// shell matches against its minted [`BrokerSessionLifecycle::id`].
    session_id: String,
    /// The brokered endpoint, or the honest reason none could be brokered.
    status: BrokeredConsoleStatus,
}

/// The outcome of resolving a brokered console endpoint from the session record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConsoleResolution {
    /// No record yet — the serving peer's broker hasn't published one (keep
    /// waiting; the connect stays in an honest "resolving" state).
    Pending,
    /// A dialable overlay endpoint the transport attaches to.
    Ready(DesktopEndpoint),
    /// The broker honestly reported it CANNOT make a reachable endpoint — the shell
    /// greys the lane and never attaches a doomed transport (§7).
    Unbrokerable(String),
}

/// Resolve the brokered console endpoint for `session_id` from the raw
/// [`CONSOLE_TOPIC`] record bodies (the latest matching record wins — the broker
/// republishes when a session's console state changes). Pure + headless-testable;
/// the Bus read that feeds it is `read_console_bodies`.
pub(crate) fn resolve_brokered_console(bodies: &[String], session_id: &str) -> ConsoleResolution {
    let mut out = ConsoleResolution::Pending;
    for body in bodies {
        let Ok(rec) = serde_json::from_str::<BrokeredConsoleRecord>(body) else {
            continue;
        };
        if rec.session_id != session_id {
            continue;
        }
        out = match rec.status {
            BrokeredConsoleStatus::Brokered { host, port } => DesktopEndpoint::new(host, port)
                .map_or_else(
                    || {
                        ConsoleResolution::Unbrokerable(
                            "the broker published an unusable endpoint".to_string(),
                        )
                    },
                    ConsoleResolution::Ready,
                ),
            BrokeredConsoleStatus::Unbrokerable { reason } => {
                ConsoleResolution::Unbrokerable(reason)
            }
        };
    }
    out
}

/// Read the raw brokered-console record bodies off [`CONSOLE_TOPIC`] — a
/// non-blocking local spool scan (empty when there's no Bus dir / nothing
/// published), mirroring the Chooser's `BusDesktopSources::latest` read idiom.
#[cfg(feature = "live-vdi")]
fn read_console_bodies(bus_root: Option<&std::path::Path>) -> Vec<String> {
    let Some(root) = bus_root else {
        return Vec::new();
    };
    let Ok(persist) = mde_bus::persist::Persist::open(root.to_path_buf()) else {
        return Vec::new();
    };
    persist
        .list_since(CONSOLE_TOPIC, None)
        .map(|msgs| msgs.into_iter().filter_map(|m| m.body).collect())
        .unwrap_or_default()
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
    /// Spice — `mde-vdi-spice` (native QEMU/KVM console).
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

    /// Whether a decoder crate exists to render this protocol today.
    pub(crate) const fn has_client(self) -> bool {
        matches!(self, Self::Rdp | Self::Vnc | Self::Spice)
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
/// live wire transport that constructs the session is the gated E12-4 layer; a
/// request is still built truthfully while the transport resolves, and no
/// placeholder session is ever faked (§7).
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
    /// vdi-vm-8 — the guest desktop size hint in **device pixels** (the shell's real
    /// output size at connect time, [`body_device_px`]), so an RDP/SPICE guest renders
    /// at near-native resolution instead of a hardcoded 1024×768 that egui upscales
    /// (blurry on modern seats). RDP/SPICE pass it at connect ([`with_resolution`] /
    /// [`with_size`]); VNC's size is server-negotiated so it is ignored there. When
    /// absent (bus-driven / test paths) the transport falls back to its prior
    /// hardcoded size.
    ///
    /// On a MATERIAL panel resize *after* connect (a seat / monitor resolution change,
    /// not a chrome toggle) an RDP/SPICE session is re-dialed at the new panel size —
    /// the only live re-negotiation the thin transports expose (a fresh connect;
    /// `note_resize_target` + `poll_resize_renegotiate`). The LINEAR upscale bridges
    /// the sub-second re-dial gap and remains the fallback for smaller deltas and for
    /// VNC (server-authoritative). The pointer transform keeps clicks correct
    /// throughout.
    ///
    /// [`body_device_px`]: crate::vdi::body_device_px
    /// [`with_resolution`]: mde_vdi_rdp::RdpConfig::with_resolution
    /// [`with_size`]: mde_vdi_spice::SpiceConfig::with_size
    pub preferred_size: Option<(u16, u16)>,
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
            preferred_size: None,
        }
    }

    /// Attach the broker session lifecycle id minted by discovery.
    pub(crate) fn with_broker_session(mut self, broker: BrokerSessionLifecycle) -> Self {
        self.broker_session = Some(broker);
        self
    }

    /// Attach the initial desktop size hint (device pixels) for RDP/SPICE
    /// negotiation (vdi-vm-8). `None` keeps the transport's fallback size.
    #[must_use]
    pub(crate) const fn with_preferred_size(mut self, size: Option<(u16, u16)>) -> Self {
        self.preferred_size = size;
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
enum LiveVncEvent {
    Connected(String),
    Frame(egui::ColorImage),
    Error(String),
    Ended(String),
}

#[cfg(feature = "live-vdi")]
struct LiveVncHandle {
    input_tx: mpsc::Sender<egui::Event>,
    stop_tx: mpsc::Sender<()>,
    event_rx: mpsc::Receiver<LiveVncEvent>,
}

#[cfg(feature = "live-vdi")]
enum LiveSpiceEvent {
    Connected(String),
    Frame(egui::ColorImage),
    Error(String),
    Ended(String),
}

#[cfg(feature = "live-vdi")]
struct LiveSpiceHandle {
    input_tx: mpsc::Sender<egui::Event>,
    stop_tx: mpsc::Sender<()>,
    event_rx: mpsc::Receiver<LiveSpiceEvent>,
}

#[cfg(feature = "live-vdi")]
impl LiveRdpHandle {
    fn spawn(request: &ConnectRequest) -> Result<Self, String> {
        let Some(endpoint) = request.target.endpoint.clone() else {
            return Err("discovery has not published a dialable endpoint for this desktop".into());
        };
        let credential = live_rdp_credential(request)?;
        if credential.username.trim().is_empty() {
            return Err("RDP requires a username in the sealed desktop credential".into());
        }

        let (width, height) = rdp_initial_resolution(request.preferred_size);
        let config = RdpConfig::new(
            endpoint.host.clone(),
            credential.username.clone(),
            credential.secret.expose().to_owned(),
        )
        .with_port(endpoint.port)
        .with_resolution(width, height);
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
impl LiveVncHandle {
    fn spawn(request: &ConnectRequest) -> Result<Self, String> {
        let config = live_vnc_config(request)?;
        let (input_tx, input_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        thread::Builder::new()
            .name(format!("mde-live-vnc-{}", request.target.name))
            .spawn(move || run_live_vnc(config, input_rx, stop_rx, event_tx))
            .map_err(|e| format!("failed to spawn live VNC worker: {e}"))?;

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
impl LiveSpiceHandle {
    fn spawn(request: &ConnectRequest) -> Result<Self, String> {
        let config = live_spice_config(request)?;
        let (input_tx, input_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        thread::Builder::new()
            .name(format!("mde-live-spice-{}", request.target.name))
            .spawn(move || run_live_spice(config, input_rx, stop_rx, event_tx))
            .map_err(|e| format!("failed to spawn live SPICE worker: {e}"))?;

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
fn live_rdp_credential(request: &ConnectRequest) -> Result<&crate::auth::Credential, String> {
    match &request.auth {
        DesktopAuth::Sealed { credential, .. } => Ok(credential),
        DesktopAuth::MeshIdentity {
            guest: Some(guest), ..
        } => Ok(&guest.credential),
        DesktopAuth::MeshIdentity { guest: None, .. } => {
            Err("mesh-gated RDP needs a sealed guest credential for OS login".into())
        }
    }
}

#[cfg(feature = "live-vdi")]
fn live_vnc_config(request: &ConnectRequest) -> Result<VncConfig, String> {
    let Some(endpoint) = request.target.endpoint.clone() else {
        return Err("discovery has not published a dialable endpoint for this desktop".into());
    };
    let mut config = VncConfig::new(endpoint.host)
        .with_port(endpoint.port)
        .shared(true);
    match &request.auth {
        DesktopAuth::Sealed { credential, .. } => {
            if !credential.secret.expose().is_empty() {
                config = config.with_password(credential.secret.expose().to_owned());
            }
        }
        DesktopAuth::MeshIdentity {
            guest: Some(guest), ..
        } => {
            if !guest.credential.secret.expose().is_empty() {
                config = config.with_password(guest.credential.secret.expose().to_owned());
            }
        }
        DesktopAuth::MeshIdentity { guest: None, .. } => {
            // XCP-ng console fallback is mesh/dom0-route gated and usually exposes
            // RFB security type None; no guest credential is required for that path.
        }
    }
    Ok(config)
}

#[cfg(feature = "live-vdi")]
fn live_spice_config(request: &ConnectRequest) -> Result<SpiceConfig, String> {
    let Some(endpoint) = request.target.endpoint.clone() else {
        return Err("discovery has not published a dialable endpoint for this desktop".into());
    };
    let (width, height) = spice_initial_size(request.preferred_size);
    let mut config = SpiceConfig::new(endpoint.host)
        .with_port(endpoint.port)
        .with_size(width, height);
    match &request.auth {
        DesktopAuth::Sealed { credential, .. } => {
            if !credential.secret.expose().is_empty() {
                config = config.with_password(credential.secret.expose().to_owned());
            }
        }
        DesktopAuth::MeshIdentity {
            guest: Some(guest), ..
        } => {
            if !guest.credential.secret.expose().is_empty() {
                config = config.with_password(guest.credential.secret.expose().to_owned());
            }
        }
        DesktopAuth::MeshIdentity { guest: None, .. } => {
            // Mesh-gated QEMU/KVM consoles commonly carry no SPICE ticket; if a
            // ticket is required, discovery/auth provides it as the optional guest
            // credential above.
        }
    }
    Ok(config)
}

/// Clamp a vdi-vm-8 device-pixel size hint into a legal RDP desktop resolution.
///
/// `RdpConfig` requires 200..=8192 px per axis and an **even** width, so the hint
/// is clamped and the width forced even. Falls back to the prior hardcoded
/// 1024×768 when the shell published no hint (bus-driven / headless connect).
#[cfg(feature = "live-vdi")]
fn rdp_initial_resolution(preferred: Option<(u16, u16)>) -> (u16, u16) {
    match preferred {
        Some((w, h)) => (
            w.clamp(RdpConfig::MIN_DIM, RdpConfig::MAX_DIM) & !1u16,
            h.clamp(RdpConfig::MIN_DIM, RdpConfig::MAX_DIM),
        ),
        None => (1024, 768),
    }
}

/// Clamp a vdi-vm-8 device-pixel size hint into a legal SPICE framebuffer size.
///
/// `SpiceConfig` allows 16..=8192 px per axis. Falls back to 1024×768 when the
/// shell published no hint.
#[cfg(feature = "live-vdi")]
fn spice_initial_size(preferred: Option<(u16, u16)>) -> (u16, u16) {
    match preferred {
        Some((w, h)) => (
            w.clamp(SpiceConfig::MIN_DIM, SpiceConfig::MAX_DIM),
            h.clamp(SpiceConfig::MIN_DIM, SpiceConfig::MAX_DIM),
        ),
        None => (1024, 768),
    }
}

#[cfg(feature = "live-vdi")]
impl Drop for LiveRdpHandle {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
    }
}

#[cfg(feature = "live-vdi")]
impl Drop for LiveVncHandle {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
    }
}

#[cfg(feature = "live-vdi")]
impl Drop for LiveSpiceHandle {
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

#[cfg(feature = "live-vdi")]
fn run_live_vnc(
    config: VncConfig,
    input_rx: mpsc::Receiver<egui::Event>,
    stop_rx: mpsc::Receiver<()>,
    event_tx: mpsc::Sender<LiveVncEvent>,
) {
    let target = format!("{}:{}", config.host, config.port);
    let mut session = match VncSession::new(config) {
        Ok(session) => session,
        Err(e) => {
            let _ = event_tx.send(LiveVncEvent::Error(format!("VNC config rejected: {e}")));
            return;
        }
    };
    let mut conn = match VncConnection::connect(&mut session) {
        Ok(conn) => conn,
        Err(e) => {
            let _ = event_tx.send(LiveVncEvent::Error(format!("VNC connect failed: {e}")));
            return;
        }
    };
    let negotiated = conn.negotiated().clone();
    let _ = event_tx.send(LiveVncEvent::Connected(format!(
        "{target} (RFB {}.{}, {}x{}, {:?})",
        negotiated.major, negotiated.minor, negotiated.width, negotiated.height, negotiated.name
    )));
    if let Some(frame) = session.frame() {
        let _ = event_tx.send(LiveVncEvent::Frame(frame));
    }

    loop {
        if stop_rx.try_recv().is_ok() {
            conn.shutdown();
            return;
        }

        let mut had_input = false;
        while let Ok(event) = input_rx.try_recv() {
            session.send_input(&event);
            had_input = true;
        }
        if had_input {
            if let Err(e) = conn.flush_input(&mut session) {
                let _ = event_tx.send(LiveVncEvent::Error(format!("VNC input failed: {e}")));
                return;
            }
        }

        match conn.pump_once(&mut session, Duration::from_millis(50)) {
            Ok(VncPumpOutcome::Processed { rects, .. }) => {
                if rects > 0 {
                    if let Some(frame) = session.frame() {
                        let _ = event_tx.send(LiveVncEvent::Frame(frame));
                    }
                }
            }
            Ok(VncPumpOutcome::TimedOut) => {}
            Ok(VncPumpOutcome::Terminated { reason }) => {
                let _ = event_tx.send(LiveVncEvent::Ended(reason));
                return;
            }
            Err(e) => {
                let _ = event_tx.send(LiveVncEvent::Error(format!("VNC pump failed: {e}")));
                return;
            }
        }
    }
}

#[cfg(feature = "live-vdi")]
fn run_live_spice(
    config: SpiceConfig,
    input_rx: mpsc::Receiver<egui::Event>,
    stop_rx: mpsc::Receiver<()>,
    event_tx: mpsc::Sender<LiveSpiceEvent>,
) {
    let target = format!("{}:{}", config.host, config.port);
    let mut session = match SpiceSession::new(config.clone()) {
        Ok(session) => session,
        Err(e) => {
            let _ = event_tx.send(LiveSpiceEvent::Error(format!("SPICE config rejected: {e}")));
            return;
        }
    };
    let mut conn = match BlockingSpiceTransport::connect(&config) {
        Ok(conn) => conn,
        Err(e) => {
            let _ = event_tx.send(LiveSpiceEvent::Error(format!("SPICE connect failed: {e}")));
            return;
        }
    };
    let _ = event_tx.send(LiveSpiceEvent::Connected(target));
    if let Some(frame) = session.frame() {
        let _ = event_tx.send(LiveSpiceEvent::Frame(frame));
    }

    loop {
        if stop_rx.try_recv().is_ok() {
            let _ = event_tx.send(LiveSpiceEvent::Ended(
                "SPICE session stopped by shell".to_string(),
            ));
            return;
        }

        let mut had_input = false;
        while let Ok(event) = input_rx.try_recv() {
            session.send_input(&event);
            had_input = true;
        }
        if had_input {
            if let Err(e) = conn.flush_input(&mut session) {
                let _ = event_tx.send(LiveSpiceEvent::Error(format!("SPICE input failed: {e}")));
                return;
            }
        }

        match conn.pump_frame(&mut session) {
            Ok(true) => {
                if let Some(frame) = session.frame() {
                    let _ = event_tx.send(LiveSpiceEvent::Frame(frame));
                }
            }
            Ok(false) => {}
            Err(e) => {
                let _ = event_tx.send(LiveSpiceEvent::Error(format!("SPICE pump failed: {e}")));
                return;
            }
        }
    }
}

// ───────────────────── vdi-vm-4 / shell-ux-1: session state ──────────────────
//
// A live transport can DROP (the server closes, or a pump errors) at any time.
// Before this, a drop froze the desktop at its last frame with no recovery and no
// honest status (shell-ux-1). Now a drop that is NOT a user-initiated close drives
// a small session-state machine: the shell auto-reconnects to the SAME endpoint
// with bounded retries + capped backoff (vdi-vm-4), and paints an honest overlay
// with Retry / pick-a-different affordances the whole time (shell-ux-1).
//
// The user-close vs transport-drop distinction is STRUCTURAL, not a flag: a clean
// close ([`VdiState::clear_target`] / [`VdiState::request_connect`]) TAKES the live
// handle before any poll re-reads it AND resets the phase to `Live`, so a drop is
// only ever driven by [`VdiState::on_transport_drop`] for an INSTALLED handle whose
// worker thread died on its own — a real drop. (The transports confirm this: their
// worker sends `Error`/`Ended` ONLY on a pump error / server termination; a
// shell-requested stop returns with NO event, and by then its handle is gone.)

/// The bounded auto-reconnect budget (vdi-vm-4): after this many failed re-dials the
/// session gives up and shows the honest Failed overlay instead of retrying forever.
#[cfg(feature = "live-vdi")]
const MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// vdi-vm-4 / shell-ux-1 — the live desktop session's connection phase. A transport
/// drop walks `Live → Reconnecting{attempt} → … → Failed{reason}`; a fresh frame
/// from a re-dialed transport walks it back to `Live`. Drives BOTH the auto-
/// reconnect scheduler and the honest overlay, so the two can never diverge.
#[cfg(feature = "live-vdi")]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum SessionPhase {
    /// The transport is live, or connecting normally on the initial dial — no
    /// overlay, no pending reconnect.
    #[default]
    Live,
    /// A drop was detected; the shell is auto-reconnecting to the SAME endpoint.
    /// `attempt` is 1-based; `reason` is the honest last drop reason.
    Reconnecting { attempt: u32, reason: String },
    /// Auto-reconnect exhausted its budget (or a re-dial could not even start) — the
    /// honest failure reason, surfaced with Retry / pick-a-different.
    Failed { reason: String },
}

/// vdi-vm-4 — capped exponential backoff before reconnect `attempt` (1-based): 0.5s,
/// 1s, 2s, 4s, then held at 8s. Bounds the reconnect storm against a flapping peer.
#[cfg(feature = "live-vdi")]
fn reconnect_backoff(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(4);
    let ms = 500u64.saturating_mul(1u64 << shift);
    Duration::from_millis(ms.min(8_000))
}

/// vdi-vm-4 — the pure transition on a detected transport drop. A drop from `Live`
/// opens attempt 1; each further drop while `Reconnecting` bumps the attempt until
/// `max` is spent, then the session is `Failed` with the honest reason. `Failed` is
/// terminal (an explicit operator Retry resets it — [`VdiState::retry_now`]).
#[cfg(feature = "live-vdi")]
fn next_phase_on_drop(current: &SessionPhase, reason: String, max: u32) -> SessionPhase {
    match current {
        SessionPhase::Live => SessionPhase::Reconnecting { attempt: 1, reason },
        SessionPhase::Reconnecting { attempt, .. } if *attempt < max => {
            SessionPhase::Reconnecting {
                attempt: attempt + 1,
                reason,
            }
        }
        SessionPhase::Reconnecting { .. } | SessionPhase::Failed { .. } => {
            SessionPhase::Failed { reason }
        }
    }
}

/// vdi-vm-8 — a live RDP/SPICE desktop is re-negotiated (re-dialed at the panel's
/// current size) only once the guest's real desktop diverges from the panel by more
/// than this many device pixels on either axis. Set well above the dock / menubar
/// chrome deltas — which the LINEAR upscale absorbs imperceptibly — so a chrome
/// toggle never triggers a disruptive re-dial; only a real seat / monitor resolution
/// change does.
#[cfg(feature = "live-vdi")]
const RESIZE_RENEGOTIATE_THRESHOLD_PX: u16 = 128;

/// vdi-vm-8 — the new panel size must hold steady this long before a resize re-dial
/// fires, so dragging / animating a resize collapses to a SINGLE re-negotiation
/// instead of a reconnect storm.
#[cfg(feature = "live-vdi")]
const RESIZE_SETTLE: Duration = Duration::from_millis(600);

/// vdi-vm-8 — two target sizes within this many device pixels count as "the same"
/// pending resize target, so sub-pixel layout jitter keeps the settle timer running
/// rather than restarting it every frame.
#[cfg(feature = "live-vdi")]
const RESIZE_TARGET_TOLERANCE_PX: u16 = 8;

/// vdi-vm-8 — a debounced resize re-negotiation in flight: the panel size to re-dial
/// at and the instant its settle window elapses. Armed by
/// [`VdiState::note_resize_target`] and fired by [`VdiState::poll_resize_renegotiate`].
#[cfg(feature = "live-vdi")]
#[derive(Debug, Clone, Copy)]
struct PendingResize {
    /// When the settle window elapses and the re-dial may fire.
    at: std::time::Instant,
    /// The panel size (device px) the transport will be re-dialed at.
    target: (u16, u16),
}

/// vdi-vm-8 — whether two desktop sizes differ by more than `tol` device pixels on
/// either axis. The pure predicate behind both the resize trigger (guest vs panel
/// beyond [`RESIZE_RENEGOTIATE_THRESHOLD_PX`]) and the "already dialed / same pending
/// target" checks (within [`RESIZE_TARGET_TOLERANCE_PX`]).
#[cfg(feature = "live-vdi")]
const fn size_diverges(a: (u16, u16), b: (u16, u16), tol: u16) -> bool {
    a.0.abs_diff(b.0) > tol || a.1.abs_diff(b.1) > tol
}

/// shell-ux-1 — an affordance the failure / reconnect overlay offers. Both are real
/// re-entries the session already owns a seam for (re-dial the retained request, or
/// fall back to the Chooser), never a dead-end.
#[cfg(feature = "live-vdi")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayAction {
    /// Reconnect now — reset the attempt ladder and re-dial the SAME endpoint
    /// immediately, skipping the pending backoff.
    Retry,
    /// Abandon this desktop and return to the Chooser to pick a different one.
    PickDifferent,
}

/// shell-ux-1 — the honest status the overlay paints OVER the (possibly frozen) last
/// frame. Derived purely from the [`SessionPhase`] so it can never diverge from the
/// real session state, and so it is unit-testable without egui paint.
#[cfg(feature = "live-vdi")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionOverlay {
    /// The heading (in-progress reconnect vs terminal failure).
    title: String,
    /// The honest detail — the reconnect attempt + real drop reason, or the failure
    /// reason. Never a generic message (shell-ux-1).
    detail: String,
    /// Terminal-failure face (tints the heading DANGER) vs the reconnect face.
    failed: bool,
    /// The affordances offered — always Retry + pick-a-different, so neither face is
    /// a dead-end.
    actions: Vec<OverlayAction>,
}

/// shell-ux-1 — build the overlay for `phase`, or `None` when the session is `Live`
/// (the desktop paints normally). Pure; the panel renders the returned model and the
/// tests assert its honest content + affordances directly.
#[cfg(feature = "live-vdi")]
fn session_overlay(phase: &SessionPhase, max: u32) -> Option<SessionOverlay> {
    match phase {
        SessionPhase::Live => None,
        SessionPhase::Reconnecting { attempt, reason } => Some(SessionOverlay {
            title: "Reconnecting to the desktop\u{2026}".to_string(),
            detail: format!(
                "Attempt {attempt} of {max} on the same endpoint \u{2014} the connection dropped: {reason}"
            ),
            failed: false,
            actions: vec![OverlayAction::Retry, OverlayAction::PickDifferent],
        }),
        SessionPhase::Failed { reason } => Some(SessionOverlay {
            title: "Desktop disconnected".to_string(),
            detail: format!("Could not reconnect after {max} attempts \u{2014} {reason}"),
            failed: true,
            actions: vec![OverlayAction::Retry, OverlayAction::PickDifferent],
        }),
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
    /// Live in-shell VNC transport for a direct endpoint / XAPI console fallback.
    #[cfg(feature = "live-vdi")]
    live_vnc: Option<LiveVncHandle>,
    /// Live in-shell SPICE transport for native QEMU/KVM consoles.
    #[cfg(feature = "live-vdi")]
    live_spice: Option<LiveSpiceHandle>,
    /// Log-safe live transport status/error shown under the empty backdrop until a
    /// frame arrives.
    #[cfg(feature = "live-vdi")]
    live_status: Option<String>,
    /// Broker lifecycle currently marked active by the live transport.
    #[cfg(feature = "live-vdi")]
    active_broker_session: Option<BrokerSessionLifecycle>,
    /// VDI-VM-1 — set once brokered-console resolution honestly gated (the serving
    /// peer reported it can't broker a reachable endpoint). Pins the honest status
    /// and stops the per-frame poll from re-reading a doomed session.
    #[cfg(feature = "live-vdi")]
    broker_resolution_gated: bool,
    /// VDI-VM-1 — throttle for the per-frame brokered-endpoint spool read while a
    /// mesh connect resolves (the connecting backdrop repaints continuously).
    #[cfg(feature = "live-vdi")]
    broker_resolve_at: Option<std::time::Instant>,
    /// vdi-vm-4 / shell-ux-1 — the live session's connection phase. `Live` while the
    /// transport is up (or on the initial dial); a transport drop that is NOT a user
    /// close walks it through `Reconnecting{attempt}` to `Failed`, and a fresh frame
    /// walks it back to `Live`. Drives the auto-reconnect scheduler + the honest
    /// overlay so they can never disagree.
    #[cfg(feature = "live-vdi")]
    session_phase: SessionPhase,
    /// vdi-vm-4 — when the next bounded re-dial is due (set on a drop with the capped
    /// [`reconnect_backoff`], cleared once the re-dial fires / the session recovers /
    /// the operator closes). The per-frame [`Self::poll_reconnect`] fires at it.
    #[cfg(feature = "live-vdi")]
    reconnect_at: Option<std::time::Instant>,
    /// vdi-vm-8 — the size (device px) the current live transport was dialed at (the
    /// last `preferred_size` passed to [`Self::spawn_live_transport`]). Lets a resize
    /// check avoid re-arming for a geometry already requested but not yet repainted by
    /// the guest. `None` on the fallback / bus-driven paths that pass no size.
    #[cfg(feature = "live-vdi")]
    negotiated_size: Option<(u16, u16)>,
    /// vdi-vm-8 — a debounced resize re-negotiation in flight for a live RDP/SPICE
    /// desktop: set when the panel drifts materially from the guest's real size and
    /// cleared once it settles back or the session leaves `Live`. Fired by
    /// [`Self::poll_resize_renegotiate`]. VNC (server-authoritative) never arms it.
    #[cfg(feature = "live-vdi")]
    pending_resize: Option<PendingResize>,
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
            self.broker_resolution_gated = false;
            self.broker_resolve_at = None;
            // A fresh operator-initiated connect is a clean start, never a reconnect:
            // reset the phase to `Live` and cancel any pending re-dial before the new
            // handle is installed, so a leftover `Reconnecting`/`Failed` from a prior
            // session cannot bleed into (or auto-reconnect) this one (vdi-vm-4).
            self.session_phase = SessionPhase::Live;
            self.reconnect_at = None;
            // vdi-vm-8 — a fresh operator connect renegotiates from scratch; drop any
            // in-flight resize re-dial and the prior dialed size.
            self.pending_resize = None;
            self.negotiated_size = None;
            self.publish_broker_close_if_active();
            if let Some(live) = self.live_rdp.take() {
                live.stop();
            }
            if let Some(live) = self.live_vnc.take() {
                live.stop();
            }
            if let Some(live) = self.live_spice.take() {
                live.stop();
            }
            self.texture = None;
            self.incoming = None;
            // VDI-VM-1 — a mesh-brokered connect with no discovery-time endpoint (a
            // local/peer VM whose loopback console has no advertised port) must first
            // resolve the endpoint the serving peer's `console_broker` publishes back
            // on the session record. Hold it in an honest "resolving" state; the
            // per-frame `poll_brokered_endpoint` attaches + spawns once it lands (or
            // pins the honest gate if the console can't be brokered). Direct
            // endpoints (manual/mDNS/external, or an already-resolved mesh row) spawn
            // straight away.
            if request.target.endpoint.is_none() && request.broker_session.is_some() {
                self.live_status = Some(
                    "Resolving the brokered console endpoint over the mesh\u{2026}".to_string(),
                );
            } else {
                self.spawn_live_transport(&request);
            }
        }
        self.requested = Some(request);
    }

    /// Spawn the live decoder transport for `request` (RDP / VNC / SPICE), routing
    /// the honest gate into `live_status` on failure. Shared by the direct-endpoint
    /// path ([`Self::request_connect`]) and the brokered-endpoint resolve path
    /// ([`Self::poll_brokered_endpoint`]).
    #[cfg(feature = "live-vdi")]
    fn spawn_live_transport(&mut self, request: &ConnectRequest) {
        // vdi-vm-8 — record the size this dial negotiates at so a later resize check
        // doesn't re-arm for a geometry already requested (see `note_resize_target`).
        self.negotiated_size = request.preferred_size;
        match request.protocol {
            VdiProtocol::Rdp => match LiveRdpHandle::spawn(request) {
                Ok(handle) => {
                    self.live_status = Some("Opening live RDP transport".to_string());
                    self.live_rdp = Some(handle);
                }
                Err(reason) => {
                    self.live_status = Some(format!("Live RDP gated: {reason}"));
                }
            },
            VdiProtocol::Vnc => match LiveVncHandle::spawn(request) {
                Ok(handle) => {
                    self.live_status = Some("Opening live VNC transport".to_string());
                    self.live_vnc = Some(handle);
                }
                Err(reason) => {
                    self.live_status = Some(format!("Live VNC gated: {reason}"));
                }
            },
            VdiProtocol::Spice => match LiveSpiceHandle::spawn(request) {
                Ok(handle) => {
                    self.live_status = Some("Opening live SPICE transport".to_string());
                    self.live_spice = Some(handle);
                }
                Err(reason) => {
                    self.live_status = Some(format!("Live SPICE gated: {reason}"));
                }
            },
        }
    }

    /// VDI-VM-1 — while a mesh-brokered connect has no dialable endpoint yet,
    /// resolve it from the serving peer's `console_broker` record (a local VM's
    /// loopback console, relayed onto the overlay and published back on the session
    /// record). On a reachable endpoint, attach it + spawn the transport; on an
    /// honest broker gate, pin the reason and stop (never a doomed transport, §7);
    /// otherwise keep the honest "resolving" status. Throttled to twice a second so
    /// the breathing connecting backdrop doesn't spin the spool read.
    #[cfg(feature = "live-vdi")]
    fn poll_brokered_endpoint(&mut self) {
        if self.broker_resolution_gated {
            return;
        }
        // Only while genuinely resolving: a pending mesh-brokered request with no
        // endpoint and no live transport attached yet.
        let Some(request) = self.requested.as_ref() else {
            return;
        };
        if request.target.endpoint.is_some() {
            return;
        }
        let Some(broker) = request.broker_session.clone() else {
            return;
        };
        if self.live_rdp.is_some() || self.live_vnc.is_some() || self.live_spice.is_some() {
            return;
        }
        let now = std::time::Instant::now();
        if let Some(prev) = self.broker_resolve_at {
            if now.duration_since(prev) < Duration::from_millis(500) {
                return;
            }
        }
        self.broker_resolve_at = Some(now);

        let bodies = read_console_bodies(broker.bus_root.as_deref());
        match resolve_brokered_console(&bodies, &broker.id) {
            ConsoleResolution::Ready(endpoint) => {
                if let Some(request) = self.requested.as_mut() {
                    request.target.endpoint = Some(endpoint);
                }
                if let Some(request) = self.requested.clone() {
                    self.spawn_live_transport(&request);
                }
            }
            ConsoleResolution::Unbrokerable(reason) => {
                self.live_status = Some(format!(
                    "This desktop can't be reached over the mesh: {reason}. Nothing was attached."
                ));
                self.broker_resolution_gated = true;
            }
            ConsoleResolution::Pending => {
                self.live_status = Some(
                    "Resolving the brokered console endpoint over the mesh\u{2026}".to_string(),
                );
            }
        }
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
            if let Some(live) = self.live_vnc.take() {
                live.stop();
            }
            if let Some(live) = self.live_spice.take() {
                live.stop();
            }
            self.publish_broker_close_if_active();
            self.live_status = None;
            self.broker_resolution_gated = false;
            self.broker_resolve_at = None;
            // A user-initiated close is NOT a transport drop: reset the phase to
            // `Live` and cancel any pending re-dial so backing out never enters (or
            // resumes) auto-reconnect (vdi-vm-4, requirement 3).
            self.session_phase = SessionPhase::Live;
            self.reconnect_at = None;
            // vdi-vm-8 — backing out cancels any pending resize re-dial too.
            self.pending_resize = None;
            self.negotiated_size = None;
            self.texture = None;
            self.incoming = None;
        }
        self.requested = None;
    }

    /// vdi-vm-4 — a transport drop that is NOT a user-initiated close. Walks the
    /// session phase forward ([`next_phase_on_drop`]): a first drop opens
    /// `Reconnecting{1}` and schedules a bounded backoff re-dial; each further drop
    /// bumps the attempt; the last drop `Failed`s the session with the honest reason
    /// and stops retrying. The caller has already taken the dead handle.
    #[cfg(feature = "live-vdi")]
    fn on_transport_drop(&mut self, reason: String) {
        let next = next_phase_on_drop(&self.session_phase, reason, MAX_RECONNECT_ATTEMPTS);
        match &next {
            SessionPhase::Reconnecting { attempt, .. } => {
                self.reconnect_at = Some(std::time::Instant::now() + reconnect_backoff(*attempt));
            }
            SessionPhase::Failed { reason } => {
                self.live_status = Some(format!(
                    "Desktop disconnected \u{2014} {reason}. Could not reconnect after {MAX_RECONNECT_ATTEMPTS} attempts."
                ));
                self.reconnect_at = None;
            }
            SessionPhase::Live => {
                self.reconnect_at = None;
            }
        }
        self.session_phase = next;
    }

    /// vdi-vm-4 — a fresh frame from a (re-dialed) transport: the desktop is live
    /// again, so walk the phase back to `Live` and cancel any pending re-dial.
    #[cfg(feature = "live-vdi")]
    fn note_live_frame(&mut self) {
        if self.session_phase != SessionPhase::Live {
            self.session_phase = SessionPhase::Live;
        }
        self.reconnect_at = None;
    }

    /// vdi-vm-4 — fire a due bounded re-dial: once `reconnect_at` elapses while the
    /// session is `Reconnecting`, re-dial the SAME retained [`ConnectRequest`] (the
    /// endpoint + credentials are already on it). A re-dial that cannot even start
    /// (the gate reason lands in `live_status`) counts as another drop, so the ladder
    /// keeps advancing toward the honest `Failed` instead of stalling.
    #[cfg(feature = "live-vdi")]
    fn poll_reconnect(&mut self) {
        let Some(at) = self.reconnect_at else {
            return;
        };
        if std::time::Instant::now() < at {
            return;
        }
        self.reconnect_at = None;
        if !matches!(self.session_phase, SessionPhase::Reconnecting { .. }) {
            return;
        }
        let Some(request) = self.requested.clone() else {
            self.session_phase = SessionPhase::Failed {
                reason: "no retained desktop connection to reconnect".to_string(),
            };
            return;
        };
        self.spawn_live_transport(&request);
        if !self.has_live_transport() {
            let reason = self
                .live_status
                .clone()
                .unwrap_or_else(|| "the re-dial could not start".to_string());
            self.on_transport_drop(reason);
        }
    }

    /// vdi-vm-8 — observe this frame's real panel size (`panel_px`, device px) against
    /// the guest's real desktop size (`guest_px`, the live texture size) and arm /
    /// disarm a debounced resize re-negotiation. The RDP/SPICE thin transports fix
    /// their desktop size at dial time and expose no in-session resize, so the only
    /// way to fit a materially-resized panel is a fresh dial at the new size — armed
    /// here, fired by [`Self::poll_resize_renegotiate`] once the size settles. VNC is
    /// server-authoritative and resizes itself, so it never arms this; smaller deltas
    /// stay on the LINEAR upscale (imperceptible, no disruptive re-dial).
    #[cfg(feature = "live-vdi")]
    fn note_resize_target(&mut self, panel_px: (u16, u16), guest_px: (u16, u16)) {
        // Only an RDP/SPICE session re-negotiates by re-dialing; VNC excludes itself.
        let renegotiable = self.live_rdp.is_some() || self.live_spice.is_some();
        if !renegotiable || self.session_phase != SessionPhase::Live {
            self.pending_resize = None;
            return;
        }
        // Already dialed (or dialing) at ~this size — the guest just hasn't repainted
        // at the new geometry yet; the upscale bridges it. Don't re-arm.
        if let Some(neg) = self.negotiated_size {
            if !size_diverges(neg, panel_px, RESIZE_TARGET_TOLERANCE_PX) {
                self.pending_resize = None;
                return;
            }
        }
        // The guest's real desktop already matches the panel closely enough — the paint
        // is ~1:1, so there is nothing worth a disruptive re-dial.
        if !size_diverges(guest_px, panel_px, RESIZE_RENEGOTIATE_THRESHOLD_PX) {
            self.pending_resize = None;
            return;
        }
        // Arm (or keep) the settle timer toward the current panel size: a materially
        // different target restarts it; a target within tolerance keeps it counting.
        match self.pending_resize {
            Some(p) if !size_diverges(p.target, panel_px, RESIZE_TARGET_TOLERANCE_PX) => {}
            _ => {
                self.pending_resize = Some(PendingResize {
                    at: std::time::Instant::now() + RESIZE_SETTLE,
                    target: panel_px,
                });
            }
        }
    }

    /// vdi-vm-8 — fire a settled resize re-negotiation: once the pending target's
    /// settle window elapses while the session is still `Live`, re-dial the SAME
    /// retained request at the new panel size. This is a DELIBERATE, operator-invisible
    /// re-negotiation, NOT a vdi-vm-4 drop: the phase stays `Live`, the attempt ladder
    /// is untouched, and the last frame + texture stay painted (LINEAR-upscaled to the
    /// new panel) so the sub-second re-dial gap shows the old desktop stretched rather
    /// than the connecting backdrop. A re-dial that cannot even start degrades into the
    /// honest vdi-vm-4 drop ladder rather than silently losing the session.
    #[cfg(feature = "live-vdi")]
    fn poll_resize_renegotiate(&mut self) {
        let Some(pending) = self.pending_resize else {
            return;
        };
        if std::time::Instant::now() < pending.at {
            return;
        }
        self.pending_resize = None;
        // Guard: only re-dial a still-live RDP/SPICE session (a drop this frame may have
        // flipped us out of `Live`, or swapped in a VNC-only handle).
        if self.session_phase != SessionPhase::Live
            || !(self.live_rdp.is_some() || self.live_spice.is_some())
        {
            return;
        }
        let Some(request) = self
            .requested
            .clone()
            .map(|r| r.with_preferred_size(Some(pending.target)))
        else {
            return;
        };
        // Stop the current transport and re-dial at the new geometry; KEEP texture /
        // incoming so the last frame bridges the gap (the upscale fallback covers it).
        if let Some(live) = self.live_rdp.take() {
            live.stop();
        }
        if let Some(live) = self.live_spice.take() {
            live.stop();
        }
        self.spawn_live_transport(&request);
        // Persist the new size so a later vdi-vm-4 re-dial keeps the resized geometry.
        self.requested = Some(request);
        if !self.has_live_transport() {
            let reason = self
                .live_status
                .clone()
                .unwrap_or_else(|| "the resize re-dial could not start".to_string());
            self.on_transport_drop(reason);
        }
    }

    /// shell-ux-1 — the operator pressed **Retry / Reconnect** on the overlay: reset
    /// the attempt ladder and re-dial the SAME retained endpoint immediately (skipping
    /// any pending backoff), carrying the last honest drop reason so the overlay stays
    /// truthful until the re-dial produces a frame. Resets a terminal `Failed`.
    #[cfg(feature = "live-vdi")]
    fn retry_now(&mut self) {
        let reason = match &self.session_phase {
            SessionPhase::Failed { reason } | SessionPhase::Reconnecting { reason, .. } => {
                reason.clone()
            }
            SessionPhase::Live => String::new(),
        };
        if let Some(live) = self.live_rdp.take() {
            live.stop();
        }
        if let Some(live) = self.live_vnc.take() {
            live.stop();
        }
        if let Some(live) = self.live_spice.take() {
            live.stop();
        }
        self.reconnect_at = None;
        let Some(request) = self.requested.clone() else {
            self.session_phase = SessionPhase::Failed {
                reason: "no retained desktop connection to reconnect".to_string(),
            };
            return;
        };
        self.session_phase = SessionPhase::Reconnecting { attempt: 1, reason };
        self.spawn_live_transport(&request);
        if !self.has_live_transport() {
            let reason = self
                .live_status
                .clone()
                .unwrap_or_else(|| "the re-dial could not start".to_string());
            self.on_transport_drop(reason);
        }
    }

    /// Whether any live transport handle is currently installed.
    #[cfg(feature = "live-vdi")]
    fn has_live_transport(&self) -> bool {
        self.live_rdp.is_some() || self.live_vnc.is_some() || self.live_spice.is_some()
    }

    #[cfg(feature = "live-vdi")]
    fn poll_live_rdp(&mut self) {
        let Some(live) = self.live_rdp.as_ref() else {
            return;
        };
        let mut publish_active = false;
        let mut got_frame = false;
        let mut drop_reason = None;
        while let Ok(event) = live.event_rx.try_recv() {
            match event {
                LiveRdpEvent::Connected(target) => {
                    self.live_status = Some(format!("Live RDP connected to {target}"));
                    publish_active = true;
                }
                LiveRdpEvent::Frame(frame) => {
                    self.incoming = Some(frame);
                    got_frame = true;
                }
                LiveRdpEvent::Error(reason) => {
                    self.live_status = Some(reason.clone());
                    drop_reason = Some(reason);
                }
                LiveRdpEvent::Ended(reason) => {
                    self.live_status = Some(format!("RDP session ended: {reason}"));
                    drop_reason = Some(reason);
                }
            }
        }
        // A fresh frame means the desktop is live again (recovering a reconnect).
        if got_frame {
            self.note_live_frame();
        }
        if publish_active {
            self.publish_broker_active();
        }
        // The worker thread has died on its own — take the dead handle and drive the
        // session through the auto-reconnect phase machine (vdi-vm-4).
        if let Some(reason) = drop_reason {
            self.live_rdp = None;
            self.publish_broker_disconnect_if_active();
            self.on_transport_drop(reason);
        }
    }

    #[cfg(feature = "live-vdi")]
    fn poll_live_vnc(&mut self) {
        let Some(live) = self.live_vnc.as_ref() else {
            return;
        };
        let mut publish_active = false;
        let mut got_frame = false;
        let mut drop_reason = None;
        while let Ok(event) = live.event_rx.try_recv() {
            match event {
                LiveVncEvent::Connected(target) => {
                    self.live_status = Some(format!("Live VNC connected to {target}"));
                    publish_active = true;
                }
                LiveVncEvent::Frame(frame) => {
                    self.incoming = Some(frame);
                    got_frame = true;
                }
                LiveVncEvent::Error(reason) => {
                    self.live_status = Some(reason.clone());
                    drop_reason = Some(reason);
                }
                LiveVncEvent::Ended(reason) => {
                    self.live_status = Some(format!("VNC session ended: {reason}"));
                    drop_reason = Some(reason);
                }
            }
        }
        if got_frame {
            self.note_live_frame();
        }
        if publish_active {
            self.publish_broker_active();
        }
        if let Some(reason) = drop_reason {
            self.live_vnc = None;
            self.publish_broker_disconnect_if_active();
            self.on_transport_drop(reason);
        }
    }

    #[cfg(feature = "live-vdi")]
    fn poll_live_spice(&mut self) {
        let Some(live) = self.live_spice.as_ref() else {
            return;
        };
        let mut publish_active = false;
        let mut got_frame = false;
        let mut drop_reason = None;
        while let Ok(event) = live.event_rx.try_recv() {
            match event {
                LiveSpiceEvent::Connected(target) => {
                    self.live_status = Some(format!("Live SPICE connected to {target}"));
                    publish_active = true;
                }
                LiveSpiceEvent::Frame(frame) => {
                    self.incoming = Some(frame);
                    got_frame = true;
                }
                LiveSpiceEvent::Error(reason) => {
                    self.live_status = Some(reason.clone());
                    drop_reason = Some(reason);
                }
                LiveSpiceEvent::Ended(reason) => {
                    self.live_status = Some(format!("SPICE session ended: {reason}"));
                    drop_reason = Some(reason);
                }
            }
        }
        if got_frame {
            self.note_live_frame();
        }
        if publish_active {
            self.publish_broker_active();
        }
        if let Some(reason) = drop_reason {
            self.live_spice = None;
            self.publish_broker_disconnect_if_active();
            self.on_transport_drop(reason);
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
    {
        // VDI-VM-1 — resolve a mesh-brokered console endpoint from the session
        // record before draining transport events (a just-resolved endpoint spawns
        // its transport here, and its events are picked up next frame).
        state.poll_brokered_endpoint();
        // vdi-vm-4 — fire a due bounded re-dial BEFORE draining transport events, so a
        // just-re-dialed transport's events are picked up on the next frame.
        state.poll_reconnect();
        // vdi-vm-8 — fire a settled resize re-negotiation on the same schedule, so a
        // just-re-dialed (resized) transport's first frame is drained next frame too.
        state.poll_resize_renegotiate();
        state.poll_live_rdp();
        state.poll_live_vnc();
        state.poll_live_spice();
    }

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
            // The uploaded framebuffer IS the guest desktop at its own negotiated
            // resolution, so the texture size is exactly the guest desktop size — the
            // denominator the pointer transform needs to turn a panel click into a
            // guest pixel (vdi-vm-2). Read it before the immutable `texture` borrow
            // ends so `forward_input` can re-borrow `state` mutably.
            let desktop_px = texture.size();
            // a11y-05 — the accessible description of the desktop about to paint,
            // read off the retained request before the mutable re-borrow below.
            let desktop_label = desktop_a11y_value(state);
            // Allocate the interactive body rect first, then paint the texture over
            // it, so the desktop both fills the panel and captures pointer input.
            let size = ui.available_size();
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
            egui::Image::new(egui::load::SizedTexture::new(tex_id, rect.size())).paint_at(ui, rect);
            // Clicking the desktop focuses it so keystrokes route to the guest.
            if resp.clicked() {
                resp.request_focus();
            }
            // a11y-05 — the remote-desktop landmark (a named `Role::Group` region)
            // so a screen reader announces which desktop is focused. Pure metadata.
            install_desktop_accessibility(ui.ctx(), resp.id, desktop_label, rect);
            let desktop_size = (
                u16::try_from(desktop_px[0]).unwrap_or(u16::MAX),
                u16::try_from(desktop_px[1]).unwrap_or(u16::MAX),
            );
            forward_input(ui, state, rect, desktop_size);
            // vdi-vm-8 — refine the live desktop geometry to the panel's REAL pixel size
            // (device px). On a MATERIAL panel resize (a seat / monitor resolution
            // change) an RDP/SPICE session is re-dialed at the true panel size so the
            // desktop fits ~1:1; smaller deltas stay on the LINEAR upscale below, and
            // VNC is left to the server. `size` is the panel's egui-points extent.
            #[cfg(feature = "live-vdi")]
            {
                let panel_px =
                    target_desktop_size(size, ui.ctx().pixels_per_point(), seat_max_px(ui.ctx()));
                state.note_resize_target(panel_px, desktop_size);
            }
            // shell-ux-1 — if the session dropped, paint the honest reconnect / failure
            // overlay OVER this (now frozen) last frame, with working Retry and
            // Pick-a-different affordances wired to real session seams (vdi-vm-4).
            #[cfg(feature = "live-vdi")]
            if let Some(overlay) = session_overlay(&state.session_phase, MAX_RECONNECT_ATTEMPTS) {
                match paint_session_overlay(ui, rect, &overlay) {
                    Some(OverlayAction::Retry) => state.retry_now(),
                    Some(OverlayAction::PickDifferent) => state.clear_target(),
                    None => {}
                }
            }
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
                // never a placeholder render (§7).
                Some(req) => {
                    let title = format!(
                        "Connecting to {} via {}",
                        req.target.name,
                        req.protocol.label()
                    );
                    // CHOOSER-6 — name the auth mode honestly (SSO vs sealed cred);
                    // `auth.summary()` is log-safe and never carries the secret.
                    let auth = req.auth.summary();
                    let endpoint = req
                        .target
                        .endpoint
                        .as_ref()
                        .map_or_else(|| req.target.serving_peer.clone(), DesktopEndpoint::label);
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
                            "the live transport is not compiled into this shell build".to_string()
                        }
                    };
                    let detail = format!(
                        "Brokering the {} desktop from {} ({} \u{00B7} {} \u{00B7} {auth}) — {live_status}.",
                        req.protocol.client_crate(),
                        endpoint,
                        req.display.label(),
                        req.monitors.label(),
                    );
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

/// shell-ux-1 — paint the honest reconnect / failure overlay OVER the (frozen) last
/// desktop frame that fills `body`, and return the affordance the operator pressed
/// this frame, if any. The content is the pure [`SessionOverlay`] model (asserted by
/// the unit tests); this function only renders it and reports the button press, so
/// the panel can route it to a real seam ([`VdiState::retry_now`] /
/// [`VdiState::clear_target`]). Never a dead-end (§7).
#[cfg(feature = "live-vdi")]
fn paint_session_overlay(
    ui: &mut egui::Ui,
    body: egui::Rect,
    overlay: &SessionOverlay,
) -> Option<OverlayAction> {
    use egui::RichText;
    use mde_egui::Style;

    // Dim the frozen desktop so the honest status reads clearly over it.
    ui.painter().rect_filled(
        body,
        egui::CornerRadius::ZERO,
        egui::Color32::from_black_alpha(180),
    );

    let accent = if overlay.failed {
        Style::DANGER
    } else {
        Style::ACCENT
    };
    let mut chosen = None;
    egui::Area::new(egui::Id::new("vdi-session-overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(body.center() - egui::vec2(220.0, 80.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(Style::SURFACE)
                .stroke(egui::Stroke::new(1.0, Style::BORDER))
                .corner_radius(Style::RADIUS)
                .inner_margin(Style::SP_L)
                .show(ui, |ui| {
                    ui.set_max_width(440.0);
                    ui.label(
                        RichText::new(&overlay.title)
                            .size(Style::TITLE)
                            .strong()
                            .color(accent),
                    );
                    ui.add_space(Style::SP_XS);
                    ui.label(
                        RichText::new(&overlay.detail)
                            .size(Style::BODY)
                            .color(Style::TEXT_DIM),
                    );
                    ui.add_space(Style::SP_M);
                    ui.horizontal(|ui| {
                        for action in &overlay.actions {
                            let (label, fill) = match action {
                                OverlayAction::Retry => (
                                    if overlay.failed {
                                        "Reconnect"
                                    } else {
                                        "Retry now"
                                    },
                                    Style::ACCENT,
                                ),
                                OverlayAction::PickDifferent => {
                                    ("Pick a different desktop", Style::SURFACE_HI)
                                }
                            };
                            let button = egui::Button::new(
                                RichText::new(label).size(Style::SMALL).color(Style::TEXT),
                            )
                            .fill(fill);
                            if ui.add(button).clicked() {
                                chosen = Some(*action);
                            }
                            ui.add_space(Style::SP_S);
                        }
                    });
                });
        });
    chosen
}

/// Map an egui pointer position (egui **points**, panel/screen space) to a guest
/// **desktop pixel**, given the desktop texture's painted `rect` (also in points)
/// and the guest `desktop_size` in pixels.
///
/// The remote framebuffer is painted to *fill* `rect`, which sits below/right of
/// the dock + menubar chrome, so its top-left origin is non-zero. A pointer at
/// fraction `f` across the rect corresponds to the same fraction across the guest
/// desktop, so the transform
///
/// 1. subtracts the rect's top-left origin (`pos - rect.min`),
/// 2. divides by the rect size for the `0..1` fraction, then
/// 3. multiplies by `desktop_size` to land in guest pixels.
///
/// Because *both* the pointer and `rect` are reported in egui points, the panel's
/// `pixels_per_point` cancels in the fraction — the mapping is DPI-independent and
/// correct whether the desktop was negotiated at the panel's native size (a crisp
/// 1:1 paint) or a smaller hardcoded size egui upscales (vdi-vm-8). The result is
/// clamped to the real guest bounds `[0, w-1] × [0, h-1]` (never `u16::MAX`), so a
/// drag that slips a pixel past the panel edge still lands on a real edge pixel.
fn map_pointer_to_desktop(
    pos: egui::Pos2,
    rect: egui::Rect,
    desktop_size: (u16, u16),
) -> egui::Pos2 {
    let (w, h) = desktop_size;
    let fraction = |v: f32, min: f32, extent: f32| {
        if extent > 0.0 {
            (v - min) / extent
        } else {
            0.0
        }
    };
    let fx = fraction(pos.x, rect.min.x, rect.width());
    let fy = fraction(pos.y, rect.min.y, rect.height());
    let last_x = f32::from(w.saturating_sub(1));
    let last_y = f32::from(h.saturating_sub(1));
    egui::pos2(
        (fx * f32::from(w)).clamp(0.0, last_x),
        (fy * f32::from(h)).clamp(0.0, last_y),
    )
}

/// Rewrite a pointer event's position from panel space into guest desktop pixels
/// via [`map_pointer_to_desktop`]. Every non-pointer event (key, wheel, text,
/// focus, touch) is returned unchanged, so ONLY the coordinate bug is fixed and
/// every other input semantic (button mapping, scroll, key events) is preserved.
fn remap_pointer_event(
    event: egui::Event,
    rect: egui::Rect,
    desktop_size: (u16, u16),
) -> egui::Event {
    match event {
        egui::Event::PointerMoved(pos) => {
            egui::Event::PointerMoved(map_pointer_to_desktop(pos, rect, desktop_size))
        }
        egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers,
        } => egui::Event::PointerButton {
            pos: map_pointer_to_desktop(pos, rect, desktop_size),
            button,
            pressed,
            modifiers,
        },
        other => other,
    }
}

/// vdi-vm-8 — the pure geometry seam: the guest desktop size to negotiate from a
/// panel's real size. `available` is the panel size in egui **points**, `ppp` the
/// output's pixels-per-point, and `max` the seat-resolution ceiling in **device
/// pixels**. The panel points are scaled to device pixels (`available * ppp`),
/// rounded, and each axis clamped to `[1, max]` so the shell never asks a guest for
/// MORE pixels than the seat can display (nor for zero). At `ppp == 1` the result
/// equals the (rounded, clamped) panel — the DPI-aware 1:1 target the pointer
/// transform then maps against, so panel↔desktop scale is ~1:1 (composes with
/// vdi-vm-2). Kept pure so the clamp / round / DPI behaviour is unit-tested off-UI.
pub(crate) fn target_desktop_size(available: egui::Vec2, ppp: f32, max: (u16, u16)) -> (u16, u16) {
    let px = available * ppp;
    let (mw, mh) = max;
    (
        to_desktop_dim(px.x).min(mw.max(1)),
        to_desktop_dim(px.y).min(mh.max(1)),
    )
}

/// vdi-vm-8 — the seat resolution ceiling in **device pixels**: the full egui output
/// rect scaled by `pixels_per_point`. Used as the `max` clamp for
/// [`target_desktop_size`] so no negotiated desktop exceeds what the seat can show.
fn seat_max_px(ctx: &egui::Context) -> (u16, u16) {
    let ppp = ctx.pixels_per_point();
    let s = ctx.screen_rect().size() * ppp;
    (to_desktop_dim(s.x), to_desktop_dim(s.y))
}

/// The shell's current output size in guest **device pixels** — the vdi-vm-8 desktop
/// size hint for a live RDP/SPICE connect. At connect time the Desktop *panel* is not
/// mounted yet (the connect is dispatched from the Chooser / menu chrome), so this
/// estimates from the full egui output rect, which the desktop panel is a sub-rect of
/// (it sits under the dock + menubar). Routed through [`target_desktop_size`] with the
/// seat resolution as both estimate and ceiling. The live path
/// (`VdiState::note_resize_target`) refines to the true panel size on a material
/// resize; the worst case here is a crisp downscale the pointer transform keeps exact.
pub(crate) fn body_device_px(ctx: &egui::Context) -> (u16, u16) {
    target_desktop_size(
        ctx.screen_rect().size(),
        ctx.pixels_per_point(),
        seat_max_px(ctx),
    )
}

/// Round + clamp a device-pixel extent into `[1, u16::MAX]` — a desktop dimension
/// is always at least one pixel, and a non-finite input degrades to `1`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "value is rounded then clamped into [1, u16::MAX]; non-finite maps to 1"
)]
fn to_desktop_dim(v: f32) -> u16 {
    if v.is_finite() {
        v.round().clamp(1.0, f32::from(u16::MAX)) as u16
    } else {
        1
    }
}

/// Forward this frame's egui input to the attached guest, reserving the Esc chord.
///
/// Esc releases the desktop back to the mesh-control chrome instead of reaching
/// the guest, so the operator is never trapped in a fullscreen session. Pointer
/// positions are transformed from egui panel space into guest desktop pixels
/// (`rect` + `desktop_size`) in this ONE shared place, so all three transports
/// receive identically-mapped coordinates (vdi-vm-2). Every other event is handed
/// through unchanged; the session maps the ones it understands (pointer / button /
/// wheel / key / text) and drops the rest.
fn forward_input(ui: &egui::Ui, state: &mut VdiState, rect: egui::Rect, desktop_size: (u16, u16)) {
    let has_live = {
        #[cfg(feature = "live-vdi")]
        {
            state.live_rdp.is_some() || state.live_vnc.is_some() || state.live_spice.is_some()
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
        // Transform pointer coordinates into guest desktop pixels BEFORE handing the
        // event to any transport, so every transport applies the same mapping and
        // clicks land on the pixel under the cursor (vdi-vm-2).
        let event = remap_pointer_event(event, rect, desktop_size);
        if let Some(session) = state.session.as_mut() {
            session.send_input(&event);
        }
        #[cfg(feature = "live-vdi")]
        if let Some(live) = state.live_rdp.as_ref() {
            live.send_input(event.clone());
        }
        #[cfg(feature = "live-vdi")]
        if let Some(live) = state.live_vnc.as_ref() {
            live.send_input(event.clone());
        }
        #[cfg(feature = "live-vdi")]
        if let Some(live) = state.live_spice.as_ref() {
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

// ── accesskit (a11y-05 / shell-ux-6) ─────────────────────────────────────────
//
// The live remote desktop is one raw-painted cell: [`vdi_panel`] allocates the
// body rect (`Sense::click_and_drag`) and paints the guest framebuffer over it,
// so egui auto-generates no accesskit node — a screen reader landing on the
// Desktop surface heard nothing. The guest's OWN pixels are opaque to a host
// reader (that is the guest OS's own a11y stack), but the shell can announce
// the landmark: which remote desktop is focused, and that input routes into it.
// This installs a `Role::Group` landmark on the desktop cell — a named region
// (not a `Button`: a click focuses the desktop, it doesn't fire a discrete
// action) carrying the connected-desktop description as its value.

/// Convert an egui rect to an accesskit one (the shell-wide per-module helper).
fn accesskit_rect(rect: egui::Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// The accessible description of the live desktop cell — the connected desktop's
/// name + the chosen protocol from the retained request, so a screen reader
/// announces which remote desktop is focused. Falls back to a plain "Connected
/// desktop" when no request record is retained (a bus-driven session).
fn desktop_a11y_value(state: &VdiState) -> String {
    match state.requested.as_ref() {
        Some(req) => format!("{} via {}", req.target.name, req.protocol.label()),
        None => "Connected desktop".to_string(),
    }
}

/// Install the live desktop cell's accesskit landmark node.
fn install_desktop_accessibility(
    ctx: &egui::Context,
    id: egui::Id,
    value: impl Into<String>,
    rect: egui::Rect,
) {
    let _ = ctx.accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::Group);
        node.set_label("Remote desktop");
        node.set_value(value.into());
        node.set_bounds(accesskit_rect(rect));
    });
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
    use mde_vdi_spice::{Scancode, SpiceConfig, SpiceInputEvent};
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
    fn desktop_a11y_value_names_the_connected_desktop_and_protocol() {
        // Default (no retained request) reads the honest generic landmark value.
        let mut state = VdiState::default();
        assert_eq!(desktop_a11y_value(&state), "Connected desktop");
        // With a picked connect, the landmark names the desktop + its protocol.
        state.request_connect(ConnectRequest::new(
            RequestedTarget::new("oak", "win11"),
            VdiProtocol::Rdp,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity("oak"),
        ));
        assert_eq!(desktop_a11y_value(&state), "win11 via RDP");
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
    fn a_spice_connect_without_an_endpoint_paints_without_faking_a_session() {
        // A Spice request is constructed honestly. Without a published endpoint,
        // the live transport gates before dialing, and the surface never fakes a
        // desktop (§7).
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
            "the endpoint-gated Spice connecting caption produced no draw primitives"
        );
    }

    #[test]
    fn the_vdi_protocol_routes_map_to_the_right_client_crate() {
        assert_eq!(VdiProtocol::Rdp.client_crate(), "mde-vdi-rdp");
        assert_eq!(VdiProtocol::Vnc.client_crate(), "mde-vdi-vnc");
        assert_eq!(VdiProtocol::Spice.client_crate(), "mde-vdi-spice");
        assert!(VdiProtocol::Rdp.has_client());
        assert!(VdiProtocol::Vnc.has_client());
        assert!(VdiProtocol::Spice.has_client());
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

    // ── VDI-VM-1: resolving the brokered console endpoint from the session record ──

    #[test]
    fn console_topic_matches_the_broker() {
        // MUST equal mackesd::workers::console_broker::CONSOLE_TOPIC.
        assert_eq!(CONSOLE_TOPIC, "state/vdi/console");
    }

    fn brokered_body(session: &str, host: &str, port: u16) -> String {
        format!(
            r#"{{"session_id":"{session}","serving_node":"peer:oak","vm_id":"win11","status":{{"state":"brokered","protocol":"spice","host":"{host}","port":{port}}}}}"#
        )
    }

    fn unbrokerable_body(session: &str, reason: &str) -> String {
        format!(
            r#"{{"session_id":"{session}","serving_node":"peer:oak","vm_id":"dev","status":{{"state":"unbrokerable","reason":"{reason}"}}}}"#
        )
    }

    #[test]
    fn resolve_pending_when_no_record_for_the_session() {
        // A record for another session must not resolve ours.
        let bodies = vec![brokered_body("other", "10.42.0.7", 5900)];
        assert_eq!(
            resolve_brokered_console(&bodies, "mine"),
            ConsoleResolution::Pending
        );
        assert_eq!(
            resolve_brokered_console(&[], "mine"),
            ConsoleResolution::Pending
        );
    }

    #[test]
    fn resolve_ready_yields_the_overlay_endpoint() {
        let bodies = vec![brokered_body("s1", "10.42.0.7", 5900)];
        match resolve_brokered_console(&bodies, "s1") {
            ConsoleResolution::Ready(ep) => {
                assert_eq!(ep.host, "10.42.0.7");
                assert_eq!(ep.port, 5900);
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn resolve_unbrokerable_surfaces_the_honest_reason() {
        let bodies = vec![unbrokerable_body("s1", "VM off")];
        assert_eq!(
            resolve_brokered_console(&bodies, "s1"),
            ConsoleResolution::Unbrokerable("VM off".to_string())
        );
    }

    #[test]
    fn resolve_latest_record_wins() {
        // The broker republishes on state change: an initial gate, then a broker.
        let bodies = vec![
            unbrokerable_body("s1", "nebula overlay not up"),
            brokered_body("s1", "10.42.0.7", 5931),
        ];
        assert!(matches!(
            resolve_brokered_console(&bodies, "s1"),
            ConsoleResolution::Ready(ep) if ep.port == 5931
        ));
    }

    #[test]
    fn resolve_ignores_malformed_and_zero_port_records() {
        // A garbage body is skipped; a port-0 brokered record is honestly unusable.
        let bodies = vec!["not json".to_string(), brokered_body("s1", "10.42.0.7", 0)];
        assert!(matches!(
            resolve_brokered_console(&bodies, "s1"),
            ConsoleResolution::Unbrokerable(_)
        ));
    }

    // ────────── vdi-vm-4 / shell-ux-1: session drop → reconnect → overlay ──────────
    //
    // The auto-reconnect + honest-overlay state machine, tested through the pure
    // seams (never egui paint): the phase ladder + capped backoff, the overlay model,
    // and that a user-initiated close never enters (or resumes) Reconnecting.

    #[cfg(feature = "live-vdi")]
    #[test]
    fn drop_ladder_walks_live_through_reconnecting_to_failed_at_max() {
        // A drop from Live opens attempt 1; each further drop bumps the attempt up to
        // `max`; the next drop Fails the session with the honest last reason.
        let max = 5;
        let mut phase = SessionPhase::Live;
        for attempt in 1..=max {
            phase = next_phase_on_drop(&phase, format!("drop {attempt}"), max);
            assert_eq!(
                phase,
                SessionPhase::Reconnecting {
                    attempt,
                    reason: format!("drop {attempt}"),
                },
                "the {attempt}th drop should be Reconnecting attempt {attempt}"
            );
        }
        // The (max+1)th drop exhausts the budget → Failed with the honest reason.
        phase = next_phase_on_drop(&phase, "final drop".to_string(), max);
        assert_eq!(
            phase,
            SessionPhase::Failed {
                reason: "final drop".to_string()
            }
        );
        // Failed is terminal: a further drop stays Failed (only an explicit Retry
        // resets it — VdiState::retry_now).
        assert!(matches!(
            next_phase_on_drop(&phase, "again".to_string(), max),
            SessionPhase::Failed { .. }
        ));
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn reconnect_backoff_is_capped_exponential() {
        assert_eq!(reconnect_backoff(1), Duration::from_millis(500));
        assert_eq!(reconnect_backoff(2), Duration::from_millis(1_000));
        assert_eq!(reconnect_backoff(3), Duration::from_millis(2_000));
        assert_eq!(reconnect_backoff(4), Duration::from_millis(4_000));
        assert_eq!(reconnect_backoff(5), Duration::from_millis(8_000));
        // Held at the 8s cap beyond the ladder, so the storm stays bounded.
        assert_eq!(reconnect_backoff(9), Duration::from_millis(8_000));
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn a_transport_drop_schedules_a_redial_and_a_frame_returns_to_live() {
        // The state-side integration of the ladder: a drop opens Reconnecting{1} AND
        // schedules a bounded re-dial; a fresh frame from the re-dialed transport
        // walks the session back to Live and cancels the pending re-dial.
        let mut state = VdiState::default();
        state.on_transport_drop("server closed the connection".to_string());
        assert!(
            matches!(
                state.session_phase,
                SessionPhase::Reconnecting { attempt: 1, .. }
            ),
            "a first drop opens Reconnecting attempt 1"
        );
        assert!(
            state.reconnect_at.is_some(),
            "a drop schedules a bounded re-dial"
        );
        state.note_live_frame();
        assert_eq!(
            state.session_phase,
            SessionPhase::Live,
            "a fresh frame returns the session to Live"
        );
        assert!(
            state.reconnect_at.is_none(),
            "recovering cancels the pending re-dial"
        );
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn session_overlay_offers_a_retry_when_failed_and_nothing_when_live() {
        // Live paints the desktop normally — no overlay.
        assert!(session_overlay(&SessionPhase::Live, 5).is_none());

        // Reconnecting: honest attempt + reason, a Retry affordance, not the failed face.
        let reconnecting = session_overlay(
            &SessionPhase::Reconnecting {
                attempt: 2,
                reason: "peer reset".to_string(),
            },
            5,
        )
        .expect("a reconnect overlay");
        assert!(!reconnecting.failed);
        assert!(reconnecting.actions.contains(&OverlayAction::Retry));
        assert!(reconnecting.actions.contains(&OverlayAction::PickDifferent));
        assert!(
            reconnecting.detail.contains('2') && reconnecting.detail.contains("peer reset"),
            "the reconnect overlay names the attempt + honest reason: {}",
            reconnecting.detail
        );

        // Failed: the failed face, still with a working Retry (never a dead-end, §7).
        let failed = session_overlay(
            &SessionPhase::Failed {
                reason: "host unreachable".to_string(),
            },
            5,
        )
        .expect("a failure overlay");
        assert!(failed.failed);
        assert!(failed.actions.contains(&OverlayAction::Retry));
        assert!(failed.actions.contains(&OverlayAction::PickDifferent));
        assert!(
            failed.detail.contains("host unreachable"),
            "the failure overlay surfaces the honest reason: {}",
            failed.detail
        );
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn a_user_initiated_close_never_enters_reconnecting() {
        // Even mid-reconnect, a user close (Return to chrome / Pick-a-different →
        // clear_target) resets the phase to Live and cancels the re-dial: backing out
        // must never auto-reconnect (requirement 3). The distinction is structural —
        // the close resets the phase before any poll can drive another drop.
        let mut state = VdiState::default();
        state.on_transport_drop("dropped".to_string());
        assert!(matches!(
            state.session_phase,
            SessionPhase::Reconnecting { .. }
        ));
        assert!(state.reconnect_at.is_some());

        state.clear_target();
        assert_eq!(
            state.session_phase,
            SessionPhase::Live,
            "a user close resets the session phase to Live"
        );
        assert!(
            state.reconnect_at.is_none(),
            "a user close cancels any pending re-dial"
        );
        assert!(
            session_overlay(&state.session_phase, MAX_RECONNECT_ATTEMPTS).is_none(),
            "and shows no overlay"
        );

        // A fresh operator connect is likewise a clean start, not a reconnect: even
        // after a drop put us mid-reconnect, requesting a new desktop resets to Live.
        state.on_transport_drop("dropped again".to_string());
        assert!(matches!(
            state.session_phase,
            SessionPhase::Reconnecting { .. }
        ));
        state.request_connect(ConnectRequest::new(
            RequestedTarget::new("node-a", "web1"),
            VdiProtocol::Rdp,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity("node-a"),
        ));
        assert_eq!(
            state.session_phase,
            SessionPhase::Live,
            "a fresh connect resets the phase to Live, never Reconnecting"
        );
        assert!(state.reconnect_at.is_none());
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn live_rdp_accepts_a_mesh_identity_with_a_guest_credential() {
        let req = ConnectRequest::new(
            RequestedTarget::new("oak", "win11")
                .with_endpoint(DesktopEndpoint::new("10.42.0.9", 3389)),
            VdiProtocol::Rdp,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity_with_guest(
                "client-node",
                "desktop/oak/rdp",
                Credential::new("administrator", "mesh-rdp-pw"),
            ),
        );
        let credential = live_rdp_credential(&req).expect("guest credential accepted");
        assert_eq!(credential.username, "administrator");
        assert_eq!(credential.secret.expose(), "mesh-rdp-pw");
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn live_rdp_gates_a_bare_mesh_identity_until_guest_login_is_available() {
        let req = ConnectRequest::new(
            RequestedTarget::new("oak", "win11")
                .with_endpoint(DesktopEndpoint::new("10.42.0.9", 3389)),
            VdiProtocol::Rdp,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity("client-node"),
        );
        let err = live_rdp_credential(&req).expect_err("guest credential required");
        assert!(err.contains("sealed guest credential"));
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn live_vnc_accepts_mesh_identity_without_a_guest_credential() {
        let req = ConnectRequest::new(
            RequestedTarget::new("oak", "bios-console")
                .with_endpoint(DesktopEndpoint::new("10.42.0.9", 5900)),
            VdiProtocol::Vnc,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity("client-node"),
        );
        let cfg = live_vnc_config(&req).expect("mesh-gated VNC console needs no guest password");
        assert_eq!(cfg.host, "10.42.0.9");
        assert_eq!(cfg.port, 5900);
        assert!(
            cfg.shared,
            "console connects should not evict existing viewers"
        );
        assert_eq!(cfg.password, None);
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn live_vnc_carries_a_sealed_guest_password_when_present() {
        let req = ConnectRequest::new(
            RequestedTarget::new("oak", "secured-vnc")
                .with_endpoint(DesktopEndpoint::new("10.42.0.9", 5901)),
            VdiProtocol::Vnc,
            DisplayMode::Windowed,
            MonitorSpan::Single,
            DesktopAuth::Sealed {
                store_ref: "desktop/oak/vnc".to_string(),
                credential: Credential::new("ignored-by-rfb", "vnc-secret"),
            },
        );
        let cfg = live_vnc_config(&req).expect("sealed VNC config builds");
        assert_eq!(cfg.port, 5901);
        assert_eq!(cfg.password.as_deref(), Some("vnc-secret"));
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn live_spice_accepts_mesh_identity_without_a_guest_ticket() {
        let req = ConnectRequest::new(
            RequestedTarget::new("oak", "qemu-console")
                .with_endpoint(DesktopEndpoint::new("10.42.0.9", 5930)),
            VdiProtocol::Spice,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity("client-node"),
        );
        let cfg = live_spice_config(&req).expect("mesh-gated SPICE console needs no guest ticket");
        assert_eq!(cfg.host, "10.42.0.9");
        assert_eq!(cfg.port, 5930);
        assert_eq!(cfg.password, None);
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn live_spice_carries_a_sealed_guest_ticket_when_present() {
        let req = ConnectRequest::new(
            RequestedTarget::new("oak", "secured-spice")
                .with_endpoint(DesktopEndpoint::new("10.42.0.9", 5931)),
            VdiProtocol::Spice,
            DisplayMode::Windowed,
            MonitorSpan::Single,
            DesktopAuth::Sealed {
                store_ref: "desktop/oak/spice".to_string(),
                credential: Credential::new("", "spice-ticket"),
            },
        );
        let cfg = live_spice_config(&req).expect("sealed SPICE config builds");
        assert_eq!(cfg.port, 5931);
        assert_eq!(cfg.password.as_deref(), Some("spice-ticket"));
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
    fn a_live_spice_session_frame_flows_to_the_texture() {
        // Proves the shell is a real caller of `mde-vdi-spice`: a fresh session
        // marks its framebuffer dirty, so the panel pulls a `frame()` and uploads
        // it with no server in the loop.
        let session = SpiceSession::new(SpiceConfig::new("host")).expect("valid SPICE config");
        let mut state = VdiState {
            session: Some(Session::Spice(session)),
            ..Default::default()
        };
        run_panel(&mut state, body_input());
        assert!(
            state.texture.is_some(),
            "the SPICE session's first frame was not pulled and uploaded"
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

    #[test]
    fn input_forwards_to_a_spice_session_and_esc_returns_to_chrome() {
        // The native QEMU/SPICE fallback receives pointer + scancode input through
        // the shell's common forwarding seam, while Esc stays reserved for chrome.
        let session = SpiceSession::new(SpiceConfig::new("host")).expect("valid SPICE config");
        let mut state = VdiState {
            session: Some(Session::Spice(session)),
            ..Default::default()
        };
        let mut input = body_input();
        input.events = vec![
            egui::Event::PointerMoved(pos2(200.0, 120.0)),
            egui::Event::Key {
                key: egui::Key::M,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            },
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
        let Some(Session::Spice(session)) = &state.session else {
            panic!("SPICE session was detached");
        };
        // The pointer position is now transformed from egui panel space into guest
        // desktop pixels (vdi-vm-2), so it lands in-bounds rather than the raw
        // (200, 120) pass-through the old bug forwarded verbatim. The exact value
        // depends on egui's panel layout; the pure-transform tests below pin the
        // math down to the pixel.
        let (px, py) = session.pointer_position();
        let (dw, dh) = session.desktop_size();
        assert!(
            px < dw && py < dh && (px, py) != (0, 0),
            "the pointer event was not forwarded + transformed into guest pixels: \
             got ({px},{py}) for a {dw}x{dh} desktop"
        );
        assert!(
            session.pending_input().contains(&SpiceInputEvent::Key {
                scancode: Scancode {
                    code: 0x32,
                    extended: false,
                },
                down: true,
            }),
            "the M key scancode was not queued for the SPICE transport"
        );
        assert!(
            !session.pending_input().contains(&SpiceInputEvent::Key {
                scancode: Scancode {
                    code: 0x01,
                    extended: false,
                },
                down: true,
            }),
            "Esc leaked through to the SPICE guest"
        );
    }

    // ───────────────── vdi-vm-2: pointer coordinate transform ────────────────
    //
    // The bug forwarded raw egui panel coordinates as guest desktop pixels — no
    // rect-origin subtraction, no scale, clamped to u16::MAX. These pin down the
    // shared transform every transport now flows through.

    #[test]
    fn pointer_maps_panel_space_into_guest_desktop_pixels() {
        // A panel with a NON-ZERO origin (dock + menubar above/left) whose size is
        // different from the guest desktop — the exact shape the bug ignored.
        let rect = Rect::from_min_size(pos2(100.0, 40.0), vec2(800.0, 600.0));
        let desktop = (1600u16, 1200u16); // 2× the panel per axis

        // Top-left corner of the panel → guest origin.
        assert_eq!(
            map_pointer_to_desktop(pos2(100.0, 40.0), rect, desktop),
            pos2(0.0, 0.0)
        );
        // Panel centre → guest centre.
        assert_eq!(
            map_pointer_to_desktop(pos2(500.0, 340.0), rect, desktop),
            pos2(800.0, 600.0)
        );
        // A quarter across the panel → a quarter across the guest.
        assert_eq!(
            map_pointer_to_desktop(pos2(300.0, 190.0), rect, desktop),
            pos2(400.0, 300.0)
        );
        // Bottom-right corner → the LAST guest pixel (w-1 / h-1), not w / h.
        assert_eq!(
            map_pointer_to_desktop(pos2(900.0, 640.0), rect, desktop),
            pos2(1599.0, 1199.0)
        );
    }

    #[test]
    fn pointer_transform_clamps_outside_the_panel_to_guest_bounds() {
        let rect = Rect::from_min_size(pos2(50.0, 30.0), vec2(500.0, 400.0));
        let desktop = (250u16, 200u16);
        // Above/left of the panel clamps to the guest origin (never negative).
        assert_eq!(
            map_pointer_to_desktop(pos2(0.0, 0.0), rect, desktop),
            pos2(0.0, 0.0)
        );
        // Far below/right clamps to the last guest pixel (never u16::MAX).
        assert_eq!(
            map_pointer_to_desktop(pos2(9000.0, 9000.0), rect, desktop),
            pos2(249.0, 199.0)
        );
    }

    #[test]
    fn pointer_transform_is_identity_when_panel_matches_desktop_at_origin() {
        // Origin (0,0), panel size == desktop size → 1:1 pass-through.
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 768.0));
        let desktop = (1024u16, 768u16);
        assert_eq!(
            map_pointer_to_desktop(pos2(200.0, 120.0), rect, desktop),
            pos2(200.0, 120.0)
        );
        assert_eq!(
            map_pointer_to_desktop(pos2(0.0, 0.0), rect, desktop),
            pos2(0.0, 0.0)
        );
    }

    #[test]
    fn pointer_transform_downscales_a_large_panel_onto_a_small_desktop() {
        // Panel bigger than the guest (guest hardcoded to 1024×768, egui upscales) —
        // a click still maps to the correct guest pixel, which is what makes clicks
        // land correctly even under upscaling (vdi-vm-8's must-have).
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1920.0, 1080.0));
        let desktop = (1024u16, 768u16);
        // Panel centre → guest centre (512, 384).
        assert_eq!(
            map_pointer_to_desktop(pos2(960.0, 540.0), rect, desktop),
            pos2(512.0, 384.0)
        );
    }

    #[test]
    fn pointer_transform_survives_a_degenerate_zero_size_panel() {
        // A zero-extent rect (pre-layout / collapsed) must not divide by zero.
        let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(0.0, 0.0));
        assert_eq!(
            map_pointer_to_desktop(pos2(50.0, 50.0), rect, (640, 480)),
            pos2(0.0, 0.0)
        );
    }

    #[test]
    fn remap_rewrites_pointer_events_and_passes_others_through() {
        let rect = Rect::from_min_size(pos2(100.0, 100.0), vec2(400.0, 400.0));
        let desktop = (800u16, 800u16); // 2× the panel

        // PointerMoved is rewritten into guest pixels.
        match remap_pointer_event(egui::Event::PointerMoved(pos2(300.0, 300.0)), rect, desktop) {
            egui::Event::PointerMoved(p) => assert_eq!(p, pos2(400.0, 400.0)),
            other => panic!("expected PointerMoved, got {other:?}"),
        }

        // PointerButton keeps its button / pressed / modifiers; only the pos remaps.
        let button_ev = egui::Event::PointerButton {
            pos: pos2(100.0, 100.0),
            button: egui::PointerButton::Secondary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        };
        match remap_pointer_event(button_ev, rect, desktop) {
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                ..
            } => {
                assert_eq!(pos, pos2(0.0, 0.0));
                assert_eq!(button, egui::PointerButton::Secondary);
                assert!(pressed);
            }
            other => panic!("expected PointerButton, got {other:?}"),
        }

        // A key event passes through byte-for-byte (no coordinate touched).
        let key_ev = egui::Event::Key {
            key: egui::Key::M,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        };
        assert_eq!(remap_pointer_event(key_ev.clone(), rect, desktop), key_ev);

        // A wheel event (carries no position) passes through unchanged.
        let wheel_ev = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, 2.0),
            modifiers: egui::Modifiers::default(),
        };
        assert_eq!(
            remap_pointer_event(wheel_ev.clone(), rect, desktop),
            wheel_ev
        );
    }

    #[test]
    fn body_device_px_scales_the_screen_rect_by_pixels_per_point() {
        // vdi-vm-8 — the initial-size hint is the output size in DEVICE pixels.
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1920.0, 1080.0))),
            ..Default::default()
        };
        let _ = ctx.run(input, |_| {});
        let ppp = ctx.pixels_per_point();
        let (w, h) = body_device_px(&ctx);
        assert_eq!(w, (1920.0 * ppp).round() as u16);
        assert_eq!(h, (1080.0 * ppp).round() as u16);
        assert!(
            w >= 1 && h >= 1,
            "a device size is always at least one pixel"
        );
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn rdp_initial_resolution_clamps_to_a_legal_even_desktop() {
        // No hint → the prior hardcoded fallback.
        assert_eq!(super::rdp_initial_resolution(None), (1024, 768));
        // In-range even size passes through.
        assert_eq!(
            super::rdp_initial_resolution(Some((1920, 1080))),
            (1920, 1080)
        );
        // Odd width is forced even (RDP requires it).
        assert_eq!(
            super::rdp_initial_resolution(Some((1921, 1080))),
            (1920, 1080)
        );
        // Below the RDP minimum (200) clamps up; above the max (8192) clamps down.
        assert_eq!(super::rdp_initial_resolution(Some((100, 100))), (200, 200));
        assert_eq!(
            super::rdp_initial_resolution(Some((9000, 9000))),
            (8192, 8192)
        );
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn spice_initial_size_clamps_to_the_framebuffer_range() {
        assert_eq!(super::spice_initial_size(None), (1024, 768));
        assert_eq!(super::spice_initial_size(Some((1920, 1080))), (1920, 1080));
        // Below the SPICE minimum (16) clamps up; above the max (8192) clamps down.
        assert_eq!(super::spice_initial_size(Some((8, 8))), (16, 16));
        assert_eq!(super::spice_initial_size(Some((9000, 9000))), (8192, 8192));
    }

    #[test]
    fn target_desktop_size_is_dpi_aware_and_clamps_to_the_seat() {
        // A generous ceiling so only the DPI + round behaviour shows.
        let uncapped = (u16::MAX, u16::MAX);
        // ppp == 1 → the target equals the (rounded) panel — the 1:1 goal (vdi-vm-8).
        assert_eq!(
            target_desktop_size(vec2(1600.0, 900.0), 1.0, uncapped),
            (1600, 900)
        );
        // HiDPI: ppp == 2 doubles the device pixels the guest is asked for.
        assert_eq!(
            target_desktop_size(vec2(800.0, 600.0), 2.0, uncapped),
            (1600, 1200)
        );
        // Fractional points round to the nearest device pixel per axis.
        assert_eq!(
            target_desktop_size(vec2(1279.4, 720.6), 1.0, uncapped),
            (1279, 721)
        );
        // The seat ceiling clamps each axis DOWN (never ask for more than the seat).
        assert_eq!(
            target_desktop_size(vec2(4000.0, 4000.0), 1.0, (1920, 1080)),
            (1920, 1080)
        );
        // A collapsed panel — or a degenerate zero ceiling — still yields ≥ 1px/axis.
        assert_eq!(target_desktop_size(vec2(0.0, 0.0), 1.0, uncapped), (1, 1));
        assert_eq!(target_desktop_size(vec2(1.0, 1.0), 1.0, (0, 0)), (1, 1));
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn size_diverges_flags_a_change_beyond_tolerance_on_either_axis() {
        assert!(!size_diverges((1920, 1080), (1920, 1080), 0));
        // Within tolerance on both axes → not a divergence.
        assert!(!size_diverges((1920, 1080), (1900, 1064), 128));
        // Either axis alone past tolerance → a divergence.
        assert!(size_diverges((1920, 1080), (1024, 1080), 128));
        assert!(size_diverges((1920, 1080), (1920, 700), 128));
        // The boundary is strict: exactly `tol` is NOT divergence; `tol + 1` is.
        assert!(!size_diverges((100, 100), (108, 100), 8));
        assert!(size_diverges((100, 100), (109, 100), 8));
    }

    // ─────────────────── vdi-vm-8: resize re-negotiation (RDP/SPICE) ──────────────
    //
    // These construct live transport HANDLES directly (no worker thread) to exercise
    // the arm / disarm + fire decisions off-network. A handle's channels' far ends are
    // dropped; the resize logic never touches them, so the tests stay hermetic.

    #[cfg(feature = "live-vdi")]
    fn dummy_rdp_handle() -> LiveRdpHandle {
        let (input_tx, _in) = mpsc::channel();
        let (stop_tx, _stop) = mpsc::channel();
        let (_ev, event_rx) = mpsc::channel();
        LiveRdpHandle {
            input_tx,
            stop_tx,
            event_rx,
        }
    }

    #[cfg(feature = "live-vdi")]
    fn dummy_vnc_handle() -> LiveVncHandle {
        let (input_tx, _in) = mpsc::channel();
        let (stop_tx, _stop) = mpsc::channel();
        let (_ev, event_rx) = mpsc::channel();
        LiveVncHandle {
            input_tx,
            stop_tx,
            event_rx,
        }
    }

    #[cfg(feature = "live-vdi")]
    fn rdp_connect_request() -> ConnectRequest {
        ConnectRequest::new(
            RequestedTarget::new("node-a", "win11"),
            VdiProtocol::Rdp,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity("node-a"),
        )
    }

    #[cfg(feature = "live-vdi")]
    fn live_rdp_state() -> VdiState {
        let mut state = VdiState::default();
        state.live_rdp = Some(dummy_rdp_handle());
        state.requested = Some(rdp_connect_request());
        state // session_phase defaults to Live
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn a_material_panel_growth_arms_a_resize_redial() {
        let mut state = live_rdp_state();
        // Guest negotiated small (1024×768); the panel is now 1920×1080 → past the
        // threshold, so a re-dial toward the panel size is armed.
        state.note_resize_target((1920, 1080), (1024, 768));
        let pending = state
            .pending_resize
            .expect("a material resize should arm a re-dial");
        assert_eq!(pending.target, (1920, 1080));
    }

    #[cfg(feature = "live-vdi")]
    fn expired() -> std::time::Instant {
        std::time::Instant::now() - Duration::from_millis(1)
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn a_panel_matching_the_guest_clears_any_pending_resize() {
        let mut state = live_rdp_state();
        state.pending_resize = Some(PendingResize {
            at: expired(),
            target: (1, 1),
        });
        // Panel within the threshold of the guest's real size → nothing to re-negotiate.
        state.note_resize_target((1900, 1064), (1920, 1080));
        assert!(
            state.pending_resize.is_none(),
            "a ~matching panel must disarm the pending re-dial"
        );
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn a_size_already_dialed_is_not_re_armed_while_the_guest_catches_up() {
        let mut state = live_rdp_state();
        state.negotiated_size = Some((1920, 1080));
        // The guest hasn't repainted at the new size yet (still 1024×768) but we already
        // dialed 1920×1080 — the upscale bridges it, so don't re-arm a second re-dial.
        state.note_resize_target((1920, 1080), (1024, 768));
        assert!(
            state.pending_resize.is_none(),
            "already dialed at this size ⇒ wait for the guest, don't re-arm"
        );
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn a_vnc_only_session_never_arms_a_resize_redial() {
        let mut state = VdiState::default();
        state.live_vnc = Some(dummy_vnc_handle());
        state.requested = Some(rdp_connect_request());
        state.note_resize_target((1920, 1080), (1024, 768));
        assert!(
            state.pending_resize.is_none(),
            "VNC is server-authoritative — the shell never re-dials it for size"
        );
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn poll_resize_before_the_settle_window_is_a_noop() {
        let mut state = live_rdp_state();
        state.pending_resize = Some(PendingResize {
            at: std::time::Instant::now() + Duration::from_secs(30),
            target: (1920, 1080),
        });
        state.poll_resize_renegotiate();
        assert!(
            state.pending_resize.is_some(),
            "before the settle window elapses nothing fires"
        );
        assert!(
            state.live_rdp.is_some(),
            "and the live transport is untouched"
        );
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn a_settled_resize_redial_that_cannot_start_degrades_to_the_reconnect_ladder() {
        let mut state = live_rdp_state();
        // The retained request carries no dialable endpoint, so the re-dial gates out
        // synchronously (no worker thread) — it must fall into the honest vdi-vm-4
        // ladder rather than silently drop the session.
        state.pending_resize = Some(PendingResize {
            at: expired(),
            target: (1920, 1080),
        });
        state.poll_resize_renegotiate();
        assert!(
            state.pending_resize.is_none(),
            "the settled re-dial is consumed"
        );
        assert!(
            matches!(state.session_phase, SessionPhase::Reconnecting { .. }),
            "a re-dial that cannot start degrades into the reconnect ladder"
        );
        // The retained request now carries the resized geometry for the ladder's re-dial.
        assert_eq!(
            state.requested.as_ref().and_then(|r| r.preferred_size),
            Some((1920, 1080))
        );
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn a_settled_resize_is_a_noop_once_the_session_left_live() {
        let mut state = live_rdp_state();
        // A drop this frame flipped the phase; a stale settled resize must not fire.
        state.session_phase = SessionPhase::Failed {
            reason: "gone".to_string(),
        };
        state.pending_resize = Some(PendingResize {
            at: expired(),
            target: (1920, 1080),
        });
        state.poll_resize_renegotiate();
        assert!(
            state.pending_resize.is_none(),
            "the stale pending resize is dropped"
        );
        assert!(
            state.live_rdp.is_some(),
            "and the (dead-but-not-yet-reaped) transport is left for the drop path"
        );
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    #[ignore = "live VNC console required — set MDE_VNC_LIVE_TARGET=host:port"]
    fn live_vnc_worker_renders_real_console_and_accepts_input() {
        let Ok(target) = std::env::var("MDE_VNC_LIVE_TARGET") else {
            eprintln!("live-shell-vnc: SKIP — MDE_VNC_LIVE_TARGET not set");
            return;
        };
        let (host, port_str) = target
            .rsplit_once(':')
            .expect("MDE_VNC_LIVE_TARGET must be host:port");
        let port: u16 = port_str.parse().expect("MDE_VNC_LIVE_TARGET port parses");
        let mut state = VdiState::default();
        state.request_connect(ConnectRequest::new(
            RequestedTarget::new("KVM-XCP1", "live-xapi-console")
                .with_endpoint(DesktopEndpoint::new(host, port)),
            VdiProtocol::Vnc,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity("live-proof"),
        ));

        let first = wait_for_live_vnc_frame(&mut state, std::time::Duration::from_secs(20))
            .expect("live VNC worker produced no frame");
        assert!(
            !first.pixels.is_empty(),
            "live VNC worker produced an empty frame"
        );
        let first_hash = color_image_fnv1a64(&first);
        println!(
            "live-shell-vnc: FRAME OK {}x{} fnv1a64={first_hash:#018x}",
            first.size[0], first.size[1]
        );

        let Some(live) = state.live_vnc.as_ref() else {
            panic!("live VNC handle disappeared after first frame");
        };
        live.send_input(egui::Event::Text("m".to_string()));
        for pressed in [true, false] {
            live.send_input(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            });
        }

        let after = wait_for_live_vnc_frame(&mut state, std::time::Duration::from_secs(20))
            .expect("live VNC worker produced no post-input frame");
        let after_hash = color_image_fnv1a64(&after);
        if after_hash == first_hash {
            println!(
                "live-shell-vnc: INPUT sent; framebuffer unchanged \
                 fnv1a64={after_hash:#018x}"
            );
        } else {
            println!(
                "live-shell-vnc: INPUT ECHOED before={first_hash:#018x} \
                 after={after_hash:#018x}"
            );
        }
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    #[ignore = "live SPICE console required — set MDE_SPICE_LIVE_TARGET=host:port[,ticket]"]
    fn live_spice_worker_renders_real_console_and_accepts_input() {
        let Ok(target) = std::env::var("MDE_SPICE_LIVE_TARGET") else {
            eprintln!("live-shell-spice: SKIP — MDE_SPICE_LIVE_TARGET not set");
            return;
        };
        let (host, port, ticket) = parse_live_spice_target(&target);
        let auth = ticket.map_or_else(
            || DesktopAuth::mesh_identity("live-proof"),
            |ticket| DesktopAuth::Sealed {
                store_ref: "desktop/live-spice/spice".to_string(),
                credential: Credential::new("", ticket),
            },
        );
        let mut state = VdiState::default();
        state.request_connect(ConnectRequest::new(
            RequestedTarget::new("libvirt-qemu", "live-spice-console")
                .with_endpoint(DesktopEndpoint::new(host, port)),
            VdiProtocol::Spice,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            auth,
        ));

        let first = wait_for_live_spice_frame(&mut state, std::time::Duration::from_secs(20))
            .expect("live SPICE worker produced no frame");
        assert!(
            !first.pixels.is_empty(),
            "live SPICE worker produced an empty frame"
        );
        let first_hash = color_image_fnv1a64(&first);
        println!(
            "live-shell-spice: FRAME OK {}x{} fnv1a64={first_hash:#018x}",
            first.size[0], first.size[1]
        );

        let Some(live) = state.live_spice.as_ref() else {
            panic!("live SPICE handle disappeared after first frame");
        };
        for key in [egui::Key::M, egui::Key::Enter] {
            for pressed in [true, false] {
                live.send_input(egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::default(),
                });
            }
        }

        let after = wait_for_live_spice_frame(&mut state, std::time::Duration::from_secs(20))
            .expect("live SPICE worker produced no post-input frame");
        let after_hash = color_image_fnv1a64(&after);
        if after_hash == first_hash {
            println!(
                "live-shell-spice: INPUT sent; framebuffer unchanged \
                 fnv1a64={after_hash:#018x}"
            );
        } else {
            println!(
                "live-shell-spice: INPUT ECHOED before={first_hash:#018x} \
                 after={after_hash:#018x}"
            );
        }
    }

    #[cfg(feature = "live-vdi")]
    fn wait_for_live_vnc_frame(
        state: &mut VdiState,
        timeout: std::time::Duration,
    ) -> Option<egui::ColorImage> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            state.poll_live_vnc();
            if let Some(frame) = state.incoming.take() {
                return Some(frame);
            }
            if state
                .live_status
                .as_deref()
                .is_some_and(|s| s.contains("failed") || s.contains("ended"))
            {
                panic!(
                    "live VNC worker failed before frame: {}",
                    state.live_status.as_deref().unwrap_or("unknown")
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        None
    }

    #[cfg(feature = "live-vdi")]
    fn wait_for_live_spice_frame(
        state: &mut VdiState,
        timeout: std::time::Duration,
    ) -> Option<egui::ColorImage> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            state.poll_live_spice();
            if let Some(frame) = state.incoming.take() {
                return Some(frame);
            }
            if state
                .live_status
                .as_deref()
                .is_some_and(|s| s.contains("failed") || s.contains("ended"))
            {
                panic!(
                    "live SPICE worker failed before frame: {}",
                    state.live_status.as_deref().unwrap_or("unknown")
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        None
    }

    #[cfg(feature = "live-vdi")]
    fn parse_live_spice_target(raw: &str) -> (&str, u16, Option<&str>) {
        let (endpoint, ticket) = raw
            .split_once(',')
            .map_or((raw, None), |(endpoint, ticket)| (endpoint, Some(ticket)));
        let (host, port_str) = endpoint
            .rsplit_once(':')
            .expect("MDE_SPICE_LIVE_TARGET must be host:port[,ticket]");
        let port = port_str.parse().expect("MDE_SPICE_LIVE_TARGET port parses");
        (host, port, ticket.filter(|s| !s.is_empty()))
    }

    #[cfg(feature = "live-vdi")]
    #[test]
    fn live_spice_target_parser_accepts_optional_ticket() {
        assert_eq!(
            parse_live_spice_target("127.0.0.1:5930"),
            ("127.0.0.1", 5930, None)
        );
        assert_eq!(
            parse_live_spice_target("spice.mesh:5900,secret"),
            ("spice.mesh", 5900, Some("secret"))
        );
    }

    #[cfg(feature = "live-vdi")]
    fn color_image_fnv1a64(image: &egui::ColorImage) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for px in &image.pixels {
            for byte in px.to_array() {
                h ^= u64::from(byte);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        h
    }
}
