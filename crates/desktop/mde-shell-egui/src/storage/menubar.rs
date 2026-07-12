//! MENU-4 — the **Local Cylinders** bar: the platform's `GParted` spine over the
//! storage plane.
//!
//! The spine mirrors `GParted`'s own menu order (app · Edit · View · Device ·
//! Partition · Help), with **Peer** in the app slot (the mesh dimension `GParted`
//! never had). Every item is the mouse twin of a seam the surface already drives
//! (§6, one path): **Peer** switches the active node (`select_node`); **Edit**
//! owns the pending queue — Undo Last / Clear All, and **Apply All Operations**,
//! which shares the inline button's exact typed-arming decision
//! ([`super::armed_apply_request`], lock 8) so the menu can never bypass the
//! typed confirm; **View** toggles the device rail + derived-geometry detail;
//! **Device** rescans the topology (`Refresh`) and stages a new partition table;
//! **Partition** stages every partition verb (new / delete / resize-move /
//! format-to‹fs› / mount-unmount / label) by jumping the composer — each staged
//! op only ever reaches a disk through the typed-armed Apply; **Help** carries
//! the surface identity and publishes the safety posture on the live toast lane.
//! Each entry is honestly gated (§7): Peer omits itself until a peer publishes,
//! destructive verbs grey without an unlocked target disk, Mount/Unmount grey
//! without a partition in the matching state, Apply All greys until the echo is
//! typed — never a dead entry. Chips: fleet rollup · peer health · the selected
//! device · the pending-op count.

use super::{BlockDevice, Filesystem, OpKind, StorageRequest, StorageState, DOT};
use mde_egui::egui::Ui;
use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
use mde_egui::{ChipTone, StatusChip, Style};

/// One menu action — each routes to a real Storage seam in [`apply`]. Owned (a
/// peer id is a `String`), so `Clone` (not `Copy`) satisfies the shared bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MenuAction {
    /// Switch the active peer (the picker's `select_node` seam).
    SelectPeer(String),
    /// Re-publish the selected peer's live topology (`StorageRequest::Refresh`).
    RescanDevices,
    /// Pop the most recently staged op (`GParted`'s Undo Last Operation).
    UndoLast,
    /// Drop the staged op queue + its arming echo.
    ClearQueue,
    /// Publish the typed-armed Apply (enabled only while the echo matches).
    ApplyAll,
    /// Toggle the View → Device Rail.
    ToggleRail,
    /// Toggle the View → Geometry / Cylinder Detail.
    ToggleGeometry,
    /// Jump the compose form to this op kind (its own dropdown seam).
    StageKind(OpKind),
    /// Jump the compose form to Format with this filesystem preset
    /// (`GParted`'s "Format to ›" submenu).
    StageFormat(Filesystem),
    /// Publish the safety-posture note on the toast lane (Help).
    HelpSafety,
}

/// Render the LOCAL CYLINDERS bar and return the action picked this frame.
pub(super) fn show(state: &StorageState, ui: &mut Ui) -> Option<MenuAction> {
    let menus = build_menus(state);
    let status = build_status(state);
    let model = MenuBarModel {
        // The operator's name for the platform's GParted (MENU-4). Storage
        // sits in the dock's "System" group (gold), so the title wears that
        // categorical accent (lock 2).
        title: "Local Cylinders",
        accent: Style::ACCENT_SYSTEM,
        menus: &menus,
        status: &status,
    };
    MenuBar::show(ui, &model)
}

/// The selected disk's live row, if any.
fn selected_disk(state: &StorageState) -> Option<BlockDevice> {
    let name = state.selected_device.as_deref()?;
    state
        .selected_devices()
        .into_iter()
        .find(|d| d.name == name)
}

/// Build the `GParted` spine from live state, each entry honestly gated (§7).
fn build_menus(state: &StorageState) -> Vec<Menu<MenuAction>> {
    let mut menus = Vec::new();

    // Peer — one radio per published node (omitted entirely until a peer lands).
    if !state.nodes.is_empty() {
        let peers: Vec<Entry<MenuAction>> = state
            .nodes
            .iter()
            .map(|n| {
                let checked = state.selected_node.as_deref() == Some(n.host.as_str());
                let label = if n.host == state.local_host {
                    format!("{} (this node)", n.host)
                } else {
                    n.host.clone()
                };
                Entry::Item(
                    Item::new(MenuAction::SelectPeer(n.host.clone()), label).checked(checked),
                )
            })
            .collect();
        menus.push(Menu::new("Peer", peers));
    }

    menus.push(build_edit_menu(state));
    menus.push(build_view_menu(state));
    menus.push(build_device_menu(state));
    menus.push(build_partition_menu(state));
    menus.push(build_help_menu(state));
    menus
}

/// **Edit** — the pending-queue verbs (`GParted`'s Edit menu, lock 8 intact):
/// Undo Last / Clear All need a queue; Apply All shares the inline button's
/// typed-arming decision and greys until the echo matches.
fn build_edit_menu(state: &StorageState) -> Menu<MenuAction> {
    let staged = !state.queue.is_empty();
    let armed = state.armed_apply().is_some();
    let mut entries = vec![
        Entry::Item(Item::new(MenuAction::UndoLast, "Undo Last Operation").enabled(staged)),
        Entry::Item(Item::new(MenuAction::ClearQueue, "Clear All Operations").enabled(staged)),
        Entry::Separator,
        Entry::Item(Item::new(MenuAction::ApplyAll, "Apply All Operations").enabled(armed)),
    ];
    if staged && !armed {
        // An honest caption, not a dead item: why Apply is grey right now.
        entries.push(Entry::Caption(
            "Type the target device below to arm Apply.".to_string(),
        ));
    }
    Menu::new("Edit", entries)
}

/// **View** — the device rail + derived-geometry toggles; greyed until the
/// selected peer has published a disk to show.
fn build_view_menu(state: &StorageState) -> Menu<MenuAction> {
    let has_disks = !state.selected_devices().is_empty();
    Menu::new(
        "View",
        vec![
            Entry::Item(
                Item::new(MenuAction::ToggleRail, "Device Rail")
                    .checked(state.view_rail)
                    .enabled(has_disks),
            ),
            Entry::Item(
                Item::new(MenuAction::ToggleGeometry, "Geometry / Cylinder Detail")
                    .checked(state.view_geometry)
                    .enabled(has_disks),
            ),
        ],
    )
}

/// **Device** — whole-disk verbs: rescan (the worker's `Refresh`), and a new
/// partition table staged through the composer (typed-armed at Apply).
fn build_device_menu(state: &StorageState) -> Menu<MenuAction> {
    let disk = selected_disk(state);
    let stageable = disk
        .as_ref()
        .is_some_and(|d| d.protected_reason().is_none());
    Menu::new(
        "Device",
        vec![
            Entry::Caption(disk.as_ref().map_or_else(
                || "No disk selected.".to_string(),
                |d| format!("Selected: {}", d.name),
            )),
            Entry::Item(
                Item::new(MenuAction::RescanDevices, "Rescan Devices")
                    .enabled(state.selected_node.is_some()),
            ),
            Entry::Separator,
            Entry::Item(
                Item::new(
                    MenuAction::StageKind(OpKind::NewTable),
                    "New Partition Table\u{2026}",
                )
                .enabled(stageable),
            ),
        ],
    )
}

/// **Partition** — the full `GParted` verb set, staged through the composer.
/// Each verb greys honestly: no unlocked target disk shuts them all;
/// partition-scoped verbs need a partition; Mount/Unmount need one in the
/// matching state; New needs free space to carve.
fn build_partition_menu(state: &StorageState) -> Menu<MenuAction> {
    let disk = selected_disk(state);
    let stageable = disk
        .as_ref()
        .is_some_and(|d| d.protected_reason().is_none());
    let has_parts = stageable && disk.as_ref().is_some_and(|d| !d.partitions.is_empty());
    let has_free = stageable && disk.as_ref().is_some_and(|d| d.free_mib() > 0);
    let any_mounted = has_parts
        && disk
            .as_ref()
            .is_some_and(|d| d.partitions.iter().any(|p| p.mountpoint.is_some()));
    let any_unmounted = has_parts
        && disk
            .as_ref()
            .is_some_and(|d| d.partitions.iter().any(|p| p.mountpoint.is_none()));

    let item_for = |kind: OpKind, label: &str, enabled: bool| {
        Entry::Item(Item::new(MenuAction::StageKind(kind), label).enabled(enabled))
    };
    let format_to: Vec<Entry<MenuAction>> = Filesystem::ALL
        .iter()
        .map(|&fs| {
            Entry::Item(Item::new(MenuAction::StageFormat(fs), fs.label()).enabled(has_parts))
        })
        .collect();

    Menu::new(
        "Partition",
        vec![
            item_for(OpKind::NewPartition, "New\u{2026}", has_free),
            Entry::Separator,
            item_for(OpKind::Delete, "Delete", has_parts),
            item_for(OpKind::Resize, "Resize (Grow / Shrink)\u{2026}", has_parts),
            item_for(OpKind::Move, "Move\u{2026}", has_parts),
            Entry::Separator,
            Entry::Submenu {
                label: "Format to".to_string(),
                mnemonic: None,
                entries: format_to,
            },
            Entry::Separator,
            item_for(OpKind::Mount, "Mount\u{2026}", any_unmounted),
            item_for(OpKind::Unmount, "Unmount", any_mounted),
            Entry::Separator,
            item_for(OpKind::SetLabel, "Label\u{2026}", has_parts),
        ],
    )
}

/// **Help** — the honest surface identity plus one real seam: the safety
/// posture published on the live toast lane (greyed with no Bus dir, so the
/// item is never a silent no-op).
fn build_help_menu(state: &StorageState) -> Menu<MenuAction> {
    Menu::new(
        "Help",
        vec![
            Entry::Caption(
                "Local Cylinders \u{2014} GParted-class disk surgery over the mesh \
                     storage plane."
                    .to_string(),
            ),
            Entry::Item(
                Item::new(MenuAction::HelpSafety, "Safety & arming posture\u{2026}")
                    .enabled(state.bus_root.is_some()),
            ),
        ],
    )
}

/// The live status cluster: the fleet rollup (disks · peers), the selected
/// peer's backend health, the selected device, and the pending-op count.
fn build_status(state: &StorageState) -> Vec<StatusChip> {
    let peers = state.nodes.len();
    let disks: usize = state.nodes.iter().map(|n| n.topology.devices.len()).sum();
    let mut chips = vec![StatusChip::new(
        format!(
            "{disks} disk{} \u{00B7} {peers} peer{}",
            if disks == 1 { "" } else { "s" },
            if peers == 1 { "" } else { "s" }
        ),
        ChipTone::Neutral,
    )];

    if let Some(node) = state.selected() {
        let tone = if node.available() {
            ChipTone::Ok
        } else {
            ChipTone::Warn
        };
        chips.push(StatusChip::with_icon(DOT, node.host.clone(), tone));
    }

    // The selected device + the pending-op count — the MENU-4 pair.
    if let Some(device) = &state.selected_device {
        chips.push(StatusChip::new(device.clone(), ChipTone::Info));
    }
    let staged = state.queue.len();
    chips.push(StatusChip::new(
        format!("{staged} pending"),
        if staged > 0 {
            ChipTone::Info
        } else {
            ChipTone::Neutral
        },
    ));
    chips
}

/// Apply a picked action to its real seam (§6, no new behaviour).
pub(super) fn apply(state: &mut StorageState, action: MenuAction) {
    match action {
        MenuAction::SelectPeer(host) => state.select_node(&host),
        MenuAction::RescanDevices => {
            if let Some(node) = state.selected_node.clone() {
                state.publish(&node, &StorageRequest::Refresh);
            }
        }
        MenuAction::UndoLast => {
            state.queue.pop();
            if state.queue.is_empty() {
                state.arming.clear();
            }
        }
        MenuAction::ClearQueue => {
            state.queue.clear();
            state.arming.clear();
            state.compose_error = None;
        }
        MenuAction::ApplyAll => {
            // The gate re-decides here (never trusts the render frame's
            // enable): no matching echo ⇒ no request ⇒ nothing publishes.
            if let (Some(node), Some(req)) = (state.selected_node.clone(), state.armed_apply()) {
                state.publish(&node, &req);
            }
        }
        MenuAction::ToggleRail => state.view_rail = !state.view_rail,
        MenuAction::ToggleGeometry => state.view_geometry = !state.view_geometry,
        MenuAction::StageKind(kind) => {
            state.compose.kind = kind;
            state.compose_error = None;
        }
        MenuAction::StageFormat(fs) => {
            state.compose.kind = OpKind::Format;
            state.compose.fs = Some(fs);
            state.compose_error = None;
        }
        MenuAction::HelpSafety => state.emit_safety_note(),
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::super::{
        project, state_body, BackendStatus, Filesystem, NodeStorage, OpKind, StorageOp,
        StorageState, Topology,
    };
    use super::{apply, build_menus, build_status, MenuAction};
    use mde_egui::menubar::{Entry, Menu};
    use mde_egui::ChipTone;

    /// One node view with the given backend health (an empty topology is enough
    /// for the bar's peer + rollup seams).
    fn node(host: &str, available: bool) -> NodeStorage {
        NodeStorage {
            host: host.to_string(),
            backend: if available {
                BackendStatus::Available
            } else {
                BackendStatus::Unavailable {
                    reason: "UDisks2 unreachable".to_string(),
                }
            },
            topology: Topology::default(),
            published_at_ms: 0,
        }
    }

    /// A state carrying two (disk-less) nodes — the peer + rollup seams.
    /// `bus_root: None` keeps the Help gate deterministic off the build host's
    /// environment.
    fn two_node_state() -> StorageState {
        StorageState {
            nodes: vec![node("nodeA", true), node("nodeB", false)],
            local_host: "nodeA".to_string(),
            selected_node: Some("nodeA".to_string()),
            bus_root: None,
            ..StorageState::default()
        }
    }

    /// A state with the two-disk fixture topology (sda protected, sdb free),
    /// sdb selected — the full Partition-spine gating context.
    fn disk_state() -> StorageState {
        let mut state = StorageState {
            nodes: project(&[state_body("nodeA", 1, true)]),
            local_host: "nodeA".to_string(),
            bus_root: None,
            ..StorageState::default()
        };
        state.ensure_selection();
        state
    }

    /// Every activatable item of `menu`, flattened through submenus.
    fn items(menu: &Menu<MenuAction>) -> Vec<&super::Item<MenuAction>> {
        fn walk<'a>(entries: &'a [Entry<MenuAction>], out: &mut Vec<&'a super::Item<MenuAction>>) {
            for e in entries {
                match e {
                    Entry::Item(i) => out.push(i),
                    Entry::Submenu { entries, .. } => walk(entries, out),
                    Entry::Separator | Entry::Caption(_) => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&menu.entries, &mut out);
        out
    }

    fn menu<'a>(menus: &'a [Menu<MenuAction>], title: &str) -> &'a Menu<MenuAction> {
        menus
            .iter()
            .find(|m| m.title == title)
            .unwrap_or_else(|| panic!("{title} menu present"))
    }

    #[test]
    fn the_spine_is_the_gparted_order_and_greys_shut_when_empty() {
        let empty = StorageState {
            bus_root: None,
            ..StorageState::default()
        };
        let menus = build_menus(&empty);
        assert!(
            !menus.iter().any(|m| m.title == "Peer"),
            "no peer ⇒ the Peer menu is omitted, not present-but-empty (§7)"
        );
        // The GParted spine stays constant (a stable chrome), items greyed.
        let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, ["Edit", "View", "Device", "Partition", "Help"]);
        for m in &menus {
            for item in items(m) {
                assert!(
                    !item.enabled,
                    "{} › {} greys with no peer / disk / queue / Bus",
                    m.title, item.label
                );
            }
        }
    }

    #[test]
    fn peer_menu_lists_every_node_with_the_active_one_checked() {
        let state = two_node_state();
        let menus = build_menus(&state);
        let peer = menu(&menus, "Peer");
        let entries = items(peer);
        assert_eq!(entries.len(), 2, "both peers reachable");
        for item in entries {
            let is_a = item.id == MenuAction::SelectPeer("nodeA".to_string());
            assert_eq!(item.checked, Some(is_a), "only the active peer is checked");
        }
    }

    #[test]
    fn the_partition_spine_arms_over_the_unlocked_disk() {
        let state = disk_state();
        assert_eq!(
            state.selected_device.as_deref(),
            Some("/dev/sdb"),
            "the default target skips the protected root disk"
        );
        let menus = build_menus(&state);
        let partition = menu(&menus, "Partition");
        let by_label = |label: &str| {
            items(partition)
                .into_iter()
                .find(|i| i.label == label)
                .unwrap_or_else(|| panic!("{label} present"))
                .enabled
        };
        assert!(by_label("New\u{2026}"), "free space ⇒ New enabled");
        assert!(by_label("Delete"), "a partition exists ⇒ Delete enabled");
        assert!(by_label("Resize (Grow / Shrink)\u{2026}"));
        assert!(by_label("Move\u{2026}"));
        assert!(
            by_label("Mount\u{2026}"),
            "sdb1 is unmounted ⇒ Mount enabled"
        );
        assert!(
            !by_label("Unmount"),
            "nothing mounted on sdb ⇒ Unmount greys (§7)"
        );
        // Format to › carries every filesystem, enabled over the live target.
        let fs_items: Vec<_> = items(partition)
            .into_iter()
            .filter(|i| matches!(i.id, MenuAction::StageFormat(_)))
            .collect();
        assert_eq!(fs_items.len(), Filesystem::ALL.len());
        assert!(fs_items.iter().all(|i| i.enabled));
        // Device › New Partition Table is stageable over the unlocked disk.
        let device = menu(&menus, "Device");
        assert!(items(device)
            .into_iter()
            .any(|i| i.id == MenuAction::StageKind(OpKind::NewTable) && i.enabled));
    }

    #[test]
    fn apply_all_arms_only_on_the_typed_echo() {
        let mut state = disk_state();
        state.queue.push(StorageOp::DeletePartition {
            partition: "/dev/sdb1".to_string(),
        });
        let enabled_apply = |state: &StorageState| {
            items(menu(&build_menus(state), "Edit"))
                .into_iter()
                .find(|i| i.label == "Apply All Operations")
                .expect("Apply All present")
                .enabled
        };
        assert!(
            !enabled_apply(&state),
            "no echo ⇒ Apply All greys (lock 8 — the menu can't bypass arming)"
        );
        state.arming = "/dev/wrong".to_string();
        assert!(!enabled_apply(&state), "a wrong echo keeps it grey");
        state.arming = "/dev/sdb".to_string();
        assert!(enabled_apply(&state), "the exact echo arms it");
        // The apply path re-decides the gate itself; with no Bus dir the
        // publish records the honest error rather than silently dropping.
        apply(&mut state, MenuAction::ApplyAll);
        assert!(state.last_error.is_some(), "no Bus dir ⇒ the honest error");
    }

    #[test]
    fn undo_last_pops_one_op_and_clearing_empties_the_queue() {
        let mut state = disk_state();
        state.queue = vec![
            StorageOp::Unmount {
                partition: "/dev/sdb1".to_string(),
            },
            StorageOp::DeletePartition {
                partition: "/dev/sdb1".to_string(),
            },
        ];
        state.arming = "/dev/sdb".to_string();
        apply(&mut state, MenuAction::UndoLast);
        assert_eq!(state.queue.len(), 1, "undo pops the most recent op");
        assert_eq!(state.arming, "/dev/sdb", "a live queue keeps the echo");
        apply(&mut state, MenuAction::UndoLast);
        assert!(state.queue.is_empty());
        assert!(
            state.arming.is_empty(),
            "an emptied queue drops the stale echo"
        );

        state.queue = vec![StorageOp::Unmount {
            partition: "/dev/sdb1".to_string(),
        }];
        state.arming = "/dev/sdb".to_string();
        apply(&mut state, MenuAction::ClearQueue);
        assert!(state.queue.is_empty(), "the queue is cleared");
        assert!(state.arming.is_empty(), "the arming echo is cleared");
    }

    #[test]
    fn selecting_a_peer_switches_the_active_node() {
        let mut state = two_node_state();
        apply(&mut state, MenuAction::SelectPeer("nodeB".to_string()));
        assert_eq!(state.selected_node.as_deref(), Some("nodeB"));
    }

    #[test]
    fn stage_verbs_jump_the_compose_form() {
        let mut state = two_node_state();
        apply(&mut state, MenuAction::StageKind(OpKind::Resize));
        assert_eq!(state.compose.kind, OpKind::Resize);
        // Format to › presets the filesystem too.
        apply(&mut state, MenuAction::StageFormat(Filesystem::Xfs));
        assert_eq!(state.compose.kind, OpKind::Format);
        assert_eq!(state.compose.fs, Some(Filesystem::Xfs));
    }

    #[test]
    fn view_toggles_flip_and_read_back_checked() {
        let mut state = disk_state();
        apply(&mut state, MenuAction::ToggleRail);
        apply(&mut state, MenuAction::ToggleGeometry);
        assert!(state.view_rail && state.view_geometry);
        let menus = build_menus(&state);
        for item in items(menu(&menus, "View")) {
            assert_eq!(item.checked, Some(true), "{} reads back on", item.label);
        }
    }

    #[test]
    fn status_shows_rollup_health_device_and_pending_count() {
        let mut state = disk_state();
        state.queue = vec![StorageOp::Unmount {
            partition: "/dev/sdb1".to_string(),
        }];
        let chips = build_status(&state);
        assert!(chips.iter().any(|c| c.text.contains("2 disks")));
        assert!(chips
            .iter()
            .any(|c| c.text == "nodeA" && c.tone == ChipTone::Ok));
        assert!(
            chips
                .iter()
                .any(|c| c.text == "/dev/sdb" && c.tone == ChipTone::Info),
            "the selected device chip (MENU-4)"
        );
        assert!(chips
            .iter()
            .any(|c| c.text == "1 pending" && c.tone == ChipTone::Info));
        // An empty queue still reads an honest zero, never a vanished chip.
        state.queue.clear();
        let chips = build_status(&state);
        assert!(chips
            .iter()
            .any(|c| c.text == "0 pending" && c.tone == ChipTone::Neutral));
    }
}
