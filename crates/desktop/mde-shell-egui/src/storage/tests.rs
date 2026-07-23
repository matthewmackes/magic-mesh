//! The **Storage** surface unit suite + the MENU-6 menubar-coverage backstop —
//! relocated verbatim from the `storage.rs` god-module (pure code move, no
//! behaviour change). As a child module `use super::*` pulls in the parent's
//! `StorageState` / `project` / `state_body` fixture + the egui/Style re-exports;
//! the shared `state_body` topology stays in the parent so `menubar::tests` can
//! still reach it, and `menubar_coverage` addresses it by its `crate::storage`
//! path unchanged.
#![allow(clippy::panic)]

use super::*;
use mde_egui::egui::{pos2, vec2, Rect};
use mde_theme::brand::icons::{icon_image, IconId};

/// A `state/storage/<node>` progress body.
fn progress_body(
    host: &str,
    idx: usize,
    total: usize,
    kind: &str,
    at: u64,
    refused: bool,
) -> String {
    let state = if refused {
        r#"{"state":"refused","reason":"backs running VM db1"}"#
    } else {
        r#"{"state":"applied"}"#
    };
    format!(
        r#"{{"host":"{host}","device":"/dev/sdb","op_index":{idx},"total":{total},"op_kind":"{kind}","state":{state},"published_at_ms":{at}}}"#
    )
}

/// Drive one headless 960×720 frame of the surface + tessellate it on the CPU —
/// the same `Context::run` → `tessellate` path the DRM runner drives, minus the
/// GPU. Returns whether it produced any draw primitives.
fn renders(state: &mut StorageState) -> bool {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 720.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| state.show(ui));
    });
    !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
}

fn storage_surface_shapes(state: &mut StorageState) -> Vec<egui::epaint::ClippedShape> {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 720.0))),
        ..Default::default()
    };
    ctx.run(input, |ctx| {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| state.show(ui));
    })
    .shapes
}

fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
    fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

fn painted_text_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Color32)> {
    fn text_color(text: &egui::epaint::TextShape) -> egui::Color32 {
        if let Some(color) = text.override_text_color {
            return color;
        }
        text.galley
            .job
            .sections
            .iter()
            .find_map(|section| {
                (section.format.color != egui::Color32::PLACEHOLDER).then_some(section.format.color)
            })
            .unwrap_or(text.fallback_color)
    }

    fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
        match shape {
            egui::Shape::Text(text) => out.push((text.galley.text().to_owned(), text_color(text))),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

fn painted_fill_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
        match shape {
            egui::Shape::Mesh(mesh) => {
                out.extend(mesh.vertices.iter().map(|vertex| vertex.color));
            }
            egui::Shape::Path(path) => {
                if path.fill != egui::Color32::TRANSPARENT {
                    out.push(path.fill);
                }
            }
            egui::Shape::Rect(rect) => {
                if rect.fill != egui::Color32::TRANSPARENT {
                    out.push(rect.fill);
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    walk(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

fn opaque_pixels(rgba: &[u8]) -> usize {
    rgba.chunks_exact(4).filter(|pixel| pixel[3] != 0).count()
}

#[test]
fn storage_queue_controls_do_not_paint_unicode_pseudo_icons() {
    assert_eq!(STORAGE_REFRESH_ICON, IconId::Reload);
    assert_eq!(STORAGE_STAGE_ICON, IconId::Add);
    assert_eq!(STORAGE_QUEUE_UP_ICON, IconId::ChevronUp);
    assert_eq!(STORAGE_QUEUE_DOWN_ICON, IconId::ArrowDown);
    assert_eq!(STORAGE_QUEUE_REMOVE_ICON, IconId::Close);

    let mut state = StorageState {
        nodes: project(&[state_body("this-node", 1, true)]),
        local_host: "this-node".to_string(),
        ..StorageState::default()
    };
    state.ensure_selection();
    state.select_node("this-node");
    state.selected_device = Some("/dev/sdb".to_string());
    state.queue.push(StorageOp::DeletePartition {
        partition: "/dev/sdb1".to_string(),
    });
    state.queue.push(StorageOp::SetLabel {
        partition: "/dev/sdb1".to_string(),
        label: "scratch".to_string(),
    });
    state.progress = project_progress(
        &[progress_body(
            "this-node",
            0,
            2,
            "delete_partition",
            3,
            true,
        )],
        "this-node",
    );

    let mut shapes = storage_surface_shapes(&mut state);
    let node = project(&[state_body("this-node", 1, true)])
        .into_iter()
        .next()
        .expect("fixture projects one node");
    let dev = node.topology.devices[1].clone();
    let mut compose = Compose::default();
    let mut compose_error = None;
    let mut staged = None;
    let mut queue = state.queue.clone();
    let mut arming = String::new();
    let mut apply = None;
    let progress = state.progress.clone();
    let mut goto_instances = false;
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 520.0))),
        ..Default::default()
    };
    shapes.extend(
        ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    show_compose(
                        ui,
                        Some(&dev),
                        &mut compose,
                        &mut compose_error,
                        &mut staged,
                    );
                    ui.separator();
                    show_queue_and_apply(ui, &node, &mut queue, &mut arming, None, &mut apply);
                    ui.separator();
                    show_progress(ui, &progress, &mut goto_instances);
                });
        })
        .shapes,
    );
    let mut progress_goto_instances = false;
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(520.0, 180.0))),
        ..Default::default()
    };
    shapes.extend(
        ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    show_progress(ui, &progress, &mut progress_goto_instances);
                });
        })
        .shapes,
    );
    let texts = painted_text(&shapes);
    for expected in [
        "Refresh topology",
        "Stage",
        "locked",
        "staging target",
        "Free the disk, then re-apply:",
    ] {
        assert!(
            texts.iter().any(|text| text == expected),
            "{expected:?} label was not painted: {texts:?}"
        );
    }
    assert!(
        texts.iter().all(|text| {
            !text.contains('\u{21BB}')
                && !text.contains('\u{FF0B}')
                && !text.contains('\u{2191}')
                && !text.contains('\u{2193}')
                && !text.contains('\u{2715}')
                && !text.contains('\u{1F512}')
                && !text.contains('\u{2699}')
        }),
        "Storage controls leaked Unicode pseudo-icons: {texts:?}"
    );
    for icon in [
        STORAGE_REFRESH_ICON,
        STORAGE_STAGE_ICON,
        STORAGE_QUEUE_UP_ICON,
        STORAGE_QUEUE_DOWN_ICON,
        STORAGE_QUEUE_REMOVE_ICON,
    ] {
        let img = icon_image(icon, 16, Style::TEXT.to_array())
            .unwrap_or_else(|err| panic!("{icon:?} failed to rasterize: {err}"));
        assert_eq!(img.width, 16, "{icon:?} icon width");
        assert_eq!(img.height, 16, "{icon:?} icon height");
        assert!(opaque_pixels(&img.rgba) > 0, "{icon:?} rasterized empty");
    }

    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 120.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                storage_tooltip(ui, "Move this operation earlier in the queue.");
            });
    });
    let tooltip_texts = painted_text_colors(&out.shapes);
    assert!(
        tooltip_texts.iter().any(|(text, color)| {
            text == "Move this operation earlier in the queue." && *color == Style::TEXT
        }),
        "Storage tooltip should paint themed text: {tooltip_texts:?}"
    );
    assert!(
        !tooltip_texts.iter().any(|(text, color)| {
            text == "Move this operation earlier in the queue." && *color == egui::Color32::BLACK
        }),
        "Storage tooltip leaked raw black text: {tooltip_texts:?}"
    );
    let tooltip_fills = painted_fill_colors(&out.shapes);
    assert!(
        tooltip_fills.contains(&Style::SURFACE),
        "Storage tooltip should paint its themed surface: {tooltip_fills:?}"
    );
}

#[test]
fn storage_tooltip_frame_sits_on_the_shared_radius_ladder() {
    // PLATFORM-INTERFACES Q19/Q20 — the tooltip card rounds on the shared
    // RADIUS_S tier of the §4 ladder, asserted off the painted shape so a raw
    // `CornerRadius::same(..)` corner literal can't silently come back.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 120.0))),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                storage_tooltip(ui, "Stage this storage operation.");
            });
    });
    fn walk(shape: &egui::Shape, out: &mut Vec<(egui::CornerRadius, egui::Color32)>) {
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
fn storage_action_buttons_use_refined_chrome_height() {
    assert_eq!(
        STORAGE_ACTION_BUTTON_H,
        Style::TOOLBAR_CONTROL_H,
        "Storage action controls should share the refined toolbar visual height"
    );
    assert!(
        STORAGE_ACTION_BUTTON_H < Style::SP_L,
        "Storage action controls should stay slimmer than the old 24pt button row"
    );
    assert_eq!(
        STORAGE_ACTION_PAD_Y,
        (STORAGE_ACTION_BUTTON_H - Style::SP_M) * 0.5,
        "Storage icon+label padding keeps a 16pt icon inside the refined height"
    );

    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut heights = Vec::new();
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(360.0, 80.0))),
        ..Default::default()
    };
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                heights.push(
                    storage_icon_label_button(ui, STORAGE_REFRESH_ICON, "Refresh topology")
                        .rect
                        .height(),
                );
                heights.push(
                    storage_icon_button(ui, STORAGE_QUEUE_REMOVE_ICON, "Remove", true)
                        .rect
                        .height(),
                );
            });
        });
    });

    assert!(
        heights
            .iter()
            .all(|height| (*height - STORAGE_ACTION_BUTTON_H).abs() <= f32::EPSILON),
        "Storage action button heights should be {STORAGE_ACTION_BUTTON_H}, got {heights:?}"
    );
}

#[test]
fn project_folds_one_row_per_host_sorted_latest_wins() {
    let bodies = vec![
        state_body("node-b", 1, true),
        state_body("node-a", 5, false),
        state_body("node-a", 9, true), // newer wins for node-a
    ];
    let nodes = project(&bodies);
    assert_eq!(nodes.len(), 2, "one row per host");
    assert_eq!(nodes[0].host, "node-a", "BTreeMap key order → host-sorted");
    assert_eq!(nodes[1].host, "node-b");
    assert!(
        nodes[0].available(),
        "the newer node-a mirror (available) wins"
    );
    assert_eq!(nodes[0].published_at_ms, 9);
    assert_eq!(nodes[0].topology.devices.len(), 2);
}

#[test]
fn project_skips_malformed_bodies() {
    let bodies = vec![
        "not json".to_string(),
        "{}".to_string(), // missing required fields
        state_body("node-a", 1, true),
    ];
    let nodes = project(&bodies);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].host, "node-a");
}

#[test]
fn protected_reason_derives_the_root_boot_and_mesh_walls() {
    let nodes = project(&[state_body("n", 1, true)]);
    let sda = &nodes[0].topology.devices[0];
    let sdb = &nodes[0].topology.devices[1];
    // /dev/sda has / and /boot/efi mounted → root/boot/EFI protection wins.
    assert_eq!(
        sda.protected_reason(),
        Some("backs the node's root / boot / EFI chain")
    );
    // /dev/sdb is a plain removable data disk → unprotected (stageable).
    assert!(sdb.protected_reason().is_none());

    // A mesh-storage backer is protected with its own reason.
    let mesh = r#"{"host":"n","backend":{"status":"available"},"topology":{"devices":[
          {"name":"/dev/sdc","size_mib":1024,"partitions":[
            {"name":"/dev/sdc1","number":1,"start_mib":1,"size_mib":1000,"filesystem":"xfs","mountpoint":"/mnt/mesh-storage"}]}
        ]},"published_at_ms":1}"#;
    let m = project(&[mesh.to_string()]);
    assert_eq!(
        m[0].topology.devices[0].protected_reason(),
        Some("backs /mnt/mesh-storage (the mesh shared volume)")
    );
}

#[test]
fn free_mib_is_total_minus_used() {
    let nodes = project(&[state_body("n", 1, true)]);
    let sdb = &nodes[0].topology.devices[1];
    assert_eq!(sdb.free_mib(), 16384 - 8192);
}

/// The fixture's free data disk: /dev/sdb, 16384 MiB, one 8192 MiB ext4
/// partition starting at 1 MiB → 8192 MiB free.
fn sdb() -> BlockDevice {
    project(&[state_body("n", 1, true)])[0].topology.devices[1].clone()
}

#[test]
fn compose_builds_each_op_kind_to_the_worker_shape() {
    let dev = sdb();
    let as_json = |op: &StorageOp| -> serde_json::Value {
        serde_json::from_str(&serde_json::to_string(op).unwrap_or_default()).unwrap_or_default()
    };

    // New partition (blank size → fills free space), ext4 + label.
    let mut new_part = Compose {
        kind: OpKind::NewPartition,
        fs: Some(Filesystem::Ext4),
        label: "scratch".to_string(),
        ..Compose::default()
    };
    let json = as_json(&new_part.build(&dev).expect("new partition builds"));
    assert_eq!(json["op"], "create_partition");
    assert_eq!(json["device"], "/dev/sdb");
    assert_eq!(json["size_mib"], 8192, "blank size fills the free space");
    assert_eq!(json["filesystem"], "ext4");
    assert_eq!(json["label"], "scratch");

    // Explicit oversize is refused.
    new_part.size_mib = "9999".to_string();
    assert!(new_part.build(&dev).is_err(), "oversize is refused");

    // New table.
    let new_table = Compose {
        kind: OpKind::NewTable,
        table: PartTable::Gpt,
        ..Compose::default()
    };
    let json = as_json(&new_table.build(&dev).expect("table builds"));
    assert_eq!(json["op"], "create_table");
    assert_eq!(json["table"], "gpt");

    // Delete needs a partition.
    let mut del = Compose {
        kind: OpKind::Delete,
        ..Compose::default()
    };
    assert!(
        del.build(&dev).is_err(),
        "delete without a partition is refused"
    );
    del.partition = "/dev/sdb1".to_string();
    let del_op = del.build(&dev).expect("delete builds");
    assert_eq!(
        del_op,
        StorageOp::DeletePartition {
            partition: "/dev/sdb1".to_string()
        }
    );

    // Format requires a filesystem.
    let mut fmt = Compose {
        kind: OpKind::Format,
        partition: "/dev/sdb1".to_string(),
        fs: None,
        ..Compose::default()
    };
    assert!(fmt.build(&dev).is_err(), "format without a fs is refused");
    fmt.fs = Some(Filesystem::Xfs);
    let json = as_json(&fmt.build(&dev).expect("format builds"));
    assert_eq!(json["op"], "format");
    assert_eq!(json["filesystem"], "xfs");
}

#[test]
fn compose_resize_picks_the_worker_direction_from_the_live_size() {
    let dev = sdb(); // sdb1 is 8192 MiB; 8192 MiB free.
    let as_json = |op: &StorageOp| -> serde_json::Value {
        serde_json::from_str(&serde_json::to_string(op).unwrap_or_default()).unwrap_or_default()
    };
    let mut rz = Compose {
        kind: OpKind::Resize,
        partition: "/dev/sdb1".to_string(),
        size_mib: "12288".to_string(),
        ..Compose::default()
    };
    // Larger than current → the worker's `grow` shape, verbatim.
    let json = as_json(&rz.build(&dev).expect("grow builds"));
    assert_eq!(json["op"], "grow");
    assert_eq!(json["partition"], "/dev/sdb1");
    assert_eq!(json["new_size_mib"], 12288);
    // Smaller than current → `shrink`.
    rz.size_mib = "4096".to_string();
    let json = as_json(&rz.build(&dev).expect("shrink builds"));
    assert_eq!(json["op"], "shrink");
    assert_eq!(json["new_size_mib"], 4096);
    // The same size, an over-growth past free space, and an off-disk
    // partition are refused with typed reasons (the worker re-checks).
    rz.size_mib = "8192".to_string();
    assert!(rz.build(&dev).is_err(), "no-op resize is refused");
    rz.size_mib = "17000".to_string();
    assert!(rz.build(&dev).is_err(), "growth past free space is refused");
    rz.size_mib = "12288".to_string();
    rz.partition = "/dev/sdb9".to_string();
    assert!(rz.build(&dev).is_err(), "an unknown partition is refused");
}

#[test]
fn compose_move_builds_the_worker_shape_and_refuses_a_no_op() {
    let dev = sdb(); // sdb1 starts at 1 MiB.
    let mut mv = Compose {
        kind: OpKind::Move,
        partition: "/dev/sdb1".to_string(),
        new_start_mib: "4096".to_string(),
        ..Compose::default()
    };
    let json: serde_json::Value = serde_json::from_str(
        &serde_json::to_string(&mv.build(&dev).expect("move builds")).unwrap_or_default(),
    )
    .unwrap_or_default();
    assert_eq!(json["op"], "move");
    assert_eq!(json["partition"], "/dev/sdb1");
    assert_eq!(json["new_start_mib"], 4096);
    mv.new_start_mib = "1".to_string();
    assert!(
        mv.build(&dev).is_err(),
        "moving to the current start is a no-op"
    );
}

#[test]
fn geometry_lines_derive_sectors_and_cylinders_from_the_real_size() {
    let dev = sdb(); // 16384 MiB = 33_554_432 × 512 B sectors.
    let [geometry, rollup] = geometry_lines(&dev);
    assert!(geometry.contains("33554432 sectors"), "{geometry}");
    // 17_179_869_184 B / (255 × 63 × 512 B per cylinder) = 2088 full cylinders.
    assert!(geometry.contains("2088 cylinders"), "{geometry}");
    assert!(geometry.contains("derived"), "derived figures say so (§7)");
    assert!(rollup.contains("GPT"), "{rollup}");
    assert!(rollup.contains("1 partition(s)"), "{rollup}");
    assert!(rollup.contains("8192 MiB free of 16384 MiB"), "{rollup}");
}

#[test]
fn armed_apply_request_demands_the_exact_typed_echo() {
    let nodes = project(&[state_body("n", 1, true)]);
    let node = &nodes[0];
    let queue = vec![StorageOp::DeletePartition {
        partition: "/dev/sdb1".to_string(),
    }];
    assert!(
        armed_apply_request(node, &[], "/dev/sdb").is_none(),
        "an empty queue never arms"
    );
    assert!(
        armed_apply_request(node, &queue, "").is_none(),
        "no echo, no request"
    );
    assert!(
        armed_apply_request(node, &queue, "/dev/sda").is_none(),
        "the wrong disk never arms"
    );
    let req = armed_apply_request(node, &queue, "  /dev/sdb  ")
        .expect("the exact echo (whitespace-trimmed) arms");
    let StorageRequest::Apply {
        armed_device,
        queue: q,
        ..
    } = req
    else {
        panic!("an armed request is an Apply");
    };
    assert_eq!(armed_device, "/dev/sdb");
    assert_eq!(q.ops.len(), 1);
}

#[test]
fn queue_target_is_single_disk_or_a_typed_reason() {
    let nodes = project(&[state_body("n", 1, true)]);
    let topo = &nodes[0].topology;

    // Empty → no target.
    assert!(queue_target(&[], topo).is_err());

    // One disk's ops → that disk.
    let ops = vec![
        StorageOp::Format {
            partition: "/dev/sdb1".to_string(),
            filesystem: Filesystem::Ext4,
            label: None,
        },
        StorageOp::CreatePartition {
            device: "/dev/sdb".to_string(),
            start_mib: 0,
            size_mib: 100,
            filesystem: None,
            label: None,
        },
    ];
    assert_eq!(queue_target(&ops, topo).as_deref(), Ok("/dev/sdb"));

    // Spanning two disks → refused (arming is per-disk, mirrors the worker).
    let spanning = vec![
        StorageOp::Unmount {
            partition: "/dev/sda2".to_string(),
        },
        StorageOp::Unmount {
            partition: "/dev/sdb1".to_string(),
        },
    ];
    assert!(
        queue_target(&spanning, topo).is_err(),
        "a multi-disk queue can't be armed"
    );
}

#[test]
fn apply_request_serializes_to_the_worker_verb_shape() {
    let nodes = project(&[state_body("n", 1, true)]);
    let req = StorageRequest::Apply {
        armed_device: "/dev/sdb".to_string(),
        staged: nodes[0].topology.clone(),
        queue: StorageQueue {
            ops: vec![StorageOp::DeletePartition {
                partition: "/dev/sdb1".to_string(),
            }],
        },
    };
    let v: serde_json::Value = serde_json::from_str(&req.to_body()).unwrap_or_default();
    assert_eq!(v["verb"], "apply");
    assert_eq!(v["armed_device"], "/dev/sdb");
    assert_eq!(v["queue"]["ops"][0]["op"], "delete_partition");
    assert!(
        v["staged"]["devices"].is_array(),
        "the drift baseline rides the verb"
    );

    let refresh: serde_json::Value =
        serde_json::from_str(&StorageRequest::Refresh.to_body()).unwrap_or_default();
    assert_eq!(refresh["verb"], "refresh");
}

#[test]
fn apply_publish_body_is_schema_v1_and_exactly_scoped() {
    let nodes = project(&[state_body("node-a", 1, true)]);
    let req = StorageRequest::Apply {
        armed_device: "/dev/sdb".to_string(),
        staged: nodes[0].topology.clone(),
        queue: StorageQueue {
            ops: vec![StorageOp::DeletePartition {
                partition: "/dev/sdb1".to_string(),
            }],
        },
    };
    let body = request_body_for_publish("node-a", &req).expect("test signer mints");
    let value: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["schema_version"], 1);
    let token = mackes_mesh_types::cloud::CloudArmedToken::parse(
        value["armed_token"].as_str().expect("capability"),
    )
    .expect("well-formed capability");
    assert_eq!(token.verb, "storage-apply");
    assert_eq!(token.node, "node-a");
    assert_eq!(token.target, "/dev/sdb");

    let refresh = request_body_for_publish("node-a", &StorageRequest::Refresh).unwrap();
    let refresh: serde_json::Value = serde_json::from_str(&refresh).unwrap();
    assert_eq!(refresh["schema_version"], 1);
    assert!(refresh.get("armed_token").is_none());
}

#[test]
fn project_progress_keeps_latest_per_op_ordered() {
    let host = "node-a";
    let lane = vec![
        progress_body(host, 0, 2, "unmount", 5, false),
        progress_body(host, 1, 2, "format", 6, true),
        progress_body(host, 0, 2, "unmount", 9, false), // newer for op 0
        progress_body("other", 0, 2, "unmount", 99, false), // wrong host
    ];
    let rows = project_progress(&lane, host);
    assert_eq!(rows.len(), 2, "one row per op index, other host dropped");
    assert_eq!(rows[0].op_index, 0);
    assert_eq!(rows[0].published_at_ms, 9, "the newer op-0 event wins");
    assert!(matches!(rows[1].state, ProgressState::Refused { .. }));
}

#[test]
fn empty_surface_renders_the_honest_state() {
    let mut s = StorageState {
        bus_root: None,
        ..StorageState::default()
    };
    assert!(s.nodes.is_empty());
    assert!(renders(&mut s), "the empty state still fully paints");
}

#[test]
fn live_surface_mounts_and_tessellates() {
    // Feed the projection directly (bypassing the Bus IO) and select the
    // removable data disk so the compose form + queue paths are all reachable,
    // then prove the whole surface tessellates headless — with both View
    // toggles on, so the device rail + geometry detail paint too (MENU-4).
    let mut s = StorageState {
        nodes: project(&[state_body("this-node", 1, true)]),
        local_host: "this-node".to_string(),
        view_rail: true,
        view_geometry: true,
        ..StorageState::default()
    };
    s.ensure_selection();
    s.select_node("this-node");
    s.selected_device = Some("/dev/sdb".to_string());
    s.queue.push(StorageOp::DeletePartition {
        partition: "/dev/sdb1".to_string(),
    });
    s.progress = project_progress(
        &[progress_body(
            "this-node",
            0,
            1,
            "delete_partition",
            3,
            true,
        )],
        "this-node",
    );
    assert!(
        renders(&mut s),
        "the live Storage surface produced no draw primitives"
    );
}

#[test]
fn unavailable_backend_renders_the_typed_not_available_state() {
    let mut s = StorageState {
        nodes: project(&[state_body("n", 1, false)]),
        local_host: "n".to_string(),
        ..StorageState::default()
    };
    s.ensure_selection();
    let node = s.selected().expect("a node is selected");
    assert!(!node.available(), "the backend is unavailable");
    assert!(renders(&mut s), "the unavailable state still fully paints");
}

#[test]
fn cloud_compat_deep_link_resolves_through_the_shell_nav_grammar() {
    // The walled-row deep-link keeps the old `instances` verb for forward
    // compatibility; WL-ARCH-006 routes it to the unified Workloads surface
    // (Infra as Code — the retired Cloud plane's successor).
    assert!(matches!(
        crate::toast_bridge::resolve_action(&format!("shell/goto/{CLOUD_COMPAT_SURFACE}")),
        Some(crate::toast_bridge::Navigate::Surface(
            crate::surfaces::Surface::InfraCode
        ))
    ));
}

#[test]
fn selecting_a_new_peer_clears_the_queue() {
    let mut s = StorageState {
        nodes: project(&[state_body("a", 1, true), state_body("b", 1, true)]),
        ..StorageState::default()
    };
    s.select_node("a");
    s.queue.push(StorageOp::Unmount {
        partition: "/dev/sdb1".to_string(),
    });
    s.arming = "/dev/sdb".to_string();
    s.select_node("b");
    assert!(
        s.queue.is_empty(),
        "switching peers clears the per-node queue"
    );
    assert!(s.arming.is_empty(), "and the arming echo");
    assert_eq!(s.selected_node.as_deref(), Some("b"));
}

/// Phase-C depth adoption — each GParted-style disk card carries the shared
/// [`Elevation::Raised`](mde_egui::style::Elevation) soft shadow verbatim from the
/// token (no hand-rolled colour, design lock #2 / §4). The surface-side conversion
/// must reproduce the token's offset/blur/spread and its exact translucent umbra,
/// and cast a real (non-zero) shadow so the card reads as genuinely lifted.
#[test]
fn disk_card_wears_the_raised_elevation_token() {
    let token = mde_egui::style::Elevation::Raised.shadow();
    let shadow = disk_card_shadow();
    assert_eq!(
        shadow.color, token.umbra,
        "the disk card's umbra comes straight from the token — no minted colour"
    );
    assert_eq!(
        shadow.offset,
        [token.offset[0] as i8, token.offset[1] as i8]
    );
    assert_eq!(shadow.blur, token.blur as u8);
    assert_eq!(shadow.spread, token.spread as u8);
    assert!(
        shadow.color.a() > 0 && shadow.blur > 0,
        "Raised casts a real, soft, translucent shadow — the card is lifted off the page"
    );
}

/// MENU-6 — the **menubar coverage backstop**: no workspace ships bare again.
///
/// Every routed [`Surface`](crate::surfaces::Surface) is enumerated against ONE
/// recorded register: it either fronts the shared `MenuBarModel` (with its
/// recorded, non-empty bar title) or sits on the explicit exemption list with the
/// reason + its MENUBAR-SWEEP follow-on. The register's `match` is deliberately
/// exhaustive (no wildcard arm), so adding a `Surface` variant without recording
/// its menubar posture **fails this crate's build** — the enforcement is the
/// compiler, not a reviewer's memory. On top of the register, the surfaces whose
/// states are cheaply constructible are driven through a REAL headless frame and
/// their UPPERCASE bar title asserted in the emitted text shapes, tying the
/// register to rendered reality (the rest are recorded with why a headless
/// construction isn't reachable from a crate-local test).
///
/// This module lives in `storage.rs` because `mde-shell-egui` is a binary-only
/// crate (no lib target): an integration test under `tests/` can't reach the
/// crate's private modules, and a dedicated `src/menubar_coverage.rs` would need
/// a `mod` line in `main.rs` — out of this unit's blast radius. The register is
/// surface-agnostic; only its file placement is a compromise.
#[cfg(test)]
#[allow(clippy::panic)]
mod menubar_coverage {
    use crate::surfaces::Surface;

    /// The recorded menubar posture of one routed surface.
    enum Coverage {
        /// The surface fronts the shared bar — its recorded title.
        Covered { title: &'static str },
        /// The surface deliberately owns first-party chrome instead of the shared bar.
        FirstPartyChrome { reason: &'static str },
        /// The surface is currently bare — the recorded reason + follow-on.
        Exempt { reason: &'static str },
    }

    /// The ONE recorded decision per routed `Surface` (exhaustive on purpose).
    const fn coverage(surface: Surface) -> Coverage {
        match surface {
            // ── covered: the MENUBAR-ALL / MENUBAR-SWEEP bars ──
            Surface::Workbench => Coverage::Covered {
                title: "State of the Mesh", // MENU-1 (workbench.rs)
            },
            Surface::InfraCode => Coverage::Covered {
                title: "Infra as Code", // IAC-5 (iac.rs)
            },
            Surface::Desktop => Coverage::Exempt {
                reason: "bare — the Remote Sessions workspace is a full-screen \
                         session picker / remote desktop surface with no workspace \
                         menu bar",
            },
            Surface::Browser => Coverage::FirstPartyChrome {
                reason: "BROWSER-CHROME C0 — Browser retired the shared MENUBAR-ALL \
                         top strip; first-party tabs, toolbar, omnibox, and menu \
                         button own this surface's chrome",
            },
            Surface::Bookmarks => Coverage::Exempt {
                reason: "bare — mde-bookmarks-egui mounts with its own manager \
                         header; folding it onto the shared bar is a MENUBAR-SWEEP \
                         follow-on",
            },
            Surface::MapsLocation => Coverage::FirstPartyChrome {
                reason: "MAPS-LOCATION-1 — Maps & Location owns a native tab rail, \
                         map canvas, driving dashboard, MG90 setup, and simulator \
                         chrome instead of the shared MENUBAR-ALL top strip",
            },
            Surface::System => Coverage::Covered { title: "System" },
            Surface::Storage => Coverage::Covered {
                title: "Local Cylinders", // MENU-4 (this file)
            },
            Surface::About => Coverage::Covered {
                title: "About", // MENU-5 / DEVMGR (device_manager.rs)
            },
            // ── recorded exemptions: bare today, each a MENUBAR-SWEEP follow-on ──
            Surface::MeshView => Coverage::Exempt {
                reason: "bare — the Mesh Map canvas renders headerless; a bar \
                         (layout toggles, fold source) is a MENUBAR-SWEEP follow-on",
            },
            Surface::Explorer => Coverage::Exempt {
                reason: "bare — the Explorer discovery hero card renders headerless \
                         (filters/actions live on the card itself); a shared bar is \
                         a MENUBAR-SWEEP follow-on",
            },
            Surface::Music => Coverage::Exempt {
                reason: "bare — mde-music-egui mounts with its own header; folding \
                         it onto the shared bar is a MENUBAR-SWEEP follow-on",
            },
            Surface::Media => Coverage::Exempt {
                reason: "bare — mde-media-egui mounts with its own header; folding \
                         it onto the shared bar is a MENUBAR-SWEEP follow-on",
            },
            Surface::Files => Coverage::Exempt {
                reason: "bare — mde-files-egui mounts with its own header; folding \
                         it onto the shared bar is a MENUBAR-SWEEP follow-on",
            },
            Surface::Phones => Coverage::Exempt {
                reason: "bare — the KDC-MESH-9 Phones hub carries its own tab header \
                         (Phones · Files · Commands · Pair); folding it onto the \
                         shared bar is a MENUBAR-SWEEP follow-on",
            },
            Surface::Communications => Coverage::Exempt {
                reason: "bare — the WL-FUNC-011 Communications surface carries its own \
                         frame (spaces rail · per-space mode tabs · persistent call \
                         bar) instead of the shared MENUBAR-ALL top strip; folding it \
                         onto the shared bar is a MENUBAR-SWEEP follow-on",
            },
            Surface::Terminal => Coverage::Exempt {
                reason: "bare — mde-term-egui carries its own tmux/session menu \
                         strip; migrating it onto the shared bar is a MENUBAR-SWEEP \
                         follow-on",
            },
            Surface::Timers => Coverage::Exempt {
                reason: "bare — the clock-owned Timers & Alarms surface is \
                         deliberately chrome-light; a bar is a MENUBAR-SWEEP \
                         follow-on",
            },
            Surface::AutoHome => Coverage::Exempt {
                reason: "bare — the Auto Mode home (AUTO-HOME) is a full-bleed \
                         glanceable tile launcher with no workspace menu bar by \
                         design (Car Mode is chrome-light)",
            },
        }
    }

    /// Routed operator-reachable views that are NOT `Surface` variants (the
    /// pre-session / overlay screens), inventoried here so the MENU-6 sweep list
    /// is complete. Each is bare today; each entry records why + the follow-on.
    const ROUTED_NON_SURFACE_VIEWS: [(&str, &str); 2] = [
        (
            "explorer/discovery",
            "bare — the Explorer/Discovery flow renders its own headers; folding \
             onto the shared bar is a MENUBAR-SWEEP follow-on",
        ),
        (
            "chooser",
            "bare — the pre-session Desktop Chooser is a full-screen picker with \
             no workspace chrome; a bar is a MENUBAR-SWEEP follow-on",
        ),
    ];

    /// Every routed surface: the picker set plus the clock-cell Timers surface
    /// (deliberately outside `Surface::ALL`, still routed by the dock).
    fn every_routed() -> Vec<Surface> {
        let mut all = Surface::ALL.to_vec();
        all.push(Surface::Timers);
        all.push(Surface::AutoHome);
        all
    }

    #[test]
    fn every_routed_surface_records_a_menubar_posture() {
        let mut covered = 0usize;
        let mut first_party = 0usize;
        let mut exempt = 0usize;
        for surface in every_routed() {
            match coverage(surface) {
                Coverage::Covered { title } => {
                    assert!(
                        !title.trim().is_empty(),
                        "{surface:?}: a covered surface records a non-empty bar title"
                    );
                    covered += 1;
                }
                Coverage::FirstPartyChrome { reason } => {
                    assert!(
                        reason.contains("BROWSER-CHROME") || reason.contains("MAPS-LOCATION"),
                        "{surface:?}: first-party chrome records its owning epic"
                    );
                    first_party += 1;
                }
                Coverage::Exempt { reason } => {
                    assert!(
                        reason.contains("MENUBAR-SWEEP")
                            || reason.contains("no workspace menu bar"),
                        "{surface:?}: an exemption names its follow-on, not just a shrug"
                    );
                    exempt += 1;
                }
            }
        }
        assert_eq!(covered + first_party + exempt, every_routed().len());
        assert_eq!(
            covered, 5,
            "the shared covered set is the five landed bars (WL-FUNC-011 Phase-2 \
             retired the Chat bar)"
        );
        assert_eq!(
            first_party, 2,
            "Browser and Maps & Location are the routed first-party chrome surfaces"
        );
        for (view, reason) in ROUTED_NON_SURFACE_VIEWS {
            assert!(
                reason.contains("MENUBAR-SWEEP"),
                "{view}: a non-Surface view exemption names its follow-on"
            );
        }
    }

    #[test]
    fn the_bare_inventory_is_exactly_the_recorded_follow_on_set() {
        let bare: Vec<Surface> = every_routed()
            .into_iter()
            .filter(|s| matches!(coverage(*s), Coverage::Exempt { .. }))
            .collect();
        assert_eq!(
            bare,
            [
                Surface::MeshView,
                Surface::Explorer,
                Surface::Desktop,
                Surface::Music,
                Surface::Media,
                Surface::Files,
                Surface::Bookmarks,
                Surface::Terminal,
                Surface::Phones,
                // WL-FUNC-011 — the Communications hub carries its own frame
                // (rail · mode tabs · call bar), a MENUBAR-SWEEP follow-on. Sits
                // here in `Surface::ALL` order (the twentieth surface), before the
                // out-of-ALL Timers `every_routed` appends.
                Surface::Communications,
                Surface::Timers,
                // AUTO-HOME — the out-of-ALL Auto Mode home, appended after Timers
                // by `every_routed`; a full-bleed Car-Mode tile launcher, bare by
                // design.
                Surface::AutoHome,
            ],
            "a surface leaving (or joining) the bare set updates this inventory \
             consciously — that's the backstop"
        );
    }

    // ── the register is tied to rendered reality where a crate-local test can ──

    /// Drive one headless frame and collect every text run the surface painted
    /// (the same `Context::run` path the DRM runner drives, minus the GPU).
    fn rendered_text(mut run: impl FnMut(&mut mde_egui::egui::Ui)) -> String {
        use mde_egui::egui;
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
        mde_egui::Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 800.0),
            )),
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

    /// The two surfaces whose states construct cheaply from here render for
    /// real, and each bar's UPPERCASE DISPLAY title appears in the painted text —
    /// the register's `Covered` claim proven at the pixel-feed level for them.
    /// The other three covered bars (Workbench / `IaC` / System)
    /// need the shell's full wiring or testkit scaffolding owned by their own
    /// files' tests, so their register rows rest on those files' render tests.
    /// (The Chat bar was dropped here — WL-FUNC-011 Phase-2 retired that surface.)
    #[test]
    fn covered_titles_render_on_the_cheaply_constructible_bars() {
        let proofs: [(Surface, fn() -> String); 2] = [
            (Surface::Storage, || {
                let mut s = crate::storage::StorageState {
                    nodes: crate::storage::project(&[crate::storage::state_body("nodeA", 1, true)]),
                    bus_root: None,
                    ..crate::storage::StorageState::default()
                };
                rendered_text(|ui| s.show(ui))
            }),
            (Surface::About, || {
                let mut s = crate::device_manager::DeviceManagerState::default();
                rendered_text(|ui| s.show(ui))
            }),
        ];
        for (surface, render) in proofs {
            let Coverage::Covered { title } = coverage(surface) else {
                panic!("{surface:?} is registered Covered");
            };
            let text = render();
            assert!(
                text.contains(&title.to_uppercase()),
                "{surface:?}: the live bar paints \u{201C}{}\u{201D} (register: {title})",
                title.to_uppercase()
            );
        }
    }
}
