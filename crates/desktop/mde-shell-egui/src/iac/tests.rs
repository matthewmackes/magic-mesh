use super::*;
use mackes_mesh_types::cloud::{shape_health, ProbeOutcome};
use mde_egui::egui::{pos2, vec2, Rect};

/// A realistic Keystone v3 token catalog — a three-interface compute service,
/// a single-interface identity service, and an image service (mirrors the
/// shared crate's fixture, so the surface is exercised against the real shape).
const V3_TOKEN: &str = r#"{
      "token": {
        "catalog": [
          {
            "type": "compute", "name": "nova",
            "endpoints": [
              {"interface": "public",   "url": "http://nova.mesh:8774/v2.1", "region": "RegionOne"},
              {"interface": "internal", "url": "http://nova.mesh:8774/v2.1", "region": "RegionOne"},
              {"interface": "admin",    "url": "http://nova.mesh:8774/v2.1", "region": "RegionOne"}
            ]
          },
          {
            "type": "identity", "name": "keystone",
            "endpoints": [
              {"interface": "public", "url": "http://keystone.mesh:5000/v3", "region": "RegionOne"}
            ]
          },
          {
            "type": "image", "name": "glance",
            "endpoints": [
              {"interface": "public", "url": "http://glance.mesh:9292", "region": "RegionOne"}
            ]
          }
        ]
      }
    }"#;

/// A fixture view: the real catalog + health rows where compute + identity
/// probe **up** and image probes **down** (2 of 3 healthy) — so the render +
/// the status counts are exercised over a mixed-health directory.
pub(super) fn fixture_view() -> CatalogView {
    let catalog = ServiceCatalog::from_keystone_token_json(V3_TOKEN).expect("fixture catalog");
    let up = |ty: &str, url: &str| {
        shape_health(
            ty,
            EndpointInterface::Public,
            url,
            &ProbeOutcome::Reachable {
                http_status: 200,
                body: String::new(),
                elapsed_ms: 12,
            },
        )
    };
    let health = vec![
        up("compute", "http://nova.mesh:8774/v2.1"),
        up("identity", "http://keystone.mesh:5000/v3"),
        shape_health(
            "image",
            EndpointInterface::Public,
            "http://glance.mesh:9292",
            &ProbeOutcome::Unreachable {
                elapsed_ms: 2000,
                reason: "connection refused".to_string(),
            },
        ),
    ];
    CatalogView { catalog, health }
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
    // §7 reachability: the surface is in Surface::ALL and wears the server /
    // infrastructure brand glyph (the group membership is pinned by dock.rs).
    use crate::dock::Surface;
    assert!(Surface::ALL.contains(&Surface::InfraCode));
    assert_eq!(
        Surface::InfraCode.icon_id(),
        mde_theme::brand::icons::IconId::Server
    );
}

#[test]
fn overview_renders_from_a_fixture_catalog() {
    let mut state = InfraCodeState {
        outcome: CatalogOutcome::Ready(fixture_view()),
        ..InfraCodeState::default()
    };
    assert!(
        run_panel(&mut state),
        "the Overview (status band + directory) produced no draw primitives"
    );
    // Expanding the endpoint URLs still tessellates cleanly.
    state.show_urls = true;
    assert!(run_panel(&mut state), "the URL-expanded tiles drew nothing");
}

#[test]
fn the_honest_not_configured_state_renders() {
    // A node with no clouds.yaml reads "not configured", never fake data (§7).
    let mut state = InfraCodeState {
        outcome: CatalogOutcome::NotConfigured("no clouds.yaml on this node".to_string()),
        ..InfraCodeState::default()
    };
    assert!(
        run_panel(&mut state),
        "the not-configured empty state produced no draw primitives"
    );
}

#[test]
fn the_querying_and_failed_states_render() {
    let mut querying = InfraCodeState::default();
    assert!(matches!(querying.outcome, CatalogOutcome::Querying));
    assert!(run_panel(&mut querying), "the querying state drew nothing");

    let mut failed = InfraCodeState {
        outcome: CatalogOutcome::Failed("keystone auth failed".to_string()),
        ..InfraCodeState::default()
    };
    assert!(run_panel(&mut failed), "the failed state drew nothing");
}

#[test]
fn fold_reply_maps_the_reply_tri_state_honestly() {
    // A successful reply (the real wire shape mackesd emits) folds to Ready.
    let view = fixture_view();
    let ok_body = serde_json::json!({
        "ok": true,
        "verb": "get-catalog",
        "audited": false,
        "catalog": view.catalog,
        "health": view.health,
    })
    .to_string();
    let reply: CatalogReply = serde_json::from_str(&ok_body).expect("ok reply parses");
    match fold_reply(reply) {
        CatalogOutcome::Ready(v) => {
            assert_eq!(v.catalog.services.len(), 3);
            assert_eq!(v.healthy_count(), 2);
        }
        other => panic!("an ok reply must fold to Ready, got {other:?}"),
    }

    // A gated reply → NotConfigured (the honest "no clouds.yaml").
    let gated: CatalogReply = serde_json::from_str(
        r#"{"ok":false,"verb":"get-catalog","audited":false,"gated":"no clouds.yaml on node-a"}"#,
    )
    .expect("gated reply parses");
    assert!(matches!(
        fold_reply(gated),
        CatalogOutcome::NotConfigured(r) if r.contains("clouds.yaml")
    ));

    // An error reply → Failed.
    let errored: CatalogReply = serde_json::from_str(
        r#"{"ok":false,"verb":"get-catalog","audited":false,"error":"keystone auth failed"}"#,
    )
    .expect("error reply parses");
    assert!(matches!(
        fold_reply(errored),
        CatalogOutcome::Failed(r) if r.contains("auth failed")
    ));

    // An `ok` reply with no directory is a failure, never a fabricated empty
    // catalog (§7).
    let empty: CatalogReply =
        serde_json::from_str(r#"{"ok":true,"verb":"get-catalog","audited":false}"#)
            .expect("bare ok reply parses");
    assert!(matches!(fold_reply(empty), CatalogOutcome::Failed(_)));
}

#[test]
fn provider_neutral_iac_labels_do_not_leak_the_openstack_backend() {
    let not_configured = cloud_provider_not_configured("no clouds.yaml on node-a");
    assert_eq!(
        not_configured,
        "Cloud provider not configured \u{2014} no clouds.yaml on node-a"
    );
    assert!(
        not_configured.contains("clouds.yaml"),
        "operator diagnostics stay intact"
    );

    let ok: CatalogReply = serde_json::from_str(
        r#"{"ok":true,"verb":"heat-update","audited":true,"stack":"mesh-net"}"#,
    )
    .expect("ok mutation reply parses");
    let gated: CatalogReply = serde_json::from_str(
        r#"{"ok":false,"verb":"heat-update","audited":true,"gated":"no clouds.yaml"}"#,
    )
    .expect("gated mutation reply parses");
    let failed: CatalogReply = serde_json::from_str(
        r#"{"ok":false,"verb":"heat-update","audited":true,"error":"HTTP 503"}"#,
    )
    .expect("failed mutation reply parses");

    let labels = [
        CLOUD_PRODUCT_LABEL.to_string(),
        CLOUD_API_STATUS_LABEL.to_string(),
        ORCHESTRATION_TAB_LABEL.to_string(),
        REVERSE_GENERATED_TEMPLATE_LABEL.to_string(),
        ORCHESTRATION_TEMPLATE_LABEL.to_string(),
        not_configured,
        heat_mutation_note(&ok),
        heat_mutation_note(&gated),
        heat_mutation_note(&failed),
    ];
    for label in labels {
        for backend in ["OpenStack", "Keystone", "Nova", "Heat", "Horizon", "HOT"] {
            assert!(
                !label.contains(backend),
                "user-facing IaC label must stay provider-neutral: {label}"
            );
        }
    }
}

#[test]
fn services_group_into_buckets_by_type() {
    assert_eq!(service_bucket("compute"), "Compute");
    assert_eq!(service_bucket("network"), "Network");
    assert_eq!(service_bucket("image"), "Image");
    assert_eq!(service_bucket("volumev3"), "Volume");
    assert_eq!(service_bucket("orchestration"), "Orchestration");
    assert_eq!(service_bucket("identity"), "Identity");
    assert_eq!(service_bucket("object-store"), "Object Storage");
    // An unknown/new service type is grouped honestly, never dropped.
    assert_eq!(service_bucket("load-balancer"), "Other");
    // Every bucket a service can map to is one of the rendered BUCKETS.
    for ty in ["compute", "network", "image", "volumev3", "dns", "weird"] {
        assert!(BUCKETS.contains(&service_bucket(ty)));
    }
}

#[test]
fn health_for_prefers_the_public_interface() {
    let view = fixture_view();
    let compute = view.catalog.service("compute").expect("compute");
    let health = view.health_for(compute).expect("compute health");
    assert_eq!(health.interface, EndpointInterface::Public);
    assert_eq!(health.state, HealthState::Up);
    // A service with no health row reads unprobed (None), never a faked up.
    let mut bare = view.clone();
    bare.health.clear();
    assert!(bare.health_for(compute).is_none());
}

#[test]
fn authority_extracts_host_and_port() {
    assert_eq!(authority("http://nova.mesh:8774/v2.1"), "nova.mesh:8774");
    assert_eq!(
        authority("https://keystone.mesh:5000/v3"),
        "keystone.mesh:5000"
    );
    assert_eq!(
        authority("http://user@glance.mesh:9292"),
        "glance.mesh:9292"
    );
    assert_eq!(authority("glance.mesh:9292"), "glance.mesh:9292");
}

// ─────────────────────────── IAC-3: Resources tab ───────────────────────────

/// A two-row Nova compute table — the fixture the Resources tab renders.
pub(super) fn fixture_resource_table() -> ResourceTable {
    ResourceTable::from_collection_json(
        "compute",
        "servers/detail",
        r#"{"servers":[
                {"id":"i-1","name":"web","status":"ACTIVE"},
                {"id":"i-2","name":"db","status":"SHUTOFF"}
            ]}"#,
    )
    .expect("fixture table")
}

/// A surface state on the Resources tab over the fixture catalog, with the
/// compute pane populated (`ready` = its resource table landed).
fn resources_state(ready: bool) -> InfraCodeState {
    let mut state = InfraCodeState {
        outcome: CatalogOutcome::Ready(fixture_view()),
        tab: IacTab::Resources,
        ..InfraCodeState::default()
    };
    if ready {
        state.resources.insert(
            "compute".to_string(),
            ResourcePane {
                outcome: Some(ResourceOutcome::Ready(fixture_resource_table())),
                ..ResourcePane::default()
            },
        );
    }
    state
}

#[test]
fn the_tab_bar_switches_and_the_heat_tab_is_an_honest_empty_state() {
    // The three tabs render; the default is Overview (IAC-2 render).
    let mut state = InfraCodeState {
        outcome: CatalogOutcome::Ready(fixture_view()),
        ..InfraCodeState::default()
    };
    assert_eq!(state.tab, IacTab::Overview);
    assert!(run_panel(&mut state), "Overview drew nothing");
    // Heat is an honest forward-looking empty state (not a disabled tab, §7).
    state.tab = IacTab::Heat;
    assert!(run_panel(&mut state), "the Heat empty state drew nothing");
}

#[test]
fn resources_renders_honestly_empty_with_no_reply_and_rows_with_one() {
    // Resources tab, catalog Ready, but no pane reply yet → honest "querying"
    // per service, never fabricated rows (§7).
    let mut empty = resources_state(false);
    assert!(
        run_panel(&mut empty),
        "the querying Resources tab drew nothing"
    );
    // A landed fixture list-resources reply renders the rows.
    let mut ready = resources_state(true);
    assert!(
        run_panel(&mut ready),
        "the populated Resources table drew nothing"
    );
    // Selecting a row + re-render (bulk selection is a real toggle set).
    ready
        .selected
        .insert(("compute".to_string(), "i-1".to_string()));
    assert!(
        run_panel(&mut ready),
        "the selected-row render drew nothing"
    );
}

#[test]
fn resources_reads_honestly_when_the_catalog_is_absent() {
    // Until the catalog answers, the Resources tab reads the same honest
    // catalog-absent story as the Overview (never an empty table of nothing).
    let mut not_configured = InfraCodeState {
        outcome: CatalogOutcome::NotConfigured("no clouds.yaml".to_string()),
        tab: IacTab::Resources,
        ..InfraCodeState::default()
    };
    assert!(run_panel(&mut not_configured), "drew nothing");
}

#[test]
fn fold_resource_reply_maps_the_reply_tri_state_honestly() {
    let table = fixture_resource_table();
    let ok_body = serde_json::json!({
        "ok": true, "verb": "list-resources", "audited": false, "resources": table,
    })
    .to_string();
    let reply: CatalogReply = serde_json::from_str(&ok_body).expect("ok reply parses");
    match fold_resource_reply(reply) {
        ResourceOutcome::Ready(t) => assert_eq!(t.rows.len(), 2),
        other => panic!("an ok reply with a table must fold to Ready, got {other:?}"),
    }
    // A gated reply → NotConfigured; an error → Failed; ok-with-no-table →
    // Failed (never a fabricated empty table).
    let gated: CatalogReply = serde_json::from_str(
        r#"{"ok":false,"verb":"list-resources","audited":false,"gated":"no clouds.yaml"}"#,
    )
    .unwrap();
    assert!(matches!(
        fold_resource_reply(gated),
        ResourceOutcome::NotConfigured(_)
    ));
    let errored: CatalogReply = serde_json::from_str(
        r#"{"ok":false,"verb":"list-resources","audited":false,"error":"HTTP 500"}"#,
    )
    .unwrap();
    assert!(matches!(
        fold_resource_reply(errored),
        ResourceOutcome::Failed(r) if r.contains("500")
    ));
    let bare: CatalogReply =
        serde_json::from_str(r#"{"ok":true,"verb":"list-resources","audited":false}"#).unwrap();
    assert!(matches!(
        fold_resource_reply(bare),
        ResourceOutcome::Failed(_)
    ));
}

#[test]
fn typed_arming_blocks_an_unconfirmed_mutation() {
    // The arming gate: only an exact (trimmed) name match arms the mutation.
    assert!(armed("web", "web"));
    assert!(armed("  web ", "web"), "surrounding space is tolerated");
    assert!(!armed("we", "web"), "a partial echo does not arm");
    assert!(!armed("", "web"), "an empty echo does not arm");

    // Applying a destructive verb OPENS the typed-arming confirm — it does
    // NOT publish anything (no note, no Bus request) until the name is typed.
    let mut state = resources_state(true);
    state
        .selected
        .insert(("compute".to_string(), "i-1".to_string()));
    menubar::apply(
        &mut state,
        menubar::MenuAction::ArmLifecycle {
            verb: "instance-delete",
            instance_id: "i-1".to_string(),
            name: "web".to_string(),
        },
    );
    let arming = state
        .arming
        .as_ref()
        .expect("delete opens the arming confirm");
    assert_eq!(arming.verb, "instance-delete");
    assert_eq!(arming.target_name, "web");
    assert!(arming.typed.is_empty());
    assert!(
        state.note.is_none(),
        "an unconfirmed mutation publishes nothing (no action note)"
    );
}

#[test]
fn drill_and_refresh_menu_actions_drive_their_real_seams() {
    let mut state = resources_state(false);
    state.tab = IacTab::Overview;
    // Drill switches to Resources + focuses the service (the linked view).
    menubar::apply(
        &mut state,
        menubar::MenuAction::Drill("network".to_string()),
    );
    assert_eq!(state.tab, IacTab::Resources);
    assert_eq!(state.linked_focus.as_deref(), Some("network"));
    // Refresh queues an immediate re-poll of that service's pane.
    menubar::apply(
        &mut state,
        menubar::MenuAction::RefreshResources("compute".to_string()),
    );
    assert!(state.resources.get("compute").expect("pane").forced);
}

#[test]
fn single_selected_instance_is_some_only_for_exactly_one_compute_row() {
    let mut state = resources_state(true);
    assert!(state.single_selected_instance().is_none(), "none selected");
    state
        .selected
        .insert(("compute".to_string(), "i-1".to_string()));
    // Resolves the name from the compute pane's table.
    assert_eq!(
        state.single_selected_instance(),
        Some(("i-1".to_string(), "web".to_string()))
    );
    // A second compute selection makes the destructive target ambiguous → None.
    state
        .selected
        .insert(("compute".to_string(), "i-2".to_string()));
    assert!(state.single_selected_instance().is_none(), "two selected");
}

// ─────────────────────────── IAC-4: Heat tab ───────────────────────────

/// A catalog view that advertises orchestration (Heat) — so the Heat tab is
/// live — plus compute (a reverse-generate source).
pub(super) fn heat_view() -> CatalogView {
    let catalog = ServiceCatalog::from_keystone_token_json(
        r#"{"token":{"catalog":[
                {"type":"orchestration","name":"heat","endpoints":[
                    {"interface":"public","url":"http://heat.mesh:8004/v1/p","region":"RegionOne"}
                ]},
                {"type":"compute","name":"nova","endpoints":[
                    {"interface":"public","url":"http://nova.mesh:8774/v2.1","region":"RegionOne"}
                ]}
            ]}}"#,
    )
    .expect("heat catalog");
    CatalogView {
        catalog,
        health: vec![],
    }
}

/// A fixture Heat stack list (the orchestration `list-resources` table).
fn fixture_stack_table() -> ResourceTable {
    ResourceTable::from_collection_json(
        "orchestration",
        "stacks",
        r#"{"stacks":[
                {"id":"s-1","stack_name":"mesh-net","stack_status":"CREATE_COMPLETE"},
                {"id":"s-2","stack_name":"web","stack_status":"UPDATE_COMPLETE"}
            ]}"#,
    )
    .expect("fixture stacks")
}

/// A fixture stack detail with resources, events, outputs, and a template.
fn fixture_stack_detail() -> HeatStackDetail {
    HeatStackDetail::from_stack_json(
            r#"{"stack":{"id":"s-1","stack_name":"mesh-net","stack_status":"CREATE_COMPLETE",
                "stack_status_reason":"Stack CREATE completed successfully",
                "outputs":[{"output_key":"net_id","output_value":"n-9","description":"the net id"}]}}"#,
        )
        .unwrap()
        .with_resources_json(
            r#"{"resources":[{"resource_name":"net","resource_type":"OS::Neutron::Net","resource_status":"CREATE_COMPLETE","physical_resource_id":"n-9"}]}"#,
        )
        .with_events_json(
            r#"{"events":[{"event_time":"2026-07-05T00:00:00Z","resource_name":"net","resource_status":"CREATE_COMPLETE","resource_status_reason":"state changed"}]}"#,
        )
        .with_template_json(r#"{"heat_template_version":"2021-04-16","resources":{}}"#)
}

/// A Heat-tab state over the Heat catalog with the stack list ready +,
/// optionally, the selected stack's detail + editable buffer loaded.
fn heat_tab_state(with_detail: bool) -> InfraCodeState {
    let mut state = InfraCodeState {
        outcome: CatalogOutcome::Ready(heat_view()),
        tab: IacTab::Heat,
        ..InfraCodeState::default()
    };
    state.resources.insert(
        "orchestration".to_string(),
        ResourcePane {
            outcome: Some(ResourceOutcome::Ready(fixture_stack_table())),
            ..ResourcePane::default()
        },
    );
    if with_detail {
        let detail = fixture_stack_detail();
        state.heat.template_buf = detail.template.clone();
        state.heat.template_for = Some("s-1".to_string());
        state.heat.selected = Some(("s-1".to_string(), "mesh-net".to_string()));
        state.heat.show_for = Some("s-1".to_string());
        state.heat.detail = Some(HeatOutcome::Ready(detail));
    }
    state
}

#[test]
fn the_heat_tab_renders_honestly_empty_with_no_reply_and_a_list_with_one() {
    // No orchestration service in the catalog → an honest "no Heat", never a
    // fabricated engine (§7).
    let mut no_heat = InfraCodeState {
        outcome: CatalogOutcome::Ready(fixture_view()),
        tab: IacTab::Heat,
        ..InfraCodeState::default()
    };
    assert!(run_panel(&mut no_heat), "the no-Heat state drew nothing");
    // Heat cataloged but no stack reply yet → honest querying, no fake stacks.
    let mut querying = InfraCodeState {
        outcome: CatalogOutcome::Ready(heat_view()),
        tab: IacTab::Heat,
        ..InfraCodeState::default()
    };
    assert!(
        run_panel(&mut querying),
        "the querying Heat tab drew nothing"
    );
    // A landed stack list renders the table.
    let mut ready = heat_tab_state(false);
    assert!(run_panel(&mut ready), "the stack list drew nothing");
}

#[test]
fn a_fixture_heat_show_renders_resources_events_outputs_and_template() {
    let mut state = heat_tab_state(true);
    // The fixture detail carries each section (proves the fold + the render).
    match &state.heat.detail {
        Some(HeatOutcome::Ready(d)) => {
            assert_eq!(d.resources.len(), 1);
            assert_eq!(d.events.len(), 1);
            assert_eq!(d.outputs.len(), 1);
            assert!(d.template.contains("heat_template_version"));
        }
        other => panic!("expected a ready detail, got {other:?}"),
    }
    assert!(
        run_panel(&mut state),
        "the stack detail (resources/events/outputs/template) drew nothing"
    );
}

#[test]
fn the_preview_update_diff_renders_a_fixture_diff() {
    let mut state = heat_tab_state(true);
    state.heat.preview = Some(HeatOutcome::Ready(HeatPreview {
        added: vec!["new_net".to_string()],
        replaced: vec!["server".to_string()],
        unchanged: vec!["router".to_string()],
        ..HeatPreview::default()
    }));
    assert!(run_panel(&mut state), "the preview diff drew nothing");
    // A no-change diff renders honestly too.
    state.heat.preview = Some(HeatOutcome::Ready(HeatPreview::default()));
    assert!(run_panel(&mut state), "the no-change preview drew nothing");
}

#[test]
fn typed_arming_blocks_an_unconfirmed_stack_delete() {
    // The arming gate is the shared exact-name match.
    assert!(armed("mesh-net", "mesh-net"));
    assert!(!armed("mesh", "mesh-net"), "a partial echo does not arm");
    // Arming a delete OPENS the confirm — it publishes nothing (no mutation
    // request) until the name is typed (#22).
    let mut state = heat_tab_state(true);
    state.arm_heat_delete();
    let arming = state
        .heat
        .arming
        .as_ref()
        .expect("delete opens the arming confirm");
    assert_eq!(arming.op, HeatOp::Delete);
    assert_eq!(arming.stack_name, "mesh-net");
    assert_eq!(arming.stack_id, "s-1");
    assert!(arming.typed.is_empty());
    assert!(
        state.heat.mutation_pending.is_none(),
        "an unconfirmed delete publishes nothing"
    );
    // The armed render still tessellates (the confirm panel).
    assert!(run_panel(&mut state), "the arming confirm drew nothing");
}

#[test]
fn reverse_generate_output_renders_and_create_arms_on_the_typed_name() {
    // The reverse-generated HOT (produced mesh-side) renders as a copyable view.
    let mut state = heat_tab_state(false);
    state.heat.reverse = Some(HeatOutcome::Ready(
        "heat_template_version: 2021-04-16\nresources: {}\n".to_string(),
    ));
    assert!(run_panel(&mut state), "the reverse output drew nothing");
    // Create is typed-armed on the entered name; an empty name refuses to arm.
    state.heat.create_name = String::new();
    state.arm_heat_create();
    assert!(
        state.heat.arming.is_none(),
        "an empty name does not arm a create"
    );
    state.heat.create_name = "fresh".to_string();
    state.heat.create_template = "heat_template_version: 2021-04-16".to_string();
    state.arm_heat_create();
    let arming = state.heat.arming.as_ref().expect("create arms on a name");
    assert_eq!(arming.op, HeatOp::Create);
    assert_eq!(arming.stack_name, "fresh");
    assert!(
        state.heat.mutation_pending.is_none(),
        "arming publishes nothing"
    );
}

#[test]
fn reverse_services_exclude_orchestration_itself() {
    // Reverse-generate captures raw infra (compute/…), not existing stacks.
    let state = InfraCodeState {
        outcome: CatalogOutcome::Ready(heat_view()),
        ..InfraCodeState::default()
    };
    let services = state.heat_reverse_services();
    assert!(services.iter().any(|(ty, _)| ty == "compute"));
    assert!(
        !services.iter().any(|(ty, _)| ty == "orchestration"),
        "orchestration is excluded from the reverse-generate source set"
    );
}

#[test]
fn heat_toolbar_uses_refined_shared_chrome_metrics() {
    assert_eq!(
        HEAT_TOOLBAR_BUTTON_H,
        Style::TOOLBAR_CONTROL_H,
        "the Heat toolbar should use the shared refined visual control height"
    );
    for label in [
        "Reverse-generate template",
        "New stack\u{2026}",
        "Close new-stack form",
    ] {
        let size = heat_toolbar_button_size(label);
        assert_eq!(
            size.y, HEAT_TOOLBAR_BUTTON_H,
            "{label:?} should use the shared compact toolbar control height"
        );
        assert!(
            size.x >= HEAT_TOOLBAR_BUTTON_MIN_W && size.x <= HEAT_TOOLBAR_BUTTON_MAX_W,
            "{label:?} width should be bounded for a refined toolbar, got {size:?}"
        );
    }
    assert!(
        HEAT_TOOLBAR_BUTTON_H < Style::SP_L,
        "the Heat toolbar buttons should stay visually slimmer than the old 24pt toolbar row"
    );
    assert_eq!(
        Style::toolbar_margin().top,
        Style::TOOLBAR_INSET_Y as i8,
        "the Heat strip relies on the shared refined toolbar inset"
    );
}

#[test]
fn heat_toolbar_actions_keep_the_existing_state_seams() {
    let mut state = heat_tab_state(false);
    assert!(!state.heat.show_create);
    toggle_heat_create_form(&mut state);
    assert!(state.heat.show_create);
    toggle_heat_create_form(&mut state);
    assert!(!state.heat.show_create);

    state.send_heat_reverse();
    assert!(
        state.heat.reverse_pending.is_some()
            || matches!(state.heat.reverse, Some(HeatOutcome::Failed(_))),
        "reverse-generate should hit the real request seam: publish when the Bus is reachable or fail honestly"
    );
}

/// The Heat panel's form / preview / arming cards cast the shared
/// `Elevation::Raised` soft shadow (Phase-C depth adoption): every field of
/// [`card_shadow`] comes straight from the token — offset/blur/spread and,
/// critically, the umbra colour (no minted `Color32`, §4) — and the umbra stays
/// translucent (design lock #2), so the cards read as genuinely lifted, never an
/// opaque fill.
#[test]
fn heat_card_shadow_is_the_raised_depth_token() {
    let raised = mde_egui::style::Elevation::Raised.shadow();
    let shadow = card_shadow();
    assert_eq!(
        shadow.offset,
        [raised.offset[0] as i8, raised.offset[1] as i8],
        "the card shadow offset comes from the Raised token"
    );
    assert_eq!(
        shadow.blur, raised.blur as u8,
        "the card shadow blur comes from the Raised token"
    );
    assert_eq!(
        shadow.spread, raised.spread as u8,
        "the card shadow spread comes from the Raised token"
    );
    assert_eq!(
        shadow.color, raised.umbra,
        "the card shadow umbra is the Raised token's, not a minted colour"
    );
    assert!(
        shadow.color.a() > 0 && shadow.color.a() < 255,
        "the depth is a translucent umbra (lock #2), never an opaque fill"
    );
}
