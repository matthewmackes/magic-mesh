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

/// A desktop target the Chooser (CHOOSER-2, née the E12-5b picker) handed to the
/// surface: the desktop the operator chose. Recorded so the surface reflects the
/// pending connect *by name* until the gated E12-4 wire transport attaches the
/// live decoder `session` — an honest "connecting" caption, never a fake desktop
/// (§7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestedTarget {
    /// The peer serving the VM (a scheduler node id).
    pub serving_peer: String,
    /// The VM's display name — the surface caption.
    pub name: String,
}

impl RequestedTarget {
    /// A target from the peer serving the VM and the VM's name.
    pub(crate) fn new(serving_peer: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            serving_peer: serving_peer.into(),
            name: name.into(),
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
}

impl VdiState {
    /// Take (and clear) the "return to chrome" request raised by the Esc chord.
    /// The shell calls this after mounting the panel to leave the surface.
    pub(crate) fn take_return_to_chrome(&mut self) -> bool {
        std::mem::take(&mut self.return_to_chrome)
    }

    /// Record the connect the Chooser's picker chose (CHOOSER-4). The surface then
    /// shows a "connecting" state naming the target + chosen protocol until the
    /// gated wire transport attaches the live decoder session.
    pub(crate) fn request_connect(&mut self, request: ConnectRequest) {
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
        self.requested = None;
    }
}

/// A remote desktop is scaled to fill the shell body, so sample it linearly —
/// crisper than nearest when the negotiated desktop size doesn't match the panel.
const DESKTOP_TEX: TextureOptions = TextureOptions::LINEAR;

/// Render the Desktop surface into `ui`: upload any new framebuffer, paint it to
/// fill the body, and forward this frame's egui input to the guest. With no
/// session attached it draws the honest "no desktop" EmptyState instead.
pub(crate) fn vdi_panel(ui: &mut egui::Ui, state: &mut VdiState) {
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
                        format!(
                            "Brokering the {} desktop from {} ({} \u{00B7} {} \u{00B7} {auth}) — the live transport (E12-4) is gated.",
                            req.protocol.client_crate(),
                            req.target.serving_peer,
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
    let Some(session) = state.session.as_mut() else {
        return;
    };
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
        session.send_input(&event);
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
            RequestedTarget::new("oak", "web1"),
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
