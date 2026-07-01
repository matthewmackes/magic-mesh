//! The render-agnostic browser model (E12-11).
//!
//! This is the part of the Files surface that holds no egui at all: a small
//! state machine over `mde-files`' [`Backend`] trait. The egui view ([`crate::view`])
//! reads this model and turns it into widgets; everything decision-shaped —
//! which pane is focused, what the current listing is, which row is selected,
//! which peer is the Send-To destination, and whether a send is even possible —
//! lives here so it can be unit-tested without a GPU or a Wayland display.
//!
//! The reuse is deliberate (governance §6): the listing comes from
//! [`Backend::list`], the roster from [`Backend::peers`], the transfer request is
//! the canonical [`SendToRequest`], and the send itself dispatches through
//! [`Backend::send_to`] — the same surfaces the retired file-manager GUI rendered.
//! In production the backend is `RealBackend` (local FS + the mesh Bus); tests
//! drive the model with the shipped `DemoBackend`/`LocalFsBackend` and a small
//! in-test double for the states only a live mesh produces.

use std::path::PathBuf;

use mde_files::backend::{Backend, BackendError, Destination, MeshOverlayBadge, OpId};
use mde_files::model::{FileRow, Peer, SelfNode};
use mde_files::send_to::{SendToEntry, SendToRequest};

/// Which surface the browser is focused on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pane {
    /// A local directory, addressed in the backend's local-path grammar:
    /// a `local:<slug>` shortcut (`local:home`, `local:docs`, `local:downloads`,
    /// `local:root`) or an absolute `/…` path.
    Local(String),
    /// A mesh peer's shared folder, addressed by peer id.
    Peer(String),
}

impl Pane {
    /// The string this pane passes to [`Backend::list`]. Local panes pass their
    /// path straight through; a peer pane becomes the `peer:<id>` route the mesh
    /// backend recognises.
    #[must_use]
    pub fn backend_path(&self) -> String {
        match self {
            Self::Local(path) => path.clone(),
            Self::Peer(id) => format!("peer:{id}"),
        }
    }

    /// `true` when this pane is browsing the local filesystem.
    #[must_use]
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    /// `true` when this pane is browsing a mesh peer's folder.
    #[must_use]
    pub fn is_peer(&self) -> bool {
        matches!(self, Self::Peer(_))
    }
}

/// A local-filesystem navigation shortcut shown in the sidebar. The `path` is a
/// real backend route, not a stand-in — clicking one lists whatever is actually
/// there (an honest empty listing when the directory is absent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalSpot {
    /// Sidebar label.
    pub label: &'static str,
    /// Backend `list()` path.
    pub path: &'static str,
}

/// The fixed set of local nav shortcuts. Each maps onto a `LocalFsBackend` slug.
pub const LOCAL_SPOTS: &[LocalSpot] = &[
    LocalSpot {
        label: "Home",
        path: "local:home",
    },
    LocalSpot {
        label: "Documents",
        path: "local:docs",
    },
    LocalSpot {
        label: "Downloads",
        path: "local:downloads",
    },
    LocalSpot {
        label: "Filesystem",
        path: "local:root",
    },
];

/// Outcome of the most recent Send-To, surfaced in the status line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SendOutcome {
    /// Nothing sent yet this session.
    #[default]
    Idle,
    /// The backend accepted the transfer and returned this op id.
    Sent {
        /// The op id mackesd (or the demo/local backend) assigned.
        op_id: OpId,
        /// The file that was sent (for the status line).
        file: String,
        /// The destination peer id.
        peer: String,
    },
    /// The backend rejected the transfer; carries the error text.
    Failed(String),
}

/// The whole render-agnostic state of the Files surface.
pub struct FileBrowser {
    /// The data + transfer surface. `RealBackend` in production; a shipped
    /// `DemoBackend`/`LocalFsBackend` (or an in-test double) under test.
    backend: Box<dyn Backend>,
    /// Cached self-identity (sidebar header).
    self_node: SelfNode,
    /// Cached peer roster (sidebar + destination picker).
    peers: Vec<Peer>,
    /// The focused pane.
    pane: Pane,
    /// The listing for `pane`, refreshed on every navigation.
    rows: Vec<FileRow>,
    /// Index into `rows` of the selected row, if any.
    selected: Option<usize>,
    /// The chosen Send-To destination (a peer id).
    destination: Option<String>,
    /// The last Send-To outcome.
    last_send: SendOutcome,
    /// Cached live Nebula overlay badge — the mesh id, active transport, and
    /// lighthouse role this node is running on — or `None` when the node is
    /// standalone or the mesh daemon isn't reachable. Refreshed with the roster
    /// (it comes from the same reconnect probe), never fabricated.
    mesh_overlay: Option<MeshOverlayBadge>,
}

impl FileBrowser {
    /// The pane the surface opens on: the local home directory — the natural
    /// source surface for a Send-To.
    pub const HOME: &'static str = "local:home";

    /// Build a browser over `backend`, eagerly loading the roster + the opening
    /// listing so the surface has content on its first frame.
    #[must_use]
    pub fn new(backend: Box<dyn Backend>) -> Self {
        let mut me = Self {
            backend,
            self_node: SelfNode::default(),
            peers: Vec::new(),
            pane: Pane::Local(Self::HOME.to_string()),
            rows: Vec::new(),
            selected: None,
            destination: None,
            last_send: SendOutcome::Idle,
            mesh_overlay: None,
        };
        me.refresh_roster();
        me.reload();
        me
    }

    // ── Roster / identity ───────────────────────────────────────────────────

    /// Re-probe the mesh (cheap + idempotent on `RealBackend`) and refresh the
    /// cached identity + roster. Drops a destination that is no longer reachable
    /// so the Send-To action can never point at a vanished peer.
    pub fn refresh_roster(&mut self) {
        self.backend.reconnect();
        self.self_node = self.backend.self_node();
        self.peers = self.backend.peers();
        self.mesh_overlay = self.backend.mesh_overlay();
        if !self.destination_reachable() {
            self.destination = None;
        }
    }

    /// This node's identity.
    #[must_use]
    pub fn self_node(&self) -> &SelfNode {
        &self.self_node
    }

    /// The full peer roster (reachable and not).
    #[must_use]
    pub fn peers(&self) -> &[Peer] {
        &self.peers
    }

    /// The live Nebula overlay badge (mesh id, active transport, lighthouse role)
    /// when this node is on a mesh; `None` when it is standalone or the mesh
    /// daemon isn't reachable — the surface renders that as an honest "no mesh"
    /// state rather than a fabricated one. Refreshed with the roster.
    #[must_use]
    pub fn mesh_overlay(&self) -> Option<&MeshOverlayBadge> {
        self.mesh_overlay.as_ref()
    }

    /// The peers that can receive a Send-To right now — online, idle, or self
    /// (per [`PeerStatus::is_reachable`]). Offline peers are excluded.
    #[must_use]
    pub fn reachable_destinations(&self) -> Vec<&Peer> {
        self.peers
            .iter()
            .filter(|p| p.status.is_reachable())
            .collect()
    }

    // ── Navigation ──────────────────────────────────────────────────────────

    /// The focused pane.
    #[must_use]
    pub fn pane(&self) -> &Pane {
        &self.pane
    }

    /// The current listing.
    #[must_use]
    pub fn rows(&self) -> &[FileRow] {
        &self.rows
    }

    /// Re-fetch the listing for the focused pane and drop any stale selection.
    pub fn reload(&mut self) {
        self.rows = self.backend.list(&self.pane.backend_path());
        self.selected = None;
    }

    /// Focus a local directory and load it.
    pub fn open_local(&mut self, path: impl Into<String>) {
        self.pane = Pane::Local(path.into());
        self.reload();
    }

    /// Focus a mesh peer's folder and load it.
    pub fn open_peer(&mut self, peer_id: impl Into<String>) {
        self.pane = Pane::Peer(peer_id.into());
        self.reload();
    }

    // ── Selection ───────────────────────────────────────────────────────────

    /// The selected row index, if any.
    #[must_use]
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Select the row at `idx`. Out-of-range indices are ignored so the view
    /// can pass a click index without bounds-checking.
    pub fn select(&mut self, idx: usize) {
        if idx < self.rows.len() {
            self.selected = Some(idx);
        }
    }

    /// Clear the selection.
    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    /// The selected row, if any.
    #[must_use]
    pub fn selected_row(&self) -> Option<&FileRow> {
        self.selected.and_then(|i| self.rows.get(i))
    }

    // ── Destination ─────────────────────────────────────────────────────────

    /// The chosen destination peer id, if any.
    #[must_use]
    pub fn destination(&self) -> Option<&str> {
        self.destination.as_deref()
    }

    /// Choose `peer_id` as the Send-To destination. (Reachability is enforced at
    /// send time, not here, so the view can reflect the click immediately.)
    pub fn set_destination(&mut self, peer_id: impl Into<String>) {
        self.destination = Some(peer_id.into());
    }

    /// `true` when a destination is set and that peer is currently reachable.
    #[must_use]
    fn destination_reachable(&self) -> bool {
        match &self.destination {
            Some(id) => self
                .peers
                .iter()
                .any(|p| &p.id == id && p.status.is_reachable()),
            None => false,
        }
    }

    // ── Send-To ─────────────────────────────────────────────────────────────

    /// The absolute path of the selected row when it is a real local file that
    /// can be a Send-To source. Directory rows and virtual mesh/peer rows (which
    /// carry no `path`) are not sendable in this slice, so they return `None`.
    #[must_use]
    pub fn send_source(&self) -> Option<PathBuf> {
        let row = self.selected_row()?;
        if row.is_dir() {
            return None;
        }
        row.path.as_ref().map(PathBuf::from)
    }

    /// Build the canonical [`SendToRequest`] for the current selection +
    /// destination, or `None` when the action is unavailable (no sendable
    /// source, no destination, or an unreachable destination). The request is a
    /// Copy with Ask-on-conflict, attributed to the toolbar entry point.
    #[must_use]
    pub fn plan_send(&self) -> Option<SendToRequest> {
        let source = self.send_source()?;
        let dest = self.destination.clone()?;
        if !self.destination_reachable() {
            return None;
        }
        Some(SendToRequest::copy_ask(
            vec![source],
            Destination::Peer(dest),
            SendToEntry::Toolbar,
        ))
    }

    /// Whether a Send-To can fire right now (drives the primary button's enabled
    /// state). Exactly [`plan_send`](Self::plan_send) being `Some`.
    #[must_use]
    pub fn can_send(&self) -> bool {
        self.plan_send().is_some()
    }

    /// Dispatch a prepared request through the backend's transfer surface. This
    /// is the unguarded forward (it sends whatever request it is handed); the
    /// guarded entry point is [`send`](Self::send).
    ///
    /// # Errors
    /// Propagates the backend's [`BackendError`] — e.g.
    /// `DestinationUnreachable` when the active backend has no mesh route.
    pub fn dispatch(&mut self, req: SendToRequest) -> Result<OpId, BackendError> {
        self.backend
            .send_to(&req.sources, req.destination, req.mode, req.conflict)
    }

    /// Plan and dispatch the Send-To for the current selection, recording the
    /// outcome for the status line. Returns `None` when the action is
    /// unavailable (nothing planned), otherwise the backend's result.
    pub fn send(&mut self) -> Option<Result<OpId, BackendError>> {
        let req = self.plan_send()?;
        let file = self
            .selected_row()
            .map(|r| r.name.clone())
            .unwrap_or_default();
        let peer = self.destination().unwrap_or_default().to_string();
        let result = self.dispatch(req);
        self.last_send = match &result {
            Ok(op_id) => SendOutcome::Sent {
                op_id: *op_id,
                file,
                peer,
            },
            Err(e) => SendOutcome::Failed(e.to_string()),
        };
        Some(result)
    }

    /// The most recent Send-To outcome.
    #[must_use]
    pub fn last_send(&self) -> &SendOutcome {
        &self.last_send
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_files::backend::LocalFsBackend;
    use mde_files::backend::{AuditEntry, ConflictPolicy, DemoBackend, SendMode};
    use mde_files::model::{Mime, PeerKind, PeerStatus};

    // ── In-test backend double ──────────────────────────────────────────────
    //
    // Production's `RealBackend` is the only shipped backend that exposes BOTH a
    // local listing with real file paths AND a peer roster at once — and it needs
    // a live mesh Bus, which a unit test has no business standing up. This double
    // reproduces just that shape (a configurable roster + a path-bearing local
    // listing) so the plan/guard/dispatch state machine can be exercised
    // offline. It is `#[cfg(test)]` only — never shipped (§7).
    struct FixtureBackend {
        peers: Vec<Peer>,
        rows: Vec<FileRow>,
        next_op: OpId,
        mesh: Option<MeshOverlayBadge>,
    }

    impl FixtureBackend {
        fn new(peers: Vec<Peer>, rows: Vec<FileRow>) -> Self {
            Self {
                peers,
                rows,
                next_op: 1,
                mesh: None,
            }
        }

        /// Give this fixture a live Nebula overlay, the way `RealBackend` reports
        /// one when `mackesd`'s Nebula.Status responder is up.
        fn with_mesh(mut self, mesh: MeshOverlayBadge) -> Self {
            self.mesh = Some(mesh);
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
        fn list(&self, _path: &str) -> Vec<FileRow> {
            self.rows.clone()
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

    /// A fixture node: one local file (real path) + an online and an offline
    /// peer — the production `RealBackend` shape, minus the live Bus.
    fn fixture_browser() -> FileBrowser {
        let rows = vec![FileRow::local("notes.md", Mime::Doc, "1 KB", "now")
            .with_path("/tmp/mde-files-egui/notes.md")];
        let peers = vec![
            peer("pine", PeerStatus::Online),
            peer("cedar", PeerStatus::Offline),
        ];
        FileBrowser::new(Box::new(FixtureBackend::new(peers, rows)))
    }

    // ── Pane → backend path ─────────────────────────────────────────────────

    #[test]
    fn pane_maps_to_the_backend_list_path() {
        assert_eq!(
            Pane::Local("local:docs".into()).backend_path(),
            "local:docs"
        );
        assert_eq!(Pane::Local("/etc".into()).backend_path(), "/etc");
        assert_eq!(Pane::Peer("pine".into()).backend_path(), "peer:pine");
        assert!(Pane::Local(String::new()).is_local());
        assert!(Pane::Peer("pine".into()).is_peer());
    }

    #[test]
    fn local_spots_are_real_backend_routes() {
        // Every shortcut is a `local:` route (never a peer route), so the
        // sidebar can't accidentally send the user to the mesh.
        assert!(!LOCAL_SPOTS.is_empty());
        for spot in LOCAL_SPOTS {
            assert!(spot.path.starts_with("local:"), "{spot:?} is not local");
            assert!(!spot.label.is_empty());
        }
    }

    // ── Listing reuse (Backend::list) ───────────────────────────────────────

    #[test]
    fn open_peer_surfaces_the_backend_listing() {
        // DemoBackend ships a curated per-peer listing; the model surfaces it
        // verbatim through the same `list()` every consumer uses.
        let mut b = FileBrowser::new(Box::new(DemoBackend::new()));
        b.open_peer("pine");
        assert!(b.pane().is_peer());
        assert_eq!(b.rows().len(), mde_files::demo_data::pine_files().len());
        assert!(!b.rows().is_empty());
    }

    #[test]
    fn open_unknown_peer_is_an_honest_empty_listing() {
        let mut b = FileBrowser::new(Box::new(DemoBackend::new()));
        b.open_peer("ghost");
        assert!(b.rows().is_empty(), "unknown peer must not fabricate rows");
        assert!(b.selected_row().is_none());
    }

    #[test]
    fn open_local_lists_a_real_directory_with_paths() {
        // A real temp dir through the shipped LocalFsBackend: the rows carry
        // absolute paths (the Send-To source).
        let dir = std::env::temp_dir().join(format!("mde-files-egui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let file = dir.join("hello.txt");
        std::fs::write(&file, b"hi").expect("write temp file");

        let mut b = FileBrowser::new(Box::new(LocalFsBackend::new()));
        b.open_local(dir.to_string_lossy().into_owned());
        assert!(b.pane().is_local());
        let row = b
            .rows()
            .iter()
            .find(|r| r.name == "hello.txt")
            .expect("temp file is listed");
        assert!(row.path.is_some(), "local rows must carry an absolute path");

        std::fs::remove_dir_all(&dir).ok();
    }

    // ── Roster / reachability ───────────────────────────────────────────────

    #[test]
    fn reachable_destinations_excludes_offline_peers() {
        // DemoBackend: pine+birch Online, oak Idle, cedar Offline → 3 reachable.
        let b = FileBrowser::new(Box::new(DemoBackend::new()));
        let reachable = b.reachable_destinations();
        assert_eq!(reachable.len(), 3);
        assert!(reachable.iter().all(|p| p.status.is_reachable()));
        assert!(
            !reachable.iter().any(|p| p.id == "cedar"),
            "the offline peer must not be a destination"
        );
    }

    // ── Mesh overlay (live Nebula badge) ────────────────────────────────────

    #[test]
    fn mesh_overlay_is_none_without_a_live_mesh() {
        // The demo/local/fixture backends have no Nebula source, so the model
        // surfaces an honest `None` — the view draws a "standalone" state, never
        // a fabricated mesh badge.
        assert!(FileBrowser::new(Box::new(DemoBackend::new()))
            .mesh_overlay()
            .is_none());
        assert!(fixture_browser().mesh_overlay().is_none());
    }

    #[test]
    fn mesh_overlay_surfaces_the_live_badge_and_refreshes_with_the_roster() {
        let badge = MeshOverlayBadge {
            is_lighthouse: true,
            ca_epoch: 7,
            mesh_id: "quasar".into(),
            peer_count: 3,
            active_transport: "udp".into(),
        };
        let mut b = FileBrowser::new(Box::new(
            FixtureBackend::new(Vec::new(), Vec::new()).with_mesh(badge.clone()),
        ));
        // Populated on construction (new() refreshes the roster + overlay)…
        assert_eq!(b.mesh_overlay(), Some(&badge));
        // …and kept fresh on an explicit refresh (same reconnect probe).
        b.refresh_roster();
        assert_eq!(b.mesh_overlay().map(|m| m.mesh_id.as_str()), Some("quasar"));
        assert_eq!(b.mesh_overlay().map(|m| m.is_lighthouse), Some(true));
    }

    // ── Selection ───────────────────────────────────────────────────────────

    #[test]
    fn selection_round_trips_and_ignores_out_of_range() {
        let mut b = FileBrowser::new(Box::new(DemoBackend::new()));
        b.open_peer("pine");
        assert!(b.selected_row().is_none());
        b.select(0);
        assert_eq!(b.selected(), Some(0));
        assert!(b.selected_row().is_some());
        b.select(9_999); // out of range → ignored, keeps the prior selection
        assert_eq!(b.selected(), Some(0));
        b.clear_selection();
        assert!(b.selected_row().is_none());
    }

    #[test]
    fn navigation_drops_a_stale_selection() {
        let mut b = FileBrowser::new(Box::new(DemoBackend::new()));
        b.open_peer("pine");
        b.select(0);
        assert!(b.selected().is_some());
        b.open_peer("birch"); // re-listing must clear the old index
        assert!(b.selected().is_none());
    }

    // ── Send-To planning + guards ───────────────────────────────────────────

    #[test]
    fn plan_send_needs_a_source_a_destination_and_reachability() {
        let mut b = fixture_browser();
        b.select(0); // the local file (has a path)

        // No destination yet.
        assert!(b.plan_send().is_none());
        assert!(!b.can_send());

        // Offline destination → still blocked (reachability guard).
        b.set_destination("cedar");
        assert!(b.plan_send().is_none());
        assert!(!b.can_send());

        // Online destination → now sendable, as a Copy/Ask/Toolbar request.
        b.set_destination("pine");
        let req = b
            .plan_send()
            .expect("a local file to a reachable peer is sendable");
        assert!(b.can_send());
        assert_eq!(req.mode, SendMode::Copy);
        assert_eq!(req.conflict, ConflictPolicy::Ask);
        assert_eq!(req.entry, SendToEntry::Toolbar);
        assert_eq!(req.destination, Destination::Peer("pine".into()));
        assert_eq!(
            req.sources,
            vec![PathBuf::from("/tmp/mde-files-egui/notes.md")]
        );
    }

    #[test]
    fn peer_rows_are_not_a_send_source() {
        // DemoBackend peer rows are virtual (no path); even with a reachable
        // destination chosen, the path guard blocks the send.
        let mut b = FileBrowser::new(Box::new(DemoBackend::new()));
        b.open_peer("pine");
        b.select(0);
        b.set_destination("birch"); // reachable
        assert!(b.send_source().is_none());
        assert!(b.plan_send().is_none());
    }

    // ── Send-To dispatch (real shipped backends) ────────────────────────────

    #[test]
    fn dispatch_drives_a_real_backend_and_returns_op_ids() {
        // DemoBackend is a shipped backend: dispatching a real request records an
        // audit row and returns an increasing op id.
        let mut b = FileBrowser::new(Box::new(DemoBackend::new()));
        let req = SendToRequest::copy_ask(
            vec![PathBuf::from("/tmp/x")],
            Destination::Peer("pine".into()),
            SendToEntry::Toolbar,
        );
        assert_eq!(b.dispatch(req.clone()).expect("first send"), 1);
        assert_eq!(b.dispatch(req).expect("second send"), 2);
    }

    #[test]
    fn dispatch_over_a_local_only_backend_is_honestly_unreachable() {
        // LocalFsBackend has no mesh: a peer destination is genuinely
        // unreachable. The model forwards that honest error, never faking a send.
        let mut b = FileBrowser::new(Box::new(LocalFsBackend::new()));
        let req = SendToRequest::copy_ask(
            vec![PathBuf::from("/tmp/x")],
            Destination::Peer("pine".into()),
            SendToEntry::Toolbar,
        );
        assert!(matches!(
            b.dispatch(req),
            Err(BackendError::DestinationUnreachable(_))
        ));
    }

    #[test]
    fn send_plans_dispatches_and_records_the_outcome() {
        let mut b = fixture_browser();
        b.select(0);
        b.set_destination("pine");

        assert_eq!(*b.last_send(), SendOutcome::Idle);
        let result = b.send().expect("a planned send fires");
        assert!(result.is_ok());
        match b.last_send() {
            SendOutcome::Sent { file, peer, .. } => {
                assert_eq!(file, "notes.md");
                assert_eq!(peer, "pine");
            }
            other => panic!("expected Sent, got {other:?}"),
        }
    }

    #[test]
    fn send_is_a_no_op_when_nothing_is_planned() {
        let mut b = fixture_browser();
        // No selection, no destination → nothing to send.
        assert!(b.send().is_none());
        assert_eq!(*b.last_send(), SendOutcome::Idle);
    }
}
