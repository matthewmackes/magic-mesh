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
        RequestedTarget::new("oak", "web1").with_endpoint(DesktopEndpoint::new("10.42.0.9", 5900)),
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

/// The reconnect / failure sheet renders through the shared `dialog()` primitive,
/// so its depth is the `Elevation::Modal` soft shadow by construction (Phase-C
/// depth adoption): the sheet's shadow is exactly the shared `Modal` token — no
/// surface-side re-derivation — and its umbra stays translucent (design lock #2),
/// so the sheet reads as a genuine modal lifted off the dimmed desktop, never an
/// opaque fill and never a minted `Color32` (§4).
#[cfg(feature = "live-vdi")]
#[test]
fn session_sheet_uses_the_shared_modal_dialog_depth() {
    let sheet = mde_egui::dialog();
    let modal = mde_egui::style::Elevation::Modal.egui_shadow();
    assert_eq!(
        sheet.shadow, modal,
        "the honest status sheet lifts on the shared dialog()'s Modal depth"
    );
    assert!(
        sheet.shadow.color.a() > 0 && sheet.shadow.color.a() < 255,
        "the depth is a translucent umbra (lock #2), never an opaque fill"
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
        RequestedTarget::new("oak", "win11").with_endpoint(DesktopEndpoint::new("10.42.0.9", 3389)),
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
        RequestedTarget::new("oak", "win11").with_endpoint(DesktopEndpoint::new("10.42.0.9", 3389)),
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
fn upload_frame_allocates_then_partial_updates_then_resizes() {
    // The perf-7 upload seam: the first frame allocates the texture, a
    // same-size damaged frame partial-uploads (size preserved), and a Full /
    // resized frame falls back to a full `set` that reallocates. This proves
    // the wiring + the size-guard fallback; the pixel-equivalence of the slice
    // itself is proven in `mde_vdi_core::damage`.
    use egui::Color32;
    use mde_vdi_core::DamageRect;

    let solid = |w: usize, h: usize, c: Color32| egui::ColorImage {
        size: [w, h],
        pixels: vec![c; w * h],
    };

    let ctx = egui::Context::default();
    let mut tex: Option<TextureHandle> = None;

    // First frame: no texture yet → allocate from the whole image.
    upload_frame(&ctx, &mut tex, solid(4, 3, Color32::BLACK), None);
    assert_eq!(tex.as_ref().expect("allocated").size(), [4, 3]);

    // Same-size frame with rect damage → the partial path runs and the texture
    // keeps its size (set_partial cannot and must not resize).
    upload_frame(
        &ctx,
        &mut tex,
        solid(4, 3, Color32::WHITE),
        Some(FrameDamage::Rects(vec![DamageRect::new(1, 1, 2, 1)])),
    );
    assert_eq!(tex.as_ref().unwrap().size(), [4, 3], "partial keeps size");

    // A resized Full frame → the size guard forces a full `set`, which resizes.
    upload_frame(
        &ctx,
        &mut tex,
        solid(6, 5, Color32::BLACK),
        Some(FrameDamage::Full),
    );
    assert_eq!(tex.as_ref().unwrap().size(), [6, 5], "full set resizes");

    // A rect-damaged frame whose size disagrees with the texture also degrades
    // to a full `set` (never a partial upload into the wrong dimensions).
    upload_frame(
        &ctx,
        &mut tex,
        solid(8, 8, Color32::WHITE),
        Some(FrameDamage::Rects(vec![DamageRect::new(0, 0, 2, 2)])),
    );
    assert_eq!(
        tex.as_ref().unwrap().size(),
        [8, 8],
        "size mismatch forces a full set"
    );
}

#[test]
fn a_live_rdp_session_frame_flows_to_the_texture() {
    // Proves the shell is a real caller of `mde-vdi-rdp`: a fresh session marks
    // its framebuffer dirty, so the panel pulls a `frame()` and uploads it with
    // no server in the loop.
    let session = RdpSession::new(RdpConfig::new("host", "user", "pw").with_resolution(640, 480))
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
fn taskbar_session_preview_frame_requires_a_real_texture_and_carries_the_broker_id() {
    let mut state = VdiState::default();
    assert!(
        state.taskbar_preview_frame().is_none(),
        "no requested desktop means no taskbar thumbnail"
    );

    state.request_connect(
        ConnectRequest::new(
            RequestedTarget::new("node-a", "web1"),
            VdiProtocol::Rdp,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity("node-a"),
        )
        .with_broker_session(BrokerSessionLifecycle::new("session-1", None)),
    );
    assert!(
        state.taskbar_preview_frame().is_none(),
        "a requested desktop still has no taskbar thumbnail until a frame lands"
    );

    let ctx = egui::Context::default();
    state.texture = Some(ctx.load_texture(
        "vdi-taskbar-preview-test",
        egui::ColorImage {
            size: [4, 3],
            pixels: vec![egui::Color32::WHITE; 12],
        },
        egui::TextureOptions::LINEAR,
    ));
    let preview = state
        .taskbar_preview_frame()
        .expect("a requested desktop with a texture publishes a taskbar thumbnail");
    assert_eq!(preview.broker_session_id.as_deref(), Some("session-1"));
    assert_eq!(preview.label, "web1");
    assert_eq!(preview.protocol, "RDP");
    assert_eq!(preview.texture.size(), [4, 3]);
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

// WL-PERF-002 — the predicate the shell host loop gates its per-frame VDI
// repaint on. It must report a LIVE transport (frames actually inbound), never
// merely a REQUESTED session, so an idle chooser / a session with no live
// stream never wakes the seat at 60Hz (the whole point of the occlusion work).
#[cfg(feature = "live-vdi")]
#[test]
fn has_live_transport_is_true_only_while_a_transport_handle_is_installed() {
    // A live RDP transport handle is installed → streaming → wake the loop.
    let rdp = live_rdp_state();
    assert!(
        rdp.has_live_transport(),
        "an installed RDP transport handle is a live stream that must drive repaints"
    );

    // A VNC-only session is equally live.
    let mut vnc = VdiState::default();
    vnc.live_vnc = Some(dummy_vnc_handle());
    vnc.requested = Some(rdp_connect_request());
    assert!(
        vnc.has_live_transport(),
        "an installed VNC transport handle is a live stream too"
    );
}

#[cfg(feature = "live-vdi")]
#[test]
fn has_live_transport_stays_false_for_an_idle_or_requested_only_session() {
    // A fresh idle state (no session at all) must leave the loop idle.
    let idle = VdiState::default();
    assert!(
        !idle.has_live_transport(),
        "a default idle VdiState has no live transport — the seat must stay idle"
    );

    // A session that has been REQUESTED but has no live transport handle yet
    // (or has lost it) must NOT wake the loop — this is the regression the
    // repaint gate exists to avoid: `requested_target().is_some()` is true here
    // but there is nothing streaming.
    let mut requested_only = VdiState::default();
    requested_only.requested = Some(rdp_connect_request());
    assert!(
        requested_only.requested_target().is_some(),
        "the session IS requested (the naive gate would have repainted)…"
    );
    assert!(
        !requested_only.has_live_transport(),
        "…but with no live transport handle the loop must stay idle"
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
