use super::*;
use mackes_mesh_types::cloud::{
    CloudProviderAdapter, DriftFlag, DriftSummary, EndpointInterface, HealthState, ImageRow,
    NodeCapacity, ServiceHealth,
};
use mde_egui::egui::{pos2, vec2, Rect};

const TEST_ARM_KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("test clock is after the Unix epoch")
        .as_millis() as i64
}

#[test]
fn cloud_arm_credential_reader_is_bounded_and_non_following() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("cloud-arm-key");
    let valid = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    std::fs::write(&path, valid).expect("credential");
    assert_eq!(
        read_cloud_arm_credential(&path).expect("valid credential"),
        valid.to_vec()
    );

    std::fs::write(&path, vec![b'x'; MAX_CLOUD_ARM_CREDENTIAL_BYTES + 1])
        .expect("oversized credential");
    assert!(
        read_cloud_arm_credential(&path).is_err(),
        "oversized credentials must fail closed"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn cloud_arm_credential_reader_rejects_a_final_symlink() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("outside-key");
    let link = tmp.path().join("cloud-arm-key");
    std::fs::write(
        &target,
        b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .expect("target credential");
    symlink(&target, &link).expect("credential symlink");

    assert!(
        read_cloud_arm_credential(&link).is_err(),
        "credential loading must not follow a replaced final leaf"
    );
}

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
        published_at_ms: now_ms(),
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

/// A surface state on `(delivery filter, lifecycle route)` with the fixture
/// mirror folded in.
fn state_on(view: DeliveryView, route: WorkloadsRoute) -> WorkloadsState {
    let mut state = WorkloadsState::default();
    state.set_view(view);
    state.set_route(route);
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

/// A Workloads state backed by an isolated fixture Bus, with one explicit
/// placement selected.
fn placed_bus_state() -> (tempfile::TempDir, WorkloadsState) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut state = WorkloadsState::default();
    state.bus_root = Some(tmp.path().join("bus"));
    state.selected_node = Some("eagle".to_string());
    state.states = vec![fixture_state()];
    state.states[0].apply_armed = true;
    state.arm_key_override = Some(TEST_ARM_KEY.to_vec());
    (tmp, state)
}

#[test]
fn cloud_mirror_freshness_rejects_missing_stale_and_far_future_stamps() {
    let now = now_ms();
    let state = fixture_state();
    assert!(cloud_state_is_fresh_at(&state, now));

    let mut stale = state.clone();
    stale.published_at_ms = now - CLOUD_MIRROR_STALE_AFTER_MS - 1;
    assert!(!cloud_state_is_fresh_at(&stale, now));

    let mut missing = state.clone();
    missing.published_at_ms = 0;
    assert!(!cloud_state_is_fresh_at(&missing, now));

    let mut future = state;
    future.published_at_ms = now + 30 * 1000 + 1;
    assert!(!cloud_state_is_fresh_at(&future, now));
}

#[test]
fn stale_armed_node_cannot_open_live_provision_confirmation() {
    let (_tmp, mut state) = placed_bus_state();
    state.states[0].published_at_ms = now_ms() - CLOUD_MIRROR_STALE_AFTER_MS - 1;

    state.arm_provision();

    assert!(
        !state.has_arming(),
        "stale capability must not arm live apply"
    );
    assert!(state
        .note_text()
        .is_some_and(|note| note.contains("unavailable")));
}

/// Decode the only UI request emitted for `verb` from a fixture Bus.
fn emitted_request(state: &WorkloadsState, verb: &str) -> serde_json::Value {
    let persist =
        Persist::open(state.bus_root.clone().expect("fixture bus root")).expect("open fixture bus");
    let topic = format!("{}{verb}", mackes_mesh_types::cloud::CLOUD_ACTION_PREFIX);
    let messages = persist
        .list_since(&topic, None)
        .expect("read request topic");
    assert_eq!(messages.len(), 1, "expected one request on {topic}");
    serde_json::from_str(
        messages[0]
            .body
            .as_deref()
            .expect("the cloud request carries a JSON body"),
    )
    .expect("request body is JSON")
}

fn emitted_request_count(state: &WorkloadsState, verb: &str) -> usize {
    let persist =
        Persist::open(state.bus_root.clone().expect("fixture bus root")).expect("open fixture bus");
    let topic = format!("{}{verb}", mackes_mesh_types::cloud::CLOUD_ACTION_PREFIX);
    persist
        .list_since(&topic, None)
        .expect("read request topic")
        .len()
}

fn confirm_pending(state: &mut WorkloadsState) {
    let arming = state.arming.take().expect("typed confirmation is pending");
    let echo = arming.action.echo();
    state.perform(arming.action, &echo);
}

#[test]
fn the_surface_is_reachable_in_the_dock() {
    // §7 reachability: the surface stays in Surface::ALL and wears the server /
    // infrastructure brand glyph (the dock mount is unchanged by the reshape).
    use crate::surfaces::Surface;
    assert!(Surface::ALL.contains(&Surface::InfraCode));
    assert_eq!(
        Surface::InfraCode.icon_id(),
        mde_theme::brand::icons::IconId::Server
    );
}

#[test]
fn default_state_opens_provision_route() {
    let state = WorkloadsState::default();
    assert_eq!(state.route(), WorkloadsRoute::Provision);
    assert_eq!(state.view(), DeliveryView::DesktopVm);
    assert_eq!(state.density(), DensityMode::Compact);
}

#[test]
fn delivery_filter_renders_plan_without_changing_route() {
    // Delivery types are filters under the Plan route, not top-level nav.
    for view in DeliveryView::ALL {
        let mut state = state_on(view, WorkloadsRoute::Plan);
        assert_eq!(state.route(), WorkloadsRoute::Plan);
        assert!(
            run_panel(&mut state),
            "{:?} Plan filter drew nothing",
            view.label()
        );
        assert_eq!(state.route(), WorkloadsRoute::Plan);
    }
}

#[test]
fn every_lifecycle_route_renders_headless() {
    // Every lifecycle route tessellates over the fixture mirror.
    for route in WorkloadsRoute::ALL {
        let mut state = state_on(DeliveryView::DesktopVm, route);
        assert!(
            run_panel(&mut state),
            "{:?} route drew nothing",
            route.label()
        );
    }
}

#[test]
fn provision_route_renders_grouped_sections_and_sticky_actions() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Provision);
    state.bus_root = Some(tmp.path().join("bus"));
    state.selected_node = Some("eagle".to_string());
    state.states[0].apply_armed = true;
    state
        .form
        .set_test_draft("seat-1", "construct-desktop", "tags = [\"ops\"]");

    let text = rendered_text(|ui| route_body(ui, &mut state));

    for expected in [
        "Placement & delivery",
        "Placement node",
        "Delivery filter",
        "Live apply gate",
        "Armed by current mirror",
        "Identity",
        "Sizing",
        "Image & network",
        "HCL override",
        "Validation",
        "Sticky actions",
        "Set desired",
        "Plan",
        "Provision",
    ] {
        assert!(
            text.contains(expected),
            "Provision route must render grouped/sticky section {expected:?}: {text}"
        );
    }
    assert_eq!(
        emitted_request_count(&state, mackes_mesh_types::cloud::VERB_SET_DESIRED),
        0,
        "passive Provision render must not publish desired-state writes"
    );
    assert_eq!(
        emitted_request_count(&state, mackes_mesh_types::cloud::VERB_PLAN),
        0,
        "passive Provision render must not publish plan requests"
    );
    assert_eq!(
        emitted_request_count(&state, "provision"),
        0,
        "passive Provision render must not publish live provision requests"
    );
}

#[test]
fn provision_route_validation_distinguishes_plan_only_nodes() {
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Provision);
    state.selected_node = Some("eagle".to_string());
    state.states[0].apply_armed = false;
    state.form.set_test_draft("seat-1", "", "");

    let text = rendered_text(|ui| route_body(ui, &mut state));

    assert!(text.contains("Plan-only / not armed"), "{text}");
    assert!(text.contains("Live apply"), "{text}");
    assert!(
        text.contains("Plan remains available; live Provision stays disabled"),
        "{text}"
    );
    assert!(
        text.contains("Provision is disabled because the selected node is plan-only"),
        "{text}"
    );
}

#[test]
fn switching_filters_routes_and_density_works() {
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Plan);
    assert_eq!(state.view(), DeliveryView::DesktopVm);
    assert_eq!(state.route(), WorkloadsRoute::Plan);
    for view in DeliveryView::ALL {
        state.set_view(view);
        assert_eq!(state.view(), view);
        assert!(run_panel(&mut state), "{:?} render failed", view.label());
    }
    for route in WorkloadsRoute::ALL {
        state.set_route(route);
        assert_eq!(state.route(), route);
        assert!(run_panel(&mut state), "{:?} render failed", route.label());
    }
    for density in DensityMode::ALL {
        state.set_density(density);
        assert_eq!(state.density(), density);
        assert!(state.density().row_height() >= 30.0);
    }
}

#[test]
fn plan_resource_rows_filter_and_stably_sort() {
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Plan);
    state.states[0].workloads = vec![
        workload("same", DeliveryType::DesktopVm, "running"),
        workload("other", DeliveryType::ServiceVm, "running"),
        workload("same", DeliveryType::DesktopVm, "paused"),
        workload("alpha", DeliveryType::DesktopVm, "running"),
    ];
    state.states[0].workloads[0].node = "node-a".to_string();
    state.states[0].workloads[0].cpu_pct = 70;
    state.states[0].workloads[2].node = "node-b".to_string();
    state.states[0].workloads[2].cpu_pct = 10;
    state.states[0].workloads[3].node = "node-c".to_string();
    state.states[0].workloads[3].cpu_pct = 30;

    let rows = plan_resource_rows(&state);
    assert_eq!(
        rows.iter()
            .map(|row| (row.name.as_str(), row.node.as_str()))
            .collect::<Vec<_>>(),
        vec![("alpha", "node-c"), ("same", "node-a"), ("same", "node-b")],
        "name sort is stable for equal names and filters out Service VM rows"
    );

    state.toggle_resource_sort(WorkloadSortColumn::Cpu);
    let rows = plan_resource_rows(&state);
    assert_eq!(
        rows.iter()
            .map(|row| (row.name.as_str(), row.cpu_pct))
            .collect::<Vec<_>>(),
        vec![("same", 10), ("alpha", 30), ("same", 70)]
    );

    state.toggle_resource_sort(WorkloadSortColumn::Cpu);
    let rows = plan_resource_rows(&state);
    assert_eq!(
        rows.iter()
            .map(|row| (row.name.as_str(), row.cpu_pct))
            .collect::<Vec<_>>(),
        vec![("same", 70), ("alpha", 30), ("same", 10)]
    );
    assert_eq!(
        state.resource_sort(),
        WorkloadSort {
            column: WorkloadSortColumn::Cpu,
            descending: true,
        }
    );
}

#[test]
fn expanded_resource_row_is_keyed_by_delivery_node_and_name() {
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Plan);
    let key = plan_resource_key(&state.states[0].workloads[0]);

    state.toggle_expanded_resource(key.clone());
    assert_eq!(state.expanded_resource(), Some(key.as_str()));

    state.toggle_expanded_resource(key);
    assert_eq!(state.expanded_resource(), None);
}

#[test]
fn expanded_plan_row_renders_metrics_drift_and_command_preview() {
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Plan);
    let key = plan_resource_key(&state.states[0].workloads[0]);
    state.toggle_expanded_resource(key);

    let text = rendered_text(|ui| infra_code_panel(ui, &mut state));
    assert!(text.contains("Details"), "{text}");
    assert!(text.contains("Actions"), "{text}");
    assert!(text.contains("Command preview"), "{text}");
    assert!(text.contains("placement"), "{text}");
    assert!(text.contains("metrics"), "{text}");
    assert!(text.contains("in sync"), "{text}");
}

#[test]
fn run_route_uses_dense_resource_table_before_configure_lens() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Run);
    state.bus_root = Some(tmp.path().join("bus"));
    let key = plan_resource_key(&state.states[0].workloads[0]);
    state.toggle_expanded_resource(key);

    let text = rendered_text(|ui| lifecycle_resource_route(ui, &mut state, ResourceTableMode::Run));

    assert!(text.contains("Run resource table"), "{text}");
    assert!(text.contains("Run Actions"), "{text}");
    assert!(
        text.contains("Command preview") && text.contains("Run:"),
        "{text}"
    );
    assert!(text.contains("Console"), "{text}");
    assert!(text.contains("seat-1"), "{text}");
    assert_eq!(state.route(), WorkloadsRoute::Run);
    assert_eq!(
        emitted_request_count(&state, mackes_mesh_types::cloud::VERB_INVENTORY),
        0,
        "passive Run table render must not publish inventory reads"
    );
    assert_eq!(
        emitted_request_count(&state, "configure"),
        0,
        "passive Run table render must not publish configure requests"
    );
}

#[test]
fn drift_route_uses_dense_resource_table_with_plan_only_row_actions() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Drift);
    state.bus_root = Some(tmp.path().join("bus"));
    state.states[0].workloads[0].drift = DriftFlag::Drift;
    let key = plan_resource_key(&state.states[0].workloads[0]);
    state.toggle_expanded_resource(key);

    let text = rendered_text(|ui| route_body(ui, &mut state));

    assert!(text.contains("Drift resource table"), "{text}");
    assert!(text.contains("Drift Actions"), "{text}");
    assert!(
        text.contains("Command preview") && text.contains("Drift:"),
        "{text}"
    );
    assert!(text.contains("Plan node"), "{text}");
    assert!(text.contains("Desired-state drift"), "{text}");
    assert!(
        !text.contains("Destroy"),
        "Drift route must not expose live destructive row actions: {text}"
    );
    assert_eq!(
        emitted_request_count(&state, VERB_PLAN),
        0,
        "passive Drift render must not publish a plan until the row action is clicked"
    );
}

#[test]
fn containers_route_uses_dense_container_table_before_deploy_form() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Containers);
    state.bus_root = Some(tmp.path().join("bus"));
    state
        .states
        .get_mut(0)
        .expect("fixture state")
        .workloads
        .push(workload("web-1", DeliveryType::ServiceContainer, "active"));
    let key = plan_resource_key(
        state
            .states
            .first()
            .expect("fixture state")
            .workloads
            .last()
            .expect("container workload"),
    );
    state.toggle_expanded_resource(key);

    let text = rendered_text(|ui| route_body(ui, &mut state));

    assert!(text.contains("Container resource table"), "{text}");
    assert!(text.contains("Container Actions"), "{text}");
    assert!(text.contains("web-1"), "{text}");
    assert!(
        text.contains("Command preview") && text.contains("Containers:"),
        "{text}"
    );
    assert!(text.contains("Restart"), "{text}");
    assert!(text.contains("Deploy a service container"), "{text}");
    assert_eq!(
        emitted_request_count(&state, "container-restart"),
        0,
        "passive Containers table render must not publish lifecycle requests"
    );
    assert_eq!(
        state.view(),
        DeliveryView::DesktopVm,
        "Containers route must not mutate the operator's delivery filter just to show existing containers"
    );
}

#[test]
fn images_route_uses_dense_table_before_the_image_build_flow() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Images);
    state.bus_root = Some(tmp.path().join("bus"));
    state.selected_node = Some("eagle".to_string());
    state.images.set_test_version("1.0");
    let rows = vec![
        ImageRow {
            name: "desktop_vm-golden".to_string(),
            sha256: "abc123def456789000000000000000000000000000000000000000000000".to_string(),
            promoted: true,
        },
        ImageRow {
            name: "app_vm-golden".to_string(),
            sha256: "001122334455667788990000000000000000000000000000000000000000".to_string(),
            promoted: false,
        },
    ];
    state.images.set_test_roster(rows.clone());

    let text = rendered_text(|ui| route_body(ui, &mut state));

    assert!(text.contains("Image lifecycle table"), "{text}");
    assert!(text.contains("Image Actions"), "{text}");
    assert!(text.contains("desktop_vm-golden"), "{text}");
    assert!(text.contains("app_vm-golden"), "{text}");
    assert!(text.contains("Promote"), "{text}");
    assert!(
        text.find("Image lifecycle table") < text.find("Build a golden image"),
        "the dense table must render before the image-build flow: {text}"
    );
    assert_eq!(
        emitted_request_count(&state, mackes_mesh_types::cloud::VERB_IMAGE_BUILD),
        0,
        "passive Images table render must not publish image-build requests"
    );

    let mut expanded_state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Images);
    expanded_state.bus_root = Some(tmp.path().join("expanded-bus"));
    expanded_state.selected_node = Some("eagle".to_string());
    expanded_state.images.set_test_version("1.0");
    let expanded = images::image_row_key(&rows[1]);
    expanded_state.images.set_test_roster(rows);
    expanded_state.images.expand_test_row(expanded);

    let expanded_text = rendered_text(|ui| route_body(ui, &mut expanded_state));

    assert!(
        expanded_text.contains("Content hash") && expanded_text.contains("00112233445566778899"),
        "{expanded_text}"
    );
    assert!(
        expanded_text.contains("Command preview")
            && expanded_text.contains("action/cloud/image-build"),
        "{expanded_text}"
    );
}

#[test]
fn audit_route_renders_dense_session_table_newest_first() {
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Audit);
    state.audit.push(AuditEntry {
        verb: "plan".to_string(),
        outcome: AuditOutcome::Staged,
        detail: "planned node eagle".to_string(),
    });
    state.audit.push(AuditEntry {
        verb: "container-deploy".to_string(),
        outcome: AuditOutcome::Applied,
        detail: "audited container web-1".to_string(),
    });

    let text = rendered_text(|ui| audit_route_panel(ui, &state));

    assert!(text.contains("Audit table"), "{text}");
    assert!(text.contains("Outcome"), "{text}");
    assert!(text.contains("Verb"), "{text}");
    assert!(text.contains("Detail"), "{text}");
    assert!(text.contains("container-deploy"), "{text}");
    assert!(text.contains("audited container web-1"), "{text}");
    assert!(
        text.find("container-deploy") < text.find("plan"),
        "newest audit rows must render first: {text}"
    );
}

#[test]
fn the_empty_mirror_reads_honestly_never_fabricated() {
    // No mirror published yet → honest empty routes, never fake.
    for route in [
        WorkloadsRoute::Plan,
        WorkloadsRoute::Drift,
        WorkloadsRoute::Provision,
    ] {
        let mut state = WorkloadsState::default();
        state.set_route(route);
        assert!(
            run_panel(&mut state),
            "{:?} empty state drew nothing",
            route.label()
        );
        assert!(
            state.mutation_pending.is_none() && state.note.is_none(),
            "{:?} must not emit a verb from an empty mirror",
            route.label()
        );
    }
}

#[test]
fn the_roster_reads_its_workloads_by_delivery_type() {
    // The idiom the U16 views share: filter the mirror's workloads by type.
    let state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Plan);
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
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Provision);
    state.selected_node = Some("eagle".to_string());
    state.states[0].apply_armed = true;

    // Reviewing a live apply OPENS the confirm and publishes NOTHING (§ RUN-006).
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
        !armed("  apply ", &ArmAction::Provision.echo()),
        "padded echo is not exact"
    );
    assert!(
        !armed("appl", &ArmAction::Provision.echo()),
        "partial does not arm"
    );
    assert!(
        !armed("", &ArmAction::Provision.echo()),
        "empty does not arm"
    );

    // Past the gate, perform reaches the publish seam once placement is explicit
    // (no Bus in the test → an honest error note naming the provision verb).
    state.selected_node = Some("eagle".to_string());
    state.arm_key_override = Some(TEST_ARM_KEY.to_vec());
    state.perform(ArmAction::Provision, "apply");
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
fn provision_plan_emits_dedicated_plan_request_contract() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Plan);
    state.bus_root = Some(tmp.path().join("bus"));
    state.selected_node = Some("eagle".to_string());
    state.states[0].apply_armed = false;

    state.plan_provision();

    assert!(
        state.arming.is_none(),
        "planning does not open the apply review"
    );
    assert!(
        state.mutation_pending.is_some(),
        "the Plan action tracks the worker reply"
    );
    assert_eq!(
        emitted_request_count(&state, "provision"),
        0,
        "Plan must not publish the live provision verb"
    );
    let plan = emitted_request(&state, mackes_mesh_types::cloud::VERB_PLAN);
    assert_eq!(plan["schema_version"], CLOUD_ACTION_SCHEMA_VERSION);
    assert_eq!(plan["node"], "eagle");
    assert!(
        plan.get("armed_token").is_none(),
        "dry-run plan requests are not armed live-apply mutations"
    );
}

#[test]
fn plan_only_selected_node_cannot_open_live_provision_arm() {
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Provision);
    state.selected_node = Some("eagle".to_string());

    state.arm_provision();

    assert!(
        !state.has_arming(),
        "plan-only nodes must not open live apply"
    );
    assert!(state
        .note_text()
        .is_some_and(|note| note.contains("plan-only")));

    state.states[0].apply_armed = true;
    state.arm_provision();
    assert!(
        state.has_arming(),
        "an armed node may open the typed confirm"
    );
}

#[test]
fn configure_apply_refuses_missing_or_plan_only_selected_node() {
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Provision);

    state.arm_configure();
    assert!(!state.has_arming(), "a missing node must fail closed");
    assert!(state
        .note_text()
        .is_some_and(|note| note.contains("Live configuration is unavailable")));

    state.selected_node = Some("eagle".to_string());
    state.arm_configure();
    assert!(!state.has_arming(), "a plan-only node must fail closed");
    assert!(state
        .note_text()
        .is_some_and(|note| note.contains("plan-only")));
}

#[test]
fn armed_selected_node_accepts_configure_confirmation() {
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Provision);
    state.selected_node = Some("eagle".to_string());
    state.states[0].apply_armed = true;

    state.arm_configure();

    let arming = state.arming.as_ref().expect("armed node opens configure");
    assert_eq!(arming.action, ArmAction::Configure);
    assert_eq!(arming.action.verb(), "configure");
    assert!(arming.typed.is_empty());
    assert!(state.note.is_none());
}

#[test]
fn configure_apply_rechecks_capability_after_confirmation() {
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Provision);
    state.selected_node = Some("eagle".to_string());
    state.states[0].apply_armed = true;
    state.arm_configure();

    let action = state.arming.take().expect("configure confirmation").action;
    let echo = action.echo();
    state.states[0].apply_armed = false;
    state.perform(action, &echo);

    assert!(state.mutation_pending.is_none());
    assert!(state
        .note_text()
        .is_some_and(|note| note.contains("Nothing was sent")));
}

#[test]
fn set_desired_emits_the_worker_envelope_instead_of_a_bare_spec() {
    let (_tmp, mut state) = placed_bus_state();
    let spec = WorkloadSpec {
        name: "seat-1".to_string(),
        delivery_type: DeliveryType::DesktopVm,
        node: "eagle".to_string(),
        vcpu: 4,
        memory_mb: 8192,
        disk_gb: 60,
        image: Some("construct-desktop".to_string()),
        network_isolation: true,
        raw_hcl: None,
    };

    state.set_desired(&spec);
    assert!(
        state.mutation_pending.is_none(),
        "unconfirmed desired write is not published"
    );
    confirm_pending(&mut state);

    let body = emitted_request(&state, mackes_mesh_types::cloud::VERB_SET_DESIRED);
    assert_eq!(
        body["schema_version"],
        mackes_mesh_types::cloud::CLOUD_ACTION_SCHEMA_VERSION
    );
    assert_eq!(body["node"], "eagle");
    assert_eq!(body["spec"], serde_json::to_value(&spec).unwrap());
    let token = CloudArmedToken::parse(body["armed_token"].as_str().unwrap()).unwrap();
    assert_eq!(token.target, "desired:seat-1");
    assert!(body.get("name").is_none(), "spec leaked to request root");
}

#[test]
fn android_cuttlefish_action_emits_the_dedicated_cloud_contract() {
    let (_tmp, mut state) = placed_bus_state();

    state.arm_android_provision("  droid-1  ");
    let arming = state
        .arming
        .take()
        .expect("Android action opens confirmation");
    assert_eq!(arming.action.echo(), "droid-1");
    assert_eq!(arming.action.verb(), VERB_ANDROID_PROVISION);
    state.perform(arming.action, "droid-1");

    let body = emitted_request(&state, VERB_ANDROID_PROVISION);
    assert_eq!(body["schema_version"], CLOUD_ACTION_SCHEMA_VERSION);
    assert_eq!(body["node"], "eagle");
    assert_eq!(body["name"], "droid-1");
    let token = CloudArmedToken::parse(body["armed_token"].as_str().unwrap())
        .expect("Android action is capability-bound");
    assert_eq!(token.verb, VERB_ANDROID_PROVISION);
    assert_eq!(token.node, "eagle");
    assert_eq!(token.target, "droid-1");
}

#[test]
fn android_cuttlefish_reply_reads_as_desired_saved_not_live_applied() {
    let reply: CloudReply = serde_json::from_str(
        r#"{"ok":true,"verb":"android-provision","desired":[{"name":"droid-1"}]}"#,
    )
    .expect("Android reply parses");
    let (note, entry) = fold_mutation(&reply);
    assert!(
        note.contains("saved desired state") && note.contains("no VM"),
        "{note}"
    );
    assert_eq!(entry.outcome, AuditOutcome::Desired);
    assert!(entry.detail.contains("separate action"));
}

#[test]
fn ui_mutation_requests_carry_their_explicit_placement_node() {
    let (_tmp, mut state) = placed_bus_state();

    state.perform(ArmAction::Provision, "apply");
    let provision = emitted_request(&state, "provision");
    assert_eq!(provision["schema_version"], 1);
    assert_eq!(provision["node"], "eagle");
    let provision_token = CloudArmedToken::parse(provision["armed_token"].as_str().unwrap())
        .expect("root shell minted provision token");
    assert_eq!(provision_token.verb, "provision");
    assert_eq!(provision_token.node, "eagle");
    assert_eq!(provision_token.target, CLOUD_ARM_NODE_SCOPE);
    assert_eq!(
        provision_token.request_sha256,
        mackes_mesh_types::cloud::cloud_request_digest(&provision.to_string()).unwrap()
    );

    state.perform(ArmAction::Configure, "apply");
    let configure = emitted_request(&state, "configure");
    assert_eq!(configure["schema_version"], 1);
    assert_eq!(configure["node"], "eagle");
    assert_eq!(configure["playbook"], "site.yml");
    assert_eq!(configure["group"], "cloud_vm");
    assert!(CloudArmedToken::parse(configure["armed_token"].as_str().unwrap()).is_some());

    state.perform(
        ArmAction::Lifecycle {
            verb: "instance-start",
            node: "otter".to_string(),
            instance_id: "seat-1".to_string(),
            name: "seat-1".to_string(),
        },
        "seat-1",
    );
    let start = emitted_request(&state, "instance-start");
    assert_eq!(start["node"], "otter");
    assert_eq!(start["instance"], "seat-1");
    let start_token = CloudArmedToken::parse(start["armed_token"].as_str().unwrap()).unwrap();
    assert_eq!(start_token.target, "seat-1");

    state.issue_console_attach("otter", "seat-1", "seat-1");
    assert!(
        state.mutation_pending.is_some(),
        "prior start remains pending"
    );
    // Resolve the fixture's single-pending limitation before confirming console.
    state.mutation_pending = None;
    confirm_pending(&mut state);
    let console = emitted_request(&state, "console-attach");
    assert_eq!(console["schema_version"], 1);
    assert_eq!(console["node"], "otter");
    assert_eq!(console["instance"], "seat-1");
    let console_token = CloudArmedToken::parse(console["armed_token"].as_str().unwrap()).unwrap();
    assert_eq!(console_token.target, "seat-1");
}

#[test]
fn selected_node_forms_do_not_emit_node_agnostic_requests() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut state = WorkloadsState::default();
    state.bus_root = Some(tmp.path().join("bus"));

    state.plan_provision();

    assert!(state.mutation_pending.is_none());
    assert!(state
        .note
        .as_deref()
        .is_some_and(|note| note.contains("Select a placement node")));
}

#[test]
fn run_images_and_containers_share_and_retain_the_placement_selector() {
    for route in [
        WorkloadsRoute::Run,
        WorkloadsRoute::Images,
        WorkloadsRoute::Containers,
    ] {
        let mut state = state_on(DeliveryView::DesktopVm, route);
        state.selected_node = Some("eagle".to_string());

        let text = rendered_text(|ui| route_body(ui, &mut state));

        assert_eq!(
            state.selected_node(),
            Some("eagle"),
            "{} must retain the shared placement selection",
            route.label()
        );
        assert!(
            text.contains("Placement") && text.contains("eagle") && text.contains("Selected"),
            "{} must visibly render the shared placement selector: {text}",
            route.label()
        );
    }
}

#[test]
fn node_scoped_routes_without_selection_emit_no_node_agnostic_requests() {
    for (route, verbs) in [
        (
            WorkloadsRoute::Run,
            &[
                mackes_mesh_types::cloud::VERB_INVENTORY,
                mackes_mesh_types::cloud::VERB_OUTPUT,
                "configure",
            ][..],
        ),
        (
            WorkloadsRoute::Images,
            &[mackes_mesh_types::cloud::VERB_IMAGE_BUILD][..],
        ),
        (
            WorkloadsRoute::Containers,
            &[mackes_mesh_types::cloud::VERB_CONTAINER_DEPLOY][..],
        ),
    ] {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut state = state_on(DeliveryView::DesktopVm, route);
        state.bus_root = Some(tmp.path().join("bus"));
        state.selected_node = None;

        assert!(
            run_panel(&mut state),
            "{} route drew nothing",
            route.label()
        );
        assert!(
            state.mutation_pending.is_none(),
            "{} must not track a node-agnostic request",
            route.label()
        );
        for verb in verbs {
            assert_eq!(
                emitted_request_count(&state, verb),
                0,
                "{} must not publish node-agnostic {verb}",
                route.label()
            );
        }
    }
}

#[test]
fn run_and_prepared_route_actions_fail_closed_without_a_selected_node() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Run);
    state.bus_root = Some(tmp.path().join("bus"));
    state.selected_node = None;

    state.check_configure();
    state.arm_configure();

    assert!(state.mutation_pending.is_none());
    assert!(!state.has_arming());
    assert_eq!(emitted_request_count(&state, "configure"), 0);

    state.note = None;
    state.arm_prepared(
        mackes_mesh_types::cloud::VERB_CONTAINER_DEPLOY,
        "  ".to_string(),
        "web".to_string(),
        "{}".to_string(),
        "container deploy (web)".to_string(),
        "web".to_string(),
        "Deploy",
        "container web".to_string(),
    );

    assert!(
        !state.has_arming(),
        "prepared route actions must not open review with a blank node"
    );
    assert!(state
        .note_text()
        .is_some_and(|note| note.contains("Select a placement node")));
}

#[test]
fn prepared_review_sheet_renders_frozen_mutation_facts_before_confirm() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut state = state_on(DeliveryView::ServiceContainer, WorkloadsRoute::Containers);
    state.bus_root = Some(tmp.path().join("bus"));
    let body = serde_json::json!({
        "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        "node": "eagle",
        "name": "web",
        "image": "registry.example.test/web:1",
        "rootful": false,
    })
    .to_string();
    let digest = cloud_request_digest(&body).expect("fixture body has a stable digest");

    state.arm_prepared(
        mackes_mesh_types::cloud::VERB_CONTAINER_DEPLOY,
        "eagle".to_string(),
        "web".to_string(),
        body,
        "container deploy (web)".to_string(),
        "web".to_string(),
        "Deploy",
        "container web".to_string(),
    );

    let text = rendered_text(|ui| render_review_sheet(ui, &mut state));

    assert!(
        text.contains("Command") && text.contains("action/cloud/container-deploy"),
        "{text}"
    );
    assert!(
        text.contains("Subject") && text.contains("container web"),
        "{text}"
    );
    assert!(text.contains("Target") && text.contains("web"), "{text}");
    assert!(
        text.contains("Placement node") && text.contains("eagle"),
        "{text}"
    );
    assert!(
        text.contains("Body digest") && text.contains(&format!("sha256:{digest}")),
        "{text}"
    );
    assert!(
        text.contains("Body summary")
            && text.contains("schema_version")
            && text.contains("image")
            && text.contains("name"),
        "{text}"
    );
    assert!(
        text.contains("Frozen body")
            && text.contains("registry.example.test/web:1")
            && text.contains("\"rootful\":false"),
        "{text}"
    );
    assert!(
        text.contains("Blast radius")
            && text.contains("target web")
            && text.contains("placement node eagle"),
        "{text}"
    );
    assert!(state.mutation_pending.is_none());
    assert_eq!(
        emitted_request_count(&state, mackes_mesh_types::cloud::VERB_CONTAINER_DEPLOY),
        0,
        "review render must not publish before the exact echo confirms"
    );
}

#[test]
fn lifecycle_review_sheet_renders_frozen_mutation_facts_before_confirm() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Run);
    state.bus_root = Some(tmp.path().join("bus"));
    let body = lifecycle_request_body("eagle", "seat-1", Some("seat-1"));
    let digest = cloud_request_digest(&body).expect("fixture body has a stable digest");

    state.arm_lifecycle("instance-delete", "eagle", "seat-1", "seat-1");

    let text = rendered_text(|ui| render_review_sheet(ui, &mut state));

    assert!(
        text.contains("Command") && text.contains("action/cloud/instance-delete"),
        "{text}"
    );
    assert!(
        text.contains("Subject") && text.contains("workload seat-1"),
        "{text}"
    );
    assert!(text.contains("Target") && text.contains("seat-1"), "{text}");
    assert!(
        text.contains("Placement node") && text.contains("eagle"),
        "{text}"
    );
    assert!(
        text.contains("Body digest") && text.contains(&format!("sha256:{digest}")),
        "{text}"
    );
    assert!(
        text.contains("Body summary")
            && text.contains("instance")
            && text.contains("typed_name")
            && text.contains("schema_version"),
        "{text}"
    );
    assert!(
        text.contains("Frozen body")
            && text.contains("\"instance\":\"seat-1\"")
            && text.contains("\"typed_name\":\"seat-1\""),
        "{text}"
    );
    assert!(
        text.contains("Blast radius")
            && text.contains("one workload")
            && text.contains("No other node or workload"),
        "{text}"
    );
    assert!(state.mutation_pending.is_none());
    assert_eq!(
        emitted_request_count(&state, "instance-delete"),
        0,
        "review render must not publish before the exact echo confirms"
    );
}

#[test]
fn lifecycle_reboot_and_delete_are_typed_confirm_gated() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Plan);
    state.bus_root = Some(tmp.path().join("bus"));
    // A destructive lifecycle op arms on the workload name (the roster row seam).
    state.arm_lifecycle("instance-delete", "eagle", "seat-1", "seat-1");
    let arming = state.arming.as_ref().expect("delete opens the confirm");
    assert_eq!(arming.action.verb(), "instance-delete");
    assert_eq!(arming.action.echo(), "seat-1");
    assert!(state.mutation_pending.is_none() && state.note.is_none());
    // The armed confirm panel still tessellates.
    assert!(run_panel(&mut state), "the arming confirm drew nothing");

    state.arm_key_override = Some(TEST_ARM_KEY.to_vec());
    state.perform(
        ArmAction::Lifecycle {
            verb: "instance-delete",
            node: "eagle".to_string(),
            instance_id: "seat-1".to_string(),
            name: "seat-1".to_string(),
        },
        "seat-1",
    );
    let delete = emitted_request(&state, "instance-delete");
    assert_eq!(delete["schema_version"], 1);
    assert_eq!(delete["node"], "eagle");
    assert_eq!(delete["instance"], "seat-1");
    assert_eq!(delete["typed_name"], "seat-1");
    assert!(CloudArmedToken::parse(delete["armed_token"].as_str().unwrap()).is_some());
}

#[test]
fn lifecycle_and_console_actions_reject_incomplete_workload_identity() {
    let mut state = state_on(DeliveryView::DesktopVm, WorkloadsRoute::Plan);

    state.arm_lifecycle("instance-delete", "eagle", "seat-1", "");
    assert!(
        !state.has_arming(),
        "blank names must not open delete confirmation"
    );
    assert!(state
        .note_text()
        .is_some_and(|note| note.contains("identity is incomplete")));

    state.note = None;
    state.arm_lifecycle("instance-delete", "eagle", "", "seat-1");
    assert!(
        !state.has_arming(),
        "blank instance ids must not open delete confirmation"
    );

    state.note = None;
    state.issue_console_attach("eagle", "", "seat-1");
    assert!(
        !state.has_arming(),
        "blank console instance ids must not open attachment confirmation"
    );
    assert!(
        state.console_target.is_none(),
        "a rejected console request must not retain a stale target"
    );
}

#[test]
fn perform_rechecks_confirmation_and_mints_nothing_on_mismatch() {
    let (_tmp, mut state) = placed_bus_state();
    state.perform(ArmAction::Provision, "appl");
    assert!(state.mutation_pending.is_none());
    assert!(state
        .note
        .as_deref()
        .is_some_and(|note| note.contains("did not match")));

    state.note = None;
    state.perform(ArmAction::Provision, " apply ");
    assert!(
        state.mutation_pending.is_none(),
        "padded confirmation must not mint a capability"
    );
    assert!(state
        .note
        .as_deref()
        .is_some_and(|note| note.contains("did not match")));
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
fn carbon_icons_are_registered_for_every_view_and_route() {
    // Every delivery filter + every route resolves in the embedded
    // Mackes-Carbon registry (no glyph text, mesh present).
    for view in DeliveryView::ALL {
        assert!(
            mde_egui::carbon_svg_bytes(view.icon()).is_some(),
            "{:?} icon `{}` is not a registered Carbon glyph",
            view.label(),
            view.icon()
        );
    }
    for route in WorkloadsRoute::ALL {
        assert!(
            mde_egui::carbon_svg_bytes(route.icon()).is_some(),
            "{:?} icon `{}` is not a registered Carbon glyph",
            route.label(),
            route.icon()
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
    state.arm_key_override = Some(TEST_ARM_KEY.to_vec());

    // Dispatch console-attach the way the roster's Console button does.
    state.issue_console_attach("eagle", "seat-1", "seat-1");
    assert!(
        state.mutation_pending.is_none(),
        "unconfirmed console request is not published"
    );
    confirm_pending(&mut state);
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
    // The lifecycle app is provider-neutral: zero OpenStack-family terms in its
    // user-facing copy (grep-clean, §6).
    let mut labels: Vec<String> =
        vec![CLOUD_PRODUCT_LABEL.to_string(), WORKSPACE_TITLE.to_string()];
    labels.extend(DeliveryView::ALL.iter().map(|v| v.label().to_string()));
    labels.extend(
        WorkloadsRoute::ALL
            .iter()
            .map(|route| route.label().to_string()),
    );
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
