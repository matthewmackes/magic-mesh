//! The render-agnostic browser model (FILEMGR-8, née E12-11).
//!
//! This is the whole decision layer of the Files surface with no egui in it: a
//! state machine over `mde-files`' [`Backend`] trait plus the FILEMGR-2 op
//! queue. The egui view ([`crate::view`]) reads this model and turns it into
//! widgets; everything decision-shaped — which pane and tab are focused, the
//! current listing, the view mode and sort order (remembered per folder), the
//! show-hidden filter, the multi-row selection, the back/forward history, and
//! what a drag-and-drop drop *means* (a copy or a move of which paths into which
//! directory) — lives here so it can be unit-tested without a GPU.
//!
//! Reuse is deliberate (governance §6): listings come from [`Backend::list`],
//! the roster from [`Backend::peers`], every actual file mutation runs through
//! the shipped FILEMGR-2 [`crate::ops::Ops`] queue (which itself drives the
//! FILEMGR-1 `FileOps` trait) — the surface never re-implements a file op. In
//! production the backend is `RealBackend` (local FS + the mesh Bus) and the
//! queue runs over `LiveFileOps`; tests drive both with in-memory fakes.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mde_files::backend::{Backend, BackendError, Destination, MeshOverlayBadge, OpId};
use mde_files::fileops::{FileOps, LiveFileOps};
use mde_files::model::{FileRow, Mime, Peer, SelfNode};
use mde_files::opqueue::OpKind;
use mde_files::send_to::{SendToEntry, SendToRequest};

use crate::mesh_mount::{BusMeshMount, MeshMountClient, MeshMountVerb, MountView};
use crate::ops::Ops;
use crate::preview::{PreviewState, Previews, ThumbState};

/// How often the Mesh sidebar re-reads `state/mesh-mount/*` from the Bus. The read
/// is a cheap local spool scan; a worker transition surfaces within this window.
/// Matches the other Bus surfaces' cadence.
const MOUNT_POLL: Duration = Duration::from_secs(2);

/// The short mount hostname for a peer — the `<host>` verb slot.
///
/// The FILEMGR-5 worker keys `action/mesh-mount/<host>` + `state/mesh-mount/<host>`
/// on this. Both roster sources (`WirePeer` / Nebula) carry the short name in
/// `label`; `id` (a `peer:<node>` or bare name) is the honest fallback when a
/// label is absent.
#[must_use]
pub fn mount_host_of(peer: &Peer) -> &str {
    if peer.label.is_empty() {
        peer.id.as_str()
    } else {
        peer.label.as_str()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Where a pane is pointed.
// ═══════════════════════════════════════════════════════════════════════════

/// The location a pane/tab is browsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// A local directory, in the backend's local-path grammar: a `local:<slug>`
    /// shortcut (`local:home`, `local:docs`, …) or an absolute `/…` path.
    Local(String),
    /// A mesh peer's shared folder, addressed by peer id.
    Peer(String),
}

impl Location {
    /// The string this location passes to [`Backend::list`]. Local locations
    /// pass their path straight through; a peer becomes the `peer:<id>` route.
    #[must_use]
    pub fn backend_path(&self) -> String {
        match self {
            Self::Local(path) => path.clone(),
            Self::Peer(id) => format!("peer:{id}"),
        }
    }

    /// `true` when this location is on the local filesystem.
    #[must_use]
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    /// `true` when this location is a mesh peer's folder.
    #[must_use]
    pub fn is_peer(&self) -> bool {
        matches!(self, Self::Peer(_))
    }

    /// The absolute path when this location is an absolute local path (the
    /// common case after descending into a folder row, whose paths are
    /// absolute). `None` for a `local:` shortcut slug or a peer.
    #[must_use]
    pub fn abs_path(&self) -> Option<PathBuf> {
        match self {
            Self::Local(p) if p.starts_with('/') => Some(PathBuf::from(p)),
            _ => None,
        }
    }

    /// The parent location for the "up" action — only meaningful for an absolute
    /// local path. A shortcut slug or a peer has no navigable parent here.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        let abs = self.abs_path()?;
        abs.parent()
            .map(|p| Self::Local(p.to_string_lossy().into_owned()))
    }

    /// The clickable breadcrumb trail for this location, oldest ancestor first.
    /// Absolute local paths decompose into per-segment crumbs (each navigable);
    /// a shortcut slug or a peer is a single crumb.
    #[must_use]
    pub fn crumbs(&self) -> Vec<Crumb> {
        match self {
            Self::Local(p) if p.starts_with('/') => {
                let mut out = vec![Crumb {
                    label: "/".to_string(),
                    location: Self::Local("/".to_string()),
                }];
                let mut acc = PathBuf::from("/");
                for seg in Path::new(p).components().filter_map(|c| match c {
                    std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                    _ => None,
                }) {
                    acc.push(&seg);
                    out.push(Crumb {
                        label: seg,
                        location: Self::Local(acc.to_string_lossy().into_owned()),
                    });
                }
                out
            }
            Self::Local(slug) => vec![Crumb {
                label: slug.strip_prefix("local:").unwrap_or(slug).to_string(),
                location: self.clone(),
            }],
            Self::Peer(id) => vec![
                Crumb {
                    label: "Mesh".to_string(),
                    location: self.clone(),
                },
                Crumb {
                    label: id.clone(),
                    location: self.clone(),
                },
            ],
        }
    }
}

/// One breadcrumb: a label and the location clicking it navigates to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    /// The visible segment label.
    pub label: String,
    /// The location this crumb navigates the pane to.
    pub location: Location,
}

// ═══════════════════════════════════════════════════════════════════════════
// View mode + sort + per-folder memory.
// ═══════════════════════════════════════════════════════════════════════════

/// How a listing is laid out (lock 20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// One row per entry, type tag + name (the compact default).
    #[default]
    List,
    /// A wrapped grid of tiles (the icons view).
    Grid,
    /// A columned table: name · size · type · modified, with sortable headers.
    Details,
}

impl ViewMode {
    /// The three view modes, in toolbar order.
    pub const ALL: [Self; 3] = [Self::List, Self::Grid, Self::Details];

    /// The toolbar label for this mode.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::Grid => "Grid",
            Self::Details => "Details",
        }
    }
}

/// The column a listing is sorted on (lock 20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    /// Alphabetical by name.
    #[default]
    Name,
    /// By size (parsed from the displayed size; folders group via dirs-first).
    Size,
    /// By MIME class.
    Kind,
    /// By modified age (parsed from the displayed age; newest first ascending).
    Modified,
}

impl SortKey {
    /// The four sort keys, in Details-header order.
    pub const ALL: [Self; 4] = [Self::Name, Self::Size, Self::Kind, Self::Modified];

    /// The column header label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Size => "Size",
            Self::Kind => "Type",
            Self::Modified => "Modified",
        }
    }
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortDir {
    /// Ascending (A→Z, small→large, newest→oldest by age).
    #[default]
    Asc,
    /// Descending.
    Desc,
}

impl SortDir {
    /// The opposite direction (a header re-click toggles this).
    #[must_use]
    pub const fn flip(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }

    /// A small caret glyph for the active sort column.
    #[must_use]
    pub const fn caret(self) -> &'static str {
        match self {
            Self::Asc => "\u{2191}",
            Self::Desc => "\u{2193}",
        }
    }
}

/// A folder's sort key + direction + dirs-first grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortSpec {
    /// The column sorted on.
    pub key: SortKey,
    /// Ascending or descending.
    pub dir: SortDir,
    /// Keep directories grouped ahead of files regardless of direction.
    pub dirs_first: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        Self {
            key: SortKey::Name,
            dir: SortDir::Asc,
            dirs_first: true,
        }
    }
}

/// The remembered per-folder presentation (lock 20 — "view+sort persist
/// per-folder"): view mode, sort order, and the show-hidden toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FolderPrefs {
    /// The last view mode used in this folder.
    pub view: ViewMode,
    /// The last sort order used in this folder.
    pub sort: SortSpec,
    /// Whether hidden (dot) entries were shown in this folder.
    pub show_hidden: bool,
}

// ── sort / filter helpers ────────────────────────────────────────────────────

/// A dot-file (hidden) name. The listing display adds a trailing `/` to
/// directories, never a leading dot, so a simple prefix test is correct.
fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// The MIME sort rank for the "Type" column.
const fn mime_rank(mime: Mime) -> u8 {
    match mime {
        Mime::Folder => 0,
        Mime::Doc => 1,
        Mime::Image => 2,
        Mime::Pdf => 3,
        Mime::Archive => 4,
        Mime::Disk => 5,
    }
}

/// Parse a human size string (as FILEMGR-1's `fmt_bytes` renders it — `"512 B"`,
/// `"2 KB"`, `"5.0 MB"`, `"3.0 GB"`) back to an approximate byte count, purely as
/// a monotonic *sort key*. A folder summary (`"— · 122 items"`) or an unknown
/// shape sorts as zero — directories are grouped by dirs-first, so their order
/// falls back to the name tie-break. This is honest ordering of the value the
/// user actually sees, not a fabricated exact size.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn parse_size_bytes(s: &str) -> u64 {
    let s = s.trim();
    if s.starts_with('\u{2014}') {
        return 0; // "— · N items" — a folder summary, not a byte size.
    }
    let mut num = String::new();
    let mut unit = "";
    for (i, ch) in s.char_indices() {
        if ch.is_ascii_digit() || ch == '.' {
            num.push(ch);
        } else {
            unit = s[i..].trim();
            break;
        }
    }
    let value: f64 = num.parse().unwrap_or(0.0);
    let mult = match unit.split_whitespace().next().unwrap_or("") {
        "GB" => 1024.0_f64.powi(3),
        "MB" => 1024.0_f64.powi(2),
        "KB" => 1024.0_f64,
        _ => 1.0,
    };
    (value * mult) as u64
}

/// Parse a human age string (as FILEMGR-1's `fmt_age` renders it — `"4 min"`,
/// `"2 h"`, `"3 d"`, `"now"`) back to an approximate "seconds since modified"
/// sort key: smaller = newer. An empty/`"—"` age sorts last (unknown). Purely
/// for ordering the value the user sees.
fn parse_age_secs(s: &str) -> u64 {
    let s = s.trim();
    if s.is_empty() || s == "\u{2014}" {
        return u64::MAX;
    }
    if s.eq_ignore_ascii_case("now") {
        return 0;
    }
    let mut it = s.split_whitespace();
    let n: u64 = it.next().and_then(|t| t.parse().ok()).unwrap_or(0);
    let mult = match it.next().unwrap_or("") {
        "min" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "mo" => 30 * 86_400,
        "y" => 365 * 86_400,
        // "s" (seconds) and any unrecognised unit are 1 second per count.
        _ => 1,
    };
    n.saturating_mul(mult)
}

fn cmp_name(a: &FileRow, b: &FileRow) -> Ordering {
    a.name.to_lowercase().cmp(&b.name.to_lowercase())
}

/// Sort `rows` in place per `spec`. Directories stay grouped ahead of files when
/// `dirs_first` (independent of direction — the desktop convention); within a
/// group the chosen key orders, with name as the stable tie-break.
fn sort_rows(rows: &mut [FileRow], spec: SortSpec) {
    rows.sort_by(|a, b| {
        if spec.dirs_first {
            match (a.is_dir(), b.is_dir()) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                _ => {}
            }
        }
        let primary = match spec.key {
            SortKey::Name => cmp_name(a, b),
            SortKey::Kind => mime_rank(a.mime)
                .cmp(&mime_rank(b.mime))
                .then_with(|| cmp_name(a, b)),
            SortKey::Size => parse_size_bytes(&a.size)
                .cmp(&parse_size_bytes(&b.size))
                .then_with(|| cmp_name(a, b)),
            SortKey::Modified => parse_age_secs(&a.age)
                .cmp(&parse_age_secs(&b.age))
                .then_with(|| cmp_name(a, b)),
        };
        match spec.dir {
            SortDir::Asc => primary,
            SortDir::Desc => primary.reverse(),
        }
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// Sidebar shortcuts + Send-To outcome (carried over from E12-11).
// ═══════════════════════════════════════════════════════════════════════════

/// A local-filesystem navigation shortcut shown in the sidebar. The `path` is a
/// real backend route — clicking one lists whatever is actually there.
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
        /// The op id mackesd (or the local backend) assigned.
        op_id: OpId,
        /// The file that was sent (for the status line).
        file: String,
        /// The destination peer id.
        peer: String,
    },
    /// The backend rejected the transfer; carries the error text.
    Failed(String),
}

// ═══════════════════════════════════════════════════════════════════════════
// A tab: one pane's full navigation + view + selection state.
// ═══════════════════════════════════════════════════════════════════════════

/// One browser tab's full state.
///
/// Where it's pointed, the listing it's showing (already filtered + sorted for
/// display), the multi-row selection, the per-folder presentation, and its own
/// back/forward history + editable path buffer.
pub struct Tab {
    location: Location,
    /// The raw backend listing (unfiltered/unsorted) — kept so a hidden-toggle
    /// or a re-sort re-derives the display without another backend round-trip.
    all_rows: Vec<FileRow>,
    /// The rows as displayed (filtered + sorted). Selection indexes into this.
    rows: Vec<FileRow>,
    selection: BTreeSet<usize>,
    anchor: Option<usize>,
    view: ViewMode,
    sort: SortSpec,
    show_hidden: bool,
    back: Vec<Location>,
    forward: Vec<Location>,
    path_edit: String,
}

impl Tab {
    fn new(location: Location) -> Self {
        let path_edit = location.backend_path();
        Self {
            location,
            all_rows: Vec::new(),
            rows: Vec::new(),
            selection: BTreeSet::new(),
            anchor: None,
            view: ViewMode::default(),
            sort: SortSpec::default(),
            show_hidden: false,
            back: Vec::new(),
            forward: Vec::new(),
            path_edit,
        }
    }

    /// Re-derive [`rows`](Self::rows) from `all_rows` under the current filter +
    /// sort, dropping the now-invalid selection.
    fn recompute(&mut self) {
        let mut rows: Vec<FileRow> = self
            .all_rows
            .iter()
            .filter(|r| self.show_hidden || !is_hidden(&r.name))
            .cloned()
            .collect();
        sort_rows(&mut rows, self.sort);
        self.rows = rows;
        self.selection.clear();
        self.anchor = None;
    }

    // ── read side (the view consumes these) ─────────────────────────────────

    /// Where this tab is pointed.
    #[must_use]
    pub fn location(&self) -> &Location {
        &self.location
    }

    /// The displayed listing (filtered + sorted).
    #[must_use]
    pub fn rows(&self) -> &[FileRow] {
        &self.rows
    }

    /// The current view mode.
    #[must_use]
    pub fn view(&self) -> ViewMode {
        self.view
    }

    /// The current sort order.
    #[must_use]
    pub fn sort(&self) -> SortSpec {
        self.sort
    }

    /// Whether hidden entries are shown.
    #[must_use]
    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// The selected row indices (into [`rows`](Self::rows)).
    #[must_use]
    pub fn selection(&self) -> &BTreeSet<usize> {
        &self.selection
    }

    /// `true` when row `idx` is selected.
    #[must_use]
    pub fn is_selected(&self, idx: usize) -> bool {
        self.selection.contains(&idx)
    }

    /// The editable path-bar buffer.
    #[must_use]
    pub fn path_edit(&self) -> &str {
        &self.path_edit
    }

    /// `true` when there is somewhere to go back to.
    #[must_use]
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// `true` when there is somewhere to go forward to.
    #[must_use]
    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// A short strip title: the last path segment of a local dir, else the
    /// location's own short label.
    #[must_use]
    pub fn title(&self) -> String {
        match &self.location {
            Location::Local(p) => {
                if let Some(slug) = p.strip_prefix("local:") {
                    return slug.to_string();
                }
                Path::new(p)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("/")
                    .to_string()
            }
            Location::Peer(id) => id.clone(),
        }
    }

    /// The absolute directory this tab is browsing, for a drop-onto-background
    /// target: the location itself when absolute, else derived from a loaded
    /// row's parent. `None` for a peer folder or an unresolved empty shortcut.
    #[must_use]
    pub fn current_dir(&self) -> Option<PathBuf> {
        if let Some(p) = self.location.abs_path() {
            return Some(p);
        }
        self.rows
            .iter()
            .find_map(|r| r.path.clone())
            .and_then(|p| PathBuf::from(p).parent().map(Path::to_path_buf))
    }

    /// The absolute paths of the currently-selected rows that carry one (local
    /// rows). Peer/virtual rows have no path and are silently excluded — they
    /// can't be a filesystem-op source (that's the mesh-mount path, FILEMGR-9).
    #[must_use]
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selection
            .iter()
            .filter_map(|&i| self.rows.get(i))
            .filter_map(|r| r.path.as_ref())
            .map(PathBuf::from)
            .collect()
    }

    // ── selection state machine ─────────────────────────────────────────────

    fn click(&mut self, idx: usize) {
        if idx >= self.rows.len() {
            return;
        }
        self.selection.clear();
        self.selection.insert(idx);
        self.anchor = Some(idx);
    }

    fn ctrl_click(&mut self, idx: usize) {
        if idx >= self.rows.len() {
            return;
        }
        if !self.selection.remove(&idx) {
            self.selection.insert(idx);
        }
        self.anchor = Some(idx);
    }

    fn shift_click(&mut self, idx: usize) {
        if idx >= self.rows.len() {
            return;
        }
        let anchor = self.anchor.unwrap_or(idx);
        let (lo, hi) = (anchor.min(idx), anchor.max(idx));
        self.selection = (lo..=hi).filter(|i| *i < self.rows.len()).collect();
        // The anchor stays put so a further shift-click re-ranges from it.
    }

    fn select_all(&mut self) {
        self.selection = (0..self.rows.len()).collect();
        self.anchor = self.rows.len().checked_sub(1).map(|_| 0);
    }

    fn clear_selection(&mut self) {
        self.selection.clear();
        self.anchor = None;
    }

    /// Replace the selection with the rubber-band's covered set (the view
    /// computes it from row geometry; the model just stores the result).
    fn set_rubber(&mut self, covered: BTreeSet<usize>) {
        self.anchor = covered.iter().next().copied();
        self.selection = covered;
    }

    /// The row the preview pane / quick-look targets (FILEMGR-10): the
    /// selection anchor when it is still selected, else the highest-index
    /// selected row, else `None`.
    #[must_use]
    pub fn focused_row(&self) -> Option<&FileRow> {
        let idx = self
            .anchor
            .filter(|i| self.selection.contains(i))
            .or_else(|| self.selection.iter().next_back().copied())?;
        self.rows.get(idx)
    }
}

/// One pane (viewport): its own tab strip.
pub struct Pane {
    tabs: Vec<Tab>,
    active_tab: usize,
}

impl Pane {
    fn new(location: Location) -> Self {
        Self {
            tabs: vec![Tab::new(location)],
            active_tab: 0,
        }
    }

    /// The pane's tabs.
    #[must_use]
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// The index of the active tab.
    #[must_use]
    pub fn active_tab_index(&self) -> usize {
        self.active_tab
    }

    /// The active tab.
    #[must_use]
    pub fn active_tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// A drop plan (pure) — what a drag-and-drop drop means.
// ═══════════════════════════════════════════════════════════════════════════

/// Build the [`OpKind`] a drop means.
///
/// A **Move** by default, a **Copy** when the modifier (Ctrl) is held (lock 24).
/// Pure, so the intent is unit-tested without egui; the queue then runs it.
/// Placing items *into* `dest_dir` is the classic "drop here" shape the queue
/// expects.
#[must_use]
pub fn plan_transfer(sources: Vec<PathBuf>, dest_dir: PathBuf, copy: bool) -> OpKind {
    if copy {
        OpKind::Copy {
            items: sources,
            dest_dir,
        }
    } else {
        OpKind::Move {
            items: sources,
            dest_dir,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The whole surface model.
// ═══════════════════════════════════════════════════════════════════════════

/// The render-agnostic state of the Files surface: the backend + op queue, the
/// mesh roster, the two panes (dual-pane), the per-folder view memory, and the
/// Send-To destination.
pub struct FileBrowser {
    backend: Box<dyn Backend>,
    ops: Ops,
    /// FILEMGR-9 — the mesh-mount client (reads `state/mesh-mount/*`, writes
    /// `action/mesh-mount/<host>`). Injectable so the model is unit-tested
    /// headless; production is [`BusMeshMount`].
    mesh: Box<dyn MeshMountClient>,
    /// FILEMGR-9 — the latest worker-published mount view per peer (`host` →
    /// [`MountView`]), refreshed on the [`MOUNT_POLL`] cadence.
    mounts: HashMap<String, MountView>,
    /// When the mount state was last polled (drives the fixed cadence).
    last_mount_poll: Option<Instant>,
    self_node: SelfNode,
    peers: Vec<Peer>,
    mesh_overlay: Option<MeshOverlayBadge>,
    panes: [Pane; 2],
    active_pane: usize,
    dual: bool,
    folder_prefs: HashMap<String, FolderPrefs>,
    destination: Option<String>,
    last_send: SendOutcome,
    last_note: Option<String>,
    /// FILEMGR-10 — the preview/thumbnail decode worker + bounded caches +
    /// pane/quick-look toggles (render-agnostic; the view uploads textures).
    previews: Previews,
}

impl FileBrowser {
    /// The location a fresh surface opens on: the local home directory.
    pub const HOME: &'static str = "local:home";

    /// Build a browser over `backend`, running file operations through a queue
    /// over the real filesystem ([`LiveFileOps`]). This is the production path.
    #[must_use]
    pub fn new(backend: Box<dyn Backend>) -> Self {
        Self::with_file_ops(backend, LiveFileOps::new())
    }

    /// Build a browser over `backend` with an explicit [`FileOps`] for the op
    /// queue — production passes `LiveFileOps`; tests pass a `FakeFileOps` so the
    /// whole submit → run → report path runs with no disk I/O.
    #[must_use]
    pub fn with_file_ops<F: FileOps + Send + 'static>(
        backend: Box<dyn Backend>,
        fileops: F,
    ) -> Self {
        let home = Location::Local(Self::HOME.to_string());
        let mut me = Self {
            backend,
            ops: Ops::spawn(fileops),
            mesh: Box::new(BusMeshMount::from_env()),
            mounts: HashMap::new(),
            last_mount_poll: None,
            self_node: SelfNode::default(),
            peers: Vec::new(),
            mesh_overlay: None,
            panes: [Pane::new(home.clone()), Pane::new(home)],
            active_pane: 0,
            dual: false,
            folder_prefs: HashMap::new(),
            destination: None,
            last_send: SendOutcome::Idle,
            last_note: None,
            previews: Previews::spawn(),
        };
        me.refresh_roster();
        me.reload(0);
        me.reload(1);
        me
    }

    /// Swap in an explicit [`MeshMountClient`] (tests inject a fake; production
    /// keeps the [`BusMeshMount`] from [`Self::with_file_ops`]). Re-reads the
    /// mount state through the new client so the sidebar reflects it immediately.
    #[must_use]
    pub fn with_mesh_mount(mut self, mesh: Box<dyn MeshMountClient>) -> Self {
        self.mesh = mesh;
        self.read_mounts();
        self
    }

    // ── roster / identity ───────────────────────────────────────────────────

    /// Re-probe the mesh (cheap + idempotent on `RealBackend`) and refresh the
    /// cached identity + roster. Drops a Send-To destination that's no longer
    /// reachable.
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

    /// The live Nebula overlay badge, or `None` when standalone / no daemon.
    #[must_use]
    pub fn mesh_overlay(&self) -> Option<&MeshOverlayBadge> {
        self.mesh_overlay.as_ref()
    }

    /// The peers that can receive a Send-To right now (online / idle / self).
    #[must_use]
    pub fn reachable_destinations(&self) -> Vec<&Peer> {
        self.peers
            .iter()
            .filter(|p| p.status.is_reachable())
            .collect()
    }

    // ── mesh-mount (FILEMGR-9 — the Mesh sidebar tree) ───────────────────────

    /// Re-read `state/mesh-mount/*` into the cache. A local spool scan — never a
    /// peer probe — so it can't hang the UI (lock 15).
    fn read_mounts(&mut self) {
        self.mounts = self.mesh.views();
        self.last_mount_poll = Some(Instant::now());
    }

    /// Refresh the mount state on the [`MOUNT_POLL`] cadence (call once per frame;
    /// it self-gates, so it's cheap to call every frame). A worker transition —
    /// mounting → mounted, a drop → reconnecting — surfaces within the window.
    pub fn pump_mounts(&mut self) {
        let due = self
            .last_mount_poll
            .is_none_or(|t| t.elapsed() >= MOUNT_POLL);
        if due {
            self.read_mounts();
        }
    }

    /// The worker's published mount view for a peer host (`None` when the worker
    /// has never reported on it — i.e. it's never been navigated to).
    #[must_use]
    pub fn mount_view(&self, host: &str) -> Option<&MountView> {
        self.mounts.get(host)
    }

    /// The mount view for a specific roster peer (keyed by its short mount host).
    #[must_use]
    pub fn peer_mount(&self, peer: &Peer) -> Option<&MountView> {
        self.mounts.get(mount_host_of(peer))
    }

    /// `true` when any peer's mount is still moving (mounting / reconnecting) — the
    /// view keeps a repaint heartbeat alive so those pips animate to completion.
    #[must_use]
    pub fn any_mount_transitional(&self) -> bool {
        self.mounts.values().any(|m| m.phase.is_transitional())
    }

    /// Navigate `pane` into peer `host` (its short mount name): request the mount
    /// (FILEMGR-5) and browse it. If the worker already reports it mounted, browse
    /// the live sshfs mountpoint directly; otherwise request a mount and browse the
    /// peer's virtual listing meanwhile (the sidebar pip shows it coming up). An
    /// **offline** peer is an honest no-op with a note — never a request, never a
    /// hang (reachability is read from the roster, not a blocking probe).
    pub fn open_peer(&mut self, pane: usize, host: &str) {
        let Some(peer) = self
            .peers
            .iter()
            .find(|p| mount_host_of(p) == host)
            .cloned()
        else {
            return;
        };
        if !peer.status.is_reachable() {
            self.last_note = Some(format!("{host} is offline \u{2014} can't mount it."));
            return;
        }
        // Already mounted with a live path → browse it directly (and keep it warm
        // so the idle-unmount clock resets on the worker side).
        let mounted_path = self
            .peer_mount(&peer)
            .and_then(MountView::mountpoint)
            .map(str::to_string);
        if let Some(mountpoint) = mounted_path {
            let _ = self.mesh.request(host, MeshMountVerb::Mount);
            self.navigate(pane, Location::Local(mountpoint));
            self.last_note = None;
            return;
        }
        // Otherwise request the mount + browse the peer's virtual listing while it
        // comes up. The request is a local Bus append the worker drains on its tick.
        match self.mesh.request(host, MeshMountVerb::Mount) {
            Ok(()) => self.last_note = None,
            Err(e) => self.last_note = Some(e),
        }
        self.navigate(pane, Location::Peer(peer.id));
        self.read_mounts();
    }

    /// Escalate peer `host` from home to **full-filesystem** access (lock 14 — the
    /// `escalate` verb). The GUI action behind the sidebar's "full FS" control.
    /// Offline peers are an honest no-op.
    pub fn escalate_peer(&mut self, host: &str) {
        if self
            .peers
            .iter()
            .find(|p| mount_host_of(p) == host)
            .is_some_and(|p| !p.status.is_reachable())
        {
            self.last_note = Some(format!("{host} is offline \u{2014} can't escalate it."));
            return;
        }
        match self.mesh.request(host, MeshMountVerb::Escalate) {
            Ok(()) => {
                self.last_note = Some(format!(
                    "Escalating {host} to full-filesystem access\u{2026}"
                ));
            }
            Err(e) => self.last_note = Some(e),
        }
        self.read_mounts();
    }

    /// Unmount peer `host` (the `unmount` verb) — tears the mount down + forgets
    /// it. The sidebar's eject control.
    pub fn unmount_peer(&mut self, host: &str) {
        match self.mesh.request(host, MeshMountVerb::Unmount) {
            Ok(()) => self.last_note = Some(format!("Unmounting {host}\u{2026}")),
            Err(e) => self.last_note = Some(e),
        }
        self.read_mounts();
    }

    // ── pane / tab structure ────────────────────────────────────────────────

    /// A pane by index (`0` = left/primary, `1` = right).
    #[must_use]
    pub fn pane(&self, pane: usize) -> &Pane {
        &self.panes[pane.min(1)]
    }

    /// The focused pane's index.
    #[must_use]
    pub fn active_pane_index(&self) -> usize {
        self.active_pane
    }

    /// `true` when the second pane is shown (dual-pane mode).
    #[must_use]
    pub fn is_dual(&self) -> bool {
        self.dual
    }

    /// The active pane's active tab — the target of the toolbar + Send-To.
    #[must_use]
    pub fn active_tab(&self) -> &Tab {
        self.pane(self.active_pane).active_tab()
    }

    /// Focus a pane (a click anywhere in it, or a sidebar action).
    pub fn set_active_pane(&mut self, pane: usize) {
        if pane <= 1 {
            self.active_pane = pane;
        }
    }

    /// Show / hide the second pane. Hiding it refocuses the primary.
    pub fn toggle_dual(&mut self) {
        self.dual = !self.dual;
        if !self.dual {
            self.active_pane = 0;
        }
    }

    fn tab_index(&self, pane: usize) -> usize {
        self.panes[pane].active_tab
    }

    // ── navigation ──────────────────────────────────────────────────────────

    /// Re-fetch the active tab of `pane` from the backend, applying that
    /// folder's remembered presentation.
    pub fn reload(&mut self, pane: usize) {
        let ti = self.tab_index(pane);
        let loc = self.panes[pane].tabs[ti].location.clone();
        let key = loc.backend_path();
        let prefs = self.folder_prefs.get(&key).copied().unwrap_or_default();
        let all = self.backend.list(&key);
        let tab = &mut self.panes[pane].tabs[ti];
        tab.all_rows = all;
        tab.view = prefs.view;
        tab.sort = prefs.sort;
        tab.show_hidden = prefs.show_hidden;
        tab.path_edit = key;
        tab.recompute();
    }

    /// Reload both panes (after an op finishes, so results appear/disappear).
    pub fn reload_all(&mut self) {
        self.reload(0);
        self.reload(1);
    }

    /// Point `pane`'s active tab at `loc`, pushing the prior location onto its
    /// back-history (and clearing forward), then load it.
    pub fn navigate(&mut self, pane: usize, loc: Location) {
        let ti = self.tab_index(pane);
        {
            let tab = &mut self.panes[pane].tabs[ti];
            if tab.location != loc {
                let prev = std::mem::replace(&mut tab.location, loc);
                tab.back.push(prev);
                tab.forward.clear();
            }
        }
        self.reload(pane);
    }

    /// Go back one step in `pane`'s history.
    pub fn go_back(&mut self, pane: usize) {
        let ti = self.tab_index(pane);
        {
            let tab = &mut self.panes[pane].tabs[ti];
            let Some(prev) = tab.back.pop() else {
                return;
            };
            let cur = std::mem::replace(&mut tab.location, prev);
            tab.forward.push(cur);
        }
        self.reload(pane);
    }

    /// Go forward one step in `pane`'s history.
    pub fn go_forward(&mut self, pane: usize) {
        let ti = self.tab_index(pane);
        {
            let tab = &mut self.panes[pane].tabs[ti];
            let Some(next) = tab.forward.pop() else {
                return;
            };
            let cur = std::mem::replace(&mut tab.location, next);
            tab.back.push(cur);
        }
        self.reload(pane);
    }

    /// Navigate `pane` to its current location's parent (absolute local only).
    pub fn go_up(&mut self, pane: usize) {
        let ti = self.tab_index(pane);
        if let Some(parent) = self.panes[pane].tabs[ti].location.parent() {
            self.navigate(pane, parent);
        }
    }

    /// Update `pane`'s editable path buffer (a keystroke in the path box).
    pub fn set_path_edit(&mut self, pane: usize, text: String) {
        let ti = self.tab_index(pane);
        self.panes[pane].tabs[ti].path_edit = text;
    }

    /// Navigate `pane` to whatever its editable path buffer currently holds
    /// (Enter in the path box). `peer:<id>` routes to that peer; anything else is
    /// a local path (absolute or a `local:` shortcut).
    pub fn open_path_edit(&mut self, pane: usize) {
        let ti = self.tab_index(pane);
        let raw = self.panes[pane].tabs[ti].path_edit.trim().to_string();
        if raw.is_empty() {
            return;
        }
        let loc = raw.strip_prefix("peer:").map_or_else(
            || Location::Local(raw.clone()),
            |id| Location::Peer(id.to_string()),
        );
        self.navigate(pane, loc);
    }

    /// Activate row `idx` in `pane` (a double-click / Enter): descend into a
    /// directory, or open a **file** in the built-in quick-look viewer
    /// (FILEMGR-10 / lock 23 — built-in viewers only, never an external
    /// program spawn §9).
    pub fn open_row(&mut self, pane: usize, idx: usize) {
        let ti = self.tab_index(pane);
        let Some(row) = self.panes[pane].tabs[ti].rows.get(idx).cloned() else {
            return;
        };
        if row.is_dir() {
            if let Some(path) = row.path {
                self.navigate(pane, Location::Local(path));
            }
            // A virtual peer folder has no local path — descent needs the mesh
            // mount (FILEMGR-9); honestly a no-op until then.
        } else {
            // Select it (so the quick-look target fold finds it) and open the
            // built-in viewer overlay.
            self.panes[pane].tabs[ti].click(idx);
            self.previews.set_quick_look(true);
        }
    }

    // ── view / sort / filter (persist per folder) ───────────────────────────

    /// Set `pane`'s view mode and remember it for this folder.
    pub fn set_view(&mut self, pane: usize, mode: ViewMode) {
        let ti = self.tab_index(pane);
        self.panes[pane].tabs[ti].view = mode;
        self.remember(pane);
    }

    /// Click a sort column header: same key toggles direction, a new key sorts
    /// ascending. Re-sorts + remembers for this folder.
    pub fn sort_by(&mut self, pane: usize, key: SortKey) {
        let ti = self.tab_index(pane);
        {
            let tab = &mut self.panes[pane].tabs[ti];
            if tab.sort.key == key {
                tab.sort.dir = tab.sort.dir.flip();
            } else {
                tab.sort.key = key;
                tab.sort.dir = SortDir::Asc;
            }
            tab.recompute();
        }
        self.remember(pane);
    }

    /// Toggle the show-hidden filter for `pane` (Ctrl+H) and remember it.
    pub fn toggle_hidden(&mut self, pane: usize) {
        let ti = self.tab_index(pane);
        {
            let tab = &mut self.panes[pane].tabs[ti];
            tab.show_hidden = !tab.show_hidden;
            tab.recompute();
        }
        self.remember(pane);
    }

    /// Toggle dirs-first grouping for `pane` and remember it.
    pub fn toggle_dirs_first(&mut self, pane: usize) {
        let ti = self.tab_index(pane);
        {
            let tab = &mut self.panes[pane].tabs[ti];
            tab.sort.dirs_first = !tab.sort.dirs_first;
            tab.recompute();
        }
        self.remember(pane);
    }

    fn remember(&mut self, pane: usize) {
        let ti = self.tab_index(pane);
        let (key, prefs) = {
            let tab = &self.panes[pane].tabs[ti];
            (
                tab.location.backend_path(),
                FolderPrefs {
                    view: tab.view,
                    sort: tab.sort,
                    show_hidden: tab.show_hidden,
                },
            )
        };
        self.folder_prefs.insert(key, prefs);
    }

    // ── selection ───────────────────────────────────────────────────────────

    /// Plain click: select only row `idx` in `pane`.
    pub fn click(&mut self, pane: usize, idx: usize) {
        let ti = self.tab_index(pane);
        self.panes[pane].tabs[ti].click(idx);
    }

    /// Ctrl-click: toggle row `idx` in `pane`'s selection.
    pub fn ctrl_click(&mut self, pane: usize, idx: usize) {
        let ti = self.tab_index(pane);
        self.panes[pane].tabs[ti].ctrl_click(idx);
    }

    /// Shift-click: select the range from the anchor to row `idx` in `pane`.
    pub fn shift_click(&mut self, pane: usize, idx: usize) {
        let ti = self.tab_index(pane);
        self.panes[pane].tabs[ti].shift_click(idx);
    }

    /// Ctrl-A: select every row in `pane`'s active tab.
    pub fn select_all(&mut self, pane: usize) {
        let ti = self.tab_index(pane);
        self.panes[pane].tabs[ti].select_all();
    }

    /// Clear `pane`'s selection (Escape / a background click).
    pub fn clear_selection(&mut self, pane: usize) {
        let ti = self.tab_index(pane);
        self.panes[pane].tabs[ti].clear_selection();
    }

    /// Set `pane`'s selection to a rubber-band's covered rows (view geometry).
    pub fn set_rubber(&mut self, pane: usize, covered: BTreeSet<usize>) {
        let ti = self.tab_index(pane);
        self.panes[pane].tabs[ti].set_rubber(covered);
    }

    // ── tabs ────────────────────────────────────────────────────────────────

    /// Open a new tab in `pane` at the active tab's location and focus it.
    pub fn new_tab(&mut self, pane: usize) {
        let loc = {
            let ti = self.tab_index(pane);
            self.panes[pane].tabs[ti].location.clone()
        };
        self.panes[pane].tabs.push(Tab::new(loc));
        self.panes[pane].active_tab = self.panes[pane].tabs.len() - 1;
        self.reload(pane);
    }

    /// Close tab `tab_ix` in `pane` (always keeping at least one open).
    pub fn close_tab(&mut self, pane: usize, tab_ix: usize) {
        let p = &mut self.panes[pane];
        if p.tabs.len() <= 1 || tab_ix >= p.tabs.len() {
            return;
        }
        p.tabs.remove(tab_ix);
        if p.active_tab >= p.tabs.len() {
            p.active_tab = p.tabs.len() - 1;
        } else if tab_ix < p.active_tab {
            p.active_tab -= 1;
        }
    }

    /// Focus tab `tab_ix` in `pane` (each tab keeps its own loaded state).
    pub fn select_tab(&mut self, pane: usize, tab_ix: usize) {
        if tab_ix < self.panes[pane].tabs.len() {
            self.panes[pane].active_tab = tab_ix;
        }
    }

    // ── drag-and-drop transfer (through the FILEMGR-2 queue) ────────────────

    /// A drop of `source_pane`'s selection into `dest_dir`. Move by default,
    /// copy when `copy` (Ctrl held at drop). Submits the transfer to the queue
    /// and returns its op id; `None` (with an honest note) when the selection
    /// carries no filesystem paths (a peer/virtual selection needs a mesh mount,
    /// FILEMGR-9).
    pub fn drop_transfer(
        &mut self,
        source_pane: usize,
        dest_dir: PathBuf,
        copy: bool,
    ) -> Option<OpId> {
        let sources = self.pane(source_pane).active_tab().selected_paths();
        if sources.is_empty() {
            self.last_note =
                Some("Nothing to transfer — mesh/peer files need a mount (FILEMGR-9).".to_string());
            return None;
        }
        let count = sources.len();
        let dest = dest_dir.file_name().map_or_else(
            || dest_dir.display().to_string(),
            |s| s.to_string_lossy().into_owned(),
        );
        let kind = plan_transfer(sources, dest_dir, copy);
        let verb = if copy { "Copying" } else { "Moving" };
        let noun = if count == 1 { "item" } else { "items" };
        let id = self
            .ops
            .submit(kind, format!("{verb} {count} {noun} \u{2192} {dest}"));
        self.last_note = None;
        Some(id)
    }

    // ── the op queue (progress strip) ───────────────────────────────────────

    /// Drain the op queue's events (call once per frame). Reloads both panes
    /// when any op finished, so moved/copied files appear or disappear.
    pub fn pump_ops(&mut self) {
        let finished = self.ops.pump();
        if !finished.is_empty() {
            self.reload_all();
        }
    }

    /// The op queue (its live [`crate::ops::ActiveOp`] list for the strip).
    #[must_use]
    pub fn ops(&self) -> &Ops {
        &self.ops
    }

    /// Pause a running op (its strip button).
    pub fn pause_op(&mut self, op_id: OpId) {
        if let Some(op) = self.ops.active().iter().find(|o| o.op_id == op_id) {
            op.control.pause();
        }
    }

    /// Resume a paused op.
    pub fn resume_op(&mut self, op_id: OpId) {
        if let Some(op) = self.ops.active().iter().find(|o| o.op_id == op_id) {
            op.control.resume();
        }
    }

    /// Cancel a running op (rolls back its in-flight item).
    pub fn cancel_op(&mut self, op_id: OpId) {
        if let Some(op) = self.ops.active().iter().find(|o| o.op_id == op_id) {
            op.control.cancel();
        }
    }

    /// Dismiss a finished op from the strip.
    pub fn dismiss_op(&mut self, op_id: OpId) {
        self.ops.dismiss(op_id);
    }

    /// The most recent non-Send status note (a drag-and-drop guard message), if
    /// any.
    #[must_use]
    pub fn last_note(&self) -> Option<&str> {
        self.last_note.as_deref()
    }

    // ── previews + thumbnails + quick-look (FILEMGR-10) ─────────────────────

    /// Fold finished decodes into the caches (call once per frame). Returns
    /// `true` when anything landed, so the view repaints.
    pub fn pump_previews(&mut self) -> bool {
        self.previews.pump()
    }

    /// `true` while any thumbnail/preview decode is still in flight (the view
    /// keeps a repaint heartbeat alive so results appear without input).
    #[must_use]
    pub fn previews_pending(&self) -> bool {
        self.previews.any_pending()
    }

    /// The thumbnail slot for `path` (`None` = never requested / evicted).
    #[must_use]
    pub fn thumb_state(&self, path: &str) -> Option<&ThumbState> {
        self.previews.thumb(path)
    }

    /// The preview slot for `path` (`None` = never requested / evicted).
    #[must_use]
    pub fn preview_state(&self, path: &str) -> Option<&PreviewState> {
        self.previews.preview(path)
    }

    /// Want a thumbnail for `path` — decodes off-thread when cold, keeps the
    /// LRU slot warm when already cached. The view calls this every frame a
    /// cell is actually visible, so eviction order tracks visibility.
    pub fn request_thumb(&mut self, path: &str) {
        self.previews.request_thumb(path);
    }

    /// Want a pane/quick-look preview for `path` (same contract as
    /// [`request_thumb`](Self::request_thumb)).
    pub fn request_preview(&mut self, path: &str) {
        self.previews.request_preview(path);
    }

    /// Bust the preview/thumbnail caches (lock 18 — a manual refresh
    /// re-decodes; a changed file re-thumbnails on the next request).
    pub fn clear_previews(&mut self) {
        self.previews.clear();
    }

    /// Whether the right-hand preview pane is shown.
    #[must_use]
    pub fn preview_pane_open(&self) -> bool {
        self.previews.pane_open()
    }

    /// Toggle the preview pane.
    pub fn toggle_preview_pane(&mut self) {
        self.previews.toggle_pane();
    }

    /// Whether the List view shows its thumbnail column (Grid always does).
    #[must_use]
    pub fn list_thumbs(&self) -> bool {
        self.previews.list_thumbs()
    }

    /// Toggle the List view's thumbnail column.
    pub fn toggle_list_thumbs(&mut self) {
        self.previews.toggle_list_thumbs();
    }

    /// Whether the quick-look overlay is up.
    #[must_use]
    pub fn quick_look_open(&self) -> bool {
        self.previews.quick_look()
    }

    /// Space: toggle the quick-look overlay. Opening requires a focused row —
    /// with nothing selected there is honestly nothing to look at.
    pub fn toggle_quick_look(&mut self) {
        if self.previews.quick_look() {
            self.previews.set_quick_look(false);
        } else if self.preview_target().is_some() {
            self.previews.set_quick_look(true);
        }
    }

    /// Close the quick-look overlay (Escape / a backdrop click).
    pub fn close_quick_look(&mut self) {
        self.previews.set_quick_look(false);
    }

    /// The row the preview pane / quick-look shows: the active tab's focused
    /// selection.
    #[must_use]
    pub fn preview_target(&self) -> Option<&FileRow> {
        self.active_tab().focused_row()
    }

    /// `true` when `path` sits under a mesh mount — the stable
    /// `/run/user/<uid>/mde-mesh/<host>` root (lock 11) or any mountpoint the
    /// FILEMGR-5 worker has published. Remote files are never bulk-decoded
    /// (lock 18): thumbnails only when selected, previews on demand.
    #[must_use]
    pub fn is_remote_path(&self, path: &str) -> bool {
        if path.contains("/mde-mesh/") {
            return true;
        }
        self.mounts
            .values()
            .filter_map(|m| m.path.as_deref())
            .any(|mp| {
                path.strip_prefix(mp)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
            })
    }

    // ── Send-To (mesh transfer — carried from E12-11) ───────────────────────

    /// The chosen destination peer id, if any.
    #[must_use]
    pub fn destination(&self) -> Option<&str> {
        self.destination.as_deref()
    }

    /// Choose `peer_id` as the Send-To destination.
    pub fn set_destination(&mut self, peer_id: impl Into<String>) {
        self.destination = Some(peer_id.into());
    }

    fn destination_reachable(&self) -> bool {
        self.destination.as_ref().is_some_and(|id| {
            self.peers
                .iter()
                .any(|p| &p.id == id && p.status.is_reachable())
        })
    }

    /// The first selected, sendable local file in the active tab (a real file
    /// with a path — directories and virtual peer rows are not Send-To sources).
    #[must_use]
    pub fn send_source(&self) -> Option<PathBuf> {
        let tab = self.active_tab();
        tab.selection
            .iter()
            .filter_map(|&i| tab.rows.get(i))
            .find(|r| !r.is_dir() && r.path.is_some())
            .and_then(|r| r.path.as_ref().map(PathBuf::from))
    }

    /// Build the canonical [`SendToRequest`] for the current selection +
    /// destination, or `None` when unavailable.
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

    /// Whether a Send-To can fire right now (drives the button's enabled state).
    #[must_use]
    pub fn can_send(&self) -> bool {
        self.plan_send().is_some()
    }

    /// Dispatch a prepared request through the backend's transfer surface.
    ///
    /// # Errors
    /// Propagates the backend's [`BackendError`].
    pub fn dispatch(&mut self, req: SendToRequest) -> Result<OpId, BackendError> {
        self.backend
            .send_to(&req.sources, req.destination, req.mode, req.conflict)
    }

    /// Plan + dispatch the Send-To for the current selection, recording the
    /// outcome for the status line. `None` when nothing is planned.
    pub fn send(&mut self) -> Option<Result<OpId, BackendError>> {
        let req = self.plan_send()?;
        let file = self
            .send_source()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
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
    use mde_files::backend::{AuditEntry, ConflictPolicy, LocalFsBackend, SendMode};
    use mde_files::fileops::{FakeFileOps, FileOps};
    use mde_files::model::{PeerKind, PeerStatus};
    use std::collections::HashMap as Map;

    // ── In-test backend double (from E12-11, unchanged shape) ────────────────

    struct FixtureBackend {
        peers: Vec<Peer>,
        rows: Vec<FileRow>,
        peer_rows: Map<String, Vec<FileRow>>,
        next_op: OpId,
        mesh: Option<MeshOverlayBadge>,
    }

    impl FixtureBackend {
        fn new(peers: Vec<Peer>, rows: Vec<FileRow>) -> Self {
            Self {
                peers,
                rows,
                peer_rows: Map::new(),
                next_op: 1,
                mesh: None,
            }
        }
        fn with_peer(mut self, id: &str, rows: Vec<FileRow>) -> Self {
            self.peer_rows.insert(id.to_string(), rows);
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
        fn list(&self, path: &str) -> Vec<FileRow> {
            if let Some(id) = path.strip_prefix("peer:") {
                return self.peer_rows.get(id).cloned().unwrap_or_default();
            }
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
        assert!(b.last_note().is_some(), "an honest note explains why");
    }

    #[test]
    fn escalate_requests_the_escalate_verb_for_a_reachable_peer() {
        let fake = FakeMeshMount::new();
        let probe = fake.clone();
        let mut b = browser_over(roster_backend()).with_mesh_mount(Box::new(fake));
        b.escalate_peer("pine");
        assert_eq!(probe.verbs_for("pine"), vec![MeshMountVerb::Escalate]);
    }

    #[test]
    fn escalate_is_a_no_op_for_an_offline_peer() {
        let fake = FakeMeshMount::new();
        let probe = fake.clone();
        let mut b = browser_over(roster_backend()).with_mesh_mount(Box::new(fake));
        b.escalate_peer("cedar"); // Offline
        assert_eq!(probe.request_count(), 0);
        assert!(b.last_note().is_some());
    }

    #[test]
    fn unmount_requests_the_unmount_verb() {
        let fake = FakeMeshMount::new();
        let probe = fake.clone();
        let mut b = browser_over(roster_backend()).with_mesh_mount(Box::new(fake));
        b.unmount_peer("pine");
        assert_eq!(probe.verbs_for("pine"), vec![MeshMountVerb::Unmount]);
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
}
