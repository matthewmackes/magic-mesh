use super::{
    console_confirm_id, console_content, console_entry_id, console_heading_id, console_power_id,
    console_rail_id, custom_sync, entry_at, identity_line, jump_caption, launch_argv, static_rows,
    tool_present, tool_present_in, total_rows, ConsoleRequest, ConsoleState, CustomEntry,
    EntryKind, GateReason, PowerAction, Provenance, CUSTOM_GROUP_LABEL, GROUPS, PANEL_H, PANEL_W,
    PINNED, POWER_H, RAIL_SECTION_GAP,
};
use crate::dock::Surface;
use crate::workbench::Plane;
use mde_egui::egui;
use mde_egui::Style;
use mde_seat::PowerVerb;

/// Drive ONE headless frame of the console content over a stand-in surface
/// (the dock tests' `drive_vdock` idiom — the same `Context::run` path the
/// DRM runner drives, minus the GPU). Mounts [`console_content`] at the
/// SAME rect the old standalone `console_panel` used to settle at
/// (bottom-left, `PANEL_W` × `PANEL_H`) so every existing coordinate-based
/// row/rail assertion below still lands on the same pixels — this helper
/// stands in for `start_menu`'s outer Area now that `console.rs` no longer
/// mounts one itself (WIN7-2). Renders content only while `state.is_open()`,
/// the closest local analogue to the old Motion-settled visibility gate.
fn drive(
    ctx: &egui::Context,
    state: &mut ConsoleState,
    events: Vec<egui::Event>,
    size: egui::Vec2,
) -> egui::FullOutput {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), size)),
        events,
        ..Default::default()
    };
    ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = ui.button("surface");
        });
        if state.is_open() {
            egui::Area::new(egui::Id::new("test-console-content-area"))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(crate::dock::DOCK_W, size.y - PANEL_H))
                .show(ctx, |ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(PANEL_W, PANEL_H), egui::Sense::hover());
                    console_content(ui, rect, state);
                });
        }
    })
}

fn key(k: egui::Key) -> egui::Event {
    egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::default(),
    }
}

fn press_at(pos: egui::Pos2) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::default(),
    }
}

fn release_at(pos: egui::Pos2) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    }
}

/// Click `center` — press one frame, release the next (the egui click model
/// the dock tests use). The caller primes the layout first.
fn click(ctx: &egui::Context, state: &mut ConsoleState, center: egui::Pos2, size: egui::Vec2) {
    drive(
        ctx,
        state,
        vec![egui::Event::PointerMoved(center), press_at(center)],
        size,
    );
    drive(ctx, state, vec![release_at(center)], size);
}

const SZ: egui::Vec2 = egui::Vec2::new(1280.0, 800.0);

// ── the entry table (design "Entry model" — real, locked, no dead rows) ──

#[test]
fn the_entry_table_matches_the_locked_taxonomy_and_holds_no_dead_rows() {
    // Lock #6 — the seven operational groups in locked order (Power joins
    // the rail and Custom the tail under CONSOLE-4).
    let labels: Vec<&str> = GROUPS.iter().map(|g| g.label).collect();
    assert_eq!(
        labels,
        [
            "System",
            "Network",
            "Packages",
            "Storage",
            "Mesh",
            "Containers & VMs",
            "Shells"
        ],
        "the locked group taxonomy + order"
    );
    // No dead entries: every group populated, every row fully described,
    // every command entry a real command line.
    for group in &GROUPS {
        assert!(!group.entries.is_empty(), "{} is empty", group.label);
    }
    for entry in static_rows() {
        assert!(!entry.label.is_empty() && !entry.desc.is_empty());
        if let EntryKind::Tab(cmd) = entry.kind {
            assert!(
                !cmd.trim().is_empty(),
                "{} has a blank command",
                entry.label
            );
            assert!(
                !entry.tool.is_empty() || cmd.starts_with("bash "),
                "{} declares no presence-check tool",
                entry.label
            );
        }
    }
    // Lock #31 — pinned is exactly a plain Terminal + Monitor: the Terminal
    // a LIVE surface link, the Monitor the btop command entry.
    assert_eq!(PINNED.len(), 2);
    assert_eq!(PINNED[0].kind, EntryKind::Link(Surface::Terminal));
    assert_eq!(PINNED[1].kind, EntryKind::Tab("btop"));
    // Lock #41 — Containers & VMs carries the Cloud-plane surface link.
    let cvm = GROUPS
        .iter()
        .find(|g| g.label == "Containers & VMs")
        .expect("the combined group exists");
    assert!(
        cvm.entries
            .iter()
            .any(|e| e.kind == EntryKind::Plane(Plane::Cloud)),
        "the Containers & VMs group links to the Cloud plane"
    );
    // The flat index space is coherent.
    assert_eq!(static_rows().count(), total_rows());
    assert_eq!(entry_at(0).label, "Terminal");
}

#[test]
fn platform_provenance_label_uses_the_canonical_build_codename() {
    let codename = mde_theme::brand::build::info().codename;

    assert_eq!(codename, "Quazar", "the current brand codename changed");
    assert_eq!(Provenance::Quasar.label(), codename);
    assert_ne!(Provenance::Quasar.label(), "Quasar");
    assert_eq!(Provenance::Quasar.color(), Style::ACCENT);
}

#[test]
fn tool_presence_is_a_real_path_check() {
    // `sh` exists on any Linux build host; a nonsense binary does not; the
    // empty tool (surface links) is always present.
    assert!(tool_present("sh"), "sh must resolve on $PATH");
    assert!(!tool_present("definitely-not-a-real-tool-xyzzy"));
    assert!(tool_present(""));
}

#[test]
fn every_entry_wears_its_own_domain_glyph_and_links_match_their_surface() {
    // Lock #33 — every row declares its OWN domain glyph (not one blanket
    // terminal icon), and a surface-link entry wears its surface's own
    // glyph so the iconography stays 1:1 with the surface identity.
    let mut glyphs = std::collections::BTreeSet::new();
    for entry in static_rows() {
        if let EntryKind::Link(surface) = entry.kind {
            assert_eq!(
                entry.icon,
                surface.icon_id(),
                "{} links to {surface:?} but wears a different glyph",
                entry.label,
            );
        }
        glyphs.insert(entry.icon.name());
    }
    // The table spans several distinct domain glyphs — proof it is NOT the
    // old wall of identical terminal icons.
    assert!(
        glyphs.len() >= 6,
        "the entry table should span several domain glyphs, saw {glyphs:?}"
    );
}

#[test]
fn every_declared_tool_resolves_against_a_fixture_path() {
    // The §7 honest gate's positive proof: stage a stub executable for
    // every tool the table declares, then assert every entry resolves
    // present on that fixture $PATH — every entry maps to a REAL,
    // correctly-named command (a typo'd tool would fail here), while an
    // unstaged name stays absent (the greying's ground truth). No global
    // env mutation — the fixture PATH is passed straight to the core.
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = dir.path();
    let tools: std::collections::BTreeSet<&str> = static_rows()
        .map(|e| e.tool)
        .filter(|t| !t.is_empty())
        .collect();
    for tool in &tools {
        let path = bin.join(tool);
        std::fs::write(&path, "#!/bin/sh\n").expect("write stub");
        let mut perms = std::fs::metadata(&path).expect("stat stub").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod stub");
    }
    let fixture = bin.as_os_str().to_os_string();
    for entry in static_rows() {
        assert!(
            tool_present_in(entry.tool, Some(fixture.clone())),
            "{} tool {:?} did not resolve on the fixture PATH",
            entry.label,
            entry.tool,
        );
    }
    assert!(
        !tool_present_in("mcnf-definitely-absent-xyzzy", Some(fixture)),
        "an unstaged tool must stay absent (the honest gate's ground truth)"
    );
}

#[test]
fn the_containers_and_vms_plane_link_routes_to_the_cloud_plane() {
    // Q41/Q50 — the combined Containers & VMs group carries the surface
    // link that routes to the Cloud plane (a GUI plane), NOT a terminal tab.
    let flat = static_rows()
        .position(|e| e.kind == EntryKind::Plane(Plane::Cloud))
        .expect("the Cloud-plane surface link exists");
    let mut s = ConsoleState::with_store(None);
    s.toggle();
    s.activate(flat);
    assert_eq!(
        s.take_request(),
        Some(ConsoleRequest::Plane(Plane::Cloud)),
        "the Containers & VMs plane link routes to the Cloud plane"
    );
    assert!(!s.is_open(), "a routed surface link closes the panel");
}

#[test]
fn the_identity_line_reads_user_at_host() {
    // Named for `identity_line()` itself, not its on-screen position —
    // WIN7-5 relocated the rendered identity block from a bottom footer
    // to the rail's top (see the module doc + `IDENTITY_H`'s own doc),
    // so a "footer" name here would now describe a spot this text no
    // longer occupies. The format this test checks is position-agnostic.
    let line = identity_line();
    assert!(line.contains('@'), "identity must read user@host: {line}");
    assert!(!line.starts_with('@') && !line.ends_with('@'));
}

// ── open/close (locks #1/#4) ─────────────────────────────────────────────

#[test]
fn the_start_toggle_opens_and_a_second_toggle_closes() {
    // Pressing the Start cell again closes (lock #4) — the dock drains the
    // click into this same toggle either way.
    let mut s = ConsoleState::default();
    assert!(!s.is_open(), "closed by default");
    s.toggle();
    assert!(s.is_open(), "the Start toggle opens the panel");
    s.toggle();
    assert!(!s.is_open(), "pressing Start again closes it");
}

#[test]
fn esc_closes_the_panel() {
    // The panel-level Area/slide/click-away machinery moved to
    // `start_menu` (WIN7-2) — the "mounts no layer while closed" and
    // "click away dismisses" contracts are now tested there, over the
    // real embedding Area. This module keeps the content-level half of
    // Esc: `handle_keys` still calls `ConsoleState::close` on its own
    // `state.open`, which is what `start_menu` reads to propagate the
    // dismissal to the whole panel (see its module doc).
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = ConsoleState::default();
    s.toggle();
    drive(&ctx, &mut s, Vec::new(), SZ);
    assert!(s.is_open());
    drive(&ctx, &mut s, vec![key(egui::Key::Escape)], SZ);
    assert!(!s.is_open(), "Esc dismisses the Console (lock #4)");
}

// ── keyboard nav + activation (locks #40/#48, §7 honest gates) ──────────

#[test]
fn arrows_move_the_focus_ring_and_enter_routes_a_live_surface_link() {
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = ConsoleState::default();
    s.toggle();
    drive(&ctx, &mut s, Vec::new(), SZ);
    assert_eq!(s.focus, 0, "the ring opens on the pinned Terminal");

    drive(&ctx, &mut s, vec![key(egui::Key::ArrowDown)], SZ);
    assert_eq!(s.focus, 1, "ArrowDown advances the ring");
    drive(&ctx, &mut s, vec![key(egui::Key::ArrowUp)], SZ);
    assert_eq!(s.focus, 0, "ArrowUp retreats the ring");
    drive(&ctx, &mut s, vec![key(egui::Key::ArrowUp)], SZ);
    assert_eq!(s.focus, total_rows() - 1, "the ring wraps at the top");

    // Enter on the pinned Terminal (a LIVE link): routes + closes.
    let mut s2 = ConsoleState::default();
    s2.toggle();
    drive(&ctx, &mut s2, Vec::new(), SZ);
    drive(&ctx, &mut s2, vec![key(egui::Key::Enter)], SZ);
    assert_eq!(
        s2.take_request(),
        Some(ConsoleRequest::Goto(Surface::Terminal)),
        "the pinned Terminal routes to the Terminal surface"
    );
    assert!(!s2.is_open(), "a routed link closes the panel");
    assert_eq!(s2.take_request(), None, "the request drains exactly once");
}

#[test]
fn a_present_command_entry_opens_its_named_tab_and_a_missing_one_greys() {
    // CONSOLE-5 — the front door opens: Enter on a present command entry
    // records the SpawnTab request that opens its NAMED tab and closes the
    // panel; a still-missing tool stays honestly greyed (§7, ToolMissing)
    // and launches nothing. Presence is pinned so the verdict is
    // host-independent.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = ConsoleState::default();
    s.toggle();
    drive(&ctx, &mut s, Vec::new(), SZ);
    s.force_presence(1, true); // the pinned Monitor (btop), "installed"
    drive(&ctx, &mut s, vec![key(egui::Key::ArrowDown)], SZ);
    drive(&ctx, &mut s, vec![key(egui::Key::Enter)], SZ);
    assert_eq!(
        s.take_request(),
        Some(ConsoleRequest::SpawnTab {
            name: "Monitor".to_owned(),
            argv: launch_argv("btop"),
        }),
        "the front door opens the entry's named tab running its command",
    );
    assert!(s.gate.is_none(), "a launched entry raises no gate");
    assert!(!s.is_open(), "launching closes the panel and shows the tab");

    // A fresh state with the tool ABSENT: the row greys + names the missing
    // tool, and routes NOTHING (§7 — never a faked launch).
    let mut s = ConsoleState::default();
    s.toggle();
    drive(&ctx, &mut s, Vec::new(), SZ);
    s.force_presence(1, false);
    drive(&ctx, &mut s, vec![key(egui::Key::ArrowDown)], SZ);
    drive(&ctx, &mut s, vec![key(egui::Key::Enter)], SZ);
    assert_eq!(
        s.gate.clone().expect("a missing tool gates").reason,
        GateReason::ToolMissing("btop")
    );
    assert_eq!(s.take_request(), None, "a missing tool launches nothing");
    assert!(s.is_open(), "the panel stays up so the notice is read");
}

#[test]
fn a_root_op_launches_through_the_documented_sudo_argv_path() {
    // Lock #29 — a leading `sudo ` op runs its login shell UNDER sudo
    // (`sudo -- bash -lc …`, the sudo prompts in the tab's PTY); a plain op
    // is just its login shell; a `sudo` owning its own flag (the Root
    // Shell's `sudo -i`) is left verbatim, never fed to sudo as a program.
    let words = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        launch_argv("sudo dnf upgrade"),
        words(&["sudo", "--", "bash", "-lc", "dnf upgrade"]),
    );
    assert_eq!(
        launch_argv("sudo firewall-cmd --list-all"),
        words(&["sudo", "--", "bash", "-lc", "firewall-cmd --list-all"]),
    );
    assert_eq!(launch_argv("btop"), words(&["bash", "-lc", "btop"]));
    assert_eq!(launch_argv("sudo -i"), words(&["bash", "-lc", "sudo -i"]));
}

#[test]
fn clicking_an_entry_row_activates_it() {
    // The pointer path matches the keyboard path: a click on the pinned
    // Terminal's row routes to the Terminal surface (through the same
    // activate). Uses the stable per-row id (the dock's addressable idiom).
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = ConsoleState::default();
    s.toggle();
    drive(&ctx, &mut s, Vec::new(), SZ);
    drive(&ctx, &mut s, Vec::new(), SZ);
    let row = ctx
        .read_response(console_entry_id(0))
        .expect("the pinned Terminal row is registered")
        .rect;
    click(&ctx, &mut s, row.center(), SZ);
    assert_eq!(
        s.take_request(),
        Some(ConsoleRequest::Goto(Surface::Terminal)),
        "a row click routes like Enter"
    );
}

// ── the rail jump-index (lock #49) ───────────────────────────────────────
//
// The original single-group ("Shells") jump-scroll proof that used to live
// here was folded into WIN7-5's
// `clicking_any_jump_row_scrolls_that_same_groups_heading_up_the_list`
// below (── WIN7-5 section), which parametrizes over a representative
// spread of groups INCLUDING Shells — the exact same case, so the
// original added no coverage beyond what that generalized test already
// proves. A cross-unit polish pass removed the now-redundant duplicate
// rather than leave both.

// ── CONSOLE-4: the Power section (locks #28/#36 — real seams, typed-armed) ──

#[test]
fn power_lock_and_suspend_fire_at_once_through_the_real_seams() {
    // Lock → the shell curtain request (NOT a logind verb); Suspend → its
    // real PowerVerb. Both act on a single press and close the panel.
    let mut s = ConsoleState::with_store(None);
    s.toggle();
    s.power_press(PowerAction::Lock);
    assert_eq!(
        s.take_request(),
        Some(ConsoleRequest::Lock),
        "Lock drops the curtain, not a logind verb"
    );
    assert!(!s.is_open(), "a fired power action closes the panel");
    assert_eq!(s.take_request(), None, "the request drains exactly once");

    let mut s = ConsoleState::with_store(None);
    s.toggle();
    s.power_press(PowerAction::Suspend);
    assert_eq!(
        s.take_request(),
        Some(ConsoleRequest::Power(PowerVerb::Suspend)),
        "Suspend drives the real seat verb (no arming — reversible)"
    );
    assert!(!s.is_open());
}

#[test]
fn reboot_and_shut_down_demand_the_typed_echo_before_firing() {
    // Lock #36 — the host-down pair fires ONLY past the typed echo: a
    // blank / mistyped echo never arms, a disarmed confirm refuses (§7).
    let mut s = ConsoleState::with_store(None);
    s.toggle();
    s.power_press(PowerAction::Reboot);
    assert!(s.is_open(), "arming keeps the panel up");
    assert_eq!(s.take_request(), None, "entering arming fires NOTHING");
    assert!(!s.armed(), "an empty echo never arms");
    assert!(!s.confirm_armed(), "a disarmed confirm refuses to fire");
    s.arming.as_mut().expect("arming set").echo = "nope".to_owned();
    assert!(!s.armed(), "a mistyped echo never arms");
    s.arming.as_mut().expect("arming set").echo = "reboot".to_owned();
    assert!(s.armed(), "the exact verb name (any case) arms it");
    assert!(s.confirm_armed());
    assert_eq!(
        s.take_request(),
        Some(ConsoleRequest::Power(PowerVerb::Reboot)),
        "a confirmed Reboot records the real logind verb"
    );
    assert!(!s.is_open(), "firing closes the panel");
    assert!(s.arming.is_none(), "the stage cleared");

    // Shut Down maps to logind PowerOff behind its own echo ("Shut Down").
    let mut s = ConsoleState::with_store(None);
    s.toggle();
    s.power_press(PowerAction::ShutDown);
    s.arming.as_mut().expect("arming set").echo = "shut down".to_owned();
    assert!(s.confirm_armed());
    assert_eq!(
        s.take_request(),
        Some(ConsoleRequest::Power(PowerVerb::PowerOff)),
        "Shut Down maps to logind PowerOff"
    );

    // Cancel drops the stage without firing; a close drops it too, so a
    // reopened Console never resumes a stale half-typed confirm.
    let mut s = ConsoleState::with_store(None);
    s.toggle();
    s.power_press(PowerAction::ShutDown);
    s.cancel_arming();
    assert!(s.arming.is_none());
    assert_eq!(s.take_request(), None, "a cancelled arming fired nothing");
    s.power_press(PowerAction::Reboot);
    s.close();
    assert!(s.arming.is_none(), "closing drops the in-flight arming");
}

#[test]
fn the_rail_power_rows_dispatch_and_only_an_armed_confirm_fires() {
    // The pointer path: the rail's Lock row fires its request; the Reboot
    // row only ARMS; the Confirm row is inert until the echo matches.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = ConsoleState::with_store(None);
    s.toggle();
    drive(&ctx, &mut s, Vec::new(), SZ);
    drive(&ctx, &mut s, Vec::new(), SZ);
    let lock = ctx
        .read_response(console_power_id(PowerAction::Lock))
        .expect("the Lock power row is registered")
        .rect;
    click(&ctx, &mut s, lock.center(), SZ);
    assert_eq!(s.take_request(), Some(ConsoleRequest::Lock));

    let ctx2 = egui::Context::default();
    Style::install(&ctx2);
    let mut s2 = ConsoleState::with_store(None);
    s2.toggle();
    drive(&ctx2, &mut s2, Vec::new(), SZ);
    drive(&ctx2, &mut s2, Vec::new(), SZ);
    let reboot = ctx2
        .read_response(console_power_id(PowerAction::Reboot))
        .expect("the Reboot power row is registered")
        .rect;
    click(&ctx2, &mut s2, reboot.center(), SZ);
    assert!(s2.arming.is_some(), "the Reboot row enters arming");
    assert_eq!(s2.take_request(), None, "the row itself fires nothing");

    // The arming stage mounted in the same box; its DISARMED Confirm is inert.
    drive(&ctx2, &mut s2, Vec::new(), SZ);
    drive(&ctx2, &mut s2, Vec::new(), SZ);
    let confirm = ctx2
        .read_response(console_confirm_id())
        .expect("the Confirm row is registered")
        .rect;
    click(&ctx2, &mut s2, confirm.center(), SZ);
    assert_eq!(
        s2.take_request(),
        None,
        "a disarmed Confirm never fires (§7)"
    );
    assert!(s2.arming.is_some(), "still arming");

    // Arm the echo (the dock tests' direct-echo idiom) — the Confirm fires.
    s2.arming.as_mut().expect("arming set").echo = "Reboot".to_owned();
    drive(&ctx2, &mut s2, Vec::new(), SZ);
    click(&ctx2, &mut s2, confirm.center(), SZ);
    assert_eq!(
        s2.take_request(),
        Some(ConsoleRequest::Power(PowerVerb::Reboot)),
        "the armed Confirm fires the real verb"
    );
    assert!(!s2.is_open(), "firing closed the panel");
}

// ── CONSOLE-4: the Custom group (lock #35 — config round-trip + honest gate) ──

#[test]
fn custom_entries_round_trip_the_config_and_survive_a_reload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = dir.path().join("console-custom.json");
    let mut s = ConsoleState::with_store(Some(store.clone()));
    assert!(s.custom.entries.is_empty(), "a fresh store starts empty");

    // A blank draft is refused — nothing registered, nothing written.
    assert!(!s.add_custom(), "a blank draft is refused");
    assert!(!store.exists(), "a refused add persists nothing");

    s.draft_name = "Fleet status".to_owned();
    s.draft_command = "meshctl fleet status".to_owned();
    assert!(s.add_custom(), "a full draft registers");
    assert!(
        s.draft_name.is_empty() && s.draft_command.is_empty(),
        "a registered draft clears its fields"
    );
    assert!(
        store.exists(),
        "the add persisted the config (atomic write)"
    );

    // The round trip: a fresh state over the same store loads it back.
    let reloaded = ConsoleState::with_store(Some(store.clone()));
    assert_eq!(
        reloaded.custom.entries,
        vec![CustomEntry {
            name: "Fleet status".to_owned(),
            command: "meshctl fleet status".to_owned(),
        }]
    );

    // Removal persists too.
    let mut s2 = reloaded;
    s2.remove_custom(0);
    assert!(
        ConsoleState::with_store(Some(store.clone()))
            .custom
            .entries
            .is_empty(),
        "a removal persists"
    );

    // A malformed file folds honestly to the empty store (§7).
    std::fs::write(&store, "{not json").expect("write");
    assert!(
        ConsoleState::with_store(Some(store))
            .custom
            .entries
            .is_empty(),
        "a malformed config folds to empty, never a panic or a fake entry"
    );
}

#[test]
fn a_custom_entry_opens_its_own_named_tab() {
    // CONSOLE-5 — a Custom entry's launch rides the SAME spawn-tab seam,
    // opening its own named tab running the operator's command line and
    // closing the panel; the keyboard ring includes the custom tail.
    let dir = tempfile::tempdir().expect("tempdir");
    let store = dir.path().join("console-custom.json");
    let mut s = ConsoleState::with_store(Some(store));
    s.draft_name = "Farm top".to_owned();
    s.draft_command = "ssh mm@bigboy btop".to_owned();
    assert!(s.add_custom());
    s.toggle();
    assert_eq!(
        s.rows_total(),
        total_rows() + 1,
        "the activation ring includes the custom tail"
    );
    s.activate(total_rows());
    assert_eq!(
        s.take_request(),
        Some(ConsoleRequest::SpawnTab {
            name: "Farm top".to_owned(),
            argv: launch_argv("ssh mm@bigboy btop"),
        }),
        "a custom entry opens its own named tab running the operator's line",
    );
    assert!(s.gate.is_none(), "a launched custom entry raises no gate");
    assert!(!s.is_open(), "launching a custom entry closes the panel");
}

// ── WIN7-8: multi-seat sync (lock #21) — Custom entries sync mesh-wide ──

#[test]
fn win7_8_a_custom_entry_added_on_one_seat_syncs_to_another_over_the_shared_root() {
    let root = tempfile::tempdir().expect("tempdir");
    let local_a = tempfile::tempdir().expect("tempdir");
    let local_b = tempfile::tempdir().expect("tempdir");

    let sync_a = custom_sync::CustomSync::new(
        custom_sync::CustomSyncStore::new(root.path().to_path_buf()),
        "matthew",
        "seat-a",
    );
    let mut a = ConsoleState::with_store_and_sync(
        Some(local_a.path().join("console-custom.json")),
        Some(sync_a),
    );
    a.draft_name = "Fleet status".to_owned();
    a.draft_command = "meshctl fleet status".to_owned();
    assert!(a.add_custom());

    let sync_b = custom_sync::CustomSync::new(
        custom_sync::CustomSyncStore::new(root.path().to_path_buf()),
        "matthew",
        "seat-b",
    );
    let mut b = ConsoleState::with_store_and_sync(
        Some(local_b.path().join("console-custom.json")),
        Some(sync_b),
    );
    // Opening is what refolds the merged mesh view (the `refresh_presence`
    // "refreshed on each open" cadence restated) — the constructor above
    // already folds once too, so this also proves a SECOND refold
    // (on open) re-reads rather than just trusting the first.
    b.toggle();
    assert_eq!(
        b.custom.entries,
        vec![CustomEntry {
            name: "Fleet status".to_owned(),
            command: "meshctl fleet status".to_owned(),
        }],
        "seat A's add roamed to seat B"
    );
}

#[test]
fn win7_8_removing_a_custom_entry_converges_mesh_wide_even_for_another_seats_entry() {
    let root = tempfile::tempdir().expect("tempdir");
    let local_a = tempfile::tempdir().expect("tempdir");
    let local_b = tempfile::tempdir().expect("tempdir");

    let sync_a = custom_sync::CustomSync::new(
        custom_sync::CustomSyncStore::new(root.path().to_path_buf()),
        "matthew",
        "seat-a",
    );
    let mut a = ConsoleState::with_store_and_sync(
        Some(local_a.path().join("console-custom.json")),
        Some(sync_a),
    );
    a.draft_name = "Fleet status".to_owned();
    a.draft_command = "meshctl fleet status".to_owned();
    assert!(a.add_custom());

    let sync_b = custom_sync::CustomSync::new(
        custom_sync::CustomSyncStore::new(root.path().to_path_buf()),
        "matthew",
        "seat-b",
    );
    let mut b = ConsoleState::with_store_and_sync(
        Some(local_b.path().join("console-custom.json")),
        Some(sync_b),
    );
    b.toggle();
    assert_eq!(b.custom.entries.len(), 1, "seat B sees seat A's entry");

    // Seat B removes an entry it never itself added.
    b.remove_custom(0);
    assert!(b.custom.entries.is_empty(), "seat B's own view drops it");

    // Seat A folds the remove back on its next open.
    let sync_a2 = custom_sync::CustomSync::new(
        custom_sync::CustomSyncStore::new(root.path().to_path_buf()),
        "matthew",
        "seat-a",
    );
    let mut a2 = ConsoleState::with_store_and_sync(
        Some(local_a.path().join("console-custom.json")),
        Some(sync_a2),
    );
    a2.toggle();
    assert!(
        a2.custom.entries.is_empty(),
        "seat B's remove of seat A's entry roamed back to seat A"
    );
}

#[test]
fn win7_8_without_a_workgroup_root_custom_entries_stay_purely_local() {
    // `with_store` (the constructor every pre-existing test in this file
    // uses) leaves the sync session `None` — confirms the pre-WIN7-8
    // local-only contract is completely unchanged for every caller that
    // doesn't explicitly opt in via `with_store_and_sync`/`for_shell`.
    let dir = tempfile::tempdir().expect("tempdir");
    let store = dir.path().join("console-custom.json");
    let mut s = ConsoleState::with_store(Some(store));
    s.draft_name = "Fleet status".to_owned();
    s.draft_command = "meshctl fleet status".to_owned();
    assert!(s.add_custom());
    s.toggle(); // would refold from a sync session if one were wired in
    assert_eq!(s.custom.entries.len(), 1);
}

// ── WIN7-8: transient UI state is NEVER published (lock #21's other half) ──
//
// Design doc lock #21: "transient state (menu open/closed, scroll
// position, which group is expanded) stays local per seat." Investigated
// before writing these: this module has no persisted "scroll position"
// field at all (scroll offset is owned entirely by egui's own internal
// `ScrollArea` memory, never read or serialized by this crate) and no
// "which group is expanded" concept either (the rail's `jump` field is a
// one-shot scroll TRIGGER drained by the next render, not a persisted
// expand/collapse state — the list pane always shows every group, it is
// not an accordion) — both are already local-only purely by construction,
// with nothing in this crate that could even attempt to publish them.
// The tests below pin the closest honest, real proof: driving every
// piece of `ConsoleState`'s OWN transient state (open/close, jump,
// focus) against a REAL, ready sync session never creates a synced
// record, and a synced record that DOES exist (from a real Custom-entry
// add) never carries any of it.

#[test]
fn win7_8_opening_and_closing_the_panel_never_writes_to_the_synced_store() {
    let root = tempfile::tempdir().expect("tempdir");
    let sync = custom_sync::CustomSync::new(
        custom_sync::CustomSyncStore::new(root.path().to_path_buf()),
        "matthew",
        "seat-a",
    );
    let mut s = ConsoleState::with_store_and_sync(None, Some(sync));
    for _ in 0..5 {
        s.toggle(); // open
        s.toggle(); // close
    }
    let identity_dir = root
        .path()
        .join(custom_sync::CUSTOM_SYNC_SUBDIR)
        .join("matthew");
    assert!(
        !identity_dir.exists(),
        "open/close activity alone must never create a synced record"
    );
}

#[test]
fn win7_8_jump_scroll_and_focus_movement_never_write_to_the_synced_store() {
    let root = tempfile::tempdir().expect("tempdir");
    let sync = custom_sync::CustomSync::new(
        custom_sync::CustomSyncStore::new(root.path().to_path_buf()),
        "matthew",
        "seat-a",
    );
    let mut s = ConsoleState::with_store_and_sync(None, Some(sync));
    s.toggle();
    // The rail's jump-scroll target and the keyboard focus ring — the
    // closest things to a "scroll position" / "which group is active"
    // this module actually has.
    s.jump = Some(2);
    s.focus = 3;
    s.focus_moved = true;
    let identity_dir = root
        .path()
        .join(custom_sync::CUSTOM_SYNC_SUBDIR)
        .join("matthew");
    assert!(
        !identity_dir.exists(),
        "jump-scroll/focus activity alone must never create a synced record"
    );
}

#[test]
fn win7_8_the_synced_record_carries_only_custom_entry_data_never_transient_ui_state() {
    // A belt-and-suspenders proof over the RAW JSON on disk (not just the
    // typed shape, which already forbids this by construction): add a
    // real entry (so a file actually gets written), drive open/close and
    // jump/focus around it, then read the file back and confirm it names
    // only what CONSOLE-4's Custom entries actually are.
    let root = tempfile::tempdir().expect("tempdir");
    let sync = custom_sync::CustomSync::new(
        custom_sync::CustomSyncStore::new(root.path().to_path_buf()),
        "matthew",
        "seat-a",
    );
    let mut s = ConsoleState::with_store_and_sync(None, Some(sync));
    s.toggle();
    s.jump = Some(1);
    s.focus = 4;
    s.draft_name = "Fleet status".to_owned();
    s.draft_command = "meshctl fleet status".to_owned();
    assert!(s.add_custom());
    s.toggle();
    s.toggle();

    let path = root
        .path()
        .join(custom_sync::CUSTOM_SYNC_SUBDIR)
        .join("matthew")
        .join("seat-a.json");
    let raw = std::fs::read_to_string(&path).expect("the synced record was written");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
    assert_eq!(v["entries"][0]["name"], "Fleet status");
    assert_eq!(v["entries"][0]["command"], "meshctl fleet status");
    for field in [
        "open", "jump", "focus", "scroll", "expanded", "gate", "arming",
    ] {
        assert!(
            !raw.contains(field),
            "the synced record must never mention transient UI state \
             ({field}), got: {raw}"
        );
    }
}

// ── WIN7-5: the redesigned right pane (locks #10/#11/#14) ────────────────

#[test]
fn jump_caption_reads_singular_for_exactly_one_and_plural_otherwise() {
    assert_eq!(jump_caption(0), "0 entries");
    assert_eq!(jump_caption(1), "1 entry");
    assert_eq!(jump_caption(4), "4 entries");
}

#[test]
fn the_power_section_is_flush_with_the_panes_true_bottom_edge() {
    // Lock #11 — Power anchors the right pane's TRUE bottom, not
    // "wherever the jump-index above it happens to end" (the WIN7-2-era
    // straight-embed bug this unit fixed — see the module doc). The
    // test harness (`drive`, above) mounts `console_content`'s rect
    // flush with the screen bottom, so the pane's real bottom edge is
    // exactly `SZ.y`.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = ConsoleState::default();
    s.toggle();
    drive(&ctx, &mut s, Vec::new(), SZ);
    drive(&ctx, &mut s, Vec::new(), SZ);
    let shutdown = ctx
        .read_response(console_power_id(PowerAction::ShutDown))
        .expect("the Shut Down power row is registered")
        .rect;
    assert!(
        (shutdown.bottom() - SZ.y).abs() < 0.5,
        "the last Power row must sit flush with the pane's true bottom \
         edge: got {} vs {}",
        shutdown.bottom(),
        SZ.y,
    );
}

#[test]
fn the_gap_between_the_jump_index_and_power_is_small_not_the_old_dead_void() {
    // The WIN7-2-era straight embed left an unaccounted ~168pt dead gap
    // between the jump-index and the Power section (a big blank void
    // with nothing marking it as deliberate) — this pins the fix as a
    // real, regression-tested invariant instead of trusting it stays
    // fixed by eye. `RAIL_SECTION_GAP` (32pt) replaces it.
    let ctx = egui::Context::default();
    Style::install(&ctx);
    let mut s = ConsoleState::default();
    s.toggle();
    drive(&ctx, &mut s, Vec::new(), SZ);
    drive(&ctx, &mut s, Vec::new(), SZ);
    let last_jump_row = ctx
        .read_response(console_rail_id(CUSTOM_GROUP_LABEL))
        .expect("the Custom jump row is registered")
        .rect;
    let power_section_top = SZ.y - POWER_H;
    let gap = power_section_top - last_jump_row.bottom();
    assert!(
        gap >= 0.0,
        "the jump-index must not overlap the Power section: gap {gap}"
    );
    assert!(
        gap <= RAIL_SECTION_GAP + 0.5,
        "the gap between the jump-index and Power must be the one \
         deliberate RAIL_SECTION_GAP, not a large accidental void: got {gap}"
    );
}

#[test]
fn clicking_any_jump_row_scrolls_that_same_groups_heading_up_the_list() {
    // Supersedes the original Shells-only jump-scroll proof (folded in
    // here, see the section banner above): the WIN7-5 rewrite touches
    // every jump row's paint path, so this proves the click-to-
    // scroll-target mapping wasn't disturbed for OTHER groups too, not
    // just Shells. A representative spread (first, middle, last group)
    // rather than all seven, to keep the test focused.
    for label in ["Network", "Storage", "Shells"] {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = ConsoleState::default();
        s.toggle();
        drive(&ctx, &mut s, Vec::new(), SZ);
        drive(&ctx, &mut s, Vec::new(), SZ);

        let before = ctx
            .read_response(console_heading_id(label))
            .unwrap_or_else(|| panic!("{label} heading is registered"))
            .rect
            .top();
        let rail_row = ctx
            .read_response(console_rail_id(label))
            .unwrap_or_else(|| panic!("{label} rail cell is registered"))
            .rect;
        click(&ctx, &mut s, rail_row.center(), SZ);
        for _ in 0..6 {
            drive(&ctx, &mut s, Vec::new(), SZ);
        }
        let after = ctx
            .read_response(console_heading_id(label))
            .unwrap_or_else(|| panic!("{label} heading is still registered"))
            .rect
            .top();
        assert!(
            after < before - Style::SP_XL,
            "{label}: the jump must scroll ITS OWN group up the pane \
             (before {before}, after {after})"
        );
    }
}

#[test]
fn every_jump_row_reports_its_real_group_size_and_tracks_custom_live() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = ConsoleState::default();
    s.toggle();
    let out = drive(&ctx, &mut s, Vec::new(), SZ);
    let nodes = out
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("accesskit update")
        .nodes
        .clone();

    for group in &GROUPS {
        let node = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some(group.label))
            .unwrap_or_else(|| panic!("{} jump row exports no accesskit node", group.label));
        assert_eq!(node.role(), egui::accesskit::Role::Button);
        let expected = jump_caption(group.entries.len());
        assert_eq!(
            node.value(),
            Some(expected.as_str()),
            "{}'s jump row must report its real entry count",
            group.label
        );
    }

    // Custom starts empty on a fresh store...
    let custom = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some(CUSTOM_GROUP_LABEL))
        .expect("the Custom jump row exports an accesskit node");
    assert_eq!(custom.value(), Some("0 entries"));
}

#[test]
fn adding_a_custom_entry_updates_the_customs_jump_row_count_live() {
    // Not a fixed number baked in at open time — `state.custom`'s real,
    // live length, re-read every frame.
    let dir = tempfile::tempdir().expect("tempdir");
    let store = dir.path().join("console-custom.json");
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = ConsoleState::with_store(Some(store));
    s.draft_name = "Fleet status".to_owned();
    s.draft_command = "meshctl fleet status".to_owned();
    assert!(s.add_custom());
    s.toggle();
    let out = drive(&ctx, &mut s, Vec::new(), SZ);
    let nodes = out
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("accesskit update")
        .nodes
        .clone();
    let custom = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some(CUSTOM_GROUP_LABEL))
        .expect("the Custom jump row exports an accesskit node");
    assert_eq!(custom.value(), Some("1 entry"));
}

#[test]
fn every_row_this_unit_touched_exports_a_clickable_button_accesskit_node() {
    // Lock #14 — WIN7-2 shipped this whole module's embedding with only
    // panel-level accesskit landmarks (`start_menu.rs`'s
    // `install_accessibility`); individual rows were explicitly flagged
    // as not-yet-covered. Proves every raw-painted interactive row this
    // unit rewrote now exports a real Button node: the eight jump rows,
    // the pinned Terminal entry row, and the four Power action rows.
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = ConsoleState::default();
    s.toggle();
    let out = drive(&ctx, &mut s, Vec::new(), SZ);
    let nodes = out
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("accesskit update")
        .nodes
        .clone();

    let mut expect_labels: Vec<&str> = GROUPS.iter().map(|g| g.label).collect();
    expect_labels.push(CUSTOM_GROUP_LABEL);
    expect_labels.push("Terminal");
    expect_labels.push(PowerAction::Lock.label());
    expect_labels.push(PowerAction::Suspend.label());
    expect_labels.push(PowerAction::Reboot.label());
    expect_labels.push(PowerAction::ShutDown.label());

    for label in expect_labels {
        let node = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some(label))
            .unwrap_or_else(|| panic!("{label} exports no accesskit node"));
        assert_eq!(node.role(), egui::accesskit::Role::Button, "{label}'s role");
    }
}

#[test]
fn a_custom_rows_accesskit_carries_its_real_name_and_command() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = dir.path().join("console-custom.json");
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = ConsoleState::with_store(Some(store));
    s.draft_name = "Fleet status".to_owned();
    s.draft_command = "meshctl fleet status".to_owned();
    assert!(s.add_custom());
    s.toggle();
    let out = drive(&ctx, &mut s, Vec::new(), SZ);
    let nodes = out
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("accesskit update")
        .nodes
        .clone();

    let row = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Fleet status"))
        .expect("the custom row exports an accesskit node");
    assert_eq!(row.role(), egui::accesskit::Role::Button);
    assert_eq!(row.value(), Some("meshctl fleet status"));

    let remove = nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Remove Fleet status"))
        .expect("the remove cross exports an accesskit node");
    assert_eq!(remove.role(), egui::accesskit::Role::Button);
}

#[test]
fn the_gate_notice_exports_a_live_polite_region_only_while_showing() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = ConsoleState::default();
    s.toggle();
    let out0 = drive(&ctx, &mut s, Vec::new(), SZ);
    let nodes0 = out0
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("accesskit update")
        .nodes
        .clone();
    assert!(
        !nodes0
            .iter()
            .any(|(_, n)| n.label() == Some("Console notice")),
        "no gate has fired yet — no live region should export"
    );

    // Force the pinned Monitor (btop) ABSENT, activate it, and confirm
    // the resulting gate notice (§7) is now announced — it was
    // visual-only before this unit, so a screen-reader user pressing a
    // greyed row heard nothing explaining why.
    s.force_presence(1, false);
    drive(&ctx, &mut s, vec![key(egui::Key::ArrowDown)], SZ);
    let out1 = drive(&ctx, &mut s, vec![key(egui::Key::Enter)], SZ);
    assert!(s.gate.is_some(), "the gate should have fired");
    let nodes1 = out1
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("accesskit update")
        .nodes
        .clone();
    let notice = nodes1
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Console notice"))
        .expect("the gate notice must export a live region while showing");
    assert_eq!(notice.role(), egui::accesskit::Role::Status);
    assert_eq!(notice.live(), Some(egui::accesskit::Live::Polite));
    assert!(
        notice.value().unwrap_or_default().contains("Monitor"),
        "the announced text must name the gated entry: {:?}",
        notice.value()
    );
}

#[test]
fn the_confirm_and_cancel_rows_report_their_armed_state_via_accesskit_value() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    Style::install(&ctx);
    let mut s = ConsoleState::with_store(None);
    s.toggle();
    drive(&ctx, &mut s, Vec::new(), SZ);
    s.power_press(PowerAction::Reboot);
    let out0 = drive(&ctx, &mut s, Vec::new(), SZ);
    let nodes0 = out0
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("accesskit update")
        .nodes
        .clone();
    let confirm0 = nodes0
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Confirm Reboot"))
        .expect("the Confirm row exports an accesskit node while arming");
    assert_eq!(confirm0.role(), egui::accesskit::Role::Button);
    assert!(
        confirm0.value().unwrap_or_default().contains("Disabled"),
        "a disarmed Confirm's value must say so: {:?}",
        confirm0.value()
    );
    let cancel0 = nodes0
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Cancel"))
        .expect("the Cancel row exports an accesskit node");
    assert_eq!(cancel0.role(), egui::accesskit::Role::Button);

    // Arm the echo — the Confirm row's value flips to "ready."
    s.arming.as_mut().expect("arming set").echo = "Reboot".to_owned();
    let out1 = drive(&ctx, &mut s, Vec::new(), SZ);
    let nodes1 = out1
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("accesskit update")
        .nodes
        .clone();
    let confirm1 = nodes1
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.label() == Some("Confirm Reboot"))
        .expect("the Confirm row still exports a node once armed");
    assert!(
        confirm1.value().unwrap_or_default().contains("Ready"),
        "an armed Confirm's value must say so: {:?}",
        confirm1.value()
    );
}
