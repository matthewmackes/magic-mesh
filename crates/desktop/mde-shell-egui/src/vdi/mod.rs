//! The VDI **Desktop** surface — a remote VM desktop rendered egui-native.
//!
//! E12 "Construct" brokers VM desktops *into* the one shell (§5 EMBED, lock 21):
//! there is no external viewer. The remote framebuffer is decoded by
//! `mde-vdi-rdp` (RDP-primary), `mde-vdi-vnc` (VNC / XAPI-console fallback), or
//! `mde-vdi-spice` (native QEMU/KVM console) into an [`egui::ColorImage`]; this
//! panel uploads that image to a `TextureHandle` and paints it as the shell body,
//! and forwards the frame's egui input straight back to the session's input
//! mapper.
//!
//! ```text
//!   session.frame_with_damage() ─▶ (ColorImage, FrameDamage) ─▶ TextureHandle
//!                                     └▶ set_partial only the changed rects (perf-7)
//!   ui.input events ────────────────────────────────────────▶ session.send_input()
//! ```
//!
//! This unit is the **first caller** of the two decoder crates — it gives their
//! `frame()`/`send_input()` surface a home. Until a session is attached (the live
//! wire transport is the gated E12-4 layer) the panel shows an honest "no desktop"
//! EmptyState, never a placeholder render of a fake desktop (§7).

use mde_egui::egui::{self, Sense, TextureHandle, TextureOptions};

use mde_vdi_core::{sub_color_image, FrameDamage};
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
    /// The latest decoded desktop plus which rectangles changed since the last
    /// frame ([`FrameDamage`]), or `None` if nothing changed. The shell partial-
    /// uploads the damaged sub-rectangles instead of the whole framebuffer (perf-7).
    fn frame_with_damage(&mut self) -> Option<(egui::ColorImage, FrameDamage)> {
        match self {
            Session::Rdp(s) => s.frame_with_damage(),
            Session::Vnc(s) => s.frame_with_damage(),
            Session::Spice(s) => s.frame_with_damage(),
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
#[cfg(any(test, feature = "live-vdi"))]
pub(crate) const CONSOLE_TOPIC: &str = "state/vdi/console";

/// The shell's read mirror of `console_broker`'s brokered-console status — only the
/// fields the transport needs. serde ignores the rest (e.g. the record's `protocol`
/// tag: the transport uses the operator's chosen protocol, the record only supplies
/// the dialable `host:port`).
#[cfg(any(test, feature = "live-vdi"))]
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
#[cfg(any(test, feature = "live-vdi"))]
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct BrokeredConsoleRecord {
    /// The session this console serves — the globally-unique correlation key the
    /// shell matches against its minted [`BrokerSessionLifecycle::id`].
    session_id: String,
    /// The brokered endpoint, or the honest reason none could be brokered.
    status: BrokeredConsoleStatus,
}

/// The outcome of resolving a brokered console endpoint from the session record.
#[cfg(any(test, feature = "live-vdi"))]
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
#[cfg(any(test, feature = "live-vdi"))]
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

/// Log-safe taskbar thumbnail source for the currently requested Desktop
/// session. The GPU handle is an egui ref-counted texture clone, not a framebuffer
/// copy.
#[derive(Clone)]
pub(crate) struct DesktopPreviewFrame {
    /// Broker roster key when this desktop came from `action/vdi/session`.
    pub(crate) broker_session_id: Option<String>,
    /// Human target label.
    pub(crate) label: String,
    /// Short protocol badge.
    pub(crate) protocol: &'static str,
    /// Live desktop texture.
    pub(crate) texture: TextureHandle,
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
    Frame(egui::ColorImage, FrameDamage),
    /// The host's TLS certificate changed since it was pinned (vdi-vm-6) — a
    /// non-fatal MITM warning; the session stays live (the Nebula link is the
    /// trust floor). Strict mode instead surfaces as [`LiveRdpEvent::Error`].
    CertWarning(String),
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
    Frame(egui::ColorImage, FrameDamage),
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
    Frame(egui::ColorImage, FrameDamage),
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
    // vdi-vm-6: surface a trust-on-first-use certificate change (possible MITM)
    // as a non-fatal banner — the connection is already up on the Nebula link.
    if let Some(change) = conn.cert_pin_change() {
        let _ = event_tx.send(LiveRdpEvent::CertWarning(change.operator_message()));
    }
    if let Some((frame, damage)) = session.frame_with_damage() {
        let _ = event_tx.send(LiveRdpEvent::Frame(frame, damage));
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
                    if let Some((frame, damage)) = session.frame_with_damage() {
                        let _ = event_tx.send(LiveRdpEvent::Frame(frame, damage));
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
    if let Some((frame, damage)) = session.frame_with_damage() {
        let _ = event_tx.send(LiveVncEvent::Frame(frame, damage));
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
                    if let Some((frame, damage)) = session.frame_with_damage() {
                        let _ = event_tx.send(LiveVncEvent::Frame(frame, damage));
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
    if let Some((frame, damage)) = session.frame_with_damage() {
        let _ = event_tx.send(LiveSpiceEvent::Frame(frame, damage));
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
                if let Some((frame, damage)) = session.frame_with_damage() {
                    let _ = event_tx.send(LiveSpiceEvent::Frame(frame, damage));
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
    /// Which rectangles changed in `incoming` (perf-7). Written next to `incoming`
    /// by whoever produced the frame; the upload drains both together and
    /// `set_partial`s only the damaged sub-rectangles. `None` (or
    /// [`FrameDamage::Full`]) means "no reliable rect info" → a full `set`. Kept as a
    /// parallel slot so the existing `incoming` writers/tests that don't carry damage
    /// still compile and safely fall back to a full upload.
    incoming_damage: Option<FrameDamage>,
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

    /// Return a live taskbar thumbnail source for the current Desktop session.
    /// No texture means no frame has landed yet, so the dock keeps its static
    /// protocol-card fallback.
    pub(crate) fn taskbar_preview_frame(&self) -> Option<DesktopPreviewFrame> {
        let request = self.requested.as_ref()?;
        let texture = self.texture.clone()?;
        Some(DesktopPreviewFrame {
            broker_session_id: request.broker_session.as_ref().map(|b| b.id.clone()),
            label: request.target.name.clone(),
            protocol: request.protocol.label(),
            texture,
        })
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
            self.incoming_damage = None;
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
            self.incoming_damage = None;
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
                LiveRdpEvent::Frame(frame, damage) => {
                    self.incoming = Some(frame);
                    self.incoming_damage = Some(damage);
                    got_frame = true;
                }
                LiveRdpEvent::CertWarning(message) => {
                    // Non-fatal: keep the session live, just raise the banner.
                    self.live_status = Some(message);
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
                LiveVncEvent::Frame(frame, damage) => {
                    self.incoming = Some(frame);
                    self.incoming_damage = Some(damage);
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
                LiveSpiceEvent::Frame(frame, damage) => {
                    self.incoming = Some(frame);
                    self.incoming_damage = Some(damage);
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

/// Upload one decoded desktop frame into `texture` (perf-7).
///
/// * **No texture yet** (the first frame) → allocate it from the whole image.
/// * **Concrete per-rectangle damage AND an unchanged texture size** →
///   [`TextureHandle::set_partial`] each damaged sub-rectangle, moving only the
///   changed pixels to the GPU. The size guard is essential: `set_partial` cannot
///   resize a texture, so a dimension change must go through the reallocating full
///   `set`.
/// * **Anything else** ([`FrameDamage::Full`], no damage, a size change, or an
///   empty rect list) → a full [`TextureHandle::set`] of the whole image.
///
/// Correctness over optimisation: a full `set` is always valid, so every uncertain
/// path degrades to it and no upload a full `set` would have done is ever skipped.
/// The `(offset, sub_image)` pairs handed to `set_partial` come from the same
/// [`sub_color_image`] slice the unit tests prove pixel-identical to a full upload.
fn upload_frame(
    ctx: &egui::Context,
    texture: &mut Option<TextureHandle>,
    img: egui::ColorImage,
    damage: Option<FrameDamage>,
) {
    match texture.as_mut() {
        // First frame / freshly-(re)allocated texture: allocate from the whole image.
        None => {
            *texture = Some(ctx.load_texture("vdi-desktop", img, DESKTOP_TEX));
        }
        Some(handle) => {
            // Partial-upload only with concrete rectangles AND a matching texture
            // size — a resize (or any size mismatch) must reallocate through the
            // full `set` below, because `set_partial` cannot resize a texture.
            let rects = match &damage {
                Some(FrameDamage::Rects(rects))
                    if !rects.is_empty() && handle.size() == img.size =>
                {
                    rects
                }
                _ => {
                    handle.set(img, DESKTOP_TEX);
                    return;
                }
            };
            for rect in rects {
                // Each rect is clamped to the frame bounds; a fully-clipped one
                // yields None and is skipped (a full `set` would not draw it either).
                if let Some((offset, sub)) = sub_color_image(&img, *rect) {
                    handle.set_partial(offset, sub, DESKTOP_TEX);
                }
            }
        }
    }
}

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

    // 1. Pull the newest decoded frame — plus which rectangles changed (perf-7) —
    //    off the live session into the upload slot.
    if let Some(session) = state.session.as_mut() {
        if let Some((img, damage)) = session.frame_with_damage() {
            state.incoming = Some(img);
            state.incoming_damage = Some(damage);
        }
    }

    // 2. Upload a pending frame. The texture is allocated on the first frame; after
    //    that, a frame carrying per-rectangle damage moves only its changed
    //    sub-rectangles to the GPU with `set_partial`, and everything else
    //    (first frame, a resize, a whole-surface / batch replace, or no reliable
    //    damage info) falls back to a full `set` — never a skipped upload.
    if let Some(img) = state.incoming.take() {
        let damage = state.incoming_damage.take();
        upload_frame(ui.ctx(), &mut state.texture, img, damage);
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

/// The reconnect / failure overlay's depth — the surface-side conversion of the
/// shared [`Elevation::Modal`](mde_egui::style::Elevation::Modal) depth token into
/// an [`egui::Shadow`] (the token module stays free of egui's shadow type). Reads the
/// token's offset/blur/spread/umbra, casting the logical-px floats onto epaint's small
/// integer fields; mints **no** colour of its own (the umbra comes straight from the
/// token), so the honest status sheet reads as a genuine modal lifted off the dimmed
/// desktop while the look still comes only from `mde_egui` (§4, lock #2).
#[cfg(feature = "live-vdi")]
fn overlay_shadow() -> egui::Shadow {
    let token = mde_egui::style::Elevation::Modal.shadow();
    egui::Shadow {
        offset: [token.offset[0] as i8, token.offset[1] as i8],
        blur: token.blur as u8,
        spread: token.spread as u8,
        color: token.umbra,
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
    ui.painter()
        .rect_filled(body, egui::CornerRadius::ZERO, Style::SCRIM);

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
                .shadow(overlay_shadow())
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

mod pointer;
pub(crate) use pointer::body_device_px;
use pointer::*;

#[cfg(test)]
mod tests;
