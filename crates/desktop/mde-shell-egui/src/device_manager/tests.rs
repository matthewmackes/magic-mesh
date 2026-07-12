use super::{
    build_connection_tree, build_node_tree, build_rail, cpu_line, derive_bus, device_a11y_label,
    device_a11y_value, device_armed, device_status_display, device_target, export_dir,
    format_mem_kb, header_lines, host_a11y_value, host_dot_tone, host_hover, humanize_ago,
    humanize_uptime, now_ms, problem_code, render_device_details, render_json, render_report,
    sanitize, scanned_label, status_tone, write_export, DeviceAction, DeviceArming,
    DeviceManagerState, DeviceSelection, DrawerTab, HostEntry, HostFreshness, MenuAction,
    RowActionRequest, ViewMode, STALE_AFTER,
};
use mackes_mesh_types::device_control::{DeviceControlOp, DeviceTarget};
use mackes_mesh_types::device_inventory::{
    self, category, DeviceInventory, DeviceRecord, DeviceStatus, HostSummary,
};
use mde_egui::menubar::{Entry, Menu};
use mde_egui::{egui, ChipTone, Style};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A throwaway substrate root under the system temp dir (this crate does not
/// vendor `tempfile`), removed on drop. Holds a `device-inventory/` dir the
/// rail-read tests publish host fixtures into, so `refresh` exercises the real
/// [`device_inventory::read_all`] path (DEVMGR-4's actual read).
struct ScratchRoot(PathBuf);

impl ScratchRoot {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let root = std::env::temp_dir().join(format!("devmgr-{tag}-{nanos}"));
        std::fs::create_dir_all(device_inventory::inventory_dir(&root)).unwrap();
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Publish a host's inventory (a re-hosted fixture at `published_at_ms`).
    fn publish(&self, host: &str, published_at_ms: u64) {
        let mut inv = DeviceInventory::fixture();
        inv.host = host.to_string();
        inv.published_at_ms = published_at_ms;
        let path = device_inventory::inventory_path(&self.0, host);
        std::fs::write(&path, serde_json::to_string(&inv).unwrap()).unwrap();
    }
}

impl Drop for ScratchRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A published fixture inventory re-hosted under `host` (a distinct rail peer).
fn host_inventory(host: &str) -> DeviceInventory {
    let mut inv = DeviceInventory::fixture();
    inv.host = host.to_string();
    inv
}

/// A state carrying a chosen inventory + seen flag, rooted at a non-existent
/// path so `refresh` reads an honest `None` (no real substrate touched).
fn state_with(inv: Option<DeviceInventory>, seen: bool) -> DeviceManagerState {
    DeviceManagerState {
        workgroup_root: PathBuf::from("/nonexistent-devmgr-test-root"),
        local_host: "laptop-mm".to_string(),
        selected_host: "laptop-mm".to_string(),
        hosts: Vec::new(),
        // Seed the fleet set with the given inventory so a By-node render off a
        // bare state (no refresh) is coherent; a `refresh` repopulates it.
        all_inventories: inv.iter().cloned().collect(),
        inventory: inv,
        non_pc: Vec::new(),
        bus_root: None,
        seen,
        last_poll: None,
        expanded: BTreeSet::new(),
        view: ViewMode::ByType,
        selected: None,
        active_tab: DrawerTab::General,
        show_about: false,
        arming: None,
    }
}

/// Drive one headless frame of the surface (the same `Context::run` →
/// tessellate path the DRM runner uses, minus the GPU) and return the drawn
/// primitive count — proving it is a live render, not dead code.
fn drive(state: &mut DeviceManagerState) -> usize {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1000.0, 800.0),
        )),
        ..Default::default()
    };
    let out = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| state.show(ui));
    });
    ctx.tessellate(out.shapes, out.pixels_per_point).len()
}

// ── a11y-05: the device-row + host-row accessible name/state seam ──

#[test]
fn device_a11y_label_and_value_read_the_name_and_mdm_status() {
    // A faulted device speaks the MDM problem code + the honest Linux reason.
    let mut dev = DeviceRecord::new("Intel Wi-Fi 6 AX200", DeviceStatus::NoDriver);
    dev.problem = Some("no kernel driver bound".to_string());
    assert_eq!(device_a11y_label(&dev), "Intel Wi-Fi 6 AX200");
    assert_eq!(
        device_a11y_value(&dev),
        "Code 28 \u{2014} no kernel driver bound",
        "the a11y value is the exact MDM status the drawer shows",
    );
    // A healthy device reads the plain "working properly" line.
    let ok = DeviceRecord::new("Samsung NVMe SSD", DeviceStatus::Ok);
    assert_eq!(device_a11y_value(&ok), "This device is working properly.");
}

#[test]
fn host_a11y_value_reads_freshness_and_device_and_problem_counts() {
    // A live host with a faulted device.
    let live = HostEntry {
        host: "node-a".to_string(),
        label: "node-a".to_string(),
        kind: HostKind::Node,
        published_at_ms: Some(1_000),
        device_count: 12,
        problem_count: 1,
    };
    assert_eq!(
        host_a11y_value(&live, 1_500),
        "live \u{00B7} 12 devices \u{00B7} 1 problem",
    );
    // An absent host reads the honest offline line (§7).
    let absent = HostEntry::absent("node-b");
    assert_eq!(
        host_a11y_value(&absent, 1_500),
        "offline \u{00B7} nothing published",
    );
}

#[test]
fn the_tree_renders_headless_from_a_fixture_inventory() {
    let mut s = state_with(Some(DeviceInventory::fixture()), true);
    // Expand so category bodies (the device rows) render too.
    s.expand_all();
    assert!(drive(&mut s) > 0, "the device tree drew nothing");
}

#[test]
fn categories_default_all_collapsed_then_expand_and_collapse_all() {
    let mut s = state_with(Some(DeviceInventory::fixture()), true);
    // #18 — every category is collapsed on open.
    assert!(s.expanded.is_empty(), "all categories collapsed on open");
    s.expand_all();
    // The fixture publishes exactly the Display + System(PCI) categories.
    assert_eq!(s.expanded.len(), 2);
    assert!(s.expanded.contains(category::DISPLAY));
    assert!(s.expanded.contains(category::PCI_DEVICES));
    // Toggling one collapses just it; toggling again re-expands it.
    s.toggle(category::DISPLAY);
    assert!(!s.expanded.contains(category::DISPLAY));
    assert!(s.expanded.contains(category::PCI_DEVICES));
    s.toggle(category::DISPLAY);
    assert!(s.expanded.contains(category::DISPLAY));
    // Collapse-all clears everything back to the #18 default.
    s.collapse_all();
    assert!(s.expanded.is_empty());
}

#[test]
fn header_card_fields_derive_from_the_summary() {
    let inv = DeviceInventory::fixture();
    let lines = header_lines(&inv);
    let get = |k: &str| {
        lines
            .iter()
            .find(|(l, _)| *l == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    assert!(get("OS").contains("Fedora"), "OS: {}", get("OS"));
    assert!(get("Kernel").contains("fc44"), "kernel: {}", get("Kernel"));
    assert!(get("CPU").contains("i7-8650U"), "cpu: {}", get("CPU"));
    assert!(
        get("CPU").contains('8'),
        "logical count folded in: {}",
        get("CPU")
    );
    assert!(get("Memory").ends_with("GiB"), "memory: {}", get("Memory"));
    assert_ne!(get("Uptime"), "\u{2014}", "uptime present in the fixture");
    // The header badge counts (#20) come straight off the schema helpers.
    assert_eq!(inv.device_count(), 2);
    assert_eq!(inv.problem_count(), 1);
}

#[test]
fn absent_summary_fields_render_as_an_em_dash_not_a_fake_value() {
    // A shallow / non-PC host (#22) carries a bare summary — every field is an
    // honest em-dash, never a fabricated total (§7).
    let inv = DeviceInventory {
        host: "vyos-edge".to_string(),
        published_at_ms: 0,
        summary: HostSummary::default(),
        tools: mackes_mesh_types::device_inventory::ToolAvailability::default(),
        categories: vec![],
    };
    for (_, value) in header_lines(&inv) {
        assert_eq!(value, "\u{2014}", "an absent field must dash, not fake");
    }
}

#[test]
fn honest_pre_poll_then_an_empty_host_read() {
    // Fresh: unseen + no inventory — the dim pre-poll (§7), no fake tree.
    let mut s = state_with(None, false);
    assert!(!s.seen);
    assert!(drive(&mut s) > 0, "the pre-poll state drew nothing");
    // A read of a missing inventory dir flips `seen` but yields an honest None.
    s.refresh();
    assert!(s.seen, "seen after the first read");
    assert!(
        s.inventory.is_none(),
        "a missing inventory reads as None, not a fabricated tree"
    );
    assert!(drive(&mut s) > 0, "the empty-host state drew nothing");
}

#[test]
fn all_three_view_modes_are_wired() {
    // #3 — the View menu offers all three modes: DEVMGR-2 wired By type,
    // DEVMGR-5 By connection, and DEVMGR-10 By node (the cross-fleet flatten).
    // All three now render, so no honestly-disabled seam remains (§7).
    assert_eq!(ViewMode::ALL.len(), 3);
    assert!(ViewMode::ByType.is_available());
    assert!(ViewMode::ByConnection.is_available());
    assert!(ViewMode::ByNode.is_available());
    assert_eq!(ViewMode::default(), ViewMode::ByType);
}

#[test]
fn the_info_dialog_opens_and_renders_the_about_content() {
    // #24 — the ⓘ dialog reuses the canonical identity screen; opening it must
    // render (the modal + the About body) without panicking.
    let mut s = state_with(Some(DeviceInventory::fixture()), true);
    s.show_about = true;
    assert!(drive(&mut s) > 0, "the about dialog drew nothing");
}

#[test]
fn uptime_and_memory_format_honestly() {
    assert_eq!(humanize_uptime(48_120), "13h 22m");
    assert_eq!(humanize_uptime(90_061), "1d 1h 1m");
    assert_eq!(humanize_uptime(59), "0m");
    let m = format_mem_kb(16_072_192);
    assert!(m.ends_with(" GiB"), "memory unit: {m}");
    assert!(m.starts_with("15."), "16 GB laptop reads ~15.3 GiB: {m}");
}

#[test]
fn status_tones_separate_ok_from_problems() {
    // Ok is the success green; the problem states are visibly distinct tones,
    // and none of them read as Ok (so a problem never renders "healthy").
    assert_eq!(status_tone(DeviceStatus::Ok), Style::OK);
    for bad in [
        DeviceStatus::NoDriver,
        DeviceStatus::Degraded,
        DeviceStatus::Disabled,
        DeviceStatus::Unknown,
    ] {
        assert_ne!(status_tone(bad), Style::OK, "{bad:?} must not read as Ok");
    }
    // A hard error is the danger tone; a driverless device warns.
    assert_eq!(status_tone(DeviceStatus::Degraded), Style::DANGER);
    assert_eq!(status_tone(DeviceStatus::NoDriver), Style::WARN);
}

#[test]
fn cpu_line_degrades_over_a_partial_summary() {
    let mut s = HostSummary {
        cpu_model: Some("Intel Xeon".to_string()),
        cpu_count: Some(16),
        ..Default::default()
    };
    assert!(cpu_line(&s).contains("Intel Xeon") && cpu_line(&s).contains("16"));
    s.cpu_count = None;
    assert_eq!(cpu_line(&s), "Intel Xeon");
    s.cpu_model = None;
    s.cpu_count = Some(4);
    assert!(cpu_line(&s).contains('4'));
    s.cpu_count = None;
    assert_eq!(cpu_line(&s), "\u{2014}");
}

// ── DEVMGR-3 helpers ─────────────────────────────────────────────────────

/// The fixture's driverless PCI device (`NoDriver` + the honest Linux reason,
/// with no driver / events / resources — the empty-tab cases).
fn orphan() -> DeviceRecord {
    DeviceInventory::fixture()
        .categories
        .into_iter()
        .find(|c| c.key == category::PCI_DEVICES)
        .and_then(|c| c.devices.into_iter().next())
        .expect("the fixture publishes a PCI device")
}

/// The activation ids of a menu's items, in order.
fn item_ids(menu: &Menu<MenuAction>) -> Vec<MenuAction> {
    menu.entries
        .iter()
        .filter_map(|e| match e {
            Entry::Item(item) => Some(item.id.clone()),
            _ => None,
        })
        .collect()
}

// ── (b) MDM problem-code parity (#11) ────────────────────────────────────

#[test]
fn linux_state_maps_to_the_mdm_problem_code() {
    // The faithful emulation the design locks: no-driver→28, disabled→22,
    // degraded→10; Ok + Unknown carry no fabricated Windows code.
    assert_eq!(problem_code(DeviceStatus::NoDriver), Some(28));
    assert_eq!(problem_code(DeviceStatus::Disabled), Some(22));
    assert_eq!(problem_code(DeviceStatus::Degraded), Some(10));
    assert_eq!(problem_code(DeviceStatus::Ok), None);
    assert_eq!(problem_code(DeviceStatus::Unknown), None);
}

#[test]
fn device_status_display_keeps_the_real_linux_reason_beside_the_code() {
    // A driverless device → Code 28 WITH the honest Linux reason, in the warn
    // tone — the code never stands alone (design "keep the emulation honest").
    let (text, tone) = device_status_display(&orphan());
    assert!(text.contains("Code 28"), "the MDM code: {text}");
    assert!(
        text.contains("no kernel driver bound"),
        "the honest Linux reason rides beside the code: {text}"
    );
    assert_eq!(tone, Style::WARN);
    // A healthy device reads the MDM "working properly", in the Ok tone.
    let gpu = DeviceRecord::new("Intel UHD Graphics", DeviceStatus::Ok);
    let (text, tone) = device_status_display(&gpu);
    assert_eq!(text, "This device is working properly.");
    assert_eq!(tone, Style::OK);
    // An unknown state stays honest — never dressed as a fabricated code.
    let mut unk = DeviceRecord::new("Unclaimed bus device", DeviceStatus::Unknown);
    let (text, _) = device_status_display(&unk);
    assert!(!text.contains("Code"), "unknown fabricates no code: {text}");
    unk.problem = Some("state could not be read".to_string());
    let (text, _) = device_status_display(&unk);
    assert!(
        text.contains("state could not be read"),
        "reason kept: {text}"
    );
}

// ── (a) the bottom detail drawer (#9/#10) ────────────────────────────────

#[test]
fn the_drawer_has_the_full_mdm_tab_set() {
    assert_eq!(DrawerTab::ALL.len(), 5);
    let labels: Vec<&str> = DrawerTab::ALL.iter().map(|t| t.label()).collect();
    assert_eq!(
        labels,
        vec!["General", "Driver", "Details", "Events", "Resources"]
    );
    assert_eq!(DrawerTab::default(), DrawerTab::General);
}

#[test]
fn the_five_tab_drawer_renders_for_a_selected_device() {
    // Selecting a device opens the drawer; each of the five MDM tabs renders
    // from the record without panicking (a live render, not dead code) — and
    // the orphan exercises the honest empty Driver / Events / Resources tabs.
    let inv = DeviceInventory::fixture();
    let orphan = orphan();
    for tab in DrawerTab::ALL {
        let mut s = state_with(Some(inv.clone()), true);
        s.selected = Some(DeviceSelection::of(category::PCI_DEVICES, &orphan));
        s.active_tab = tab;
        assert!(drive(&mut s) > 0, "the {} tab drew nothing", tab.label());
        assert!(
            s.selected.is_some(),
            "a live selection stays open on the {} tab",
            tab.label()
        );
    }
}

#[test]
fn the_drawer_prunes_a_selection_that_vanished() {
    // A device no longer published closes the drawer rather than freezing a
    // stale clone (§7 — honest, never a fabricated render).
    let mut s = state_with(Some(DeviceInventory::fixture()), true);
    s.selected = Some(DeviceSelection {
        category: category::PCI_DEVICES.to_string(),
        name: "A device that was unplugged".to_string(),
        sysfs_path: None,
    });
    let _ = drive(&mut s);
    assert!(s.selected.is_none(), "an unresolvable selection is dropped");
}

#[test]
fn a_device_selection_keys_on_category_name_and_sysfs() {
    let orphan = orphan();
    let sel = DeviceSelection::of(category::PCI_DEVICES, &orphan);
    // The same device in the same category matches (a re-publish preserves it).
    assert!(sel.matches(category::PCI_DEVICES, &orphan));
    // A different category, or a different device, does not.
    assert!(!sel.matches(category::DISPLAY, &orphan));
    let other = DeviceRecord::new("Something else entirely", DeviceStatus::Ok);
    assert!(!sel.matches(category::PCI_DEVICES, &other));
}

// ── (c) the shared MenuBar drives the real seams ─────────────────────────

#[test]
fn the_menu_bar_menus_drive_the_real_seams() {
    let s = state_with(Some(DeviceInventory::fixture()), true);
    let menus = s.build_menus();
    let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
    // MENU-5 grew the spine so the bar covers the extended Device Manager.
    assert_eq!(titles, vec!["Action", "View", "Hosts", "Device", "Help"]);
    // No invented File/Edit spine — About has no file/clipboard seam (§7).
    for banned in ["File", "Edit"] {
        assert!(!titles.contains(&banned), "{banned} shipped without a seam");
    }
    // Action → Scan + the DEVMGR-6 export/copy report seams (MDM's Action →
    // generate a report). Separators drop out of `item_ids`.
    assert_eq!(
        item_ids(&menus[0]),
        vec![
            MenuAction::Scan,
            MenuAction::ExportJson,
            MenuAction::ExportMarkdown,
            MenuAction::CopyReport,
        ]
    );
    // View → the three modes (By type live + checked, the others disabled
    // seams §7) + Expand/Collapse-all (enabled with a loaded inventory).
    let view = &menus[1];
    for entry in &view.entries {
        if let Entry::Item(item) = entry {
            if let MenuAction::View(mode) = &item.id {
                let mode = *mode;
                assert_eq!(
                    item.enabled,
                    mode.is_available(),
                    "{mode:?} enablement tracks whether it is wired"
                );
                assert_eq!(
                    item.checked,
                    Some(mode == ViewMode::ByType),
                    "the active mode is the checked one"
                );
            }
        }
    }
    let enabled = |id: MenuAction| {
        view.entries
            .iter()
            .any(|e| matches!(e, Entry::Item(it) if it.id == id && it.enabled))
    };
    assert!(enabled(MenuAction::ExpandAll));
    assert!(enabled(MenuAction::CollapseAll));
    // MENU-5 — View grew a Jump-to-category submenu (the fixture's two categories).
    assert!(
        view.entries.iter().any(|e| matches!(
            e,
            Entry::Submenu { label, entries, .. }
                if label == "Jump to category" && entries.len() == 2
        )),
        "View carries a Jump-to-category submenu over the published categories"
    );
    // Help → the ⓘ dialog (now the 5th menu after MENU-5's Hosts + Device).
    assert_eq!(item_ids(&menus[4]), vec![MenuAction::About]);
}

#[test]
fn expand_collapse_disable_without_a_loaded_inventory() {
    // §7 — with nothing published there is nothing to expand, so the two are
    // honestly disabled (never a silent no-op).
    let s = state_with(None, true);
    let view = &s.build_menus()[1];
    for id in [MenuAction::ExpandAll, MenuAction::CollapseAll] {
        assert!(
            view.entries
                .iter()
                .any(|e| matches!(e, Entry::Item(it) if it.id == id && !it.enabled)),
            "{id:?} greys with no tree"
        );
    }
}

#[test]
fn apply_dispatches_each_action_to_its_seam() {
    // Scan re-reads (seen flips true even off a fresh, empty state).
    let mut s = state_with(None, false);
    s.apply(MenuAction::Scan);
    assert!(s.seen, "Scan drove a read");
    // Expand / Collapse over the fixture.
    let mut s = state_with(Some(DeviceInventory::fixture()), true);
    s.apply(MenuAction::ExpandAll);
    assert_eq!(s.expanded.len(), 2, "Expand all filled the set");
    s.apply(MenuAction::CollapseAll);
    assert!(s.expanded.is_empty(), "Collapse all cleared it");
    // A view switch + the ⓘ dialog.
    s.apply(MenuAction::View(ViewMode::ByConnection));
    assert_eq!(s.view, ViewMode::ByConnection);
    assert!(!s.show_about);
    s.apply(MenuAction::About);
    assert!(s.show_about, "About opened the ⓘ dialog");
}

#[test]
fn the_status_cluster_reflects_host_devices_and_problems() {
    let inv = DeviceInventory::fixture();
    let published = inv.published_at_ms;
    let s = state_with(Some(inv), true);
    let chips = s.status_chips(published + 90_000); // 90 s after publish
    assert!(
        chips
            .iter()
            .any(|c| c.text == "laptop-mm" && c.tone == ChipTone::Info),
        "the host chip reads Info once an inventory loads"
    );
    assert!(chips.iter().any(|c| c.text == "2 devices"), "device count");
    assert!(
        chips
            .iter()
            .any(|c| c.text.contains("1 problem") && c.tone == ChipTone::Danger),
        "the one faulted device reads a danger problem chip"
    );
    assert!(
        chips.iter().any(|c| c.text == "Scanned 1m ago"),
        "the freshness chip: {:?}",
        chips.iter().map(|c| c.text.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn the_status_cluster_is_honest_before_a_read_and_when_clean() {
    // Pre-read: only the host chip, neutral, no fabricated counts.
    let pre = state_with(None, false);
    let chips = pre.status_chips(0);
    assert_eq!(chips.len(), 1, "no counts before the first read");
    assert_eq!(chips[0].tone, ChipTone::Neutral);
    // A clean host reads an Ok "No problems".
    let mut inv = DeviceInventory::fixture();
    for cat in &mut inv.categories {
        for dev in &mut cat.devices {
            dev.status = DeviceStatus::Ok;
            dev.problem = None;
        }
    }
    let clean = state_with(Some(inv), true);
    let chips = clean.status_chips(0);
    assert!(
        chips
            .iter()
            .any(|c| c.text == "No problems" && c.tone == ChipTone::Ok),
        "a clean host reads an Ok 'No problems'"
    );
}

#[test]
fn scanned_freshness_humanizes_and_stays_honest() {
    assert_eq!(humanize_ago(3), "just now");
    assert_eq!(humanize_ago(42), "42s ago");
    assert_eq!(humanize_ago(600), "10m ago");
    assert_eq!(humanize_ago(7_200), "2h ago");
    assert_eq!(humanize_ago(180_000), "2d ago");
    // A publish time of 0 (the schema's honest "unknown") fabricates no age.
    assert_eq!(scanned_label(1_000_000, 0), "Scanned \u{2014}");
    assert_eq!(scanned_label(1_000_000, 940_000), "Scanned 1m ago");
}

// ── DEVMGR-4: the host rail + mesh-node switching ────────────────────────

#[test]
fn the_rail_lists_every_published_host_with_local_pinned_first() {
    // read_all delivers the published peers sorted; build_rail injects the
    // absent local "you are here" row and pins it first, the rest alphabetical.
    let all = vec![
        host_inventory("alpha"),
        host_inventory("mid-node"),
        host_inventory("zulu"),
    ];
    let rail = build_rail(&all, "laptop-mm");
    let names: Vec<&str> = rail.iter().map(|e| e.host.as_str()).collect();
    assert_eq!(names, vec!["laptop-mm", "alpha", "mid-node", "zulu"]);
    // The local node was not among the published set, so it is an honest absent
    // row (§7) — a selectable "you are here" that has published nothing yet.
    assert_eq!(rail[0].published_at_ms, None);
    assert_eq!(rail[0].freshness(0), HostFreshness::Absent);
    // A published peer carries its real counts (the fixture: 2 devices, 1 fault).
    let alpha = rail.iter().find(|e| e.host == "alpha").unwrap();
    assert_eq!(alpha.device_count, 2);
    assert_eq!(alpha.problem_count, 1);
}

#[test]
fn the_local_host_is_pinned_first_even_when_it_published_and_sorts_late() {
    // "zeta" is the local node AND published; alphabetically last, but the rail
    // pins it first (you-are-here) with no duplicate row.
    let all = vec![
        host_inventory("alpha"),
        host_inventory("beta"),
        host_inventory("zeta"),
    ];
    let rail = build_rail(&all, "zeta");
    let names: Vec<&str> = rail.iter().map(|e| e.host.as_str()).collect();
    assert_eq!(names, vec!["zeta", "alpha", "beta"]);
    assert_eq!(
        rail.iter().filter(|e| e.host == "zeta").count(),
        1,
        "the published local host is not duplicated by the injected row"
    );
    assert!(
        rail[0].published_at_ms.is_some(),
        "local published, not absent"
    );
}

#[test]
fn refresh_reads_the_rail_and_switching_loads_the_selected_hosts_tree() {
    // A real multi-host substrate — the DEVMGR-4 read path end to end.
    let scratch = ScratchRoot::new("switch");
    scratch.publish("laptop-mm", 1_000); // the local node
    scratch.publish("edge-1", 2_000);
    scratch.publish("edge-2", 3_000);
    let mut s = state_with(None, false);
    s.workgroup_root = scratch.path().to_path_buf();
    s.refresh();
    // The rail lists every published host from the peer directory, local first.
    let names: Vec<String> = s.hosts.iter().map(|e| e.host.clone()).collect();
    assert_eq!(names, vec!["laptop-mm", "edge-1", "edge-2"]);
    // The default selection loaded the LOCAL host's tree.
    assert_eq!(s.inventory.as_ref().unwrap().host, "laptop-mm");
    // Switching selects the right host's published tree, instantly (#7 hybrid).
    s.select_host("edge-2".to_string());
    assert_eq!(s.selected_host, "edge-2");
    assert_eq!(s.inventory.as_ref().unwrap().host, "edge-2");
    assert_eq!(s.inventory.as_ref().unwrap().device_count(), 2);
    // Switching resets any open device drawer (a selection is per-host).
    assert!(s.selected.is_none());
    // And it still renders headless (a live render of the switched host).
    assert!(drive(&mut s) > 0, "the switched-host surface drew nothing");
}

#[test]
fn an_unpublished_selected_host_reads_an_honest_empty_tree() {
    // Only the local node has published; selecting a never-seen peer reads an
    // honest None (the empty-host state), never a fabricated tree (§7).
    let scratch = ScratchRoot::new("absent");
    scratch.publish("laptop-mm", 5_000);
    let mut s = state_with(None, false);
    s.workgroup_root = scratch.path().to_path_buf();
    s.refresh();
    s.select_host("ghost-node".to_string());
    assert_eq!(s.selected_host, "ghost-node");
    assert!(
        s.inventory.is_none(),
        "an unpublished host reads as None, not a fake tree"
    );
    assert!(s.seen);
    assert!(drive(&mut s) > 0, "the empty-host state drew nothing");
    // The local "you are here" row stays present in the rail regardless.
    assert!(s.hosts.iter().any(|e| e.host == "laptop-mm"));
}

#[test]
fn freshness_maps_to_honest_dim_stale_and_offline_dots() {
    let now = 10_000_000_u64;
    let stale_ms = u64::try_from(STALE_AFTER.as_millis()).unwrap();
    // Absent — nothing published: dim (offline), never green.
    let absent = HostEntry::absent("ghost");
    assert_eq!(absent.freshness(now), HostFreshness::Absent);
    assert_eq!(host_dot_tone(&absent, now), Style::TEXT_DIM);
    // Fresh + clean → OK green; fresh + a fault → danger red.
    let fresh_ok = HostEntry {
        host: "a".into(),
        label: "a".into(),
        kind: HostKind::Node,
        published_at_ms: Some(now - 1_000),
        device_count: 3,
        problem_count: 0,
    };
    assert_eq!(fresh_ok.freshness(now), HostFreshness::Fresh);
    assert_eq!(host_dot_tone(&fresh_ok, now), Style::OK);
    let fresh_bad = HostEntry {
        problem_count: 2,
        ..fresh_ok
    };
    assert_eq!(host_dot_tone(&fresh_bad, now), Style::DANGER);
    // Stale — published, but older than STALE_AFTER: amber, not green (its
    // health can't be trusted), even with no problems in the stale snapshot.
    let stale = HostEntry {
        host: "b".into(),
        label: "b".into(),
        kind: HostKind::Node,
        published_at_ms: Some(now - stale_ms - 1),
        device_count: 5,
        problem_count: 0,
    };
    assert_eq!(stale.freshness(now), HostFreshness::Stale);
    assert_eq!(host_dot_tone(&stale, now), Style::WARN);
    // A published-but-unknown-time snapshot (the schema's honest 0) reads stale.
    let unknown = HostEntry {
        host: "c".into(),
        label: "c".into(),
        kind: HostKind::Node,
        published_at_ms: Some(0),
        device_count: 1,
        problem_count: 0,
    };
    assert_eq!(unknown.freshness(now), HostFreshness::Stale);
}

#[test]
fn the_rail_renders_headless_and_its_hover_stays_honest() {
    let now = 10_000_000_u64;
    // An absent host's hover is the honest offline line — it invents no counts
    // and no freshness read-out (a single line, no "N devices" / "Scanned …").
    let absent = HostEntry::absent("ghost");
    let h = host_hover(&absent, now);
    assert!(
        h.contains("No device inventory published"),
        "absent hover: {h}"
    );
    assert!(
        !h.contains("Scanned"),
        "an absent hover invents no freshness: {h}"
    );
    assert!(
        !h.contains('\n'),
        "an absent hover is a single honest line: {h}"
    );
    // A stale host's hover is flagged honestly, with its real counts.
    let stale = HostEntry {
        host: "b".into(),
        label: "b".into(),
        kind: HostKind::Node,
        published_at_ms: Some(now - 600_000),
        device_count: 5,
        problem_count: 1,
    };
    let h = host_hover(&stale, now);
    assert!(h.contains("Stale"), "a stale hover flags staleness: {h}");
    assert!(
        h.contains("5 devices") && h.contains("1 problem"),
        "the real counts: {h}"
    );
    // The rail itself renders headless from a populated hosts list (a live
    // render — a fresh local peer + an offline one — proving it isn't dead).
    let mut s = state_with(Some(DeviceInventory::fixture()), true);
    s.hosts = vec![
        HostEntry::from_inventory(&DeviceInventory::fixture()),
        HostEntry::absent("edge-offline"),
    ];
    assert!(drive(&mut s) > 0, "the host rail drew nothing");
}

// ── DEVMGR-5: the By-connection (bus / controller) view ──────────────────

#[test]
fn by_connection_nests_each_device_under_its_parent_bus() {
    // The fixture's two PCI devices sit on distinct bus segments (0000:00 and
    // 0000:02); the by-connection tree re-roots them under those bus branches
    // (host → bus → device) — correct parent→child nesting, no flat degrade.
    let tree = build_connection_tree(&DeviceInventory::fixture());
    assert!(!tree.flat_no_bus, "the fixture carries real bus topology");
    let labels: Vec<&str> = tree.roots.iter().map(|n| n.label.as_str()).collect();
    assert_eq!(labels, vec!["PCI bus 0000:00", "PCI bus 0000:02"]);
    // Every root is a bus branch (no parentless leaves), each holding its one
    // device as a child leaf under the correct parent bus.
    for bus in &tree.roots {
        assert!(
            bus.device.is_none(),
            "a root bus branch is not a device leaf"
        );
        assert_eq!(bus.children.len(), 1, "one device on each fixture bus");
    }
    assert_eq!(
        tree.roots[0].children[0].label, "Intel UHD Graphics 620",
        "the GPU nests under its own bus segment 0000:00"
    );
    // The bus keys (Expand-all fodder) are exactly the two segment branches.
    assert_eq!(tree.bus_keys().len(), 2);
}

#[test]
fn by_connection_puts_a_parentless_device_under_the_host_root() {
    // A device with no sysfs path (nothing to resolve a bus from) is never
    // dropped — it renders as a leaf directly among the roots (§7).
    let mut inv = DeviceInventory::fixture();
    inv.categories.push(device_inventory::DeviceCategory::new(
        category::SENSORS,
        vec![DeviceRecord::new("ACPI thermal zone", DeviceStatus::Ok)],
    ));
    let tree = build_connection_tree(&inv);
    assert!(!tree.flat_no_bus, "some devices still carry a bus");
    // The parentless sensor is a root-level leaf (device Some, no bus branch).
    let leaf = tree
        .roots
        .iter()
        .find(|n| {
            n.device
                .as_ref()
                .is_some_and(|d| d.name == "ACPI thermal zone")
        })
        .expect("the parentless device stays under the root, never dropped");
    assert!(leaf.children.is_empty(), "a leaf has no children");
    assert!(leaf.key.is_empty(), "a leaf is not a bus branch");
    // The two PCI bus branches are still present alongside it.
    assert_eq!(
        tree.roots.iter().filter(|n| n.device.is_none()).count(),
        2,
        "the PCI bus branches remain"
    );
}

#[test]
fn by_connection_degrades_honestly_with_no_bus_data() {
    // A host that published no sysfs paths at all (a shallow / non-PC host,
    // #22) has no derivable topology — the tree renders flat under the root
    // with the honest note flag set, never a fabricated hierarchy (§7).
    let inv = DeviceInventory {
        host: "vyos-edge".to_string(),
        published_at_ms: 1,
        summary: HostSummary::default(),
        tools: device_inventory::ToolAvailability::default(),
        categories: vec![device_inventory::DeviceCategory::new(
            category::NETWORK_ADAPTERS,
            vec![
                DeviceRecord::new("eth0", DeviceStatus::Ok),
                DeviceRecord::new("eth1", DeviceStatus::Ok),
            ],
        )],
    };
    let tree = build_connection_tree(&inv);
    assert!(tree.flat_no_bus, "no bus data → the honest flat degrade");
    // Both devices are flat leaves under the root (no bus branch invented).
    assert_eq!(tree.roots.len(), 2);
    assert!(
        tree.roots
            .iter()
            .all(|n| n.device.is_some() && n.children.is_empty()),
        "every node is a flat device leaf, no fabricated bus branch"
    );
    assert!(tree.bus_keys().is_empty(), "no bus branches to expand");
}

#[test]
fn switching_to_by_connection_preserves_the_selected_host_and_renders() {
    // The rail selection (DEVMGR-4) governs the host; flipping the view mode
    // (DEVMGR-5) re-groups the SAME host's devices without changing which host
    // is inspected or its loaded inventory.
    let scratch = ScratchRoot::new("view-switch");
    scratch.publish("laptop-mm", 1_000);
    scratch.publish("edge-2", 2_000);
    let mut s = state_with(None, false);
    s.workgroup_root = scratch.path().to_path_buf();
    s.refresh();
    s.select_host("edge-2".to_string());
    assert_eq!(s.selected_host, "edge-2");
    // Flip to By-connection — the seam the View menu drives.
    s.apply(MenuAction::View(ViewMode::ByConnection));
    assert_eq!(s.view, ViewMode::ByConnection);
    // The selected host + its inventory are unchanged by the view switch.
    assert_eq!(s.selected_host, "edge-2", "the host survives a view flip");
    assert_eq!(s.inventory.as_ref().unwrap().host, "edge-2");
    // Expand-all now fills the BUS branches (not the category keys) for this
    // view — the one control tracks whichever tree is showing.
    s.expand_all();
    assert_eq!(
        s.expanded,
        build_connection_tree(s.inventory.as_ref().unwrap()).bus_keys(),
        "Expand-all fills the by-connection bus branches"
    );
    // And the by-connection tree renders headless (a live render, not dead).
    assert!(drive(&mut s) > 0, "the by-connection surface drew nothing");
}

#[test]
fn derive_bus_reads_pci_usb_and_generic_paths_and_honest_none() {
    // PCI: the device's own DDDD:BB bus segment (the flat symlink form).
    assert_eq!(
        derive_bus(Some("/sys/bus/pci/devices/0000:02:00.0")).map(|b| b.label),
        Some("PCI bus 0000:02".to_string())
    );
    // PCI: a real /sys/devices/… path resolves to the device's own bus, not
    // the bridge's (the last address in the path).
    assert_eq!(
        derive_bus(Some("/sys/devices/pci0000:00/0000:00:1c.5/0000:03:00.0")).map(|b| b.label),
        Some("PCI bus 0000:03".to_string())
    );
    // USB: the bus number of a port path (topology from the sysfs name).
    assert_eq!(
        derive_bus(Some("/sys/bus/usb/devices/1-1.2")).map(|b| b.label),
        Some("USB bus 1".to_string())
    );
    // A generic bus kind is title-cased.
    assert_eq!(
        derive_bus(Some("/sys/bus/virtio/devices/virtio0")).map(|b| b.label),
        Some("Virtio bus".to_string())
    );
    // No path, or an unrecognized one, resolves no bus (→ the host root).
    assert_eq!(derive_bus(None).map(|b| b.key), None);
    assert_eq!(derive_bus(Some("/proc/cpuinfo")).map(|b| b.key), None);
}

// ── DEVMGR-6: export / print the inventory (#23) ─────────────────────────

#[test]
fn export_json_round_trips_the_fixture_inventory() {
    // The machine export serde-serializes the §6 contract and round-trips it
    // byte-for-value — the JSON is the same DeviceInventory back.
    let inv = DeviceInventory::fixture();
    let json = render_json(Some(&inv), &inv.host);
    let back: DeviceInventory = serde_json::from_str(&json).unwrap();
    assert_eq!(back, inv, "the JSON export round-trips the §6 contract");
}

#[test]
fn the_markdown_report_lists_the_host_every_device_and_the_problem_code() {
    let inv = DeviceInventory::fixture();
    let report = render_report(Some(&inv), &inv.host, ViewMode::ByType);
    // The host header + the mirrored header-card summary fields (#20).
    assert!(report.contains("laptop-mm"), "the host header: {report}");
    assert!(report.contains("Fedora"), "the OS summary line: {report}");
    // Every published device is named (the on-screen tree membership).
    assert!(report.contains("Intel UHD Graphics 620"), "the GPU row");
    assert!(report.contains("SD Host Controller"), "the PCI device row");
    // The driverless device carries its MDM problem code + the honest Linux
    // reason, identical to the drawer's General tab (DEVMGR-3 reuse).
    assert!(report.contains("Code 28"), "the MDM problem code: {report}");
    assert!(
        report.contains("no kernel driver bound"),
        "the honest reason"
    );
    // A healthy device reads the working-properly line, never a fake code.
    assert!(report.contains("This device is working properly."));
}

#[test]
fn an_absent_host_exports_an_honest_empty_report_not_a_fabricated_one() {
    // Markdown names the host + an honest "no inventory" note, no device rows.
    let report = render_report(None, "ghost-node", ViewMode::ByType);
    assert!(report.contains("ghost-node"), "the host is named: {report}");
    assert!(
        report.contains("No device inventory has been published"),
        "the honest empty note: {report}"
    );
    assert!(
        !report.contains("Code"),
        "no fabricated device rows: {report}"
    );
    // JSON is an honest published:false object, never a faked inventory tree.
    let json = render_json(None, "ghost-node");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["host"], serde_json::json!("ghost-node"));
    assert_eq!(v["published"], serde_json::json!(false));
    assert!(
        v.get("categories").is_none(),
        "an absent export fabricates no category tree: {json}"
    );
}

#[test]
fn the_report_groups_to_reflect_the_active_view_mode() {
    let inv = DeviceInventory::fixture();
    // By type groups under the category labels (the default tree).
    let by_type = render_report(Some(&inv), &inv.host, ViewMode::ByType);
    assert!(by_type.contains("view: By type"), "the provenance line");
    assert!(
        by_type.contains("## Display adapters"),
        "by-type groups under category headings: {by_type}"
    );
    assert!(
        !by_type.contains("PCI bus 0000:00"),
        "by-type does not bus-group: {by_type}"
    );
    // By connection re-groups the SAME devices under the bus / controller
    // topology instead (DEVMGR-5 parity, reflected in the export).
    let by_conn = render_report(Some(&inv), &inv.host, ViewMode::ByConnection);
    assert!(
        by_conn.contains("view: By connection"),
        "the provenance line"
    );
    assert!(
        by_conn.contains("## PCI bus 0000:00"),
        "by-connection groups under bus headings: {by_conn}"
    );
    assert!(
        !by_conn.contains("## Display adapters"),
        "by-connection regroups off the function category: {by_conn}"
    );
    // The grouping changes, not the membership — every device is still listed.
    for report in [&by_type, &by_conn] {
        assert!(report.contains("Intel UHD Graphics 620"));
        assert!(report.contains("SD Host Controller"));
    }
}

#[test]
fn write_export_writes_a_real_file_that_round_trips() {
    // §7 — a real write, not a stub: the bytes land on disk and read back to
    // the same inventory, and the tmp-then-rename leaves no stray sibling.
    let scratch = ScratchRoot::new("export-write");
    let dir = scratch.path().join("exports");
    let inv = DeviceInventory::fixture();
    let json = render_json(Some(&inv), &inv.host);
    let path = write_export(&dir, "laptop-mm-by-type.json", &json).expect("the export writes");
    assert!(path.exists(), "the export file is on disk");
    let read = std::fs::read_to_string(&path).unwrap();
    let back: DeviceInventory = serde_json::from_str(&read).unwrap();
    assert_eq!(back, inv, "the written file round-trips the inventory");
    assert!(
        !dir.join(".laptop-mm-by-type.json.tmp").exists(),
        "the rename consumed the temp sibling"
    );
}

#[test]
fn a_failed_export_write_is_an_honest_error_not_a_silent_no_op() {
    // A target whose parent component is a regular file cannot be created —
    // even as root — so write_export returns the honest io::Error rather than
    // pretending success (§7 — the shell then raises an error toast).
    let scratch = ScratchRoot::new("export-fail");
    let blocker = scratch.path().join("blocker");
    std::fs::write(&blocker, "not a directory").unwrap();
    let result = write_export(&blocker.join("under-a-file"), "x.json", "{}");
    assert!(
        result.is_err(),
        "writing under a file surfaces an error, never a silent no-op"
    );
}

#[test]
fn the_export_dir_is_a_deterministic_user_data_location() {
    // No native save dialog exists on this seat, so the path is deterministic:
    // an absolute mde/device-inventory location under the user data home (or
    // the temp-dir fallback), never the cwd or a fabricated path.
    let dir = export_dir();
    assert!(dir.is_absolute(), "an absolute path: {dir:?}");
    assert!(
        dir.ends_with("mde/device-inventory") || dir.ends_with("mde-device-inventory"),
        "a stable data-home location: {dir:?}"
    );
}

#[test]
fn sanitize_keeps_hostnames_and_neutralizes_path_separators() {
    // A DNS-safe hostname + view slug + extension survives intact.
    assert_eq!(sanitize("laptop-mm-by-type.json"), "laptop-mm-by-type.json");
    // A path separator can never survive to escape the export dir.
    let hostile = sanitize("../../etc/passwd");
    assert!(!hostile.contains('/'), "no separator survives: {hostile}");
    assert_eq!(sanitize("a b/c"), "a_b_c");
}

// ── DEVMGR-7: the honest device actions (#12) ────────────────────────────

#[test]
fn the_device_action_set_is_the_honest_read_only_subset() {
    // §7/§8 — the context menu offers ONLY the actions a read-only inventory
    // consumer can perform honestly: Properties (open the drawer), Scan (re-read),
    // Copy device details. MDM's hardware-mutating verbs (Enable/Disable, Reload
    // module) are OMITTED, never greyed — there is no honest seam for them here.
    assert_eq!(DeviceAction::ALL.len(), 3);
    assert_eq!(
        DeviceAction::ALL,
        [
            DeviceAction::Properties,
            DeviceAction::Scan,
            DeviceAction::CopyDetails,
        ]
    );
    // The labels name only the honest verbs — no Disable / Enable / Reload /
    // Uninstall / Update anywhere in the offered set.
    for action in DeviceAction::ALL {
        let l = action.label().to_lowercase();
        for banned in [
            "disable",
            "enable",
            "reload",
            "uninstall",
            "update",
            "unbind",
        ] {
            assert!(
                !l.contains(banned),
                "{action:?} must not offer the mutating verb '{banned}': {l}"
            );
        }
    }
}

#[test]
fn device_details_dump_carries_every_field_and_the_problem_code() {
    // Copy-info (#12) dumps every drawer-tab field. The rich fixture GPU carries
    // ids / driver / sysfs / resources; the dump names each, plus the honest MDM
    // status line — identical to what the drawer's General tab shows.
    let gpu = DeviceInventory::fixture()
        .categories
        .into_iter()
        .find(|c| c.key == category::DISPLAY)
        .and_then(|c| c.devices.into_iter().next())
        .expect("the fixture publishes a display device");
    let dump = render_device_details(&gpu);
    assert!(dump.contains(&gpu.name), "the device name: {dump}");
    assert!(dump.contains("Status:"), "the MDM status line: {dump}");
    assert!(dump.contains("Manufacturer:") && dump.contains("Model:"));
    assert!(
        dump.contains("Driver:") && dump.contains("sysfs path:"),
        "the driver + details lines: {dump}"
    );
    assert!(
        dump.contains("Resources:") && dump.contains("Events:"),
        "the resources + events sections: {dump}"
    );
    // The driverless PCI device dumps its Code 28 + the honest Linux reason,
    // byte-for-byte the drawer's General tab (device_status_display reuse).
    let (status, _) = device_status_display(&orphan());
    let dump = render_device_details(&orphan());
    assert!(dump.contains("Code 28"), "the problem code: {dump}");
    assert!(
        dump.contains(&status),
        "the same status line the drawer shows"
    );
    assert!(dump.contains("no kernel driver bound"), "the honest reason");
}

#[test]
fn a_sparse_device_dump_is_honest_never_fabricated() {
    // A minimal record (name + status, nothing else) dumps an honest em-dash for
    // each absent scalar + a "none reported" for the empty lists (§7) — never a
    // fabricated vendor / driver / resource.
    let bare = DeviceRecord::new("Unclaimed bus device", DeviceStatus::Unknown);
    let dump = render_device_details(&bare);
    assert!(dump.contains("Device: Unclaimed bus device"));
    assert!(
        dump.contains("Manufacturer: \u{2014}") && dump.contains("Driver: \u{2014}"),
        "absent scalars dash, never fabricate: {dump}"
    );
    assert_eq!(
        dump.matches("none reported").count(),
        2,
        "both the empty Resources + Events read an honest 'none reported': {dump}"
    );
    assert!(
        !dump.contains("Code"),
        "an unknown state fabricates no code: {dump}"
    );
}

/// A headless [`egui::Context`] for driving the action seams that copy to the
/// clipboard (no GPU, no real seat — the copy queues an output command).
fn headless_ctx() -> egui::Context {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    ctx
}

#[test]
fn apply_row_action_dispatches_each_action_to_its_real_seam() {
    let ctx = headless_ctx();
    let inv = DeviceInventory::fixture();

    // Properties OPENS the drawer for the device (never toggles it closed like a
    // row click), on the General tab.
    let mut s = state_with(Some(inv.clone()), true);
    s.active_tab = DrawerTab::Driver;
    let sel = DeviceSelection::of(category::PCI_DEVICES, &orphan());
    s.apply_row_action(RowActionRequest::Properties(sel.clone()), &ctx);
    assert_eq!(
        s.selected,
        Some(sel.clone()),
        "Properties opened the drawer"
    );
    assert_eq!(
        s.active_tab,
        DrawerTab::General,
        "Properties resets to General"
    );
    // Re-issuing Properties keeps it open (it opens, it does not toggle closed).
    s.apply_row_action(RowActionRequest::Properties(sel.clone()), &ctx);
    assert_eq!(
        s.selected,
        Some(sel),
        "Properties stays open, never toggles shut"
    );

    // Scan re-reads the inventory (the honest rescan) — seen flips even off a
    // fresh, empty state, exactly like the Action-menu Scan.
    let mut s = state_with(None, false);
    s.apply_row_action(RowActionRequest::Scan, &ctx);
    assert!(s.seen, "the Scan action drove a real re-read");

    // Copy device details drives the clipboard seam without panicking (a live
    // seam, not dead code); it mutates no state.
    let mut s = state_with(Some(inv), true);
    let before = s.selected.clone();
    s.apply_row_action(RowActionRequest::CopyDetails(Box::new(orphan())), &ctx);
    assert_eq!(s.selected, before, "Copy touches no selection state");
}

#[test]
fn a_device_row_context_menu_renders_and_the_drawer_copy_path_is_live() {
    // The row (now carrying the DEVMGR-7 context menu) + the drawer (now carrying
    // the Copy button) both render headless — a live render, not dead code. The
    // context menu itself only opens on right-click, but attaching it is exercised
    // by the row render, and its seams (apply_row_action / render_device_details)
    // are covered above.
    let inv = DeviceInventory::fixture();
    let mut s = state_with(Some(inv), true);
    s.expand_all();
    s.selected = Some(DeviceSelection::of(category::PCI_DEVICES, &orphan()));
    assert!(
        drive(&mut s) > 0,
        "the action-carrying rows + drawer drew nothing"
    );
    assert!(
        s.selected.is_some(),
        "the drawer stayed open across the frame"
    );
}

// ── DEVMGR-8: the privileged, armed, node-side device actions ─────────────

/// A device record carrying a real sysfs anchor + a bound driver (the fields
/// the node-side executor needs), for the arming/dispatch tests.
fn nic() -> DeviceRecord {
    DeviceRecord {
        sysfs_path: Some("/sys/bus/pci/devices/0000:00:1f.6".into()),
        driver: Some("e1000e".into()),
        ..DeviceRecord::new("Intel I219-V", DeviceStatus::Ok)
    }
}

#[test]
fn the_context_menu_now_offers_every_privileged_op() {
    // The armed action set IS exactly MDM's four hardware-mutating verbs (#12) —
    // now PRESENT (DEVMGR-8's node-side seam exists), no longer omitted.
    assert_eq!(DeviceControlOp::ALL.len(), 4);
    // device_target carries the exact exec fields the node's executor resolves
    // the seam from (§9 — typed params, not a command).
    let t = device_target(category::NETWORK_ADAPTERS, &nic());
    assert_eq!(t.name, "Intel I219-V");
    assert_eq!(t.category, category::NETWORK_ADAPTERS);
    assert_eq!(
        t.sysfs_path.as_deref(),
        Some("/sys/bus/pci/devices/0000:00:1f.6")
    );
    assert_eq!(t.driver.as_deref(), Some("e1000e"));
}

#[test]
fn a_privileged_op_arms_first_and_never_fires_directly() {
    // Choosing Disable from the context menu stages the typed-arming confirm —
    // it does NOT dispatch (#14). Nothing routes to a node until the echo arms.
    let mut s = state_with(Some(host_inventory("laptop-mm")), true);
    let ctx = egui::Context::default();
    s.apply_row_action(
        RowActionRequest::Control {
            op: DeviceControlOp::Disable,
            target: Box::new(device_target(category::NETWORK_ADAPTERS, &nic())),
        },
        &ctx,
    );
    let arming = s
        .arming
        .as_ref()
        .expect("Disable opens the typed-arming stage, never fires directly");
    assert_eq!(arming.op, DeviceControlOp::Disable);
    assert_eq!(arming.target.name, "Intel I219-V");
    assert_eq!(arming.target_host, "laptop-mm");
    assert!(
        arming.typed.is_empty(),
        "arms empty — nothing dispatched yet"
    );
}

#[test]
fn typed_arming_blocks_an_unconfirmed_disable() {
    // The single gate the confirm button + the test share (#14).
    assert!(
        !device_armed("", "Intel I219-V"),
        "an empty echo never arms"
    );
    assert!(
        !device_armed("intel i219-v", "Intel I219-V"),
        "a mistyped echo never arms"
    );
    assert!(
        device_armed("  Intel I219-V  ", "Intel I219-V"),
        "the exact device name (trimmed) arms"
    );
}

#[test]
fn dispatch_to_a_fresh_host_writes_the_request_to_the_targets_replicated_dir() {
    // A confirmed op routes to the RAIL-selected host's replicated dir (#13) —
    // the node's device_control worker drains it. A real write, not a stub.
    let scratch = ScratchRoot::new("dispatch");
    scratch.publish("edge-2", now_ms()); // a fresh (reachable) target host
    let mut s = state_with(None, true);
    s.workgroup_root = scratch.path().to_path_buf();
    s.local_host = "laptop-mm".into();
    s.selected_host = "edge-2".into();
    s.refresh(); // builds the rail → edge-2 is a Fresh, reachable host

    s.dispatch_control(DeviceArming {
        op: DeviceControlOp::Disable,
        target: device_target(category::NETWORK_ADAPTERS, &nic()),
        target_host: "edge-2".into(),
        typed: "Intel I219-V".into(),
    });

    let reqs = mackes_mesh_types::device_control::take_requests(scratch.path(), "edge-2");
    assert_eq!(
        reqs.len(),
        1,
        "the armed op wrote one request to edge-2's dir"
    );
    assert_eq!(reqs[0].op, DeviceControlOp::Disable);
    assert_eq!(reqs[0].target_host, "edge-2");
    assert_eq!(reqs[0].target.name, "Intel I219-V");
    assert_eq!(
        reqs[0].from, "peer:laptop-mm",
        "the requesting seat is recorded"
    );
}

#[test]
fn dispatch_to_an_absent_host_writes_nothing_honest_error() {
    // §7 — a target host that has published no inventory is offline/never-seen:
    // the dispatch refuses (an honest error toast) and writes NO request, never
    // a silent no-op that would leave a request no node ever drains.
    let scratch = ScratchRoot::new("absent");
    let mut s = state_with(None, true);
    s.workgroup_root = scratch.path().to_path_buf();
    s.local_host = "laptop-mm".into();
    s.selected_host = "ghost-node".into();
    s.refresh(); // ghost-node published nothing → not in the rail → Absent
    assert_eq!(s.selected_host_freshness(), HostFreshness::Absent);

    s.dispatch_control(DeviceArming {
        op: DeviceControlOp::Disable,
        target: DeviceTarget::new("Ghost NIC", category::NETWORK_ADAPTERS),
        target_host: "ghost-node".into(),
        typed: "Ghost NIC".into(),
    });
    assert!(
        mackes_mesh_types::device_control::take_requests(scratch.path(), "ghost-node").is_empty(),
        "an absent host must get no dispatched request"
    );
}

#[test]
fn the_arming_banner_renders_headless_with_a_pending_confirm() {
    // A staged arming draws its warn banner (the reach-loss caption for a NIC)
    // — a live render, not dead code.
    let mut s = state_with(Some(host_inventory("laptop-mm")), true);
    s.arming = Some(DeviceArming {
        op: DeviceControlOp::Disable,
        target: device_target(category::NETWORK_ADAPTERS, &nic()),
        target_host: "laptop-mm".into(),
        typed: String::new(),
    });
    assert!(drive(&mut s) > 0, "the arming banner drew nothing");
    assert!(
        s.arming.is_some(),
        "an unconfirmed arming persists across the frame"
    );
}

// ── DEVMGR-10: the By-node cross-fleet view ──────────────────────────────

/// A fixture inventory re-hosted under `host` with every device forced healthy
/// — a clean node (0 problems) for the By-node ranking tests.
fn clean_host(host: &str) -> DeviceInventory {
    let mut inv = host_inventory(host);
    for cat in &mut inv.categories {
        for dev in &mut cat.devices {
            dev.status = DeviceStatus::Ok;
            dev.problem = None;
        }
    }
    inv
}

/// A clean host with `problems` extra faulted devices bolted on — a node whose
/// `problem_count` is exactly `problems`, for the By-node ranking + badge tests.
fn faulted_host(host: &str, problems: usize) -> DeviceInventory {
    let mut inv = clean_host(host);
    let devs: Vec<DeviceRecord> = (0..problems)
        .map(|i| {
            let mut d = DeviceRecord::new(format!("Faulted device {i}"), DeviceStatus::Degraded);
            d.problem = Some("simulated I/O fault".into());
            d
        })
        .collect();
    inv.categories.push(device_inventory::DeviceCategory::new(
        category::SENSORS,
        devs,
    ));
    inv
}

#[test]
fn by_node_aggregates_every_host_into_one_cross_fleet_tree() {
    // #3 — read_all delivers every published host; build_node_tree flattens
    // them all into ONE tree, each host a top-level branch carrying its own
    // device tree (host → its devices), so a fleet scan sees every node at once.
    let all = vec![
        host_inventory("alpha"),
        host_inventory("beta"),
        host_inventory("gamma"),
    ];
    let tree = build_node_tree(&all, "alpha"); // alpha published → no absent inject
    assert_eq!(
        tree.hosts.len(),
        3,
        "every published host is a top-level node"
    );
    for h in &tree.hosts {
        assert_eq!(h.device_count, 2, "{} carries its own device tree", h.host);
        assert!(
            !h.categories.is_empty(),
            "{}'s categories nest under it",
            h.host
        );
    }
    // The aggregate spans the whole fleet — three fixture hosts × 2 devices.
    let total: usize = tree.hosts.iter().map(|h| h.device_count).sum();
    assert_eq!(total, 6, "the tree aggregates the whole fleet's devices");
}

#[test]
fn by_node_ranks_problem_hosts_above_clean_ones_with_exact_counts() {
    // #3 — problem hosts sort near the top (most problems highest) so a fleet
    // scan surfaces faults first; clean hosts follow alphabetically. And the
    // per-host problem count is exact (the ⚠ N badge is truthful).
    let all = vec![
        clean_host("aaa-clean"),    // 0 problems, alphabetically first
        faulted_host("mmm-two", 2), // 2 problems
        clean_host("zzz-clean"),    // 0 problems
        faulted_host("bbb-one", 1), // 1 problem
    ];
    let tree = build_node_tree(&all, "aaa-clean");
    let order: Vec<&str> = tree.hosts.iter().map(|h| h.host.as_str()).collect();
    assert_eq!(
        order,
        vec!["mmm-two", "bbb-one", "aaa-clean", "zzz-clean"],
        "problem hosts (most first) rank above clean hosts"
    );
    let count = |h: &str| {
        tree.hosts
            .iter()
            .find(|n| n.host == h)
            .unwrap()
            .problem_count
    };
    assert_eq!(count("mmm-two"), 2, "the per-host problem count is exact");
    assert_eq!(count("bbb-one"), 1);
    assert_eq!(count("aaa-clean"), 0);
    assert_eq!(count("zzz-clean"), 0);
}

#[test]
fn by_node_renders_an_absent_host_honestly_and_sinks_it() {
    // §7 — the local "you are here" node that has published nothing is still
    // present in the cross-fleet tree as an honest absent leaf (no device tree),
    // sunk below the published hosts, and NOT expandable.
    let all = vec![host_inventory("edge-1")]; // one published, faulted peer
    let tree = build_node_tree(&all, "laptop-mm"); // local never published
    let names: Vec<&str> = tree.hosts.iter().map(|h| h.host.as_str()).collect();
    assert_eq!(
        names,
        vec!["edge-1", "laptop-mm"],
        "the absent local sinks below the published peer"
    );
    let local = tree.hosts.iter().find(|h| h.host == "laptop-mm").unwrap();
    assert_eq!(local.published_at_ms, None, "absent — nothing published");
    assert_eq!(local.device_count, 0, "no fabricated devices");
    assert!(local.categories.is_empty(), "no fabricated category tree");
    // Expand-all only fills PUBLISHED host keys — an absent host is a leaf.
    assert_eq!(
        tree.host_keys(),
        BTreeSet::from(["node:edge-1".to_string()])
    );
}

#[test]
fn switching_to_by_node_preserves_the_fleet_inventory_set_and_renders() {
    // Flipping to By-node re-groups the SAME cross-fleet data (read_all) into
    // the host-flattened tree without changing which hosts are loaded; expand-all
    // is then host-keyed (mode-aware), and the tree renders headless (a live
    // render across the fleet, not dead code).
    let scratch = ScratchRoot::new("by-node");
    scratch.publish("laptop-mm", 1_000);
    scratch.publish("edge-1", 2_000);
    scratch.publish("edge-2", 3_000);
    let mut s = state_with(None, false);
    s.workgroup_root = scratch.path().to_path_buf();
    s.refresh();
    assert_eq!(s.all_inventories.len(), 3, "read_all kept the whole fleet");
    // Flip to By-node — the seam the View menu drives.
    s.apply(MenuAction::View(ViewMode::ByNode));
    assert_eq!(s.view, ViewMode::ByNode);
    // The inventory set is unchanged by the view flip (same three hosts).
    let hosts: BTreeSet<&str> = s.all_inventories.iter().map(|i| i.host.as_str()).collect();
    assert_eq!(
        hosts,
        BTreeSet::from(["laptop-mm", "edge-1", "edge-2"]),
        "the fleet set survives the view flip"
    );
    // Expand-all fills the HOST keys (mode-aware), one per published host.
    s.expand_all();
    assert_eq!(
        s.expanded,
        build_node_tree(&s.all_inventories, &s.local_host).host_keys()
    );
    assert_eq!(s.expanded.len(), 3, "one expand key per published host");
    assert!(
        drive(&mut s) > 0,
        "the by-node cross-fleet tree drew nothing"
    );
}

#[test]
fn a_by_node_device_click_jumps_the_inspected_host() {
    // In By-node the tree spans the fleet; clicking a device on another host is
    // an honest cross-fleet jump — the rail-selected host follows so the drawer
    // resolves against the right host (DEVMGR-4 selection stays truthful).
    let scratch = ScratchRoot::new("by-node-jump");
    scratch.publish("laptop-mm", 1_000);
    scratch.publish("edge-2", 2_000);
    let mut s = state_with(None, false);
    s.workgroup_root = scratch.path().to_path_buf();
    s.refresh();
    s.view = ViewMode::ByNode;
    assert_eq!(s.selected_host, "laptop-mm", "local is the default host");
    // Click a device that lives on edge-2 (not the current host).
    let sel = DeviceSelection::of(category::PCI_DEVICES, &orphan());
    s.select_node_device("edge-2".to_string(), sel.clone());
    assert_eq!(s.selected_host, "edge-2", "the rail follows the click");
    assert_eq!(s.inventory.as_ref().unwrap().host, "edge-2");
    assert_eq!(
        s.selected,
        Some(sel.clone()),
        "the clicked device's drawer opened"
    );
    assert_eq!(s.active_tab, DrawerTab::General, "opens on General");
    // Clicking a device already on the selected host toggles the drawer (no jump).
    s.select_node_device("edge-2".to_string(), sel);
    assert_eq!(
        s.selected, None,
        "a same-host re-click toggles the drawer closed"
    );
    assert_eq!(s.selected_host, "edge-2", "no jump within the same host");
}

#[test]
fn by_node_renders_the_fleet_even_when_the_selected_host_is_absent() {
    // §7 — By-node reads the WHOLE fleet, so it renders the other hosts even
    // when the rail-selected host itself has published nothing; it never falls
    // into the single-host empty state.
    let scratch = ScratchRoot::new("by-node-absent-sel");
    scratch.publish("laptop-mm", now_ms());
    scratch.publish("edge-1", now_ms());
    let mut s = state_with(None, false);
    s.workgroup_root = scratch.path().to_path_buf();
    s.refresh();
    s.view = ViewMode::ByNode;
    s.expand_all(); // expand every host so the device rows render too
    assert!(drive(&mut s) > 0, "the by-node tree drew nothing");
    // Select a host that published nothing — the selected inventory goes None.
    s.select_host("ghost-node".to_string());
    assert!(s.inventory.is_none(), "the selected host is absent");
    assert!(
        drive(&mut s) > 0,
        "by-node still renders the fleet despite an absent selection"
    );
}

// ── DEVMGR-11: the non-PC host types (#6/#22) ─────────────────────────────

use super::{
    lan_host, merge_rail, nova_host, phone_host, router_host, CloudDetailMirror, ExtrasMirror,
    HostKind, PhoneMirror, RouterMirror, UnitKindMirror, UnitMirror,
};

/// A Nova instance unit as the `state/units` mirror reports it (EXPLORER-9
/// detail present).
fn nova_unit() -> UnitMirror {
    UnitMirror {
        id: "cloud:instance:0f3a".into(),
        kind: UnitKindMirror::Instance,
        name: "web-1".into(),
        address: Some("10.0.0.5".into()),
        health: None,
        cloud: Some(CloudDetailMirror {
            flavor: Some("m1.small".into()),
            vcpus: Some(2),
            ram_mb: Some(2048),
            disk_gb: Some(20),
            status: Some("ACTIVE".into()),
            fixed_ips: vec!["10.0.0.5".into()],
            floating_ips: vec!["203.0.113.9".into()],
            ports: vec!["p-1".into()],
            created: None,
        }),
        extras: ExtrasMirror::default(),
        last_seen_ms: now_ms(),
    }
}

/// A LAN-scan unit with the EXPLORER-9 enrichment facts.
fn lan_unit() -> UnitMirror {
    UnitMirror {
        id: "lan:192.168.1.50".into(),
        kind: UnitKindMirror::LanHost,
        name: "printer.local".into(),
        address: Some("192.168.1.50".into()),
        health: Some("degraded".into()),
        cloud: None,
        extras: ExtrasMirror {
            rdns: Some("printer.local".into()),
            oui_vendor: Some("Brother Industries".into()),
            fingerprint: Some("ipp \u{2014} looks like a printer".into()),
        },
        last_seen_ms: now_ms(),
    }
}

fn paired_phone() -> PhoneMirror {
    PhoneMirror {
        device_id: "abc123".into(),
        device_name: "Pixel 8".into(),
        overlay_ip: Some("10.42.0.7".into()),
        fingerprint: "AA:BB:CC".into(),
        paired_at_ms: 1_720_000_000_000,
    }
}

fn edge_router() -> RouterMirror {
    RouterMirror {
        id: "aa:bb:cc:dd:ee:ff".into(),
        ip: "172.20.0.1".into(),
        node_id: "peer:eagle".into(),
        vendor: "edgeos".into(),
        version: "EdgeOS v2.0.9".into(),
        managed: true,
        needs_creds: false,
        is_default: true,
    }
}

#[test]
fn a_nova_instance_maps_to_a_virtio_only_tree_off_the_units_mirror() {
    // #22 — "a Nova instance shows virtio devices": ports → virtio-net, the
    // flavor's root disk → virtio-blk; vCPU/RAM land in the header summary,
    // never as fabricated device rows; no other category is invented.
    let h = nova_host(&nova_unit());
    assert_eq!(h.kind, HostKind::Nova);
    assert_eq!(h.key, "cloud:instance:0f3a");
    assert_eq!(h.inventory.host, "web-1");
    let keys: Vec<&str> = h
        .inventory
        .categories
        .iter()
        .map(|c| c.key.as_str())
        .collect();
    assert_eq!(keys, vec!["virtio"], "exactly the virtio category");
    let virtio = &h.inventory.categories[0];
    assert_eq!(virtio.label, "Virtio devices");
    assert!(!virtio.devices.is_empty(), "never an empty category (#22)");
    // One NIC per reported IP (fixed + floating) + the root disk.
    assert_eq!(virtio.devices.len(), 3);
    assert!(virtio.devices[0]
        .events
        .iter()
        .any(|e| e == "fixed IP 10.0.0.5"));
    assert!(virtio.devices[1]
        .events
        .iter()
        .any(|e| e == "floating IP 203.0.113.9"));
    assert!(virtio.devices[2].name.contains("20 GiB root disk"));
    assert!(virtio
        .devices
        .iter()
        .all(|d| d.events.iter().any(|e| e == "instance status: ACTIVE")));
    // Flavor facts ride the summary (real), the rest stays an honest None.
    assert_eq!(h.inventory.summary.cpu_count, Some(2));
    assert_eq!(h.inventory.summary.mem_total_kb, Some(2048 * 1024));
    assert_eq!(h.inventory.summary.os, None, "guest OS is unreported");
}

#[test]
fn a_detailless_nova_instance_shows_no_fabricated_tree() {
    // §7 — no reported Nova detail ⇒ zero categories + an explicit note,
    // never an invented virtio tree.
    let mut u = nova_unit();
    u.cloud = None;
    let h = nova_host(&u);
    assert!(h.inventory.categories.is_empty());
    assert!(h
        .note
        .as_deref()
        .unwrap()
        .contains("no attached-device detail"));
}

#[test]
fn a_paired_phone_maps_to_radios_only_and_never_fabricates_power_or_sensors() {
    // #22 — "phone → Power/Sensors/Radios": the KDC pairing roster can honestly
    // answer only the network-radio path (the overlay IP proves it); Power +
    // Sensors are explicitly unreported in the note, never invented or empty.
    let h = phone_host(&paired_phone(), "eagle");
    assert_eq!(h.kind, HostKind::Phone);
    assert_eq!(h.key, "phone:abc123");
    assert_eq!(h.inventory.host, "Pixel 8");
    let keys: Vec<&str> = h
        .inventory
        .categories
        .iter()
        .map(|c| c.key.as_str())
        .collect();
    assert_eq!(keys, vec!["radios"], "exactly the radios category");
    let radio = &h.inventory.categories[0].devices[0];
    assert!(radio.events.iter().any(|e| e == "overlay IP 10.42.0.7"));
    assert!(radio.events.iter().any(|e| e == "paired via eagle"));
    assert!(radio
        .events
        .iter()
        .any(|e| e.contains("fingerprint pinned")));
    assert!(radio.events.iter().any(|e| e.contains("unreported")));
    let note = h.note.as_deref().unwrap();
    assert!(note.contains("Power and Sensors are unreported"));

    // No overlay IP ⇒ not even the radio link can be shown — zero categories,
    // the note says why (§7), never a fabricated row.
    let mut p = paired_phone();
    p.overlay_ip = None;
    let bare = phone_host(&p, "eagle");
    assert!(bare.inventory.categories.is_empty());
    assert!(bare.note.as_deref().unwrap().contains("no overlay IP"));
}

#[test]
fn a_lan_host_maps_to_the_observed_nic_with_only_detectable_facts() {
    // #22 — "LAN → what's remotely detectable": one observed NIC carrying the
    // scan's real facts (address, OUI vendor, rDNS, service fingerprint); a
    // reported degraded health is an honest problem, not a guess.
    let h = lan_host(&lan_unit());
    assert_eq!(h.kind, HostKind::Lan);
    assert_eq!(h.key, "lan:192.168.1.50");
    let keys: Vec<&str> = h
        .inventory
        .categories
        .iter()
        .map(|c| c.key.as_str())
        .collect();
    assert_eq!(keys, vec![category::NETWORK_ADAPTERS]);
    let nic = &h.inventory.categories[0].devices[0];
    assert!(nic.name.contains("192.168.1.50"));
    assert_eq!(nic.vendor.as_deref(), Some("Brother Industries"));
    assert!(nic.events.iter().any(|e| e.contains("printer.local")));
    assert!(nic.events.iter().any(|e| e.contains("service fingerprint")));
    assert_eq!(nic.status, DeviceStatus::Degraded);
    assert!(nic.problem.as_deref().unwrap().contains("degraded"));
}

#[test]
fn a_router_maps_to_network_system_firmware_and_gates_what_it_cannot_read() {
    // #22 — "router → Network/System/Firmware": a managed, fingerprinted
    // appliance fills all three off real registry facts.
    let h = router_host(&edge_router());
    assert_eq!(h.kind, HostKind::Router);
    assert_eq!(h.key, "router:aa:bb:cc:dd:ee:ff");
    let keys: Vec<&str> = h
        .inventory
        .categories
        .iter()
        .map(|c| c.key.as_str())
        .collect();
    assert_eq!(keys, vec![category::NETWORK_ADAPTERS, "system", "firmware"]);
    let nic = &h.inventory.categories[0].devices[0];
    assert!(nic.events.iter().any(|e| e == "MAC aa:bb:cc:dd:ee:ff"));
    assert!(nic
        .events
        .iter()
        .any(|e| e.contains("primary default route")));
    assert!(h.inventory.categories[1].devices[0].name.contains("EdgeOS"));
    assert!(h.inventory.categories[2].devices[0].name.contains("v2.0.9"));
    for c in &h.inventory.categories {
        assert!(!c.devices.is_empty(), "never an empty category (#22)");
    }

    // Unfingerprinted + credential-less ⇒ only the Network facts exist; System
    // and Firmware are ABSENT (not fabricated), and the note names the gate.
    let r = RouterMirror {
        vendor: "unknown".into(),
        version: String::new(),
        managed: false,
        needs_creds: true,
        ..edge_router()
    };
    let bare = router_host(&r);
    let keys: Vec<&str> = bare
        .inventory
        .categories
        .iter()
        .map(|c| c.key.as_str())
        .collect();
    assert_eq!(keys, vec![category::NETWORK_ADAPTERS]);
    assert!(bare
        .note
        .as_deref()
        .unwrap()
        .contains("no router credential"));
}

#[test]
fn the_rail_groups_host_kinds_in_order_with_collision_proof_keys() {
    // #6 — the rail = mesh nodes first (local pinned), then Cloud → Phones →
    // LAN → Routers; every non-PC key is source-namespaced so a node hostname
    // can never shadow it.
    let nodes = build_rail(&[host_inventory("laptop-mm")], "laptop-mm");
    let non_pc = vec![
        router_host(&edge_router()),
        lan_host(&lan_unit()),
        phone_host(&paired_phone(), "eagle"),
        nova_host(&nova_unit()),
    ];
    let rail = merge_rail(nodes, &non_pc);
    let kinds: Vec<HostKind> = rail.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        vec![
            HostKind::Node,
            HostKind::Nova,
            HostKind::Phone,
            HostKind::Lan,
            HostKind::Router
        ]
    );
    // Labels are the display names, keys the namespaced ids.
    let phone = rail.iter().find(|e| e.kind == HostKind::Phone).unwrap();
    assert_eq!(phone.label, "Pixel 8");
    assert_eq!(phone.host, "phone:abc123");
    // A router registry row carries no publish time — its freshness reads an
    // honest Stale (unknown age), never a fabricated "fresh".
    let router = rail.iter().find(|e| e.kind == HostKind::Router).unwrap();
    assert_eq!(router.freshness(now_ms()), HostFreshness::Stale);
    // The keys never collide.
    let mut keys: Vec<&str> = rail.iter().map(|e| e.host.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), rail.len());
}

#[test]
fn refresh_folds_every_non_pc_source_and_selecting_one_loads_its_tree() {
    // Each host type flows from its REAL source (#6): the `state/units/<node>`
    // Bus mirror (Nova + LAN), the replicated `kdc-phones/<host>.json` roster
    // (phones), and `<host>/router-registry.json` (routers) — end to end
    // through `refresh`, selection, and a headless render.
    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;

    let scratch = ScratchRoot::new("nonpc");
    scratch.publish("laptop-mm", now_ms());
    // The KDC pairing roster eagle published.
    let phones = scratch.path().join("kdc-phones");
    std::fs::create_dir_all(&phones).unwrap();
    std::fs::write(
        phones.join("eagle.json"),
        serde_json::json!({
            "host_device_id": "h-eagle",
            "host_overlay_ip": "10.42.0.2",
            "devices": [{
                "device_id": "abc123",
                "device_name": "Pixel 8",
                "overlay_ip": "10.42.0.7",
                "fingerprint": "AA:BB:CC",
                "paired_at_ms": 1_720_000_000_000_i64,
            }],
        })
        .to_string(),
    )
    .unwrap();
    // The router-registry mirror eagle wrote.
    let eagle_dir = scratch.path().join("eagle");
    std::fs::create_dir_all(&eagle_dir).unwrap();
    std::fs::write(
        eagle_dir.join("router-registry.json"),
        serde_json::json!({
            "id": "aa:bb:cc:dd:ee:ff",
            "ip": "172.20.0.1",
            "node_id": "peer:eagle",
            "vendor": "edgeos",
            "version": "EdgeOS v2.0.9",
            "managed": true,
            "needs_creds": false,
            "is_default": true,
        })
        .to_string(),
    )
    .unwrap();
    // The units mirror on the Bus spool: an instance + a LAN host (+ a peer
    // and a volume, which are NOT rail material and must be ignored).
    let bus = scratch.path().join("bus");
    let persist = Persist::open(bus.clone()).unwrap();
    let body = serde_json::json!({
        "host": "laptop-mm",
        "units": [
            {
                "id": "cloud:instance:0f3a", "kind": "instance", "name": "web-1",
                "reachability": {"where": "cloud_object", "node": "laptop-mm"},
                "cloud": {"vcpus": 2, "ram_mb": 2048, "disk_gb": 20,
                           "status": "ACTIVE", "fixed_ips": ["10.0.0.5"]},
                "last_seen_ms": 5,
            },
            {
                "id": "lan:192.168.1.50", "kind": "lan_host", "name": "printer.local",
                "reachability": {"where": "on_lan"},
                "address": "192.168.1.50",
                "extras": {"oui_vendor": "Brother Industries"},
                "last_seen_ms": 5,
            },
            {"id": "peer:eagle", "kind": "peer", "name": "eagle",
             "reachability": {"where": "in_mesh"}},
            {"id": "cloud:volume:v1", "kind": "volume", "name": "data",
             "reachability": {"where": "cloud_object", "node": "laptop-mm"}},
        ],
    })
    .to_string();
    persist
        .write(
            "state/units/laptop-mm",
            Priority::Default,
            None,
            Some(&body),
        )
        .unwrap();

    let mut s = state_with(None, false);
    s.workgroup_root = scratch.path().to_path_buf();
    s.bus_root = Some(bus);
    s.refresh();

    let kind_of = |key: &str| s.hosts.iter().find(|e| e.host == key).map(|e| e.kind);
    assert_eq!(kind_of("laptop-mm"), Some(HostKind::Node));
    assert_eq!(kind_of("cloud:instance:0f3a"), Some(HostKind::Nova));
    assert_eq!(kind_of("phone:abc123"), Some(HostKind::Phone));
    assert_eq!(kind_of("lan:192.168.1.50"), Some(HostKind::Lan));
    assert_eq!(kind_of("router:aa:bb:cc:dd:ee:ff"), Some(HostKind::Router));
    assert_eq!(
        kind_of("peer:eagle"),
        None,
        "a mesh peer unit is not rail material (it rails via its inventory)"
    );

    // Selecting the instance loads its synthesized virtio tree + source note.
    s.select_host("cloud:instance:0f3a".to_string());
    let inv = s.inventory.as_ref().expect("the non-PC tree loads");
    assert_eq!(inv.host, "web-1");
    assert_eq!(inv.categories.len(), 1);
    assert_eq!(inv.categories[0].key, "virtio");
    assert!(s.selected_note().unwrap().contains("virtio"));
    assert!(drive(&mut s) > 0, "the non-PC tree renders headless");

    // Selecting the phone renders its radios-only tree.
    s.select_host("phone:abc123".to_string());
    let inv = s.inventory.as_ref().unwrap();
    assert_eq!(inv.host, "Pixel 8");
    assert_eq!(inv.categories[0].key, "radios");
    assert!(drive(&mut s) > 0, "the phone tree renders headless");
}

#[test]
fn non_pc_hosts_never_take_a_privileged_device_op() {
    // §7 — only a mesh node runs the device_control worker; the kind gate is
    // both a menu omission (controllable()) and a dispatch-seam backstop: an
    // armed op against a phone writes NO request anywhere.
    assert!(HostKind::Node.controllable());
    for kind in [
        HostKind::Nova,
        HostKind::Phone,
        HostKind::Lan,
        HostKind::Router,
    ] {
        assert!(!kind.controllable(), "{kind:?} must not take device ops");
    }

    let scratch = ScratchRoot::new("nonpc-refuse");
    let phones = scratch.path().join("kdc-phones");
    std::fs::create_dir_all(&phones).unwrap();
    std::fs::write(
        phones.join("eagle.json"),
        serde_json::json!({"devices": [{
            "device_id": "abc123", "device_name": "Pixel 8",
            "overlay_ip": "10.42.0.7", "paired_at_ms": 1_i64,
        }]})
        .to_string(),
    )
    .unwrap();
    let mut s = state_with(None, true);
    s.workgroup_root = scratch.path().to_path_buf();
    s.refresh();
    s.select_host("phone:abc123".to_string());
    assert_eq!(s.selected_kind(), HostKind::Phone);

    s.dispatch_control(DeviceArming {
        op: DeviceControlOp::Disable,
        target: DeviceTarget::new("Network radio (mesh overlay link)", "radios"),
        target_host: "phone:abc123".into(),
        typed: "Network radio (mesh overlay link)".into(),
    });
    assert!(
        mackes_mesh_types::device_control::take_requests(scratch.path(), "phone:abc123").is_empty(),
        "a non-node target must get no dispatched request"
    );
}

// ── MENU-5: the bar covers the extended Device Manager ───────────────────

#[test]
fn the_hosts_menu_surfaces_rail_node_switching_incl_non_pc_kinds() {
    // MENU-5 (#5/#6) — the Hosts menu is the bar twin of the host rail: a
    // Refresh-this-host seam, then every rail row grouped by kind, the selected
    // host checked. The non-PC kinds ride it too (Cloud / Phones).
    let nodes = build_rail(&[host_inventory("laptop-mm")], "laptop-mm");
    let non_pc = vec![
        nova_host(&nova_unit()),
        phone_host(&paired_phone(), "eagle"),
    ];
    let mut s = state_with(Some(DeviceInventory::fixture()), true);
    s.hosts = merge_rail(nodes, &non_pc);
    s.selected_host = "laptop-mm".into();

    let menus = s.build_menus();
    let hosts = &menus[2];
    assert_eq!(hosts.title, "Hosts");
    // The rail's ↻ live-refresh is the first item (== Action → Scan, one seam).
    assert_eq!(item_ids(hosts)[0], MenuAction::Scan);
    // Grouped section captions mirror the rail (only kinds that have rows).
    let captions: Vec<&str> = hosts
        .entries
        .iter()
        .filter_map(|e| match e {
            Entry::Caption(c) => Some(c.as_str()),
            _ => None,
        })
        .collect();
    for want in ["Mesh nodes", "Cloud instances", "Phones"] {
        assert!(
            captions.contains(&want),
            "the {want} section header is present"
        );
    }
    // A SelectHost item per rail row, the selected node checked, another not.
    let select_of = |key: &str| {
        hosts.entries.iter().find_map(|e| match e {
            Entry::Item(it) if it.id == MenuAction::SelectHost(key.to_string()) => Some(it),
            _ => None,
        })
    };
    assert_eq!(
        select_of("laptop-mm")
            .expect("local node is a switch target")
            .checked,
        Some(true),
        "the selected host is the checked radio"
    );
    assert_eq!(
        select_of("cloud:instance:0f3a")
            .expect("the Nova instance is a switch target")
            .checked,
        Some(false)
    );
    assert!(
        select_of("phone:abc123").is_some(),
        "the paired phone is a bar switch target (#6)"
    );

    // Activating one switches the inspected host (the rail-click seam).
    let mut s2 = state_with(Some(DeviceInventory::fixture()), true);
    s2.apply(MenuAction::SelectHost("cloud:instance:0f3a".into()));
    assert_eq!(s2.selected_host, "cloud:instance:0f3a");
}

#[test]
fn the_device_menu_carries_the_armed_posture_gated_by_selection() {
    // MENU-5 (#12/#14) — on a mesh node the Device menu carries the four DEVMGR-8
    // armed verbs + Copy details, context-gated on a live device selection.
    let mut s = state_with(Some(DeviceInventory::fixture()), true);
    s.selected = Some(DeviceSelection::of(category::PCI_DEVICES, &orphan()));
    let device = &s.build_menus()[3];
    assert_eq!(device.title, "Device");
    let armed: Vec<DeviceControlOp> = device
        .entries
        .iter()
        .filter_map(|e| match e {
            Entry::Item(it) => match &it.id {
                MenuAction::ArmControl(op) => Some(*op),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        armed,
        DeviceControlOp::ALL.to_vec(),
        "all four armed verbs present"
    );
    // A device is selected, so both the copy + the armed verbs are enabled.
    let all_enabled = device
        .entries
        .iter()
        .all(|e| matches!(e, Entry::Item(it) if it.enabled) || !matches!(e, Entry::Item(_)));
    assert!(all_enabled, "every device verb is live with a selection");

    // No selection → every device verb greys, behind an honest leading caption.
    let none = state_with(Some(DeviceInventory::fixture()), true);
    let device = &none.build_menus()[3];
    assert!(
        device
            .entries
            .iter()
            .any(|e| matches!(e, Entry::Caption(c) if c.contains("Select a device"))),
        "no selection reads an honest caption"
    );
    let none_disabled = device
        .entries
        .iter()
        .all(|e| matches!(e, Entry::Item(it) if !it.enabled) || !matches!(e, Entry::Item(_)));
    assert!(
        none_disabled,
        "every device verb greys without a selection (§7)"
    );
}

#[test]
fn a_non_pc_host_omits_the_armed_device_ops_from_the_bar() {
    // §7 — a phone runs no device_control worker, so the Device menu OMITS the
    // privileged verbs (never a greyed placebo) and discloses why.
    let mut s = state_with(Some(DeviceInventory::fixture()), true);
    s.hosts = merge_rail(
        build_rail(&[], "laptop-mm"),
        &[phone_host(&paired_phone(), "eagle")],
    );
    s.selected_host = "phone:abc123".into();
    assert_eq!(s.selected_kind(), HostKind::Phone);
    let device = &s.build_menus()[3];
    let has_armed = device
        .entries
        .iter()
        .any(|e| matches!(e, Entry::Item(it) if matches!(it.id, MenuAction::ArmControl(_))));
    assert!(!has_armed, "a non-PC host offers no armed verb in the bar");
    assert!(
        device
            .entries
            .iter()
            .any(|e| matches!(e, Entry::Caption(c) if c.contains("mesh nodes only"))),
        "the honest disclosure is present"
    );
}

#[test]
fn arm_control_stages_the_typed_arming_confirm_and_a_non_node_never_arms() {
    // MENU-5 → DEVMGR-8 (#14) — the Device-menu armed verb opens the same
    // typed-arming stage as the row context-menu, never firing directly.
    let mut s = state_with(Some(DeviceInventory::fixture()), true);
    s.selected = Some(DeviceSelection::of(category::PCI_DEVICES, &orphan()));
    s.apply(MenuAction::ArmControl(DeviceControlOp::Disable));
    let arming = s
        .arming
        .as_ref()
        .expect("ArmControl opens the arming stage");
    assert_eq!(arming.op, DeviceControlOp::Disable);
    assert_eq!(arming.target.name, orphan().name);
    assert_eq!(arming.target_host, "laptop-mm");
    assert!(
        arming.typed.is_empty(),
        "nothing dispatched until the echo arms"
    );

    // §7 — a non-PC host never arms from the bar (guarded on controllable()).
    let mut phone = state_with(Some(DeviceInventory::fixture()), true);
    phone.hosts = merge_rail(
        build_rail(&[], "laptop-mm"),
        &[phone_host(&paired_phone(), "eagle")],
    );
    phone.selected_host = "phone:abc123".into();
    phone.selected = Some(DeviceSelection::of(category::PCI_DEVICES, &orphan()));
    phone.apply(MenuAction::ArmControl(DeviceControlOp::Disable));
    assert!(
        phone.arming.is_none(),
        "a non-node host never arms from the bar"
    );
}

#[test]
fn view_jump_to_category_switches_to_by_type_and_expands() {
    // MENU-5 — a category jump lands the operator on it: By-type + expanded.
    let mut s = state_with(Some(DeviceInventory::fixture()), true);
    s.view = ViewMode::ByConnection;
    s.apply(MenuAction::JumpCategory(category::DISPLAY.to_string()));
    assert_eq!(
        s.view,
        ViewMode::ByType,
        "a jump re-roots into the by-type tree"
    );
    assert!(
        s.expanded.contains(category::DISPLAY),
        "the jumped-to category is expanded so the operator lands on it"
    );
}
