use super::*;
use crate::dialogs::NameOperation;
use mde_egui::search_omnibox::{ranked_hits, SearchDomain};
use mde_files::backend::{AuditEntry, ConflictPolicy, LocalFsBackend, SendMode};
use mde_files::fileops::{FakeFileOps, FileOps, LiveFileOps};
use mde_files::model::{PeerKind, PeerStatus};
use mde_files::opqueue::Resolution;
use mde_files::{ArchiveFormat, OpKind};
use std::collections::HashMap as Map;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// ── In-test backend double (from E12-11, unchanged shape) ────────────────

struct FixtureBackend {
    peers: Vec<Peer>,
    rows: Vec<FileRow>,
    local_rows: Map<String, Vec<FileRow>>,
    peer_rows: Map<String, Vec<FileRow>>,
    list_errors: Map<String, String>,
    next_op: OpId,
    mesh: Option<MeshOverlayBadge>,
}

impl FixtureBackend {
    fn new(peers: Vec<Peer>, rows: Vec<FileRow>) -> Self {
        Self {
            peers,
            rows,
            local_rows: Map::new(),
            peer_rows: Map::new(),
            list_errors: Map::new(),
            next_op: 1,
            mesh: None,
        }
    }
    fn with_local(mut self, path: &str, rows: Vec<FileRow>) -> Self {
        self.local_rows.insert(path.to_string(), rows);
        self
    }
    fn with_peer(mut self, id: &str, rows: Vec<FileRow>) -> Self {
        self.peer_rows.insert(id.to_string(), rows);
        self
    }

    fn with_list_error(mut self, path: &str, error: &str) -> Self {
        self.list_errors.insert(path.to_string(), error.to_string());
        self
    }
}

impl Backend for FixtureBackend {
    fn self_node(&self) -> SelfNode {
        SelfNode {
            host: "fixture.mesh".into(),
            ..SelfNode::default()
        }
    }
    fn peers(&self) -> Vec<Peer> {
        self.peers.clone()
    }
    fn list(&self, path: &str) -> Result<Vec<FileRow>, BackendError> {
        if let Some(error) = self.list_errors.get(path) {
            return Err(BackendError::Rejected(error.clone()));
        }
        if let Some(id) = path.strip_prefix("peer:") {
            return Ok(self.peer_rows.get(id).cloned().unwrap_or_default());
        }
        if let Some(rows) = self.local_rows.get(path) {
            return Ok(rows.clone());
        }
        Ok(self.rows.clone())
    }
    fn audit_log(&self) -> Vec<AuditEntry> {
        Vec::new()
    }
    fn send_to(
        &mut self,
        sources: &[PathBuf],
        _destination: Destination,
        _mode: SendMode,
        _conflict: ConflictPolicy,
    ) -> Result<OpId, BackendError> {
        if sources.is_empty() {
            return Err(BackendError::Rejected("empty source list".into()));
        }
        let id = self.next_op;
        self.next_op += 1;
        Ok(id)
    }
    fn rollback(&mut self, op_id: OpId) -> Result<OpId, BackendError> {
        Err(BackendError::NotFound(op_id))
    }
    fn mesh_overlay(&self) -> Option<MeshOverlayBadge> {
        self.mesh.clone()
    }
}

fn peer(id: &str, status: PeerStatus) -> Peer {
    Peer {
        id: id.into(),
        host: format!("{id}.mesh"),
        label: id.into(),
        kind: PeerKind::Desktop,
        addr: "10.0.0.9".into(),
        status,
        latency: None,
        files: 0,
        shared: 0,
        last: String::new(),
    }
}

/// A roster fixture: pine+birch Online, oak Idle, cedar Offline (→ 3
/// reachable), with virtual per-peer listings for pine and birch.
fn roster_backend() -> FixtureBackend {
    let peers = vec![
        peer("pine", PeerStatus::Online),
        peer("birch", PeerStatus::Online),
        peer("oak", PeerStatus::Idle),
        peer("cedar", PeerStatus::Offline),
    ];
    let pine = vec![
        FileRow::local("design-notes.md", Mime::Doc, "8 KB", "4 min"),
        FileRow::local(
            "screenshots/",
            Mime::Folder,
            "\u{2014} \u{b7} 122 items",
            "\u{2014}",
        ),
    ];
    FixtureBackend::new(peers, Vec::new()).with_peer("pine", pine)
}

fn browser_over(backend: FixtureBackend) -> FileBrowser {
    FileBrowser::with_file_ops(Box::new(backend), FakeFileOps::new())
}

// ── Location grammar + crumbs ────────────────────────────────────────────

#[test]
fn location_maps_to_the_backend_list_path() {
    assert_eq!(
        Location::Local("local:docs".into()).backend_path(),
        "local:docs"
    );
    assert_eq!(Location::Local("/etc".into()).backend_path(), "/etc");
    assert_eq!(Location::Peer("pine".into()).backend_path(), "peer:pine");
    assert!(Location::Local(String::new()).is_local());
    assert!(Location::Peer("pine".into()).is_peer());
}

#[test]
fn absolute_crumbs_decompose_and_navigate_to_ancestors() {
    let crumbs = Location::Local("/home/mac/docs".into()).crumbs();
    let labels: Vec<&str> = crumbs.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, vec!["/", "home", "mac", "docs"]);
    // The 3rd crumb navigates to /home/mac, not the leaf.
    assert_eq!(crumbs[2].location, Location::Local("/home/mac".into()));
}

#[test]
fn parent_only_for_absolute_local_paths() {
    assert_eq!(
        Location::Local("/a/b".into()).parent(),
        Some(Location::Local("/a".into()))
    );
    assert!(Location::Local("local:home".into()).parent().is_none());
    assert!(Location::Peer("pine".into()).parent().is_none());
}

// ── sort key parsers ─────────────────────────────────────────────────────

#[test]
fn size_parser_orders_by_magnitude() {
    assert!(parse_size_bytes("512 B") < parse_size_bytes("2 KB"));
    assert!(parse_size_bytes("2 KB") < parse_size_bytes("5.0 MB"));
    assert!(parse_size_bytes("5.0 MB") < parse_size_bytes("3.0 GB"));
    // A folder summary is not a byte size.
    assert_eq!(parse_size_bytes("\u{2014} \u{b7} 122 items"), 0);
}

#[test]
fn age_parser_orders_newest_first() {
    assert!(parse_age_secs("now") < parse_age_secs("4 min"));
    assert!(parse_age_secs("4 min") < parse_age_secs("2 h"));
    assert!(parse_age_secs("2 h") < parse_age_secs("3 d"));
    // Unknown age sorts last.
    assert_eq!(parse_age_secs("\u{2014}"), u64::MAX);
}

#[test]
fn sort_groups_dirs_first_then_by_key_and_flips() {
    let mut rows = vec![
        FileRow::local("zeta.txt", Mime::Doc, "1 KB", "1 h"),
        FileRow::local("alpha/", Mime::Folder, "\u{2014}", "\u{2014}"),
        FileRow::local("beta.txt", Mime::Doc, "2 KB", "2 h"),
    ];
    sort_rows(&mut rows, SortSpec::default());
    // dir first, then files A→Z.
    assert_eq!(rows[0].name, "alpha/");
    assert_eq!(rows[1].name, "beta.txt");
    assert_eq!(rows[2].name, "zeta.txt");
    // Descending name keeps the dir grouped first.
    let spec = SortSpec {
        key: SortKey::Name,
        dir: SortDir::Desc,
        dirs_first: true,
    };
    sort_rows(&mut rows, spec);
    assert_eq!(rows[0].name, "alpha/", "dir stays first regardless of dir");
    assert_eq!(rows[1].name, "zeta.txt");
}

// ── navigation + history ─────────────────────────────────────────────────

#[test]
fn navigate_records_history_and_back_forward_walk_it() {
    let mut b = browser_over(roster_backend());
    b.navigate(0, Location::Local("/a".into()));
    b.navigate(0, Location::Local("/a/b".into()));
    assert!(b.active_tab().can_back());
    assert!(!b.active_tab().can_forward());
    b.go_back(0);
    assert_eq!(*b.active_tab().location(), Location::Local("/a".into()));
    assert!(b.active_tab().can_forward());
    b.go_forward(0);
    assert_eq!(*b.active_tab().location(), Location::Local("/a/b".into()));
}

#[test]
fn go_up_walks_to_the_parent_directory() {
    let mut b = browser_over(roster_backend());
    b.navigate(0, Location::Local("/a/b/c".into()));
    b.go_up(0);
    assert_eq!(*b.active_tab().location(), Location::Local("/a/b".into()));
}

#[test]
fn open_path_edit_routes_peer_and_local() {
    let mut b = browser_over(roster_backend());
    b.set_path_edit(0, "peer:pine".into());
    b.open_path_edit(0);
    assert!(b.active_tab().location().is_peer());
    assert_eq!(b.active_tab().rows().len(), 2, "pine's listing loaded");
    b.set_path_edit(0, "/etc".into());
    b.open_path_edit(0);
    assert_eq!(*b.active_tab().location(), Location::Local("/etc".into()));
}

#[test]
fn open_row_descends_into_a_folder_via_its_path() {
    let rows = vec![
        FileRow::local("sub/", Mime::Folder, "\u{2014}", "\u{2014}").with_path("/data/sub"),
        FileRow::local("a.txt", Mime::Doc, "1 KB", "now").with_path("/data/a.txt"),
    ];
    let mut b = browser_over(FixtureBackend::new(Vec::new(), rows));
    b.open_row(0, 0); // the folder row (index 0 after dirs-first sort)
    assert_eq!(
        *b.active_tab().location(),
        Location::Local("/data/sub".into())
    );
}

// ── selection state machine ──────────────────────────────────────────────

fn five_row_browser() -> FileBrowser {
    let rows = (0..5)
        .map(|i| {
            FileRow::local(format!("f{i}.txt"), Mime::Doc, "1 KB", "now")
                .with_path(format!("/d/f{i}.txt"))
        })
        .collect();
    browser_over(FixtureBackend::new(Vec::new(), rows))
}

#[test]
fn click_selects_one_ctrl_toggles_shift_ranges() {
    let mut b = five_row_browser();
    b.click(0, 2);
    assert_eq!(b.active_tab().selection(), &BTreeSet::from([2]));
    // Ctrl-click adds another, and toggles it back off.
    b.ctrl_click(0, 4);
    assert_eq!(b.active_tab().selection(), &BTreeSet::from([2, 4]));
    b.ctrl_click(0, 4);
    assert_eq!(b.active_tab().selection(), &BTreeSet::from([2]));
    // A fresh click re-anchors; a shift-click then ranges from it (2 → 4).
    // (Ctrl-click moves the anchor to the ctrl-clicked row, the desktop
    // convention, so we re-click to set a known anchor first.)
    b.click(0, 2);
    b.shift_click(0, 4);
    assert_eq!(b.active_tab().selection(), &BTreeSet::from([2, 3, 4]));
    // Shift-clicking backwards ranges the other way from the same anchor.
    b.shift_click(0, 0);
    assert_eq!(b.active_tab().selection(), &BTreeSet::from([0, 1, 2]));
}

#[test]
fn select_all_and_clear_and_rubber_band() {
    let mut b = five_row_browser();
    b.select_all(0);
    assert_eq!(b.active_tab().selection().len(), 5);
    b.clear_selection(0);
    assert!(b.active_tab().selection().is_empty());
    // The rubber-band result (the view computes the covered set).
    b.set_rubber(0, BTreeSet::from([1, 2, 3]));
    assert_eq!(b.active_tab().selection(), &BTreeSet::from([1, 2, 3]));
}

#[test]
fn a_re_sort_drops_the_stale_selection() {
    let mut b = five_row_browser();
    b.select_all(0);
    assert_eq!(b.active_tab().selection().len(), 5);
    b.sort_by(0, SortKey::Name); // re-sort → selection invalidated
    assert!(b.active_tab().selection().is_empty());
}

#[test]
fn listing_failure_is_retained_instead_of_becoming_an_empty_directory() {
    let mut b = browser_over(
        FixtureBackend::new(Vec::new(), Vec::new())
            .with_list_error("/restricted", "permission denied"),
    );

    b.navigate(0, Location::Local("/restricted".into()));

    assert!(b.active_tab().rows().is_empty());
    assert_eq!(
        b.active_tab().list_error(),
        Some("rejected: permission denied")
    );
}

// ── per-folder view memory ───────────────────────────────────────────────

#[test]
fn view_and_sort_and_hidden_persist_per_folder() {
    let mut b = browser_over(roster_backend());
    b.navigate(0, Location::Local("/one".into()));
    b.set_view(0, ViewMode::Grid);
    b.toggle_hidden(0);
    b.sort_by(0, SortKey::Size);
    // Navigate away then back — the folder's presentation is restored.
    b.navigate(0, Location::Local("/two".into()));
    assert_eq!(b.active_tab().view(), ViewMode::default());
    assert!(!b.active_tab().show_hidden());
    b.navigate(0, Location::Local("/one".into()));
    assert_eq!(b.active_tab().view(), ViewMode::Grid);
    assert!(b.active_tab().show_hidden());
    assert_eq!(b.active_tab().sort().key, SortKey::Size);
}

#[test]
fn revisiting_a_folder_refreshes_preference_lru_recency() {
    let mut b = browser_over(roster_backend());
    b.navigate(0, Location::Local("/one".into()));
    b.set_view(0, ViewMode::Grid);
    b.navigate(0, Location::Local("/two".into()));
    b.set_view(0, ViewMode::Details);

    b.navigate(0, Location::Local("/one".into()));

    assert_eq!(
        b.folder_prefs_lru.back().map(String::as_str),
        Some("/one"),
        "a visit must make the folder most recently used"
    );
}

#[test]
fn show_hidden_filters_dotfiles() {
    let rows = vec![
        FileRow::local(".secret", Mime::Doc, "1 KB", "now").with_path("/d/.secret"),
        FileRow::local("visible.txt", Mime::Doc, "1 KB", "now").with_path("/d/visible.txt"),
    ];
    let mut b = browser_over(FixtureBackend::new(Vec::new(), rows));
    assert_eq!(b.active_tab().rows().len(), 1, "dotfile hidden by default");
    b.toggle_hidden(0);
    assert_eq!(b.active_tab().rows().len(), 2, "dotfile shown after toggle");
}

// ── tabs + dual pane ─────────────────────────────────────────────────────

#[test]
fn tabs_open_close_and_keep_one() {
    let mut b = browser_over(roster_backend());
    assert_eq!(b.pane(0).tabs().len(), 1);
    b.new_tab(0);
    assert_eq!(b.pane(0).tabs().len(), 2);
    assert_eq!(b.pane(0).active_tab_index(), 1);
    b.close_tab(0, 1);
    assert_eq!(b.pane(0).tabs().len(), 1);
    b.close_tab(0, 0); // refuses to close the last tab
    assert_eq!(b.pane(0).tabs().len(), 1);
}

#[test]
fn dual_pane_toggles_and_focuses_independently() {
    let mut b = browser_over(roster_backend());
    assert!(!b.is_dual());
    b.toggle_dual();
    assert!(b.is_dual());
    b.set_active_pane(1);
    b.navigate(1, Location::Local("/right".into()));
    assert_eq!(
        *b.pane(1).active_tab().location(),
        Location::Local("/right".into())
    );
    // The left pane is untouched.
    assert_eq!(
        *b.pane(0).active_tab().location(),
        Location::Local("local:home".into())
    );
    b.toggle_dual();
    assert!(!b.is_dual());
    assert_eq!(
        b.active_pane_index(),
        0,
        "hiding pane 2 refocuses the primary"
    );
}

// ── DnD transfer planning + queue submission ─────────────────────────────

#[test]
fn plan_transfer_is_move_by_default_and_copy_with_ctrl() {
    let src = vec![PathBuf::from("/a/x")];
    let dst = PathBuf::from("/b");
    assert!(matches!(
        plan_transfer(src.clone(), dst.clone(), false),
        OpKind::Move { .. }
    ));
    assert!(matches!(plan_transfer(src, dst, true), OpKind::Copy { .. }));
}

#[test]
fn drop_transfer_submits_a_queued_op_for_a_local_selection() {
    // A real fake FS with the source + dest so the queued op actually runs.
    let fs = FakeFileOps::new();
    fs.create_dir(Path::new("/d")).expect("mkdir");
    fs.create_dir(Path::new("/dst")).expect("mkdir");
    fs.seed_file("/d/f0.txt", b"x").expect("seed");
    let rows = vec![FileRow::local("f0.txt", Mime::Doc, "1 KB", "now").with_path("/d/f0.txt")];
    let mut b = FileBrowser::with_file_ops(Box::new(FixtureBackend::new(Vec::new(), rows)), fs);
    b.click(0, 0);
    let id = b
        .drop_transfer(0, PathBuf::from("/dst"), true)
        .expect("a local selection is transferable");
    assert!(b.ops().active().iter().any(|o| o.op_id == id));
    assert_eq!(
        b.operation_progress_summary()
            .expect("the queued copy has a shell summary")
            .label,
        "Copying f0.txt → dst"
    );
    assert!(b.last_note().is_none());
}

#[test]
fn drop_transfer_of_a_pathless_peer_selection_is_an_honest_no_op() {
    let mut b = browser_over(roster_backend());
    b.navigate(0, Location::Peer("pine".into()));
    b.click(0, 0); // a virtual peer row (no path)
    assert!(b.drop_transfer(0, PathBuf::from("/dst"), false).is_none());
    assert!(b.last_note().is_some(), "an honest note explains why");
}

// ── Send-To (mesh) still works over the new selection ────────────────────

#[test]
fn send_to_plans_from_the_selected_local_file() {
    let rows =
        vec![FileRow::local("notes.md", Mime::Doc, "1 KB", "now").with_path("/tmp/notes.md")];
    let mut b = browser_over(FixtureBackend::new(
        vec![
            peer("pine", PeerStatus::Online),
            peer("cedar", PeerStatus::Offline),
        ],
        rows,
    ));
    b.click(0, 0);
    assert!(!b.can_send(), "no destination yet");
    b.set_destination("cedar"); // offline → still blocked
    assert!(!b.can_send());
    b.set_destination("pine");
    let req = b.plan_send().expect("sendable to a reachable peer");
    assert_eq!(req.mode, SendMode::Copy);
    assert_eq!(req.destination, Destination::Peer("pine".into()));
    let result = b.send().expect("a planned send fires");
    assert!(result.is_ok());
    assert!(matches!(b.last_send(), SendOutcome::Sent { peer, .. } if peer == "pine"));
}

#[test]
fn reachable_destinations_excludes_offline_peers() {
    let b = browser_over(roster_backend());
    let reachable = b.reachable_destinations();
    assert_eq!(reachable.len(), 3);
    assert!(!reachable.iter().any(|p| p.id == "cedar"));
}

// ── mesh integration: Send-To + Send-in-Chat + clipboard (FILEMGR-12) ────

/// A `ChatBridge` recorder: captures every "Send in Chat" offer so the test
/// proves the transfer AND the chat hand-off both fired, keyed by the peer host.
struct RecordingChat {
    log: std::sync::Arc<std::sync::Mutex<Vec<(String, PathBuf)>>>,
}

impl crate::chat_bridge::ChatBridge for RecordingChat {
    fn offer_file(&self, to: &str, path: &Path) {
        self.log
            .lock()
            .unwrap()
            .push((to.to_string(), path.to_path_buf()));
    }
}

#[test]
fn context_menu_send_to_peer_dispatches_the_selection() {
    let rows =
        vec![FileRow::local("notes.md", Mime::Doc, "1 KB", "now").with_path("/tmp/notes.md")];
    let mut b = browser_over(FixtureBackend::new(
        vec![
            peer("pine", PeerStatus::Online),
            peer("cedar", PeerStatus::Offline),
        ],
        rows,
    ));
    b.click(0, 0);
    let result = b.send_to_peer(0, "pine").expect("a selected file sends");
    assert!(result.is_ok());
    assert!(matches!(b.last_send(), SendOutcome::Sent { peer, .. } if peer == "pine"));
}

#[test]
fn context_menu_send_to_peer_is_an_honest_no_op_with_no_selection() {
    let mut b = browser_over(FixtureBackend::new(
        vec![peer("pine", PeerStatus::Online)],
        Vec::new(),
    ));
    assert!(b.send_to_peer(0, "pine").is_none());
    assert!(b.last_note().is_some(), "an honest note explains why");
}

#[test]
fn send_in_chat_transfers_and_offers_the_file_kind_keyed_by_host() {
    let rows =
        vec![FileRow::local("notes.md", Mime::Doc, "1 KB", "now").with_path("/tmp/notes.md")];
    let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut b = browser_over(FixtureBackend::new(
        vec![peer("pine", PeerStatus::Online)],
        rows,
    ))
    .with_chat_bridge(Box::new(RecordingChat { log: log.clone() }));
    b.click(0, 0);
    let result = b
        .send_in_chat(0, "pine")
        .expect("a selected file sends in chat");
    assert!(result.is_ok(), "the real transfer fired");
    // The chat offer was handed off, keyed by the peer HOST (the contact
    // username = hostname), carrying the exact file path.
    let recorded = log.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, "pine.mesh");
    assert_eq!(recorded[0].1, PathBuf::from("/tmp/notes.md"));
    assert!(matches!(b.last_send(), SendOutcome::Sent { peer, .. } if peer == "pine.mesh"));
}

#[test]
fn send_in_chat_posts_no_offer_when_nothing_is_selected() {
    let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut b = browser_over(FixtureBackend::new(
        vec![peer("pine", PeerStatus::Online)],
        Vec::new(),
    ))
    .with_chat_bridge(Box::new(RecordingChat { log: log.clone() }));
    assert!(b.send_in_chat(0, "pine").is_none());
    assert!(log.lock().unwrap().is_empty(), "no transfer ⇒ no chat card");
}

#[test]
fn clip_copy_stages_the_paths_and_yields_shell_clipboard_text() {
    let rows = vec![
        FileRow::local("a.txt", Mime::Doc, "1 KB", "now").with_path("/src/a.txt"),
        FileRow::local("b.txt", Mime::Doc, "1 KB", "now").with_path("/src/b.txt"),
    ];
    let mut b = browser_over(FixtureBackend::new(Vec::new(), rows));
    assert!(!b.can_paste());
    b.select_all(0);
    let text = b.clip_copy(0).expect("a selection stages");
    assert_eq!(text, "/src/a.txt\n/src/b.txt", "one absolute path per line");
    assert!(b.can_paste());
}

#[test]
fn clip_copy_then_paste_queues_a_copy_and_keeps_the_clipboard() {
    let fs = FakeFileOps::new();
    fs.create_dir(Path::new("/src")).expect("mkdir");
    fs.create_dir(Path::new("/dst")).expect("mkdir");
    fs.seed_file("/src/a.txt", b"x").expect("seed");
    let rows = vec![FileRow::local("a.txt", Mime::Doc, "1 KB", "now").with_path("/src/a.txt")];
    let mut b = FileBrowser::with_file_ops(Box::new(FixtureBackend::new(Vec::new(), rows)), fs);
    b.click(0, 0);
    b.clip_copy(0).expect("staged");
    b.navigate(0, Location::Local("/dst".into()));
    let id = b.clip_paste(0).expect("an in-app paste submits a transfer");
    assert!(b.ops().active().iter().any(|o| o.op_id == id));
    assert!(
        b.can_paste(),
        "a Copy leaves the clipboard for repeat pastes"
    );
}

#[test]
fn clip_cut_then_paste_queues_a_move_and_clears_the_clipboard() {
    let fs = FakeFileOps::new();
    fs.create_dir(Path::new("/src")).expect("mkdir");
    fs.create_dir(Path::new("/dst")).expect("mkdir");
    fs.seed_file("/src/a.txt", b"x").expect("seed");
    let rows = vec![FileRow::local("a.txt", Mime::Doc, "1 KB", "now").with_path("/src/a.txt")];
    let mut b = FileBrowser::with_file_ops(Box::new(FixtureBackend::new(Vec::new(), rows)), fs);
    b.click(0, 0);
    b.clip_cut(0).expect("staged");
    b.navigate(0, Location::Local("/dst".into()));
    let id = b.clip_paste(0).expect("a cut paste submits a transfer");
    assert!(b.ops().active().iter().any(|o| o.op_id == id));
    assert!(!b.can_paste(), "a Cut is consumed by its paste");
}

#[test]
fn clip_copy_of_a_pathless_selection_is_an_honest_no_op() {
    let mut b = browser_over(roster_backend());
    b.navigate(0, Location::Peer("pine".into()));
    b.click(0, 0); // a virtual peer row (no path)
    assert!(b.clip_copy(0).is_none());
    assert!(b.last_note().is_some());
    assert!(!b.can_paste());
}

#[test]
fn clip_paste_text_pastes_shell_clipboard_paths_cross_surface() {
    // A real temp file so the cross-surface parse (which keeps only existing
    // absolute paths) accepts it — the path could have been copied in ANY
    // surface, so there's no in-app clipboard entry backing it.
    let dir = std::env::temp_dir().join(format!("mde-fm12-x-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let src = dir.join("shared.txt");
    std::fs::write(&src, b"hi").expect("write");
    let mut b = FileBrowser::with_file_ops(
        Box::new(FixtureBackend::new(Vec::new(), Vec::new())),
        FakeFileOps::new(),
    );
    b.navigate(0, Location::Local("/dst".into()));
    // Text with a real path + a bogus line: only the real path transfers.
    let pasted = format!("{}\nnot a path — just prose", src.display());
    let id = b
        .clip_paste_text(0, &pasted)
        .expect("a shell-clipboard paste of a real path submits a transfer");
    assert!(b.ops().active().iter().any(|o| o.op_id == id));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn clip_paste_text_of_no_real_paths_is_a_no_op() {
    let mut b = browser_over(FixtureBackend::new(Vec::new(), Vec::new()));
    b.navigate(0, Location::Local("/dst".into()));
    // A URL from another surface is not a file path — nothing transfers.
    assert!(b.clip_paste_text(0, "https://example.com/x").is_none());
}

#[test]
fn parse_clip_paths_keeps_only_existing_absolute_paths() {
    let dir = std::env::temp_dir().join(format!("mde-fm12-p-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let f = dir.join("real.txt");
    std::fs::write(&f, b"x").expect("write");
    let text = format!(
        "{}\n/definitely/not/here/ghost.txt\nrelative.txt\n   \n",
        f.display()
    );
    let got = parse_clip_paths(&text);
    assert_eq!(got, vec![f.clone()], "only the real absolute path survives");
    assert_eq!(join_clip_paths(&got), f.display().to_string());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn open_local_directory_over_the_real_backend_carries_paths() {
    // A real temp dir through the shipped LocalFsBackend: rows carry paths.
    let dir = std::env::temp_dir().join(format!("mde-files-fm8-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("hello.txt"), b"hi").expect("write");
    let mut b = FileBrowser::new(Box::new(LocalFsBackend::new()));
    b.navigate(0, Location::Local(dir.to_string_lossy().into_owned()));
    let row = b
        .active_tab()
        .rows()
        .iter()
        .find(|r| r.name == "hello.txt")
        .expect("temp file listed");
    assert!(row.path.is_some());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn files_search_omnibox_items_project_current_folder_rows_into_shared_ranker() {
    let rows = vec![
        FileRow::local("report.txt", Mime::Doc, "1 KB", "now").with_path("/d/report.txt"),
        FileRow::local("photo.png", Mime::Image, "80 KB", "2 h")
            .with_path("/d/photo.png")
            .with_mesh("pine.mesh"),
        FileRow::local("alpha/", Mime::Folder, "-", "-").with_path("/d/alpha"),
    ];
    let mut b = browser_over(FixtureBackend::new(Vec::new(), rows));

    let items = b.search_omnibox_items(0);
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|item| item.domain == SearchDomain::File));

    let report = items
        .iter()
        .find(|item| item.title == "report.txt")
        .expect("report candidate");
    assert_eq!(report.target, "/d/report.txt");
    assert_eq!(report.payload.path, Some(PathBuf::from("/d/report.txt")));
    assert!(report.terms.iter().any(|term| term == "document"));

    let mesh_hit = ranked_hits("pine", items.clone(), 8)
        .into_iter()
        .next()
        .expect("mesh attribution ranks as an auxiliary field");
    assert_eq!(mesh_hit.item.title, "photo.png");

    let folder_target = items
        .into_iter()
        .find(|item| item.title == "alpha/")
        .expect("folder candidate")
        .payload;
    b.open_search_omnibox_target(&folder_target);
    assert_eq!(
        b.active_tab().location(),
        &Location::Local("/d/alpha".to_string())
    );
}

#[test]
fn home_search_includes_bounded_home_rows_with_metadata() {
    let current_rows = vec![FileRow::local("work-report.txt", Mime::Doc, "4 KB", "now")
        .with_path("/work/work-report.txt")];
    let mut home_rows = vec![
        FileRow::local("home-notes.md", Mime::Doc, "2 KB", "3 h")
            .with_path("/home/me/home-notes.md"),
        FileRow::local("photos/", Mime::Folder, "12 items", "1 d").with_path("/home/me/photos"),
    ];
    for ix in 0..40 {
        home_rows.push(
            FileRow::local(format!("older-{ix}.txt"), Mime::Doc, "1 KB", "1 w")
                .with_path(format!("/home/me/older-{ix}.txt")),
        );
    }
    let mut b = browser_over(
        FixtureBackend::new(Vec::new(), Vec::new())
            .with_local("local:home", home_rows)
            .with_local("/work", current_rows),
    );
    b.navigate(0, Location::Local("/work".into()));

    let home_items = b.home_search_omnibox_items();
    assert_eq!(home_items.len(), 32, "home snapshot is bounded");

    let items = b.unified_search_omnibox_items();
    assert!(items.iter().any(|item| item.title == "work-report.txt"));
    let home = items
        .iter()
        .find(|item| item.title == "home-notes.md")
        .expect("home file candidate");
    assert_eq!(home.target, "/home/me/home-notes.md");
    assert!(home.terms.iter().any(|term| term == "document"));
    assert!(home.terms.iter().any(|term| term == "2 KB"));
    assert!(home.terms.iter().any(|term| term == "3 h"));

    let hit = ranked_hits("home-notes", items, 8)
        .into_iter()
        .next()
        .expect("home filename ranks through shared search");
    assert_eq!(hit.item.title, "home-notes.md");
}

#[test]
fn home_search_result_opens_through_the_files_model() {
    let home_row = FileRow::local("home-notes.md", Mime::Doc, "2 KB", "3 h")
        .with_path("/home/me/home-notes.md");
    let mut b = browser_over(
        FixtureBackend::new(Vec::new(), Vec::new())
            .with_local(
                "local:home",
                vec![
                    home_row.clone(),
                    FileRow::local("photos/", Mime::Folder, "12 items", "1 d")
                        .with_path("/home/me/photos"),
                ],
            )
            .with_local("/home/me", vec![home_row.clone()])
            .with_local(
                "/work",
                vec![FileRow::local("work-report.txt", Mime::Doc, "4 KB", "now")
                    .with_path("/work/work-report.txt")],
            ),
    );
    b.navigate(0, Location::Local("/work".into()));
    let target = b
        .unified_search_omnibox_items()
        .into_iter()
        .find(|item| item.title == "home-notes.md")
        .expect("home candidate")
        .payload;

    b.open_search_omnibox_target(&target);

    assert_eq!(
        b.active_tab().location(),
        &Location::Local("/home/me".to_string())
    );
    assert_eq!(
        b.active_tab().selected_paths(),
        vec![PathBuf::from("/home/me/home-notes.md")]
    );
}

// ── recursive search (FILEMGR-4) ─────────────────────────────────────────

#[test]
fn search_streams_an_operable_results_tab_over_the_real_fs() {
    // A real temp tree, searched recursively: hits stream into the active tab
    // as ordinary rows with real paths, so selection + ops apply to a result.
    let dir = std::env::temp_dir().join(format!(
        "mde-files-fm4-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(dir.join("nested")).expect("mkdir");
    std::fs::write(dir.join("alpha.log"), b"needle here").expect("write");
    std::fs::write(dir.join("beta.txt"), b"nothing").expect("write");
    std::fs::write(dir.join("nested/gamma.log"), b"deeper needle").expect("write");

    let mut b = FileBrowser::new(Box::new(LocalFsBackend::new()));
    b.navigate(0, Location::Local(dir.to_string_lossy().into_owned()));

    b.set_search_form(SearchForm {
        name_glob: "*.log".to_string(),
        ..Default::default()
    });
    b.start_search(0);
    assert!(b.search_active(), "a search is now running");

    // Drain the stream to completion (bounded so a bug can't hang the suite).
    for _ in 0..2000 {
        b.pump_search();
        if !b.search_running() {
            break;
        }
        // logic-timing, not motion (bounded test drain cadence)
        std::thread::sleep(Duration::from_millis(1));
    }
    b.pump_search();
    assert!(!b.search_running(), "search must finish");

    let mut names: Vec<String> = b
        .active_tab()
        .rows()
        .iter()
        .map(|r| r.name.clone())
        .collect();
    names.sort();
    assert_eq!(names, vec!["alpha.log", "gamma.log"], "recursive name hits");

    let search_items = b.search_omnibox_items(0);
    assert_eq!(search_items.len(), 2);
    assert!(search_items
        .iter()
        .all(|item| item.domain == SearchDomain::File));
    let gamma = ranked_hits("gamma", search_items.clone(), 8)
        .into_iter()
        .next()
        .expect("recursive hit candidate");
    assert_eq!(gamma.item.title, "gamma.log");
    assert!(ranked_hits("beta", search_items, 8).is_empty());

    // Results are a normal file view: every hit carries a real path, so the op
    // surface (selected_paths → copy/move/delete/Send-To) applies directly.
    b.select_all(0);
    let paths = b.active_tab().selected_paths();
    assert_eq!(paths.len(), 2, "both hits are operable");
    assert!(paths.iter().all(|p| p.exists()), "paths are live on disk");

    // Leaving search restores the folder's own listing.
    b.clear_search(0);
    assert!(!b.search_active());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn search_form_to_query_guards_the_empty_case() {
    let empty = SearchForm::default();
    assert!(empty.to_query().is_none(), "nothing typed ⇒ no query");

    let named = SearchForm {
        name_glob: "*.rs".to_string(),
        ..Default::default()
    };
    assert!(named.to_query().is_some());

    // A lone filter (folders only) is enough to be meaningful.
    let filtered = SearchForm {
        kind: TypeFilter::DirsOnly,
        ..Default::default()
    };
    assert!(filtered.to_query().is_some());
}

#[test]
fn start_search_on_a_pathless_view_is_an_honest_no_op() {
    // A virtual peer folder has no real directory, so a search there can't
    // root — it must note the reason, not spin.
    let mut b = browser_over(roster_backend());
    b.navigate(0, Location::Peer("pine".into()));
    b.set_search_form(SearchForm {
        name_glob: "*.rs".to_string(),
        ..Default::default()
    });
    b.start_search(0);
    assert!(!b.search_active(), "no root ⇒ no search");
    assert!(b.last_note().is_some(), "the reason is surfaced");
}

// ── mesh-mount sidebar tree (FILEMGR-9) ──────────────────────────────────

use crate::mesh_mount::test_support::FakeMeshMount;
use crate::mesh_mount::{MeshMountVerb, MountPhase, MountScope, MountView};

fn mounted_view(path: &str, scope: MountScope) -> MountView {
    MountView {
        phase: MountPhase::Mounted,
        scope: Some(scope),
        path: Some(path.to_string()),
        reason: None,
    }
}

#[test]
fn peer_mount_view_projects_from_the_client() {
    let fake = FakeMeshMount::new().with_view(
        "pine",
        mounted_view("/run/user/1000/mde-mesh/pine", MountScope::Home),
    );
    let b = browser_over(roster_backend()).with_mesh_mount(Box::new(fake));
    let view = b.mount_view("pine").expect("pine's state is projected");
    assert_eq!(view.phase, MountPhase::Mounted);
    assert_eq!(view.mountpoint(), Some("/run/user/1000/mde-mesh/pine"));
    // And it's reachable through the roster peer, keyed by the short mount host.
    let pine = b
        .peers()
        .iter()
        .find(|p| p.label == "pine")
        .expect("pine is in the roster");
    assert!(b.peer_mount(pine).is_some());
}

#[test]
fn navigating_a_reachable_peer_requests_a_mount_and_browses() {
    let fake = FakeMeshMount::new();
    let probe = fake.clone();
    let mut b = browser_over(roster_backend()).with_mesh_mount(Box::new(fake));
    b.open_peer(0, "pine"); // pine is Online → reachable
    assert_eq!(probe.verbs_for("pine"), vec![MeshMountVerb::Mount]);
    // Not mounted yet → browse the peer's virtual listing while it comes up.
    assert_eq!(*b.active_tab().location(), Location::Peer("pine".into()));
}

#[test]
fn navigating_a_mounted_peer_browses_the_live_path() {
    let fake = FakeMeshMount::new().with_view(
        "pine",
        mounted_view("/run/user/1000/mde-mesh/pine", MountScope::Home),
    );
    let probe = fake.clone();
    let mut b = browser_over(roster_backend()).with_mesh_mount(Box::new(fake));
    b.open_peer(0, "pine");
    // Browses the live sshfs mountpoint (a local path), not the virtual peer.
    assert_eq!(
        *b.active_tab().location(),
        Location::Local("/run/user/1000/mde-mesh/pine".into())
    );
    // Still re-requests a mount to keep the idle clock warm.
    assert_eq!(probe.verbs_for("pine"), vec![MeshMountVerb::Mount]);
}

#[test]
fn navigating_an_offline_peer_is_an_honest_no_op() {
    let fake = FakeMeshMount::new();
    let probe = fake.clone();
    let mut b = browser_over(roster_backend()).with_mesh_mount(Box::new(fake));
    let before = b.active_tab().location().clone();
    b.open_peer(0, "cedar"); // cedar is Offline
    assert_eq!(
        probe.request_count(),
        0,
        "no mount request is issued for an offline peer"
    );
    assert_eq!(*b.active_tab().location(), before, "location is unchanged");
    assert_eq!(
        b.last_note(),
        Some("cedar is offline - can't mount it."),
        "an honest ASCII note explains why"
    );
}

#[test]
fn escalate_requests_the_escalate_verb_for_a_reachable_peer() {
    let fake = FakeMeshMount::new();
    let probe = fake.clone();
    let mut b = browser_over(roster_backend()).with_mesh_mount(Box::new(fake));
    b.escalate_peer("pine");
    assert_eq!(probe.verbs_for("pine"), vec![MeshMountVerb::Escalate]);
    assert_eq!(
        b.last_note(),
        Some("Escalating pine to full-filesystem access...")
    );
}

#[test]
fn escalate_is_a_no_op_for_an_offline_peer() {
    let fake = FakeMeshMount::new();
    let probe = fake.clone();
    let mut b = browser_over(roster_backend()).with_mesh_mount(Box::new(fake));
    b.escalate_peer("cedar"); // Offline
    assert_eq!(probe.request_count(), 0);
    assert_eq!(b.last_note(), Some("cedar is offline - can't escalate it."));
}

#[test]
fn unmount_requests_the_unmount_verb() {
    let fake = FakeMeshMount::new();
    let probe = fake.clone();
    let mut b = browser_over(roster_backend()).with_mesh_mount(Box::new(fake));
    b.unmount_peer("pine");
    assert_eq!(probe.verbs_for("pine"), vec![MeshMountVerb::Unmount]);
    assert_eq!(b.last_note(), Some("Unmounting pine..."));
}

#[test]
fn transitional_mounts_flag_a_repaint_heartbeat() {
    let mounting = MountView {
        phase: MountPhase::Mounting,
        scope: Some(MountScope::Home),
        path: None,
        reason: None,
    };
    let fake = FakeMeshMount::new().with_view("pine", mounting);
    let b = browser_over(roster_backend()).with_mesh_mount(Box::new(fake));
    assert!(b.any_mount_transitional());
}

// ── previews + quick-look (FILEMGR-10) ───────────────────────────────────

/// A browser over two local files with real paths (a text file + an image).
fn preview_browser() -> FileBrowser {
    let rows = vec![
        FileRow::local("notes.md", Mime::Doc, "1 KB", "now").with_path("/d/notes.md"),
        FileRow::local("photo.png", Mime::Image, "80 KB", "2 h").with_path("/d/photo.png"),
    ];
    browser_over(FixtureBackend::new(Vec::new(), rows))
}

#[test]
fn preview_target_follows_the_selection_anchor() {
    let mut b = preview_browser();
    assert!(b.preview_target().is_none(), "nothing selected → no target");
    b.click(0, 0);
    assert_eq!(b.preview_target().expect("target").name, "notes.md");
    // Ctrl-click adds row 1 and moves the anchor there.
    b.ctrl_click(0, 1);
    assert_eq!(b.preview_target().expect("target").name, "photo.png");
    // Ctrl-click the anchor off again → falls back to the last selected.
    b.ctrl_click(0, 1);
    assert_eq!(b.preview_target().expect("target").name, "notes.md");
    b.clear_selection(0);
    assert!(b.preview_target().is_none());
}

#[test]
fn quick_look_only_opens_with_a_target_and_closes_cleanly() {
    let mut b = preview_browser();
    b.toggle_quick_look();
    assert!(!b.quick_look_open(), "no selection → nothing to look at");
    b.click(0, 1);
    b.toggle_quick_look();
    assert!(b.quick_look_open());
    b.toggle_quick_look();
    assert!(!b.quick_look_open(), "Space toggles closed");
    b.toggle_quick_look();
    b.close_quick_look();
    assert!(!b.quick_look_open(), "Escape closes");
}

#[test]
fn double_clicking_a_file_opens_the_built_in_quick_look() {
    // Lock 23: activating a file opens the built-in viewer — never an
    // external program spawn.
    let mut b = preview_browser();
    b.open_row(0, 1);
    assert!(b.quick_look_open());
    assert_eq!(b.preview_target().expect("target").name, "photo.png");
}

#[test]
fn preview_toggles_start_at_the_locked_defaults() {
    let mut b = preview_browser();
    assert!(b.preview_pane_open(), "the pane ships on (lock 22)");
    assert!(b.list_thumbs(), "the List thumbnail column ships on");
    b.toggle_preview_pane();
    assert!(!b.preview_pane_open());
    b.toggle_list_thumbs();
    assert!(!b.list_thumbs());
}

#[test]
fn refresh_busts_the_preview_caches() {
    let mut b = preview_browser();
    // A request against a path that can't decode still occupies a slot…
    b.request_thumb("/d/photo.png");
    assert!(b.thumb_state("/d/photo.png").is_some());
    // …until the lock-18 cache bust clears it.
    b.clear_previews();
    assert!(b.thumb_state("/d/photo.png").is_none());
}

#[test]
fn remote_paths_are_detected_by_mount_root_and_published_mountpoints() {
    let fake =
        FakeMeshMount::new().with_view("pine", mounted_view("/mnt/pine-x", MountScope::Home));
    let b = browser_over(roster_backend()).with_mesh_mount(Box::new(fake));
    // The stable lock-11 root is always remote, even before state arrives.
    assert!(b.is_remote_path("/run/user/1000/mde-mesh/pine/docs/a.png"));
    // A worker-published mountpoint is remote…
    assert!(b.is_remote_path("/mnt/pine-x/file.txt"));
    assert!(b.is_remote_path("/mnt/pine-x"));
    // …but a sibling that merely shares the prefix is not.
    assert!(!b.is_remote_path("/mnt/pine-xylophone/file.txt"));
    assert!(!b.is_remote_path("/home/mac/file.txt"));
}

// ── the operation dialogs (FILEMGR-11) ───────────────────────────────────

/// A browser over a real fake FS with a `/dst` and a source row, so a queued
/// transfer really runs (and a collision really surfaces).
fn transfer_browser(collide: bool) -> (FileBrowser, PathBuf) {
    let fs = FakeFileOps::new();
    fs.create_dir(Path::new("/d")).expect("mkdir");
    fs.create_dir(Path::new("/dst")).expect("mkdir");
    fs.seed_file("/d/f0.txt", b"payload").expect("seed");
    if collide {
        fs.seed_file("/dst/f0.txt", b"older")
            .expect("seed collision");
    }
    let rows = vec![FileRow::local("f0.txt", Mime::Doc, "1 KB", "now").with_path("/d/f0.txt")];
    let b = FileBrowser::with_file_ops(Box::new(FixtureBackend::new(Vec::new(), rows)), fs);
    (b, PathBuf::from("/dst"))
}

#[test]
fn a_local_delete_confirm_arms_without_typing_and_submits_on_confirm() {
    let mut b = five_row_browser();
    b.select_all(0);
    b.request_delete(0);
    let confirm = b.pending_delete().expect("a confirm opened");
    assert_eq!(confirm.count(), 5);
    assert!(confirm.arming.is_none(), "a local delete needs no arming");
    assert!(confirm.armed(), "and is armed immediately");
    // Confirming submits a real Delete op to the queue.
    b.confirm_delete();
    assert!(b.pending_delete().is_none(), "the confirm closed");
    assert_eq!(b.ops().active().len(), 1, "a Delete op is queued");
}

#[test]
fn an_empty_selection_delete_is_an_honest_note_not_a_dialog() {
    let mut b = five_row_browser();
    b.clear_selection(0);
    b.request_delete(0);
    assert!(b.pending_delete().is_none());
    assert!(b.last_note().is_some(), "an honest note explains why");
}

#[test]
fn a_remote_delete_demands_the_typed_node_and_flags_escalation() {
    // A row on the stable lock-11 mount root for peer `oak`, whose worker
    // state reports an escalated (full-filesystem) mount.
    let remote = "/run/user/1000/mde-mesh/oak/docs/report.txt";
    let rows = vec![FileRow::local("report.txt", Mime::Doc, "1 KB", "now").with_path(remote)];
    let fake = FakeMeshMount::new().with_view("oak", mounted_view(remote, MountScope::Full));
    let mut b = FileBrowser::with_file_ops(
        Box::new(FixtureBackend::new(Vec::new(), rows)),
        FakeFileOps::new(),
    )
    .with_mesh_mount(Box::new(fake));
    b.click(0, 0);
    b.request_delete(0);
    let confirm = b.pending_delete().expect("a confirm opened");
    let arming = confirm.arming.as_ref().expect("a remote delete arms");
    assert_eq!(arming.node, "oak");
    assert!(arming.full_fs, "the escalated full-fs mount is flagged");
    assert!(!confirm.armed(), "un-typed → not armed");
    // A confirm while unarmed is a no-op (the button is disabled too).
    b.confirm_delete();
    assert!(
        b.pending_delete().is_some(),
        "an unarmed confirm never fires"
    );
    assert!(b.ops().active().is_empty());
    // The exact node name arms it, and it then submits.
    b.set_delete_echo("oak".into());
    assert!(b.pending_delete().expect("still open").armed());
    b.confirm_delete();
    assert_eq!(b.ops().active().len(), 1, "the armed delete submitted");
}

#[test]
fn a_home_mount_delete_arms_on_the_node_but_is_not_escalated() {
    let remote = "/run/user/1000/mde-mesh/oak/docs/a.txt";
    let rows = vec![FileRow::local("a.txt", Mime::Doc, "1 KB", "now").with_path(remote)];
    let mut b = FileBrowser::with_file_ops(
        Box::new(FixtureBackend::new(Vec::new(), rows)),
        FakeFileOps::new(),
    );
    b.click(0, 0);
    b.request_delete(0);
    let arming = b
        .pending_delete()
        .and_then(|c| c.arming.as_ref())
        .expect("the stable mesh root arms even with no worker state");
    assert_eq!(arming.node, "oak");
    assert!(!arming.full_fs, "a home mount is not escalated");
}

#[test]
fn a_conflict_surfaces_to_the_model_and_the_answer_completes_the_op() {
    let (mut b, dst) = transfer_browser(true);
    b.click(0, 0);
    let id = b
        .drop_transfer(0, dst, true)
        .expect("a local copy is queued");
    // Pump until the collision surfaces through the model.
    // logic-timing, not motion (test poll loop — bounded timeout + pump cadence)
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        b.pump_ops();
        if b.pending_conflict().is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "collision never surfaced");
        std::thread::sleep(Duration::from_millis(5));
    }
    let (blocked, _) = b.pending_conflict().expect("pending");
    assert_eq!(blocked, id);
    assert!(b.any_pending_conflict());
    // Answer keep-both through the model; the op then finishes.
    b.resolve_conflict(id, Resolution::KeepBoth, false);
    assert!(!b.any_pending_conflict(), "the prompt was consumed");
    // logic-timing, not motion (test poll loop — bounded timeout + pump cadence)
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        b.pump_ops();
        let done = b
            .ops()
            .active()
            .iter()
            .find(|o| o.op_id == id)
            .is_some_and(crate::ops::ActiveOp::is_done);
        if done {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "op never finished after the answer"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let outcome = b
        .ops()
        .active()
        .iter()
        .find(|o| o.op_id == id)
        .and_then(|o| o.outcome.as_ref())
        .expect("finished");
    assert_eq!(
        outcome.items_completed, 1,
        "keep-both copied the incoming file"
    );
}

#[test]
fn properties_load_edit_and_apply_run_through_the_injected_meta_ops() {
    // A seeded fake FS is BOTH the source of truth for Properties and where a
    // chmod actually lands — the model drives it through the meta-ops seam.
    let meta = FakeFileOps::privileged();
    meta.create_dir(Path::new("/d")).expect("mkdir");
    meta.seed_file("/d/report.txt", b"hello").expect("seed");
    meta.set_permissions(Path::new("/d/report.txt"), 0o644)
        .expect("seed mode");
    let rows =
        vec![FileRow::local("report.txt", Mime::Doc, "5 B", "now").with_path("/d/report.txt")];
    let mut b = FileBrowser::with_file_ops(
        Box::new(FixtureBackend::new(Vec::new(), rows)),
        FakeFileOps::new(),
    )
    .with_meta_ops(meta, true);
    b.click(0, 0);
    b.open_properties(0);
    assert_eq!(b.properties().expect("open").perms.octal(), "0644");
    // Toggle owner-exec via the model, then apply — the chmod really takes.
    b.properties_toggle_perm(PermClass::Owner, Perm::Exec);
    assert_eq!(b.properties().expect("open").octal_edit, "0744");
    b.properties_apply(0);
    assert!(matches!(
        b.properties().expect("still open").outcome,
        Some(Ok(()))
    ));
    assert_eq!(
        b.properties().expect("open").perms.octal(),
        "0744",
        "the dialog re-synced to the applied mode"
    );
    b.close_properties();
    assert!(b.properties().is_none());
}

#[test]
fn open_properties_on_a_pathless_peer_row_is_an_honest_note() {
    let mut b = browser_over(roster_backend());
    b.navigate(0, Location::Peer("pine".into()));
    b.click(0, 0); // a virtual peer row (no path)
    b.open_properties(0);
    assert!(
        b.properties().is_none(),
        "no dialog opens for a pathless row"
    );
    assert_eq!(
        b.last_note(),
        Some("This entry has no local path - mount the peer to inspect it.")
    );
}

#[test]
fn new_folder_and_rename_execute_through_the_existing_fileops_authority() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("report.txt");
    std::fs::write(&source, b"report").expect("seed source");
    let rows =
        vec![FileRow::local("report.txt", Mime::Doc, "6 B", "now")
            .with_path(source.to_string_lossy())];
    let mut b = FileBrowser::with_file_ops(
        Box::new(FixtureBackend::new(Vec::new(), rows)),
        FakeFileOps::new(),
    )
    .with_meta_ops(mde_files::fileops::LiveFileOps::new(), false);

    b.open_new_folder(0);
    b.set_name_dialog_input("Projects".to_string());
    b.submit_name_dialog(0);
    assert!(
        b.name_dialog().is_none(),
        "successful create closes the dialog"
    );
    assert!(
        temp.path().join("Projects").is_dir(),
        "FileOps created the folder"
    );

    b.click(0, 0);
    b.open_rename(0);
    b.set_name_dialog_input("report-final.txt".to_string());
    b.submit_name_dialog(0);
    assert!(
        b.name_dialog().is_none(),
        "successful rename closes the dialog"
    );
    assert!(!source.exists(), "the old name is gone");
    assert_eq!(
        std::fs::read(temp.path().join("report-final.txt")).expect("renamed file"),
        b"report",
        "FileOps preserved the renamed payload"
    );
}

#[test]
fn name_operations_refuse_invalid_or_existing_targets_without_mutation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("source.txt");
    let occupied = temp.path().join("occupied.txt");
    std::fs::write(&source, b"source").expect("seed source");
    std::fs::write(&occupied, b"occupied").expect("seed destination");
    let rows =
        vec![FileRow::local("source.txt", Mime::Doc, "6 B", "now")
            .with_path(source.to_string_lossy())];
    let mut b = FileBrowser::with_file_ops(
        Box::new(FixtureBackend::new(Vec::new(), rows)),
        FakeFileOps::new(),
    )
    .with_meta_ops(mde_files::fileops::LiveFileOps::new(), false);

    b.open_new_folder(0);
    b.set_name_dialog_input("nested/path".to_string());
    b.submit_name_dialog(0);
    assert!(b.name_dialog().is_some(), "invalid input stays open");
    assert!(
        !temp.path().join("nested").exists(),
        "no partial path was created"
    );
    b.cancel_name_dialog();

    b.click(0, 0);
    b.open_rename(0);
    b.set_name_dialog_input("occupied.txt".to_string());
    b.submit_name_dialog(0);
    let error = b
        .name_dialog()
        .and_then(|d| d.error.as_deref())
        .expect("collision is visible");
    assert!(
        error.contains("already exists"),
        "honest collision: {error}"
    );
    assert_eq!(std::fs::read(&source).expect("source intact"), b"source");
    assert_eq!(
        std::fs::read(&occupied).expect("target intact"),
        b"occupied"
    );
}

// ── TRANSFERS-8: the tab + the three Submit entry points (Q13) ────────────

use crate::transfers::test_support::FakeTransfers;
use crate::transfers::{
    Method as XMethod, TransferJob as XJob, TransferPolicy as XPolicy, TransferState as XState,
    TransferVerb,
};

/// A one-file local browser wired to a recording [`FakeTransfers`] — the fixture
/// the entry-point tests submit from. Returns the browser + the fake handle
/// (a clone shares the recorded dispatch log).
fn transfers_browser(rows: Vec<FileRow>, fake: FakeTransfers) -> FileBrowser {
    FileBrowser::with_file_ops(
        Box::new(FixtureBackend::new(Vec::new(), rows)),
        FakeFileOps::new(),
    )
    .with_transfers(Box::new(fake))
}

fn one_local_file() -> Vec<FileRow> {
    vec![FileRow::local("clip.mp3", Mime::Doc, "3 MB", "now").with_path("/home/me/clip.mp3")]
}

#[test]
fn surface_tab_defaults_to_files_and_switches() {
    let mut b = browser_over(roster_backend());
    assert_eq!(b.surface_tab(), SurfaceTab::Files);
    b.set_surface_tab(SurfaceTab::Transfers);
    assert_eq!(b.surface_tab(), SurfaceTab::Transfers);
}

#[test]
fn peer_sync_label_is_honest_when_no_transfer_worker_exists() {
    let b = browser_over(roster_backend());
    assert_eq!(b.peer_sync_label("pine"), "sync unavailable");
}

#[test]
fn entry_point_right_click_send_to_target_emits_a_submit() {
    let fake = FakeTransfers::new();
    let mut b = transfers_browser(one_local_file(), fake.clone());
    b = b.with_music_service_available(true);
    b.click(0, 0);
    let target = b
        .transfer_targets()
        .into_iter()
        .find(|t| t.label == "Music Library")
        .expect("Music Library is a standing target");
    b.send_to_target(0, &target);
    let verbs = fake.verbs();
    assert_eq!(verbs.len(), 1, "one Submit per selected file");
    assert!(
        matches!(&verbs[0], TransferVerb::Submit(job)
                if job.source == "/home/me/clip.mp3"
                    && job.dest == "music:library"
                    && job.method == XMethod::Music),
        "the right-click entry point emits a Music Submit: {:?}",
        verbs[0]
    );
}

#[test]
fn entry_point_drag_drop_onto_a_target_emits_a_submit() {
    let fake = FakeTransfers::new();
    let mut b = transfers_browser(one_local_file(), fake.clone());
    b.click(0, 0);
    let target = b
        .transfer_targets()
        .into_iter()
        .find(|t| t.kind == crate::transfers::TargetKind::MeshShare)
        .expect("Mesh Share is a standing target");
    b.drop_on_target(0, &target);
    let verbs = fake.verbs();
    assert_eq!(verbs.len(), 1);
    assert!(matches!(&verbs[0], TransferVerb::Submit(j) if j.dest == "mesh-share:"));
}

#[test]
fn entry_point_new_transfer_dialog_submits_and_closes() {
    let fake = FakeTransfers::new();
    let mut b = transfers_browser(Vec::new(), fake.clone());
    b.open_new_transfer();
    let mut form = b.new_transfer().expect("dialog open").clone();
    form.source = "https://example.com/f.iso".into();
    form.dest = "/downloads".into();
    form.method = XMethod::Http;
    b.set_new_transfer_form(form);
    b.submit_new_transfer();
    assert!(b.new_transfer().is_none(), "submit closes the dialog");
    let verbs = fake.verbs();
    assert_eq!(verbs.len(), 1);
    assert!(
        matches!(&verbs[0], TransferVerb::Submit(job)
                if job.source == "https://example.com/f.iso"
                    && job.dest == "/downloads"
                    && job.method == XMethod::Http),
        "the dialog entry point emits an HTTP Submit: {:?}",
        verbs[0]
    );
}

#[test]
fn send_to_target_with_no_local_selection_is_an_honest_note() {
    let fake = FakeTransfers::new();
    let mut b = transfers_browser(one_local_file(), fake.clone());
    // No selection → nothing submitted, an honest note instead of a silent no-op.
    let target = b.transfer_targets().into_iter().next().unwrap();
    b.send_to_target(0, &target);
    assert_eq!(fake.dispatch_count(), 0, "nothing selected → no Submit");
    assert!(b.last_note().is_some());
}

#[test]
fn batch_verbs_dispatch_per_eligible_ledger_job() {
    let mut running = XJob::new("/a", "/b", XMethod::Rsync, XPolicy::default());
    running.state = XState::Running;
    let mut paused = XJob::new("/c", "/d", XMethod::Http, XPolicy::default());
    paused.state = XState::Paused;
    let mut done = XJob::new("/e", "/f", XMethod::Node, XPolicy::default());
    done.state = XState::Done;
    let fake = FakeTransfers::new().with_jobs(vec![running.clone(), paused.clone(), done.clone()]);
    let mut b = transfers_browser(Vec::new(), fake.clone());
    // Pause-all → one Pause (the running); Resume-all → one Resume (the paused);
    // Clear-completed → one Cancel (the done).
    b.transfer_pause_all();
    b.transfer_resume_all();
    b.transfer_clear_completed();
    let verbs = fake.verbs();
    assert_eq!(verbs.len(), 3, "one verb per eligible job");
    assert!(verbs
        .iter()
        .any(|v| matches!(v, TransferVerb::Pause(id) if *id == running.id)));
    assert!(verbs
        .iter()
        .any(|v| matches!(v, TransferVerb::Resume(id) if *id == paused.id)));
    assert!(verbs
        .iter()
        .any(|v| matches!(v, TransferVerb::Cancel(id) if *id == done.id)));
}

#[test]
fn transfers_view_and_counts_read_the_injected_ledger() {
    let mut running = XJob::new("/a", "/b", XMethod::Rsync, XPolicy::default());
    running.state = XState::Running;
    let mut done = XJob::new("/e", "/f", XMethod::Node, XPolicy::default());
    done.state = XState::Done;
    let fake = FakeTransfers::new().with_jobs(vec![running, done]);
    let b = transfers_browser(Vec::new(), fake);
    assert_eq!(b.transfers_view().len(), 2);
    let c = b.transfers_counts();
    assert_eq!(c.active, 1, "the running job is active");
    assert_eq!(c.terminal, 1, "the done job is terminal");
    assert!(b.transfers_active());
}

#[test]
fn operation_progress_summary_folds_active_transfer_jobs_for_shell_chrome() {
    let mut running = XJob::new("/a/large.iso", "/b", XMethod::Rsync, XPolicy::default());
    running.state = XState::Running;
    running.progress = Some(40);
    let mut queued = XJob::new("/c/archive.tar", "/d", XMethod::Http, XPolicy::default());
    queued.state = XState::Queued;
    let mut done = XJob::new("/e/old.log", "/f", XMethod::Node, XPolicy::default());
    done.state = XState::Done;
    let fake = FakeTransfers::new().with_jobs(vec![running, queued, done]);
    let b = transfers_browser(Vec::new(), fake);

    let summary = b
        .operation_progress_summary()
        .expect("two active transfer jobs");
    assert_eq!(summary.active, 2);
    assert_eq!(summary.known_progress, 1);
    assert_eq!(summary.fraction, Some(0.4));
    assert_eq!(summary.label, "2 transfers");
}

#[test]
fn operation_progress_summary_folds_archive_queue_ops_for_shell_chrome() {
    let mut b = transfers_browser(Vec::new(), FakeTransfers::new());
    b.ops.submit(
        OpKind::Compress {
            items: vec![PathBuf::from("project")],
            base_dir: PathBuf::from("/home/me"),
            archive: PathBuf::from("/home/me/project.zip"),
            format: ArchiveFormat::Zip,
        },
        "Compress project.zip",
    );
    b.ops.submit(
        OpKind::Extract {
            archive: PathBuf::from("/home/me/archive.zip"),
            dest_dir: PathBuf::from("/home/me/archive"),
        },
        "Extract archive.zip",
    );

    let summary = b
        .operation_progress_summary()
        .expect("archive queue ops should feed shell chrome");
    assert_eq!(summary.active, 2);
    assert_eq!(summary.known_progress, 0);
    assert_eq!(summary.fraction, None);
    assert_eq!(summary.label, "2 local file operations");
}

#[test]
fn operation_progress_summary_bounds_labels_with_ascii_ellipsis_for_shell_chrome() {
    let label = truncate_operation_label("Copy very-long-platform-operation-report-final.txt");

    assert!(label.ends_with("..."), "truncated label = {label}");
    assert!(label.is_ascii(), "truncated label = {label}");
    assert!(
        label.chars().count() <= 42,
        "Files progress summary labels must stay bounded: {label}"
    );
    assert!(!label.contains('…'), "label must not use Unicode ellipsis");
}

// ── WL-FUNC-025 POSIX ops + WL-FUNC-026 prefs + WL-FUNC-027 bookmarks ─────

fn live_posix_browser(rows: Vec<FileRow>) -> FileBrowser {
    FileBrowser::with_file_ops(
        Box::new(FixtureBackend::new(Vec::new(), rows)),
        LiveFileOps::new(),
    )
    .with_meta_ops(LiveFileOps::new(), false)
}

fn pump_until_idle(b: &mut FileBrowser) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        b.pump_ops();
        let busy = b.ops().any_running() || b.any_pending_conflict();
        if !busy {
            b.pump_ops();
            return;
        }
        assert!(Instant::now() < deadline, "op queue never went idle");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn raw_tar_bytes(name: &str, data: &[u8]) -> Vec<u8> {
    let mut h = [0u8; 512];
    h[..name.len()].copy_from_slice(name.as_bytes());
    h[100..108].copy_from_slice(b"0000644\0");
    h[108..116].copy_from_slice(b"0000000\0");
    h[116..124].copy_from_slice(b"0000000\0");
    h[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
    h[136..148].copy_from_slice(b"00000000000\0");
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    for b in &mut h[148..156] {
        *b = b' ';
    }
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    let mut out = h.to_vec();
    out.extend_from_slice(data);
    out.resize(out.len() + (512 - data.len() % 512) % 512, 0);
    out.resize(out.len() + 1024, 0);
    out
}

#[test]
fn new_file_refuses_an_existing_name_without_mutation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let occupied = temp.path().join("taken.txt");
    std::fs::write(&occupied, b"keep").expect("seed");
    let rows =
        vec![FileRow::local("taken.txt", Mime::Doc, "4 B", "now")
            .with_path(occupied.to_string_lossy())];
    let mut b = live_posix_browser(rows);
    b.open_new_file(0);
    b.set_name_dialog_input("taken.txt".to_string());
    b.submit_name_dialog(0);
    let error = b
        .name_dialog()
        .and_then(|d| d.error.as_deref())
        .expect("collision is visible");
    assert!(
        error.contains("already exists"),
        "honest collision: {error}"
    );
    assert_eq!(std::fs::read(&occupied).expect("intact"), b"keep");
}

#[test]
fn new_file_and_duplicate_refuse_a_read_only_directory() {
    // /proc is not a writable parent even for root (procfs), so this is an
    // honest read-only/unwritable-directory refusal without chmod races.
    let rows = vec![FileRow::local("stat", Mime::Doc, "0 B", "now").with_path("/proc/stat")];
    let mut b = live_posix_browser(rows);
    b.open_new_file(0);
    b.set_name_dialog_input("mcnf-ro-test".to_string());
    b.submit_name_dialog(0);
    let error = b
        .name_dialog()
        .and_then(|d| d.error.as_deref())
        .expect("unwritable create is visible");
    assert!(
        error.contains("Couldn't create file") || error.to_ascii_lowercase().contains("permission"),
        "honest read-only: {error}"
    );
    b.cancel_name_dialog();
    b.click(0, 0);
    b.duplicate_selection(0);
    let note = b.last_note().unwrap_or("");
    assert!(
        note.contains("Couldn't duplicate") || note.to_ascii_lowercase().contains("permission"),
        "honest duplicate: {note}"
    );
}

#[test]
fn new_file_duplicate_and_symlink_execute_on_a_local_tree() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("report.txt");
    std::fs::write(&source, b"report").expect("seed");
    let rows =
        vec![FileRow::local("report.txt", Mime::Doc, "6 B", "now")
            .with_path(source.to_string_lossy())];
    let mut b = live_posix_browser(rows);

    b.open_new_file(0);
    b.set_name_dialog_input("notes.txt".to_string());
    b.submit_name_dialog(0);
    assert!(b.name_dialog().is_none());
    assert_eq!(
        std::fs::read(temp.path().join("notes.txt")).expect("new file"),
        b""
    );

    b.click(0, 0);
    b.duplicate_selection(0);
    pump_until_idle(&mut b);
    let dup = temp.path().join("report (copy).txt");
    assert_eq!(std::fs::read(&dup).expect("duplicate payload"), b"report");

    b.click(0, 0);
    b.open_symlink(0);
    b.set_name_dialog_input("report.link".to_string());
    b.submit_name_dialog(0);
    assert!(b.name_dialog().is_none(), "symlink created");
    let link = temp.path().join("report.link");
    let meta = std::fs::symlink_metadata(&link).expect("lstat");
    assert!(meta.file_type().is_symlink());
}

#[test]
fn duplicate_of_an_existing_copy_name_raises_the_conflict_dialog() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("report.txt");
    let occupied = temp.path().join("report (copy).txt");
    std::fs::write(&source, b"new").expect("seed");
    std::fs::write(&occupied, b"old").expect("seed dest");
    let rows =
        vec![FileRow::local("report.txt", Mime::Doc, "3 B", "now")
            .with_path(source.to_string_lossy())];
    let mut b = live_posix_browser(rows);
    b.click(0, 0);
    b.duplicate_selection(0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while b.pending_conflict().is_none() {
        b.pump_ops();
        assert!(Instant::now() < deadline, "collision never surfaced");
        std::thread::sleep(Duration::from_millis(5));
    }
    let (id, conflict) = b.pending_conflict().expect("conflict");
    assert!(
        conflict.dst.ends_with("report (copy).txt"),
        "conflict dest = {}",
        conflict.dst.display()
    );
    b.resolve_conflict(id, Resolution::Skip, true);
    pump_until_idle(&mut b);
    assert_eq!(std::fs::read(&occupied).expect("kept"), b"old");
}

#[test]
fn posix_ops_refuse_pathless_peer_rows() {
    let mut b = browser_over(roster_backend());
    b.navigate(0, Location::Peer("pine".into()));
    b.click(0, 0);
    b.open_new_file(0);
    assert!(b.name_dialog().is_none());
    assert!(b.last_note().is_some());
    b.duplicate_selection(0);
    assert!(b.last_note().unwrap_or("").contains("Select a local item"));
    b.compress_selection(0, ArchiveFormat::Zip);
    assert!(b.last_note().unwrap_or("").contains("Select a local item"));
}

#[test]
fn extract_here_refuses_path_traversal_members() {
    let temp = tempfile::tempdir().expect("tempdir");
    let archive = temp.path().join("evil.tar");
    std::fs::write(&archive, raw_tar_bytes("../escaped.txt", b"pwned")).expect("seed tar");
    let dest = temp.path().join("dest");
    std::fs::create_dir(&dest).expect("dest");
    let rows = vec![FileRow::local("evil.tar", Mime::Archive, "1 KB", "now")
        .with_path(archive.to_string_lossy())];
    let mut b = live_posix_browser(rows);
    b.click(0, 0);
    b.extract_here(0);
    pump_until_idle(&mut b);
    assert!(
        !temp.path().join("escaped.txt").exists(),
        "traversal member was not written beside the archive"
    );
    assert!(!dest.join("escaped.txt").exists());
}

#[test]
fn extract_to_refuses_a_symlinked_destination_without_queueing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let archive = temp.path().join("payload.zip");
    std::fs::write(&archive, b"not an archive").expect("seed archive path");
    let outside = temp.path().join("outside");
    std::fs::create_dir(&outside).expect("outside");
    let destination = temp.path().join("chosen");
    std::os::unix::fs::symlink(&outside, &destination).expect("destination symlink");
    let rows = vec![FileRow::local("payload.zip", Mime::Archive, "14 B", "now")
        .with_path(archive.to_string_lossy())];
    let mut b = live_posix_browser(rows);
    b.click(0, 0);
    b.open_extract_to(0);
    b.set_extract_to_dest(destination.to_string_lossy().into_owned());
    b.submit_extract_to();

    let dialog = b
        .extract_to_dialog()
        .expect("hostile destination stays open");
    let error = dialog.error.as_deref().unwrap_or("");
    assert!(
        error.contains("cannot contain a symbolic link"),
        "honest symlink refusal: {error}"
    );
    assert!(
        b.ops().active().is_empty(),
        "invalid destination never reaches the queue"
    );
    assert!(outside
        .read_dir()
        .expect("outside remains readable")
        .next()
        .is_none());
}

#[test]
fn cancel_compress_leaves_no_half_archive() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("blob.bin");
    std::fs::write(&source, vec![b'A'; 64 * 1024]).expect("seed");
    let rows =
        vec![FileRow::local("blob.bin", Mime::Doc, "64 KB", "now")
            .with_path(source.to_string_lossy())];
    let mut b = live_posix_browser(rows);
    b.click(0, 0);
    b.compress_selection(0, ArchiveFormat::Zip);
    let ids: Vec<_> = b.ops().active().iter().map(|op| op.op_id).collect();
    for id in ids {
        b.cancel_op(id);
    }
    pump_until_idle(&mut b);
    let archive = temp.path().join("blob.zip");
    if archive.exists() {
        let bytes = std::fs::read(&archive).expect("read");
        assert!(
            bytes.starts_with(b"PK"),
            "a finished archive is complete, never a half-write"
        );
    }
}

#[test]
fn zip_and_tar_gz_round_trip_through_the_queue() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("payload.txt");
    std::fs::write(&source, b"round-trip").expect("seed");
    let rows =
        vec![FileRow::local("payload.txt", Mime::Doc, "10 B", "now")
            .with_path(source.to_string_lossy())];
    let mut b = live_posix_browser(rows);
    b.click(0, 0);
    b.compress_selection(0, ArchiveFormat::Zip);
    pump_until_idle(&mut b);
    let zip = temp.path().join("payload.zip");
    assert!(zip.is_file(), "zip archive written");

    b.click(0, 0);
    b.compress_selection(0, ArchiveFormat::TarGz);
    pump_until_idle(&mut b);
    let tgz = temp.path().join("payload.tar.gz");
    assert!(tgz.is_file(), "tar.gz archive written");

    let out = temp.path().join("out");
    std::fs::create_dir(&out).expect("out");
    let rows = vec![FileRow::local("payload.zip", Mime::Archive, "1 KB", "now")
        .with_path(zip.to_string_lossy())];
    let mut b = live_posix_browser(rows);
    b.click(0, 0);
    b.open_extract_to(0);
    b.set_extract_to_dest(out.to_string_lossy().into_owned());
    b.submit_extract_to();
    pump_until_idle(&mut b);
    assert_eq!(
        std::fs::read(out.join("payload.txt")).expect("extracted zip"),
        b"round-trip"
    );
}

#[test]
fn symlink_escaping_a_mesh_mount_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mount = temp.path().join("run/user/1000/mde-mesh/oak/docs");
    std::fs::create_dir_all(&mount).expect("mount");
    let source = mount.join("notes.txt");
    std::fs::write(&source, b"notes").expect("seed");
    let rows =
        vec![FileRow::local("notes.txt", Mime::Doc, "5 B", "now")
            .with_path(source.to_string_lossy())];
    let mut b = live_posix_browser(rows);
    b.click(0, 0);
    b.open_symlink(0);
    b.set_name_dialog_input("escape.link".to_string());
    b.set_name_operation_for_test(NameOperation::Symlink {
        target: PathBuf::from("/etc/passwd"),
        parent: mount.clone(),
    });
    b.submit_name_dialog(0);
    let error = b
        .name_dialog()
        .and_then(|d| d.error.as_deref())
        .unwrap_or_else(|| b.last_note().unwrap_or(""));
    assert!(
        error.contains("escapes this mesh mount"),
        "mesh-escape refused: {error}"
    );
}

#[test]
fn symlink_escape_helper_detects_a_target_outside_the_mount() {
    let parent = Path::new("/run/user/1000/mde-mesh/oak/docs");
    let link = parent.join("escape.link");
    assert!(super::symlink_escapes_mesh(
        parent,
        &link,
        Path::new("/etc/passwd")
    ));
    assert!(!super::symlink_escapes_mesh(
        parent,
        &link,
        Path::new("/run/user/1000/mde-mesh/oak/docs/notes.txt")
    ));
}

#[test]
fn hardlink_across_devices_is_an_honest_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("report.txt");
    std::fs::write(&source, b"report").expect("seed");
    let rows =
        vec![FileRow::local("report.txt", Mime::Doc, "6 B", "now")
            .with_path(source.to_string_lossy())];
    let mut b = live_posix_browser(rows);
    b.click(0, 0);
    b.open_hard_link(0);
    b.set_name_operation_for_test(NameOperation::HardLink {
        target: source,
        parent: PathBuf::from("/proc"),
    });
    b.set_name_dialog_input("mcnf-hl-test".to_string());
    b.submit_name_dialog(0);
    let error = b
        .name_dialog()
        .and_then(|d| d.error.as_deref())
        .unwrap_or("");
    assert!(
        error.contains("across devices")
            || error.contains("Couldn't create hard link")
            || error.contains("Couldn't check the destination"),
        "honest hardlink error: {error}"
    );
}

#[test]
fn folder_prefs_survive_restart_and_hostile_files_degrade() {
    let temp = tempfile::tempdir().expect("tempdir");
    let rows = vec![FileRow::local("a.txt", Mime::Doc, "1 B", "now").with_path("/work/a.txt")];
    let mut b = live_posix_browser(rows.clone()).with_config_dir(temp.path());
    b.navigate(0, Location::Local("/work".into()));
    b.set_view(0, ViewMode::Grid);
    b.toggle_hidden(0);
    b.flush_persisted();
    let stored = temp.path().join("files-folder-prefs.json");
    assert!(stored.is_file());

    let b2 = live_posix_browser(rows).with_config_dir(temp.path());
    let prefs = b2.folder_prefs().get("/work").copied().expect("hydrated");
    assert_eq!(prefs.view, ViewMode::Grid);
    assert!(prefs.show_hidden);

    let hostile = tempfile::tempdir().expect("hostile");
    std::os::unix::fs::symlink("/tmp", hostile.path().join("files-folder-prefs.json"))
        .expect("symlink");
    let b3 = live_posix_browser(Vec::new()).with_config_dir(hostile.path());
    assert!(b3.folder_prefs().is_empty());
    assert!(
        b3.last_note()
            .is_some_and(|n| n.contains("symlink") || n.contains("defaults")),
        "symlink prefs: {:?}",
        b3.last_note()
    );

    let corrupt = tempfile::tempdir().expect("corrupt");
    std::fs::write(corrupt.path().join("files-folder-prefs.json"), b"{nope").expect("write");
    let err = load_folder_prefs_at(&corrupt.path().join("files-folder-prefs.json"))
        .expect_err("corrupt JSON");
    assert!(err.contains("not valid JSON") || err.contains("defaults"));

    let oversize = tempfile::tempdir().expect("oversize");
    std::fs::write(
        oversize.path().join("files-folder-prefs.json"),
        vec![b'x'; 256 * 1024 + 8],
    )
    .expect("write");
    let err = load_folder_prefs_at(&oversize.path().join("files-folder-prefs.json"))
        .expect_err("oversize");
    assert!(err.contains("larger than"));
}

#[test]
fn persistence_replaces_a_store_symlink_without_following_it() {
    let dir = tempfile::tempdir().expect("store dir");
    let target = dir.path().join("outside.json");
    std::fs::write(&target, b"must stay untouched").expect("seed target");
    let store = dir.path().join("files-folder-prefs.json");
    std::os::unix::fs::symlink(&target, &store).expect("symlink store");

    super::write_json_store(&store, &super::FolderPrefsFile::default()).expect("safe write");

    assert_eq!(
        std::fs::read(&target).expect("target"),
        b"must stay untouched"
    );
    assert!(!std::fs::symlink_metadata(&store)
        .expect("store")
        .file_type()
        .is_symlink());
    assert!(load_folder_prefs_at(&store)
        .expect("new store")
        .entries
        .is_empty());
}

#[test]
fn bookmarks_pin_rename_reorder_and_refuse_hostile_input() {
    let temp = tempfile::tempdir().expect("tempdir");
    let folder = temp.path().join("Projects");
    std::fs::create_dir(&folder).expect("mkdir");
    let rows = vec![
        FileRow::local("Projects", Mime::Folder, "\u{2014}", "\u{2014}")
            .with_path(folder.to_string_lossy()),
    ];
    let mut b = live_posix_browser(rows).with_config_dir(temp.path());
    b.click(0, 0);
    b.pin_focused(0);
    assert_eq!(b.bookmarks().len(), 1);
    b.pin_focused(0);
    assert!(
        b.last_note().is_some_and(|n| n.contains("already pinned")),
        "duplicate pin: {:?}",
        b.last_note()
    );
    b.flush_persisted();
    drop(b);

    let other = temp.path().join("Docs");
    std::fs::create_dir(&other).expect("mkdir");
    let store = tempfile::tempdir().expect("bookmark store");
    let rows = vec![
        FileRow::local("Projects", Mime::Folder, "\u{2014}", "\u{2014}")
            .with_path(folder.to_string_lossy()),
        FileRow::local("Docs", Mime::Folder, "\u{2014}", "\u{2014}")
            .with_path(other.to_string_lossy()),
    ];
    let mut b = live_posix_browser(rows).with_config_dir(store.path());
    b.click(0, 0);
    b.pin_focused(0);
    b.click(0, 1);
    b.pin_focused(0);
    assert_eq!(b.bookmarks().len(), 2);
    b.reorder_bookmark(1, -1);
    assert_eq!(b.bookmarks()[0].path, folder.to_string_lossy());
    b.open_rename_bookmark(0);
    b.set_name_dialog_input("Documents".to_string());
    b.submit_name_dialog(0);
    assert_eq!(b.bookmarks()[0].label, "Documents");
    b.unpin_bookmark(1);
    assert_eq!(b.bookmarks().len(), 1);
    b.flush_persisted();
    drop(b);

    let b2 = live_posix_browser(Vec::new()).with_config_dir(store.path());
    assert_eq!(b2.bookmarks().len(), 1);
    assert_eq!(b2.bookmarks()[0].label, "Documents");

    let missing = load_bookmarks_at(Path::new("/no/such/files-bookmarks.json"));
    assert!(missing
        .expect("missing store is empty defaults")
        .bookmarks
        .is_empty());

    let hostile = tempfile::tempdir().expect("hostile");
    std::fs::write(hostile.path().join("files-bookmarks.json"), b"{").expect("write");
    let note =
        load_bookmarks_at(&hostile.path().join("files-bookmarks.json")).expect_err("corrupt");
    assert!(note.contains("not valid JSON") || note.contains("defaults"));
}

#[test]
fn bookmark_validate_refuses_peer_and_escaping_paths() {
    assert!(super::validate_bookmark_path("peer:oak").is_err());
    assert!(super::validate_bookmark_path("/tmp/../etc/passwd").is_err());
    assert!(super::validate_bookmark_path("relative").is_err());
    assert!(super::validate_bookmark_path("/home/me/docs").is_ok());
    assert!(super::validate_bookmark_path("local:home").is_ok());
}

// WL-FUNC-027 — add/remove/reorder round-trip: every mutation on the user
// bookmark list survives a fresh browser restart with the same on-disk store.
// The prior test only verified rename survived; this one exercises reorder
// and remove explicitly across process boundaries so the persisted display
// order can never drift from what the operator last saw.
#[test]
fn bookmarks_add_remove_and_reorder_all_survive_restart() {
    let temp = tempfile::tempdir().expect("tempdir");
    let alpha = temp.path().join("Alpha");
    let bravo = temp.path().join("Bravo");
    let charlie = temp.path().join("Charlie");
    for dir in [&alpha, &bravo, &charlie] {
        std::fs::create_dir(dir).expect("mkdir");
    }
    let rows_for = || {
        vec![
            FileRow::local("Alpha", Mime::Folder, "\u{2014}", "\u{2014}")
                .with_path(alpha.to_string_lossy()),
            FileRow::local("Bravo", Mime::Folder, "\u{2014}", "\u{2014}")
                .with_path(bravo.to_string_lossy()),
            FileRow::local("Charlie", Mime::Folder, "\u{2014}", "\u{2014}")
                .with_path(charlie.to_string_lossy()),
        ]
    };
    let store = tempfile::tempdir().expect("store");

    let mut b = live_posix_browser(rows_for()).with_config_dir(store.path());
    b.click(0, 0);
    b.pin_focused(0);
    b.click(0, 1);
    b.pin_focused(0);
    b.click(0, 2);
    b.pin_focused(0);
    assert_eq!(b.bookmarks().len(), 3);
    let display_before = b
        .bookmarks()
        .iter()
        .map(|bm| bm.path.clone())
        .collect::<Vec<_>>();
    // Move the bottom bookmark to the top so the persisted order is neither
    // the pinning order nor the sort-collated ordering the sidebar reads.
    b.reorder_bookmark(2, -1);
    b.reorder_bookmark(1, -1);
    let mut expected_after_reorder = display_before.clone();
    expected_after_reorder.swap(2, 1);
    expected_after_reorder.swap(1, 0);
    assert_eq!(
        b.bookmarks()
            .iter()
            .map(|bm| bm.path.clone())
            .collect::<Vec<_>>(),
        expected_after_reorder
    );
    b.unpin_bookmark(1);
    let expected_after_remove: Vec<String> = expected_after_reorder
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 1)
        .map(|(_, p)| p.clone())
        .collect();
    b.flush_persisted();
    drop(b);

    let b2 = live_posix_browser(rows_for()).with_config_dir(store.path());
    let paths_after: Vec<String> = b2.bookmarks().iter().map(|bm| bm.path.clone()).collect();
    assert_eq!(paths_after, expected_after_remove);
    assert_eq!(b2.bookmarks().len(), 2);
}

// WL-FUNC-027 — a symlink at the bookmarks store path is a classic
// preferences-hijack attack: the attacker leaves a symlink pointing at a
// sensitive file so the surface either reads it as JSON or overwrites it on
// the next flush. `load_bookmarks_at` must refuse the symlink outright, the
// hydrating browser must degrade to empty defaults with an operator-visible
// note, and the target file bytes must survive untouched.
#[test]
fn bookmarks_reject_a_symlinked_store_and_leave_target_intact() {
    let dir = tempfile::tempdir().expect("store dir");
    let target = dir.path().join("outside.json");
    std::fs::write(&target, b"secret sibling bytes").expect("seed target");
    let store = dir.path().join(super::BOOKMARKS_FILE);
    std::os::unix::fs::symlink(&target, &store).expect("symlink store");

    let note = super::load_bookmarks_at(&store).expect_err("symlinked store refuses");
    assert!(
        note.contains("symlink") || note.contains("defaults"),
        "honest refusal: {note}"
    );

    let b = live_posix_browser(Vec::new()).with_config_dir(dir.path());
    assert!(b.bookmarks().is_empty());
    assert!(
        b.last_note()
            .is_some_and(|n| n.contains("symlink") || n.contains("defaults")),
        "operator-visible note: {:?}",
        b.last_note()
    );
    assert_eq!(
        std::fs::read(&target).expect("target"),
        b"secret sibling bytes",
        "hostile symlink target was never rewritten"
    );
    assert!(
        std::fs::symlink_metadata(&store)
            .expect("store still exists")
            .file_type()
            .is_symlink(),
        "hydrate must not replace the hostile symlink"
    );
}

// WL-FUNC-027 — a tampered on-disk representation (invalid JSON, wrong shape,
// oversize) must be rejected without silently overwriting the operator's file.
// The failure has to stay honestly visible in `last_note` and the raw bytes
// must remain on disk so an operator can copy them out and repair them by
// hand; only a subsequent, deliberate mutation is allowed to replace the
// store contents.
#[test]
fn bookmarks_corrupt_store_is_preserved_until_the_operator_mutates() {
    let dir = tempfile::tempdir().expect("store dir");
    let store = dir.path().join(super::BOOKMARKS_FILE);
    let corrupt = br#"{"bookmarks": [not valid json"#;
    std::fs::write(&store, corrupt).expect("seed corrupt");

    let b = live_posix_browser(Vec::new()).with_config_dir(dir.path());
    assert!(
        b.bookmarks().is_empty(),
        "in-memory refuses the tampered file"
    );
    assert!(
        b.last_note()
            .is_some_and(|n| n.contains("not valid JSON") || n.contains("defaults")),
        "honest refusal in the note: {:?}",
        b.last_note()
    );
    drop(b);
    assert_eq!(
        std::fs::read(&store).expect("store"),
        corrupt,
        "no silent overwrite of the tampered on-disk representation"
    );

    // A deliberate mutation replaces the store all-or-nothing through the
    // hardened write path — the operator's next pin is what displaces the
    // corrupted bytes, not the hydrate.
    let target = dir.path().join("Docs");
    std::fs::create_dir(&target).expect("mkdir");
    let rows = vec![FileRow::local("Docs", Mime::Folder, "\u{2014}", "\u{2014}")
        .with_path(target.to_string_lossy())];
    let mut b2 = live_posix_browser(rows).with_config_dir(dir.path());
    b2.click(0, 0);
    b2.pin_focused(0);
    b2.flush_persisted();
    drop(b2);

    let reloaded = super::load_bookmarks_at(&store).expect("valid after mutation");
    assert_eq!(reloaded.bookmarks.len(), 1);
    assert_eq!(reloaded.bookmarks[0].path, target.to_string_lossy());
}

// WL-FUNC-027 — a failed bookmark write must be visible instead of making a
// pin look durable when the configured store parent cannot be created.
#[test]
fn bookmarks_report_persistence_failure_and_keep_dirty_state() {
    let dir = tempfile::tempdir().expect("config parent");
    let blocked_parent = dir.path().join("not-a-directory");
    std::fs::write(&blocked_parent, b"file blocker").expect("seed blocker");
    let target = dir.path().join("Docs");
    std::fs::create_dir(&target).expect("mkdir");
    let rows = vec![FileRow::local("Docs", Mime::Folder, "\u{2014}", "\u{2014}")
        .with_path(target.to_string_lossy())];
    let mut b = live_posix_browser(rows).with_config_dir(&blocked_parent);
    b.click(0, 0);
    b.pin_focused(0);
    b.flush_persisted();

    assert_eq!(b.bookmarks().len(), 1, "the in-memory pin remains usable");
    assert!(
        b.last_note()
            .is_some_and(|note| note.contains("Bookmarks could not be saved")),
        "persistence failure must be visible: {:?}",
        b.last_note()
    );
    assert!(
        !blocked_parent.join(super::BOOKMARKS_FILE).exists(),
        "a failed write must not claim to have created the store"
    );

    std::fs::remove_file(&blocked_parent).expect("remove blocker");
    std::fs::create_dir(&blocked_parent).expect("repair config parent");
    b.flush_persisted();
    let saved = super::load_bookmarks_at(&blocked_parent.join(super::BOOKMARKS_FILE))
        .expect("retry should persist after repair");
    assert_eq!(saved.bookmarks, b.bookmarks());
}
