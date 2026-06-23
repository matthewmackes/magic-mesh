//! libcosmic application — top-level State, Message, update, view (GUI-7).
//!
//! Ported off iced 0.14 onto `cosmic::Application` (the maximize-Cosmic-native
//! cutover). The view/widget layer renders against `cosmic::iced` (libcosmic's
//! vendored iced fork), so the Carbon style closures port unchanged; the shell
//! is a `cosmic::Application` with a `Core`. The custom titlebar is kept
//! (Cosmic's headerbar is disabled in `init`).

use crate::cosmic_compat::{ContainerSty, TextSty};
use cosmic::app::ApplicationExt;
use cosmic::iced::widget::{column, container, row, scrollable};
use cosmic::iced::{window, Background, Border, Color, Length, Padding, Task};
use cosmic::{Application, Element};

use crate::backend::{
    Backend, BackendSnapshot, ConflictPolicy, Destination, RealBackend, SendMode,
};
use crate::model::{Layout, View};
use crate::panels::{
    ContextMenu, ContextMenuItem, DetailsPanel, DragSession, DragTarget, OpRow, OpState,
    OperationDrawer,
};
use crate::prefs::Accessibility;
use crate::selection::Selection;
use crate::send_to::SendToRequest;
use crate::theme as t;
use crate::views;

#[derive(Debug, Clone)]
pub enum Message {
    SelectView(View),
    /// E10.5 — open a fresh browser tab (Mesh overview) and switch to it.
    NewTab,
    /// E10.5 — close the tab at the given index (no-op when only one remains).
    CloseTab(usize),
    /// E10.5 — close the currently-active tab (Ctrl+W).
    CloseActiveTab,
    /// E10.5 — switch to the tab at the given index.
    SwitchTab(usize),
    /// E10.5 — cycle to the next tab, wrapping (Ctrl+Tab).
    CycleTab,
    /// E10 — navigate the Local browser to the parent directory.
    LocalUp,
    /// AFM-4 — activate a Local-browser row by its display name: descend into a
    /// directory, or open a file with its default app (`xdg-open`). The reducer
    /// resolves the name against the current `local_files` listing for the path.
    LocalActivate(String),
    /// E10 — jump the Local browser to an absolute path (sidebar quick-access:
    /// Home / XDG dirs / the GVfs network-mount dir for mounted SMB shares).
    LocalGoto(String),
    /// E10 — Network view: the SMB host box changed.
    NetHostChanged(String),
    /// E10 — Network view: browse the typed host's SMB shares (smbclient -L).
    NetBrowse,
    /// E10 — Network view: mount a share over GVfs + open it in the browser.
    NetMount(String),
    ToggleLocal,
    /// AFM-2 — collapse / expand the sidebar rail.
    ToggleSidebar,
    /// AFM-2 — the "+ Peer" footer button: launch the workbench (where mesh
    /// enrollment / peer registration lives).
    OpenRegistration,
    SetLayout(Layout),
    /// DENSITY-SYMMETRY — set the listing density (Compact / Comfortable /
    /// Spacious). Re-rhythms the file-list chrome via `FileListMetrics`.
    SetDensity(mde_theme::Density),
    SearchChanged(String),
    Refresh,
    TitlebarMinimize,
    TitlebarMaximize,
    TitlebarClose,
    PeerCardBrowse(String),
    PeerCardSend(String),
    PrimaryAction,
    /// AFM-9 — create a new folder in the current Local directory.
    MakeDir,
    /// v2.0.0 Phase 1.3 — plain click on a file row.
    RowClick(String),
    /// v2.0.0 Phase 1.3 — ctrl-click on a file row (toggle in
    /// selection).
    RowCtrlClick(String),
    /// v2.0.0 Phase 1.3 — shift-click on a file row. The view
    /// passes the visible row order so the selection model can
    /// build the inclusive range.
    RowShiftClick(String, Vec<String>),
    /// v2.0.0 Phase 1.3 — keyboard down / up arrows. The visible
    /// row order is supplied so wrap-around behaves correctly.
    FocusNext(Vec<String>),
    /// v2.0.0 Phase 1.3 — keyboard up arrow.
    FocusPrev(Vec<String>),
    /// v2.0.0 Phase 1.3 — keyboard space-bar: toggle focused row.
    ToggleFocused,
    /// v2.0.0 Phase 1.3 — keyboard Escape: clear selection.
    ClearSelection,
    /// v2.0.0 Phase 1.4 — toggle the right-side details panel.
    ToggleDetails,
    /// v2.0.0 Phase 1.5 — open the right-click context menu over
    /// the given row at the given window-coord anchor.
    OpenContextMenu(String, f32, f32),
    /// v2.0.0 Phase 1.5 — close the context menu.
    CloseContextMenu,
    /// v2.0.0 Phase 1.5 — a context-menu item was clicked. View
    /// code routes this to the appropriate side-effect (Send-To
    /// dialog, clipboard, etc.); the reducer just closes the
    /// menu so the floating widget disappears.
    ContextMenuItemClicked(ContextMenuItem),
    /// v2.0.0 Phase 1.7 — show / hide the operation drawer.
    ToggleOperationDrawer,
    /// v2.0.0 Phase 1.7 — backend pushed a fresh op row (new or
    /// progress update).
    OpRowUpsert(OpRow),
    /// v2.0.0 Phase 1.7 — dismiss a terminal op from the drawer.
    OpRowDismiss(u64),
    /// v2.0.0 Phase 1.6 — user grabbed a row (or the current
    /// selection) and started dragging.
    DragStart(Vec<String>),
    /// v2.0.0 Phase 1.6 — cursor entered (`Some`) or left (`None`)
    /// a drop target.
    DragHover(Option<DragTarget>),
    /// v2.0.0 Phase 1.6 — user dropped over a target (or empty
    /// space). The reducer translates a target landing into a
    /// `Backend::send_to` call at the view-side; here it just
    /// finishes the drag session.
    DragDrop,
    /// v2.0.0 Phase 1.6 — user pressed Escape mid-drag.
    DragCancel,
    /// v2.0.0 Phase 3.1 — canonical Send-To dispatch. Every
    /// entry point (toolbar / context menu / command palette /
    /// drag-drop / details panel / bulk-select bar) builds a
    /// `SendToRequest` + fires this message.
    SendTo(SendToRequest),
    /// v2.0.0 Phase 5.1 — Tab cycles keyboard focus through panes.
    TabFocus,
    /// v2.0.0 Phase 5.1 — Shift-Tab reverses.
    ShiftTabFocus,
    /// v2.0.0 Phase 5.1 — Ctrl/Cmd-F focuses the toolbar search
    /// field.
    FocusSearch,
    /// v2.0.0 Phase 5.1 — any keyboard input arrived. Flips
    /// `keyboard_active = true` so `FocusVisibility::Auto`
    /// renders rings.
    KeyboardActivity,
    /// v2.0.0 Phase 5.1 — mouse moved / clicked. Flips
    /// `keyboard_active = false`.
    PointerActivity,
    /// No-op message used by buttons that don't have a wired behaviour yet
    /// (e.g. the sidebar's panel-toggle, the peer card's `…` button).
    Noop,
    /// AF-mesh.3 (2026-05-24) — operator clicked into a sub-
    /// directory inside `View::MeshHomeChild`. The name is the
    /// row label (without the trailing `/` the renderer adds for
    /// folders). Pushes onto the path stack so the breadcrumb
    /// + the next backend list call reflect the descent.
    MeshFolderEnter(String),
    /// AF-mesh.3 — pop back up one level inside Mesh Home. Used
    /// by the toolbar back button + the parent-link breadcrumb
    /// click.
    MeshFolderUp,
    /// AF-mesh.3 — pop back to a specific depth (used by
    /// breadcrumb mid-segment clicks). 0 = the slug root.
    MeshFolderPop(usize),
    /// MESHFS-8.1 — trash listing loaded (or errored).
    UndeleteLoaded(Result<Vec<TrashItem>, String>),
    /// MESHFS-8.1 — user clicked "Restore" on a trash entry.
    RestoreTrashItem(String),
    /// MESHFS-8.1 — restore operation completed.
    TrashRestored(String, Result<(), String>),
    /// MESHFS-11.1 — raw JSON from `mackesd mesh-fs-status --json` loaded.
    MeshFsHealthLoaded(String),
    /// MESHFS-11.1 — user clicked the yellow conflict chip: open the
    /// resolve dialog. `(original_name, conflict_sibling_name)` — both
    /// are bare filenames, not full paths. The view knows the directory.
    ConflictResolve(String, String),
    /// MESHFS-11.1 — user dismissed the resolve dialog without action.
    DismissConflictDialog,
    /// MESHFS-11.1 — archive one side of the conflict to
    /// `~/Local/conflict-archive/`. The caller supplies the loser's
    /// full path; the winner was already shown to the operator.
    ArchiveConflictFile(String),
    /// MESHFS-11.1 — archive operation completed.
    ConflictArchived(Result<(), String>),
    /// MOTION-FEEDBACK — cursor entered a file row/tile (key from
    /// [`crate::widgets::row_hover_key`]). Arms its hover-lift tween.
    RowHoverEnter(String),
    /// MOTION-FEEDBACK — cursor left a file row/tile. Settles the hover-lift back.
    RowHoverExit(String),
    /// MOTION-FEEDBACK — one animation frame: advance/GC the [`Animator`] and the
    /// reveal. Emitted by the tick subscription only while a tween is in flight.
    AnimTick,
}

/// MESHFS-8.1 — one recoverable file from the LizardFS `.trash` directory.
#[derive(Debug, Clone)]
pub struct TrashItem {
    /// Display name (leading 8-hex-char inode prefix stripped).
    pub name: String,
    /// Full path of the `.trash` entry (passed to `mackesd meshfs-undelete`).
    pub trash_path: String,
}

/// Breadcrumb segment used by the toolbar.
#[derive(Debug, Clone)]
pub struct Crumb {
    pub label: String,
    /// True if this crumb belongs to a mesh path. Affects colour + the trailing tag chip.
    pub mesh: bool,
    /// AFM-3 — navigation target when this crumb is clicked. `None` for the
    /// leaf (current location) + purely-decorative segments; `Some` makes the
    /// crumb a real button that routes there.
    pub nav: Option<Message>,
}

impl Crumb {
    /// A non-clickable crumb (the current location / decorative segment).
    fn leaf(label: impl Into<String>, mesh: bool) -> Self {
        Self {
            label: label.into(),
            mesh,
            nav: None,
        }
    }

    /// A clickable crumb that routes to `nav` when pressed.
    fn link(label: impl Into<String>, mesh: bool, nav: Message) -> Self {
        Self {
            label: label.into(),
            mesh,
            nav: Some(nav),
        }
    }
}

pub struct MdeFiles {
    /// GUI-7 — the libcosmic application core (window state, theme, nav). Set
    /// from the `Core` libcosmic hands `Application::init`; a throwaway
    /// `Core::default()` fills it on the `Default`/test path (the GUI never
    /// runs there).
    pub core: cosmic::app::Core,
    /// v4.0.1 AF-* (2026-05-23) — backend that supplies the
    /// rendered roster + file lists. Defaults to `RealBackend`
    /// in production builds (LocalFsBackend + DBusBackend
    /// composed); tests can swap a `DemoBackend` via
    /// `MdeFiles::with_backend`.
    pub backend: Box<dyn Backend>,
    /// v4.0.1 AF-* — last `BackendSnapshot` captured. Refreshed
    /// in `update()` so `view()` returns an `Element` tied to
    /// `&self`'s lifetime (Iced can't borrow from a local).
    pub snapshot: BackendSnapshot,
    /// v4.0.1 AF-* — files loaded for the currently-active peer
    /// view. Refreshed when `View::Peer` is entered so `view()`
    /// can borrow without re-querying the backend per render.
    pub peer_files: Vec<crate::model::FileRow>,
    /// E10 — the current local directory (absolute) when browsing `View::Local`.
    /// Defaults to `$HOME`.
    pub local_path: String,
    /// E10 — real files in `local_path`, refreshed when `View::Local` is active.
    pub local_files: Vec<crate::model::FileRow>,
    /// E10 — paired KDE-Connect device rows, refreshed on `View::CloudDevices`.
    pub cloud_files: Vec<crate::model::FileRow>,
    /// E10 — the SMB host typed in the Network view's host box.
    pub net_host: String,
    /// E10 — Disk shares from the last successful `smbclient -L` browse.
    pub net_shares: Vec<String>,
    /// E10 — Network view status / error line (None = idle).
    pub net_status: Option<String>,
    pub view: View,
    /// E10.5 — open browser tabs. The active tab's nav state is mirrored into
    /// the flat `view`/`local_path`/`mesh_home_path`/`search` fields above (the
    /// "active mirror"); `tabs[active_tab]` is re-synced at the end of every
    /// `update()` so the strip always reflects live state.
    pub tabs: Vec<crate::model::Tab>,
    /// E10.5 — index into `tabs` of the currently-shown tab.
    pub active_tab: usize,
    pub local_open: bool,
    /// AFM-2 — sidebar collapse state. When true the full rail is replaced by a
    /// slim button strip (the prototype's `.sidebar-collapsed` 0px grid), with
    /// the panel-toggle still reachable to expand it again.
    pub sidebar_collapsed: bool,
    pub layout: Layout,
    /// DENSITY-SYMMETRY — the listing density (Compact / Comfortable /
    /// Spacious). The view resolves `density::FileListMetrics::for_density`
    /// against this each frame, so flipping it re-rhythms the file-list chrome
    /// (gaps / paddings density-scaled; column widths held). Default per Q26.
    pub density: mde_theme::Density,
    pub search: String,
    /// AF-mesh.3 — path stack inside `View::MeshHomeChild(slug)`.
    /// Empty = top of the XDG dir. Each entry is a single
    /// subdirectory name. Cleared whenever the active slug
    /// changes so the stack never carries stale state across
    /// dirs.
    pub mesh_home_path: Vec<String>,
    /// v2.0.0 Phase 1.3 — row selection state (focus + anchor +
    /// selected set). Cleared on view change.
    pub selection: Selection,
    /// v2.0.0 Phase 1.4 — right-side details panel state.
    pub details: DetailsPanel,
    /// v2.0.0 Phase 1.5 — right-click context-menu state.
    pub context_menu: ContextMenu,
    /// v2.0.0 Phase 1.7 — slide-up operation drawer state.
    pub op_drawer: OperationDrawer,
    /// v2.0.0 Phase 1.6 — drag-and-drop session state.
    pub drag: DragSession,
    /// v2.0.0 Phase 5.x — accessibility prefs (direction / motion
    /// / focus-ring policy). Loaded once at startup from
    /// `Accessibility::load_from_env`. View code reads these each
    /// frame.
    pub a11y: Accessibility,
    /// v2.0.0 Phase 5.1 — which pane currently owns keyboard focus.
    /// Tab cycles through the locked order: Toolbar → Sidebar →
    /// FileList. Used by the focus-ring renderer + the keyboard
    /// dispatcher.
    pub keyboard_pane: KeyboardPane,
    /// v2.0.0 Phase 5.1 — whether the most recent input was a
    /// keyboard interaction. `FocusVisibility::Auto` consults this
    /// to decide whether to render focus rings.
    pub keyboard_active: bool,
    /// MESHFS-8.1 — last loaded trash listing.
    pub trash_items: Vec<TrashItem>,
    /// MESHFS-8.1 — true while a trash load or restore is in flight.
    pub trash_busy: bool,
    /// MESHFS-8.1 — last error from trash load/restore.
    pub trash_error: Option<String>,
    /// MESHFS-11.1 — true while the LizardFS fleet is healing
    /// (under-replicated). Applied as the `syncing` badge on every
    /// mesh-homed `FileRow` in the current listing.
    pub meshfs_healing: bool,
    /// MESHFS-11.1 — active resolve dialog: `(original_name,
    /// conflict_sibling_name)`. `None` means the dialog is closed.
    pub resolve_dialog: Option<(String, String)>,
    /// MESHFS-11.1 — error from the most recent archive operation.
    pub conflict_error: Option<String>,
    /// MOTION-FEEDBACK — the shared `mde_theme::animation` registry driving the
    /// file-row/tile hover-lift + selection-accent tweens off ONE subscription
    /// tick. Idle at rest (no tweens) ⇒ the tick subscription isn't created.
    pub anim: mde_theme::animation::Animator,
    /// MOTION-FEEDBACK — the hover key ([`crate::widgets::row_hover_key`]) of the
    /// currently-hovered row, if any.
    pub hovered_row: Option<String>,
    /// MOTION-FEEDBACK — the hover key of the row whose hover is settling back
    /// (exit in flight). Cleared once its tween completes.
    pub releasing_row: Option<String>,
    /// MOTION-FEEDBACK — when the active listing was (re)loaded — the staggered
    /// reveal origin. Set on every navigation (view/path change); cleared once
    /// the reveal has fully settled so a settled listing costs no per-row work.
    pub reveal_origin: Option<std::time::Instant>,
    /// MOTION-FEEDBACK — the signature of the listing the current `reveal_origin`
    /// belongs to (view + path + mesh path). A change ⇒ a fresh reveal.
    pub listing_sig: String,
    /// BEAUT-FILES — perceived-performance load state of the active file listing:
    /// drives the skeleton-first paint (new listing, no prior content) and the
    /// stale-while-refreshing dim + crossfade (refresh over existing content).
    pub listing_load: crate::loading::ListingLoad,
}

/// v2.0.0 Phase 5.1 — pane currently receiving keyboard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyboardPane {
    /// Toolbar (search input + layout toggle).
    Toolbar,
    /// Left-rail sidebar (peer + pin list).
    Sidebar,
    /// Main file-list pane.
    #[default]
    FileList,
}

impl KeyboardPane {
    /// Tab order: Toolbar → Sidebar → FileList → Toolbar.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Toolbar => Self::Sidebar,
            Self::Sidebar => Self::FileList,
            Self::FileList => Self::Toolbar,
        }
    }

    /// Shift-Tab: reverse direction.
    #[must_use]
    pub fn prev(self) -> Self {
        match self {
            Self::Toolbar => Self::FileList,
            Self::Sidebar => Self::Toolbar,
            Self::FileList => Self::Sidebar,
        }
    }
}

impl Default for MdeFiles {
    fn default() -> Self {
        let backend: Box<dyn Backend> = Box::new(RealBackend::new());
        let snapshot = BackendSnapshot::capture(&*backend);
        Self {
            core: cosmic::app::Core::default(),
            backend,
            snapshot,
            peer_files: Vec::new(),
            local_path: std::env::var("HOME").unwrap_or_else(|_| "/".to_string()),
            local_files: Vec::new(),
            cloud_files: Vec::new(),
            net_host: String::new(),
            net_shares: Vec::new(),
            net_status: None,
            view: View::default(),
            tabs: vec![crate::model::Tab::default()],
            active_tab: 0,
            local_open: false,
            sidebar_collapsed: false,
            layout: Layout::default(),
            density: mde_theme::Density::default(),
            search: String::new(),
            mesh_home_path: Vec::new(),
            selection: Selection::default(),
            details: DetailsPanel::default(),
            context_menu: ContextMenu::default(),
            op_drawer: OperationDrawer::default(),
            drag: DragSession::default(),
            a11y: Accessibility::default(),
            keyboard_pane: KeyboardPane::default(),
            keyboard_active: false,
            trash_items: Vec::new(),
            trash_busy: false,
            trash_error: None,
            meshfs_healing: false,
            resolve_dialog: None,
            conflict_error: None,
            anim: mde_theme::animation::Animator::new(),
            hovered_row: None,
            releasing_row: None,
            reveal_origin: None,
            listing_sig: String::new(),
            listing_load: crate::loading::ListingLoad::default(),
        }
    }
}

impl MdeFiles {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build with an injected backend (useful for unit tests +
    /// dev modes). Production code lands through `Default`.
    #[must_use]
    pub fn with_backend(backend: Box<dyn Backend>) -> Self {
        Self {
            backend,
            ..Self::default()
        }
    }

    /// Run the libcosmic application (GUI-7).
    ///
    /// Builds the cosmic [`Settings`](cosmic::app::Settings) (1480×940 window,
    /// Carbon dark) + passes the optional initial directory as the flag, then
    /// hands off to `cosmic::app::run`.
    ///
    /// E10 — `mde-files [PATH]` opens the Local browser at PATH (a directory),
    /// so a "open folder" / inode-directory handler that execs this binary
    /// lands there.
    pub fn run() -> cosmic::iced::Result {
        let initial_dir = std::env::args()
            .nth(1)
            .filter(|p| std::path::Path::new(p).is_dir());
        // The compositor manages window geometry under Cosmic; Settings carries
        // the defaults (Carbon dark theme is applied via the per-widget style
        // closures + the Application `style` override).
        cosmic::app::run::<MdeFiles>(cosmic::app::Settings::default(), initial_dir)
    }

    /// E10.5 — global tab keybindings: Ctrl+T new tab, Ctrl+W close tab,
    /// Ctrl+Tab cycle. Plain keys are left to the focused widget (the
    /// `listen_with` filter drops everything else).
    fn key_subscription() -> cosmic::iced::Subscription<Message> {
        cosmic::iced::event::listen_with(|event, _status, _window| {
            use cosmic::iced::keyboard::{key::Named, Event as Kbd, Key};
            let cosmic::iced::Event::Keyboard(Kbd::KeyPressed { key, modifiers, .. }) = event
            else {
                return None;
            };
            if !modifiers.command() {
                return None;
            }
            match key.as_ref() {
                Key::Character("t") => Some(Message::NewTab),
                Key::Character("w") => Some(Message::CloseActiveTab),
                Key::Named(Named::Tab) => Some(Message::CycleTab),
                _ => None,
            }
        })
    }

    /// Update reducer — every interaction in the UI flows through this single
    /// function. No async work happens here yet (the demo backend is in-memory);
    /// once `mded` is wired, several variants will return real `Task`s.
    pub fn update(&mut self, msg: Message) -> Task<Message> {
        // MESHFS-8.1: arms that need to return a real Task set this; all
        // others leave it `None` and fall through to `Task::none()`.
        let mut pending_task: Option<Task<Message>> = None;

        match msg {
            Message::NewTab => {
                // Mirror the live state into the active tab, then push + switch
                // to a fresh Mesh-overview tab (end-of-update sync re-captures).
                self.sync_active_tab();
                self.tabs.push(crate::model::Tab::default());
                self.active_tab = self.tabs.len() - 1;
                self.load_active_tab();
            }
            Message::CloseTab(i) => self.close_tab(i),
            Message::CloseActiveTab => self.close_tab(self.active_tab),
            Message::SwitchTab(i) => {
                if i < self.tabs.len() && i != self.active_tab {
                    self.sync_active_tab();
                    self.active_tab = i;
                    self.load_active_tab();
                }
            }
            Message::CycleTab => {
                if self.tabs.len() > 1 {
                    self.sync_active_tab();
                    self.active_tab = (self.active_tab + 1) % self.tabs.len();
                    self.load_active_tab();
                }
            }
            Message::LocalUp => {
                // E10 — ascend to the parent directory (end-of-update refresh
                // re-lists). Stops at the filesystem root.
                if let Some(parent) = std::path::Path::new(&self.local_path).parent() {
                    self.local_path = parent.to_string_lossy().into_owned();
                    self.selection.clear();
                }
            }
            Message::LocalGoto(path) => {
                // E10 — jump the Local browser to an absolute path.
                self.local_path = path;
                self.view = View::Local;
                self.selection.clear();
            }
            Message::LocalActivate(name) => {
                // AFM-4 — resolve the clicked row in the current listing and
                // either descend (directory) or launch it (file). The
                // end-of-update refresh re-lists the new directory.
                if let Some(row) = self.local_files.iter().find(|r| r.name == name) {
                    if let Some(path) = row.path.clone() {
                        if row.is_dir() {
                            self.local_path = path;
                            self.selection.clear();
                        } else {
                            let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
                        }
                    }
                }
            }
            Message::NetHostChanged(host) => self.net_host = host,
            Message::NetBrowse => {
                // E10 — synchronous SMB browse (bounded by smbclient's timeout;
                // consistent with the crate's blocking-I/O pattern). Making it
                // async is a UX follow-on.
                let host = self.net_host.trim().to_string();
                if host.is_empty() {
                    self.net_status = Some("Enter a host to browse.".into());
                } else {
                    self.net_status = Some(format!("Browsing \\\\{host}…"));
                    match crate::smb::smb_shares(&host, 8) {
                        Ok(shares) => {
                            self.net_status = if shares.is_empty() {
                                Some(format!("No shares found on '{host}'."))
                            } else {
                                None
                            };
                            self.net_shares = shares;
                        }
                        Err(e) => {
                            self.net_shares.clear();
                            self.net_status = Some(e);
                        }
                    }
                }
            }
            Message::NetMount(share) => {
                // E10 — mount the share over GVfs, then open it in the local
                // browser at its GVfs FUSE path.
                let host = self.net_host.trim().to_string();
                let path = crate::smb::mount_share(&host, &share);
                self.local_path = path;
                self.view = View::Local;
                self.selection.clear();
            }
            Message::SelectView(v) => {
                let is_local = matches!(v, View::Local);
                let is_undelete = matches!(v, View::MeshUndelete);
                let is_mesh = v.is_mesh();
                // AF-mesh.3 — clear the path stack whenever we
                // leave a MeshHomeChild OR switch to a different
                // slug. Entering MeshHomeChild from the parent
                // implicitly starts at the slug root.
                let drop_path = match (&self.view, &v) {
                    (View::MeshHomeChild(a), View::MeshHomeChild(b)) => a != b,
                    _ => !matches!(v, View::MeshHomeChild(_)),
                };
                if drop_path {
                    self.mesh_home_path.clear();
                }
                self.view = v;
                if !is_local {
                    self.local_open = false;
                }
                // Phase 1.3 — selection is per-view; clear on
                // navigation so stale row keys don't leak across
                // peer folders.
                self.selection.clear();
                // MESHFS-8.1 — entering the Recycle Bin triggers a trash load.
                if is_undelete {
                    self.trash_busy = true;
                    self.trash_error = None;
                    pending_task = Some(load_trash());
                }
                // MESHFS-11.1 — entering any mesh view refreshes the fleet
                // health so the sync badge reflects current healing state.
                if is_mesh && !is_undelete {
                    pending_task = Some(load_meshfs_health());
                }
            }
            Message::MeshFolderEnter(name) => {
                if matches!(self.view, View::MeshHomeChild(_)) {
                    // Strip the trailing `/` the renderer adds
                    // for folders. Reject empty + `..` segments
                    // so the path stack stays canonical.
                    let clean = name.trim_end_matches('/').to_owned();
                    if !clean.is_empty() && clean != ".." && !clean.contains('/') {
                        self.mesh_home_path.push(clean);
                        self.selection.clear();
                    }
                }
            }
            Message::MeshFolderUp => {
                if matches!(self.view, View::MeshHomeChild(_)) && !self.mesh_home_path.is_empty() {
                    self.mesh_home_path.pop();
                    self.selection.clear();
                }
            }
            Message::MeshFolderPop(depth) => {
                if matches!(self.view, View::MeshHomeChild(_)) && depth < self.mesh_home_path.len()
                {
                    self.mesh_home_path.truncate(depth);
                    self.selection.clear();
                }
            }
            Message::ToggleLocal => {
                self.local_open = !self.local_open;
                if self.local_open && !matches!(self.view, View::Local) {
                    self.view = View::Local;
                    self.selection.clear();
                } else if !self.local_open && matches!(self.view, View::Local) {
                    self.view = View::default();
                    self.selection.clear();
                }
            }
            Message::ToggleSidebar => self.sidebar_collapsed = !self.sidebar_collapsed,
            Message::OpenRegistration => {
                // AFM-2 — peer enrollment/registration lives in the workbench;
                // launch it detached. Best-effort: a missing binary is a no-op.
                let _ = std::process::Command::new("mde-workbench").spawn();
            }
            Message::SetLayout(l) => self.layout = l,
            // DENSITY-SYMMETRY — re-rhythm the listing: the next `view()` resolves
            // `FileListMetrics::for_density(self.density)`, so the gaps / paddings
            // re-scale while the column widths hold.
            Message::SetDensity(d) => self.density = d,
            Message::SearchChanged(s) => self.search = s,
            Message::PeerCardBrowse(id) => {
                self.view = View::Peer(id);
                self.selection.clear();
            }
            Message::RowClick(key) => {
                self.selection.click(key);
                // Phase 1.4 — details panel tracks focus.
                self.details.set_target(self.selection.focused());
                self.arm_selection_accents();
            }
            Message::RowCtrlClick(key) => {
                self.selection.ctrl_click(key);
                self.details.set_target(self.selection.focused());
                self.arm_selection_accents();
            }
            Message::RowShiftClick(key, rows) => {
                self.selection.shift_click(key, &rows);
                self.details.set_target(self.selection.focused());
                self.arm_selection_accents();
            }
            Message::FocusNext(rows) => {
                self.selection.focus_next(&rows);
                self.details.set_target(self.selection.focused());
            }
            Message::FocusPrev(rows) => {
                self.selection.focus_prev(&rows);
                self.details.set_target(self.selection.focused());
            }
            Message::ToggleFocused => {
                self.selection.toggle_focused();
                self.arm_selection_accents();
            }
            Message::ClearSelection => {
                self.selection.clear();
                self.details.set_target(None);
            }
            Message::ToggleDetails => {
                self.details.toggle(self.selection.focused());
            }
            Message::OpenContextMenu(row, x, y) => {
                self.context_menu.open(row, (x, y));
            }
            Message::CloseContextMenu => self.context_menu.close(),
            Message::ContextMenuItemClicked(item) => {
                // E10 — resolve the row the menu was opened over (local listing)
                // and route the path-based actions. Send-To / Rename / Delete /
                // Properties route elsewhere / are future.
                let row = self.context_menu.row().and_then(|key| {
                    self.local_files
                        .iter()
                        .chain(self.peer_files.iter())
                        .find(|r| r.name == key)
                        .cloned()
                });
                match item {
                    ContextMenuItem::Open => {
                        if let Some(r) = &row {
                            if let Some(p) = &r.path {
                                if r.is_dir() {
                                    // Descend — the end-of-update refresh re-lists.
                                    self.local_path = p.clone();
                                    self.view = View::Local;
                                } else {
                                    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
                                }
                            }
                        }
                    }
                    ContextMenuItem::CopyPath => {
                        if let Some(p) = row.and_then(|r| r.path) {
                            let _ = std::process::Command::new("wl-copy").arg(p).spawn();
                        }
                    }
                    _ => {}
                }
                self.context_menu.close();
            }
            Message::ToggleOperationDrawer => {
                let open = !self.op_drawer.is_open();
                self.op_drawer.set_open(open);
            }
            Message::OpRowUpsert(row) => self.op_drawer.upsert(row),
            Message::OpRowDismiss(id) => {
                self.op_drawer.dismiss(id);
            }
            Message::DragStart(rows) => self.drag.start(rows),
            Message::DragHover(target) => self.drag.set_hover(target),
            Message::DragDrop => {
                // AUD-1 — a drop onto a sidebar peer is a Send-To. `finish()`
                // yields (row keys, target); resolve the keys to paths under
                // the current local dir and dispatch through the backend.
                if let Some((keys, target)) = self.drag.finish() {
                    let sources = keys
                        .into_iter()
                        .map(|k| {
                            let p = std::path::PathBuf::from(&k);
                            if p.is_absolute() {
                                p
                            } else {
                                std::path::Path::new(&self.local_path).join(k)
                            }
                        })
                        .collect();
                    let destination = drag_target_to_destination(target);
                    self.dispatch_send(
                        sources,
                        destination,
                        SendMode::Copy,
                        ConflictPolicy::Rename,
                    );
                }
            }
            Message::DragCancel => {
                let _ = self.drag.cancel();
            }
            Message::SendTo(req) => {
                // AUD-1 — every entry point funnels here; perform the real
                // send through the backend (mesh → mackesd file-ops → the
                // target peer's replicated inbox) and record an op row.
                self.dispatch_send(req.sources, req.destination, req.mode, req.conflict);
            }
            Message::TabFocus => {
                self.keyboard_pane = self.keyboard_pane.next();
                self.keyboard_active = true;
            }
            Message::ShiftTabFocus => {
                self.keyboard_pane = self.keyboard_pane.prev();
                self.keyboard_active = true;
            }
            Message::FocusSearch => {
                self.keyboard_pane = KeyboardPane::Toolbar;
                self.keyboard_active = true;
            }
            Message::KeyboardActivity => self.keyboard_active = true,
            Message::PointerActivity => self.keyboard_active = false,
            // AFM-1 — real window controls. `window::latest()` resolves the
            // most-recently-created window (the single app window) and the inner
            // match maps to the iced window Task, mirroring
            // `mde-workbench::dispatch_window_action`.
            Message::TitlebarMinimize => {
                pending_task = Some(window::latest().and_then(|id| window::minimize(id, true)));
            }
            Message::TitlebarMaximize => {
                pending_task = Some(window::latest().and_then(window::toggle_maximize));
            }
            Message::TitlebarClose => {
                pending_task = Some(window::latest().and_then(window::close));
            }
            // AFM-9 — per-peer Send: open a file chooser, then dispatch a
            // real Send-To to that peer through the wired backend transport.
            Message::PeerCardSend(id) => {
                if let Some(dest) = self.peer_destination(&id) {
                    pending_task = Some(pick_file_then_send(dest));
                }
            }
            // AFM-9 — the toolbar primary action is view-sensitive: "New" makes
            // a folder (Local), "Send" sends to the open peer (Peer view).
            // Views with no single destination (overview/inbox/outbox/downloads)
            // have no toolbar send target — the per-peer card / drag is the path.
            Message::PrimaryAction => match &self.view {
                View::Local => make_unique_dir(&self.local_path),
                View::Peer(id) => {
                    if let Some(dest) = self.peer_destination(id) {
                        pending_task = Some(pick_file_then_send(dest));
                    }
                }
                _ => {}
            },
            Message::MakeDir => {
                // AFM-9 — create a uniquely-named folder in the current local
                // directory; the end-of-update refresh re-lists it.
                if matches!(self.view, View::Local) {
                    make_unique_dir(&self.local_path);
                }
            }
            Message::Refresh | Message::Noop => {
                // Refresh is the explicit reload signal (the end-of-update
                // `refresh_snapshot()` re-captures live state). Noop is the
                // sink for affordances with no destination-specific action.
            }
            // MESHFS-8.1 — trash operations.
            Message::UndeleteLoaded(result) => {
                match result {
                    Ok(items) => {
                        self.trash_items = items;
                        self.trash_error = None;
                    }
                    Err(e) => self.trash_error = Some(e),
                }
                self.trash_busy = false;
            }
            Message::RestoreTrashItem(path) => {
                self.trash_busy = true;
                pending_task = Some(restore_trash_item(path));
            }
            Message::TrashRestored(path, result) => {
                self.trash_busy = false;
                match result {
                    Ok(()) => {
                        self.trash_items.retain(|i| i.trash_path != path);
                        self.trash_error = None;
                    }
                    Err(e) => self.trash_error = Some(e),
                }
            }
            // MESHFS-11.1 — per-file sync badge + conflict chip + resolve.
            Message::MeshFsHealthLoaded(json) => {
                self.meshfs_healing = parse_meshfs_healing(&json);
            }
            Message::ConflictResolve(orig, sibling) => {
                self.resolve_dialog = Some((orig, sibling));
                self.conflict_error = None;
            }
            Message::DismissConflictDialog => {
                self.resolve_dialog = None;
            }
            Message::ArchiveConflictFile(path) => {
                self.resolve_dialog = None;
                pending_task = Some(archive_conflict_file(path));
            }
            Message::ConflictArchived(result) => match result {
                Ok(()) => self.conflict_error = None,
                Err(e) => self.conflict_error = Some(e),
            },
            // MOTION-FEEDBACK — hover lift on a file row/tile: arm the eased
            // rise, cancel any in-flight release of the same row.
            Message::RowHoverEnter(key) => {
                if self.releasing_row.as_deref() == Some(&key) {
                    self.releasing_row = None;
                }
                if self.hovered_row.as_deref() != Some(&key) {
                    self.hovered_row = Some(key.clone());
                    self.start_row_anim(key);
                }
            }
            // MOTION-FEEDBACK — settle the lift back (ignore a stale exit after a
            // fast re-enter onto a different row).
            Message::RowHoverExit(key) => {
                if self.hovered_row.as_deref() == Some(&key) {
                    self.hovered_row = None;
                    self.releasing_row = Some(key.clone());
                    self.start_row_anim(key);
                }
            }
            // MOTION-FEEDBACK — one frame: GC settled tweens; clear a finished
            // release marker so it isn't re-eased; drop the reveal origin once the
            // whole staggered reveal has elapsed (so the listing goes fully idle).
            Message::AnimTick => {
                let now = std::time::Instant::now();
                self.anim.gc(now);
                if let Some(key) = self.releasing_row.clone() {
                    if !self.anim.is_animating(&key, now) {
                        self.releasing_row = None;
                    }
                }
                if self
                    .reveal_origin
                    .is_some_and(|o| now >= o + Self::reveal_window())
                {
                    self.reveal_origin = None;
                }
                // BEAUT-FILES — advance the load state to Loaded once its window
                // elapses so the skeleton/crossfade stops and the tick can idle.
                self.listing_load.settle(now);
                // An AnimTick mutates only animation state — skip the snapshot
                // refresh + tab sync the data messages need.
                return Task::none();
            }
        }
        self.refresh_snapshot();
        // E10.5 — keep the active tab's stored nav state in lock-step with the
        // live mirror so the tab strip always renders the current view/path.
        self.sync_active_tab();
        pending_task.unwrap_or_else(Task::none)
    }

    /// AUD-1 — perform a Send-To through the backend (mesh → mackesd file-ops
    /// → the target peer's replicated inbox) and record the outcome as an op
    /// row in the drawer. Shared by `Message::SendTo` (all six entry points)
    /// and `Message::DragDrop`.
    fn dispatch_send(
        &mut self,
        sources: Vec<std::path::PathBuf>,
        destination: Destination,
        mode: SendMode,
        conflict: ConflictPolicy,
    ) {
        if sources.is_empty() {
            return;
        }
        let dest_label = match &destination {
            Destination::Peer(n) => format!("{n}.mesh"),
            Destination::Group(g) => format!("{g} group"),
            Destination::Role(r) => format!("role:{r}"),
            Destination::Site(s) => format!("site:{s}"),
        };
        let src_label = match sources.len() {
            1 => sources[0]
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string(),
            n => format!("{n} files"),
        };
        let result = self.backend.send_to(&sources, destination, mode, conflict);
        let (op_id, state, progress) = match &result {
            Ok(id) => (*id, OpState::Completed, 1000),
            Err(_) => (0, OpState::Failed, 0),
        };
        self.op_drawer.upsert(OpRow {
            op_id,
            source: src_label,
            destination: dest_label,
            progress_permille: progress,
            state,
        });
        self.op_drawer.set_open(true);
    }

    /// AFM-9 — resolve a peer id (as carried by the card / peer view) to a
    /// Send-To destination, using the live roster to prefer the real host.
    fn peer_destination(&self, id: &str) -> Option<Destination> {
        let host = self
            .snapshot
            .peers
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.host.clone())
            .unwrap_or_else(|| id.to_string());
        // Strip a trailing ".mesh" — the transport keys on the bare hostname.
        let name = host
            .strip_suffix(".mesh")
            .map(str::to_owned)
            .unwrap_or(host);
        if name.is_empty() {
            None
        } else {
            Some(Destination::Peer(name))
        }
    }

    /// E10.5 — copy the live nav fields into the active tab's stored state.
    fn sync_active_tab(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.view = self.view.clone();
            tab.local_path = self.local_path.clone();
            tab.mesh_home_path = self.mesh_home_path.clone();
            tab.search = self.search.clone();
        }
    }

    /// E10.5 — load the active tab's stored nav state into the live mirror.
    /// `refresh_snapshot()` (end of `update()`) then re-lists for the new view.
    fn load_active_tab(&mut self) {
        if let Some(tab) = self.tabs.get(self.active_tab).cloned() {
            self.view = tab.view;
            self.local_path = tab.local_path;
            self.mesh_home_path = tab.mesh_home_path;
            self.search = tab.search;
            self.selection.clear();
        }
    }

    /// E10.5 — remove the tab at `i`, keeping at least one open. The active
    /// index is clamped so it still points at a live tab.
    fn close_tab(&mut self, i: usize) {
        if self.tabs.len() <= 1 || i >= self.tabs.len() {
            return; // never close the last tab
        }
        // Persist the live mirror first so a non-active close keeps state.
        self.sync_active_tab();
        self.tabs.remove(i);
        if self.active_tab > i || self.active_tab >= self.tabs.len() {
            self.active_tab = self.active_tab.saturating_sub(1);
        }
        self.load_active_tab();
    }

    /// Re-capture the `BackendSnapshot` + (when on a peer view)
    /// the active peer's file list. Called at the end of every
    /// `update()` so the next `view()` render sees fresh data.
    /// O(few backend calls); per-tick cost is acceptable since
    /// Iced only re-runs `update()` on Message arrival.
    fn refresh_snapshot(&mut self) {
        // BEAUT-FILES — remember the signature + whether the active listing had
        // rows BEFORE re-listing, so the load-state transition below knows whether
        // to skeleton (new + empty) or stale-dim (refresh / new-with-content).
        let prev_sig = self.listing_sig.clone();
        let had_content = self.active_listing_len() > 0;
        // AFM-RECONNECT — re-attempt any mesh/bus connection that wasn't live at
        // launch (the cold-boot race that left the roster empty) before
        // capturing, so peers populate on their own.
        self.backend.reconnect();
        self.snapshot = BackendSnapshot::capture(&*self.backend);
        let raw_files = match &self.view {
            View::Peer(id) => self.backend.list(&format!("peer:{id}")),
            View::MeshHomeChild(slug) => {
                let mut p = format!("local:{slug}");
                for seg in &self.mesh_home_path {
                    p.push('/');
                    p.push_str(seg);
                }
                self.backend.list(&p)
            }
            _ => Vec::new(),
        };
        // MESHFS-11.1 — annotate rows with conflict / syncing state.
        self.peer_files = annotate_conflict_and_sync(raw_files, self.meshfs_healing);
        // E10 — real local filesystem listing for the Local browser.
        if matches!(self.view, View::Local) {
            self.local_files = self.backend.list(&self.local_path);
        }
        // E10 — Cloud-Files device roster (paired KDE-Connect peers).
        if matches!(self.view, View::CloudDevices) {
            self.cloud_files = self.backend.list("cloud:");
        }
        // MOTION-FEEDBACK — arm a fresh staggered reveal whenever the active
        // listing changed (navigated to a new view / dir / mesh path).
        self.arm_reveal_if_changed();
        // BEAUT-FILES — drive the perceived-performance load state. A changed
        // signature ⇒ a fresh load (skeleton when there was nothing to keep, stale
        // dim+crossfade when prior rows are carried over); an unchanged signature
        // that still has rows ⇒ a quiet background refresh; then settle to Loaded
        // once the window elapses so the listing idles.
        let now = std::time::Instant::now();
        let sig_changed = self.listing_sig != prev_sig;
        if sig_changed {
            self.listing_load.begin(now, had_content);
        } else {
            self.listing_load
                .refresh_in_place(now, self.active_listing_len() > 0);
        }
        self.listing_load.settle(now);
    }

    /// BEAUT-FILES — the row count of whichever file listing the active view
    /// renders, or `0` for non-listing views (overview / mesh-home cards /
    /// network). Used to decide skeleton-vs-stale-dim and to gate the load
    /// animation off an empty listing.
    fn active_listing_len(&self) -> usize {
        match &self.view {
            View::Peer(_) | View::MeshHomeChild(_) => self.peer_files.len(),
            View::Inbox => self.snapshot.inbox.len(),
            View::Outbox => self.snapshot.outbox.len(),
            View::Downloads => self.snapshot.downloads.len(),
            View::Local => self.local_files.len(),
            View::CloudDevices => self.cloud_files.len(),
            View::MeshOverview | View::MeshHome | View::MeshUndelete | View::Network => 0,
        }
    }

    /// MOTION-FEEDBACK — the listing identity (view + local path + mesh path). A
    /// change between two `refresh_snapshot`s means a directory's entries (re)loaded.
    fn current_listing_sig(&self) -> String {
        format!(
            "{:?}|{}|{}",
            self.view,
            self.local_path,
            self.mesh_home_path.join("/")
        )
    }

    /// MOTION-FEEDBACK — if the active listing changed since the last render, set
    /// a fresh `reveal_origin` so the new entries stagger in (≤8 cap).
    fn arm_reveal_if_changed(&mut self) {
        let sig = self.current_listing_sig();
        if sig != self.listing_sig {
            self.listing_sig = sig;
            self.reveal_origin = Some(std::time::Instant::now());
        }
    }

    /// MOTION-FEEDBACK — the longest the whole staggered reveal can run: the
    /// capped stagger delay (≤8 items) plus one item's reveal duration. After
    /// this the reveal is done and `reveal_origin` is cleared so the listing idles.
    fn reveal_window() -> std::time::Duration {
        use mde_theme::motion::list;
        let max_delay = (list::STAGGER_CAP as u64 - 1) * u64::from(list::STAGGER_STEP_MS);
        std::time::Duration::from_millis(max_delay + u64::from(list::STAGGER_REVEAL_MS))
    }

    /// MOTION-FEEDBACK — start (or restart) the hover tween under `key` from now,
    /// using the Carbon `hover` preset resolved against reduce-motion. Routing all
    /// row tweens through here keeps the reduce-motion contract in one place and
    /// also covers the selection-accent tween (same preset family).
    fn start_row_anim(&mut self, key: impl Into<String>) {
        let now = std::time::Instant::now();
        self.anim.gc(now);
        self.anim.start(
            key,
            now,
            mde_theme::motion::Motion::hover(),
            self.reduce_motion(),
        );
    }

    /// MOTION-FEEDBACK — true when reduce-motion is active (instant state changes,
    /// no movement). Reads the loaded accessibility prefs.
    fn reduce_motion(&self) -> bool {
        self.a11y.motion.is_reduced()
    }

    /// MOTION-FEEDBACK — arm the selection-accent tween for every currently-
    /// selected row so the indigo accent eases in (the row keeps its accent under
    /// reduce-motion — the tween simply settles in ≤80 ms). Called after any
    /// selection-changing message. No-op under reduce-motion (the accent is shown
    /// instantly by [`crate::widgets::RowMotionCtx::for_row`] without a tween).
    fn arm_selection_accents(&mut self) {
        if self.reduce_motion() {
            return;
        }
        let now = std::time::Instant::now();
        self.anim.gc(now);
        let preset = mde_theme::motion::Motion::hover();
        let keys: Vec<String> = self
            .selection
            .iter_sorted()
            .iter()
            .map(|name| crate::widgets::accent_key(name))
            .collect();
        for key in keys {
            // Only (re)start an accent that isn't already running, so dragging the
            // focus across an existing selection doesn't restart settled accents.
            if !self.anim.is_animating(&key, now) {
                self.anim.start(key, now, preset, false);
            }
        }
    }

    /// MOTION-FEEDBACK — true while any file-row tween OR the staggered reveal is
    /// still in flight, so the subscription ticks only while there's motion.
    fn animating(&self) -> bool {
        let now = std::time::Instant::now();
        !self.anim.is_idle(now)
            || self
                .reveal_origin
                .is_some_and(|o| now < o + Self::reveal_window())
            // BEAUT-FILES — keep ticking while the skeleton shimmer / refresh
            // crossfade is in flight, so the placeholder breathes + fresh content
            // fades in; goes idle the instant it settles (MOTION-PERF-1).
            || self.listing_load.is_animating(now)
    }

    /// MOTION-FEEDBACK — build the read-only [`RowMotionCtx`] the file views pass
    /// down so every row derives its motion from this one animator + frame.
    fn row_motion_ctx(&self) -> crate::widgets::RowMotionCtx<'_> {
        crate::widgets::RowMotionCtx {
            anim: &self.anim,
            hovered: self.hovered_row.as_deref(),
            releasing: self.releasing_row.as_deref(),
            reveal_origin: self.reveal_origin,
            now: std::time::Instant::now(),
            reduce_motion: self.reduce_motion(),
            load: self.listing_load,
        }
    }

    /// Top-level view tree.
    pub fn view(&self) -> Element<'_, Message> {
        let crumbs = breadcrumbs_for_with_path(&self.view, &self.mesh_home_path);
        let snap = &self.snapshot;

        // MOTION-FEEDBACK — one shared motion context for every file view this
        // frame (hover + selection accent + staggered reveal off one animator).
        let rm = self.row_motion_ctx();
        // DENSITY-SYMMETRY — resolve the file-list metrics from the live density
        // once per frame; every listing view reads the same token-derived rhythm,
        // so flipping the density re-rhythms the whole chrome at once.
        let metrics = crate::density::FileListMetrics::for_density(self.density);
        let main_body: Element<'_, Message> = match &self.view {
            View::MeshOverview => views::mesh_overview(snap),
            View::Inbox => views::inbox(snap, self.layout, metrics, &self.selection, rm),
            View::Outbox => views::outbox(snap, self.layout, metrics, &self.selection, rm),
            View::Peer(id) => {
                if let Some(p) = snap.peers.iter().find(|p| &p.id == id) {
                    views::peer_folder(
                        p,
                        &snap.self_node,
                        self.peer_files.clone(),
                        &self.search,
                        self.layout,
                        metrics,
                        &self.selection,
                        rm,
                    )
                } else {
                    empty_state("no peer").into()
                }
            }
            View::Downloads => views::downloads(snap, self.layout, metrics, &self.selection, rm),
            View::Local => views::local_browser(
                &self.local_files,
                &self.local_path,
                self.layout,
                metrics,
                &self.selection,
                rm,
            ),
            View::MeshHome => views::mesh_home(snap),
            View::MeshHomeChild(slug) => views::mesh_home_child(
                slug,
                self.peer_files.clone(),
                &self.search,
                self.layout,
                metrics,
                &self.mesh_home_path,
                &self.selection,
                rm,
            ),
            View::MeshUndelete => views::mesh_undelete(
                &self.trash_items,
                self.trash_busy,
                self.trash_error.as_deref(),
            ),
            View::CloudDevices => {
                views::cloud_devices(&self.cloud_files, metrics, &self.selection, rm)
            }
            View::Network => views::network(
                &self.net_host,
                &self.net_shares,
                self.net_status.as_deref(),
                metrics,
            ),
        };

        let content = container(scrollable(container(main_body).padding(Padding {
            top: 18.0,
            right: 22.0,
            bottom: 28.0,
            left: 22.0,
        })))
        .width(Length::Fill)
        .height(Length::Fill)
        .sty(|_| container::Style {
            snap: false,
            background: Some(Background::Color(t::PF_BG_300)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        });

        let main = column![
            views::tab_strip(&self.tabs, self.active_tab),
            views::toolbar(&self.view, self.layout, self.density, &self.search, crumbs),
            content,
        ]
        .spacing(0);

        let body = row![
            views::sidebar(&self.view, self.local_open, self.sidebar_collapsed, snap),
            container(main).width(Length::Fill).height(Length::Fill),
        ]
        .height(Length::Fill);

        let online = snap
            .peers
            .iter()
            .filter(|p| matches!(p.status, crate::model::PeerStatus::Online))
            .count();
        let total = snap.peers.len();

        let root: Element<'_, Message> =
            container(column![views::titlebar_with_status(online, total), body].spacing(0))
                .width(Length::Fill)
                .height(Length::Fill)
                .sty(|_| container::Style {
                    snap: false,
                    background: Some(Background::Color(t::WINDOW)),
                    border: Border {
                        color: Color {
                            a: 0.08,
                            ..Color::WHITE
                        },
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..container::Style::default()
                })
                .into();

        // MESHFS-11.1 — overlay the resolve dialog when active.
        if let Some((orig, sib)) = &self.resolve_dialog {
            cosmic::iced::widget::Stack::with_children(vec![
                root,
                views::resolve_conflict_dialog(orig, sib),
            ])
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            root
        }
    }
}

/// GUI-7 — the `cosmic::Application` shell. The inherent `update`/`view`/
/// `key_subscription` carry the real logic (inherent methods win direct calls,
/// so these trait methods delegate without recursion); the trait wraps the
/// reducer's iced `Task` into the cosmic `Action` space.
impl Application for MdeFiles {
    type Executor = cosmic::executor::Default;
    /// The optional initial directory (`mde-files [PATH]`).
    type Flags = Option<String>;
    type Message = Message;
    const APP_ID: &'static str = "com.mackes.MagicMeshFiles";

    fn core(&self) -> &cosmic::app::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::app::Core {
        &mut self.core
    }

    fn init(
        core: cosmic::app::Core,
        flags: Self::Flags,
    ) -> (Self, cosmic::app::Task<Self::Message>) {
        let mut s = Self::new();
        s.core = core;
        // Keep mde-files' custom titlebar; suppress Cosmic's headerbar.
        s.core.window.show_headerbar = false;
        s.set_header_title("Artifact Manager".to_string());
        if let Some(dir) = flags {
            s.local_path = dir;
            s.view = View::Local;
            s.sync_active_tab();
        }
        (s, cosmic::app::Task::none())
    }

    fn subscription(&self) -> cosmic::iced::Subscription<Self::Message> {
        // AFM-RECONNECT — a slow tick so a GUI launched before mackesd's
        // responders were ready re-attempts the mesh/bus connection and
        // populates its peers on its own (every `update` ends in
        // `refresh_snapshot`, which now reconnects). No user interaction needed.
        let reconnect =
            cosmic::iced::time::every(std::time::Duration::from_secs(5)).map(|_| Message::Refresh);
        // MOTION-FEEDBACK — a ~60 fps animation clock, but ONLY while a tween or
        // the staggered reveal is in flight (MOTION-PERF-1 — a settled listing
        // costs no idle wakeups). At rest `animating()` is false and this tick
        // isn't created.
        if self.animating() {
            let tick = cosmic::iced::time::every(std::time::Duration::from_millis(16))
                .map(|_| Message::AnimTick);
            cosmic::iced::Subscription::batch([Self::key_subscription(), reconnect, tick])
        } else {
            cosmic::iced::Subscription::batch([Self::key_subscription(), reconnect])
        }
    }

    fn update(&mut self, message: Self::Message) -> cosmic::app::Task<Self::Message> {
        // Delegate to the inherent reducer (inherent resolution wins), then lift
        // the iced Task into the cosmic Action space the runtime expects.
        MdeFiles::update(self, message).map(cosmic::Action::App)
    }

    fn view(&self) -> Element<'_, Self::Message> {
        MdeFiles::view(self)
    }
}

/// AF-mesh.3 — path-aware breadcrumb builder. Identical to
/// `breadcrumbs_for` except for `MeshHomeChild`, where each
/// element of `path` becomes its own crumb between the dir-name
/// crumb and the leaf.
pub fn breadcrumbs_for_with_path(view: &View, path: &[String]) -> Vec<Crumb> {
    if let View::MeshHomeChild(slug) = view {
        // Mesh → overview · Home → mesh-home landing · <Dir> → the slug root
        // (resets the descent) · each path segment → pop to that depth.
        let mut out = vec![
            Crumb::link("Mesh", true, Message::SelectView(View::MeshOverview)),
            Crumb::link("Home", true, Message::SelectView(View::MeshHome)),
            Crumb::link(
                mesh_home_label(slug),
                true,
                Message::SelectView(View::MeshHomeChild(slug.clone())),
            ),
        ];
        for (depth, seg) in path.iter().enumerate() {
            out.push(Crumb::link(
                seg.clone(),
                true,
                Message::MeshFolderPop(depth),
            ));
        }
        // The final crumb is the current location: leaf styling, not clickable.
        if let Some(last) = out.last_mut() {
            last.mesh = false;
            last.nav = None;
        }
        return out;
    }
    breadcrumbs_for(view)
}

fn breadcrumbs_for(view: &View) -> Vec<Crumb> {
    let mesh_root = || Crumb::link("Mesh", true, Message::SelectView(View::MeshOverview));
    match view {
        View::MeshOverview => vec![mesh_root(), Crumb::leaf("Overview", false)],
        View::Inbox => vec![mesh_root(), Crumb::leaf("Inbox", false)],
        View::Outbox => vec![mesh_root(), Crumb::leaf("Outbox", false)],
        View::CloudDevices => vec![Crumb::leaf("Cloud Files", false)],
        View::Network => vec![Crumb::leaf("Network", false)],
        View::Peer(id) => {
            // The host string is built from the peer id by convention
            // (id "pine" → host "pine.mesh"); the runtime patches the live
            // host on next render. The leaf is the current peer (not clickable).
            vec![mesh_root(), Crumb::leaf(format!("{id}.mesh"), false)]
        }
        View::Downloads => vec![mesh_root(), Crumb::leaf("Downloads", false)],
        View::Local => vec![Crumb::leaf("Local", false), Crumb::leaf("/", false)],
        View::MeshHome => vec![mesh_root(), Crumb::leaf("Home", false)],
        View::MeshHomeChild(slug) => vec![
            mesh_root(),
            Crumb::link("Home", true, Message::SelectView(View::MeshHome)),
            Crumb::leaf(mesh_home_label(slug), false),
        ],
        View::MeshUndelete => vec![mesh_root(), Crumb::leaf("Recycle Bin", false)],
    }
}

/// Human-readable label for a mesh-home XDG-dir slug.
pub fn mesh_home_label(slug: &str) -> &'static str {
    match slug {
        "docs" => "Documents",
        "pics" => "Pictures",
        "music" => "Music",
        "videos" => "Videos",
        "downloads" => "Downloads",
        _ => "Files",
    }
}

// ── MESHFS-8.1: trash load + restore helpers ────────────────────────────────

/// Shell `mackesd meshfs-trash-list` and return the parsed entry list.
fn fetch_trash_items() -> Result<Vec<TrashItem>, String> {
    let out = std::process::Command::new("mackesd")
        .args(["meshfs-trash-list"])
        .output()
        .map_err(|e| format!("mackesd meshfs-trash-list: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.trim().is_empty() {
        return Ok(Vec::new());
    }
    // Parse JSON array of objects with "name" + "trash_path" keys.
    let vals: Vec<serde_json::Value> =
        serde_json::from_str(stdout.trim()).map_err(|e| format!("JSON parse: {e}"))?;
    Ok(vals
        .into_iter()
        .filter_map(|v| {
            Some(TrashItem {
                name: v["name"].as_str()?.to_owned(),
                trash_path: v["trash_path"].as_str()?.to_owned(),
            })
        })
        .collect())
}

/// AUD-1 — map a drag drop target onto the canonical Send-To destination.
fn drag_target_to_destination(t: DragTarget) -> Destination {
    match t {
        DragTarget::Peer(n) => Destination::Peer(n),
        DragTarget::Group(g) => Destination::Group(g),
        DragTarget::Role(r) => Destination::Role(r),
        DragTarget::Site(s) => Destination::Site(s),
    }
}

/// Build a Task that shells `mackesd meshfs-trash-list` and emits
/// `Message::UndeleteLoaded`.
fn load_trash() -> Task<Message> {
    Task::perform(async { fetch_trash_items() }, Message::UndeleteLoaded)
}

/// Build a Task that calls `mackesd meshfs-undelete --path <path>` and
/// emits `Message::TrashRestored`.
fn restore_trash_item(path: String) -> Task<Message> {
    let path_msg = path.clone();
    Task::perform(
        async move {
            let ok = std::process::Command::new("mackesd")
                .args(["meshfs-undelete", "--path", &path])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                Ok(())
            } else {
                Err("TRASH-RECOVER failed".to_string())
            }
        },
        move |result| Message::TrashRestored(path_msg.clone(), result),
    )
}

// ── MESHFS-11.1: sync badge + conflict chip + resolve helpers ────────────────

/// Detect `.conflict-<ts>-<host>` siblings, annotate the parent `FileRow`
/// with `has_conflict` + `conflict_sibling`, set `syncing` on all mesh-homed
/// rows when the fleet is healing, and filter out the raw conflict entries.
fn annotate_conflict_and_sync(
    rows: Vec<crate::model::FileRow>,
    healing: bool,
) -> Vec<crate::model::FileRow> {
    use std::collections::HashMap;
    // Find all filenames that look like "<base>.conflict-<ts>-<host>".
    // Build a map from base name → conflict filename.
    let mut conflicts: HashMap<String, String> = HashMap::new();
    for row in &rows {
        if let Some(base) = strip_conflict_suffix(&row.name) {
            conflicts.insert(base, row.name.clone());
        }
    }
    rows.into_iter()
        // Filter out the raw conflict entries (they surface as chips on their parent).
        .filter(|row| strip_conflict_suffix(&row.name).is_none())
        .map(|mut row| {
            if let Some(sibling) = conflicts.get(&row.name) {
                row.has_conflict = true;
                row.conflict_sibling = Some(sibling.clone());
            }
            if healing && row.is_mesh() {
                row.syncing = true;
            }
            row
        })
        .collect()
}

/// Return the base name if `name` matches `<base>.conflict-<anything>`,
/// otherwise `None`. The pattern is `.<word>.conflict-` — at least two
/// dash-separated tokens after `.conflict-` (timestamp + host).
fn strip_conflict_suffix(name: &str) -> Option<String> {
    // Find the last `.conflict-` segment.
    let marker = ".conflict-";
    let idx = name.rfind(marker)?;
    let suffix = &name[idx + marker.len()..];
    // Require at least one non-empty segment after the marker.
    if suffix.is_empty() {
        return None;
    }
    Some(name[..idx].to_owned())
}

/// Parse raw mesh-fs-status JSON to extract whether the fleet is healing.
fn parse_meshfs_healing(json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| {
            // master must be reachable for healing to apply.
            let reachable = v["master_reachable"].as_bool().unwrap_or(false);
            if !reachable {
                return Some(false);
            }
            let peers = v["peers"].as_array().map(|a| a.len()).unwrap_or(0);
            let goal = v["goal"].as_u64().unwrap_or(0) as usize;
            let any_undergoal = v["peers"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .any(|p| p["undergoal_chunks"].as_u64().unwrap_or(0) > 0);
            let offline = !v["offline_peers"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(true);
            let under_replicated = goal > 0 && peers < goal;
            Some(under_replicated || any_undergoal || offline)
        })
        .unwrap_or(false)
}

/// Shell `mackesd mesh-fs-status --json` and emit `Message::MeshFsHealthLoaded`.
fn load_meshfs_health() -> Task<Message> {
    Task::perform(
        async {
            std::process::Command::new("mackesd")
                .args(["mesh-fs-status"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default()
        },
        Message::MeshFsHealthLoaded,
    )
}

/// Shell `mackesd meshfs-resolve-conflict --path <path>` and emit
/// `Message::ConflictArchived`.
fn archive_conflict_file(path: String) -> Task<Message> {
    Task::perform(
        async move {
            let status = std::process::Command::new("mackesd")
                .args(["meshfs-resolve-conflict", "--path", &path])
                .status();
            match status {
                Ok(s) if s.success() => Ok(()),
                Ok(_) => Err("mackesd meshfs-resolve-conflict failed".to_owned()),
                Err(e) => Err(format!("spawn: {e}")),
            }
        },
        Message::ConflictArchived,
    )
}

/// AFM-9 — pop a desktop file chooser (zenity → kdialog), then dispatch a real
/// Send-To of the chosen file to `dest`. Cancelling / no chooser installed →
/// `Noop` (no fake transfer). Runs the chooser off the UI thread via a Task.
fn pick_file_then_send(dest: Destination) -> Task<Message> {
    Task::perform(async { pick_file_blocking() }, move |picked| match picked {
        Some(path) => Message::SendTo(SendToRequest::copy_ask(
            vec![path],
            dest.clone(),
            crate::send_to::SendToEntry::Toolbar,
        )),
        None => Message::Noop,
    })
}

/// Run a system file chooser and return the selected path. Tries `zenity` then
/// `kdialog`; returns `None` on cancel, error, or when neither is installed.
fn pick_file_blocking() -> Option<std::path::PathBuf> {
    let attempts: [(&str, &[&str]); 2] = [
        (
            "zenity",
            &["--file-selection", "--title=Send a file to the mesh"],
        ),
        ("kdialog", &["--getopenfilename"]),
    ];
    for (bin, args) in attempts {
        match std::process::Command::new(bin).args(args).output() {
            Ok(out) if out.status.success() => {
                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !path.is_empty() {
                    return Some(std::path::PathBuf::from(path));
                }
                return None; // chooser ran but selected nothing
            }
            Ok(_) => return None, // user cancelled
            Err(_) => continue,   // binary missing — try the next chooser
        }
    }
    None
}

/// AFM-9 — create a uniquely-named "New Folder" (then "New Folder 2"…) in `dir`.
fn make_unique_dir(dir: &str) {
    let base = std::path::Path::new(dir);
    for n in 0..100 {
        let name = if n == 0 {
            "New Folder".to_string()
        } else {
            format!("New Folder {}", n + 1)
        };
        let candidate = base.join(&name);
        if !candidate.exists() {
            let _ = std::fs::create_dir(&candidate);
            return;
        }
    }
}

fn empty_state(label: &str) -> Element<'static, Message> {
    container(
        cosmic::iced::widget::text(label.to_string())
            .size(12)
            .colr(t::FG_FAINT),
    )
    .padding(Padding::new(56.0))
    .width(Length::Fill)
    .sty(|_| container::Style {
        snap: false,
        background: Some(Background::Color(Color::TRANSPARENT)),
        border: Border {
            color: Color {
                a: 0.10,
                ..Color::WHITE
            },
            width: 1.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_view_is_mesh_overview() {
        let s = MdeFiles::default();
        assert_eq!(s.view, View::MeshOverview);
        assert!(!s.local_open);
        assert_eq!(s.layout, Layout::List);
        assert!(s.search.is_empty());
    }

    #[test]
    fn toggle_local_opens_local_view() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::ToggleLocal);
        assert!(s.local_open);
        assert_eq!(s.view, View::Local);
        let _ = s.update(Message::ToggleLocal);
        assert!(!s.local_open);
        assert_eq!(s.view, View::MeshOverview);
    }

    #[test]
    fn selecting_non_local_view_closes_local_disclosure() {
        let mut s = MdeFiles::default();
        s.local_open = true;
        s.view = View::Local;
        let _ = s.update(Message::SelectView(View::Inbox));
        assert_eq!(s.view, View::Inbox);
        assert!(
            !s.local_open,
            "local disclosure must close when leaving Local view"
        );
    }

    #[test]
    fn peer_card_browse_routes_to_peer_view() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::PeerCardBrowse("pine".into()));
        assert_eq!(s.view, View::Peer("pine".into()));
    }

    #[test]
    fn row_click_message_updates_selection() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::RowClick("doc.txt".into()));
        assert_eq!(s.selection.len(), 1);
        assert!(s.selection.is_selected("doc.txt"));
    }

    #[test]
    fn row_ctrl_click_toggles() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::RowCtrlClick("a".into()));
        let _ = s.update(Message::RowCtrlClick("b".into()));
        assert_eq!(s.selection.len(), 2);
        let _ = s.update(Message::RowCtrlClick("a".into()));
        assert_eq!(s.selection.len(), 1);
        assert!(s.selection.is_selected("b"));
    }

    #[test]
    fn row_shift_click_extends_range() {
        let mut s = MdeFiles::default();
        let rows: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let _ = s.update(Message::RowClick("a".into()));
        let _ = s.update(Message::RowShiftClick("c".into(), rows));
        assert_eq!(s.selection.len(), 3);
    }

    #[test]
    fn focus_next_and_prev_messages() {
        let mut s = MdeFiles::default();
        let rows: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let _ = s.update(Message::FocusNext(rows.clone()));
        assert_eq!(s.selection.focused(), Some("a"));
        let _ = s.update(Message::FocusPrev(rows));
        assert_eq!(s.selection.focused(), Some("c"));
    }

    #[test]
    fn toggle_focused_message() {
        let mut s = MdeFiles::default();
        let rows: Vec<String> = vec!["x".into()];
        let _ = s.update(Message::FocusNext(rows));
        let _ = s.update(Message::ToggleFocused);
        assert!(s.selection.is_selected("x"));
    }

    #[test]
    fn clear_selection_message_resets() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::RowClick("x".into()));
        let _ = s.update(Message::ClearSelection);
        assert!(s.selection.is_empty());
    }

    #[test]
    fn view_change_clears_selection() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::RowClick("x".into()));
        assert!(!s.selection.is_empty());
        let _ = s.update(Message::SelectView(View::Inbox));
        assert!(s.selection.is_empty(), "view change must clear selection");
    }

    #[test]
    fn peer_card_browse_clears_selection() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::RowClick("x".into()));
        let _ = s.update(Message::PeerCardBrowse("pine".into()));
        assert!(s.selection.is_empty());
    }

    #[test]
    fn toggle_details_panel_message() {
        let mut s = MdeFiles::default();
        // No focus → toggle is a no-op (Phase 1.4 lock).
        let _ = s.update(Message::ToggleDetails);
        assert!(!s.details.is_open());
        // After focusing a row, toggle opens it.
        let _ = s.update(Message::RowClick("a.txt".into()));
        let _ = s.update(Message::ToggleDetails);
        assert!(s.details.is_open());
        assert_eq!(s.details.target(), Some("a.txt"));
    }

    #[test]
    fn row_click_updates_details_target_when_open() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::RowClick("a".into()));
        let _ = s.update(Message::ToggleDetails);
        let _ = s.update(Message::RowClick("b".into()));
        assert_eq!(s.details.target(), Some("b"));
        assert!(s.details.is_open());
    }

    #[test]
    fn clear_selection_closes_details() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::RowClick("a".into()));
        let _ = s.update(Message::ToggleDetails);
        assert!(s.details.is_open());
        let _ = s.update(Message::ClearSelection);
        assert!(!s.details.is_open(), "details hides when nothing selected");
    }

    #[test]
    fn open_context_menu_message() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::OpenContextMenu("a.txt".into(), 100.0, 200.0));
        assert!(s.context_menu.is_open());
        assert_eq!(s.context_menu.row(), Some("a.txt"));
        assert_eq!(s.context_menu.anchor(), Some((100.0, 200.0)));
    }

    #[test]
    fn context_menu_item_clicked_closes_menu() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::OpenContextMenu("a.txt".into(), 0.0, 0.0));
        let _ = s.update(Message::ContextMenuItemClicked(ContextMenuItem::Open));
        assert!(!s.context_menu.is_open());
    }

    #[test]
    fn net_browse_empty_host_sets_status_and_mount_navigates() {
        let mut s = MdeFiles::default();
        s.view = View::Network;
        let _ = s.update(Message::NetBrowse); // empty host
        assert!(s.net_status.as_deref().unwrap().contains("Enter a host"));
        let _ = s.update(Message::NetHostChanged("nas".into()));
        assert_eq!(s.net_host, "nas");
        // Mount routes into the local browser at the share's GVfs path.
        let _ = s.update(Message::NetMount("docs".into()));
        assert!(s.local_path.contains("smb-share:server=nas,share=docs"));
        assert!(matches!(s.view, View::Local));
    }

    #[test]
    fn local_goto_jumps_to_path_and_enters_local_view() {
        let mut s = MdeFiles::default();
        s.view = View::CloudDevices;
        let _ = s.update(Message::LocalGoto("/tmp".into()));
        assert_eq!(s.local_path, "/tmp");
        assert!(matches!(s.view, View::Local));
    }

    #[test]
    fn local_up_ascends_to_parent_dir() {
        let mut s = MdeFiles::default();
        s.local_path = "/home/user/Documents".into();
        let _ = s.update(Message::LocalUp);
        assert_eq!(s.local_path, "/home/user");
        // Root stays root (no parent).
        s.local_path = "/".into();
        let _ = s.update(Message::LocalUp);
        assert_eq!(s.local_path, "/");
    }

    #[test]
    fn context_open_on_a_local_dir_descends() {
        // A real temp dir with a subfolder; the end-of-update refresh lists it
        // so the "docs/" row carries a real path the Open handler descends into.
        let tmp = std::env::temp_dir().join(format!("mdefiles-descend-{}", std::process::id()));
        let sub = tmp.join("docs");
        std::fs::create_dir_all(&sub).unwrap();
        let mut s = MdeFiles::default();
        s.view = View::Local;
        s.local_path = tmp.to_string_lossy().into_owned();
        // OpenContextMenu's end-of-update refresh populates local_files from tmp.
        let _ = s.update(Message::OpenContextMenu("docs/".into(), 0.0, 0.0));
        let _ = s.update(Message::ContextMenuItemClicked(ContextMenuItem::Open));
        // Open on the directory row descends into it.
        assert_eq!(s.local_path, sub.to_string_lossy());
        assert!(matches!(s.view, View::Local));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn toggle_operation_drawer_message() {
        let mut s = MdeFiles::default();
        assert!(!s.op_drawer.is_open());
        let _ = s.update(Message::ToggleOperationDrawer);
        assert!(s.op_drawer.is_open());
        let _ = s.update(Message::ToggleOperationDrawer);
        assert!(!s.op_drawer.is_open());
    }

    #[test]
    fn op_row_upsert_and_dismiss_messages() {
        use crate::panels::{OpRow, OpState};
        let mut s = MdeFiles::default();
        let row = OpRow {
            op_id: 7,
            source: "a.txt".into(),
            destination: "pine".into(),
            progress_permille: 500,
            state: OpState::Running,
        };
        let _ = s.update(Message::OpRowUpsert(row));
        assert_eq!(s.op_drawer.len(), 1);
        let _ = s.update(Message::OpRowDismiss(7));
        assert_eq!(s.op_drawer.len(), 0);
    }

    #[test]
    fn drag_start_and_drop_messages() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::DragStart(vec!["a.txt".into(), "b.txt".into()]));
        assert!(s.drag.is_active());
        assert_eq!(s.drag.sources().len(), 2);
        let _ = s.update(Message::DragHover(Some(DragTarget::Peer("pine".into()))));
        assert_eq!(
            s.drag.hover_target(),
            Some(&DragTarget::Peer("pine".into()))
        );
        let _ = s.update(Message::DragDrop);
        assert!(!s.drag.is_active(), "drag finishes after drop");
    }

    #[test]
    fn drag_cancel_message() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::DragStart(vec!["a".into()]));
        let _ = s.update(Message::DragCancel);
        assert!(!s.drag.is_active());
    }

    #[test]
    fn tab_focus_cycles_through_panes() {
        let mut s = MdeFiles::default();
        assert_eq!(s.keyboard_pane, KeyboardPane::FileList);
        let _ = s.update(Message::TabFocus);
        assert_eq!(s.keyboard_pane, KeyboardPane::Toolbar);
        let _ = s.update(Message::TabFocus);
        assert_eq!(s.keyboard_pane, KeyboardPane::Sidebar);
        let _ = s.update(Message::TabFocus);
        assert_eq!(s.keyboard_pane, KeyboardPane::FileList);
    }

    #[test]
    fn shift_tab_focus_reverses() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::ShiftTabFocus);
        assert_eq!(s.keyboard_pane, KeyboardPane::Sidebar);
        let _ = s.update(Message::ShiftTabFocus);
        assert_eq!(s.keyboard_pane, KeyboardPane::Toolbar);
    }

    #[test]
    fn focus_search_jumps_to_toolbar() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::FocusSearch);
        assert_eq!(s.keyboard_pane, KeyboardPane::Toolbar);
        assert!(s.keyboard_active);
    }

    #[test]
    fn keyboard_activity_toggles_keyboard_active_flag() {
        let mut s = MdeFiles::default();
        assert!(!s.keyboard_active);
        let _ = s.update(Message::KeyboardActivity);
        assert!(s.keyboard_active);
        let _ = s.update(Message::PointerActivity);
        assert!(!s.keyboard_active);
    }

    #[test]
    fn keyboard_pane_tab_order_is_three_step_cycle() {
        let start = KeyboardPane::Toolbar;
        let one = start.next();
        let two = one.next();
        let three = two.next();
        assert_eq!(three, start, "Tab returns to start after 3 hops");
    }

    #[test]
    fn send_to_message_is_a_silent_routing_hook() {
        use crate::backend::{ConflictPolicy, Destination, SendMode};
        use crate::send_to::{SendToEntry, SendToRequest};
        let mut s = MdeFiles::default();
        // The reducer just routes — no observable state change.
        // The DemoBackend doesn't get called from here (the
        // view-side `Backend` consumer does that), so we only
        // assert the message round-trips without panicking.
        let req = SendToRequest {
            sources: vec![std::path::PathBuf::from("/tmp/a.txt")],
            destination: Destination::Peer("pine".into()),
            mode: SendMode::Copy,
            conflict: ConflictPolicy::Ask,
            entry: SendToEntry::Toolbar,
        };
        let _ = s.update(Message::SendTo(req));
        // No assertion on state — that's the contract.
    }

    #[test]
    fn breadcrumbs_match_view() {
        let c = breadcrumbs_for(&View::MeshOverview);
        assert_eq!(c.len(), 2);
        assert!(c[0].mesh);
        assert_eq!(c[0].label, "Mesh");
        assert_eq!(c[1].label, "Overview");

        let c = breadcrumbs_for(&View::Peer("birch".into()));
        assert_eq!(c[1].label, "birch.mesh");

        let c = breadcrumbs_for(&View::Local);
        assert_eq!(c.len(), 2);
        assert!(!c.iter().any(|x| x.mesh));
    }

    #[test]
    fn selecting_mesh_home_clears_selection() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::RowClick("x".into()));
        let _ = s.update(Message::SelectView(View::MeshHome));
        assert_eq!(s.view, View::MeshHome);
        assert!(s.selection.is_empty(), "mesh-home should clear selection");
    }

    #[test]
    fn mesh_home_child_refreshes_local_listing() {
        let mut s = MdeFiles::default();
        // Route into a child; refresh_snapshot should query the
        // backend for `local:<slug>`. With the default RealBackend
        // pointing at $HOME, the call returns whatever's on disk
        // (or an empty Vec). The contract under test is just that
        // refresh_snapshot doesn't panic + the view variant is
        // accepted.
        let _ = s.update(Message::SelectView(View::MeshHomeChild("docs".into())));
        assert_eq!(s.view, View::MeshHomeChild("docs".into()));
    }

    #[test]
    fn mesh_home_label_covers_xdg_slugs() {
        assert_eq!(mesh_home_label("docs"), "Documents");
        assert_eq!(mesh_home_label("pics"), "Pictures");
        assert_eq!(mesh_home_label("music"), "Music");
        assert_eq!(mesh_home_label("videos"), "Videos");
        assert_eq!(mesh_home_label("downloads"), "Downloads");
        assert_eq!(mesh_home_label("unknown"), "Files");
    }

    #[test]
    fn breadcrumbs_for_mesh_home_marks_mesh_segments() {
        let c = breadcrumbs_for(&View::MeshHome);
        assert_eq!(c.len(), 2);
        assert!(c[0].mesh);
        assert_eq!(c[0].label, "Mesh");
        assert_eq!(c[1].label, "Home");

        let c = breadcrumbs_for(&View::MeshHomeChild("docs".into()));
        assert_eq!(c.len(), 3);
        assert_eq!(c[2].label, "Documents");
    }

    #[test]
    fn mesh_folder_enter_pushes_onto_path_stack() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::SelectView(View::MeshHomeChild("docs".into())));
        let _ = s.update(Message::MeshFolderEnter("Projects/".into()));
        assert_eq!(s.mesh_home_path, vec!["Projects".to_string()]);
        let _ = s.update(Message::MeshFolderEnter("MDE".into()));
        assert_eq!(
            s.mesh_home_path,
            vec!["Projects".to_string(), "MDE".to_string()]
        );
    }

    #[test]
    fn mesh_folder_enter_outside_mesh_home_child_is_noop() {
        let mut s = MdeFiles::default();
        // Default view is MeshOverview, not MeshHomeChild.
        let _ = s.update(Message::MeshFolderEnter("anywhere".into()));
        assert!(s.mesh_home_path.is_empty());
    }

    #[test]
    fn mesh_folder_enter_rejects_path_separators() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::SelectView(View::MeshHomeChild("docs".into())));
        // Reject anything that smells like an escape attempt.
        let _ = s.update(Message::MeshFolderEnter("..".into()));
        let _ = s.update(Message::MeshFolderEnter("a/b".into()));
        let _ = s.update(Message::MeshFolderEnter("".into()));
        assert!(s.mesh_home_path.is_empty());
    }

    #[test]
    fn mesh_folder_up_pops_one_level() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::SelectView(View::MeshHomeChild("docs".into())));
        let _ = s.update(Message::MeshFolderEnter("a".into()));
        let _ = s.update(Message::MeshFolderEnter("b".into()));
        let _ = s.update(Message::MeshFolderUp);
        assert_eq!(s.mesh_home_path, vec!["a".to_string()]);
    }

    #[test]
    fn mesh_folder_up_at_root_is_noop() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::SelectView(View::MeshHomeChild("docs".into())));
        let _ = s.update(Message::MeshFolderUp);
        assert!(s.mesh_home_path.is_empty());
    }

    #[test]
    fn mesh_folder_pop_truncates_to_depth() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::SelectView(View::MeshHomeChild("docs".into())));
        let _ = s.update(Message::MeshFolderEnter("a".into()));
        let _ = s.update(Message::MeshFolderEnter("b".into()));
        let _ = s.update(Message::MeshFolderEnter("c".into()));
        let _ = s.update(Message::MeshFolderPop(1));
        assert_eq!(s.mesh_home_path, vec!["a".to_string()]);
    }

    #[test]
    fn changing_slug_clears_path_stack() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::SelectView(View::MeshHomeChild("docs".into())));
        let _ = s.update(Message::MeshFolderEnter("a".into()));
        let _ = s.update(Message::SelectView(View::MeshHomeChild("pics".into())));
        assert!(
            s.mesh_home_path.is_empty(),
            "path must reset when slug changes"
        );
    }

    #[test]
    fn leaving_mesh_home_child_clears_path_stack() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::SelectView(View::MeshHomeChild("docs".into())));
        let _ = s.update(Message::MeshFolderEnter("a".into()));
        let _ = s.update(Message::SelectView(View::MeshOverview));
        assert!(s.mesh_home_path.is_empty());
    }

    #[test]
    fn breadcrumbs_with_path_lists_each_segment() {
        let path = vec!["Projects".to_string(), "MDE".to_string()];
        let c = breadcrumbs_for_with_path(&View::MeshHomeChild("docs".into()), &path);
        assert_eq!(c.len(), 5);
        assert_eq!(c[0].label, "Mesh");
        assert_eq!(c[1].label, "Home");
        assert_eq!(c[2].label, "Documents");
        assert_eq!(c[3].label, "Projects");
        assert_eq!(c[4].label, "MDE");
        // The leaf crumb is rendered without the mesh tint.
        assert!(!c[4].mesh);
    }

    // ── MESHFS-11.1: conflict chip + sync badge + resolve helpers ────────────

    #[test]
    fn strip_conflict_suffix_detects_conflict_filenames() {
        assert_eq!(
            strip_conflict_suffix("report.pdf.conflict-20260529-oak"),
            Some("report.pdf".to_owned())
        );
        assert_eq!(
            strip_conflict_suffix("notes.txt.conflict-1234567890-pine"),
            Some("notes.txt".to_owned())
        );
        // Normal filenames — not conflict siblings.
        assert_eq!(strip_conflict_suffix("report.pdf"), None);
        assert_eq!(strip_conflict_suffix("notes.txt"), None);
        // Empty suffix after marker → not valid.
        assert_eq!(strip_conflict_suffix("file.conflict-"), None);
    }

    #[test]
    fn annotate_conflict_and_sync_filters_and_annotates() {
        use crate::model::{FileRow, Mime};

        let rows = vec![
            FileRow::local("report.pdf", Mime::Pdf, "100 KB", "now").with_mesh("oak"),
            FileRow::local(
                "report.pdf.conflict-20260529-oak",
                Mime::Pdf,
                "98 KB",
                "now",
            )
            .with_mesh("oak"),
            FileRow::local("notes.txt", Mime::Doc, "2 KB", "1 h ago").with_mesh("pine"),
        ];

        let annotated = annotate_conflict_and_sync(rows, false);
        // Conflict sibling is filtered out.
        assert_eq!(annotated.len(), 2);
        let report = annotated.iter().find(|r| r.name == "report.pdf").unwrap();
        assert!(report.has_conflict);
        assert_eq!(
            report.conflict_sibling.as_deref(),
            Some("report.pdf.conflict-20260529-oak")
        );
        // syncing=false when healing=false.
        assert!(!report.syncing);
    }

    #[test]
    fn annotate_conflict_and_sync_sets_syncing_when_healing() {
        use crate::model::{FileRow, Mime};

        let rows = vec![
            FileRow::local("file.txt", Mime::Doc, "1 KB", "now").with_mesh("oak"),
            FileRow::local("local.txt", Mime::Doc, "1 KB", "now"), // no mesh
        ];
        let annotated = annotate_conflict_and_sync(rows, true);
        let mesh_row = annotated.iter().find(|r| r.name == "file.txt").unwrap();
        assert!(
            mesh_row.syncing,
            "mesh-homed row must be syncing when healing"
        );
        let local_row = annotated.iter().find(|r| r.name == "local.txt").unwrap();
        assert!(!local_row.syncing, "local row must not be syncing");
    }

    #[test]
    fn parse_meshfs_healing_detects_under_replicated() {
        let json = r#"{"master_reachable":true,"goal":2,"peers":[{"addr":"10.0.0.1","used_bytes":0,"avail_bytes":0}],"offline_peers":[]}"#;
        assert!(parse_meshfs_healing(json), "1 peer < goal 2 → healing");
    }

    #[test]
    fn parse_meshfs_healing_false_when_healthy() {
        let json = r#"{"master_reachable":true,"goal":2,"peers":[{"addr":"10.0.0.1","used_bytes":0,"avail_bytes":0,"undergoal_chunks":0},{"addr":"10.0.0.2","used_bytes":0,"avail_bytes":0,"undergoal_chunks":0}],"offline_peers":[]}"#;
        assert!(!parse_meshfs_healing(json));
    }

    #[test]
    fn parse_meshfs_healing_false_when_master_down() {
        let json = r#"{"master_reachable":false,"goal":2,"peers":[],"offline_peers":["10.0.0.3"]}"#;
        assert!(
            !parse_meshfs_healing(json),
            "master down → cannot judge healing"
        );
    }

    #[test]
    fn parse_meshfs_healing_detects_offline_peers() {
        let json = r#"{"master_reachable":true,"goal":2,"peers":[{"addr":"10.0.0.1","used_bytes":0,"avail_bytes":0,"undergoal_chunks":0},{"addr":"10.0.0.2","used_bytes":0,"avail_bytes":0,"undergoal_chunks":0}],"offline_peers":["10.0.0.3"]}"#;
        assert!(parse_meshfs_healing(json), "offline peer → healing");
    }

    #[test]
    fn conflict_resolve_message_opens_dialog() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::ConflictResolve(
            "report.pdf".into(),
            "report.pdf.conflict-20260529-oak".into(),
        ));
        assert_eq!(
            s.resolve_dialog,
            Some((
                "report.pdf".to_owned(),
                "report.pdf.conflict-20260529-oak".to_owned()
            ))
        );
        assert!(s.conflict_error.is_none());
    }

    #[test]
    fn dismiss_conflict_dialog_closes_dialog() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::ConflictResolve(
            "a.txt".into(),
            "a.txt.conflict-x".into(),
        ));
        assert!(s.resolve_dialog.is_some());
        let _ = s.update(Message::DismissConflictDialog);
        assert!(s.resolve_dialog.is_none());
    }

    #[test]
    fn archive_conflict_file_clears_dialog() {
        let mut s = MdeFiles::default();
        s.resolve_dialog = Some(("a.txt".into(), "a.txt.conflict-x".into()));
        let _ = s.update(Message::ArchiveConflictFile("a.txt.conflict-x".into()));
        assert!(
            s.resolve_dialog.is_none(),
            "dialog must close on archive action"
        );
    }

    #[test]
    fn conflict_archived_ok_clears_error() {
        let mut s = MdeFiles::default();
        s.conflict_error = Some("prev error".into());
        let _ = s.update(Message::ConflictArchived(Ok(())));
        assert!(s.conflict_error.is_none());
    }

    #[test]
    fn conflict_archived_err_stores_error() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::ConflictArchived(Err("archive failed".into())));
        assert_eq!(s.conflict_error.as_deref(), Some("archive failed"));
    }

    #[test]
    fn meshfs_health_loaded_sets_healing_flag() {
        let mut s = MdeFiles::default();
        assert!(!s.meshfs_healing);
        let json = r#"{"master_reachable":true,"goal":2,"peers":[{"addr":"10.0.0.1","used_bytes":0,"avail_bytes":0,"undergoal_chunks":0}],"offline_peers":[]}"#;
        let _ = s.update(Message::MeshFsHealthLoaded(json.to_owned()));
        assert!(s.meshfs_healing);
    }

    // ── E10.5 tabs ───────────────────────────────────────────────────────────

    #[test]
    fn starts_with_one_tab() {
        let s = MdeFiles::default();
        assert_eq!(s.tabs.len(), 1);
        assert_eq!(s.active_tab, 0);
    }

    #[test]
    fn new_tab_appends_and_switches_to_it() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::NewTab);
        assert_eq!(s.tabs.len(), 2);
        assert_eq!(s.active_tab, 1);
        // A fresh tab lands on the default Mesh overview.
        assert_eq!(s.view, View::default());
    }

    #[test]
    fn switching_tabs_preserves_per_tab_state() {
        let mut s = MdeFiles::default();
        // Park tab 0 on a Local path.
        let _ = s.update(Message::LocalGoto("/tmp".to_string()));
        assert_eq!(s.view, View::Local);
        // Open a second tab and navigate it elsewhere.
        let _ = s.update(Message::NewTab);
        let _ = s.update(Message::LocalGoto("/usr".to_string()));
        assert_eq!(s.local_path, "/usr");
        // Back to tab 0 — its /tmp Local view is restored, not /usr.
        let _ = s.update(Message::SwitchTab(0));
        assert_eq!(s.active_tab, 0);
        assert_eq!(s.view, View::Local);
        assert_eq!(s.local_path, "/tmp");
    }

    #[test]
    fn cycle_tab_wraps() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::NewTab); // now 2 tabs, active 1
        let _ = s.update(Message::CycleTab); // 1 -> 0 (wrap)
        assert_eq!(s.active_tab, 0);
        let _ = s.update(Message::CycleTab); // 0 -> 1
        assert_eq!(s.active_tab, 1);
    }

    #[test]
    fn close_tab_keeps_at_least_one() {
        let mut s = MdeFiles::default();
        let _ = s.update(Message::CloseActiveTab); // single tab: no-op
        assert_eq!(s.tabs.len(), 1);
        let _ = s.update(Message::NewTab);
        let _ = s.update(Message::NewTab); // 3 tabs, active 2
        let _ = s.update(Message::CloseTab(0)); // remove first
        assert_eq!(s.tabs.len(), 2);
        // active was 2, index 0 removed -> shifts down to 1.
        assert_eq!(s.active_tab, 1);
    }
}
