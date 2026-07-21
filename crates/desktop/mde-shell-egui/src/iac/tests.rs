use super::*;
use mackes_mesh_types::cloud::{
    CloudProviderAdapter, DriftFlag, DriftSummary, EndpointInterface, HealthState, NodeCapacity,
    ServiceHealth,
};
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

/// One workload row (the shape the worker folds onto the mirror from virsh + the
/// desired doc).
fn workload(name: &str, delivery_type: DeliveryType, status: &str) -> WorkloadRow {
    WorkloadRow {
        name: name.to_string(),
        delivery_type,
        node: "eagle".to_string(),
        status: status.to_string(),
        cpu_pct: 12,
        mem_mb: 2048,
        disk_gb: 40,
        reachable: true,
        drift: DriftFlag::InSync,
    }
}

/// A fixture `state/cloud` mirror: OpenTofu **up**, Ansible **down**, libvirt
/// **absent** (the honest Up/Down/Absent tri-state), plus one Desktop VM + one
/// Service VM workload, plan-only (apply not armed).
fn fixture_state() -> CloudState {
    CloudState {
        host: "eagle".to_string(),
        adapter: CloudProviderAdapter::ConstructCloud,
        health: vec![
            health("opentofu", HealthState::Up),
            health("ansible", HealthState::Down),
            health("libvirt", HealthState::Absent),
        ],
        resources: Vec::new(),
        apply_armed: false,
        published_at_ms: 42,
        workloads: vec![
            workload("seat-1", DeliveryType::DesktopVm, "running"),
            workload("svc-1", DeliveryType::ServiceVm, "running"),
        ],
        drift_summary: DriftSummary::default(),
        node_capacity: NodeCapacity {
            vcpu_total: 16,
            vcpu_used: 4,
            mem_total_mb: 32768,
            mem_used_mb: 4096,
        },
    }
}

/// A surface state on `(view, panel)` with the fixture mirror folded in.
fn state_on(view: DeliveryView, panel: Panel) -> WorkloadsState {
    let mut state = WorkloadsState::default();
    state.set_view(view);
    state.set_panel(panel);
    state.states = vec![fixture_state()];
    state
}

/// Drive one headless frame of `infra_code_panel` and tessellate it on the CPU
/// (the DRM runner's path minus the GPU). Returns whether it drew primitives.
fn run_panel(state: &mut WorkloadsState) -> bool {
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
    // infrastructure brand glyph (the dock mount is unchanged by the reshape).
    use crate::dock::Surface;
    assert!(Surface::ALL.contains(&Surface::InfraCode));
    assert_eq!(
        Surface::InfraCode.icon_id(),
        mde_theme::brand::icons::IconId::Server
    );
}

#[test]
fn every_delivery_view_roster_renders_headless() {
    // Every delivery view's roster tessellates over the fixture mirror.
    for view in DeliveryView::ALL {
        let mut state = state_on(view, Panel::Roster);
        assert!(
            run_panel(&mut state),
            "{:?} roster drew nothing",
            view.label()
        );
    }
}

#[test]
fn every_lens_renders_headless() {
    // Every panel lens tessellates over the fixture mirror (each honest stub or
    // the roster).
    for panel in Panel::ALL {
        let mut state = state_on(DeliveryView::DesktopVm, panel);
        assert!(
            run_panel(&mut state),
            "{:?} lens drew nothing",
            panel.label()
        );
    }
}

#[test]
fn switching_views_and_lenses_works() {
    let mut state = state_on(DeliveryView::DesktopVm, Panel::Roster);
    assert_eq!(state.view(), DeliveryView::DesktopVm);
    assert_eq!(state.panel(), Panel::Roster);
    for view in DeliveryView::ALL {
        state.set_view(view);
        assert_eq!(state.view(), view);
        assert!(run_panel(&mut state), "{:?} render failed", view.label());
    }
    for panel in Panel::ALL {
        state.set_panel(panel);
        assert_eq!(state.panel(), panel);
        assert!(run_panel(&mut state), "{:?} render failed", panel.label());
    }
}

#[test]
fn the_empty_mirror_reads_honestly_never_fabricated() {
    // No mirror published yet → honest empty rosters / stubs per lens, never fake.
    for panel in [Panel::Roster, Panel::Status, Panel::Provision] {
        let mut state = WorkloadsState::default();
        state.set_panel(panel);
        assert!(
            run_panel(&mut state),
            "{:?} empty state drew nothing",
            panel.label()
        );
        assert!(
            state.mutation_pending.is_none() && state.note.is_none(),
            "{:?} must not emit a verb from an empty mirror",
            panel.label()
        );
    }
}

#[test]
fn the_roster_reads_its_workloads_by_delivery_type() {
    // The idiom the U16 views share: filter the mirror's workloads by type.
    let state = state_on(DeliveryView::DesktopVm, Panel::Roster);
    assert_eq!(state.workloads_of(DeliveryView::DesktopVm).count(), 1);
    assert_eq!(state.workloads_of(DeliveryView::ServiceVm).count(), 1);
    assert_eq!(state.workloads_of(DeliveryView::AppVm).count(), 0);
    assert_eq!(state.workloads_of(DeliveryView::AndroidVm).count(), 0);
    assert_eq!(
        state.workloads_of(DeliveryView::ServiceContainer).count(),
        0
    );
    // The DesktopVm roster tessellates with its single matching row.
    let mut state = state;
    assert!(run_panel(&mut state), "the Desktop VM roster drew nothing");
}

#[test]
fn provision_apply_is_typed_confirm_gated_and_emits_provision_only_after_confirm() {
    // Dry-run default: a plan is a direct emit (no confirm). Apply is gated.
    let mut state = state_on(DeliveryView::DesktopVm, Panel::Provision);

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
    let mut state = state_on(DeliveryView::DesktopVm, Panel::Roster);
    // Destroy opens the confirm on the DESTROY echo, publishes nothing yet.
    state.arm_destroy();
    let arming = state.arming.take().expect("destroy opens the confirm");
    assert_eq!(arming.action, ArmAction::Destroy);
    assert_eq!(arming.action.verb(), "destroy");
    assert!(armed("destroy", &arming.action.echo()));
    assert!(state.mutation_pending.is_none());

    // A destructive lifecycle op arms on the workload name (the roster row seam).
    state.arm_lifecycle("instance-delete", "seat-1", "seat-1");
    let arming = state.arming.as_ref().expect("delete opens the confirm");
    assert_eq!(arming.action.verb(), "instance-delete");
    assert_eq!(arming.action.echo(), "seat-1");
    assert!(state.mutation_pending.is_none() && state.note.is_none());
    // The armed confirm panel still tessellates.
    assert!(run_panel(&mut state), "the arming confirm drew nothing");
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
        r#"{"ok":false,"verb":"provision","gated":"live apply is capability-gated — tofu plan (staged): 2 to add — nothing applied"}"#,
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
fn carbon_icons_are_registered_for_every_view_and_lens() {
    // Every delivery-view tab + every lens tab resolves in the embedded
    // Mackes-Carbon registry (no glyph text, mesh present).
    for view in DeliveryView::ALL {
        assert!(
            mde_egui::carbon_svg_bytes(view.icon()).is_some(),
            "{:?} icon `{}` is not a registered Carbon glyph",
            view.label(),
            view.icon()
        );
    }
    for panel in Panel::ALL {
        assert!(
            mde_egui::carbon_svg_bytes(panel.icon()).is_some(),
            "{:?} icon `{}` is not a registered Carbon glyph",
            panel.label(),
            panel.icon()
        );
    }
    // The stub-card glyph resolves too.
    assert!(mde_egui::carbon_svg_bytes("view-grid").is_some());
}

/// Drive `run` in a headless frame and collect every text run painted — the
/// pixel-feed proof a fixture decode actually renders (the same `Context::run`
/// path the DRM runner drives, minus the GPU).
fn rendered_text(mut run: impl FnMut(&mut egui::Ui)) -> String {
    fn collect(shape: &egui::epaint::Shape, out: &mut String) {
        match shape {
            egui::epaint::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => {
                for s in shapes {
                    collect(s, out);
                }
            }
            _ => {}
        }
    }
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 720.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| run(ui));
    });
    let mut text = String::new();
    for clipped in &out.shapes {
        collect(&clipped.shape, &mut text);
    }
    text
}

#[test]
fn console_attach_decodes_the_endpoint_and_renders_it_honestly() {
    // Before any resolve, the section reads honestly — no fabricated handle.
    let unresolved = WorkloadsState::default();
    let before = rendered_text(|ui| console_section(ui, &unresolved));
    assert!(
        before.contains("No console resolved"),
        "an unresolved console must read honestly: {before}"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let bus_root = tmp.path().join("bus");
    let mut state = WorkloadsState::default();
    state.bus_root = Some(bus_root.clone());

    // Dispatch console-attach the way the roster's Console button does.
    state.issue_console_attach("seat-1", "seat-1");
    let ulid = state
        .mutation_pending
        .as_ref()
        .expect("console-attach published a pending request")
        .ulid
        .clone();

    // Write the fixture full-payload WireCloudReply the worker would answer with.
    let persist = Persist::open(bus_root).expect("open the fixture bus");
    let body = serde_json::json!({
        "ok": true,
        "verb": "console-attach",
        "audited": false,
        "console": {
            "proto": "spice",
            "uri": "spice://10.42.0.7:5901",
            "ticket": "one-time-token"
        }
    })
    .to_string();
    persist
        .write(&reply_topic(&ulid), Priority::Default, None, Some(&body))
        .expect("write the fixture reply");

    state.resolve_mutation();

    let resolved = state
        .console
        .as_ref()
        .expect("the console endpoint decoded from the full-payload wire reply");
    assert_eq!(resolved.name, "seat-1");
    assert_eq!(
        resolved.endpoint.proto,
        mackes_mesh_types::cloud::ConsoleProto::Spice
    );
    assert_eq!(resolved.endpoint.uri, "spice://10.42.0.7:5901");
    assert_eq!(resolved.endpoint.ticket.as_deref(), Some("one-time-token"));
    assert!(
        state.console_target.is_none(),
        "the target is cleared once resolved"
    );

    // The panel renders the resolved handle; the one-time ticket stays masked
    // (never painted in the clear, §7).
    let after = rendered_text(|ui| console_section(ui, &state));
    assert!(after.contains("spice://10.42.0.7:5901"), "{after}");
    assert!(after.contains("SPICE"), "{after}");
    assert!(
        !after.contains("one-time-token"),
        "the ticket must render masked: {after}"
    );
}

#[test]
fn labels_carry_no_legacy_backend_terminology() {
    // The cockpit is provider-neutral: zero OpenStack-family terms in its
    // user-facing copy (grep-clean, §6).
    let mut labels: Vec<String> =
        vec![CLOUD_PRODUCT_LABEL.to_string(), WORKSPACE_TITLE.to_string()];
    labels.extend(DeliveryView::ALL.iter().map(|v| v.label().to_string()));
    labels.extend(Panel::ALL.iter().map(|p| p.label().to_string()));
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
