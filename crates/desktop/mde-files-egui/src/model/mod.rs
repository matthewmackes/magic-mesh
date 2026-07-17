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
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use mde_egui::search_omnibox::{SearchDomain, SearchItem};
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
use crate::transfers::{
    build_targets, display_order, FileTransfers, LedgerCounts, Method, NewTransferForm,
    TransferFilter, TransferJob, TransferTarget, TransferVerb, TransfersClient,
};
use mde_files::opqueue::{ConflictChoice, Resolution};

/// How often the Mesh sidebar re-reads `state/mesh-mount/*` from the Bus. The read
/// is a cheap local spool scan; a worker transition surfaces within this window.
/// Matches the other Bus surfaces' cadence.
const MOUNT_POLL: Duration = Duration::from_secs(2);
const HOME_SEARCH_LIMIT: usize = 32;

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

fn truncate_operation_label(label: &str) -> String {
    const MAX_CHARS: usize = 42;
    let char_count = label.chars().count();
    if char_count <= MAX_CHARS {
        return label.to_owned();
    }
    let mut out = label
        .chars()
        .take(MAX_CHARS.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
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

fn file_search_target_label(row: &FileRow, location: &Location) -> String {
    if let Some(path) = &row.path {
        return path.clone();
    }
    let base = location.backend_path();
    let name = row.name.trim_end_matches('/');
    if base == "/" {
        format!("/{name}")
    } else {
        format!("{base}/{name}")
    }
}

fn file_search_terms(row: &FileRow, location: &Location) -> Vec<String> {
    let mut terms = vec![
        mime_search_label(row.mime).to_string(),
        row.size.clone(),
        row.age.clone(),
        location.backend_path(),
    ];
    if row.is_dir() {
        terms.push("folder".to_string());
        terms.push("directory".to_string());
    } else {
        terms.push("file".to_string());
    }
    if let Some(peer) = &row.mesh {
        terms.push(peer.clone());
    }
    if let Some(peer) = &row.from {
        terms.push(peer.clone());
    }
    if row.has_conflict {
        terms.push("conflict".to_string());
    }
    if let Some(sibling) = &row.conflict_sibling {
        terms.push(sibling.clone());
    }
    if row.syncing {
        terms.push("syncing".to_string());
    }
    terms
}

fn file_search_item(
    pane: usize,
    row_ix: usize,
    row: &FileRow,
    location: &Location,
    source_rank: usize,
) -> SearchItem<FileSearchTarget> {
    SearchItem::new(
        SearchDomain::File,
        row.name.clone(),
        file_search_target_label(row, location),
        FileSearchTarget {
            pane,
            row: row_ix,
            path: row.path.as_deref().map(PathBuf::from),
        },
    )
    .with_terms(file_search_terms(row, location))
    .with_source_rank(source_rank)
}

fn file_search_key(item: &SearchItem<FileSearchTarget>) -> String {
    item.payload
        .path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| item.target.clone())
}

const fn mime_search_label(mime: Mime) -> &'static str {
    match mime {
        Mime::Folder => "folder",
        Mime::Doc => "document",
        Mime::Image => "image",
        Mime::Pdf => "pdf",
        Mime::Archive => "archive",
        Mime::Disk => "disk image",
    }
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

/// Payload carried by Files candidates in the shared search/omnibox ranker.
///
/// The row index deliberately points back into the pane's displayed rows: current
/// folders and recursive-search result sets already share that model, and
/// activation must keep using [`FileBrowser::open_row`] instead of duplicating
/// file/directory behavior in the search layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSearchTarget {
    /// The pane whose active tab owns the row.
    pub pane: usize,
    /// The displayed row index in that pane's active tab.
    pub row: usize,
    /// The row's absolute path when this is a real local/mounted file.
    pub path: Option<PathBuf>,
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
// Surface tab (Files ↔ Transfers) — TRANSFERS-8.
// ═══════════════════════════════════════════════════════════════════════════

/// How often the Transfers tab re-reads the worker's ledger from the node-local
/// store. A cheap local directory scan (never a peer probe), so a worker
/// transition surfaces within this window — matches the mesh-mount cadence.
const TRANSFERS_POLL: Duration = Duration::from_secs(2);

/// Which top-level surface the File Browser is showing (Q1).
///
/// The Transfers **tab inside** the File Browser: the `MenuBar` + sidebar are
/// shared across both (Q16); only the central content switches (the file panes
/// vs. the transfers ledger).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SurfaceTab {
    /// The file panes (the classic File Browser).
    #[default]
    Files,
    /// The transfers ledger (the TRANSFERS-8 renderer).
    Transfers,
}

impl SurfaceTab {
    /// The two surface tabs, in strip order.
    pub const ALL: [Self; 2] = [Self::Files, Self::Transfers];

    /// The tab-strip label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Transfers => "Transfers",
        }
    }
}

/// A compact, reusable summary of active file work for shell chrome. It folds both
/// the local op queue and the daemon transfer ledger without exposing either internal
/// model to the bottom navigation bar.
#[derive(Debug, Clone, PartialEq)]
pub struct OperationProgressSummary {
    /// Active file jobs: queued/running/paused transfer jobs plus local ops that are
    /// not dismissed/done.
    pub active: usize,
    /// The number of active jobs that reported a real percentage.
    pub known_progress: usize,
    /// Average of known real percentages, `None` while all active jobs are still
    /// queued/starting/otherwise unknown.
    pub fraction: Option<f32>,
    /// Short human label for the global status strip.
    pub label: String,
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
    /// TRANSFERS-8 — which top-level surface is showing (Files ↔ Transfers, Q1).
    surface_tab: SurfaceTab,
    /// TRANSFERS-8 — the transfers worker client (reads the ledger, submits typed
    /// verbs over the node-local inbox). Injectable so the model is unit-tested
    /// headless; production is [`FileTransfers::from_env`].
    transfers: Box<dyn TransfersClient>,
    /// TRANSFERS-8 — the latest ledger snapshot, refreshed on the [`TRANSFERS_POLL`]
    /// cadence (a job appearing / a state change surfaces within the window).
    transfers_jobs: Vec<TransferJob>,
    /// When the ledger was last polled (drives the fixed cadence).
    last_transfers_poll: Option<Instant>,
    /// TRANSFERS-8 — the Transfers tab's live view filter (state + method, Q16).
    transfers_filter: TransferFilter,
    /// TRANSFERS-8 — the open New Transfer dialog's entry state, if any (Q13).
    new_transfer: Option<NewTransferForm>,
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
            surface_tab: SurfaceTab::default(),
            transfers: Box::new(FileTransfers::from_env()),
            transfers_jobs: Vec::new(),
            last_transfers_poll: None,
            transfers_filter: TransferFilter::default(),
            new_transfer: None,
        };
        me.refresh_roster();
        me.reload(0);
        me.reload(1);
        me
    }

    /// Swap in an explicit [`TransfersClient`] (TRANSFERS-8). Tests inject a
    /// [`FakeTransfers`](crate::transfers::test_support::FakeTransfers) to assert
    /// the exact verb the surface emitted; production keeps the [`FileTransfers`]
    /// from [`Self::with_file_ops`]. Re-reads the ledger through the new client so
    /// the tab reflects it immediately.
    #[must_use]
    pub fn with_transfers(mut self, transfers: Box<dyn TransfersClient>) -> Self {
        self.transfers = transfers;
        self.read_transfers();
        self
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
            self.last_note = Some(format!("{host} is offline - can't mount it."));
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
            self.last_note = Some(format!("{host} is offline - can't escalate it."));
            return;
        }
        match self.mesh.request(host, MeshMountVerb::Escalate) {
            Ok(()) => {
                self.last_note = Some(format!("Escalating {host} to full-filesystem access..."));
            }
            Err(e) => self.last_note = Some(e),
        }
        self.read_mounts();
    }

    /// Unmount peer `host` (the `unmount` verb) — tears the mount down + forgets
    /// it. The sidebar's eject control.
    pub fn unmount_peer(&mut self, host: &str) {
        match self.mesh.request(host, MeshMountVerb::Unmount) {
            Ok(()) => self.last_note = Some(format!("Unmounting {host}...")),
            Err(e) => self.last_note = Some(e),
        }
        self.read_mounts();
    }

    // ── transfers (TRANSFERS-8 — the Transfers tab + the three entry points) ──

    /// Which top-level surface is showing (Files ↔ Transfers, Q1).
    #[must_use]
    pub fn surface_tab(&self) -> SurfaceTab {
        self.surface_tab
    }

    /// Switch the top-level surface (the tab strip). Switching to Transfers reads
    /// the ledger immediately so the tab is live on the first frame.
    pub fn set_surface_tab(&mut self, tab: SurfaceTab) {
        self.surface_tab = tab;
        if tab == SurfaceTab::Transfers {
            self.read_transfers();
        }
    }

    /// Re-read the worker's ledger into the cache. A local directory scan — never a
    /// peer probe — so it can't hang the UI (mirrors [`read_mounts`](Self::read_mounts)).
    fn read_transfers(&mut self) {
        self.transfers_jobs = self.transfers.jobs();
        self.last_transfers_poll = Some(Instant::now());
    }

    /// Refresh the ledger on the [`TRANSFERS_POLL`] cadence (call once per frame; it
    /// self-gates, so it's cheap to call every frame). A submitted job / a state
    /// change surfaces within the window.
    pub fn pump_transfers(&mut self) {
        let due = self
            .last_transfers_poll
            .is_none_or(|t| t.elapsed() >= TRANSFERS_POLL);
        if due {
            self.read_transfers();
        }
    }

    /// `true` while the ledger holds an in-flight job — the view keeps a repaint
    /// heartbeat alive so live progress updates without input.
    #[must_use]
    pub fn transfers_active(&self) -> bool {
        self.transfers_jobs.iter().any(|j| j.state.is_active())
    }

    /// Whether a transfers worker has ever run on this node (drives the `EmptyState`'s
    /// "no worker" vs. "no jobs yet" honesty, §7).
    #[must_use]
    pub fn transfers_worker_present(&self) -> bool {
        self.transfers.worker_present()
    }

    /// The Transfers tab's live view filter (state + method, Q16).
    #[must_use]
    pub fn transfers_filter(&self) -> TransferFilter {
        self.transfers_filter
    }

    /// Replace the Transfers view filter (the `MenuBar`'s View-by-state / -by-method).
    pub fn set_transfers_filter(&mut self, filter: TransferFilter) {
        self.transfers_filter = filter;
    }

    /// The ledger jobs in **newest-relevant** display order under the current
    /// filter — the list the Transfers tab renders (Q1: "live progress list").
    #[must_use]
    pub fn transfers_view(&self) -> Vec<TransferJob> {
        display_order(&self.transfers_jobs, &self.transfers_filter)
    }

    /// The unfiltered ledger tallies (drive the `MenuBar`'s control gating + the
    /// active-count badge).
    #[must_use]
    pub fn transfers_counts(&self) -> LedgerCounts {
        LedgerCounts::of(&self.transfers_jobs)
    }

    /// A compact active-work summary for shell chrome. This is the one model the
    /// platform bottom rail can reuse for file operations instead of each caller
    /// hand-rolling a progress strip.
    #[must_use]
    pub fn operation_progress_summary(&self) -> Option<OperationProgressSummary> {
        let mut active = 0;
        let mut local_active = 0;
        let mut transfer_active = 0;
        let mut known_progress = 0;
        let mut progress_total = 0.0;
        let mut first_label: Option<String> = None;

        for op in self.ops.active().iter().filter(|op| !op.is_done()) {
            active += 1;
            local_active += 1;
            if first_label.is_none() {
                first_label = Some(op.label.clone());
            }
            if let Some(progress) = &op.progress {
                known_progress += 1;
                progress_total += progress.fraction().clamp(0.0, 1.0);
            }
        }

        for job in self
            .transfers_jobs
            .iter()
            .filter(|job| job.state.is_active())
        {
            active += 1;
            transfer_active += 1;
            if first_label.is_none() {
                first_label = Some(job.route());
            }
            if let Some(progress) = job.progress {
                known_progress += 1;
                progress_total += (f32::from(progress) / 100.0).clamp(0.0, 1.0);
            }
        }

        if active == 0 {
            return None;
        }

        let label = if active == 1 {
            first_label.unwrap_or_else(|| "File operation".to_owned())
        } else if local_active > 0 && transfer_active > 0 {
            format!("{active} file operations")
        } else if local_active > 0 {
            format!("{local_active} local file operations")
        } else {
            format!("{transfer_active} transfers")
        };

        Some(OperationProgressSummary {
            active,
            known_progress,
            fraction: (known_progress > 0).then_some(progress_total / known_progress as f32),
            label: truncate_operation_label(&label),
        })
    }

    /// The auto-only destination registry (Q10): the two standing node-state
    /// targets (Music Library / Mesh Share) plus one per **reachable** peer. The
    /// drop / "Send to →" / dialog entry points target these.
    #[must_use]
    pub fn transfer_targets(&self) -> Vec<TransferTarget> {
        let peers: Vec<(String, String)> = self
            .reachable_destinations()
            .into_iter()
            .map(|p| (p.id.clone(), p.host.clone()))
            .collect();
        build_targets(&peers)
    }

    /// Submit a client-minted job to the worker (the one path every entry point
    /// funnels through). Records an honest status note either way (§7).
    fn submit_transfer_job(&mut self, job: TransferJob) {
        let route = job.route();
        match self.transfers.dispatch(&TransferVerb::Submit(job)) {
            Ok(()) => self.last_note = Some(format!("Queued transfer {route}")),
            Err(e) => self.last_note = Some(e),
        }
    }

    /// **Entry point 1 (Q13) — the New Transfer dialog.** Open a blank dialog.
    pub fn open_new_transfer(&mut self) {
        self.new_transfer = Some(NewTransferForm::default());
    }

    /// Open the New Transfer dialog pre-pointed at a chosen destination + method
    /// (the drop / "Send to →" entry points route here when there's no selection to
    /// submit outright — the user fills only the source).
    pub fn open_new_transfer_to(&mut self, dest: impl Into<String>, method: Method) {
        self.new_transfer = Some(NewTransferForm::to(dest, method));
    }

    /// The open New Transfer dialog's entry state, if any.
    #[must_use]
    pub fn new_transfer(&self) -> Option<&NewTransferForm> {
        self.new_transfer.as_ref()
    }

    /// Commit an edit to the New Transfer form (render → intents → apply, like the
    /// search form).
    pub fn set_new_transfer_form(&mut self, form: NewTransferForm) {
        if self.new_transfer.is_some() {
            self.new_transfer = Some(form);
        }
    }

    /// Close the New Transfer dialog without submitting.
    pub fn cancel_new_transfer(&mut self) {
        self.new_transfer = None;
    }

    /// Submit the New Transfer dialog's job (if complete) and close it. A blank /
    /// incomplete form is an honest no-op (the Submit button is disabled behind
    /// [`NewTransferForm::runnable`], this is the belt-and-braces guard).
    pub fn submit_new_transfer(&mut self) {
        let Some(job) = self.new_transfer.as_ref().and_then(NewTransferForm::to_job) else {
            return;
        };
        self.submit_transfer_job(job);
        self.new_transfer = None;
        self.read_transfers();
    }

    /// **Entry point 2 (Q13) — right-click "Send to → `<target>`".** Submit
    /// `pane`'s selected local files to `target` (one job per file). `None` (with an
    /// honest note) when nothing local is selected — a peer/virtual row carries no
    /// path.
    pub fn send_to_target(&mut self, pane: usize, target: &TransferTarget) {
        let sources = self.pane(pane).active_tab().selected_paths();
        if sources.is_empty() {
            self.last_note = Some(
                "Nothing to send — select a local file first (mesh files need a mount).".into(),
            );
            return;
        }
        self.submit_sources_to(&sources, target);
    }

    /// **Entry point 3 (Q13) — drag-drop onto a destination.** Submit
    /// `source_pane`'s selection to `target` (one job per file). `None` (with a
    /// note) when the selection carries no filesystem path.
    pub fn drop_on_target(&mut self, source_pane: usize, target: &TransferTarget) {
        let sources = self.pane(source_pane).active_tab().selected_paths();
        if sources.is_empty() {
            self.last_note =
                Some("Nothing to transfer — mesh/peer files need a mount (FILEMGR-9).".into());
            return;
        }
        self.submit_sources_to(&sources, target);
    }

    /// Submit one job per source path to `target` (the shared body of the "Send
    /// to →" + drag-drop entry points). Each job rides the target's method + dest.
    fn submit_sources_to(&mut self, sources: &[PathBuf], target: &TransferTarget) {
        let mut queued = 0usize;
        let mut last_err = None;
        for src in sources {
            let job = TransferJob::new(
                src.to_string_lossy().into_owned(),
                target.dest.clone(),
                target.method,
                crate::transfers::TransferPolicy::default(),
            );
            match self.transfers.dispatch(&TransferVerb::Submit(job)) {
                Ok(()) => queued += 1,
                Err(e) => last_err = Some(e),
            }
        }
        self.last_note = Some(last_err.unwrap_or_else(|| {
            let noun = if queued == 1 { "transfer" } else { "transfers" };
            format!("Queued {queued} {noun} \u{2192} {}", target.label)
        }));
        self.read_transfers();
    }

    /// Lifecycle: pause one job (its ledger row's control). An illegal/absent verb
    /// is honestly refused by the daemon (never a silent GUI no-op).
    pub fn transfer_pause(&mut self, id: &str) {
        self.dispatch_transfer_lifecycle(&TransferVerb::Pause(id.to_string()));
    }

    /// Lifecycle: resume one Paused job.
    pub fn transfer_resume(&mut self, id: &str) {
        self.dispatch_transfer_lifecycle(&TransferVerb::Resume(id.to_string()));
    }

    /// Lifecycle: cancel one job (removes it from the ledger + frees any slot).
    pub fn transfer_cancel(&mut self, id: &str) {
        self.dispatch_transfer_lifecycle(&TransferVerb::Cancel(id.to_string()));
    }

    /// **Pause-all** (Q16 menu): pause every pausable job (Queued/Running).
    pub fn transfer_pause_all(&mut self) {
        let ids: Vec<String> = self
            .transfers_jobs
            .iter()
            .filter(|j| j.state.can_pause())
            .map(|j| j.id.clone())
            .collect();
        self.dispatch_transfer_batch(&ids, TransferVerb::Pause);
    }

    /// **Resume-all** (Q16 menu): resume every Paused job.
    pub fn transfer_resume_all(&mut self) {
        let ids: Vec<String> = self
            .transfers_jobs
            .iter()
            .filter(|j| j.state.can_resume())
            .map(|j| j.id.clone())
            .collect();
        self.dispatch_transfer_batch(&ids, TransferVerb::Resume);
    }

    /// **Clear-completed** (Q16 menu): cancel every terminal job (Done/Failed) — a
    /// cancel removes the row, which is how the worker clears history (there is no
    /// distinct clear verb; cancel is legal from any state).
    pub fn transfer_clear_completed(&mut self) {
        let ids: Vec<String> = self
            .transfers_jobs
            .iter()
            .filter(|j| j.state.is_terminal())
            .map(|j| j.id.clone())
            .collect();
        self.dispatch_transfer_batch(&ids, TransferVerb::Cancel);
    }

    /// Dispatch one lifecycle verb + refresh the ledger, recording an honest note
    /// on a store error.
    fn dispatch_transfer_lifecycle(&mut self, verb: &TransferVerb) {
        if let Err(e) = self.transfers.dispatch(verb) {
            self.last_note = Some(e);
        }
        self.read_transfers();
    }

    /// Dispatch a batch of the same lifecycle verb (Pause-all / Resume-all /
    /// Clear-completed), then refresh. An empty batch is a no-op.
    fn dispatch_transfer_batch(&mut self, ids: &[String], verb: impl Fn(String) -> TransferVerb) {
        let mut last_err = None;
        for id in ids {
            if let Err(e) = self.transfers.dispatch(&verb(id.clone())) {
                last_err = Some(e);
            }
        }
        if let Some(e) = last_err {
            self.last_note = Some(e);
        }
        self.read_transfers();
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

    /// Shared omnibox candidates for `pane`'s active tab.
    ///
    /// This is intentionally a projection of the Files model's current rows,
    /// not a filesystem crawl. A normal folder listing and a running/finished
    /// recursive search both render through the same rows, so the shared ranker
    /// sees current-folder entries and streamed search hits with no fake indexer.
    #[must_use]
    pub fn search_omnibox_items(&self, pane: usize) -> Vec<SearchItem<FileSearchTarget>> {
        if pane > 1 {
            return Vec::new();
        }
        let tab = self.panes[pane].active_tab();
        tab.rows()
            .iter()
            .enumerate()
            .map(|(row_ix, row)| file_search_item(pane, row_ix, row, tab.location(), row_ix))
            .collect()
    }

    /// Shared omnibox candidates for the focused pane.
    #[must_use]
    pub fn active_search_omnibox_items(&self) -> Vec<SearchItem<FileSearchTarget>> {
        self.search_omnibox_items(self.active_pane)
    }

    /// Shared omnibox candidates for local home entries, even when Files is not
    /// currently displaying the home folder.
    ///
    /// This is a bounded snapshot from the existing backend `list()` seam, not a
    /// crawler or persistent index. It gives the shell front door the first
    /// local-file slice required by unified search while keeping activation in
    /// the Files model and keeping private paths local to this process.
    #[must_use]
    pub fn home_search_omnibox_items(&self) -> Vec<SearchItem<FileSearchTarget>> {
        self.home_search_omnibox_items_with_rank(0)
    }

    fn home_search_omnibox_items_with_rank(
        &self,
        rank_base: usize,
    ) -> Vec<SearchItem<FileSearchTarget>> {
        let location = Location::Local(Self::HOME.to_string());
        self.backend
            .list(Self::HOME)
            .into_iter()
            .take(HOME_SEARCH_LIMIT)
            .enumerate()
            .map(|(row_ix, row)| {
                file_search_item(
                    self.active_pane,
                    row_ix,
                    &row,
                    &location,
                    rank_base + row_ix,
                )
            })
            .collect()
    }

    /// Combined local file candidates for the shell front door.
    ///
    /// The focused pane stays first because those entries are closest to the
    /// user's current workflow. Home entries are appended and de-duplicated by
    /// path/target so opening Files on Home does not produce doubled results.
    #[must_use]
    pub fn unified_search_omnibox_items(&self) -> Vec<SearchItem<FileSearchTarget>> {
        let mut items = self.active_search_omnibox_items();
        let mut seen: HashSet<String> = items.iter().map(file_search_key).collect();
        for item in self.home_search_omnibox_items_with_rank(items.len()) {
            if seen.insert(file_search_key(&item)) {
                items.push(item);
            }
        }
        items
    }

    /// Activate a Files omnibox payload through the same path as Enter/double-click.
    pub fn open_search_omnibox_target(&mut self, target: &FileSearchTarget) {
        if target.pane <= 1 {
            self.set_active_pane(target.pane);
            let current_matches_path = target.path.as_ref().is_some_and(|path| {
                let ti = self.tab_index(target.pane);
                self.panes[target.pane].tabs[ti]
                    .rows
                    .get(target.row)
                    .and_then(|row| row.path.as_deref())
                    .is_some_and(|row_path| Path::new(row_path) == path.as_path())
            });
            if target.path.is_none() || current_matches_path {
                self.open_row(target.pane, target.row);
            } else if let Some(path) = &target.path {
                self.open_path_target(target.pane, path);
            }
        }
    }

    fn open_path_target(&mut self, pane: usize, path: &Path) {
        let Some(parent) = path.parent() else {
            return;
        };
        self.navigate(pane, Location::Local(parent.to_string_lossy().into_owned()));
        let target = path.to_string_lossy();
        let ti = self.tab_index(pane);
        if let Some(idx) = self.panes[pane].tabs[ti]
            .rows
            .iter()
            .position(|row| row.path.as_deref() == Some(target.as_ref()))
        {
            self.open_row(pane, idx);
        }
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
                    "This entry has no local path - mount the peer to inspect it.".to_string(),
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

    /// EDITOR-9 — "Send-to-Editor": post the pane's focused **local** file onto
    /// [`ACTION_EDITOR_OPEN`](mde_files::editor_open::ACTION_EDITOR_OPEN) so the one
    /// Quazar shell's editor mount opens it (`EditorSurface::open_path`) — the same
    /// persist-first Bus verb pattern as Send-in-Chat (§6 reuse). A no-op with an
    /// honest note when nothing local is focused (peer/virtual rows carry no path),
    /// and a silent no-op on a node with no Bus.
    pub fn send_to_editor(&mut self, pane: usize) {
        let Some(path) = self
            .pane(pane)
            .active_tab()
            .focused_row()
            .and_then(|row| row.path.clone())
            .map(PathBuf::from)
        else {
            self.last_note = Some(
                "Nothing to open — select a local file first (mesh files need a mount).".into(),
            );
            return;
        };
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        mde_files::editor_open::BusEditorLaunch::from_env().send(&path);
        self.last_note = Some(format!("Sent {name} to the Editor."));
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

mod sort;
use sort::*;

#[cfg(test)]
mod tests;
