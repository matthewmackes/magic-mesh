use super::*;

/// A fake units reader that replays preset per-node mirror states.
struct FakeUnits(Vec<UnitsState>);
impl UnitsClient for FakeUnits {
    fn read(&self) -> Vec<UnitsState> {
        self.0.clone()
    }
}

impl ExplorerState {
    /// Build headless over a fake reader + a fixed hostname, folded once.
    fn with_fake(states: Vec<UnitsState>, host: &str) -> Self {
        let mut s = Self {
            client: Box::new(FakeUnits(states)),
            local_host: host.to_string(),
            units: Vec::new(),
            edges: Vec::new(),
            history: HashMap::new(),
            focus: 0,
            filter: None,
            last_poll: None,
            action_sink: Box::new(BusActions { bus_root: None }),
            arm: None,
            last_action_note: None,
            mode: SurfaceMode::default(),
            zoom_from: None,
            zoom_start: None,
            mosaic_enter: None,
            focus_rect: None,
            prefs: ExplorerPrefs::default(),
            prefs_path: None,
            last_input_at: None,
            last_advance_at: None,
            pending_focus: None,
            search: None,
            marked: Vec::new(),
            bulk_arm: None,
            bulk_rollup: None,
        };
        s.refresh();
        s
    }

    /// Build headless with a restored view record — the EXPLORER-13 restore
    /// path ([`Self::apply_restore`], the same one `Default` drives), then a
    /// first refresh so a remembered selection can land.
    fn with_prefs(states: Vec<UnitsState>, host: &str, prefs: ExplorerPrefs) -> Self {
        let mut s = Self::with_fake(states, host);
        s.apply_restore(prefs);
        s.refresh();
        s
    }
}

/// A recording action sink: captures every (topic, body) the action bar
/// dispatches so a verb's real seam is asserted headless (no Bus).
#[derive(Clone, Default)]
struct FakeActions {
    calls: std::rc::Rc<std::cell::RefCell<Vec<(String, String)>>>,
}
impl ActionSink for FakeActions {
    fn publish(&self, topic: &str, body: &str) -> Result<(), String> {
        self.calls
            .borrow_mut()
            .push((topic.to_string(), body.to_string()));
        Ok(())
    }
}

impl ExplorerState {
    /// Swap in a recording sink and return its shared log — the EXPLORER-5
    /// verb-dispatch test seam.
    fn recording(&mut self) -> FakeActions {
        let fake = FakeActions::default();
        self.action_sink = Box::new(fake.clone());
        fake
    }
}

/// The single focused unit of `s`'s current view (the hero the bar acts on).
fn focused(s: &ExplorerState) -> Unit {
    let idx = s.filtered_indices()[s.focus];
    s.units[idx].clone()
}

/// Render one headless frame of `s` and return every vertex colour the
/// tessellator actually emitted — the token-application probe (EXPLORER-15/
/// EXPLORER-18): a §4 token is *applied* iff its exact colour reaches the
/// draw list, not merely referenced in code.
fn painted_colors(s: &mut ExplorerState) -> std::collections::HashSet<[u8; 4]> {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            Vec2::new(1200.0, 800.0),
        )),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
    });
    let mut colors = std::collections::HashSet::new();
    for clipped in ctx.tessellate(out.shapes, out.pixels_per_point) {
        if let egui::epaint::Primitive::Mesh(mesh) = clipped.primitive {
            for v in &mesh.vertices {
                colors.insert([v.color.r(), v.color.g(), v.color.b(), v.color.a()]);
            }
        }
    }
    colors
}

/// Whether the painted-colour set contains exactly the §4 token `c`.
fn painted(colors: &std::collections::HashSet<[u8; 4]>, c: Color32) -> bool {
    colors.contains(&[c.r(), c.g(), c.b(), c.a()])
}

/// A reachable peer carrying live telemetry — the sparkline-path fixture.
fn peer_with_telemetry(id: &str, name: &str, t: Telemetry) -> Unit {
    Unit {
        telemetry: Some(t),
        ..unit(id, UnitKind::Peer, name, now_ms())
    }
}

fn unit(id: &str, kind: UnitKind, name: &str, last: u64) -> Unit {
    Unit {
        id: id.to_string(),
        kind,
        name: name.to_string(),
        reachability: match kind {
            UnitKind::Peer => Reachability::InMesh,
            UnitKind::LanHost => Reachability::OnLan,
            _ => Reachability::CloudObject {
                node: "node-a".to_string(),
            },
        },
        address: None,
        health: matches!(kind, UnitKind::Peer).then_some(Health::Healthy),
        telemetry: None,
        mesh: None,
        first_seen_ms: 100,
        last_seen_ms: last,
        extras: UnitExtras::default(),
    }
}

// ── a11y-05: the pickable-cell accessible name/state seam ──

#[test]
fn unit_a11y_label_is_the_display_name() {
    let u = unit("peer:node-a", UnitKind::Peer, "node-a", now_ms());
    assert_eq!(unit_a11y_label(&u), "node-a");
}

#[test]
fn unit_a11y_state_reads_kind_reachability_health_and_markers() {
    let mut peer = unit("peer:node-a", UnitKind::Peer, "node-a", now_ms());
    peer.address = Some("10.42.0.1".to_string());
    assert_eq!(
        unit_a11y_state(&peer, false, false),
        "Peer \u{00B7} In mesh \u{00B7} 10.42.0.1 \u{00B7} Healthy",
        "kind + reachability/address + health, mirroring the painted tile",
    );
    assert_eq!(
        unit_a11y_state(&peer, true, true),
        "Peer \u{00B7} In mesh \u{00B7} 10.42.0.1 \u{00B7} Healthy \u{00B7} pinned \u{00B7} marked",
        "the pin + EXPLORER-17 mark glyphs ride the value as trailing markers",
    );
    // A cloud unit with no reported health omits the tier — never a faked
    // "healthy" (§7).
    let vol = unit("vol:v1", UnitKind::Volume, "vol-1", now_ms());
    assert_eq!(
        unit_a11y_state(&vol, false, false),
        "Volume \u{00B7} Cloud object \u{00B7} node-a",
    );
}

#[test]
fn occupant_a11y_value_reads_kind_address_and_gateway() {
    let occ = IpamOccupant {
        addr: "10.42.0.1".parse().unwrap(),
        unit_id: "peer:node-a".to_string(),
        name: "node-a".to_string(),
        kind: UnitKind::Peer,
    };
    assert_eq!(occupant_a11y_value(&occ, false), "Peer \u{00B7} 10.42.0.1");
    assert_eq!(
        occupant_a11y_value(&occ, true),
        "Peer \u{00B7} 10.42.0.1 \u{00B7} gateway",
    );
}

#[test]
fn wire_mirror_decodes_a_real_aggregator_body_ignoring_daemon_only_fields() {
    // Byte-for-byte the shape `unit_aggregator::UnitsState` serialises, incl.
    // the `published_at_ms` / cloud / extras daemon-only fields the shell
    // ignores, and the typed `edges` set (EXPLORER-7) the chips now decode.
    let body = r#"{
            "host":"node-a",
            "units":[{
                "id":"peer:node-a","kind":"peer","name":"node-a",
                "reachability":{"where":"in_mesh"},
                "address":"10.42.0.1","health":"healthy",
                "mesh":{"role":"lighthouse","leader":true,"mde_version":"12.0.0"},
                "cloud":null,"first_seen_ms":1,"last_seen_ms":2,
                "extras":{"rdns":"node-a.local","oui_vendor":null,
                          "fingerprint":"ssh, vnc",
                          "extra":{"open_ports":"22,5900"}}
            }],
            "edges":[{"kind":"mesh_tunnel","from":"peer:node-a","to":"peer:node-b","detail":"direct"}],
            "published_at_ms":3
        }"#;
    let state: UnitsState = serde_json::from_str(body).expect("decodes the aggregator body");
    assert_eq!(state.host, "node-a");
    assert_eq!(state.units.len(), 1);
    let u = &state.units[0];
    assert_eq!(u.kind, UnitKind::Peer);
    assert_eq!(u.reachability, Reachability::InMesh);
    assert_eq!(u.health, Some(Health::Healthy));
    assert!(u.mesh.as_ref().is_some_and(|m| m.leader));
    // The E5 enrichment mirror decodes off the same body (EXPLORER-14).
    assert_eq!(u.extras.rdns.as_deref(), Some("node-a.local"));
    assert_eq!(u.extras.fingerprint.as_deref(), Some("ssh, vnc"));
    assert_eq!(
        u.extras.extra.get("open_ports").map(String::as_str),
        Some("22,5900")
    );
    // The edge set decodes off the same body (EXPLORER-8).
    assert_eq!(state.edges.len(), 1);
    assert_eq!(state.edges[0].kind, EdgeKind::MeshTunnel);
    assert_eq!(state.edges[0].from, "peer:node-a");
    assert_eq!(state.edges[0].to, "peer:node-b");
    assert_eq!(state.edges[0].detail.as_deref(), Some("direct"));
    // The topic prefix matches the aggregator's `state/units/<node>` shape.
    assert!(super::STATE_PREFIX.starts_with("state/units/"));
}

#[test]
fn every_edge_kind_token_matches_the_worker_wire() {
    // §6 — the shell's `EdgeKind` mirror MUST decode the worker's exact
    // `rename_all = "snake_case"` tokens; a drift here silently drops chips.
    let body = r#"[
            {"kind":"mesh_tunnel","from":"a","to":"b"},
            {"kind":"cloud_attach","from":"a","to":"b"},
            {"kind":"l2_l3_adjacency","from":"a","to":"b"},
            {"kind":"host_placement","from":"a","to":"b"},
            {"kind":"storage_usage","from":"a","to":"b"}
        ]"#;
    let edges: Vec<Edge> = serde_json::from_str(body).expect("all five kinds decode");
    assert_eq!(
        edges.iter().map(|e| e.kind).collect::<Vec<_>>(),
        vec![
            EdgeKind::MeshTunnel,
            EdgeKind::CloudAttach,
            EdgeKind::L2L3Adjacency,
            EdgeKind::HostPlacement,
            EdgeKind::StorageUsage,
        ]
    );
    // A `detail`-less edge decodes with `None` (the worker skips it when empty).
    assert!(edges[0].detail.is_none());
}

#[test]
fn fold_dedups_by_id_keeping_the_freshest_observation() {
    // The same peer appears on two nodes' mirrors; the freshest last_seen wins.
    let a = UnitsState {
        host: "node-a".into(),
        units: vec![unit("peer:x", UnitKind::Peer, "x-old", 100)],
        edges: Vec::new(),
    };
    let b = UnitsState {
        host: "node-b".into(),
        units: vec![unit("peer:x", UnitKind::Peer, "x-new", 200)],
        edges: Vec::new(),
    };
    let folded = fold_units(&[a, b], "me", &[]);
    assert_eq!(folded.len(), 1, "deduped by id");
    assert_eq!(folded[0].name, "x-new", "freshest observation kept");
}

#[test]
fn fold_orders_self_first_then_proximity_then_name() {
    let state = UnitsState {
        host: "me".into(),
        units: vec![
            unit("cloud:instance:i1", UnitKind::Instance, "web", 10),
            unit("lan:aa", UnitKind::LanHost, "printer", 10),
            unit("peer:zeta", UnitKind::Peer, "zeta", 10),
            unit("peer:me", UnitKind::Peer, "me", 10),
            unit("peer:alpha", UnitKind::Peer, "alpha", 10),
        ],
        edges: Vec::new(),
    };
    let folded = fold_units(&[state], "me", &[]);
    let ids: Vec<&str> = folded.iter().map(|u| u.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "peer:me",    // self first (#23)
            "peer:alpha", // then mesh by name
            "peer:zeta",
            "lan:aa",            // then LAN
            "cloud:instance:i1", // then cloud
        ]
    );
}

#[test]
fn category_mapping_and_counts() {
    assert_eq!(UnitKind::Peer.category(), Category::Mesh);
    assert_eq!(UnitKind::LanHost.category(), Category::Lan);
    assert_eq!(UnitKind::Volume.category(), Category::Cloud);
    let s = ExplorerState::with_fake(
        vec![UnitsState {
            host: "me".into(),
            units: vec![
                unit("peer:me", UnitKind::Peer, "me", 10),
                unit("lan:a", UnitKind::LanHost, "a", 10),
                unit("cloud:instance:i", UnitKind::Instance, "i", 10),
                unit("cloud:volume:v", UnitKind::Volume, "v", 10),
            ],
            edges: Vec::new(),
        }],
        "me",
    );
    assert_eq!(s.category_counts(), [1, 1, 2]); // 1 mesh, 1 lan, 2 cloud
}

// ─────────────── EXPLORER-15 per-category visual identity ───────────────

#[test]
fn category_identity_maps_onto_the_shared_style_tokens() {
    // O8 — each category's accent IS a shared `Style` categorical token
    // (defined ONCE for picker + explorer, PICKER-2): no duplicate minted,
    // no raw hex in this crate.
    assert_eq!(Category::Mesh.accent(), Style::ACCENT_MESH);
    assert_eq!(Category::Lan.accent(), Style::ACCENT_TERMINALS);
    assert_eq!(Category::Cloud.accent(), Style::ACCENT_WORKLOADS);
    // The three identities are mutually distinct AND distinct from the one
    // interactive brand accent, so a category tint never reads as an
    // interaction affordance.
    let cats: Vec<Color32> = Category::ALL.iter().map(|c| c.accent()).collect();
    for (i, a) in cats.iter().enumerate() {
        assert_ne!(*a, Style::ACCENT, "category ≠ brand accent");
        for b in &cats[i + 1..] {
            assert_ne!(a, b, "category accents must be mutually distinct");
        }
    }
    // Every unit kind resolves into exactly one of the three identities —
    // the glyph family ([`paint_kind_glyph`]) rides the same mapping, so no
    // kind can render outside the category colour language.
    for kind in [
        UnitKind::Peer,
        UnitKind::LanHost,
        UnitKind::Instance,
        UnitKind::Volume,
        UnitKind::Image,
        UnitKind::Network,
    ] {
        assert!(Category::ALL.contains(&kind.category()));
    }
}

#[test]
fn category_accents_actually_paint_the_tiles_chips_and_rings() {
    // Token *application*, not just mapping (the EXPLORER-15 acceptance):
    // with one unit per category on the shelf, all three category accents
    // must reach the draw list — the mosaic tiles' glyphs + badges + the
    // rest-tinted filter chips on the landing, and the hero mode's status
    // ring arc/glyph + filmstrip dividers.
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    let mosaic = painted_colors(&mut s);
    for cat in Category::ALL {
        assert!(
            painted(&mosaic, cat.accent()),
            "{} accent must be painted on the mosaic landing",
            cat.label()
        );
    }
    s.set_mode(SurfaceMode::Hero);
    let hero = painted_colors(&mut s);
    for cat in Category::ALL {
        assert!(
            painted(&hero, cat.accent()),
            "{} accent must be painted in the hero mode",
            cat.label()
        );
    }
}

#[test]
fn empty_shows_the_self_placeholder_page_hash_23() {
    let s = ExplorerState::with_fake(vec![], "anvil");
    // No mirror yet → exactly one hero page (this node), never zero/blank.
    assert_eq!(s.hero_count(), 1);
    let placeholder = self_placeholder(&s.local_host);
    assert_eq!(placeholder.id, "peer:anvil");
    assert_eq!(placeholder.name, "anvil");
    assert!(placeholder.health.is_none(), "self stays honestly unprobed");
}

#[test]
fn filter_scopes_the_view_and_reanchors_focus() {
    let mut s = ExplorerState::with_fake(
        vec![UnitsState {
            host: "me".into(),
            units: vec![
                unit("peer:me", UnitKind::Peer, "me", 10),
                unit("lan:a", UnitKind::LanHost, "a", 10),
                unit("cloud:instance:i", UnitKind::Instance, "i", 10),
            ],
            edges: Vec::new(),
        }],
        "me",
    );
    s.focus = 2;
    s.set_filter(Some(Category::Lan));
    assert_eq!(s.filtered_indices().len(), 1);
    assert_eq!(s.focus, 0, "focus re-anchors to the front of the new view");
    // A filter with no matches yields zero pages (honest empty, not the self card).
    s.set_filter(Some(Category::Cloud));
    assert_eq!(s.hero_count(), 1); // one instance
    let empty = ExplorerState::with_fake(vec![], "me");
    let mut empty = empty;
    empty.set_filter(Some(Category::Cloud));
    assert_eq!(empty.hero_count(), 0);
}

#[test]
fn paging_clamps_at_both_ends() {
    let mut s = ExplorerState::with_fake(
        vec![UnitsState {
            host: "me".into(),
            units: vec![
                unit("peer:me", UnitKind::Peer, "me", 10),
                unit("lan:a", UnitKind::LanHost, "a", 10),
                unit("cloud:instance:i", UnitKind::Instance, "i", 10),
            ],
            edges: Vec::new(),
        }],
        "me",
    );
    assert_eq!(s.hero_count(), 3);
    s.page_prev();
    assert_eq!(s.focus, 0, "clamps at the start");
    s.page_next();
    s.page_next();
    s.page_next(); // past the end
    assert_eq!(s.focus, 2, "clamps at the end");
}

#[test]
fn reachability_line_is_honest_per_kind() {
    assert_eq!(
        reachability_line(&Reachability::InMesh, Some("10.42.0.1")),
        "In mesh · 10.42.0.1"
    );
    assert_eq!(reachability_line(&Reachability::OnLan, None), "On LAN");
    assert_eq!(
        reachability_line(
            &Reachability::CloudObject {
                node: "bigboy".into()
            },
            None
        ),
        "Cloud object · bigboy"
    );
}

#[test]
fn rich_vs_dimmed_classification() {
    // A live in-mesh peer → rich; an off-mesh LAN host → dimmed-minimal (#12).
    assert!(hero_is_rich(&unit("peer:x", UnitKind::Peer, "x", 10)));
    assert!(!hero_is_rich(&unit("lan:a", UnitKind::LanHost, "a", 10)));
    assert!(hero_is_rich(&unit(
        "cloud:instance:i",
        UnitKind::Instance,
        "i",
        10
    )));
    // A volume/image/network is a summary card, not rich telemetry.
    assert!(!hero_is_rich(&unit(
        "cloud:volume:v",
        UnitKind::Volume,
        "v",
        10
    )));
}

#[test]
fn fmt_duration_reads_compactly() {
    assert_eq!(fmt_duration(30), "30s");
    assert_eq!(fmt_duration(90), "1m");
    assert_eq!(fmt_duration(3_720), "1h 2m");
    assert_eq!(fmt_duration(90_000), "1d 1h");
}

#[test]
fn hero_card_renders_headless_across_states() {
    // Exercise the real render (glyphs, ring, telemetry, dimmed, empty) so a
    // panic in any painter path is caught headless — no GPU, like backdrop's.
    let states = vec![UnitsState {
        host: "me".into(),
        units: vec![
            Unit {
                mesh: Some(MeshFacts {
                    role: Some("lighthouse".into()),
                    leader: true,
                    mde_version: Some("12.0.0".into()),
                }),
                telemetry: Some(Telemetry {
                    load1: Some(0.42),
                    mem_used_pct: Some(37.0),
                    uptime_s: Some(90_061),
                }),
                ..unit("peer:me", UnitKind::Peer, "me", now_ms())
            },
            unit("lan:aa", UnitKind::LanHost, "printer", now_ms()),
            unit("cloud:instance:i1", UnitKind::Instance, "web", now_ms()),
        ],
        edges: Vec::new(),
    }];

    for filter in [
        None,
        Some(Category::Mesh),
        Some(Category::Lan),
        Some(Category::Cloud),
    ] {
        let mut s = ExplorerState::with_fake(states.clone(), "me");
        s.mode = SurfaceMode::Hero; // exercise the hero-card path (mosaic lands)
        s.set_filter(filter);
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                Vec2::new(1200.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "the hero surface drew primitives");
    }

    // And the honest empty (#23) self card renders too.
    let mut empty = ExplorerState::with_fake(vec![], "solo");
    empty.mode = SurfaceMode::Hero;
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            Vec2::new(1000.0, 700.0),
        )),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| empty.show(ui));
    });
    assert!(!ctx.tessellate(out.shapes, out.pixels_per_point).is_empty());
}

#[test]
fn telemetry_history_accumulates_bounded_real_samples() {
    // A reachable peer that reports load/mem: repeated polls build a REAL
    // observed series (every point a value we read, never synthesised, §7),
    // ring-bounded to the history cap.
    let peer = peer_with_telemetry(
        "peer:me",
        "me",
        Telemetry {
            load1: Some(0.5),
            mem_used_pct: Some(40.0),
            uptime_s: Some(120),
        },
    );
    let states = vec![UnitsState {
        host: "me".into(),
        units: vec![peer],
        edges: Vec::new(),
    }];
    let mut s = ExplorerState::with_fake(states, "me"); // one refresh already
    for _ in 0..(HISTORY_LEN + 5) {
        s.refresh();
    }
    let h = s.history.get("peer:me").expect("peer accrued history");
    assert_eq!(h.load1.len(), HISTORY_LEN, "series ring-bounded to the cap");
    assert_eq!(h.mem_used_pct.len(), HISTORY_LEN);
    assert!(
        h.load1.iter().all(|&v| (v - 0.5).abs() < f32::EPSILON),
        "each point is the real observed value, not a faked curve"
    );
}

#[test]
fn history_prunes_departed_units() {
    // A unit that leaves the shelf drops its stale history — no ghost curve.
    let present = vec![UnitsState {
        host: "me".into(),
        units: vec![peer_with_telemetry(
            "peer:gone",
            "gone",
            Telemetry {
                load1: Some(1.0),
                ..Default::default()
            },
        )],
        edges: Vec::new(),
    }];
    let mut s = ExplorerState::with_fake(present, "me");
    assert!(s.history.contains_key("peer:gone"));
    // The next read returns an empty shelf → the unit departs.
    s.client = Box::new(FakeUnits(vec![]));
    s.refresh();
    assert!(
        !s.history.contains_key("peer:gone"),
        "stale history pruned when the unit leaves"
    );
}

#[test]
fn a_unit_without_a_series_metric_records_no_history() {
    // Telemetry with only a scalar counter (uptime) and no load/mem must NOT
    // start a trend — the sparkline source stays honestly empty (§7).
    let peer = peer_with_telemetry(
        "peer:me",
        "me",
        Telemetry {
            load1: None,
            mem_used_pct: None,
            uptime_s: Some(999),
        },
    );
    let s = ExplorerState::with_fake(
        vec![UnitsState {
            host: "me".into(),
            units: vec![peer],
            edges: Vec::new(),
        }],
        "me",
    );
    assert!(
        !s.history.contains_key("peer:me"),
        "no load/mem → no sparkline history minted"
    );
}

#[test]
fn hero_card_renders_sparklines_when_reachable_else_dimmed() {
    // A reachable peer with telemetry, polled enough to fill a real sparkline,
    // renders the rich metric grid; an off-mesh LAN host renders the
    // dimmed-minimal card with no telemetry grid (#11/#12).
    let peer = Unit {
        mesh: Some(MeshFacts {
            role: Some("workstation".into()),
            leader: false,
            mde_version: Some("12.0.0".into()),
        }),
        ..peer_with_telemetry(
            "peer:me",
            "me",
            Telemetry {
                load1: Some(0.8),
                mem_used_pct: Some(55.0),
                uptime_s: Some(90_061),
            },
        )
    };
    let states = vec![UnitsState {
        host: "me".into(),
        units: vec![peer, unit("lan:aa", UnitKind::LanHost, "printer", now_ms())],
        edges: Vec::new(),
    }];
    let mut s = ExplorerState::with_fake(states, "me");
    s.mode = SurfaceMode::Hero; // the hero-card path (mosaic is the landing)
    for _ in 0..4 {
        s.refresh(); // ≥2 samples → a drawable sparkline
    }
    assert!(
        s.history
            .get("peer:me")
            .is_some_and(|h| h.load1.len() >= 2 && h.mem_used_pct.len() >= 2),
        "the sparkline has ≥2 real points to draw"
    );

    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            Vec2::new(1200.0, 800.0),
        )),
        ..Default::default()
    };
    // Reachable peer focused → the sparkline / metric-grid path.
    s.focus = 0;
    let out = ctx.run(input.clone(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
    });
    assert!(
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty(),
        "the rich sparkline card drew primitives"
    );
    // Dimmed LAN host focused → the dimmed-minimal path (no metric grid).
    s.focus = 1;
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
    });
    assert!(
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty(),
        "the dimmed-minimal card drew primitives"
    );
}

// ─────────────────────── EXPLORER-5 action bars ───────────────────────

/// A cloud instance with a known Nova id + address (the console-enabled path).
fn instance_unit(id: &str, name: &str) -> Unit {
    Unit {
        address: Some("10.0.0.5".to_string()),
        ..unit(id, UnitKind::Instance, name, now_ms())
    }
}

#[test]
fn topic_and_id_mirrors_match_the_worker_contract() {
    // §6 — the shell's local mirrors must equal the openstack worker's wire.
    assert_eq!(CLOUD_ACTION_PREFIX, "action/cloud/");
    assert_eq!(UNITS_REQUEST_TOPIC, "action/units/get-stream");
    assert_eq!(cloud_topic("instance-stop"), "action/cloud/instance-stop");
    // The Nova id is the aggregator's `cloud:<kind>:<object-id>` tail.
    assert_eq!(
        cloud_object_id(&unit("cloud:instance:uuid-1", UnitKind::Instance, "web", 1)),
        "uuid-1"
    );
    // An object id that itself contains a colon keeps its remainder.
    assert_eq!(
        cloud_object_id(&unit("cloud:volume:pool:vol-9", UnitKind::Volume, "v", 1)),
        "pool:vol-9"
    );
}

#[test]
fn instance_lifecycle_verbs_dispatch_over_the_cloud_bus() {
    let mut s = ExplorerState::with_fake(
        vec![UnitsState {
            host: "me".into(),
            units: vec![instance_unit("cloud:instance:i-9", "web")],
            edges: Vec::new(),
        }],
        "me",
    );
    let fake = s.recording();
    let u = instance_unit("cloud:instance:i-9", "web");

    // Start is non-destructive → fires immediately.
    s.fire(Verb::Start, &u);
    assert_eq!(
        fake.calls.borrow().as_slice(),
        &[(
            "action/cloud/instance-start".to_string(),
            r#"{"instance":"i-9"}"#.to_string()
        )],
        "Start publishes the QC-11 InstanceReq on action/cloud/instance-start"
    );

    // The three destructive verbs each publish their own topic once armed.
    for (verb, topic) in [
        (Verb::Stop, "action/cloud/instance-stop"),
        (Verb::Reboot, "action/cloud/instance-reboot"),
        (Verb::Delete, "action/cloud/instance-delete"),
    ] {
        fake.calls.borrow_mut().clear();
        s.arm_verb(verb, &u.id);
        s.arm.as_mut().expect("armed").echo = "web".to_string();
        assert!(s.confirm_armed(&u), "the typed-name confirm fires the verb");
        assert_eq!(
            fake.calls.borrow().as_slice(),
            &[(topic.to_string(), r#"{"instance":"i-9"}"#.to_string())],
            "{verb:?} publishes on {topic}"
        );
        assert!(s.arm.is_none(), "arming clears after the confirm");
    }
}

#[test]
fn arming_gates_the_destructive_verbs() {
    let mut s = ExplorerState::with_fake(
        vec![UnitsState {
            host: "me".into(),
            units: vec![instance_unit("cloud:instance:i-9", "web")],
            edges: Vec::new(),
        }],
        "me",
    );
    let fake = s.recording();
    let u = instance_unit("cloud:instance:i-9", "web");

    // Arm Delete but leave the echo blank / wrong → nothing dispatches.
    s.arm_verb(Verb::Delete, &u.id);
    assert!(
        !s.confirm_armed(&u),
        "an un-echoed destructive verb is a no-op"
    );
    s.arm.as_mut().expect("armed").echo = "wrong".to_string();
    assert!(!s.confirm_armed(&u), "a mismatched echo never fires");
    assert!(
        fake.calls.borrow().is_empty(),
        "a destructive verb publishes NOTHING until armed + echoed"
    );

    // The exact name arms it.
    s.arm.as_mut().expect("armed").echo = "web".to_string();
    assert!(s.arm_ready("web"));
    assert!(s.confirm_armed(&u));
    assert_eq!(
        fake.calls.borrow().len(),
        1,
        "now it dispatches exactly once"
    );
}

#[test]
fn peer_verbs_reach_the_fleet_and_the_live_stream() {
    let mut s = ExplorerState::with_fake(
        vec![UnitsState {
            host: "me".into(),
            units: vec![unit("peer:zeta", UnitKind::Peer, "zeta", now_ms())],
            edges: Vec::new(),
        }],
        "me",
    );
    let fake = s.recording();
    let peer = unit("peer:zeta", UnitKind::Peer, "zeta", now_ms());

    // Open in Fleet → a nav chyron carrying shell/goto/mesh.
    s.fire(Verb::OpenInFleet, &peer);
    {
        let calls = fake.calls.borrow();
        assert_eq!(calls[0].0, TOAST_TOPIC);
        assert!(
            calls[0].1.contains("shell/goto/mesh"),
            "open-in-Fleet routes to the mesh view: {}",
            calls[0].1
        );
    }

    // Health-check → the aggregator's get-stream refresh.
    fake.calls.borrow_mut().clear();
    s.fire(Verb::HealthCheck, &peer);
    assert_eq!(fake.calls.borrow()[0].0, "action/units/get-stream");

    // Evict has no bus verb → honestly disabled, never a dispatch.
    assert!(verb_seam(Verb::Evict, &peer).is_err());
}

#[test]
fn lan_invite_is_armed_and_routes_to_provisioning() {
    let mut s = ExplorerState::with_fake(
        vec![UnitsState {
            host: "me".into(),
            units: vec![unit("lan:printer", UnitKind::LanHost, "printer", now_ms())],
            edges: Vec::new(),
        }],
        "me",
    );
    let fake = s.recording();
    let host = unit("lan:printer", UnitKind::LanHost, "printer", now_ms());

    // Invite is destructive (trust change) → gated on the typed name.
    s.arm_verb(Verb::Invite, &host.id);
    assert!(!s.confirm_armed(&host), "invite is a no-op until echoed");
    assert!(fake.calls.borrow().is_empty());
    s.arm.as_mut().expect("armed").echo = "printer".to_string();
    assert!(s.confirm_armed(&host));
    let calls = fake.calls.borrow();
    assert_eq!(calls[0].0, TOAST_TOPIC);
    assert!(
        calls[0].1.contains("shell/plane/provisioning"),
        "invite kicks the Provisioning pairing flow: {}",
        calls[0].1
    );
}

#[test]
fn verbs_without_a_seam_are_honestly_disabled() {
    // Console with no address, object delete, and evict all resolve to a
    // reason, never a live no-op button (§7).
    let bare_instance = unit("cloud:instance:i", UnitKind::Instance, "web", 1);
    assert!(verb_seam(Verb::Console, &bare_instance).is_err());
    assert!(verb_seam(Verb::Console, &instance_unit("cloud:instance:i", "web")).is_ok());
    assert!(verb_seam(
        Verb::ObjectDelete,
        &unit("cloud:volume:v", UnitKind::Volume, "vol", 1)
    )
    .is_err());
    assert!(verb_seam(Verb::Evict, &unit("peer:x", UnitKind::Peer, "x", 1)).is_err());
    // Inspect routes to the Cloud surface (a real hand-off).
    assert!(verb_seam(
        Verb::Inspect,
        &unit("cloud:network:n", UnitKind::Network, "net", 1)
    )
    .is_ok());
}

#[test]
fn each_kind_offers_its_own_verbs() {
    assert_eq!(verbs_for(UnitKind::Instance).len(), 5);
    assert_eq!(
        verbs_for(UnitKind::Volume),
        [Verb::Inspect, Verb::ObjectDelete].as_slice()
    );
    assert_eq!(
        verbs_for(UnitKind::Peer),
        [Verb::OpenInFleet, Verb::HealthCheck, Verb::Evict].as_slice()
    );
    assert_eq!(
        verbs_for(UnitKind::LanHost),
        [Verb::Invite, Verb::HealthCheck].as_slice()
    );
}

#[test]
fn the_armed_action_bar_renders_headless() {
    // The hero + action bar + typed-arming challenge all tessellate cleanly.
    let mut s = ExplorerState::with_fake(
        vec![UnitsState {
            host: "me".into(),
            units: vec![instance_unit("cloud:instance:i-9", "web")],
            edges: Vec::new(),
        }],
        "me",
    );
    s.mode = SurfaceMode::Hero; // the action bar lives on the hero card
    let u = focused(&s);
    s.arm_verb(Verb::Delete, &u.id); // show the challenge row
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            Vec2::new(1200.0, 800.0),
        )),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
    });
    assert!(!ctx.tessellate(out.shapes, out.pixels_per_point).is_empty());
}

// ─────────────────────── EXPLORER-8 edge chips ───────────────────────

/// A typed edge between two ids (test fixtures build them directly, no wire).
fn edge(kind: EdgeKind, from: &str, to: &str) -> Edge {
    Edge {
        kind,
        from: from.to_string(),
        to: to.to_string(),
        detail: None,
    }
}

/// A connectivity fixture: self + a peer, an instance wired to a network +
/// volume, running on the peer, the volume attached (+ backed by a non-unit
/// pool). One state so `fold_edges` + `grouped_edges` see the whole graph.
fn connected_state() -> Vec<UnitsState> {
    vec![UnitsState {
        host: "me".into(),
        units: vec![
            unit("peer:me", UnitKind::Peer, "me", 10),
            unit("peer:anvil", UnitKind::Peer, "anvil", 10),
            unit("cloud:instance:i1", UnitKind::Instance, "web", 10),
            unit("cloud:network:n1", UnitKind::Network, "tenant", 10),
            unit("cloud:volume:v1", UnitKind::Volume, "data", 10),
        ],
        edges: vec![
            edge(EdgeKind::MeshTunnel, "peer:me", "peer:anvil"),
            edge(
                EdgeKind::CloudAttach,
                "cloud:instance:i1",
                "cloud:network:n1",
            ),
            edge(
                EdgeKind::CloudAttach,
                "cloud:instance:i1",
                "cloud:volume:v1",
            ),
            edge(EdgeKind::HostPlacement, "cloud:instance:i1", "peer:anvil"),
            edge(
                EdgeKind::StorageUsage,
                "cloud:volume:v1",
                "cloud:instance:i1",
            ),
            // Backing pool: a non-unit endpoint — never a jump chip (§7).
            edge(EdgeKind::StorageUsage, "cloud:volume:v1", "pool:ceph"),
        ],
    }]
}

/// Focus the hero on the unit `id` (its position in the current filtered view).
fn focus_on(s: &mut ExplorerState, id: &str) {
    let abs = s
        .units
        .iter()
        .position(|u| u.id == id)
        .expect("unit present");
    s.focus = s
        .filtered_indices()
        .iter()
        .position(|&i| i == abs)
        .expect("unit is in the active view");
}

#[test]
fn edges_fold_and_dedup_across_node_mirrors() {
    // Two nodes republish the same derived edge — the union keeps one.
    let states = vec![
        UnitsState {
            host: "a".into(),
            units: vec![],
            edges: vec![edge(EdgeKind::MeshTunnel, "peer:a", "peer:b")],
        },
        UnitsState {
            host: "b".into(),
            units: vec![],
            edges: vec![
                edge(EdgeKind::MeshTunnel, "peer:a", "peer:b"), // dup
                edge(EdgeKind::MeshTunnel, "peer:b", "peer:c"), // new
            ],
        },
    ];
    assert_eq!(
        fold_edges(&states).len(),
        2,
        "cross-node duplicate collapses"
    );
}

#[test]
fn edge_chips_group_by_kind_and_omit_absent_sections() {
    let s = ExplorerState::with_fake(connected_state(), "me");
    let instance = s
        .units
        .iter()
        .find(|u| u.id == "cloud:instance:i1")
        .cloned()
        .expect("instance folded");

    let sections = s.grouped_edges(&instance);
    // Design order: Networks, Volumes, Runs on <node>, Storage. Tunnels + Same
    // subnet are absent from an instance's view → no empty headers (§7).
    let headers: Vec<&str> = sections.iter().map(|sec| sec.header.as_str()).collect();
    assert_eq!(
        headers,
        vec!["Networks", "Volumes", "Runs on anvil", "Storage"]
    );
    // Each chip is the related unit (name + kind), jumpable.
    let chip_of = |header: &str| -> Vec<&str> {
        sections
            .iter()
            .find(|sec| sec.header == header)
            .map(|sec| sec.chips.iter().map(|c| c.name.as_str()).collect())
            .unwrap_or_default()
    };
    assert_eq!(chip_of("Networks"), vec!["tenant"]);
    assert_eq!(chip_of("Volumes"), vec!["data"]);
    assert_eq!(chip_of("Runs on anvil"), vec!["anvil"]);
    // Storage shows the attached volume; the non-unit backing pool is skipped.
    assert_eq!(chip_of("Storage"), vec!["data"]);
    assert!(
        sections
            .iter()
            .all(|sec| sec.chips.iter().all(|c| c.id != "pool:ceph")),
        "a non-unit pool endpoint never becomes a chip"
    );

    // A peer's view has only the mesh tunnel — every cloud section is absent.
    let me = s.units.iter().find(|u| u.id == "peer:me").cloned().unwrap();
    let peer_sections = s.grouped_edges(&me);
    assert_eq!(
        peer_sections
            .iter()
            .map(|x| x.header.as_str())
            .collect::<Vec<_>>(),
        vec!["Tunnels"]
    );
    assert_eq!(peer_sections[0].chips[0].id, "peer:anvil");
}

#[test]
fn a_unit_with_only_a_non_unit_endpoint_shows_no_section() {
    // A volume backed solely by a pool (no attachment, no unit neighbour) has
    // nothing jumpable → the whole edge region is empty, not an empty header.
    let s = ExplorerState::with_fake(
        vec![UnitsState {
            host: "me".into(),
            units: vec![unit("cloud:volume:v9", UnitKind::Volume, "lonely", 10)],
            edges: vec![edge(EdgeKind::StorageUsage, "cloud:volume:v9", "pool:ceph")],
        }],
        "me",
    );
    let vol = s
        .units
        .iter()
        .find(|u| u.id == "cloud:volume:v9")
        .cloned()
        .unwrap();
    assert!(s.grouped_edges(&vol).is_empty());
}

#[test]
fn a_chip_click_jumps_the_hero_focus_to_the_neighbour() {
    let mut s = ExplorerState::with_fake(connected_state(), "me");
    focus_on(&mut s, "cloud:instance:i1");
    // The Networks chip points at the tenant network.
    let sections = s.grouped_edges(&focused(&s));
    let net_chip = sections
        .iter()
        .find(|sec| sec.header == "Networks")
        .and_then(|sec| sec.chips.first())
        .expect("a network chip")
        .clone();
    assert_eq!(net_chip.id, "cloud:network:n1");

    // Clicking it (the jump path) moves the hero focus to that neighbour.
    s.jump_to_id(&net_chip.id);
    assert_eq!(focused(&s).id, "cloud:network:n1");
}

#[test]
fn a_jump_to_a_filtered_out_neighbour_clears_the_filter() {
    // Focused on a cloud instance under the Cloud filter, jumping to its host
    // peer (a Mesh unit hidden by the filter) clears the filter so the jump
    // always lands — reusing the one focus-set path.
    let mut s = ExplorerState::with_fake(connected_state(), "me");
    s.set_filter(Some(Category::Cloud));
    focus_on(&mut s, "cloud:instance:i1");
    s.jump_to_id("peer:anvil");
    assert_eq!(
        s.filter, None,
        "the hiding filter clears on a cross-filter jump"
    );
    assert_eq!(focused(&s).id, "peer:anvil");
}

#[test]
fn the_edge_chip_region_renders_headless() {
    // The grouped chips tessellate cleanly under the hero card.
    let mut s = ExplorerState::with_fake(connected_state(), "me");
    s.mode = SurfaceMode::Hero; // the edge chips ride the hero card
    focus_on(&mut s, "cloud:instance:i1");
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            Vec2::new(1200.0, 900.0),
        )),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
    });
    assert!(!ctx.tessellate(out.shapes, out.pixels_per_point).is_empty());
}

// ─────────────────────── EXPLORER-10 IPAM table ───────────────────────

/// A unit that reported an address — the IPAM-occupant fixture.
fn addr_unit(id: &str, kind: UnitKind, name: &str, addr: &str) -> Unit {
    Unit {
        address: Some(addr.to_string()),
        ..unit(id, kind, name, now_ms())
    }
}

/// A live-discovered shelf spanning a mesh /24, a LAN /24, and a cloud tenant
/// /24 (named by a `CloudAttach` edge), plus an address-less network + volume
/// that must never become occupants.
fn addressed_state() -> Vec<UnitsState> {
    vec![UnitsState {
        host: "me".into(),
        units: vec![
            addr_unit("peer:me", UnitKind::Peer, "me", "10.42.0.1"),
            addr_unit("peer:anvil", UnitKind::Peer, "anvil", "10.42.0.7"),
            addr_unit("lan:printer", UnitKind::LanHost, "printer", "172.20.0.50"),
            addr_unit("cloud:instance:i1", UnitKind::Instance, "web", "10.0.0.5"),
            addr_unit("cloud:instance:i2", UnitKind::Instance, "db", "10.0.0.9"),
            unit("cloud:network:n1", UnitKind::Network, "tenant", 10),
            unit("cloud:volume:v1", UnitKind::Volume, "data", 10),
        ],
        edges: vec![
            edge(
                EdgeKind::CloudAttach,
                "cloud:instance:i1",
                "cloud:network:n1",
            ),
            edge(
                EdgeKind::CloudAttach,
                "cloud:instance:i2",
                "cloud:network:n1",
            ),
        ],
    }]
}

#[test]
fn ipam_aggregates_addresses_into_slash24_prefixes() {
    let s = ExplorerState::with_fake(addressed_state(), "me");
    let prefixes = s.ipam_prefixes();
    // Three /24s, proximity-ordered (mesh → LAN → cloud) then by network.
    let cidrs: Vec<String> = prefixes.iter().map(IpamPrefix::cidr).collect();
    assert_eq!(cidrs, vec!["10.42.0.0/24", "172.20.0.0/24", "10.0.0.0/24"]);

    // The mesh prefix: two peers sorted by address, gateway is the .1 host.
    let mesh = &prefixes[0];
    assert_eq!(mesh.category, Category::Mesh);
    assert_eq!(mesh.gateway(), "10.42.0.1".parse::<Ipv4Addr>().unwrap());
    let names: Vec<&str> = mesh.occupants.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["me", "anvil"],
        "occupants sorted by address (.1, .7)"
    );

    // The cloud prefix reads as Cloud; the address-less volume/network are
    // never phantom occupants (§7).
    let cloud = &prefixes[2];
    assert_eq!(cloud.category, Category::Cloud);
    assert_eq!(cloud.occupants.len(), 2);
    assert!(cloud
        .occupants
        .iter()
        .all(|o| o.unit_id != "cloud:volume:v1" && o.unit_id != "cloud:network:n1"));
}

#[test]
fn ipam_occupancy_counts_used_and_free_over_the_slash24() {
    let s = ExplorerState::with_fake(addressed_state(), "me");
    let prefixes = s.ipam_prefixes();
    let mesh = &prefixes[0];
    assert_eq!(mesh.used(), 2);
    assert_eq!(mesh.free(), IPAM_USABLE_PER_24 - 2);
    let lan = &prefixes[1];
    assert_eq!(lan.used(), 1);
    assert_eq!(lan.free(), 253);
}

#[test]
fn ipam_labels_a_tenant_prefix_from_a_cloud_attach_edge() {
    let s = ExplorerState::with_fake(addressed_state(), "me");
    let prefixes = s.ipam_prefixes();
    let cloud = prefixes
        .iter()
        .find(|p| p.category == Category::Cloud)
        .expect("a cloud prefix");
    assert_eq!(
        cloud.label.as_deref(),
        Some("tenant"),
        "the tenant net names its prefix via the CloudAttach edge (EXPLORER-7)"
    );
    // Mesh/LAN prefixes have no network object → no fabricated label (§7).
    assert!(prefixes[0].label.is_none());
    assert!(prefixes[1].label.is_none());
}

#[test]
fn ipam_ignores_absent_and_unparseable_addresses() {
    // Parse tolerances: a CIDR mask + a :port tail both resolve; junk doesn't.
    assert_eq!(parse_ipv4("10.0.0.5/24"), "10.0.0.5".parse().ok());
    assert_eq!(parse_ipv4("10.0.0.5:5900"), "10.0.0.5".parse().ok());
    assert!(parse_ipv4("not-an-ip").is_none());
    assert!(
        parse_ipv4("fe80::1").is_none(),
        "IPv6 isn't a /24 occupant here"
    );

    // A unit with no address, and an IPv6 unit, yield no phantom prefixes.
    let units = vec![
        unit("peer:me", UnitKind::Peer, "me", 10),
        addr_unit("peer:v6", UnitKind::Peer, "v6", "fe80::1"),
        addr_unit("peer:ok", UnitKind::Peer, "ok", "10.42.0.3"),
    ];
    let prefixes = derive_prefixes(&units, &[]);
    assert_eq!(prefixes.len(), 1, "only the IPv4 unit anchors a prefix");
    assert_eq!(prefixes[0].occupants.len(), 1);
    // A wholly empty shelf → no prefixes at all (honest-empty, §7).
    assert!(derive_prefixes(&[], &[]).is_empty());
}

#[test]
fn ipam_filter_scopes_prefixes_by_category() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    s.set_filter(Some(Category::Cloud));
    let cloud = s.ipam_prefixes();
    assert_eq!(cloud.len(), 1);
    assert_eq!(cloud[0].category, Category::Cloud);
    s.set_filter(Some(Category::Lan));
    assert_eq!(s.ipam_prefixes().len(), 1);
    s.set_filter(None);
    assert_eq!(s.ipam_prefixes().len(), 3);
}

#[test]
fn ipam_row_click_jumps_to_the_occupant_hero() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    s.mode = SurfaceMode::Ipam;
    // A row click returns to the hero card, focused on the occupant.
    s.jump_from_ipam("lan:printer");
    assert_eq!(s.mode, SurfaceMode::Hero);
    assert_eq!(focused(&s).id, "lan:printer");

    // A jump from under a hiding category filter clears it so the jump lands.
    s.mode = SurfaceMode::Ipam;
    s.set_filter(Some(Category::Cloud));
    s.jump_from_ipam("peer:me");
    assert_eq!(s.mode, SurfaceMode::Hero);
    assert_eq!(s.filter, None, "the hiding filter clears on the jump");
    assert_eq!(focused(&s).id, "peer:me");
}

#[test]
fn ipam_table_renders_headless_and_when_empty() {
    let render = |s: &mut ExplorerState| {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                Vec2::new(1200.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    };
    // The populated IPAM table draws its prefix bands + address rows.
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    s.mode = SurfaceMode::Ipam;
    assert!(render(&mut s), "the IPAM table drew primitives");
    // Honest-empty (no addressed units) still draws the note, never panics.
    let mut empty = ExplorerState::with_fake(vec![], "solo");
    empty.mode = SurfaceMode::Ipam;
    assert!(render(&mut empty));
}

// ─────────────────────── EXPLORER-11 mosaic overview ───────────────────────

#[test]
fn mosaic_is_the_landing_mode() {
    // The surface lands on the whole-fleet mosaic (O1), not the hero card.
    assert_eq!(SurfaceMode::default(), SurfaceMode::Mosaic);
    let s = ExplorerState::with_fake(addressed_state(), "me");
    assert_eq!(s.mode, SurfaceMode::Mosaic);
}

#[test]
fn mode_toggles_switch_between_all_three() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    assert_eq!(s.mode, SurfaceMode::Mosaic);
    s.set_mode(SurfaceMode::Hero);
    assert_eq!(s.mode, SurfaceMode::Hero);
    s.set_mode(SurfaceMode::Ipam);
    assert_eq!(s.mode, SurfaceMode::Ipam);
    s.set_mode(SurfaceMode::Mosaic);
    assert_eq!(s.mode, SurfaceMode::Mosaic);
    // Landing back on the mosaic seeds the O3 settle fade.
    assert!(s.mosaic_enter.is_some());
    // A no-op toggle to the current mode is inert.
    s.mosaic_enter = None;
    s.set_mode(SurfaceMode::Mosaic);
    assert!(
        s.mosaic_enter.is_none(),
        "re-selecting the same mode is a no-op"
    );
}

// ─────────────────── EXPLORER-12 ambient idle auto-cycle ───────────────────

/// A unique per-test temp dir (the manual `power_honor` idiom — no tempfile dep
/// on the airgapped farm).
fn ambient_temp_dir(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "mde-explorer-prefs-{tag}-{}-{n}",
        std::process::id()
    ))
}

/// Run one headless Explorer frame at `time` seconds carrying `events`.
fn ambient_frame(ctx: &egui::Context, s: &mut ExplorerState, time: f64, events: Vec<egui::Event>) {
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            Vec2::new(1200.0, 800.0),
        )),
        time: Some(time),
        events,
        ..Default::default()
    };
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
    });
}

#[test]
fn ambient_toggle_defaults_off_and_round_trips_through_disk() {
    // OFF by default — an unattended screen never moves unless opted in (§7).
    assert!(!ExplorerPrefs::default().ambient_idle);
    assert!(
        !ExplorerState::with_fake(addressed_state(), "me")
            .prefs
            .ambient_idle,
        "a fresh surface loads the OFF default"
    );

    // The SETTINGS-nav persistence idiom: a missing file folds to the default,
    // and a flipped toggle survives a restart (write → read back).
    let dir = ambient_temp_dir("rt");
    std::fs::create_dir_all(&dir).expect("mkroot");
    let path = dir.join(PREFS_FILE);
    assert_eq!(
        ExplorerPrefs::load_from(&path),
        ExplorerPrefs::default(),
        "a missing prefs file folds to the OFF default"
    );

    let on = ExplorerPrefs {
        ambient_idle: true,
        ..Default::default()
    };
    on.save_to(&path).expect("save");
    assert!(
        ExplorerPrefs::load_from(&path).ambient_idle,
        "the enabled toggle round-trips through disk (survives restart)"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ambient_due_waits_out_idle_then_dwell() {
    let idle = AMBIENT_IDLE.as_secs_f64();
    let dwell = AMBIENT_DWELL.as_secs_f64();
    // Still inside the idle window → never due.
    assert!(!ambient_due(idle - 1.0, 0.0, 0.0));
    // Idle window AND a full dwell elapsed → due (the entry step).
    assert!(ambient_due(idle + dwell, 0.0, 0.0));
    // Past idle, but the previous step was under a dwell ago → throttled crawl.
    let now = idle + dwell + 5.0;
    assert!(!ambient_due(now, 0.0, now - (dwell - 0.5)));
}

#[test]
fn ambient_idle_advances_focus_and_input_pauses_it() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    s.set_mode(SurfaceMode::Hero);
    s.prefs.ambient_idle = true;
    s.focus = 0;

    let ctx = egui::Context::default();
    Style::install(&ctx);

    // Frame 1 at t=0 only arms the idle clock — nothing advances.
    ambient_frame(&ctx, &mut s, 0.0, vec![]);
    assert_eq!(s.focus, 0, "the first frame just arms the idle clock");

    // A quiet frame past idle+dwell → the ambient cycle steps one unit.
    let past = AMBIENT_IDLE.as_secs_f64() + AMBIENT_DWELL.as_secs_f64() + 1.0;
    ambient_frame(&ctx, &mut s, past, vec![]);
    assert_eq!(s.focus, 1, "sitting idle past the interval auto-advances");

    // ANY input pauses it — a key press even further along holds the focus
    // (the idle clock re-arms this frame; the cycle never fights the operator).
    let key = egui::Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    };
    ambient_frame(
        &ctx,
        &mut s,
        past + AMBIENT_DWELL.as_secs_f64() + 1.0,
        vec![key],
    );
    assert_eq!(s.focus, 1, "input pauses the cycle — the focus holds");
}

#[test]
fn ambient_stays_off_by_default_and_under_reduce_motion() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let past = AMBIENT_IDLE.as_secs_f64() + AMBIENT_DWELL.as_secs_f64() + 1.0;

    // Toggle OFF (the default) → no advance no matter how long it sits idle.
    let mut off = ExplorerState::with_fake(addressed_state(), "me");
    off.set_mode(SurfaceMode::Hero);
    off.focus = 0;
    ambient_frame(&ctx, &mut off, 0.0, vec![]);
    ambient_frame(&ctx, &mut off, past, vec![]);
    assert_eq!(off.focus, 0, "the default-off toggle never auto-advances");

    // Toggle ON but reduce-motion set → the cycle stays parked (WCAG 2.2.2).
    ctx.style_mut(|st| st.animation_time = 0.0);
    assert!(
        reduce_motion(&ctx),
        "zero animation_time reads as reduce-motion"
    );
    let mut rm = ExplorerState::with_fake(addressed_state(), "me");
    rm.set_mode(SurfaceMode::Hero);
    rm.prefs.ambient_idle = true;
    rm.focus = 0;
    ambient_frame(&ctx, &mut rm, 0.0, vec![]);
    ambient_frame(&ctx, &mut rm, past, vec![]);
    assert_eq!(rm.focus, 0, "reduce-motion parks the ambient cycle");
}

#[test]
fn ambient_step_wraps_around_the_shelf() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    let count = s.hero_count();
    assert!(count > 1, "the fixture has a shelf to cycle");
    s.focus = count - 1;
    s.ambient_step();
    assert_eq!(s.focus, 0, "the wall display loops back to the start");
}

#[test]
fn picking_a_tile_zooms_into_its_hero() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    s.last_action_note = Some(("stale".into(), false)); // a note from a prior view
    let rect = Rect::from_min_size(egui::pos2(10.0, 10.0), Vec2::splat(100.0));
    s.zoom_into(2, Some(rect));
    assert_eq!(s.mode, SurfaceMode::Hero, "a pick zooms into the hero");
    assert_eq!(s.focus, 2, "the picked tile becomes the focused hero");
    assert_eq!(
        s.zoom_from,
        Some(rect),
        "the zoom animates from the tile rect"
    );
    assert!(
        s.zoom_start.is_some(),
        "the shared-element zoom clock is running"
    );
    assert!(
        s.last_action_note.is_none() && s.arm.is_none(),
        "the zoom reuses the focus path and drops stale arm/note"
    );
}

#[test]
fn back_zooms_out_to_the_mosaic() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    s.zoom_into(1, None);
    assert_eq!(s.mode, SurfaceMode::Hero);
    s.back_to_mosaic();
    assert_eq!(s.mode, SurfaceMode::Mosaic, "Back returns to the overview");
    assert_eq!(s.focus, 1, "the just-viewed tile stays selected (coherent)");
    assert!(s.zoom_from.is_none() && s.zoom_start.is_none());
    assert!(
        s.mosaic_enter.is_some(),
        "the reverse settle fade is seeded"
    );
}

#[test]
fn grid_nav_walks_the_mosaic_and_clamps_at_the_edges() {
    // A 5-item, 3-wide grid: rows [0 1 2] [3 4].
    let (n, cols) = (5, 3);
    assert_eq!(grid_move(0, n, cols, GridDir::Right), 1);
    assert_eq!(
        grid_move(2, n, cols, GridDir::Right),
        3,
        "steps into the next row"
    );
    assert_eq!(
        grid_move(0, n, cols, GridDir::Left),
        0,
        "clamps at the start"
    );
    assert_eq!(
        grid_move(4, n, cols, GridDir::Right),
        4,
        "clamps at the end"
    );
    assert_eq!(grid_move(0, n, cols, GridDir::Down), 3, "down a whole row");
    assert_eq!(grid_move(3, n, cols, GridDir::Up), 0, "up a whole row");
    assert_eq!(
        grid_move(1, n, cols, GridDir::Up),
        1,
        "the top row can't rise"
    );
    assert_eq!(
        grid_move(4, n, cols, GridDir::Down),
        4,
        "the last item can't fall"
    );
    // Degenerate inputs never panic.
    assert_eq!(
        grid_move(0, 0, cols, GridDir::Right),
        0,
        "an empty grid stays put"
    );
    assert_eq!(grid_move(2, n, 0, GridDir::Down), 3, "cols floors to 1");
}

#[test]
fn mosaic_columns_fit_and_floor_to_one() {
    assert!(
        mosaic_columns(2000.0) >= 3,
        "a wide surface fits several tiles"
    );
    assert_eq!(
        mosaic_columns(10.0),
        1,
        "a narrow surface still shows one column"
    );
    assert_eq!(
        mosaic_columns(-50.0),
        1,
        "a nonsense width never underflows"
    );
}

#[test]
fn zoom_geometry_interpolates_from_tile_to_full() {
    let from = Rect::from_min_size(egui::pos2(20.0, 20.0), Vec2::splat(10.0));
    let to = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::splat(100.0));
    assert_eq!(lerp_rect(from, to, 0.0), from, "t=0 sits on the tile");
    assert_eq!(lerp_rect(from, to, 1.0), to, "t=1 fills the hero frame");
    assert!(ease_out(0.0).abs() < f32::EPSILON);
    assert!((ease_out(1.0) - 1.0).abs() < f32::EPSILON);
    assert!(ease_out(0.5) > 0.5, "ease-out leads linear at the midpoint");
}

#[test]
fn rollup_counts_are_honest_over_the_shelf() {
    // Mixed health + addresses: green/warn/down tallies count only real tiers,
    // unknown/unprobed count in none; total addresses counts only reporters.
    let states = vec![UnitsState {
        host: "me".into(),
        units: vec![
            Unit {
                health: Some(Health::Healthy),
                address: Some("10.42.0.1".into()),
                ..unit("peer:me", UnitKind::Peer, "me", 10)
            },
            Unit {
                health: Some(Health::Degraded),
                address: Some("10.42.0.2".into()),
                ..unit("peer:b", UnitKind::Peer, "b", 10)
            },
            Unit {
                health: Some(Health::Critical),
                address: None,
                ..unit("peer:c", UnitKind::Peer, "c", 10)
            },
            Unit {
                health: Some(Health::Unreachable),
                address: Some("172.20.0.9".into()),
                ..unit("lan:d", UnitKind::LanHost, "d", 10)
            },
            Unit {
                health: Some(Health::Unknown),
                address: None,
                ..unit("cloud:instance:i", UnitKind::Instance, "i", 10)
            },
        ],
        edges: Vec::new(),
    }];
    let s = ExplorerState::with_fake(states, "me");
    assert_eq!(
        s.health_rollup(),
        [1, 1, 2],
        "1 green, 1 warn, 2 down (critical + unreachable); unknown counts in none"
    );
    assert_eq!(
        s.total_addresses(),
        3,
        "only the three address-reporting units"
    );
    assert_eq!(s.category_counts(), [3, 1, 1], "3 mesh, 1 lan, 1 cloud");
}

#[test]
fn mosaic_renders_headless_across_filters_and_empty() {
    let render = |s: &mut ExplorerState| {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                Vec2::new(1200.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    };
    // The mosaic is the default landing → show() drives the mosaic path.
    for filter in [
        None,
        Some(Category::Mesh),
        Some(Category::Lan),
        Some(Category::Cloud),
    ] {
        let mut s = ExplorerState::with_fake(addressed_state(), "me");
        s.set_filter(filter);
        assert!(render(&mut s), "the mosaic drew primitives for {filter:?}");
    }
    // The honest empty (#23) self tile renders in the mosaic too, never blank.
    let mut empty = ExplorerState::with_fake(vec![], "solo");
    assert!(render(&mut empty), "the empty mosaic drew the self tile");
}

// ─────────────── EXPLORER-13 view/selection/filter persistence ───────────────

#[test]
fn the_view_record_round_trips_through_disk() {
    // The full O5 record (mode + selection + filter, with the EXPLORER-12
    // toggle riding along) survives a write → read-back — the restart path.
    let dir = ambient_temp_dir("view-rt");
    std::fs::create_dir_all(&dir).expect("mkroot");
    let path = dir.join(PREFS_FILE);
    let prefs = ExplorerPrefs {
        ambient_idle: true,
        mode: SurfaceMode::Ipam,
        selected: Some("lan:printer".to_string()),
        filter: Some(Category::Lan),
        search: "vnc".to_string(),
        pinned: vec!["peer:me".to_string()],
        pinned_only: true,
    };
    prefs.save_to(&path).expect("save");
    assert_eq!(
        ExplorerPrefs::load_from(&path),
        prefs,
        "the whole view record survives a restart"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_legacy_ambient_only_record_folds_the_view_fields_to_default() {
    // A pre-EXPLORER-13 prefs file carries only the ambient toggle — the new
    // fields fold to their defaults instead of failing the whole load (§7).
    let dir = ambient_temp_dir("legacy");
    std::fs::create_dir_all(&dir).expect("mkroot");
    let path = dir.join(PREFS_FILE);
    std::fs::write(&path, r#"{"ambient_idle":true}"#).expect("write legacy");
    let prefs = ExplorerPrefs::load_from(&path);
    assert!(prefs.ambient_idle, "the legacy toggle still reads");
    assert_eq!(prefs.mode, SurfaceMode::Mosaic);
    assert_eq!(prefs.selected, None);
    assert_eq!(prefs.filter, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn restore_returns_to_the_remembered_mode_filter_and_unit() {
    let prefs = ExplorerPrefs {
        mode: SurfaceMode::Hero,
        selected: Some("lan:printer".to_string()),
        filter: Some(Category::Lan),
        ..Default::default()
    };
    let s = ExplorerState::with_prefs(addressed_state(), "me", prefs);
    assert_eq!(s.mode, SurfaceMode::Hero, "the last mode restores");
    assert_eq!(s.filter, Some(Category::Lan), "the active filter restores");
    assert_eq!(
        focused(&s).id,
        "lan:printer",
        "the remembered unit is focused again"
    );
    assert!(
        s.pending_focus.is_none(),
        "the landed selection releases the hold"
    );
}

#[test]
fn restore_falls_back_gracefully_when_the_remembered_unit_is_gone() {
    let prefs = ExplorerPrefs {
        mode: SurfaceMode::Hero,
        selected: Some("peer:departed".to_string()),
        ..Default::default()
    };
    let mut s = ExplorerState::with_prefs(addressed_state(), "me", prefs);
    // The vanished unit can't land — focus stays at the front of the shelf.
    assert_eq!(s.focus, 0, "a gone selection folds to the front");
    assert_eq!(
        s.pending_focus.as_deref(),
        Some("peer:departed"),
        "the hold stays armed in case the unit streams back in"
    );
    // … and when it DOES stream back in, the remembered selection lands.
    s.client = Box::new(FakeUnits(vec![UnitsState {
        host: "me".into(),
        units: vec![unit("peer:departed", UnitKind::Peer, "departed", now_ms())],
        edges: Vec::new(),
    }]));
    s.refresh();
    assert_eq!(
        focused(&s).id,
        "peer:departed",
        "a late-arriving remembered unit still lands"
    );
}

#[test]
fn the_view_snapshot_persists_on_change_and_only_on_change() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    let dir = ambient_temp_dir("persist");
    std::fs::create_dir_all(&dir).expect("mkroot");
    let path = dir.join(PREFS_FILE);
    s.prefs_path = Some(path.clone());

    // A view change → the snapshot lands on disk.
    s.set_mode(SurfaceMode::Ipam);
    s.set_filter(Some(Category::Mesh));
    s.persist_view();
    let on_disk = ExplorerPrefs::load_from(&path);
    assert_eq!(on_disk.mode, SurfaceMode::Ipam);
    assert_eq!(on_disk.filter, Some(Category::Mesh));
    assert_eq!(
        on_disk.selected.as_deref(),
        Some("peer:me"),
        "the focused unit rides the snapshot"
    );

    // No change → no rewrite: delete the file; persist_view must not re-mint it.
    std::fs::remove_file(&path).expect("rm");
    s.persist_view();
    assert!(
        !path.exists(),
        "an unchanged view never rewrites the record"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rendered_frame_persists_the_view_through_show() {
    // show() drives persist_view — one headless frame lands the record.
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    let dir = ambient_temp_dir("frame");
    std::fs::create_dir_all(&dir).expect("mkroot");
    let path = dir.join(PREFS_FILE);
    s.prefs_path = Some(path.clone());
    s.set_mode(SurfaceMode::Hero);
    let ctx = egui::Context::default();
    Style::install(&ctx);
    ambient_frame(&ctx, &mut s, 0.0, vec![]);
    assert_eq!(
        ExplorerPrefs::load_from(&path).mode,
        SurfaceMode::Hero,
        "the frame's view change reached the disk record"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_early_empty_frame_never_clobbers_a_held_selection() {
    // Restored with a remembered unit that hasn't streamed in yet: the first
    // snapshot must keep remembering it, not overwrite `selected` with the
    // placeholder's `None`.
    let prefs = ExplorerPrefs {
        selected: Some("peer:later".to_string()),
        ..Default::default()
    };
    let s = ExplorerState::with_prefs(vec![], "me", prefs);
    assert_eq!(
        s.view_snapshot().selected.as_deref(),
        Some("peer:later"),
        "the held restore target stays the remembered selection"
    );
}

// ─────────────── EXPLORER-14 universal search + jump ───────────────

/// A shelf spanning every searchable field (O7): a MAC-keyed LAN host
/// fingerprinted with VNC on 5900, an IP-keyed LAN host, a Nova instance on
/// node `bigboy`, peers, and a volume.
fn searchable_state() -> Vec<UnitsState> {
    let mut vnc_box = addr_unit(
        "lan:aa:bb:cc:dd:ee:ff",
        UnitKind::LanHost,
        "media-box",
        "172.20.0.60",
    );
    vnc_box.extras.fingerprint = Some("vnc".to_string());
    vnc_box
        .extras
        .extra
        .insert("open_ports".to_string(), "5900".to_string());
    let mut printer = addr_unit(
        "lan:172.20.0.50",
        UnitKind::LanHost,
        "printer",
        "172.20.0.50",
    );
    printer
        .extras
        .extra
        .insert("open_ports".to_string(), "80,443".to_string());
    let web = Unit {
        reachability: Reachability::CloudObject {
            node: "bigboy".to_string(),
        },
        ..unit("cloud:instance:i1", UnitKind::Instance, "web", 10)
    };
    vec![UnitsState {
        host: "me".into(),
        units: vec![
            unit("peer:me", UnitKind::Peer, "me", 10),
            unit("peer:anvil", UnitKind::Peer, "anvil", 10),
            vnc_box,
            printer,
            web,
            unit("cloud:volume:v1", UnitKind::Volume, "data", 10),
        ],
        edges: Vec::new(),
    }]
}

#[test]
fn explorer_search_items_feed_the_shared_ranker_for_unit_fields() {
    let s = ExplorerState::with_fake(searchable_state(), "me");
    let media_idx = s
        .units
        .iter()
        .position(|unit| unit.name == "media-box")
        .expect("media-box");
    let web_idx = s
        .units
        .iter()
        .position(|unit| unit.name == "web")
        .expect("web");
    let items = s.search_items();

    assert!(items.iter().all(|item| item.domain == SearchDomain::Mesh));
    assert!(
        items
            .iter()
            .any(|item| item.payload == media_idx && item.title == "5900"),
        "discovered service/port fields become shared candidates",
    );
    assert!(
        items
            .iter()
            .any(|item| item.payload == web_idx && item.title == "bigboy"),
        "hosting-node fields become shared candidates",
    );

    let hits = ranked_hits("mdb", items.clone(), items.len());
    assert_eq!(
        hits.first()
            .map(|hit| s.units[hit.item.payload].name.as_str()),
        Some("media-box"),
        "shared fuzzy title ranking still finds the unit name",
    );
}

#[test]
fn search_spans_every_field() {
    let s = ExplorerState::with_fake(searchable_state(), "me");
    let names = |q: &str| -> Vec<String> {
        s.search_hits(q)
            .iter()
            .map(|&i| s.units[i].name.clone())
            .collect()
    };
    // "5900" → the VNC host, via the discovered open-ports field (O7).
    assert_eq!(names("5900"), vec!["media-box"]);
    // "nova" → the instance, via the design's own type taxonomy (lock #4).
    assert_eq!(names("nova"), vec!["web"]);
    // A MAC prefix → the MAC-keyed LAN host (the aggregator's lan:<mac> id).
    assert_eq!(names("aa:bb:cc"), vec!["media-box"]);
    // A node name → the cloud object it hosts (lock #20's host-node tag).
    assert_eq!(names("bigboy"), vec!["web"]);
    // A service label → the fingerprinted host.
    assert_eq!(names("vnc"), vec!["media-box"]);
    // An IP → the addressed unit.
    assert_eq!(names("172.20.0.50"), vec!["printer"]);
    // Junk matches nothing; an empty/blank query lists nothing (§7 — the
    // just-opened box never fakes an "everything matches" wall).
    assert!(names("zzzz").is_empty());
    assert!(names("").is_empty());
    assert!(names("   ").is_empty());
}

#[test]
fn search_ranks_the_name_hit_first_and_caps_the_list() {
    let s = ExplorerState::with_fake(searchable_state(), "me");
    let hits = s.search_hits("an");
    assert_eq!(
        s.units[hits[0]].id, "peer:anvil",
        "the boundary name hit outranks buried subsequences"
    );
    assert!(hits.len() <= SEARCH_MAX_HITS, "the hit list is capped");
}

#[test]
fn a_search_pick_jumps_the_focus_and_closes_the_overlay() {
    let mut s = ExplorerState::with_fake(searchable_state(), "me");
    s.set_filter(Some(Category::Mesh)); // a filter that hides the hit
    s.open_search();
    let hits = s.search_hits("5900");
    let id = s.units[hits[0]].id.clone();
    s.jump_to_search_hit(&id);
    assert_eq!(focused(&s).id, "lan:aa:bb:cc:dd:ee:ff");
    assert_eq!(s.filter, None, "a hiding filter clears so the jump lands");
    assert!(s.search.is_none(), "the overlay closes on a pick");

    // From the IPAM table a pick returns to the hero card (no table focus).
    s.mode = SurfaceMode::Ipam;
    s.open_search();
    s.jump_to_search_hit("peer:anvil");
    assert_eq!(s.mode, SurfaceMode::Hero);
    assert_eq!(focused(&s).id, "peer:anvil");
}

#[test]
fn an_active_search_persists_and_restores_open() {
    // The active query rides the O5 view record and reopens on restore …
    let prefs = ExplorerPrefs {
        search: "nova".to_string(),
        ..Default::default()
    };
    let s = ExplorerState::with_prefs(searchable_state(), "me", prefs);
    assert!(
        s.search.as_ref().is_some_and(|x| x.query == "nova"),
        "a restored search reopens with its query"
    );
    // … the live query rides the snapshot …
    let mut live = ExplorerState::with_fake(searchable_state(), "me");
    live.open_search();
    if let Some(x) = live.search.as_mut() {
        x.query = "web".to_string();
    }
    assert_eq!(live.view_snapshot().search, "web");
    // … and closing the overlay clears the persisted half.
    live.search = None;
    assert_eq!(live.view_snapshot().search, "");
}

#[test]
fn slash_opens_the_search_and_esc_closes_it() {
    let mut s = ExplorerState::with_fake(searchable_state(), "me");
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let slash = egui::Event::Key {
        key: egui::Key::Slash,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    };
    ambient_frame(
        &ctx,
        &mut s,
        0.0,
        vec![slash, egui::Event::Text("/".to_string())],
    );
    assert!(s.search.is_some(), "`/` opens the universal search");
    assert_eq!(
        s.search.as_ref().map(|x| x.query.as_str()),
        Some(""),
        "the opening slash is consumed, never typed into the box"
    );
    // Esc closes it again.
    let esc = egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    };
    ambient_frame(&ctx, &mut s, 0.5, vec![esc]);
    assert!(s.search.is_none(), "Esc closes the search");
}

#[test]
fn search_selection_keys_walk_the_hits_and_enter_jumps() {
    let mut s = ExplorerState::with_fake(searchable_state(), "me");
    s.open_search();
    if let Some(x) = s.search.as_mut() {
        x.query = "17".to_string(); // two LAN addresses match
    }
    let hits = s.search_hits("17");
    assert!(hits.len() >= 2, "the fixture yields a walkable list");
    let second = s.units[hits[1]].id.clone();
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let key = |k: egui::Key| egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    };
    ambient_frame(&ctx, &mut s, 0.0, vec![key(egui::Key::ArrowDown)]);
    assert_eq!(
        s.search.as_ref().map(|x| x.sel),
        Some(1),
        "Down moves the selection"
    );
    ambient_frame(&ctx, &mut s, 0.5, vec![key(egui::Key::Enter)]);
    assert!(s.search.is_none(), "Enter closes the search");
    assert_eq!(focused(&s).id, second, "Enter jumped the selected hit");
}

#[test]
fn the_search_overlay_renders_headless() {
    let mut s = ExplorerState::with_fake(searchable_state(), "me");
    s.open_search();
    if let Some(x) = s.search.as_mut() {
        x.query = "a".to_string();
    }
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            Vec2::new(1200.0, 800.0),
        )),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
    });
    assert!(
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty(),
        "the search overlay drew primitives"
    );
}

// ─────────────── EXPLORER-16 pinning + the Pinned cluster ───────────────

#[test]
fn pinned_units_sort_to_the_front_of_the_fold() {
    let state = UnitsState {
        host: "me".into(),
        units: vec![
            unit("cloud:instance:i1", UnitKind::Instance, "web", 10),
            unit("lan:aa", UnitKind::LanHost, "printer", 10),
            unit("peer:me", UnitKind::Peer, "me", 10),
            unit("peer:alpha", UnitKind::Peer, "alpha", 10),
        ],
        edges: Vec::new(),
    };
    // Unpinned: self first, then proximity (the #23/#7 order).
    let plain = fold_units(std::slice::from_ref(&state), "me", &[]);
    assert_eq!(plain[0].id, "peer:me");
    // Pin the cloud instance: it jumps to the very front (O9), the rest keep
    // their order.
    let pinned = vec!["cloud:instance:i1".to_string()];
    let folded = fold_units(&[state], "me", &pinned);
    let ids: Vec<&str> = folded.iter().map(|u| u.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["cloud:instance:i1", "peer:me", "peer:alpha", "lan:aa"],
        "pinned first, then self, then proximity+name"
    );
}

#[test]
fn toggle_pin_reorders_live_keeps_focus_and_round_trips_through_disk() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    s.set_mode(SurfaceMode::Hero);
    focus_on(&mut s, "peer:anvil");

    // Pin the printer: it moves to the front; the operator's focus stays on
    // anvil (a pin re-orders, never teleports).
    s.toggle_pin("lan:printer");
    assert!(s.is_pinned("lan:printer"));
    assert_eq!(
        s.units[0].id, "lan:printer",
        "the pin surfaced to the front"
    );
    assert_eq!(
        focused(&s).id,
        "peer:anvil",
        "focus held through the re-sort"
    );

    // The pin set persists (rides the ONE prefs record).
    let dir = ambient_temp_dir("pin-rt");
    std::fs::create_dir_all(&dir).expect("mkroot");
    let path = dir.join(PREFS_FILE);
    s.prefs_path = Some(path.clone());
    s.persist_view();
    assert_eq!(
        ExplorerPrefs::load_from(&path).pinned,
        vec!["lan:printer".to_string()],
        "the pin set survives a restart"
    );

    // Unpin: the shelf returns to the plain order.
    s.toggle_pin("lan:printer");
    assert!(!s.is_pinned("lan:printer"));
    assert_eq!(s.units[0].id, "peer:me", "unpinning restores self-first");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_restored_pin_set_orders_the_first_fold() {
    let prefs = ExplorerPrefs {
        pinned: vec!["cloud:instance:i1".to_string()],
        ..Default::default()
    };
    let s = ExplorerState::with_prefs(addressed_state(), "me", prefs);
    assert!(s.is_pinned("cloud:instance:i1"));
    assert_eq!(
        s.units[0].id, "cloud:instance:i1",
        "the restored pin set fronts the very first fold"
    );
}

#[test]
fn the_pinned_chip_scopes_the_view_and_composes_with_a_category() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    s.toggle_pin("lan:printer");
    s.toggle_pin("cloud:instance:i1");

    // Pinned alone: exactly the two pinned units.
    s.set_pinned_only(true);
    let ids: Vec<String> = s
        .filtered_indices()
        .iter()
        .map(|&i| s.units[i].id.clone())
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"lan:printer".to_string()));
    assert!(ids.contains(&"cloud:instance:i1".to_string()));

    // Pinned ∩ Cloud: the pinned instance only.
    s.set_filter(Some(Category::Cloud));
    let ids: Vec<String> = s
        .filtered_indices()
        .iter()
        .map(|&i| s.units[i].id.clone())
        .collect();
    assert_eq!(ids, vec!["cloud:instance:i1".to_string()]);

    // Clearing both restores the whole shelf.
    s.set_filter(None);
    s.set_pinned_only(false);
    assert_eq!(s.filtered_indices().len(), s.units.len());
}

#[test]
fn the_pinned_scope_with_no_pins_is_honestly_empty() {
    // No self-placeholder fake under the Pinned chip (§7): zero pages + the
    // honest how-to note.
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    s.set_pinned_only(true);
    assert_eq!(s.hero_count(), 0, "no pins → an honest empty view");
    assert!(
        s.empty_note_text().contains("No pinned units"),
        "the note says why it's empty"
    );
}

#[test]
fn cluster_runs_front_the_pinned_units_under_their_own_header() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    s.toggle_pin("cloud:instance:i1");
    let indices = s.filtered_indices();
    let runs = s.cluster_runs(&indices);
    assert_eq!(
        runs[0].0,
        Cluster::Pinned,
        "the Pinned cluster leads the mosaic"
    );
    assert_eq!(runs[0].1.len(), 1);
    assert_eq!(s.units[runs[0].1[0]].id, "cloud:instance:i1");
    // The remaining runs are the plain category clusters in proximity order.
    assert_eq!(runs[1].0, Cluster::Cat(Category::Mesh));
    assert!(
        runs.iter().skip(1).all(|(c, _)| *c != Cluster::Pinned),
        "exactly one Pinned run"
    );
    // The cluster identity tokens: Pinned wears the highlight accent.
    assert_eq!(Cluster::Pinned.label(), "Pinned");
    assert_eq!(Cluster::Pinned.accent(), Style::ACCENT_HI);
    assert_eq!(Cluster::Cat(Category::Lan).label(), "LAN");
}

#[test]
fn the_pinned_ipam_scope_keeps_prefixes_hosting_a_pinned_unit() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    s.toggle_pin("lan:printer");
    s.set_pinned_only(true);
    let prefixes = s.ipam_prefixes();
    assert_eq!(prefixes.len(), 1, "only the pinned unit's /24 remains");
    assert_eq!(prefixes[0].cidr(), "172.20.0.0/24");
}

#[test]
fn the_p_key_pins_the_focused_unit() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    s.set_mode(SurfaceMode::Hero);
    focus_on(&mut s, "peer:anvil");
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let p = egui::Event::Key {
        key: egui::Key::P,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    };
    ambient_frame(&ctx, &mut s, 0.0, vec![p.clone()]);
    assert!(s.is_pinned("peer:anvil"), "P pins the focused unit");
    assert_eq!(
        focused(&s).id,
        "peer:anvil",
        "the focus stays on the unit through its pin"
    );
    ambient_frame(&ctx, &mut s, 0.5, vec![p]);
    assert!(!s.is_pinned("peer:anvil"), "P again unpins it");
}

#[test]
fn the_pinned_mosaic_and_filmstrip_render_headless() {
    let render = |s: &mut ExplorerState| {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                Vec2::new(1200.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    };
    // The mosaic with a Pinned cluster + pin markers.
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    s.toggle_pin("lan:printer");
    assert!(render(&mut s), "the pinned mosaic drew primitives");
    // The hero + filmstrip with a pinned thumb + the Pin button.
    s.set_mode(SurfaceMode::Hero);
    assert!(render(&mut s), "the pinned hero/filmstrip drew primitives");
    // The honest empty Pinned scope.
    let mut none = ExplorerState::with_fake(addressed_state(), "me");
    none.set_pinned_only(true);
    assert!(render(&mut none), "the empty Pinned scope drew its note");
}

// ─────────── EXPLORER-17 multi-select + armed bulk actions ───────────

/// A sink that accepts the first `fail_from` publishes then faults — the
/// partial-failure fixture for the bulk rollup.
struct FailingActions {
    calls: std::rc::Rc<std::cell::RefCell<Vec<(String, String)>>>,
    fail_from: usize,
}
impl ActionSink for FailingActions {
    fn publish(&self, topic: &str, body: &str) -> Result<(), String> {
        let mut calls = self.calls.borrow_mut();
        if calls.len() >= self.fail_from {
            return Err("bus write fault".to_string());
        }
        calls.push((topic.to_string(), body.to_string()));
        Ok(())
    }
}

#[test]
fn bulk_verbs_are_the_shared_real_intersection() {
    // Peers share exactly the health check (open/evict are single-target
    // or dead seams).
    let peers = vec![
        unit("peer:a", UnitKind::Peer, "a", 1),
        unit("peer:b", UnitKind::Peer, "b", 1),
    ];
    assert_eq!(shared_bulk_verbs(&peers), vec![Verb::HealthCheck]);
    // Instances share the four lifecycle verbs; Console is a single-target
    // navigation hand-off and never a bulk verb, even with addresses.
    let instances = vec![
        instance_unit("cloud:instance:i1", "web"),
        instance_unit("cloud:instance:i2", "db"),
    ];
    assert_eq!(
        shared_bulk_verbs(&instances),
        vec![Verb::Start, Verb::Stop, Verb::Reboot, Verb::Delete]
    );
    // A mixed peer+instance selection shares NOTHING — the bar offers no
    // padded verb (§7, "no dead bulk verb").
    let mixed = vec![
        unit("peer:a", UnitKind::Peer, "a", 1),
        instance_unit("cloud:instance:i1", "web"),
    ];
    assert!(shared_bulk_verbs(&mixed).is_empty());
    // A peer + LAN host DO share the health check.
    let peer_lan = vec![
        unit("peer:a", UnitKind::Peer, "a", 1),
        unit("lan:x", UnitKind::LanHost, "x", 1),
    ];
    assert_eq!(shared_bulk_verbs(&peer_lan), vec![Verb::HealthCheck]);
    // Volumes offer only nav/dead verbs → nothing bulk-capable.
    let volumes = vec![
        unit("cloud:volume:v1", UnitKind::Volume, "v1", 1),
        unit("cloud:volume:v2", UnitKind::Volume, "v2", 1),
    ];
    assert!(shared_bulk_verbs(&volumes).is_empty());
    // An empty selection has no verbs at all.
    assert!(shared_bulk_verbs(&[]).is_empty());
}

#[test]
fn marks_toggle_range_and_prune_with_the_shelf() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    // Toggle on / off.
    s.toggle_mark("peer:anvil");
    assert!(s.is_marked("peer:anvil"));
    s.toggle_mark("peer:anvil");
    assert!(!s.is_marked("peer:anvil"));
    // Range over the filtered view (positions 0..=2 = me, anvil, printer),
    // additive + idempotent in either direction.
    s.mark_range(0, 2);
    assert_eq!(s.marked_units().len(), 3);
    s.mark_range(2, 0);
    assert_eq!(s.marked_units().len(), 3, "range marking is additive");
    // A unit leaving the shelf prunes its mark; an emptied selection
    // disarms a pending bulk verb.
    s.bulk_arm = Some(BulkArm {
        verb: Verb::HealthCheck,
        echo: String::new(),
    });
    s.client = Box::new(FakeUnits(vec![UnitsState {
        host: "me".into(),
        units: vec![addr_unit(
            "peer:anvil",
            UnitKind::Peer,
            "anvil",
            "10.42.0.7",
        )],
        edges: Vec::new(),
    }]));
    s.refresh();
    assert_eq!(
        s.marked,
        vec!["peer:anvil".to_string()],
        "departed units' marks pruned"
    );
    assert!(s.bulk_arm.is_some(), "a live selection keeps its arm");
    s.toggle_mark("peer:anvil");
    assert!(
        s.bulk_arm.is_none(),
        "an emptied selection disarms the bulk verb"
    );
}

#[test]
fn bulk_destructive_is_gated_on_the_typed_count_phrase() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    let fake = s.recording();
    s.toggle_mark("cloud:instance:i1"); // web
    s.toggle_mark("cloud:instance:i2"); // db
    s.bulk_arm = Some(BulkArm {
        verb: Verb::Delete,
        echo: String::new(),
    });
    // Un-echoed / mis-counted phrases never fire (§7 + O10 arming).
    assert!(!s.confirm_bulk(), "a blank echo is a no-op");
    s.bulk_arm.as_mut().expect("armed").echo = "delete 3".to_string();
    assert!(!s.confirm_bulk(), "a wrong count never fires");
    assert!(
        fake.calls.borrow().is_empty(),
        "nothing published while gated"
    );
    // The exact phrase fires one real dispatch per unit, in shelf order.
    s.bulk_arm.as_mut().expect("armed").echo = " delete 2 ".to_string();
    assert!(s.bulk_ready(), "the trimmed exact phrase arms");
    assert!(s.confirm_bulk());
    assert_eq!(
        fake.calls.borrow().as_slice(),
        &[
            (
                "action/cloud/instance-delete".to_string(),
                r#"{"instance":"i2"}"#.to_string() // db sorts before web
            ),
            (
                "action/cloud/instance-delete".to_string(),
                r#"{"instance":"i1"}"#.to_string()
            ),
        ],
        "one QC-11 request per marked instance"
    );
    assert!(s.bulk_arm.is_none(), "the arm clears after the run");
    let rollup = s.bulk_rollup.as_ref().expect("a rollup landed");
    assert_eq!((rollup.ok, rollup.total), (2, 2));
    let (note, is_err) = bulk_note(rollup);
    assert!(note.contains("2/2"), "the rollup names the tally: {note}");
    assert!(!is_err);
}

#[test]
fn bulk_nondestructive_fires_per_unit_with_a_rollup() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    let fake = s.recording();
    s.toggle_mark("peer:me");
    s.toggle_mark("peer:anvil");
    s.run_bulk(Verb::HealthCheck);
    let calls = fake.calls.borrow();
    assert_eq!(calls.len(), 2, "one health request per marked peer");
    assert!(calls.iter().all(|(t, _)| t == "action/units/get-stream"));
    let rollup = s.bulk_rollup.as_ref().expect("rollup");
    assert_eq!((rollup.ok, rollup.total), (2, 2));
    assert!(rollup.failed.is_empty());
}

#[test]
fn bulk_rollup_names_per_unit_failures_honestly() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    let calls = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    s.action_sink = Box::new(FailingActions {
        calls: calls.clone(),
        fail_from: 1, // the first dispatch lands, the second faults
    });
    s.toggle_mark("cloud:instance:i1"); // web (second in shelf order)
    s.toggle_mark("cloud:instance:i2"); // db (first in shelf order)
    s.run_bulk(Verb::Start);
    assert_eq!(
        calls.borrow().len(),
        1,
        "only the first per-unit dispatch reached the bus"
    );
    let rollup = s.bulk_rollup.as_ref().expect("rollup");
    assert_eq!((rollup.ok, rollup.total), (1, 2));
    assert_eq!(
        rollup.failed,
        vec![("web".to_string(), "bus write fault".to_string())],
        "the failed unit is named with its honest reason"
    );
    let (note, is_err) = bulk_note(rollup);
    assert!(is_err);
    assert!(
        note.contains("1/2") && note.contains("web — bus write fault"),
        "the note carries tally + failure: {note}"
    );
}

#[test]
fn modified_picks_mark_instead_of_zooming() {
    let plain = egui::Modifiers::default();
    assert_eq!(pick_action(plain), PickAction::Zoom);
    let ctrl = egui::Modifiers {
        ctrl: true,
        ..Default::default()
    };
    assert_eq!(pick_action(ctrl), PickAction::ToggleMark);
    let cmd = egui::Modifiers {
        command: true,
        ..Default::default()
    };
    assert_eq!(pick_action(cmd), PickAction::ToggleMark);
    let shift = egui::Modifiers {
        shift: true,
        ..Default::default()
    };
    assert_eq!(pick_action(shift), PickAction::RangeMark);
    // Ctrl outranks Shift when both are held (a single deterministic rule).
    let both = egui::Modifiers {
        ctrl: true,
        shift: true,
        ..Default::default()
    };
    assert_eq!(pick_action(both), PickAction::ToggleMark);
}

#[test]
fn space_marks_enter_zooms_and_esc_clears_on_the_dpad() {
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    assert_eq!(s.mode, SurfaceMode::Mosaic, "the landing is the mosaic");
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let key = |k: egui::Key| egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    };
    // Space marks the focused tile (peer:me at focus 0) — it no longer zooms.
    ambient_frame(&ctx, &mut s, 0.0, vec![key(egui::Key::Space)]);
    assert!(s.is_marked("peer:me"), "Space is the D-pad mark");
    assert_eq!(s.mode, SurfaceMode::Mosaic, "Space never zooms");
    // Enter zooms the focused tile into its hero; the marks survive.
    ambient_frame(&ctx, &mut s, 0.5, vec![key(egui::Key::Enter)]);
    assert_eq!(s.mode, SurfaceMode::Hero, "Enter zooms in");
    assert!(s.is_marked("peer:me"), "zooming never clears the selection");
    // Esc in the hero zooms back out (the O3 reverse), marks intact…
    ambient_frame(&ctx, &mut s, 1.0, vec![key(egui::Key::Escape)]);
    assert_eq!(s.mode, SurfaceMode::Mosaic);
    assert!(s.is_marked("peer:me"));
    // …and Esc in the mosaic clears the live selection.
    ambient_frame(&ctx, &mut s, 1.5, vec![key(egui::Key::Escape)]);
    assert!(s.marked.is_empty(), "Esc clears the selection");
}

#[test]
fn the_bulk_bar_renders_headless() {
    // Selection + shared verbs + the typed challenge + a rollup all
    // tessellate cleanly under the mosaic.
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    s.toggle_mark("cloud:instance:i1");
    s.toggle_mark("cloud:instance:i2");
    s.bulk_arm = Some(BulkArm {
        verb: Verb::Reboot,
        echo: "reb".to_string(),
    });
    s.bulk_rollup = Some(BulkRollup {
        verb: Verb::Start,
        total: 2,
        ok: 1,
        failed: vec![("web".to_string(), "bus write fault".to_string())],
    });
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            Vec2::new(1200.0, 800.0),
        )),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
    });
    assert!(
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty(),
        "the marked mosaic + bulk bar drew primitives"
    );
    // A mixed selection renders the honest no-shared-verbs note.
    let mut mixed = ExplorerState::with_fake(addressed_state(), "me");
    mixed.toggle_mark("peer:me");
    mixed.toggle_mark("cloud:instance:i1");
    let out = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                Vec2::new(1200.0, 800.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| mixed.show(ui));
        },
    );
    assert!(!ctx.tessellate(out.shapes, out.pixels_per_point).is_empty());
    assert!(
        shared_bulk_verbs(&mixed.marked_units()).is_empty(),
        "the mixed bar offers no padded verb (§7)"
    );
}

// ─────── EXPLORER-18 accessibility: type · focus ring · text-scale ───────

#[test]
#[allow(clippy::assertions_on_constants)] // the token contract IS constant (the mde-egui style-test idiom)
fn the_focus_ring_is_thick_high_contrast_and_never_camouflaged() {
    // O11 — a hairline can't carry couch-distance selection; the ring is a
    // deliberately thick stroke… (the shared platform token, now that
    // `focus_ring` delegates to `mde_egui::focus::paint_focus_ring`).
    assert!(
        mde_egui::focus::FOCUS_RING_W >= 2.0,
        "the focus ring must be thicker than a hairline"
    );
    // …in a tone distinct from every frame it can sit over: the category
    // tints (hover), the mark accent, and the calm border — so the
    // selection can never camouflage against its own element.
    for (name, c) in [
        ("mesh accent", Style::ACCENT_MESH),
        ("lan accent", Style::ACCENT_TERMINALS),
        ("cloud accent", Style::ACCENT_WORKLOADS),
        ("mark accent", Style::ACCENT),
        ("border", Style::BORDER),
        ("surface", Style::SURFACE),
    ] {
        assert_ne!(
            Style::ACCENT_HI,
            c,
            "the focus ring must stand apart from the {name}"
        );
    }
}

#[test]
#[allow(clippy::assertions_on_constants)] // the token contract IS constant (the mde-egui style-test idiom)
fn generous_display_type_leads_the_type_ramp() {
    // O11 "generous display type": the hero display name sits ABOVE the
    // largest legacy type rung — across-the-room legibility — and lands on
    // the shared HIG hero rung (PLATFORM-INTERFACES Q19/Q20), no raw px.
    assert!(
        HERO_TITLE_FS > Style::DISPLAY,
        "the hero title must out-size the display rung"
    );
    assert!(
        (HERO_TITLE_FS - Style::TYPE_LARGE_TITLE).abs() < f32::EPSILON,
        "the hero title sits on the shared TYPE_LARGE_TITLE rung, not a raw px"
    );
}

#[test]
fn explorer_chrome_strips_use_refined_toolbar_margin() {
    assert_eq!(
        explorer_toolbar_margin(),
        Style::toolbar_margin(),
        "Explorer filter/search/action strips should follow the shared toolbar density"
    );
    assert_ne!(
        explorer_toolbar_margin(),
        egui::Margin::same(Style::SP_S as i8),
        "Explorer chrome strips must not keep the old full-gutter thickness"
    );
}

#[test]
fn explorer_tooltip_frame_sits_on_the_shared_radius_ladder() {
    // PLATFORM-INTERFACES Q19/Q20 — the tooltip card rounds on the shared
    // RADIUS_S tier of the §4 ladder, asserted off the painted shape so a raw
    // `CornerRadius::same(..)` corner literal can't silently come back.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            Vec2::new(320.0, 120.0),
        )),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| explorer_tooltip(ui, "Jump to this unit"));
    });
    fn walk(shape: &egui::Shape, out: &mut Vec<(egui::CornerRadius, Color32)>) {
        match shape {
            egui::Shape::Rect(rect) => out.push((rect.corner_radius, rect.fill)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }
    let mut rects = Vec::new();
    for clipped in &out.shapes {
        walk(&clipped.shape, &mut rects);
    }
    assert!(
        rects.iter().any(|(radius, fill)| {
            *radius == egui::CornerRadius::from(Style::RADIUS_S) && *fill == Style::SURFACE
        }),
        "the tooltip surface must round on the shared RADIUS_S tier: {rects:?}"
    );
}

#[test]
fn the_focus_ring_paints_the_selection_in_mosaic_and_hero() {
    // Presence, not just definition: with NO pins and NO marks on the
    // shelf, the ONLY author of ACCENT_HI vertices is the shared
    // `focus_ring` — so finding the token in the draw list proves the
    // selection ring is actually painted.
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    assert_eq!(s.mode, SurfaceMode::Mosaic);
    assert!(
        painted(&painted_colors(&mut s), Style::ACCENT_HI),
        "the mosaic landing paints the focused tile's ring"
    );
    // The hero mode: the selection reads on the filmstrip's focused thumb.
    s.set_mode(SurfaceMode::Hero);
    assert!(
        painted(&painted_colors(&mut s), Style::ACCENT_HI),
        "the hero mode paints the focused filmstrip thumb's ring"
    );
}

#[test]
fn raised_cards_blend_the_shared_depth_tokens_rest_to_hover() {
    use mde_egui::style::Elevation;
    // At rest (t = 0) a raised card casts EXACTLY the shared Raised token —
    // the surface-side epaint conversion reuses every field and the umbra
    // comes straight from the token (§4: no minted colour).
    let raised = Elevation::Raised.shadow();
    let rest = raise_shadow(0.0);
    assert_eq!(
        rest.offset,
        [raised.offset[0] as i8, raised.offset[1] as i8],
        "the rest offset comes from the Raised token"
    );
    assert_eq!(rest.blur, raised.blur as u8, "the rest blur is the token's");
    assert_eq!(
        rest.spread, raised.spread as u8,
        "the rest spread is the token's"
    );
    assert_eq!(
        rest.color, raised.umbra,
        "the rest umbra IS the Raised token's, not a minted colour"
    );
    // A full hover-lift (t = 1) reaches EXACTLY the shared Overlay token —
    // the micro-interaction travels the token ladder, never a bespoke depth.
    let overlay = Elevation::Overlay.shadow();
    let lift = raise_shadow(1.0);
    assert_eq!(
        lift.offset,
        [overlay.offset[0] as i8, overlay.offset[1] as i8],
        "the lifted offset comes from the Overlay token"
    );
    assert_eq!(
        lift.blur, overlay.blur as u8,
        "the lifted blur is the token's"
    );
    assert_eq!(
        lift.color, overlay.umbra,
        "the lifted umbra IS the Overlay token's, not a minted colour"
    );
    // And every point of the ease keeps a translucent umbra — depth is alpha,
    // never an opaque fill (design lock #2).
    for i in 0..=10_u8 {
        let a = raise_shadow(f32::from(i) / 10.0).color.a();
        assert!(
            a > 0 && a < 255,
            "the umbra stays translucent across the ease (t={i}/10, a={a})"
        );
    }
}

#[test]
fn the_raised_cards_cast_the_depth_umbra_in_mosaic_and_hero() {
    // Presence, not just definition: the resting Raised umbra reaches the
    // draw list under the mosaic tiles AND the filmstrip thumbs, so the depth
    // adoption is actually painted (same probe as the focus-ring test).
    let umbra = mde_egui::style::Elevation::Raised.shadow().umbra;
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    assert_eq!(s.mode, SurfaceMode::Mosaic);
    assert!(
        painted(&painted_colors(&mut s), umbra),
        "the mosaic tiles cast the shared Raised depth shadow"
    );
    s.set_mode(SurfaceMode::Hero);
    assert!(
        painted(&painted_colors(&mut s), umbra),
        "the filmstrip thumbs cast the shared Raised depth shadow"
    );
}

#[test]
fn explorer_composes_with_the_platform_text_scale_zoom() {
    // SETTINGS-5 landed the whole-UI text-scale as an egui zoom_factor the
    // shell applies globally. The explorer must COMPOSE with it: render
    // under a scaled context without fighting the factor (no local
    // set_zoom/ppp write) and without double-scaling (the factor lands on
    // the context exactly once).
    let mut s = ExplorerState::with_fake(addressed_state(), "me");
    let ctx = egui::Context::default();
    Style::install(&ctx);
    ctx.set_zoom_factor(1.5);
    // Two full frames through every panel path (mosaic, then hero).
    ambient_frame(&ctx, &mut s, 0.0, vec![]);
    s.set_mode(SurfaceMode::Hero);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            Vec2::new(1200.0, 800.0),
        )),
        time: Some(0.5),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| s.show(ui));
    });
    assert!(
        (ctx.zoom_factor() - 1.5).abs() < f32::EPSILON,
        "the explorer never fights the platform text-scale factor"
    );
    assert!(
        (ctx.pixels_per_point() - 1.5).abs() < 0.01,
        "the zoom lands exactly once atop the 1.0 seat base — no double-scale"
    );
    assert!(
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty(),
        "the scaled explorer still draws"
    );
}
