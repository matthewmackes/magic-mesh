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
use mde_egui::Motion;

use mde_vdi_rdp::RdpSession;
use mde_vdi_vnc::VncSession;

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

/// A desktop target the discovery picker (E12-5b) handed to the surface: the VM
/// the operator chose. Recorded so the surface reflects the pending connect *by
/// name* until the gated E12-4 wire transport attaches the live decoder `session`
/// — an honest "connecting" caption, never a fake desktop (§7).
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
    /// The desktop target the discovery picker chose, held until the gated live
    /// transport attaches a `session`. Drives the honest "connecting" caption and
    /// tells the shell to show the Desktop surface rather than the picker.
    requested: Option<RequestedTarget>,
}

impl VdiState {
    /// Take (and clear) the "return to chrome" request raised by the Esc chord.
    /// The shell calls this after mounting the panel to leave the surface.
    pub(crate) fn take_return_to_chrome(&mut self) -> bool {
        std::mem::take(&mut self.return_to_chrome)
    }

    /// Record the target the discovery picker chose (E12-5b). The surface then
    /// shows a "connecting" state naming it until the gated wire transport attaches
    /// the live decoder session.
    pub(crate) fn request_target(&mut self, target: RequestedTarget) {
        self.requested = Some(target);
    }

    /// The picked target, if any — the shell reads it to decide whether the Desktop
    /// surface shows the discovery picker (none) or the connecting/desktop state.
    pub(crate) fn requested_target(&self) -> Option<&RequestedTarget> {
        self.requested.as_ref()
    }

    /// Clear the pending target — the operator backed out before a live session
    /// attached, so the Desktop surface falls back to the discovery picker.
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
            // Connect-state feedback: the "connecting" caption eases in on the
            // shared BASE curve when the picker hands a target over (§4 — motion
            // via the shared table only, no bespoke engine).
            let t = Motion::animate(
                ui.ctx(),
                "vdi-connecting",
                state.requested.is_some(),
                Motion::BASE,
            );
            match state.requested.as_ref() {
                // The discovery picker chose a target but no live decoder is
                // attached yet (the wire transport is gated) — an honest
                // "connecting" caption naming the VM, never a placeholder
                // desktop (§7).
                Some(target) => {
                    ui.scope(|ui| {
                        ui.set_opacity(t);
                        crate::session::empty_state(
                            ui,
                            &format!("Connecting to {}", target.name),
                            &format!(
                                "Brokering the desktop from {} — the live transport (E12-4) is gated.",
                                target.serving_peer
                            ),
                        );
                    });
                }
                None => crate::session::empty_state(
                    ui,
                    "No desktop connected",
                    "Broker a VM desktop (RDP / VNC) — it renders here in the shell.",
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
            "the no-desktop EmptyState produced no draw primitives"
        );
    }

    #[test]
    fn a_requested_target_paints_the_connecting_caption() {
        // The discovery picker handed a target but no live decoder is attached yet
        // (the wire transport is gated): the surface shows the connecting caption,
        // still with no texture and no fake desktop.
        let mut state = VdiState::default();
        state.request_target(RequestedTarget::new("node-a", "web1"));
        assert_eq!(
            state.requested_target().map(|t| t.name.as_str()),
            Some("web1")
        );
        let drew = run_panel(&mut state, body_input());
        assert!(state.texture.is_none(), "no frame attached, so no texture");
        assert!(drew, "the connecting caption produced no draw primitives");

        // Backing out clears the target so the surface returns to the picker.
        state.clear_target();
        assert!(state.requested_target().is_none());
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
