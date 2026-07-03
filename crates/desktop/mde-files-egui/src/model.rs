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
use std::time::{Duration, Instant, SystemTime};

use mde_files::backend::{Backend, BackendError, Destination, MeshOverlayBadge, OpId};
use mde_files::fileops::{FileOps, LiveFileOps};
use mde_files::model::{FileRow, Mime, Peer, SelfNode};
use mde_files::opqueue::OpKind;
use mde_files::search::{
    ContentMode, ContentQuery, Filters, SearchEvent, SearchQuery, SearchRun, SearchStats,
    TypeFilter,
};
use mde_files::send_to::{SendToEntry, SendToRequest};

use crate::chat_bridge::{BusChatBridge, ChatBridge};
use crate::dialogs::{Arming, ConfirmDelete, Perm, PermClass, PropertiesDialog};
use crate::mesh_mount::{BusMeshMount, MeshMountClient, MeshMountVerb, MountView};
use crate::ops::Ops;
use crate::preview::{PreviewState, Previews, ThumbState};
use mde_files::opqueue::{ConflictChoice, Resolution};

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

    /// FILEMGR-4 — reset this tab to hold a fresh, empty search-results listing.
    /// Hidden entries are shown so a name/content hit on a dotfile isn't filtered
    /// back out of its own result set.
    fn begin_search(&mut self) {
        self.all_rows.clear();
        self.rows.clear();
        self.selection.clear();
        self.anchor = None;
        self.show_hidden = true;
    }

    /// FILEMGR-4 — append one streamed search hit. Rows are **append-only** so a
    /// live-growing result list never shifts an existing index — a selection or a
    /// pending op stays valid while results keep arriving. The hidden filter is
    /// honored exactly as in a normal listing.
    fn push_search_hit(&mut self, row: FileRow) {
        if self.show_hidden || !is_hidden(&row.name) {
            self.rows.push(row.clone());
        }
        self.all_rows.push(row);
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

/// FILEMGR-12 — encode paths for the shared shell clipboard: one absolute path
/// per line (the plain-text form any surface's paste reads back).
fn join_clip_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// FILEMGR-12 — decode a shared-clipboard paste back into file paths: one per
/// line, keeping only **absolute paths that exist** on this node (a mounted-peer
/// path counts — it's a live sshfs path). Anything else (a URL, prose, a stale
/// path) is dropped, so a cross-surface paste never queues a bogus transfer.
fn parse_clip_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.exists())
        .collect()
}

/// FILEMGR-12 — a cut/copy set staged for Paste. The path text is mirrored onto
/// the shared shell clipboard on Cut/Copy (in the view); this in-model record
/// only decides move-vs-copy — a Paste of exactly this `cut` set *moves*, any
/// other paste (incl. one from another surface) *copies*.
#[derive(Clone)]
struct ClipEntry {
    /// The staged source paths.
    paths: Vec<PathBuf>,
    /// `true` for Cut (a matching Paste moves + clears), `false` for Copy.
    cut: bool,
}

// ═══════════════════════════════════════════════════════════════════════════
// FILEMGR-4 — recursive search: the entry/filter form + the live run.
// ═══════════════════════════════════════════════════════════════════════════

/// The search bar's current entry + filter values.
///
/// Pure data (no egui), persisted across frames on the model so the view binds to
/// it and [`FileBrowser::start_search`] compiles a [`SearchQuery`] from it. Sizes
/// are entered in KB and ages in days (the friendly units);
/// [`to_query`](Self::to_query) converts to bytes / instants.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchForm {
    /// Whether the search bar is expanded (a toolbar toggle).
    pub open: bool,
    /// The name-glob (`*.rs`, `report-*`). Empty = no name predicate.
    pub name_glob: String,
    /// The content grep text. Empty = no content predicate.
    pub content: String,
    /// Interpret [`content`](Self::content) as a regex rather than a substring.
    pub content_regex: bool,
    /// Restrict to files / folders / any.
    pub kind: TypeFilter,
    /// Restrict to this extension (no dot). Empty = unset.
    pub ext: String,
    /// Minimum size in KB (parsed; blank/garbage = unset).
    pub min_size_kb: String,
    /// Maximum size in KB (parsed; blank/garbage = unset).
    pub max_size_kb: String,
    /// "Modified within the last N days" (parsed; blank/garbage = unset).
    pub within_days: String,
}

impl SearchForm {
    /// Build a [`SearchQuery`] from the form, or `None` when it carries no active
    /// predicate (an empty search — the view keeps the Search button disabled in
    /// that case, and this is the belt-and-braces guard). The search *root* is not
    /// part of the query; the caller passes it to [`SearchRun::spawn`] separately.
    #[must_use]
    pub fn to_query(&self) -> Option<SearchQuery> {
        let name = non_blank(&self.name_glob);
        let content = non_blank(&self.content).map(|pat| ContentQuery {
            pattern: pat,
            mode: if self.content_regex {
                ContentMode::Regex
            } else {
                ContentMode::Substring
            },
            case_insensitive: true,
        });
        let ext = non_blank(&self.ext).map(|e| e.trim_start_matches('.').to_ascii_lowercase());
        let min_size = parse_kb(&self.min_size_kb);
        let max_size = parse_kb(&self.max_size_kb);
        let modified_after = parse_within_days(&self.within_days);

        let query = SearchQuery {
            name_glob: name,
            name_case_insensitive: true,
            content,
            filters: Filters {
                kinds: self.kind,
                ext,
                min_size,
                max_size,
                modified_after,
                modified_before: None,
            },
            ..SearchQuery::default()
        };
        query.is_meaningful().then_some(query)
    }
}

/// Trim `s`; return `Some(owned)` only when non-empty.
fn non_blank(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Parse a KB count into bytes (`None` on blank/garbage/overflow).
fn parse_kb(s: &str) -> Option<u64> {
    s.trim()
        .parse::<u64>()
        .ok()
        .and_then(|kb| kb.checked_mul(1024))
}

/// Parse "within the last N days" into the earliest modified instant to accept.
fn parse_within_days(s: &str) -> Option<SystemTime> {
    let days = s.trim().parse::<u64>().ok().filter(|d| *d > 0)?;
    let secs = days.checked_mul(86_400)?;
    SystemTime::now().checked_sub(Duration::from_secs(secs))
}

/// A read-only snapshot of the live search, for the status line.
#[derive(Debug, Clone)]
pub struct SearchProgress {
    /// Human label for the searched root (a directory path or a mounted peer).
    pub root_label: String,
    /// `true` while the worker is still walking.
    pub running: bool,
    /// `true` once the walk was cancelled.
    pub cancelled: bool,
    /// Hits streamed so far.
    pub matched: u64,
    /// Entries visited so far (final tally after Done).
    pub scanned: u64,
    /// The root is a mounted mesh path — the view shows the honest "slower" note.
    pub remote: bool,
}

/// The in-flight search: the worker handle, where its results render (pane + tab
/// index), and the running tallies. At most one search runs at a time.
struct SearchState {
    run: SearchRun,
    pane: usize,
    tab: usize,
    root_label: String,
    remote: bool,
    scanned: u64,
    matched: u64,
    done: bool,
    cancelled: bool,
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
    /// FILEMGR-11 — a synchronous [`FileOps`] for the *immediate* metadata ops
    /// the Properties dialog drives (`metadata` / `chmod` / `chown`), separate
    /// from the queue's worker-owned `FileOps` (which runs the long transfers).
    /// Production is [`LiveFileOps`] (the same real filesystem as the queue);
    /// tests inject a [`FakeFileOps`](mde_files::fileops::FakeFileOps).
    meta_ops: Box<dyn FileOps>,
    /// FILEMGR-11 — whether this caller may `chown` (root / `CAP_CHOWN`). Probed
    /// once from the effective uid in production; the Properties dialog offers the
    /// owner/group control only when this is `true` (lock 8).
    chown_permitted: bool,
    /// FILEMGR-11 — the open Properties dialog, if any.
    properties: Option<PropertiesDialog>,
    /// FILEMGR-11 — the pending permanent-delete confirm, if any.
    confirm_delete: Option<ConfirmDelete>,
    /// FILEMGR-9 — the mesh-mount client (reads `state/mesh-mount/*`, writes
    /// `action/mesh-mount/<host>`). Injectable so the model is unit-tested
    /// headless; production is [`BusMeshMount`].
    mesh: Box<dyn MeshMountClient>,
    /// FILEMGR-12 — the "Send in Chat" hand-off seam: offers a file to a peer's
    /// NOTIFY-CHAT conversation as the reused `mde-chat` file message-kind.
    /// Injectable so `send_in_chat` is unit-tested headless; production is
    /// [`BusChatBridge`] (the same persist-first path as [`BusMeshMount`]).
    chat: Box<dyn ChatBridge>,
    /// FILEMGR-12 — the pending cut/copy set, for the *move-vs-copy* semantics of
    /// an in-app Paste (an external paste of the same paths still moves). `None`
    /// until a Cut/Copy runs; a Cut's set is cleared once its Paste consumes it.
    /// The path text is ALSO written to the shared shell clipboard on Cut/Copy so
    /// the paste crosses surfaces (that write lives in the view — it needs the
    /// egui `Context`).
    clipboard: Option<ClipEntry>,
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
    /// FILEMGR-4 — the search bar's entry + filter values (persist across frames).
    search_form: SearchForm,
    /// FILEMGR-4 — the live recursive search, if one is running. Its streamed
    /// hits accumulate into the tab it was launched from, so a result set is an
    /// ordinary file view and every op applies.
    search: Option<SearchState>,
}

impl FileBrowser {
    /// The location a fresh surface opens on: the local home directory.
    pub const HOME: &'static str = "local:home";

    /// Build a browser over `backend`, running file operations through a queue
    /// over the real filesystem ([`LiveFileOps`]). This is the production path —
    /// it probes the effective uid so the Properties dialog offers `chown` only
    /// when this process actually may (root / `CAP_CHOWN`, lock 8).
    #[must_use]
    pub fn new(backend: Box<dyn Backend>) -> Self {
        let mut me = Self::with_file_ops(backend, LiveFileOps::new());
        // rustix's `geteuid` is a safe wrapper (no `unsafe` in this crate), the
        // same running-uid probe mackesd's seal path uses.
        me.chown_permitted = rustix::process::geteuid().is_root();
        me
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
            // A second immediate-ops handle for Properties. Production overrides
            // nothing (both are the real FS); a headless Properties test injects
            // a fake via `with_meta_ops`. `chown_permitted` defaults off — only
            // the production `new` probes the real euid.
            meta_ops: Box::new(LiveFileOps::new()),
            chown_permitted: false,
            properties: None,
            confirm_delete: None,
            chat: Box::new(BusChatBridge::from_env()),
            clipboard: None,
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
            search_form: SearchForm::default(),
            search: None,
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

    /// Swap in an explicit [`ChatBridge`] (FILEMGR-12). Tests inject a recorder to
    /// assert the "Send in Chat" offer; production keeps the [`BusChatBridge`]
    /// from [`Self::with_file_ops`].
    #[must_use]
    pub fn with_chat_bridge(mut self, chat: Box<dyn ChatBridge>) -> Self {
        self.chat = chat;
        self
    }

    /// Swap in the [`FileOps`] the Properties dialog reads/writes through, plus
    /// whether this caller may `chown` (FILEMGR-11). Tests inject a seeded
    /// [`FakeFileOps`](mde_files::fileops::FakeFileOps) with an explicit privilege
    /// so the whole load → edit → apply round-trip runs headless; production keeps
    /// the [`LiveFileOps`] + euid-probed permission from [`Self::new`].
    #[must_use]
    pub fn with_meta_ops<F: FileOps + 'static>(
        mut self,
        meta_ops: F,
        chown_permitted: bool,
    ) -> Self {
        self.meta_ops = Box::new(meta_ops);
        self.chown_permitted = chown_permitted;
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

    // ── recursive search (FILEMGR-4) ─────────────────────────────────────────

    /// The search bar's current entry + filter values.
    #[must_use]
    pub fn search_form(&self) -> &SearchForm {
        &self.search_form
    }

    /// Replace the search form (the view edits a clone, then hands it back — the
    /// same render → intents → apply flow the rest of the surface uses).
    pub fn set_search_form(&mut self, form: SearchForm) {
        self.search_form = form;
    }

    /// Toggle the search bar's expanded state.
    pub fn toggle_search_bar(&mut self) {
        self.search_form.open = !self.search_form.open;
    }

    /// `true` while a search's results are showing (running or finished, until
    /// [`clear_search`](Self::clear_search) restores the folder listing).
    #[must_use]
    pub fn search_active(&self) -> bool {
        self.search.is_some()
    }

    /// `true` while the worker is still walking (drives the repaint heartbeat).
    #[must_use]
    pub fn search_running(&self) -> bool {
        self.search.as_ref().is_some_and(|s| !s.done)
    }

    /// A snapshot of the live search for the status line, or `None` when idle.
    #[must_use]
    pub fn search_progress(&self) -> Option<SearchProgress> {
        self.search.as_ref().map(|s| SearchProgress {
            root_label: s.root_label.clone(),
            running: !s.done,
            cancelled: s.cancelled,
            matched: s.matched,
            scanned: s.scanned,
            remote: s.remote,
        })
    }

    /// Start a recursive search rooted at `pane`'s current directory, streaming
    /// results into that pane's active tab. An empty query, or a pane that isn't
    /// on a browsable local/mounted directory (e.g. a virtual peer folder with no
    /// real path), is an honest no-op with a note — never a hang. A mounted mesh
    /// path is a valid root; it just walks slower (the view says so).
    pub fn start_search(&mut self, pane: usize) {
        if pane > 1 {
            return;
        }
        self.set_active_pane(pane);
        let ti = self.tab_index(pane);
        let Some(root) = self.panes[pane].tabs[ti].current_dir() else {
            self.last_note = Some(
                "Search needs a real directory — open a local folder or a mounted peer first."
                    .to_string(),
            );
            return;
        };
        let Some(query) = self.search_form.to_query() else {
            self.last_note =
                Some("Enter a name pattern or some content text to search.".to_string());
            return;
        };
        match SearchRun::spawn(&query, root.clone()) {
            Ok(run) => {
                let remote = self.is_remote_path(&root.to_string_lossy());
                self.panes[pane].tabs[ti].begin_search();
                self.search = Some(SearchState {
                    run,
                    pane,
                    tab: ti,
                    root_label: root.display().to_string(),
                    remote,
                    scanned: 0,
                    matched: 0,
                    done: false,
                    cancelled: false,
                });
                self.last_note = None;
            }
            Err(err) => {
                self.last_note = Some(format!("Search failed to start: {err}"));
            }
        }
    }

    /// Signal the running search to stop; results found so far stay on screen.
    pub fn cancel_search(&mut self) {
        if let Some(s) = &self.search {
            s.run.cancel();
        }
    }

    /// Leave search mode: drop the (possibly still-running) worker and reload the
    /// tab's real folder listing.
    pub fn clear_search(&mut self, pane: usize) {
        if let Some(s) = &self.search {
            // Ask the worker to stop; dropping the handle also disconnects it.
            s.run.cancel();
        }
        self.search = None;
        if pane <= 1 {
            self.reload(pane);
        }
    }

    /// Fold the worker's streamed hits into the results tab (call once per frame).
    /// Returns `true` when anything landed, so the view repaints. Each hit is an
    /// ordinary [`FileRow`] with a real path — appended in discovery order so the
    /// list grows live without disturbing a selection.
    pub fn pump_search(&mut self) -> bool {
        let Some(search) = self.search.as_mut() else {
            return false;
        };
        let events = search.run.drain();
        if events.is_empty() {
            return false;
        }
        let (pane, ti) = (search.pane, search.tab);
        let mut hits: Vec<FileRow> = Vec::new();
        let mut final_stats: Option<SearchStats> = None;
        for ev in events {
            match ev {
                SearchEvent::Hit(row) => hits.push(row),
                SearchEvent::Done(stats) => final_stats = Some(stats),
            }
        }
        search.matched += hits.len() as u64;
        if let Some(stats) = final_stats {
            search.scanned = stats.scanned;
            search.matched = stats.matched;
            search.done = true;
            search.cancelled = stats.cancelled;
        }
        // The `search` field borrow ends above; `panes` is a disjoint field.
        if !hits.is_empty() {
            let tab = &mut self.panes[pane].tabs[ti];
            for row in hits {
                tab.push_search_hit(row);
            }
        }
        true
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
        Some(self.submit_transfer(sources, dest_dir, copy))
    }

    /// Queue a real Copy/Move of `sources` into `dest_dir` through the FILEMGR-2
    /// op queue and return its op id. The one submit path shared by drag-and-drop
    /// [`drop_transfer`](Self::drop_transfer) and clipboard
    /// [`clip_paste`](Self::clip_paste) — the surface never re-implements a file
    /// op (§6). Callers pre-check for an empty source list.
    fn submit_transfer(&mut self, sources: Vec<PathBuf>, dest_dir: PathBuf, copy: bool) -> OpId {
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
        id
    }

    // ── shared clipboard (FILEMGR-12 — cross-surface cut/copy/paste) ─────────

    /// Stage `pane`'s selection for a **Copy**, returning the newline-joined path
    /// text the view writes onto the shared shell clipboard (so another surface
    /// can paste the paths). `None` — with an honest note — when nothing local is
    /// selected (a peer/virtual row carries no path).
    pub fn clip_copy(&mut self, pane: usize) -> Option<String> {
        self.stage_clipboard(pane, false)
    }

    /// Stage `pane`'s selection for a **Cut** (a later in-app Paste of exactly
    /// this set moves it). Like [`clip_copy`](Self::clip_copy), returns the path
    /// text for the shared shell clipboard.
    pub fn clip_cut(&mut self, pane: usize) -> Option<String> {
        self.stage_clipboard(pane, true)
    }

    fn stage_clipboard(&mut self, pane: usize, cut: bool) -> Option<String> {
        let paths = self.pane(pane).active_tab().selected_paths();
        if paths.is_empty() {
            self.last_note = Some(
                "Nothing to copy — select a local file first (mesh files need a mount).".into(),
            );
            return None;
        }
        let text = join_clip_paths(&paths);
        self.clipboard = Some(ClipEntry { paths, cut });
        self.last_note = None;
        Some(text)
    }

    /// Whether an in-app Paste can fire right now (drives the menu item's enabled
    /// state) — `true` once a Cut/Copy has staged a set.
    #[must_use]
    pub fn can_paste(&self) -> bool {
        self.clipboard.is_some()
    }

    /// Paste the staged in-app clipboard set into `pane`'s current directory: a
    /// **Copy**, or a **Move** when it was Cut (then the set clears). Reuses the
    /// FILEMGR-2 queue via [`submit_transfer`](Self::submit_transfer). `None` when
    /// nothing is staged or the pane has no writable directory.
    pub fn clip_paste(&mut self, pane: usize) -> Option<OpId> {
        let entry = self.clipboard.clone()?;
        let dest = self.pane(pane).active_tab().current_dir()?;
        let id = self.submit_transfer(entry.paths, dest, !entry.cut);
        if entry.cut {
            self.clipboard = None;
        }
        Some(id)
    }

    /// Paste **from the shared shell clipboard text** (a Ctrl+V `Event::Paste`,
    /// so a path copied in ANY surface pastes into Files). Parses the text into
    /// existing absolute paths and queues a transfer into `pane`'s directory: a
    /// Move when the pasted set is exactly what Files Cut (then clears it), else a
    /// Copy. `None` when the text holds no real paths, or the pane has no dir.
    pub fn clip_paste_text(&mut self, pane: usize, text: &str) -> Option<OpId> {
        let paths = parse_clip_paths(text);
        if paths.is_empty() {
            return None;
        }
        let dest = self.pane(pane).active_tab().current_dir()?;
        let is_cut_set = self
            .clipboard
            .as_ref()
            .is_some_and(|c| c.cut && c.paths == paths);
        let id = self.submit_transfer(paths, dest, !is_cut_set);
        if is_cut_set {
            self.clipboard = None;
        }
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

    /// Cancel a running op (rolls back its in-flight item). Also releases any
    /// collision prompt it's parked on, so a cancel-during-conflict takes effect.
    pub fn cancel_op(&mut self, op_id: OpId) {
        self.ops.cancel(op_id);
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

    // ── the operation dialogs (FILEMGR-11) ───────────────────────────────────

    /// The collision an in-flight op is blocked on, if the user hasn't answered
    /// it — the op id + the [`Conflict`](mde_files::opqueue::Conflict) the conflict
    /// dialog renders. Populated from the FILEMGR-2 channel resolver by
    /// [`pump_ops`](Self::pump_ops).
    #[must_use]
    pub fn pending_conflict(&self) -> Option<(OpId, &mde_files::opqueue::Conflict)> {
        self.ops.pending_conflict()
    }

    /// `true` while any op is parked on a collision (keeps a repaint heartbeat).
    #[must_use]
    pub fn any_pending_conflict(&self) -> bool {
        self.ops.any_pending_conflict()
    }

    /// Answer the collision op `op_id` is blocked on — the user's
    /// Overwrite/Skip/Keep-both pick, and whether it applies to every remaining
    /// collision in this op (the apply-to-all checkbox). Unparks the worker.
    pub fn resolve_conflict(&mut self, op_id: OpId, resolution: Resolution, apply_to_all: bool) {
        self.ops.answer_conflict(
            op_id,
            ConflictChoice {
                resolution,
                apply_to_all,
            },
        );
    }

    /// Open the permanent-delete confirm for `pane`'s current selection (lock 3/6
    /// — no trash, no undo). When any target lives on a remote / escalated mesh
    /// mount the confirm demands typed-arming (lock 19); a local delete needs only
    /// the confirm itself. An empty selection is an honest note, not a dialog.
    pub fn request_delete(&mut self, pane: usize) {
        let targets = self.pane(pane).active_tab().selected_paths();
        if targets.is_empty() {
            self.last_note = Some("Nothing selected to delete.".to_string());
            return;
        }
        let names = targets
            .iter()
            .map(|p| {
                p.file_name().map_or_else(
                    || p.display().to_string(),
                    |s| s.to_string_lossy().into_owned(),
                )
            })
            .collect();
        let arming = self.arming_for(&targets);
        self.confirm_delete = Some(ConfirmDelete::new(targets, names, arming));
    }

    /// The pending permanent-delete confirm, if one is open.
    #[must_use]
    pub fn pending_delete(&self) -> Option<&ConfirmDelete> {
        self.confirm_delete.as_ref()
    }

    /// Record a keystroke in the confirm's typed-arming echo field.
    pub fn set_delete_echo(&mut self, text: String) {
        if let Some(cd) = self.confirm_delete.as_mut() {
            cd.echo = text;
        }
    }

    /// Fire the pending delete if it is armed (a local delete always is; a
    /// remote / escalated one needs the typed node echo). Submits an
    /// [`OpKind::Delete`] to the queue and closes the confirm. A no-op while
    /// unarmed (the view keeps the button disabled too).
    pub fn confirm_delete(&mut self) {
        let armed = self
            .confirm_delete
            .as_ref()
            .is_some_and(ConfirmDelete::armed);
        if !armed {
            return;
        }
        let Some(cd) = self.confirm_delete.take() else {
            return;
        };
        let count = cd.count();
        let noun = if count == 1 { "item" } else { "items" };
        self.ops.submit(
            OpKind::Delete { items: cd.targets },
            format!("Permanently deleting {count} {noun}"),
        );
        self.last_note = None;
    }

    /// Dismiss the delete confirm without deleting anything.
    pub fn cancel_delete(&mut self) {
        self.confirm_delete = None;
    }

    /// Open the Properties dialog for `pane`'s focused selection (lock 8). Reads
    /// the entry's metadata through the FILEMGR-1 [`FileOps`] seam and offers the
    /// rwx grid + octal + owner/group; the chown control is offered only when this
    /// caller may (`chown_permitted`). A pathless (virtual peer) row or a stat
    /// failure is an honest note, never a half-open dialog.
    pub fn open_properties(&mut self, pane: usize) {
        let (path, name) = {
            let Some(row) = self.pane(pane).active_tab().focused_row() else {
                self.last_note = Some("Select a file to see its properties.".to_string());
                return;
            };
            let Some(path) = row.path.clone() else {
                self.last_note = Some(
                    "This entry has no local path \u{2014} mount the peer to inspect it."
                        .to_string(),
                );
                return;
            };
            (path, row.name.clone())
        };
        match PropertiesDialog::load(
            self.meta_ops.as_ref(),
            PathBuf::from(&path),
            name,
            self.chown_permitted,
        ) {
            Ok(dlg) => {
                self.properties = Some(dlg);
                self.last_note = None;
            }
            Err(e) => self.last_note = Some(format!("Couldn't read properties: {e}")),
        }
    }

    /// The open Properties dialog, if any (the view renders it).
    #[must_use]
    pub fn properties(&self) -> Option<&PropertiesDialog> {
        self.properties.as_ref()
    }

    /// Toggle one rwx grid cell in the open Properties dialog.
    pub fn properties_toggle_perm(&mut self, class: PermClass, perm: Perm) {
        if let Some(d) = self.properties.as_mut() {
            d.toggle_perm(class, perm);
        }
    }

    /// Update the Properties dialog's octal field (moves the grid when valid).
    pub fn properties_set_octal(&mut self, text: String) {
        if let Some(d) = self.properties.as_mut() {
            d.set_octal_edit(text);
        }
    }

    /// Update the Properties dialog's owner uid text (only heeded on Apply when
    /// chown is permitted).
    pub fn properties_set_uid(&mut self, text: String) {
        if let Some(d) = self.properties.as_mut() {
            d.uid_edit = text;
        }
    }

    /// Update the Properties dialog's owner gid text.
    pub fn properties_set_gid(&mut self, text: String) {
        if let Some(d) = self.properties.as_mut() {
            d.gid_edit = text;
        }
    }

    /// Apply the Properties dialog's pending chmod / chown through the
    /// [`FileOps`] seam, then reload the pane so the listing reflects any change.
    pub fn properties_apply(&mut self, pane: usize) {
        if let Some(d) = self.properties.as_mut() {
            d.apply(self.meta_ops.as_ref());
        }
        self.reload(pane);
    }

    /// Close the Properties dialog.
    pub fn close_properties(&mut self) {
        self.properties = None;
    }

    /// The mesh node a local `path` belongs to, if it sits under a mesh mount:
    /// the stable `/run/user/<uid>/mde-mesh/<host>` root (lock 11) or any
    /// worker-published mountpoint. `None` for a genuinely-local path.
    #[must_use]
    fn mount_host_for_path(&self, path: &str) -> Option<String> {
        if let Some(rest) = path.split("/mde-mesh/").nth(1) {
            let host = rest.split('/').next().unwrap_or("");
            if !host.is_empty() {
                return Some(host.to_string());
            }
        }
        self.mounts.iter().find_map(|(host, m)| {
            let mp = m.path.as_deref()?;
            let hit = path == mp
                || path
                    .strip_prefix(mp)
                    .is_some_and(|rest| rest.starts_with('/'));
            hit.then(|| host.clone())
        })
    }

    /// The typed-arming challenge a delete of `targets` demands, or `None` when
    /// every target is local (no arming). The first target on a remote /
    /// escalated mount decides the node to type + whether it's a full-fs mount
    /// (lock 19).
    fn arming_for(&self, targets: &[PathBuf]) -> Option<Arming> {
        targets.iter().find_map(|p| {
            let s = p.to_string_lossy();
            let host = self.mount_host_for_path(&s)?;
            let full_fs = self.mounts.get(&host).is_some_and(MountView::is_full);
            Some(Arming {
                node: host,
                full_fs,
                path: s.into_owned(),
            })
        })
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

    // ── context-menu Send-To + Send-in-Chat (FILEMGR-12) ────────────────────

    /// Right-click **Send to → `<peer>`**: send `pane`'s whole selection to
    /// `peer_id` over the mesh. Reuses the shipped transfer path — a canonical
    /// [`SendToRequest`] (the [`SendToEntry::ContextMenu`] entry from the locked
    /// six-set) dispatched through [`Backend::send_to`](mde_files::backend::Backend::send_to),
    /// the same FILEMGR-7 direct-transfer wire the toolbar Send-To uses. `None`
    /// (with an honest note) when nothing local is selected.
    pub fn send_to_peer(
        &mut self,
        pane: usize,
        peer_id: &str,
    ) -> Option<Result<OpId, BackendError>> {
        let sources = self.pane(pane).active_tab().selected_paths();
        if sources.is_empty() {
            self.last_note = Some(
                "Nothing to send — select a local file first (mesh files need a mount).".into(),
            );
            return None;
        }
        let file = describe_sources(&sources);
        let req = SendToRequest::copy_ask(
            sources,
            Destination::Peer(peer_id.to_string()),
            SendToEntry::ContextMenu,
        );
        let result = self.dispatch(req);
        self.last_send = match &result {
            Ok(op_id) => SendOutcome::Sent {
                op_id: *op_id,
                file,
                peer: peer_id.to_string(),
            },
            Err(e) => SendOutcome::Failed(e.to_string()),
        };
        Some(result)
    }

    /// Right-click **Send in Chat → `<peer>`**: move the bytes AND drop a file
    /// card into the peer's NOTIFY-CHAT conversation. Two reuses, no new
    /// mechanism: (1) the real transfer runs through the same
    /// [`send_to_peer`](Self::send_to_peer) mesh path; (2) each file is offered to
    /// the conversation via the [`ChatBridge`] as the `mde-chat`
    /// [`MessageKind::File`](mde_chat::MessageKind::File) on `action/chat/send`
    /// (the worker folds it into a File card) — the offer keyed by the peer's
    /// **host** (the chat contact username). `None` when nothing local is selected.
    pub fn send_in_chat(
        &mut self,
        pane: usize,
        peer_id: &str,
    ) -> Option<Result<OpId, BackendError>> {
        let sources = self.pane(pane).active_tab().selected_paths();
        if sources.is_empty() {
            self.last_note = Some(
                "Nothing to send — select a local file first (mesh files need a mount).".into(),
            );
            return None;
        }
        // The chat contact is the peer's hostname (username = hostname, lock 2/21).
        let host = self
            .peers
            .iter()
            .find(|p| p.id == peer_id)
            .map(|p| p.host.clone());
        let file = describe_sources(&sources);
        // 1) the real transfer (reuse FILEMGR-7 Send-To over the mesh).
        let req = SendToRequest::copy_ask(
            sources.clone(),
            Destination::Peer(peer_id.to_string()),
            SendToEntry::ContextMenu,
        );
        let result = self.dispatch(req);
        // 2) hand each file to the conversation as the reused chat file-kind, but
        //    only once the transfer was actually accepted (no card for a no-op).
        if let (true, Some(host)) = (result.is_ok(), host.as_deref().filter(|h| !h.is_empty())) {
            for src in &sources {
                self.chat.offer_file(host, src);
            }
        }
        self.last_send = match &result {
            Ok(op_id) => SendOutcome::Sent {
                op_id: *op_id,
                file: format!("{file} (in chat)"),
                peer: host.unwrap_or_else(|| peer_id.to_string()),
            },
            Err(e) => SendOutcome::Failed(e.to_string()),
        };
        Some(result)
    }
}

/// A short label for a Send-To/Send-in-Chat status line: the single file's name,
/// or "N items" for a multi-selection.
fn describe_sources(sources: &[PathBuf]) -> String {
    match sources {
        [one] => one.file_name().map_or_else(
            || one.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ),
        many => format!("{} items", many.len()),
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
        assert!(b.last_note().is_some());
    }
}
