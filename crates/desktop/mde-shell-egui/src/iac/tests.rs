use super::*;
use mackes_mesh_types::cloud::{EndpointInterface, ResourceRow};
use mde_egui::egui::{pos2, vec2, Rect};

/// One backend-tool health row in a fixture mirror.
fn health(tool: &str, state: HealthState) -> ServiceHealth {
    ServiceHealth {
        service_type: tool.to_string(),
        interface: EndpointInterface::Internal,
        url: "(local)".to_string(),
        state,
        latency_ms: Some(3),
        microversion: None,
        version_id: None,
        detail: Some("probe".to_string()),
    }
}

/// A one-instance roster table (the shape the worker publishes on the mirror).
fn roster_table() -> ResourceTable {
    ResourceTable {
        service_type: "compute".to_string(),
        collection: "instances".to_string(),
        columns: vec!["name".to_string(), "status".to_string()],
        rows: vec![ResourceRow {
            id: "vm-1".to_string(),
            cells: vec!["mesh-worker".to_string(), "running".to_string()],
        }],
    }
}

/// A fixture `state/cloud` mirror: OpenTofu **up**, Ansible **down**, libvirt
/// **absent** (the honest Up/Down/Absent tri-state), plus a one-instance roster,
/// plan-only (apply not armed).
fn fixture_state() -> CloudState {
    CloudState {
        host: "eagle".to_string(),
        adapter: CloudProviderAdapter::ConstructCloud,
        health: vec![
            health("opentofu", HealthState::Up),
            health("ansible", HealthState::Down),
            health("libvirt", HealthState::Absent),
        ],
        resources: vec![roster_table()],
        apply_armed: false,
        published_at_ms: 42,
    }
}

/// A surface state on `mode` with the fixture mirror folded in.
fn state_on(mode: Mode) -> InfraCodeState {
    let mut state = InfraCodeState {
        mode,
        ..InfraCodeState::default()
    };
    state.states = vec![fixture_state()];
    state
}

/// Drive one headless frame of `infra_code_panel` and tessellate it on the CPU
/// (the DRM runner's path minus the GPU). Returns whether it drew primitives.
fn run_panel(state: &mut InfraCodeState) -> bool {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 720.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| infra_code_panel(ui, state));
    });
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    !prims.is_empty()
}

#[test]
fn the_surface_is_reachable_in_the_dock() {
    // §7 reachability: the surface stays in Surface::ALL and wears the server /
    // infrastructure brand glyph (the dock mount is unchanged by the recreate).
    use crate::dock::Surface;
    assert!(Surface::ALL.contains(&Surface::InfraCode));
    assert_eq!(
        Surface::InfraCode.icon_id(),
        mde_theme::brand::icons::IconId::Server
    );
}

#[test]
fn the_workspace_renders_all_six_modes_headless() {
    // Every mode tessellates over the fixture mirror — the six-mode workspace.
    for mode in Mode::ALL {
        let mut state = state_on(mode);
        assert!(
            run_panel(&mut state),
            "{:?} mode drew nothing",
            mode.label()
        );
    }
}

#[test]
fn switching_modes_works() {
    let mut state = state_on(Mode::Provision);
    assert_eq!(state.mode(), Mode::Provision);
    for mode in Mode::ALL {
        state.set_mode(mode);
        assert_eq!(state.mode(), mode);
        assert!(run_panel(&mut state), "{:?} render failed", mode.label());
    }
}

#[test]
fn the_empty_mirror_reads_honestly_never_fabricated() {
    // No mirror published yet → honest empty states per mode, never fake rows.
    for mode in [Mode::Provision, Mode::Status, Mode::Network] {
        let mut state = InfraCodeState {
            mode,
            ..InfraCodeState::default()
        };
        assert!(
            run_panel(&mut state),
            "{:?} empty state drew nothing",
            mode.label()
        );
    }
}

#[test]
fn provision_renders_a_fixture_cloudstate_resource_table() {
    let mut state = state_on(Mode::Provision);
    // The roster comes straight from the mirror's resource table.
    assert_eq!(state.states()[0].resources[0].rows.len(), 1);
    assert!(is_instance_roster(&state.states()[0].resources[0]));
    assert!(run_panel(&mut state), "the Provision roster drew nothing");
}

#[test]
fn provision_apply_is_typed_confirm_gated_and_emits_provision_only_after_confirm() {
    // Dry-run default: a plan is a direct emit (no confirm). Apply is gated.
    let mut state = state_on(Mode::Provision);

    // Arming a live apply OPENS the confirm and publishes NOTHING (§ RUN-006).
    state.arm_provision();
    let arming = state.arming.as_ref().expect("apply opens the confirm");
    assert_eq!(arming.action, ArmAction::Provision);
    assert_eq!(arming.action.verb(), "provision");
    assert!(arming.typed.is_empty());
    assert!(
        state.mutation_pending.is_none() && state.note.is_none(),
        "an unconfirmed apply publishes nothing"
    );

    // The gate: only the exact echo arms; a partial/empty echo does not.
    assert!(armed("apply", &ArmAction::Provision.echo()));
    assert!(
        armed("  apply ", &ArmAction::Provision.echo()),
        "space tolerated"
    );
    assert!(
        !armed("appl", &ArmAction::Provision.echo()),
        "partial does not arm"
    );
    assert!(
        !armed("", &ArmAction::Provision.echo()),
        "empty does not arm"
    );

    // Past the gate, perform reaches the publish seam (no Bus in the test → an
    // honest error note naming the provision verb; the request was attempted).
    state.perform(ArmAction::Provision);
    assert!(
        state
            .note
            .as_deref()
            .is_some_and(|n| n.contains("provision")),
        "the confirmed apply emits the provision verb: {:?}",
        state.note
    );
}

#[test]
fn destroy_and_lifecycle_reboot_delete_are_typed_confirm_gated() {
    let mut state = state_on(Mode::Provision);
    // Destroy opens the confirm on the DESTROY echo, publishes nothing yet.
    state.arm_destroy();
    let arming = state.arming.take().expect("destroy opens the confirm");
    assert_eq!(arming.action, ArmAction::Destroy);
    assert_eq!(arming.action.verb(), "destroy");
    assert!(armed("destroy", &arming.action.echo()));
    assert!(state.mutation_pending.is_none());

    // A destructive lifecycle op arms on the instance name.
    state.arm_lifecycle("instance-delete", "vm-1", "mesh-worker");
    let arming = state.arming.as_ref().expect("delete opens the confirm");
    assert_eq!(arming.action.verb(), "instance-delete");
    assert_eq!(arming.action.echo(), "mesh-worker");
    assert!(state.mutation_pending.is_none() && state.note.is_none());
    // The armed confirm panel still tessellates.
    assert!(run_panel(&mut state), "the arming confirm drew nothing");
}

#[test]
fn status_renders_per_tool_health_up_down_absent_honestly() {
    let st = fixture_state();
    // The mirror carries the honest tri-state — never a fabricated up.
    assert_eq!(
        st.tool_health("opentofu").map(|h| h.state),
        Some(HealthState::Up)
    );
    assert_eq!(
        st.tool_health("ansible").map(|h| h.state),
        Some(HealthState::Down)
    );
    assert_eq!(
        st.tool_health("libvirt").map(|h| h.state),
        Some(HealthState::Absent)
    );
    // An Absent/Down tool drops backend readiness (never faked ready).
    assert!(!st.backend_ready());

    // The state → glyph mapping is honest per state (all registered glyphs).
    assert_eq!(health_icon(HealthState::Up), "emblem-ok");
    assert_eq!(health_icon(HealthState::Down), "dialog-warning");
    assert_eq!(health_icon(HealthState::Absent), "changes-prevent");

    let mut state = state_on(Mode::Status);
    assert!(run_panel(&mut state), "the Status health rows drew nothing");
}

#[test]
fn images_network_and_containers_show_an_honest_backend_pending_note() {
    // Modes with no landed action/cloud verb render a backend-pending card and
    // never emit a fabricated verb (§7). Rendering them leaves nothing pending.
    for mode in [Mode::Images, Mode::Network, Mode::Containers] {
        let mut state = state_on(mode);
        assert!(run_panel(&mut state), "{:?} drew nothing", mode.label());
        assert!(
            state.mutation_pending.is_none() && state.note.is_none(),
            "{:?} must not emit a verb",
            mode.label()
        );
    }
}

#[test]
fn carbon_icons_are_registered_for_every_mode_and_health_state() {
    // "Carbon icons paint (mesh present), no glyph text" — every mode tab + every
    // health/status glyph resolves in the embedded Mackes-Carbon registry.
    for mode in Mode::ALL {
        assert!(
            mde_egui::carbon_svg_bytes(mode.icon()).is_some(),
            "{:?} icon `{}` is not a registered Carbon glyph",
            mode.label(),
            mode.icon()
        );
    }
    for state in [HealthState::Up, HealthState::Down, HealthState::Absent] {
        assert!(
            mde_egui::carbon_svg_bytes(health_icon(state)).is_some(),
            "health glyph for {state:?} is not registered"
        );
    }
    for glyph in [
        "view-refresh",
        "list-add",
        "process-stop",
        "document-edit",
        "dialog-warning",
        "changes-prevent",
    ] {
        assert!(
            mde_egui::carbon_svg_bytes(glyph).is_some(),
            "toolbar glyph `{glyph}` is not registered"
        );
    }
}

#[test]
fn fold_mutation_maps_the_reply_tri_state_honestly() {
    // An `ok` reply reads applied.
    let ok: CloudReply = serde_json::from_str(r#"{"ok":true,"verb":"provision","audited":false}"#)
        .expect("ok reply parses");
    let (note, entry) = fold_mutation(&ok);
    assert!(note.contains("applied"), "{note}");
    assert_eq!(entry.outcome, AuditOutcome::Applied);

    // A `gated` mutation reply reads STAGED (a dry-run — nothing applied) and
    // carries the staged plan summary honestly.
    let gated: CloudReply = serde_json::from_str(
        r#"{"ok":false,"verb":"provision","gated":"live apply is operator-gated — tofu plan (staged): 2 to add — nothing applied"}"#,
    )
    .expect("gated reply parses");
    let (note, entry) = fold_mutation(&gated);
    assert!(
        note.contains("staged") && note.contains("dry-run"),
        "{note}"
    );
    assert_eq!(entry.outcome, AuditOutcome::Staged);
    assert!(entry.detail.contains("to add"), "the plan summary is kept");

    // An `error` reply reads failed.
    let failed: CloudReply =
        serde_json::from_str(r#"{"ok":false,"verb":"destroy","error":"tofu destroy failed"}"#)
            .expect("error reply parses");
    let (note, entry) = fold_mutation(&failed);
    assert!(note.contains("failed"), "{note}");
    assert_eq!(entry.outcome, AuditOutcome::Failed);
}

#[test]
fn armed_hosts_reads_the_apply_posture_from_the_mirror() {
    // Plan-only by default → no armed hosts.
    let plan_only = fixture_state();
    assert!(armed_hosts(std::slice::from_ref(&plan_only)).is_empty());
    // A node with apply armed is reported honestly.
    let mut live = fixture_state();
    live.apply_armed = true;
    assert_eq!(
        armed_hosts(std::slice::from_ref(&live)),
        vec!["eagle".to_string()]
    );
}

#[test]
fn recreated_labels_carry_no_legacy_backend_terminology() {
    // The recreated workspace is provider-neutral: zero OpenStack-family terms in
    // its user-facing copy (grep-clean, §6).
    let mut labels: Vec<String> =
        vec![CLOUD_PRODUCT_LABEL.to_string(), WORKSPACE_TITLE.to_string()];
    labels.extend(Mode::ALL.iter().map(|m| m.label().to_string()));
    for tool in BACKEND_TOOLS {
        labels.push(tool.1.to_string());
    }
    for label in labels {
        for banned in [
            "OpenStack",
            "Nova",
            "Heat",
            "Keystone",
            "Glance",
            "Cinder",
            "Neutron",
            "Horizon",
        ] {
            assert!(
                !label.contains(banned),
                "user-facing label `{label}` leaked the legacy backend term `{banned}`"
            );
        }
    }
}
