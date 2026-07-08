//! The **Browser** surface — the sandboxed Servo browser rendered egui-native.
//!
//! BOOKMARKS-6 brokers the out-of-process `mde-web-preview` helper *into* the one
//! shell (the same EMBED model as the VDI Desktop surface): the helper renders
//! offscreen into a shared-memory frame; [`mde_web_preview_client`] receives that
//! frame fd over the per-session socket, maps it read-only, and hands the shell an
//! [`egui::ColorImage`]. This panel uploads that image to a `TextureHandle` on a
//! paint-ready (never a per-frame re-upload), paints it as the body, wires the
//! navigation chrome (back / forward / reload / address bar, §4 tokens) to the
//! control socket, and forwards this frame's egui input scaled by
//! `pixels_per_point`.
//!
//! ```text
//!   session.take_frame() ─▶ ColorImage ─▶ TextureHandle ─▶ ui paints the body
//!   chrome + ui.input     ───────────────────────────────▶ session control/input
//! ```
//!
//! Each tab is an independent [`WebSession`], so one page crashing surfaces an
//! honest "page crashed" state for THAT tab only (respawn-on-reload) and never
//! touches the others (per-session isolation). Spawning the live Servo helper is
//! the client crate's `live-helper` path, honest-gated to a GPU seat; with no live
//! session attached this surface shows an honest gated `EmptyState`, never a fake
//! page (§7).

use base64::Engine as _;
use mackes_mesh_types::peers::default_workgroup_root;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_chat::{MessageKind, Severity};
use mde_egui::egui::{self, RichText, Sense, TextureHandle, TextureOptions};
use mde_egui::{muted_note, Style};
use mde_files_egui::transfers::{
    FileTransfers, Method as TransferMethod, TransferJob, TransferPolicy, TransferState,
    TransferVerb, TransfersClient,
};

use mde_web_preview_client::{
    host_of, FilterListSource, FilterListStore, RequestFilter, SafeBrowsingBlocklist, SessionState,
    WebSession,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

// ── live-helper: spawning the real sandboxed `mde-web-preview` helper ──────────
//
// Gated behind `mde-shell-egui`'s `live-helper` feature, which turns on the client
// crate's `live-helper` spawn API ([`WebSession::spawn`] + [`SpawnSpec`]). OFF by
// default so the shell stays portable and the Browser surface shows its honest
// gated EmptyState (§7); ON, the surface spawns the sandboxed helper on first open.
#[cfg(feature = "live-helper")]
use mde_web_preview_client::session::SpawnSpec;

/// The sandboxed Servo helper binary the RPM installs; overridable via
/// [`SERVO_HELPER_BIN_ENV`] for the test bed / dev builds.
#[cfg(feature = "live-helper")]
const DEFAULT_SERVO_HELPER_BIN: &str = "/usr/bin/mde-web-preview";

/// The env var overriding [`DEFAULT_SERVO_HELPER_BIN`] (test bed / dev builds).
#[cfg(feature = "live-helper")]
const SERVO_HELPER_BIN_ENV: &str = "MDE_WEB_PREVIEW_BIN";

/// The Chromium/CEF helper binary once BROWSER-DD-1 is packaged.
#[cfg(feature = "live-helper")]
const DEFAULT_CEF_HELPER_BIN: &str = "/usr/bin/mde-web-cef";

/// The env var overriding [`DEFAULT_CEF_HELPER_BIN`] (test bed / dev builds).
#[cfg(feature = "live-helper")]
const CEF_HELPER_BIN_ENV: &str = "MDE_WEB_CEF_BIN";

/// Environment variable pointing at a pinned CEF bundle root (mirrors
/// `mde-web-cef`; duplicated here so the shell can gate honestly without
/// depending on the excluded helper crate).
#[cfg(feature = "live-helper")]
const CEF_ROOT_ENV: &str = "MDE_CEF_ROOT";

/// Conventional farm/vendor path for the pinned CEF bundle.
#[cfg(feature = "live-helper")]
const DEFAULT_CEF_ROOT: &str = "/opt/mde/cef";

/// The runtime library expected under the CEF bundle.
#[cfg(feature = "live-helper")]
const CEF_LIB_NAME: &str = "libcef.so";

/// CEF binary distributions install their runtime library under `Release/`.
#[cfg(feature = "live-helper")]
const CEF_RELEASE_DIR: &str = "Release";

/// CEF binary distributions install pak/ICU resources under `Resources/`.
#[cfg(feature = "live-helper")]
const CEF_RESOURCES_DIR: &str = "Resources";

#[cfg(feature = "live-helper")]
const CEF_ICU_DATA: &str = "icudtl.dat";

#[cfg(feature = "live-helper")]
const CEF_RESOURCES_PAK: &str = "resources.pak";

/// The native new-tab URL. A real helper session still loads this, while the shell
/// overlays the Quasar dashboard chrome for it.
const NEW_TAB_URL: &str = "about:blank";

/// The first page a freshly spawned live tab loads.
#[cfg(feature = "live-helper")]
const START_URL: &str = NEW_TAB_URL;

/// The initial helper view geometry (device px); the scaled body fills the panel,
/// and the helper repaints on the address bar's first navigation.
#[cfg(feature = "live-helper")]
const INIT_W: u32 = 1280;
#[cfg(feature = "live-helper")]
const INIT_H: u32 = 800;

const CHROME_FONT: f32 = 10.0;
const CHROME_BUTTON: f32 = 20.0;
const CHROME_TAB_H: f32 = 22.0;
const CHROME_TAB_W: f32 = 132.0;
const CHROME_TAB_CLOSE: f32 = 18.0;
const CHROME_NEW_TAB_W: f32 = 58.0;
const CHROME_OMNIBOX_H: f32 = 22.0;
const CHROME_GAP: f32 = 2.0;
const DEFAULT_DENIED_PERMISSIONS: &str = "location, camera, microphone, notifications, clipboard";
const PAGE_ZOOM_MIN: u16 = 50;
const PAGE_ZOOM_MAX: u16 = 200;
const PAGE_ZOOM_STEP: u16 = 10;
const CUPS_PRINT_TIMEOUT: Duration = Duration::from_secs(5);

/// Resolve the selected sandboxed-helper binary path.
#[cfg(feature = "live-helper")]
fn helper_bin_path(engine: BrowserEngine) -> std::path::PathBuf {
    let (env, default) = match engine {
        BrowserEngine::Servo => (SERVO_HELPER_BIN_ENV, DEFAULT_SERVO_HELPER_BIN),
        BrowserEngine::Cef => (CEF_HELPER_BIN_ENV, DEFAULT_CEF_HELPER_BIN),
    };
    std::env::var_os(env).map_or_else(
        || std::path::PathBuf::from(default),
        std::path::PathBuf::from,
    )
}

#[cfg(feature = "live-helper")]
fn cef_runtime_root() -> std::path::PathBuf {
    std::env::var_os(CEF_ROOT_ENV).map_or_else(
        || std::path::PathBuf::from(DEFAULT_CEF_ROOT),
        std::path::PathBuf::from,
    )
}

#[cfg(feature = "live-helper")]
fn cef_runtime_lib() -> std::path::PathBuf {
    let root = cef_runtime_root();
    let bundled = root.join(CEF_RELEASE_DIR).join(CEF_LIB_NAME);
    if bundled.is_file() {
        bundled
    } else {
        root.join(CEF_LIB_NAME)
    }
}

#[cfg(feature = "live-helper")]
fn cef_runtime_resources() -> std::path::PathBuf {
    let root = cef_runtime_root();
    let bundled = root.join(CEF_RESOURCES_DIR);
    if bundled.join(CEF_ICU_DATA).is_file() && bundled.join(CEF_RESOURCES_PAK).is_file() {
        bundled
    } else {
        root
    }
}

#[cfg(feature = "live-helper")]
fn cef_runtime_missing_path() -> Option<std::path::PathBuf> {
    let lib = cef_runtime_lib();
    if !lib.is_file() {
        return Some(lib);
    }
    let resources = cef_runtime_resources();
    for name in [CEF_ICU_DATA, CEF_RESOURCES_PAK] {
        let path = resources.join(name);
        if !path.is_file() {
            return Some(path);
        }
    }
    None
}

#[cfg(feature = "live-helper")]
fn preferred_default_engine() -> BrowserEngine {
    if helper_bin_path(BrowserEngine::Cef).is_file() && cef_runtime_missing_path().is_none() {
        BrowserEngine::Cef
    } else {
        BrowserEngine::Servo
    }
}

#[cfg(not(feature = "live-helper"))]
const fn preferred_default_engine() -> BrowserEngine {
    BrowserEngine::Servo
}

/// The browser body is scaled to fill the surface, so sample it linearly.
const BROWSER_TEX: TextureOptions = TextureOptions::LINEAR;

/// One browser tab: its driven session and the GPU texture its frames upload into.
struct Tab {
    /// The IPC + shm session driving one sandboxed helper.
    session: WebSession,
    /// Engine that owns this helper session.
    engine: BrowserEngine,
    /// Named container identity for the tab. Helpers are already one session per
    /// tab; this records the user-facing isolation bucket in the chrome.
    container: ContainerProfile,
    /// Browser UX intent for where this tab should land once the compositor-side
    /// multi-display handoff is wired. This is per-tab chrome state, not a fake
    /// output move.
    display_target: DisplayTarget,
    /// Per-tab audio mute state mirrored to the helper.
    muted: bool,
    /// Per-tab forced dark rendering state mirrored to the helper.
    force_dark: bool,
    /// Per-tab reader-mode state mirrored to the helper.
    reader_mode: bool,
    /// Last operator/page activity seen by the shell for idle-suspend accounting.
    last_activity: Instant,
    /// Whether this inactive tab has been shell-suspended after the idle timeout.
    idle_suspended: bool,
    /// The body texture — allocated on the first frame, then updated in place with
    /// [`TextureHandle::set`] on each subsequent paint-ready (egui reuses the
    /// allocation, so a live page is not a per-frame upload churn).
    texture: Option<TextureHandle>,
    /// Last helper frame retained on the CPU side for viewport capture. The GPU
    /// texture is not readable, so capture uses this exact pre-upload image.
    last_frame: Option<egui::ColorImage>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ContainerProfile {
    #[default]
    None,
    Personal,
    Work,
    Banking,
    Research,
}

impl ContainerProfile {
    const ALL: [Self; 5] = [
        Self::None,
        Self::Personal,
        Self::Work,
        Self::Banking,
        Self::Research,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::None => "No Container",
            Self::Personal => "Personal",
            Self::Work => "Work",
            Self::Banking => "Banking",
            Self::Research => "Research",
        }
    }

    const fn chip(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Personal => "Personal",
            Self::Work => "Work",
            Self::Banking => "Banking",
            Self::Research => "Research",
        }
    }

    const fn marker(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Personal => "P ",
            Self::Work => "W ",
            Self::Banking => "B ",
            Self::Research => "R ",
        }
    }

    const fn wire(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Personal => "personal",
            Self::Work => "work",
            Self::Banking => "banking",
            Self::Research => "research",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::None => Self::Personal,
            Self::Personal => Self::Work,
            Self::Work => Self::Banking,
            Self::Banking => Self::Research,
            Self::Research => Self::None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum DisplayTarget {
    #[default]
    Current,
    Primary,
    Secondary,
    AllDisplays,
}

impl DisplayTarget {
    const ALL: [Self; 4] = [
        Self::Current,
        Self::Primary,
        Self::Secondary,
        Self::AllDisplays,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Current => "Current Display",
            Self::Primary => "Primary Display",
            Self::Secondary => "Secondary Display",
            Self::AllDisplays => "All Displays",
        }
    }

    const fn chip(self) -> &'static str {
        match self {
            Self::Current => "",
            Self::Primary => "Display 1",
            Self::Secondary => "Display 2",
            Self::AllDisplays => "All Displays",
        }
    }

    const fn marker(self) -> &'static str {
        match self {
            Self::Current => "",
            Self::Primary => "D1 ",
            Self::Secondary => "D2 ",
            Self::AllDisplays => "DA ",
        }
    }

    const fn wire(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Primary => "primary",
            Self::Secondary => "secondary",
            Self::AllDisplays => "all_displays",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Current => Self::Primary,
            Self::Primary => Self::Secondary,
            Self::Secondary => Self::AllDisplays,
            Self::AllDisplays => Self::Current,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SiteDataRecord {
    host: String,
    open_tabs: u32,
    last_seen_ms: u64,
    cleared_count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SiteDataManager {
    sites: BTreeMap<String, SiteDataRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CupsPrintRequest {
    path: String,
    title: String,
    settings: CupsPrintSettings,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CupsPrinter {
    name: String,
    is_default: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CupsPrintSettings {
    destination: Option<String>,
    copies: u16,
    duplex: bool,
    grayscale: bool,
}

impl Default for CupsPrintSettings {
    fn default() -> Self {
        Self {
            destination: None,
            copies: 1,
            duplex: false,
            grayscale: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProcessOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

impl SiteDataManager {
    fn observe_open_tabs<'a>(&mut self, hosts: impl IntoIterator<Item = &'a str>, now_ms: u64) {
        let mut counts = BTreeMap::<String, u32>::new();
        for host in hosts {
            let host = host.trim().to_ascii_lowercase();
            if !host.is_empty() {
                *counts.entry(host).or_insert(0) += 1;
            }
        }
        let active_hosts = counts.keys().cloned().collect::<BTreeSet<_>>();
        for (host, open_tabs) in counts {
            let record = self
                .sites
                .entry(host.clone())
                .or_insert_with(|| SiteDataRecord {
                    host,
                    ..SiteDataRecord::default()
                });
            record.open_tabs = open_tabs;
            record.last_seen_ms = now_ms;
        }
        for (host, record) in &mut self.sites {
            if !active_hosts.contains(host) {
                record.open_tabs = 0;
            }
        }
    }

    fn mark_cleared(&mut self, host: &str, now_ms: u64) {
        let host = host.trim().to_ascii_lowercase();
        if host.is_empty() {
            return;
        }
        let record = self
            .sites
            .entry(host.clone())
            .or_insert_with(|| SiteDataRecord {
                host,
                ..SiteDataRecord::default()
            });
        record.cleared_count = record.cleared_count.saturating_add(1);
        record.last_seen_ms = now_ms;
    }

    fn summary(&self, active_host: Option<&str>) -> String {
        if self.sites.is_empty() {
            return "Site data: no visited sites tracked".to_owned();
        }
        let open_tabs = self.sites.values().map(|s| s.open_tabs).sum::<u32>();
        let cleared = active_host
            .and_then(|host| self.sites.get(host))
            .map_or(0, |s| s.cleared_count);
        match active_host {
            Some(host) => format!(
                "Site data: {} tracked site{} · {open_tabs} open tab{} · {host} cleared {cleared} time{}",
                self.sites.len(),
                plural(self.sites.len()),
                plural_u32(open_tabs),
                plural_u32(cleared),
            ),
            None => format!(
                "Site data: {} tracked site{} · {open_tabs} open tab{}",
                self.sites.len(),
                plural(self.sites.len()),
                plural_u32(open_tabs),
            ),
        }
    }
}

const fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

const fn plural_u32(count: u32) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

const DOWNLOADS_POLL_INTERVAL: Duration = Duration::from_secs(1);
const SEND_TAB_POLL_INTERVAL: Duration = Duration::from_secs(1);
const IDLE_TAB_SUSPEND_AFTER: Duration = Duration::from_secs(30 * 60);

const fn download_state_rank(state: TransferState) -> u8 {
    match state {
        TransferState::Running => 0,
        TransferState::Queued => 1,
        TransferState::Paused => 2,
        TransferState::Failed => 3,
        TransferState::Done => 4,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TabOpenIntent {
    NewForeground(BrowserEngine),
    NewForegroundUrl { engine: BrowserEngine, url: String },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum BrowserEngine {
    #[default]
    Servo,
    Cef,
}

impl BrowserEngine {
    const fn label(self) -> &'static str {
        match self {
            Self::Servo => "Servo",
            Self::Cef => "CEF",
        }
    }

    const fn wire(self) -> &'static str {
        match self {
            Self::Servo => "servo",
            Self::Cef => "cef",
        }
    }

    fn from_wire(s: &str) -> Option<Self> {
        match s {
            "servo" => Some(Self::Servo),
            "cef" => Some(Self::Cef),
            _ => None,
        }
    }
}

/// The Browser surface's state: the open tabs, the active one, and the address-bar
/// edit buffer.
pub(crate) struct WebState {
    /// The open browser tabs (each an isolated session). Empty until a session
    /// attaches — spawning the live helper is the gated `live-helper` path.
    tabs: Vec<Tab>,
    /// Index of the active tab in [`Self::tabs`].
    active: usize,
    /// Engine selected for future live-helper tabs.
    #[cfg_attr(
        not(any(test, feature = "live-helper")),
        allow(dead_code, reason = "read by live-helper spawning and Browser tests")
    )]
    engine: BrowserEngine,
    /// The address-bar edit buffer for the active tab.
    address: String,
    /// Set when Reload is pressed on a *crashed* active tab — the shell (or a test)
    /// drains it and swaps in a fresh session (respawn-on-reload).
    respawn_requested: bool,
    /// Set by the visible tab strip's `+` button or the session-restore seam.
    /// Live-helper builds drain this by spawning helper tabs; portable builds
    /// surface the honest gate only.
    open_requested: VecDeque<TabOpenIntent>,
    /// Set when Browser chrome asks to open the rich Bookmarks manager surface.
    open_bookmarks_requested: bool,
    /// BROWSER-DD-2 vertical-tabs preference. This is purely chrome layout: it
    /// reuses the same tab/session operations and never creates a second tab model.
    vertical_tabs: bool,
    /// HTTPS-only prompt latch. Explicit `http://` navigations pause here until
    /// the operator upgrades to HTTPS, continues over HTTP, or cancels.
    insecure_prompt: Option<String>,
    /// Quasar new-tab dashboard search draft. This is chrome state, not page
    /// content; submitted searches load the mesh SearXNG URL into the active tab.
    dashboard_query: String,
    /// New-tab speed-dial shortcuts. These start with mesh-local defaults but are
    /// browser state so session sync can carry an operator's current dashboard.
    speed_dial: Vec<SpeedDialEntry>,
    /// Page find draft shown in the compact find bar.
    find_query: String,
    /// Whether the compact find bar is open.
    find_open: bool,
    /// Current page zoom percentage sent to the active helper.
    page_zoom_percent: u16,
    /// BROWSER-DD-2 live SearXNG suggestions for the omnibox. Suggestions are
    /// fetched off-thread from the mesh-local service; the UI only polls this
    /// small state object so typing never blocks a frame.
    suggestions: SuggestionState,
    /// BROWSER-DD-3 native blocker policy. Starts with the bundled seed lists so
    /// tracker blocking is default-on offline; synced/custom sources and per-site
    /// toggles mutate this store, then every open tab receives a freshly compiled
    /// [`RequestFilter`].
    adfilter_store: FilterListStore,
    /// Mesh-hosted safe-browsing host blocklists. The worker/file-sync half can
    /// replace these hosts; the Browser compiles them into every live session.
    safe_browsing_hosts: Vec<String>,
    /// BROWSER-DD-3 per-site permission manager. The helpers enforce default-deny
    /// for sensitive prompts; the shell tracks sites the operator explicitly
    /// forgot so the menu has a real state transition without offering fake allow
    /// toggles the engines cannot honor yet.
    forgotten_permission_sites: Vec<String>,
    /// BROWSER-DD-3 per-site data manager. Tracks committed first-party hosts,
    /// live tab counts, last-seen timestamps, and explicit clear actions.
    site_data: SiteDataManager,
    /// Shared Transfers client. Browser downloads are just `browser_download`
    /// rows in the daemon-owned ledger, so Files and Browser show one queue.
    transfers: Box<dyn TransfersClient>,
    /// Browser-originated transfers filtered from the shared ledger.
    download_jobs: Vec<TransferJob>,
    /// Browser transfer ids already announced to the mesh notification feed.
    notified_downloads: BTreeSet<String>,
    /// Last `action/browser/session-sync` body published. Keeps unchanged frames
    /// and ledger refreshes from flooding the Bus while still making every state
    /// transition observable.
    last_session_sync_body: Option<String>,
    /// One-shot startup restore latch. The Browser reads the daemon-owned latest
    /// session-sync snapshot once, before the live-helper blank-tab fallback.
    startup_restore_attempted: bool,
    /// Candidate roots for daemon-persisted startup restore snapshots. Production
    /// probes the local durable root first, then the Syncthing-backed workgroup
    /// root; tests inject temp roots without touching operator state.
    session_restore_roots: Vec<PathBuf>,
    /// Last time the Browser scanned the daemon-owned send-tab outbox for concrete
    /// node-addressed records.
    incoming_send_tab_last_poll: Option<Instant>,
    /// Whether the compact download manager drawer is visible.
    downloads_open: bool,
    /// Last time the browser refreshed its ledger view.
    downloads_last_poll: Option<Instant>,
    /// Last lifecycle dispatch failure, shown inline instead of being swallowed.
    download_notice: Option<String>,
    /// Last viewport-capture result, shown inline instead of being swallowed.
    capture_notice: Option<String>,
    /// Last successfully saved user PDF. CUPS spool PDFs are excluded; this feeds
    /// the CEF-backed built-in PDF viewer action.
    last_saved_pdf: Option<PathBuf>,
    /// Compact CUPS destination/options drawer for Browser print jobs.
    print_settings_open: bool,
    /// Locally discovered CUPS destinations from `lpstat -e` plus default marker.
    cups_printers: Vec<CupsPrinter>,
    /// Last CUPS destination discovery error, shown inside the print drawer.
    cups_notice: Option<String>,
    /// Print options applied when the helper-produced PDF is submitted to CUPS.
    cups_settings: CupsPrintSettings,
    /// PDFs waiting for helper confirmation before submission to CUPS via `lp`.
    pending_cups_prints: BTreeMap<String, CupsPrintRequest>,
    /// Bus root for Browser-owned platform handoff actions. Defaults to the node
    /// client data dir; tests inject a temp root so persisted actions are asserted
    /// without touching the operator's real bus.
    bus_root: Option<PathBuf>,
    /// Region-capture mode is armed; the next drag over the page image writes a
    /// cropped PNG from the retained helper frame.
    capture_region_mode: bool,
    /// Region-capture drag anchor in helper-frame pixel coordinates.
    capture_region_start: Option<egui::Pos2>,
    /// Region-capture current drag point in helper-frame pixel coordinates.
    capture_region_current: Option<egui::Pos2>,
    /// An honest gated notice shown in place of the `EmptyState` when a `live-helper`
    /// open couldn't proceed (no seat · helper binary absent · spawn failed). `None`
    /// = the default gated caption. Only ever set on the live path — a named reason,
    /// never a fake page (§7).
    #[cfg(feature = "live-helper")]
    gate_notice: Option<String>,
    /// One-shot latch so the real `Command::spawn` is attempted at most once per
    /// surface lifetime — a spawn *failure* must not respawn a process every frame.
    #[cfg(feature = "live-helper")]
    spawn_attempted: bool,
}

impl Default for WebState {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            engine: preferred_default_engine(),
            address: String::new(),
            respawn_requested: false,
            open_requested: VecDeque::new(),
            open_bookmarks_requested: false,
            vertical_tabs: false,
            insecure_prompt: None,
            dashboard_query: String::new(),
            speed_dial: default_speed_dial(),
            find_query: String::new(),
            find_open: false,
            page_zoom_percent: 100,
            suggestions: SuggestionState::default(),
            adfilter_store: FilterListStore::with_bundled(),
            safe_browsing_hosts: Vec::new(),
            forgotten_permission_sites: Vec::new(),
            site_data: SiteDataManager::default(),
            transfers: Box::new(FileTransfers::from_env()),
            download_jobs: Vec::new(),
            notified_downloads: BTreeSet::new(),
            last_session_sync_body: None,
            startup_restore_attempted: false,
            session_restore_roots: default_session_restore_roots(),
            incoming_send_tab_last_poll: None,
            downloads_open: false,
            downloads_last_poll: None,
            download_notice: None,
            capture_notice: None,
            last_saved_pdf: None,
            print_settings_open: false,
            cups_printers: Vec::new(),
            cups_notice: None,
            cups_settings: CupsPrintSettings::default(),
            pending_cups_prints: BTreeMap::new(),
            bus_root: mde_bus::client_data_dir(),
            capture_region_mode: false,
            capture_region_start: None,
            capture_region_current: None,
            #[cfg(feature = "live-helper")]
            gate_notice: None,
            #[cfg(feature = "live-helper")]
            spawn_attempted: false,
        }
    }
}

impl WebState {
    /// The active tab, if any.
    fn active_tab(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    #[cfg(test)]
    fn with_transfers(mut self, transfers: Box<dyn TransfersClient>) -> Self {
        self.transfers = transfers;
        self.refresh_downloads();
        self
    }

    #[cfg(test)]
    fn with_bus_root(mut self, bus_root: Option<PathBuf>) -> Self {
        self.bus_root = bus_root;
        self
    }

    #[cfg(test)]
    fn with_session_restore_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.session_restore_roots = roots;
        self.startup_restore_attempted = false;
        self
    }

    /// Refresh the Browser's view of daemon-owned transfer jobs, keeping only
    /// browser-originated rows and prioritizing active work over history.
    fn refresh_downloads(&mut self) {
        let mut jobs: Vec<TransferJob> = self
            .transfers
            .jobs()
            .into_iter()
            .filter(|job| job.method == TransferMethod::BrowserDownload)
            .collect();
        jobs.sort_by(|a, b| {
            download_state_rank(a.state)
                .cmp(&download_state_rank(b.state))
                .then_with(|| b.updated_ms.cmp(&a.updated_ms))
                .then_with(|| b.created_ms.cmp(&a.created_ms))
        });
        if self.downloads_last_poll.is_some() {
            for job in &jobs {
                self.publish_download_notification(job);
            }
        }
        self.download_jobs = jobs;
        self.downloads_last_poll = Some(Instant::now());
        self.publish_session_snapshot();
    }

    fn publish_session_snapshot(&mut self) {
        let body = browser_session_sync_body(self);
        if self.last_session_sync_body.as_deref() == Some(body.as_str()) {
            return;
        }
        publish_to_bus(self.bus_root.as_deref(), ACTION_BROWSER_SESSION_SYNC, &body);
        self.last_session_sync_body = Some(body);
    }

    fn publish_download_notification(&mut self, job: &TransferJob) {
        let (severity, summary) = match job.state {
            TransferState::Done => (
                Severity::Info,
                format!("Browser download complete: {}", short_transfer_name(job)),
            ),
            TransferState::Failed => (
                Severity::Warning,
                format!("Browser download failed: {}", short_transfer_name(job)),
            ),
            _ => return,
        };
        if !self.notified_downloads.insert(job.id.clone()) {
            return;
        }
        let body = browser_notify_body(
            severity,
            &summary,
            Some(&format!("{} -> {}", job.source, job.dest)),
        );
        publish_to_bus(self.bus_root.as_deref(), EVENT_NOTIFY_BROWSER, &body);
    }

    /// Poll the transfer ledger at a UI-safe cadence. The client reads local files,
    /// but the Browser still avoids scanning it every paint.
    fn poll_downloads(&mut self) {
        if self
            .downloads_last_poll
            .is_some_and(|last| last.elapsed() < DOWNLOADS_POLL_INTERVAL)
        {
            return;
        }
        self.refresh_downloads();
    }

    fn mark_tab_active(&mut self, index: usize) {
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.last_activity = Instant::now();
            tab.idle_suspended = false;
        }
    }

    fn mark_active_tab_activity(&mut self) {
        self.mark_tab_active(self.active);
    }

    fn suspend_idle_tabs(&mut self, now: Instant) {
        let mut suspended = Vec::new();
        for (idx, tab) in self.tabs.iter_mut().enumerate() {
            if idx == self.active || tab.idle_suspended || tab.session.is_crashed() {
                continue;
            }
            if now.duration_since(tab.last_activity) < IDLE_TAB_SUSPEND_AFTER {
                continue;
            }
            tab.session.stop();
            tab.idle_suspended = true;
            suspended.push((
                idx,
                tab.engine,
                tab.session.nav().url.clone(),
                tab.session.title().to_owned(),
            ));
        }
        for (idx, engine, url, title) in suspended {
            let body = browser_tab_suspend_body(idx, engine, &url, &title, IDLE_TAB_SUSPEND_AFTER);
            publish_to_bus(self.bus_root.as_deref(), ACTION_BROWSER_TAB_SUSPEND, &body);
        }
        self.publish_session_snapshot();
    }

    fn download_counts(&self) -> (usize, usize) {
        (
            self.download_jobs
                .iter()
                .filter(|job| job.state.is_active())
                .count(),
            self.download_jobs.len(),
        )
    }

    fn dispatch_download_verb(&mut self, verb: TransferVerb) {
        match self.transfers.dispatch(&verb) {
            Ok(()) => {
                self.download_notice = None;
                self.refresh_downloads();
            }
            Err(err) => self.download_notice = Some(err),
        }
    }

    /// Append a session as a new tab and make it active. The live helper-spawn open
    /// path (gated) and the tests both funnel through here; the default gated build
    /// opens no tabs and shows the honest `EmptyState`, so this is unused there.
    #[cfg(test)]
    pub(crate) fn push_session(&mut self, session: WebSession) {
        self.push_session_with_engine(session, self.engine);
    }

    #[cfg_attr(
        not(any(test, feature = "live-helper")),
        allow(dead_code, reason = "used by live-helper spawning and Browser tests")
    )]
    fn push_session_with_engine(&mut self, session: WebSession, engine: BrowserEngine) {
        let mut session = session;
        let url = session.nav().url.clone();
        session.set_filter(self.compiled_request_filter_for_url(&url));
        self.tabs.push(Tab {
            session,
            engine,
            container: ContainerProfile::None,
            display_target: DisplayTarget::Current,
            muted: false,
            force_dark: false,
            reader_mode: false,
            last_activity: Instant::now(),
            idle_suspended: false,
            texture: None,
            last_frame: None,
        });
        self.active = self.tabs.len() - 1;
        self.publish_session_snapshot();
    }

    /// Request a foreground tab. The surface owns the visible affordance; the shell
    /// live-helper path owns the process spawn, so tests and portable builds can
    /// assert the intent without fabricating a helper.
    fn request_new_tab(&mut self, engine: BrowserEngine) {
        self.open_requested
            .push_back(TabOpenIntent::NewForeground(engine));
    }

    fn request_new_tab_with_url(&mut self, engine: BrowserEngine, url: String) {
        self.open_requested
            .push_back(TabOpenIntent::NewForegroundUrl { engine, url });
    }

    /// Apply a Browser session-sync snapshot from the future mesh sync owner by
    /// restoring shell-owned settings and enqueueing tab opens through the existing
    /// live-helper path. The active tab is queued last because each live open
    /// foregrounds the newly attached helper.
    fn restore_session_sync_snapshot(&mut self, body: &str) -> Result<usize, String> {
        let v: serde_json::Value =
            serde_json::from_str(body).map_err(|err| format!("session snapshot JSON: {err}"))?;
        if v.get("op").and_then(serde_json::Value::as_str) != Some("browser_session_sync") {
            return Err("session snapshot has the wrong op".to_owned());
        }
        let settings = v.get("settings").unwrap_or(&serde_json::Value::Null);
        if let Some(engine) = settings
            .get("future_engine")
            .and_then(serde_json::Value::as_str)
            .and_then(BrowserEngine::from_wire)
        {
            self.engine = engine;
        }
        if let Some(vertical) = settings
            .get("vertical_tabs")
            .and_then(serde_json::Value::as_bool)
        {
            self.vertical_tabs = vertical;
        }
        if let Some(zoom) = settings
            .get("page_zoom_percent")
            .and_then(serde_json::Value::as_u64)
        {
            self.page_zoom_percent = u16::try_from(zoom)
                .unwrap_or(PAGE_ZOOM_MAX)
                .clamp(PAGE_ZOOM_MIN, PAGE_ZOOM_MAX);
        }
        if let Some(find_open) = settings
            .get("find_open")
            .and_then(serde_json::Value::as_bool)
        {
            self.find_open = find_open;
        }
        if let Some(downloads_open) = settings
            .get("downloads_open")
            .and_then(serde_json::Value::as_bool)
        {
            self.downloads_open = downloads_open;
        }
        if let Some(speed_dial) = speed_dial_from_settings(settings) {
            self.speed_dial = speed_dial;
        }

        let active_index = v.get("active_index").and_then(serde_json::Value::as_u64);
        let tabs = v
            .get("tabs")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "session snapshot is missing tabs[]".to_owned())?;
        let mut restore_tabs = Vec::new();
        for (fallback_index, tab) in tabs.iter().enumerate() {
            let url = tab
                .get("url")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim();
            if url.is_empty() {
                continue;
            }
            let engine = tab
                .get("engine")
                .and_then(serde_json::Value::as_str)
                .and_then(BrowserEngine::from_wire)
                .unwrap_or(self.engine);
            let index = tab
                .get("index")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(fallback_index as u64);
            restore_tabs.push((index, engine, url.to_owned()));
        }
        restore_tabs.sort_by_key(|(index, _, _)| *index);
        if let Some(active_index) = active_index {
            restore_tabs.sort_by_key(|(index, _, _)| (*index == active_index, *index));
        }
        self.open_requested.clear();
        let count = restore_tabs.len();
        for (_, engine, url) in restore_tabs {
            self.request_new_tab_with_url(engine, url);
        }
        Ok(count)
    }

    /// One-shot startup restore from the daemon-owned latest snapshot files. The
    /// helper-spawn path drains the resulting open queue, so restore and ordinary
    /// new-tab creation stay on the same code path.
    fn restore_startup_session_once(&mut self) -> Option<usize> {
        if self.startup_restore_attempted {
            return None;
        }
        self.startup_restore_attempted = true;
        let host = local_hostname();
        for root in self.session_restore_roots.clone() {
            let path = session_sync_latest_path(&root, &host);
            let Ok(body) = std::fs::read_to_string(&path) else {
                continue;
            };
            match self.restore_session_sync_snapshot(&body) {
                Ok(0) => continue,
                Ok(count) => return Some(count),
                Err(_) => continue,
            }
        }
        None
    }

    fn drain_incoming_send_tabs(&mut self) -> usize {
        let host = local_hostname();
        let sanitized_host = sanitize_session_host(&host);
        let mut opened = 0;
        let mut seen = BTreeSet::new();
        for root in self.session_restore_roots.clone() {
            let inbox = send_tab_inbox_dir(&root, &host);
            for path in incoming_send_tab_files(&root, &host) {
                let key = path
                    .strip_prefix(&inbox)
                    .map(|rel| rel.to_string_lossy().to_string())
                    .unwrap_or_else(|_| path.to_string_lossy().to_string());
                if !seen.insert(key) {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                let Ok(body) = std::fs::read_to_string(&path) else {
                    continue;
                };
                match browser_send_tab_open_intent(&body, &sanitized_host) {
                    Ok((engine, url)) => {
                        self.request_new_tab_with_url(engine, url);
                        let _ = std::fs::remove_file(&path);
                        opened += 1;
                    }
                    Err(_) => continue,
                }
            }
        }
        opened
    }

    fn poll_incoming_send_tabs(&mut self) {
        if self
            .incoming_send_tab_last_poll
            .is_some_and(|last| last.elapsed() < SEND_TAB_POLL_INTERVAL)
        {
            return;
        }
        self.incoming_send_tab_last_poll = Some(Instant::now());
        self.drain_incoming_send_tabs();
    }

    fn request_bookmarks_manager(&mut self) {
        self.open_bookmarks_requested = true;
    }

    pub(crate) fn take_bookmarks_manager_request(&mut self) -> bool {
        std::mem::take(&mut self.open_bookmarks_requested)
    }

    /// Drain a pending new-tab request.
    #[cfg_attr(
        not(any(test, feature = "live-helper")),
        allow(dead_code, reason = "drained by the live-helper shell path and tests")
    )]
    fn take_open_request(&mut self) -> Option<TabOpenIntent> {
        self.open_requested.pop_front()
    }

    /// Select an existing tab. Out-of-range indices are ignored so stale UI events
    /// cannot panic after a close.
    fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
            self.mark_tab_active(index);
            self.sync_address_from_active();
            self.publish_session_snapshot();
        }
    }

    /// Close a tab and keep a stable active index. The helper child is killed by
    /// `WebSession`'s `Drop`, so this is the real process-teardown path.
    fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.active = 0;
            self.address.clear();
        } else if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
            self.sync_address_from_active();
        } else if index <= self.active {
            self.active = self.active.saturating_sub(1);
            self.sync_address_from_active();
        }
        self.publish_session_snapshot();
    }

    /// Move one tab to a new index while preserving which session is active.
    fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let active_tab = self.active;
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = if active_tab == from {
            to
        } else if from < active_tab && to >= active_tab {
            active_tab.saturating_sub(1)
        } else if from > active_tab && to <= active_tab {
            active_tab + 1
        } else {
            active_tab
        };
        self.sync_address_from_active();
        self.publish_session_snapshot();
    }

    fn set_vertical_tabs(&mut self, enabled: bool) {
        self.vertical_tabs = enabled;
        self.publish_session_snapshot();
    }

    fn toggle_vertical_tabs(&mut self) {
        self.set_vertical_tabs(!self.vertical_tabs);
    }

    #[cfg(test)]
    #[cfg_attr(
        not(feature = "live-helper"),
        allow(dead_code, reason = "used by live-helper-only Browser tests")
    )]
    fn select_engine(&mut self, engine: BrowserEngine) {
        self.engine = engine;
    }

    fn submit_address(&mut self) {
        let crashed = self
            .tabs
            .get(self.active)
            .is_some_and(|t| t.session.is_crashed());
        if self.tabs.is_empty() || crashed {
            return;
        }
        let Some(url) = omnibox_target(&self.address) else {
            return;
        };
        self.suggestions.clear();
        self.address = url.clone();
        self.load_target(url);
    }

    fn load_target(&mut self, url: String) {
        if is_plain_http(&url) {
            self.insecure_prompt = Some(url);
            return;
        }
        if let Some(protocol) = ExternalProtocol::from_url(&url) {
            self.insecure_prompt = None;
            self.publish_external_protocol(protocol, &url);
            return;
        }
        self.insecure_prompt = None;
        self.mark_active_tab_activity();
        if let Some(tab) = self.active_tab() {
            tab.session.load(url);
        }
    }

    fn publish_external_protocol(&mut self, protocol: ExternalProtocol, url: &str) {
        match protocol {
            ExternalProtocol::Tel => {
                let body = voice_dial_body(url);
                publish_to_bus(self.bus_root.as_deref(), ACTION_VOICE_DIAL, &body);
            }
            ExternalProtocol::Mailto | ExternalProtocol::Magnet => {
                let body = browser_protocol_handoff_body(protocol, url);
                publish_to_bus(self.bus_root.as_deref(), ACTION_BROWSER_PROTOCOL, &body);
            }
        }
        self.capture_notice = Some(format!(
            "Handed {} link to {}",
            protocol.scheme(),
            protocol.target_label()
        ));
    }

    fn submit_dashboard_search(&mut self) {
        let q = self.dashboard_query.trim();
        if q.is_empty() {
            return;
        }
        let url = format!("{DEFAULT_SEARCH_URL}?q={}", percent_encode_query(q));
        self.address = url.clone();
        self.load_target(url);
    }

    fn open_mesh_service(&mut self, url: String) {
        self.address = url.clone();
        self.load_target(url);
    }

    fn continue_insecure_load(&mut self) {
        let Some(url) = self.insecure_prompt.take() else {
            return;
        };
        self.address = url.clone();
        self.mark_active_tab_activity();
        if let Some(tab) = self.active_tab() {
            tab.session.load(url);
        }
    }

    fn upgrade_insecure_load(&mut self) {
        let Some(url) = self.insecure_prompt.take() else {
            return;
        };
        let upgraded = https_upgrade(&url);
        self.address = upgraded.clone();
        self.mark_active_tab_activity();
        if let Some(tab) = self.active_tab() {
            tab.session.load(upgraded);
        }
    }

    fn cancel_insecure_load(&mut self) {
        self.insecure_prompt = None;
    }

    fn clear_active_session_data(&mut self) {
        let cleared_host = self.active_first_party();
        self.mark_active_tab_activity();
        self.insecure_prompt = None;
        self.dashboard_query.clear();
        self.find_query.clear();
        self.find_open = false;
        self.page_zoom_percent = 100;
        self.suggestions.clear();
        self.address = NEW_TAB_URL.to_owned();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.texture = None;
            tab.last_frame = None;
            tab.muted = false;
            tab.force_dark = false;
            tab.reader_mode = false;
            tab.session.load(NEW_TAB_URL);
            tab.session.set_zoom(self.page_zoom_percent);
            tab.session.clear_find();
            tab.session.set_audio_muted(false);
            tab.session.set_force_dark(false);
            tab.session.set_reader_mode(false);
        }
        if let Some(host) = cleared_host {
            self.site_data.mark_cleared(&host, unix_ms());
        }
    }

    fn active_tab_has_frame(&self) -> bool {
        self.tabs
            .get(self.active)
            .is_some_and(|tab| tab.last_frame.is_some() && !tab.session.is_crashed())
    }

    fn capture_active_viewport(&mut self) {
        match self.capture_active_viewport_to_dir(browser_capture_dir()) {
            Ok(path) => {
                self.record_capture_success("Captured", &path);
            }
            Err(err) => {
                self.capture_notice = Some(format!("Capture failed: {err}"));
            }
        }
    }

    fn capture_active_full_page(&mut self) {
        match self.capture_active_full_page_to_dir(browser_capture_dir()) {
            Ok(path) => {
                self.record_capture_success("Captured full page", &path);
            }
            Err(err) => {
                self.capture_notice = Some(format!("Capture failed: {err}"));
            }
        }
    }

    fn capture_active_mhtml(&mut self) {
        match self.capture_active_mhtml_to_dir(browser_capture_dir()) {
            Ok(path) => {
                self.record_capture_success("Captured MHTML", &path);
            }
            Err(err) => {
                self.capture_notice = Some(format!("Capture failed: {err}"));
            }
        }
    }

    fn capture_active_annotated_viewport(&mut self) {
        match self.capture_active_annotated_viewport_to_dir(browser_capture_dir()) {
            Ok(path) => {
                self.record_capture_success("Captured annotated", &path);
            }
            Err(err) => {
                self.capture_notice = Some(format!("Capture failed: {err}"));
            }
        }
    }

    fn capture_active_callout_viewport(&mut self) {
        match self.capture_active_callout_viewport_to_dir(browser_capture_dir()) {
            Ok(path) => {
                self.record_capture_success("Captured callout", &path);
            }
            Err(err) => {
                self.capture_notice = Some(format!("Capture failed: {err}"));
            }
        }
    }

    fn capture_active_freehand_viewport(&mut self) {
        match self.capture_active_freehand_viewport_to_dir(browser_capture_dir()) {
            Ok(path) => {
                self.record_capture_success("Captured freehand", &path);
            }
            Err(err) => {
                self.capture_notice = Some(format!("Capture failed: {err}"));
            }
        }
    }

    fn record_capture_success(&mut self, label: &str, path: &Path) {
        let notice = format!("{label} {}", path.display());
        self.capture_notice = Some(notice.clone());
        let body = browser_notify_body(Severity::Info, &notice, Some(&path.to_string_lossy()));
        publish_to_bus(self.bus_root.as_deref(), EVENT_NOTIFY_BROWSER, &body);
    }

    fn capture_active_viewport_to_dir(&self, dir: impl AsRef<Path>) -> Result<PathBuf, String> {
        let Some(tab) = self.tabs.get(self.active) else {
            return Err("no active tab".to_owned());
        };
        if tab.session.is_crashed() {
            return Err("active tab is crashed".to_owned());
        }
        let Some(frame) = tab.last_frame.as_ref() else {
            return Err("the active tab has not painted yet".to_owned());
        };
        let bytes = encode_color_image_png(frame)?;
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("could not create {}: {err}", dir.display()))?;
        let name = capture_filename_for(&tab.session.nav().url, tab.session.title(), unix_ms());
        let path = dir.join(name);
        std::fs::write(&path, bytes)
            .map_err(|err| format!("could not write {}: {err}", path.display()))?;
        Ok(path)
    }

    fn capture_active_full_page_to_dir(&self, dir: impl AsRef<Path>) -> Result<PathBuf, String> {
        let Some(tab) = self.tabs.get(self.active) else {
            return Err("no active tab".to_owned());
        };
        if tab.session.is_crashed() {
            return Err("active tab is crashed".to_owned());
        }
        let Some(frame) = tab.last_frame.as_ref() else {
            return Err("the active tab has not painted yet".to_owned());
        };
        let bytes = encode_color_image_png(frame)?;
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("could not create {}: {err}", dir.display()))?;
        let name =
            capture_full_page_filename_for(&tab.session.nav().url, tab.session.title(), unix_ms());
        let path = dir.join(name);
        std::fs::write(&path, bytes)
            .map_err(|err| format!("could not write {}: {err}", path.display()))?;
        Ok(path)
    }

    fn capture_active_mhtml_to_dir(&self, dir: impl AsRef<Path>) -> Result<PathBuf, String> {
        let Some(tab) = self.tabs.get(self.active) else {
            return Err("no active tab".to_owned());
        };
        if tab.session.is_crashed() {
            return Err("active tab is crashed".to_owned());
        }
        let Some(frame) = tab.last_frame.as_ref() else {
            return Err("the active tab has not painted yet".to_owned());
        };
        let now_ms = unix_ms();
        let png = encode_color_image_png(frame)?;
        let mhtml =
            mhtml_capture_document(&tab.session.nav().url, tab.session.title(), now_ms, &png);
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("could not create {}: {err}", dir.display()))?;
        let name = capture_mhtml_filename_for(&tab.session.nav().url, tab.session.title(), now_ms);
        let path = dir.join(name);
        std::fs::write(&path, mhtml)
            .map_err(|err| format!("could not write {}: {err}", path.display()))?;
        Ok(path)
    }

    fn capture_active_annotated_viewport_to_dir(
        &self,
        dir: impl AsRef<Path>,
    ) -> Result<PathBuf, String> {
        let Some(tab) = self.tabs.get(self.active) else {
            return Err("no active tab".to_owned());
        };
        if tab.session.is_crashed() {
            return Err("active tab is crashed".to_owned());
        }
        let Some(frame) = tab.last_frame.as_ref() else {
            return Err("the active tab has not painted yet".to_owned());
        };
        let now_ms = unix_ms();
        let caption =
            capture_annotation_caption(&tab.session.nav().url, tab.session.title(), now_ms);
        let annotated = annotate_capture_image(frame, &caption)?;
        let bytes = encode_color_image_png(&annotated)?;
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("could not create {}: {err}", dir.display()))?;
        let name =
            capture_annotated_filename_for(&tab.session.nav().url, tab.session.title(), now_ms);
        let path = dir.join(name);
        std::fs::write(&path, bytes)
            .map_err(|err| format!("could not write {}: {err}", path.display()))?;
        Ok(path)
    }

    fn capture_active_callout_viewport_to_dir(
        &self,
        dir: impl AsRef<Path>,
    ) -> Result<PathBuf, String> {
        let Some(tab) = self.tabs.get(self.active) else {
            return Err("no active tab".to_owned());
        };
        if tab.session.is_crashed() {
            return Err("active tab is crashed".to_owned());
        }
        let Some(frame) = tab.last_frame.as_ref() else {
            return Err("the active tab has not painted yet".to_owned());
        };
        let now_ms = unix_ms();
        let caption =
            capture_annotation_caption(&tab.session.nav().url, tab.session.title(), now_ms);
        let annotated = annotate_callout_capture_image(frame, &caption)?;
        let bytes = encode_color_image_png(&annotated)?;
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("could not create {}: {err}", dir.display()))?;
        let name =
            capture_callout_filename_for(&tab.session.nav().url, tab.session.title(), now_ms);
        let path = dir.join(name);
        std::fs::write(&path, bytes)
            .map_err(|err| format!("could not write {}: {err}", path.display()))?;
        Ok(path)
    }

    fn capture_active_freehand_viewport_to_dir(
        &self,
        dir: impl AsRef<Path>,
    ) -> Result<PathBuf, String> {
        let Some(tab) = self.tabs.get(self.active) else {
            return Err("no active tab".to_owned());
        };
        if tab.session.is_crashed() {
            return Err("active tab is crashed".to_owned());
        }
        let Some(frame) = tab.last_frame.as_ref() else {
            return Err("the active tab has not painted yet".to_owned());
        };
        let now_ms = unix_ms();
        let caption =
            capture_annotation_caption(&tab.session.nav().url, tab.session.title(), now_ms);
        let annotated = annotate_freehand_capture_image(frame, &caption)?;
        let bytes = encode_color_image_png(&annotated)?;
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("could not create {}: {err}", dir.display()))?;
        let name =
            capture_freehand_filename_for(&tab.session.nav().url, tab.session.title(), now_ms);
        let path = dir.join(name);
        std::fs::write(&path, bytes)
            .map_err(|err| format!("could not write {}: {err}", path.display()))?;
        Ok(path)
    }

    fn start_region_capture(&mut self) {
        if !self.active_tab_has_frame() {
            self.capture_notice = Some("Capture failed: no painted page".to_owned());
            return;
        }
        self.capture_region_mode = true;
        self.capture_region_start = None;
        self.capture_region_current = None;
        self.capture_notice = Some("Drag a page region to capture".to_owned());
    }

    fn cancel_region_capture(&mut self) {
        self.capture_region_mode = false;
        self.capture_region_start = None;
        self.capture_region_current = None;
    }

    fn capture_active_region_to_dir(
        &self,
        dir: impl AsRef<Path>,
        region: PixelRegion,
    ) -> Result<PathBuf, String> {
        let Some(tab) = self.tabs.get(self.active) else {
            return Err("no active tab".to_owned());
        };
        if tab.session.is_crashed() {
            return Err("active tab is crashed".to_owned());
        }
        let Some(frame) = tab.last_frame.as_ref() else {
            return Err("the active tab has not painted yet".to_owned());
        };
        let cropped = crop_color_image(frame, region)?;
        let bytes = encode_color_image_png(&cropped)?;
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("could not create {}: {err}", dir.display()))?;
        let name =
            capture_region_filename_for(&tab.session.nav().url, tab.session.title(), unix_ms());
        let path = dir.join(name);
        std::fs::write(&path, bytes)
            .map_err(|err| format!("could not write {}: {err}", path.display()))?;
        Ok(path)
    }

    fn print_active_page(&mut self) {
        match self.queue_active_page_cups_print_to_dir(browser_print_spool_dir()) {
            Ok(path) => {
                self.capture_notice = Some(format!("CUPS print queued {}", path.display()));
            }
            Err(err) => {
                self.capture_notice = Some(format!("Print failed: {err}"));
            }
        }
    }

    fn toggle_print_settings(&mut self) {
        self.print_settings_open = !self.print_settings_open;
        if self.print_settings_open {
            self.refresh_cups_printers();
        }
    }

    fn refresh_cups_printers(&mut self) {
        match discover_cups_printers() {
            Ok(printers) => {
                if self.cups_settings.destination.is_none() {
                    self.cups_settings.destination = printers
                        .iter()
                        .find(|printer| printer.is_default)
                        .or_else(|| printers.first())
                        .map(|printer| printer.name.clone());
                }
                self.cups_printers = printers;
                self.cups_notice = None;
            }
            Err(err) => {
                self.cups_printers.clear();
                self.cups_notice = Some(err);
            }
        }
    }

    fn queue_active_page_cups_print_to_dir(
        &mut self,
        dir: impl AsRef<Path>,
    ) -> Result<PathBuf, String> {
        if !self.can_drive_page_tools() {
            return Err("no live page".to_owned());
        }
        let (url, title) = {
            let Some(tab) = self.tabs.get(self.active) else {
                return Err("no active tab".to_owned());
            };
            (
                tab.session.nav().url.clone(),
                tab.session.title().to_owned(),
            )
        };
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("could not create {}: {err}", dir.display()))?;
        let now_ms = unix_ms();
        let name = print_pdf_filename_for(&url, &title, now_ms);
        let path = dir.join(name);
        let key = path.to_string_lossy().into_owned();
        let request = CupsPrintRequest {
            path: key.clone(),
            title: cups_job_title(&url, &title, now_ms),
            settings: self.cups_settings.clone(),
        };
        self.pending_cups_prints.insert(key.clone(), request);
        if let Some(tab) = self.active_tab() {
            tab.session.save_pdf(key);
        }
        Ok(path)
    }

    fn handle_pdf_event(&mut self, path: String, ok: bool) -> String {
        if let Some(request) = self.pending_cups_prints.remove(&path) {
            if !ok {
                return format!("CUPS print failed: PDF write failed {}", request.path);
            }
            return match submit_pdf_to_cups(
                Path::new(&request.path),
                &request.title,
                &request.settings,
            ) {
                Ok(job) => format!("CUPS print submitted {job}"),
                Err(err) => format!("CUPS print failed: {err}"),
            };
        }
        if ok {
            self.last_saved_pdf = Some(PathBuf::from(&path));
            format!("PDF saved {path}")
        } else {
            format!("PDF failed {path}")
        }
    }

    fn open_last_saved_pdf(&mut self) {
        match self.last_saved_pdf_viewer_url() {
            Ok(url) => {
                self.capture_notice = Some("Opening PDF in CEF viewer".to_owned());
                self.request_new_tab_with_url(BrowserEngine::Cef, url);
            }
            Err(err) => {
                self.capture_notice = Some(format!("PDF viewer failed: {err}"));
            }
        }
    }

    fn last_saved_pdf_viewer_url(&self) -> Result<String, String> {
        let Some(path) = &self.last_saved_pdf else {
            return Err("no saved PDF".to_owned());
        };
        if !pdf_file_looks_readable(path) {
            return Err(format!("{} is not a readable PDF", path.display()));
        }
        file_url_for_path(path)
    }

    fn save_active_page_pdf(&mut self) {
        match self.save_active_page_pdf_to_dir(browser_pdf_dir()) {
            Ok(path) => {
                self.capture_notice = Some(format!("PDF requested {}", path.display()));
            }
            Err(err) => {
                self.capture_notice = Some(format!("PDF failed: {err}"));
            }
        }
    }

    fn save_active_page_pdf_to_dir(&mut self, dir: impl AsRef<Path>) -> Result<PathBuf, String> {
        if !self.can_drive_page_tools() {
            return Err("no live page".to_owned());
        }
        let (url, title) = {
            let Some(tab) = self.tabs.get(self.active) else {
                return Err("no active tab".to_owned());
            };
            (
                tab.session.nav().url.clone(),
                tab.session.title().to_owned(),
            )
        };
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("could not create {}: {err}", dir.display()))?;
        let name = pdf_filename_for(&url, &title, unix_ms());
        let path = dir.join(name);
        if let Some(tab) = self.active_tab() {
            tab.session.save_pdf(path.to_string_lossy().into_owned());
        }
        Ok(path)
    }

    fn can_drive_page_tools(&self) -> bool {
        self.tabs
            .get(self.active)
            .is_some_and(|tab| !tab.session.is_crashed())
    }

    fn set_page_zoom(&mut self, percent: u16) {
        if !self.can_drive_page_tools() {
            return;
        }
        let percent = percent.clamp(PAGE_ZOOM_MIN, PAGE_ZOOM_MAX);
        self.page_zoom_percent = percent;
        if let Some(tab) = self.active_tab() {
            tab.session.set_zoom(percent);
        }
        self.publish_session_snapshot();
    }

    fn zoom_in(&mut self) {
        let next = self
            .page_zoom_percent
            .saturating_add(PAGE_ZOOM_STEP)
            .min(PAGE_ZOOM_MAX);
        self.set_page_zoom(next);
    }

    fn zoom_out(&mut self) {
        let next = self
            .page_zoom_percent
            .saturating_sub(PAGE_ZOOM_STEP)
            .max(PAGE_ZOOM_MIN);
        self.set_page_zoom(next);
    }

    fn reset_zoom(&mut self) {
        self.set_page_zoom(100);
    }

    fn set_active_tab_muted(&mut self, muted: bool) {
        if !self.can_drive_page_tools() {
            return;
        }
        if let Some(tab) = self.active_tab() {
            tab.muted = muted;
            tab.session.set_audio_muted(muted);
        }
        self.publish_session_snapshot();
    }

    fn toggle_active_tab_mute(&mut self) {
        let muted = self.tabs.get(self.active).is_some_and(|tab| tab.muted);
        self.set_active_tab_muted(!muted);
    }

    fn set_active_tab_force_dark(&mut self, enabled: bool) {
        if !self.can_drive_page_tools() {
            return;
        }
        if let Some(tab) = self.active_tab() {
            tab.force_dark = enabled;
            tab.session.set_force_dark(enabled);
        }
        self.publish_session_snapshot();
    }

    fn toggle_active_tab_force_dark(&mut self) {
        let enabled = self.tabs.get(self.active).is_some_and(|tab| tab.force_dark);
        self.set_active_tab_force_dark(!enabled);
    }

    fn set_active_tab_reader_mode(&mut self, enabled: bool) {
        if !self.can_drive_page_tools() {
            return;
        }
        if let Some(tab) = self.active_tab() {
            tab.reader_mode = enabled;
            tab.session.set_reader_mode(enabled);
        }
        self.publish_session_snapshot();
    }

    fn toggle_active_tab_reader_mode(&mut self) {
        let enabled = self
            .tabs
            .get(self.active)
            .is_some_and(|tab| tab.reader_mode);
        self.set_active_tab_reader_mode(!enabled);
    }

    fn set_active_tab_container(&mut self, container: ContainerProfile) {
        if let Some(tab) = self.active_tab() {
            tab.container = container;
        }
        self.publish_session_snapshot();
    }

    fn cycle_active_tab_container(&mut self) {
        let next = self
            .tabs
            .get(self.active)
            .map_or(ContainerProfile::None, |tab| tab.container.next());
        self.set_active_tab_container(next);
    }

    fn set_active_tab_display_target(&mut self, display_target: DisplayTarget) {
        let active = self.active;
        let body = if let Some(tab) = self.tabs.get_mut(active) {
            tab.display_target = display_target;
            Some(browser_display_target_body(active, tab, display_target))
        } else {
            None
        };
        if let Some(body) = body {
            publish_to_bus(
                self.bus_root.as_deref(),
                ACTION_BROWSER_DISPLAY_TARGET,
                &body,
            );
            self.publish_session_snapshot();
        }
    }

    fn cycle_active_tab_display_target(&mut self) {
        let next = self
            .tabs
            .get(self.active)
            .map_or(DisplayTarget::Current, |tab| tab.display_target.next());
        self.set_active_tab_display_target(next);
    }

    fn open_find_bar(&mut self) {
        if !self.can_drive_page_tools() {
            return;
        }
        self.find_open = true;
        self.publish_session_snapshot();
    }

    fn close_find_bar(&mut self) {
        self.find_open = false;
        if let Some(tab) = self.active_tab() {
            tab.session.clear_find();
        }
        self.publish_session_snapshot();
    }

    fn submit_find(&mut self, backwards: bool) {
        let query = self.find_query.trim().to_owned();
        if query.is_empty() {
            if let Some(tab) = self.active_tab() {
                tab.session.clear_find();
            }
            return;
        }
        if let Some(tab) = self.active_tab() {
            tab.session.find_in_page(query, backwards);
        }
    }

    fn sync_address_from_active(&mut self) {
        if let Some(tab) = self.tabs.get(self.active) {
            let url = tab.session.nav().url.trim();
            if !url.is_empty() {
                self.address = url.to_owned();
            }
        }
    }

    fn poll_suggestions(&mut self) {
        self.suggestions.poll();
    }

    fn update_suggestions_for_address(&mut self) {
        self.suggestions.update_for_draft(&self.address);
    }

    fn accept_suggestion(&mut self, suggestion: String) {
        self.address = suggestion;
        self.submit_address();
    }

    /// Whether a crashed tab's Reload asked for a respawn — drained by the shell
    /// each frame (and by the tests). The live build swaps in a fresh session via
    /// [`Self::respawn_active_with`]; the gated build acknowledges it honestly.
    pub(crate) fn take_respawn_request(&mut self) -> bool {
        std::mem::take(&mut self.respawn_requested)
    }

    /// Replace the active tab's crashed session with a fresh one (respawn-on-reload),
    /// discarding its stale texture so the new page uploads cleanly.
    #[cfg(test)]
    pub(crate) fn respawn_active_with(&mut self, session: WebSession) {
        let engine = self
            .tabs
            .get(self.active)
            .map_or(self.engine, |tab| tab.engine);
        self.respawn_active_with_engine(session, engine);
    }

    #[cfg_attr(
        not(any(test, feature = "live-helper")),
        allow(dead_code, reason = "used by live-helper respawn and Browser tests")
    )]
    fn respawn_active_with_engine(&mut self, session: WebSession, engine: BrowserEngine) {
        let mut session = session;
        let url = session.nav().url.clone();
        session.set_filter(self.compiled_request_filter_for_url(&url));
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.session = session;
            tab.engine = engine;
            tab.texture = None;
            tab.last_frame = None;
            tab.last_activity = Instant::now();
            tab.idle_suspended = false;
        }
        self.publish_session_snapshot();
    }

    fn compiled_request_filter(&self) -> RequestFilter {
        RequestFilter::from_store(&self.adfilter_store)
            .with_safe_browsing(SafeBrowsingBlocklist::from_hosts(&self.safe_browsing_hosts))
    }

    fn compiled_request_filter_for_url(&self, url: &str) -> RequestFilter {
        let mut filter = self.compiled_request_filter();
        filter.set_page(url);
        filter
    }

    fn apply_adfilter_to_open_tabs(&mut self) {
        let store = self.adfilter_store.clone();
        let safe_browsing = SafeBrowsingBlocklist::from_hosts(&self.safe_browsing_hosts);
        for tab in &mut self.tabs {
            let mut filter =
                RequestFilter::from_store(&store).with_safe_browsing(safe_browsing.clone());
            filter.set_page(&tab.session.nav().url);
            tab.session.set_filter(filter);
        }
    }

    fn active_first_party(&self) -> Option<String> {
        let url = self.tabs.get(self.active)?.session.nav().url.trim();
        host_of(url)
    }

    fn active_site_blocking_enabled(&self) -> bool {
        self.active_first_party()
            .is_some_and(|host| !self.adfilter_store.allowlist().is_allowed(&host))
    }

    fn safe_browsing_summary(&self) -> String {
        if self.safe_browsing_hosts.is_empty() {
            "Safe browsing: no mesh-hosted unsafe hosts loaded".to_owned()
        } else {
            format!(
                "Safe browsing: {} mesh-hosted unsafe host{} loaded",
                self.safe_browsing_hosts.len(),
                if self.safe_browsing_hosts.len() == 1 {
                    ""
                } else {
                    "s"
                }
            )
        }
    }

    fn site_data_summary(&self) -> String {
        self.site_data.summary(self.active_first_party().as_deref())
    }

    fn update_site_data_from_tabs(&mut self) {
        let hosts = self
            .tabs
            .iter()
            .filter_map(|tab| host_of(tab.session.nav().url.trim()))
            .collect::<Vec<_>>();
        self.site_data
            .observe_open_tabs(hosts.iter().map(String::as_str), unix_ms());
    }

    fn active_site_permission_summary(&self) -> Option<String> {
        let host = self.active_first_party()?;
        let suffix = if self
            .forgotten_permission_sites
            .iter()
            .any(|site| site == &host)
        {
            "forgotten; default deny remains active"
        } else {
            "all sensitive prompts denied by default"
        };
        Some(format!("{host}: {suffix}"))
    }

    fn forget_active_site_permissions(&mut self) {
        let Some(host) = self.active_first_party() else {
            return;
        };
        self.forgotten_permission_sites.retain(|site| site != &host);
        self.forgotten_permission_sites.push(host);
    }

    fn set_active_site_blocking(&mut self, enabled: bool) {
        let Some(host) = self.active_first_party() else {
            return;
        };
        let now = unix_ms();
        if enabled {
            self.adfilter_store
                .block_site(&host, &local_hostname(), now);
            publish(ACTION_ADFILTER_BLOCK, &adfilter_domain_body(&host));
        } else {
            self.adfilter_store
                .allow_site(&host, &local_hostname(), now);
            publish(ACTION_ADFILTER_ALLOW, &adfilter_domain_body(&host));
        }
        self.apply_adfilter_to_open_tabs();
    }

    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "wired to synced browser policy follow-up")
    )]
    fn add_custom_filter_rules(&mut self, name: &str, raw: &str) {
        let name = name.trim();
        let raw = raw.trim();
        if name.is_empty() || raw.is_empty() {
            return;
        }
        self.adfilter_store
            .add_source(FilterListSource::custom(name, None, raw, unix_ms()));
        self.apply_adfilter_to_open_tabs();
    }

    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "wired to synced safe-browsing policy follow-up")
    )]
    fn set_safe_browsing_hosts(&mut self, hosts: impl IntoIterator<Item = impl AsRef<str>>) {
        self.safe_browsing_hosts = hosts
            .into_iter()
            .filter_map(|host| {
                let host = host.as_ref().trim().to_ascii_lowercase();
                (!host.is_empty()).then_some(host)
            })
            .collect();
        self.apply_adfilter_to_open_tabs();
    }
}

/// The `live-helper` spawn glue: creating live [`WebSession`]s by launching the
/// sandboxed `mde-web-preview` helper (the client crate's `live-helper` API) and
/// attaching them as tabs, behind the honest deployment gates (§7). All of this is
/// compiled out of the default portable build.
#[cfg(feature = "live-helper")]
impl WebState {
    /// Ensure a live browser tab exists — spawn the sandboxed helper on first open.
    /// The shell's Browser arm calls this each frame with the live seat verdict. A
    /// no-op once a tab is open, and the real `Command::spawn` is attempted at most
    /// once (a failure surfaces an honest notice, never a per-frame spawn-storm).
    pub(crate) fn ensure_live_tab(&mut self, seat_present: bool) {
        if !self.tabs.is_empty() || self.spawn_attempted {
            return;
        }
        self.restore_startup_session_once();
        self.drain_incoming_send_tabs();
        if !self.open_requested.is_empty() {
            self.spawn_attempted = true;
            self.drain_live_tab_requests(seat_present);
            return;
        }
        self.spawn_attempted = true;
        self.open_with(
            seat_present,
            self.engine,
            START_URL.to_owned(),
            helper_bin_path(self.engine),
            WebSession::spawn,
        );
    }

    /// Drain the visible tab strip's new-tab request into a real helper spawn.
    pub(crate) fn drain_live_tab_requests(&mut self, seat_present: bool) {
        while let Some(intent) = self.take_open_request() {
            match intent {
                TabOpenIntent::NewForeground(engine) => {
                    self.open_with(
                        seat_present,
                        engine,
                        START_URL.to_owned(),
                        helper_bin_path(engine),
                        WebSession::spawn,
                    );
                }
                TabOpenIntent::NewForegroundUrl { engine, url } => {
                    self.open_with(
                        seat_present,
                        engine,
                        url,
                        helper_bin_path(engine),
                        WebSession::spawn,
                    );
                }
            }
        }
    }

    /// Respawn the active crashed tab with a fresh live session (respawn-on-reload),
    /// drained by the Browser arm when [`Self::take_respawn_request`] fires. Driven
    /// by an explicit user Reload, so it is not rate-limited by the one-shot latch.
    pub(crate) fn respawn_live(&mut self) {
        // A tab was already open, so the seat gate is already proven live.
        let engine = self
            .tabs
            .get(self.active)
            .map_or(self.engine, |tab| tab.engine);
        if let Some(session) = self.make_session(
            true,
            engine,
            START_URL.to_owned(),
            helper_bin_path(engine),
            WebSession::spawn,
        ) {
            self.respawn_active_with_engine(session, engine);
        }
    }

    /// Testable core of [`Self::ensure_live_tab`]: attach a session from `spawn` as
    /// the first tab, applying the honest gates. Production passes
    /// [`WebSession::spawn`]; the tests inject a `testkit` factory so no real process
    /// is spawned (hermetic CI).
    fn open_with(
        &mut self,
        seat_present: bool,
        engine: BrowserEngine,
        url: String,
        helper_bin: std::path::PathBuf,
        spawn: impl FnOnce(&SpawnSpec) -> std::io::Result<WebSession>,
    ) {
        if let Some(session) = self.make_session(seat_present, engine, url, helper_bin, spawn) {
            self.push_session_with_engine(session, engine);
        }
    }

    /// Build one live session behind the honest gates (a usable seat · the helper
    /// binary installed · the spawn succeeding), or record a NAMED notice and return
    /// `None`. Never fakes a page, never panics, never hangs — a spawn failure
    /// surfaces its reason (§7).
    fn make_session(
        &mut self,
        seat_present: bool,
        engine: BrowserEngine,
        url: String,
        helper_bin: std::path::PathBuf,
        spawn: impl FnOnce(&SpawnSpec) -> std::io::Result<WebSession>,
    ) -> Option<WebSession> {
        if !seat_present {
            self.gate_notice =
                Some("The sandboxed browser needs a GPU seat — none is available here.".to_owned());
            return None;
        }
        if !helper_bin.exists() {
            self.gate_notice = Some(format!(
                "The sandboxed browser helper is not installed ({}).",
                helper_bin.display()
            ));
            return None;
        }
        if engine == BrowserEngine::Cef {
            if let Some(missing) = cef_runtime_missing_path() {
                self.gate_notice = Some(format!(
                    "The Chromium/CEF runtime is not installed (missing {}).",
                    missing.display()
                ));
                return None;
            }
        }
        let spec = SpawnSpec {
            helper_bin,
            url,
            width: INIT_W,
            height: INIT_H,
        };
        match spawn(&spec) {
            Ok(session) => {
                self.gate_notice = None;
                Some(session)
            }
            Err(e) => {
                self.gate_notice =
                    Some(format!("The sandboxed browser helper failed to start: {e}"));
                None
            }
        }
    }
}

/// Render the Browser surface into `ui`: poll every tab, upload any fresh frame on
/// the active tab, draw the navigation chrome, and paint the body (or the honest
/// crashed / loading / gated states).
pub(crate) fn web_panel(ui: &mut egui::Ui, state: &mut WebState) {
    state.poll_suggestions();
    state.poll_downloads();
    state.poll_incoming_send_tabs();
    state.suspend_idle_tabs(Instant::now());

    // 1. Poll every tab so background tabs keep receiving — and so ONE tab's crash
    //    is observed here without disturbing the others (per-session isolation).
    let mut pdf_events = Vec::new();
    for (idx, tab) in state.tabs.iter_mut().enumerate() {
        if tab.idle_suspended && idx != state.active {
            continue;
        }
        tab.session.poll();
        for event in tab.session.drain_pdf_events() {
            pdf_events.push((event.path, event.ok));
        }
    }
    let mut pdf_notice = None;
    for (path, ok) in pdf_events {
        pdf_notice = Some(state.handle_pdf_event(path, ok));
    }
    if let Some(notice) = pdf_notice {
        state.capture_notice = Some(notice);
    }
    state.update_site_data_from_tabs();
    state.publish_session_snapshot();

    // 2. Upload the active tab's pending frame — ONLY when one is present, so an
    //    idle page never triggers a re-upload.
    if let Some(tab) = state.active_tab() {
        if let Some(img) = tab.session.take_frame() {
            tab.last_frame = Some(img.clone());
            match tab.texture.as_mut() {
                Some(handle) => handle.set(img, BROWSER_TEX),
                None => tab.texture = Some(ui.ctx().load_texture("web-preview", img, BROWSER_TEX)),
            }
        }
    }

    // 3. The shared MENUBAR-ALL top bar — the UPPERCASE BROWSER title, the real
    //    WebSession menus (Edit / View / History / Bookmarks), and the live status
    //    cluster. It COMPLEMENTS the navigation chrome below (never replaces it —
    //    the address bar + back/forward/reload buttons stay), the same model→seam
    //    pattern every other surface uses (design: `docs/design/menubar-all.md`).
    if let Some(action) = menubar::show(state, ui) {
        menubar::apply(ui.ctx(), state, action);
    }
    ui.add_space(CHROME_GAP);

    if state.vertical_tabs {
        ui.horizontal(|ui| {
            tab_strip(ui, state);
            ui.add_space(CHROME_GAP);
            ui.vertical(|ui| {
                // The navigation chrome (back / forward / reload / address bar),
                // wired to the active session's control socket.
                nav_chrome(ui, state);
                find_chrome(ui, state);
                insecure_prompt(ui, state);
                capture_notice(ui, state);
                print_settings_drawer(ui, state);
                downloads_drawer(ui, state);
                ui.add_space(CHROME_GAP);
                active_body(ui, state);
            });
        });
    } else {
        // First-class tab strip (BROWSER-DD-2): switch/close existing isolated
        // sessions and expose a real new-tab intent for the live-helper path.
        tab_strip(ui, state);
        ui.add_space(CHROME_GAP);

        // The navigation chrome (back / forward / reload / address bar), wired to
        // the active session's control socket.
        nav_chrome(ui, state);
        find_chrome(ui, state);
        insecure_prompt(ui, state);
        capture_notice(ui, state);
        print_settings_drawer(ui, state);
        downloads_drawer(ui, state);
        ui.add_space(CHROME_GAP);
        active_body(ui, state);
    }
}

fn active_body(ui: &mut egui::Ui, state: &mut WebState) {
    // Read the active tab's status first so the crashed arm can set the respawn
    // flag without holding a `&mut Tab` borrow of `state`.
    let active = state.active;
    let status = state.tabs.get(active).map(|t| {
        (
            t.session.is_crashed(),
            t.texture.is_some(),
            is_new_tab_url(t.session.nav().url.trim()),
            crash_reason(&t.session),
        )
    });
    match status {
        Some((true, _, _, reason)) => crashed_body(ui, reason, &mut state.respawn_requested),
        Some((false, _, true, _)) => new_tab_dashboard(ui, state),
        Some((false, true, false, _)) => paint_body(ui, state, active),
        Some((false, false, false, _)) => {
            // Connected, no first frame yet — an honest loading note, never a blank.
            centered(ui, |ui| {
                muted_note(ui, "Loading the page\u{2026}");
            });
        }
        None => {
            // The honest gated body — a `live-helper` build shows the NAMED gate
            // notice (no seat · helper absent · spawn failed) when one is set; the
            // default build always shows the standard gated caption (§7).
            #[cfg(feature = "live-helper")]
            let notice = state.gate_notice.as_deref();
            #[cfg(not(feature = "live-helper"))]
            let notice: Option<&str> = None;
            empty_body(ui, notice);
        }
    }
}

fn tab_strip(ui: &mut egui::Ui, state: &mut WebState) {
    if state.vertical_tabs {
        vertical_tab_strip(ui, state);
    } else {
        horizontal_tab_strip(ui, state);
    }
}

fn horizontal_tab_strip(ui: &mut egui::Ui, state: &mut WebState) {
    let mut select: Option<usize> = None;
    let mut close: Option<usize> = None;
    let mut move_tab: Option<(usize, usize)> = None;
    let mut mute_tab: Option<(usize, bool)> = None;
    let mut force_dark_tab: Option<(usize, bool)> = None;
    let mut reader_tab: Option<(usize, bool)> = None;
    let mut container_tab: Option<(usize, ContainerProfile)> = None;
    let mut display_tab: Option<(usize, DisplayTarget)> = None;
    ui.horizontal_wrapped(|ui| {
        for (idx, tab) in state.tabs.iter().enumerate() {
            let active = idx == state.active;
            let label = tab_label(tab);
            let tab_response = tab_pill(ui, &label, active);
            if tab_response.clicked() {
                select = Some(idx);
            }
            tab_response
                .on_hover_text(tab_hover(tab))
                .context_menu(|ui| {
                    if ui
                        .add_enabled(idx > 0, compact_menu_item("Move tab left"))
                        .clicked()
                    {
                        move_tab = Some((idx, idx - 1));
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            idx + 1 < state.tabs.len(),
                            compact_menu_item("Move tab right"),
                        )
                        .clicked()
                    {
                        move_tab = Some((idx, idx + 1));
                        ui.close_menu();
                    }
                    let mute_label = if tab.muted { "Unmute tab" } else { "Mute tab" };
                    if ui.add(compact_menu_item(mute_label)).clicked() {
                        mute_tab = Some((idx, !tab.muted));
                        ui.close_menu();
                    }
                    let dark_label = if tab.force_dark {
                        "Disable force dark"
                    } else {
                        "Enable force dark"
                    };
                    if ui.add(compact_menu_item(dark_label)).clicked() {
                        force_dark_tab = Some((idx, !tab.force_dark));
                        ui.close_menu();
                    }
                    let reader_label = if tab.reader_mode {
                        "Disable reader mode"
                    } else {
                        "Enable reader mode"
                    };
                    if ui.add(compact_menu_item(reader_label)).clicked() {
                        reader_tab = Some((idx, !tab.reader_mode));
                        ui.close_menu();
                    }
                    ui.separator();
                    for container in ContainerProfile::ALL {
                        if ui
                            .add_enabled(
                                tab.container != container,
                                compact_menu_item(container.label()),
                            )
                            .clicked()
                        {
                            container_tab = Some((idx, container));
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    for display_target in DisplayTarget::ALL {
                        if ui
                            .add_enabled(
                                tab.display_target != display_target,
                                compact_menu_item(display_target.label()),
                            )
                            .clicked()
                        {
                            display_tab = Some((idx, display_target));
                            ui.close_menu();
                        }
                    }
                    if ui.add(compact_menu_item("Close tab")).clicked() {
                        close = Some(idx);
                        ui.close_menu();
                    }
                });
            if inline_close_button(ui).clicked() {
                close = Some(idx);
            }
        }
        engine_new_tab_buttons(ui, state, false);
    });
    if let Some((idx, muted)) = mute_tab {
        state.select_tab(idx);
        state.set_active_tab_muted(muted);
    } else if let Some((idx, enabled)) = force_dark_tab {
        state.select_tab(idx);
        state.set_active_tab_force_dark(enabled);
    } else if let Some((idx, enabled)) = reader_tab {
        state.select_tab(idx);
        state.set_active_tab_reader_mode(enabled);
    } else if let Some((idx, container)) = container_tab {
        state.select_tab(idx);
        state.set_active_tab_container(container);
    } else if let Some((idx, display_target)) = display_tab {
        state.select_tab(idx);
        state.set_active_tab_display_target(display_target);
    } else if let Some((from, to)) = move_tab {
        state.move_tab(from, to);
    } else if let Some(idx) = close {
        state.close_tab(idx);
    } else if let Some(idx) = select {
        state.select_tab(idx);
    }
}

fn vertical_tab_strip(ui: &mut egui::Ui, state: &mut WebState) {
    let mut select: Option<usize> = None;
    let mut close: Option<usize> = None;
    let mut move_tab: Option<(usize, usize)> = None;
    let mut mute_tab: Option<(usize, bool)> = None;
    let mut force_dark_tab: Option<(usize, bool)> = None;
    let mut reader_tab: Option<(usize, bool)> = None;
    let mut container_tab: Option<(usize, ContainerProfile)> = None;
    let mut display_tab: Option<(usize, DisplayTarget)> = None;
    egui::Frame::NONE
        .fill(Style::SURFACE)
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            ui.set_width(184.0);
            egui::ScrollArea::vertical()
                .id_salt("browser-vertical-tabs")
                .max_height(ui.available_height())
                .show(ui, |ui| {
                    for (idx, tab) in state.tabs.iter().enumerate() {
                        let active = idx == state.active;
                        let label = tab_label(tab);
                        ui.horizontal(|ui| {
                            let width = (ui.available_width() - CHROME_TAB_CLOSE - CHROME_GAP)
                                .max(CHROME_NEW_TAB_W);
                            let resp = tab_pill_sized(ui, &label, active, width);
                            if resp.clicked() {
                                select = Some(idx);
                            }
                            resp.on_hover_text(tab_hover(tab)).context_menu(|ui| {
                                if ui
                                    .add_enabled(idx > 0, compact_menu_item("Move tab up"))
                                    .clicked()
                                {
                                    move_tab = Some((idx, idx - 1));
                                    ui.close_menu();
                                }
                                if ui
                                    .add_enabled(
                                        idx + 1 < state.tabs.len(),
                                        compact_menu_item("Move tab down"),
                                    )
                                    .clicked()
                                {
                                    move_tab = Some((idx, idx + 1));
                                    ui.close_menu();
                                }
                                let mute_label = if tab.muted { "Unmute tab" } else { "Mute tab" };
                                if ui.add(compact_menu_item(mute_label)).clicked() {
                                    mute_tab = Some((idx, !tab.muted));
                                    ui.close_menu();
                                }
                                let dark_label = if tab.force_dark {
                                    "Disable force dark"
                                } else {
                                    "Enable force dark"
                                };
                                if ui.add(compact_menu_item(dark_label)).clicked() {
                                    force_dark_tab = Some((idx, !tab.force_dark));
                                    ui.close_menu();
                                }
                                let reader_label = if tab.reader_mode {
                                    "Disable reader mode"
                                } else {
                                    "Enable reader mode"
                                };
                                if ui.add(compact_menu_item(reader_label)).clicked() {
                                    reader_tab = Some((idx, !tab.reader_mode));
                                    ui.close_menu();
                                }
                                ui.separator();
                                for container in ContainerProfile::ALL {
                                    if ui
                                        .add_enabled(
                                            tab.container != container,
                                            compact_menu_item(container.label()),
                                        )
                                        .clicked()
                                    {
                                        container_tab = Some((idx, container));
                                        ui.close_menu();
                                    }
                                }
                                ui.separator();
                                for display_target in DisplayTarget::ALL {
                                    if ui
                                        .add_enabled(
                                            tab.display_target != display_target,
                                            compact_menu_item(display_target.label()),
                                        )
                                        .clicked()
                                    {
                                        display_tab = Some((idx, display_target));
                                        ui.close_menu();
                                    }
                                }
                                if ui.add(compact_menu_item("Close tab")).clicked() {
                                    close = Some(idx);
                                    ui.close_menu();
                                }
                            });
                            if inline_close_button(ui).clicked() {
                                close = Some(idx);
                            }
                        });
                    }
                    engine_new_tab_buttons(ui, state, true);
                });
        });
    if let Some((idx, muted)) = mute_tab {
        state.select_tab(idx);
        state.set_active_tab_muted(muted);
    } else if let Some((idx, enabled)) = force_dark_tab {
        state.select_tab(idx);
        state.set_active_tab_force_dark(enabled);
    } else if let Some((idx, enabled)) = reader_tab {
        state.select_tab(idx);
        state.set_active_tab_reader_mode(enabled);
    } else if let Some((idx, container)) = container_tab {
        state.select_tab(idx);
        state.set_active_tab_container(container);
    } else if let Some((idx, display_target)) = display_tab {
        state.select_tab(idx);
        state.set_active_tab_display_target(display_target);
    } else if let Some((from, to)) = move_tab {
        state.move_tab(from, to);
    } else if let Some(idx) = close {
        state.close_tab(idx);
    } else if let Some(idx) = select {
        state.select_tab(idx);
    }
}

fn engine_new_tab_buttons(ui: &mut egui::Ui, state: &mut WebState, vertical: bool) {
    let mut button = |ui: &mut egui::Ui, engine: BrowserEngine| {
        let label = format!("+{}", engine.label());
        let mut widget =
            egui::Button::new(RichText::new(label).size(CHROME_FONT).color(Style::TEXT))
                .fill(Style::SURFACE)
                .min_size(egui::vec2(CHROME_NEW_TAB_W, CHROME_TAB_H));
        if vertical {
            widget = widget.min_size(egui::vec2(ui.available_width(), CHROME_TAB_H));
        }
        if ui
            .add(widget)
            .on_hover_text(format!("New {} tab", engine.label()))
            .clicked()
        {
            state.request_new_tab(engine);
        }
    };
    if vertical {
        button(ui, BrowserEngine::Servo);
        button(ui, BrowserEngine::Cef);
    } else {
        button(ui, BrowserEngine::Servo);
        button(ui, BrowserEngine::Cef);
    }
}

fn tab_pill(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    tab_pill_sized(ui, label, active, CHROME_TAB_W)
}

fn tab_pill_sized(ui: &mut egui::Ui, label: &str, active: bool, width: f32) -> egui::Response {
    let color = if active { Style::TEXT } else { Style::TEXT_DIM };
    let fill = if active {
        Style::SURFACE_HI
    } else {
        Style::SURFACE
    };
    ui.add(
        egui::Button::new(RichText::new(label).size(CHROME_FONT).color(color))
            .fill(fill)
            .min_size(egui::vec2(width, CHROME_TAB_H)),
    )
}

fn inline_close_button(ui: &mut egui::Ui) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new("\u{00D7}")
                .size(CHROME_FONT)
                .color(Style::TEXT_DIM),
        )
        .fill(Style::SURFACE)
        .min_size(egui::vec2(CHROME_TAB_CLOSE, CHROME_TAB_H)),
    )
    .on_hover_text("Close tab")
}

fn compact_menu_item(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).size(CHROME_FONT).color(Style::TEXT))
        .min_size(egui::vec2(124.0, CHROME_TAB_H))
}

fn tab_label(tab: &Tab) -> String {
    let title = tab.session.title().trim();
    let url = tab.session.nav().url.trim();
    let base = if !title.is_empty() {
        title
    } else if !url.is_empty() {
        url
    } else {
        "New tab"
    };
    let state = if tab.idle_suspended {
        "\u{25D2}"
    } else {
        match tab.session.state() {
            SessionState::Loading => "\u{25CC}",
            SessionState::Live => "\u{25CF}",
            SessionState::Crashed { .. } => "!",
        }
    };
    let container = tab.container.marker();
    let display = tab.display_target.marker();
    let muted = if tab.muted { "M " } else { "" };
    let force_dark = if tab.force_dark { "D " } else { "" };
    let reader = if tab.reader_mode { "R " } else { "" };
    format!(
        "{state} {container}{display}{muted}{force_dark}{reader}{}",
        ellipsize(base, 24)
    )
}

fn tab_hover(tab: &Tab) -> String {
    let url = tab.session.nav().url.trim();
    let state = if tab.idle_suspended {
        "Idle suspended"
    } else {
        match tab.session.state() {
            SessionState::Loading => "Loading",
            SessionState::Live => "Live",
            SessionState::Crashed { .. } => "Crashed",
        }
    };
    let container = match tab.container {
        ContainerProfile::None => String::new(),
        profile => format!(" - Container: {}", profile.label()),
    };
    let display = match tab.display_target {
        DisplayTarget::Current => String::new(),
        target => format!(" - Display target: {}", target.label()),
    };
    let audio = if tab.muted { " - Muted" } else { "" };
    let force_dark = if tab.force_dark { " - Force dark" } else { "" };
    let reader = if tab.reader_mode { " - Reader" } else { "" };
    if url.is_empty() {
        format!("{state}{container}{display}{audio}{force_dark}{reader}")
    } else {
        format!("{state} - {url}{container}{display}{audio}{force_dark}{reader}")
    }
}

fn ellipsize(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i + 1 >= max_chars {
            out.push('\u{2026}');
            return out;
        }
        out.push(ch);
    }
    out
}

/// The crash reason string for a session, or empty if it has not crashed.
fn crash_reason(session: &WebSession) -> String {
    match session.state() {
        SessionState::Crashed { reason } => reason.clone(),
        _ => String::new(),
    }
}

// ── BOOKMARKS-10: mesh integration (Send-in-Chat + copy-URL + add-from-page) ──
//
// The Browser surface composes the live page with two mesh services by REUSING
// their existing Bus verbs (§6 JSON boundaries — a local mirror of each topic,
// never a dep on the mackesd worker, never a re-derived store):
//
//   * add-from-page → `action/bookmarks/add` — the BOOKMARKS-2 worker (the
//                      single writer of this node's op-log segment + HLC clock,
//                      the services/desktop tier boundary) drains it, mints the
//                      real `Op::Add`, persists it, and Syncthing-syncs it across
//                      the mesh. `source` is omitted → the honest `Source::Manual`.
//   * Send-in-Chat  → `action/chat/send` — the SAME verb the shell's Chat composer
//                      and Files' `chat_bridge` publish; a link rides the
//                      NOTIFY-CHAT `Clipboard` message-kind (a `kind` wins over
//                      `text` in the worker), addressed to this node's own Chat
//                      contact (the notification hub where its clips land).
//   * copy-URL      → the shell clipboard — egui's output command (the DRM /
//                      windowed backend owns the wire), the same one-click re-copy
//                      the Chat clipboard card uses.

/// The mackesd bookmarks worker's add verb (`action/bookmarks/<verb>`, §9).
const ACTION_BOOKMARKS_ADD: &str = "action/bookmarks/add";

/// The mackesd chat worker's send verb (reused, never re-invented).
const ACTION_CHAT_SEND: &str = "action/chat/send";

/// The mackesd adfilter worker's mesh-wide per-site allowlist verbs.
const ACTION_ADFILTER_ALLOW: &str = "action/adfilter/allow";
const ACTION_ADFILTER_BLOCK: &str = "action/adfilter/block";

/// Browser-to-platform display placement handoff. The shell owns tab intent; the
/// compositor/display owner drains this stream when it can perform the output
/// migration.
const ACTION_BROWSER_DISPLAY_TARGET: &str = "action/browser/display-target";

/// Browser external-protocol handoff for schemes without a dedicated worker yet.
const ACTION_BROWSER_PROTOCOL: &str = "action/browser/protocol";

/// Browser page-share handoff for platform targets whose receivers live outside
/// the Browser surface.
const ACTION_BROWSER_SHARE: &str = "action/browser/share";

/// Browser follow-me/send-tab handoff. The session-sync owner drains this stream
/// and delivers the live tab to a mesh node or a paired phone; Browser only owns
/// the tab metadata and stable user action.
const ACTION_BROWSER_SEND_TAB: &str = "action/browser/send-tab";

/// Browser follow-me session snapshot. The sync owner drains this stream into the
/// Nebula+Syncthing session store and later drives startup restore; Browser only
/// publishes the state it already owns.
const ACTION_BROWSER_SESSION_SYNC: &str = "action/browser/session-sync";

/// Daemon-owned Browser session-sync snapshot subdirectory. Must match
/// `mackesd::workers::browser_session_sync::SESSION_SYNC_SUBDIR` without creating
/// a desktop-shell dependency on the daemon crate.
const SESSION_SYNC_SUBDIR: &str = "browser-session-sync";

/// Daemon-owned latest snapshot filename. The file body is the Browser snapshot
/// JSON itself, so startup restore can feed it straight into the parser.
const SESSION_SYNC_LATEST_FILE: &str = "latest.json";

/// Daemon-owned send-tab outbox subdirectory. Must match
/// `mackesd::workers::browser_session_sync::SEND_TAB_OUTBOX_SUBDIR`.
const SEND_TAB_OUTBOX_SUBDIR: &str = "browser-send-tab";

/// Browser idle-tab suspension handoff for deeper engine/process orchestration.
const ACTION_BROWSER_TAB_SUSPEND: &str = "action/browser/tab-suspend";

/// Existing Voice handoff used by Chat's Call action; Browser `tel:` URLs reuse
/// it instead of creating a second dial verb.
const ACTION_VOICE_DIAL: &str = "action/voice/dial";

/// Browser-originated notifications folded by Chat's alert lane into the unified
/// Notifications feed.
const EVENT_NOTIFY_BROWSER: &str = "event/notify/browser";

/// The mesh-hosted SearXNG endpoint used by the first-cut omnibox when the draft
/// is plain search text rather than a URL. This is intentionally mesh-local, not a
/// public search provider default.
const DEFAULT_SEARCH_URL: &str = "https://search.mesh/search";

/// Mesh-local SearXNG autocomplete endpoint. SearXNG deployments commonly expose
/// autocomplete through this path; the parser below accepts the JSON shapes used
/// by SearXNG/OpenSearch-style providers so the shell does not bake in one brittle
/// response variant.
const DEFAULT_SUGGEST_URL: &str = "https://search.mesh/autocompleter";

/// Keep a live suggestion request tight; this is interactive chrome, not page load.
const SUGGEST_TIMEOUT: Duration = Duration::from_millis(900);

type SuggestionResult = Result<(String, Vec<String>), String>;

#[derive(Default)]
struct SuggestionState {
    draft: String,
    items: Vec<String>,
    notice: Option<String>,
    in_flight: Option<String>,
    rx: Option<mpsc::Receiver<SuggestionResult>>,
}

impl SuggestionState {
    fn clear(&mut self) {
        self.draft.clear();
        self.items.clear();
        self.notice = None;
        self.in_flight = None;
        self.rx = None;
    }

    fn poll(&mut self) {
        let Some(rx) = self.rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok((draft, items))) => {
                if draft == self.draft {
                    self.items = items;
                    self.notice = None;
                }
                self.in_flight = None;
            }
            Ok(Err(err)) => {
                self.items.clear();
                self.notice = Some(err);
                self.in_flight = None;
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.rx = Some(rx);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.items.clear();
                self.notice = Some("Suggestions unavailable".to_owned());
                self.in_flight = None;
            }
        }
    }

    fn update_for_draft(&mut self, draft: &str) {
        self.poll();
        let draft = draft.trim();
        if !should_fetch_suggestions(draft) {
            self.clear();
            return;
        }
        if self.draft != draft {
            self.draft = draft.to_owned();
            self.items.clear();
            self.notice = None;
        }
        if self.in_flight.as_deref() == Some(draft) {
            return;
        }
        self.in_flight = Some(draft.to_owned());
        let request = draft.to_owned();
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let result = fetch_suggestions(&request).map(|items| (request.clone(), items));
            let _ = tx.send(result);
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MeshServiceShortcut {
    label: &'static str,
    url: &'static str,
    hint: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SpeedDialEntry {
    label: String,
    url: String,
    hint: String,
}

impl SpeedDialEntry {
    fn new(label: impl Into<String>, url: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            url: url.into(),
            hint: hint.into(),
        }
    }
}

const NEW_TAB_SERVICES: [MeshServiceShortcut; 4] = [
    MeshServiceShortcut {
        label: "Search",
        url: "https://search.mesh/",
        hint: "Open the mesh SearXNG front page",
    },
    MeshServiceShortcut {
        label: "Music",
        url: "http://music.mesh:4533/",
        hint: "Open the active-active Navidrome mesh service",
    },
    MeshServiceShortcut {
        label: "Horizon",
        url: "https://horizon.mesh/",
        hint: "Open the optional OpenStack dashboard when enabled",
    },
    MeshServiceShortcut {
        label: "Keystone",
        url: "http://keystone.mesh:5000/v3",
        hint: "Open the OpenStack identity API endpoint",
    },
];

fn default_speed_dial() -> Vec<SpeedDialEntry> {
    NEW_TAB_SERVICES
        .iter()
        .map(|service| SpeedDialEntry::new(service.label, service.url, service.hint))
        .collect()
}

fn default_session_restore_roots() -> Vec<PathBuf> {
    vec![local_session_sync_root(), default_workgroup_root()]
}

fn local_session_sync_root() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map_or_else(
        || {
            std::env::var_os("HOME").map_or_else(
                || PathBuf::from("/var/lib/mde/browser-session-sync"),
                |home| {
                    PathBuf::from(home)
                        .join(".local")
                        .join("share")
                        .join("mde")
                        .join("browser-session-sync")
                },
            )
        },
        |data_home| {
            PathBuf::from(data_home)
                .join("mde")
                .join("browser-session-sync")
        },
    )
}

fn session_sync_latest_path(root: &Path, host: &str) -> PathBuf {
    root.join(SESSION_SYNC_SUBDIR)
        .join(sanitize_session_host(host))
        .join(SESSION_SYNC_LATEST_FILE)
}

fn send_tab_inbox_dir(root: &Path, host: &str) -> PathBuf {
    root.join(SEND_TAB_OUTBOX_SUBDIR)
        .join("node")
        .join(sanitize_session_host(host))
}

fn sanitize_session_host(host: &str) -> String {
    host.chars()
        .filter_map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                Some(c)
            } else if c.is_ascii_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

fn browser_send_tab_open_intent(body: &str, host: &str) -> Result<(BrowserEngine, String), String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|err| format!("send-tab JSON: {err}"))?;
    if v.get("op").and_then(serde_json::Value::as_str) != Some("browser_send_tab") {
        return Err("send-tab has the wrong op".to_owned());
    }
    if v.get("target").and_then(serde_json::Value::as_str) != Some("node") {
        return Err("send-tab is not node-addressed".to_owned());
    }
    let target_id = v
        .get("target_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|target_id| !target_id.is_empty())
        .ok_or_else(|| "send-tab is missing target_id".to_owned())?;
    if sanitize_session_host(target_id) != sanitize_session_host(host) {
        return Err("send-tab is for a different node".to_owned());
    }
    let engine = v
        .get("engine")
        .and_then(serde_json::Value::as_str)
        .and_then(BrowserEngine::from_wire)
        .ok_or_else(|| "send-tab has an unsupported engine".to_owned())?;
    let url = v
        .get("url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| "send-tab is missing url".to_owned())?;
    Ok((engine, url.to_owned()))
}

fn incoming_send_tab_files(root: &Path, host: &str) -> Vec<PathBuf> {
    let inbox = send_tab_inbox_dir(root, host);
    let Ok(sources) = std::fs::read_dir(&inbox) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for source in sources.filter_map(Result::ok) {
        let source_path = source.path();
        if !source_path.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&source_path) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn speed_dial_from_settings(settings: &serde_json::Value) -> Option<Vec<SpeedDialEntry>> {
    let entries = settings.get("speed_dial")?.as_array()?;
    let restored = entries
        .iter()
        .filter_map(|entry| {
            let label = entry
                .get("label")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim();
            let url = entry
                .get("url")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim();
            if label.is_empty() || url.is_empty() {
                return None;
            }
            let hint = entry
                .get("hint")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|hint| !hint.is_empty())
                .unwrap_or(url);
            Some(SpeedDialEntry::new(label, url, hint))
        })
        .take(32)
        .collect::<Vec<_>>();
    (!restored.is_empty()).then_some(restored)
}

/// Build the `action/bookmarks/add` body for the live page. Pure — the wire shape
/// is asserted headless. `source` is omitted, so the worker mints the default
/// `Source::Manual` (a page the user bookmarked in-app).
fn bookmark_add_body(url: &str, title: &str) -> String {
    serde_json::json!({ "url": url, "title": title }).to_string()
}

fn adfilter_domain_body(domain: &str) -> String {
    serde_json::json!({ "domain": domain.trim() }).to_string()
}

fn browser_display_target_body(
    tab_index: usize,
    tab: &Tab,
    display_target: DisplayTarget,
) -> String {
    serde_json::json!({
        "op": "browser_display_target",
        "tab_index": tab_index,
        "engine": tab.engine.wire(),
        "target": display_target.wire(),
        "url": tab.session.nav().url.as_str(),
        "title": tab.session.title(),
    })
    .to_string()
}

/// Build the `action/chat/send` body sharing the live page into Chat. A link is
/// carried as the NOTIFY-CHAT [`MessageKind::Clipboard`] kind — its `preview`
/// (the title, falling back to the URL) shows in the timeline and its `full` (the
/// exact URL) is what a one-click re-copy puts back. Pure: the `kind` is a real
/// `mde_chat::MessageKind`, so it round-trips straight into what the worker
/// accepts (the same shape Files' `chat_bridge` writes).
fn chat_share_body(to: &str, url: &str, title: &str) -> String {
    let preview = if title.trim().is_empty() { url } else { title };
    let kind = MessageKind::Clipboard {
        preview: preview.to_string(),
        full: url.to_string(),
    };
    let kind_val = serde_json::to_value(&kind).unwrap_or(serde_json::Value::Null);
    serde_json::json!({ "scope": "peer", "to": to, "kind": kind_val }).to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserShareTarget {
    Peer,
    Email,
    Qr,
}

impl BrowserShareTarget {
    const fn wire(self) -> &'static str {
        match self {
            Self::Peer => "peer",
            Self::Email => "email",
            Self::Qr => "qr",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Peer => "Peer",
            Self::Email => "Email",
            Self::Qr => "QR",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserSendTabTarget {
    Node,
    Phone,
}

impl BrowserSendTabTarget {
    const fn wire(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Phone => "phone",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Node => "Node",
            Self::Phone => "Phone",
        }
    }

    fn destination(self) -> Option<(String, String)> {
        match self {
            Self::Node => {
                let host = local_hostname();
                Some((host.clone(), host))
            }
            Self::Phone => std::env::var("MDE_BROWSER_SEND_PHONE_TARGET")
                .ok()
                .map(|id| id.trim().to_owned())
                .filter(|id| !id.is_empty())
                .map(|id| {
                    let label = std::env::var("MDE_BROWSER_SEND_PHONE_LABEL")
                        .ok()
                        .map(|label| label.trim().to_owned())
                        .filter(|label| !label.is_empty())
                        .unwrap_or_else(|| id.clone());
                    (id, label)
                }),
        }
    }
}

/// Build the browser-owned platform share handoff. The receiving surfaces are
/// intentionally outside Browser ownership, so this publishes a stable typed verb
/// instead of pretending to complete peer/email/QR delivery in-process.
fn browser_share_body(target: BrowserShareTarget, url: &str, title: &str) -> String {
    let title = title.trim();
    let preview = if title.is_empty() { url } else { title };
    serde_json::json!({
        "op": "browser_share",
        "target": target.wire(),
        "url": url,
        "title": title,
        "preview": preview,
        "source": "browser",
        "host": local_hostname(),
    })
    .to_string()
}

fn publish_browser_share(root: Option<&Path>, target: BrowserShareTarget, url: &str, title: &str) {
    let body = browser_share_body(target, url, title);
    if root.is_some() {
        publish_to_bus(root, ACTION_BROWSER_SHARE, &body);
    } else {
        publish(ACTION_BROWSER_SHARE, &body);
    }
}

/// Build the browser-owned send-tab handoff for BROWSER-DD-7. Target selection and
/// delivery live in the session-sync / phone owners, so the Browser publishes the
/// current tab's URL/title/engine metadata and lets those owners route it.
fn browser_send_tab_body(
    target: BrowserSendTabTarget,
    engine: BrowserEngine,
    url: &str,
    title: &str,
) -> String {
    let title = title.trim();
    let preview = if title.is_empty() { url } else { title };
    let mut body = serde_json::json!({
        "op": "browser_send_tab",
        "target": target.wire(),
        "engine": engine.wire(),
        "url": url,
        "title": title,
        "preview": preview,
        "source": "browser",
        "host": local_hostname(),
    });
    if let Some((id, label)) = target.destination() {
        if let Some(obj) = body.as_object_mut() {
            obj.insert("target_id".to_owned(), serde_json::json!(id));
            obj.insert("target_label".to_owned(), serde_json::json!(label));
        }
    }
    body.to_string()
}

fn publish_browser_send_tab(
    root: Option<&Path>,
    target: BrowserSendTabTarget,
    engine: BrowserEngine,
    url: &str,
    title: &str,
) {
    let body = browser_send_tab_body(target, engine, url, title);
    if root.is_some() {
        publish_to_bus(root, ACTION_BROWSER_SEND_TAB, &body);
    } else {
        publish(ACTION_BROWSER_SEND_TAB, &body);
    }
}

fn session_state_wire(state: &SessionState) -> &'static str {
    match state {
        SessionState::Loading => "loading",
        SessionState::Live => "live",
        SessionState::Crashed { .. } => "crashed",
    }
}

fn browser_session_sync_body(state: &WebState) -> String {
    let tabs = state
        .tabs
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            let nav = tab.session.nav();
            serde_json::json!({
                "index": index,
                "engine": tab.engine.wire(),
                "container": tab.container.wire(),
                "display_target": tab.display_target.wire(),
                "url": nav.url.as_str(),
                "title": tab.session.title(),
                "state": session_state_wire(&tab.session.state()),
                "loading": nav.loading,
                "can_back": nav.can_back,
                "can_forward": nav.can_forward,
                "muted": tab.muted,
                "force_dark": tab.force_dark,
                "reader_mode": tab.reader_mode,
                "idle_suspended": tab.idle_suspended,
            })
        })
        .collect::<Vec<_>>();
    let downloads = state
        .download_jobs
        .iter()
        .map(|job| {
            serde_json::json!({
                "id": job.id.as_str(),
                "source": job.source.as_str(),
                "dest": job.dest.as_str(),
                "method": job.method,
                "state": job.state,
                "progress": job.progress,
                "updated_ms": job.updated_ms,
            })
        })
        .collect::<Vec<_>>();
    let speed_dial = state
        .speed_dial
        .iter()
        .map(|entry| {
            serde_json::json!({
                "label": entry.label.as_str(),
                "url": entry.url.as_str(),
                "hint": entry.hint.as_str(),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "op": "browser_session_sync",
        "source": "browser",
        "host": local_hostname(),
        "active_index": if state.tabs.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::json!(state.active.min(state.tabs.len().saturating_sub(1)))
        },
        "settings": {
            "future_engine": state.engine.wire(),
            "vertical_tabs": state.vertical_tabs,
            "page_zoom_percent": state.page_zoom_percent,
            "find_open": state.find_open,
            "downloads_open": state.downloads_open,
            "speed_dial": speed_dial,
        },
        "tabs": tabs,
        "downloads": downloads,
    })
    .to_string()
}

fn browser_tab_suspend_body(
    tab_index: usize,
    engine: BrowserEngine,
    url: &str,
    title: &str,
    idle_after: Duration,
) -> String {
    let idle_after_ms = u64::try_from(idle_after.as_millis()).unwrap_or(u64::MAX);
    serde_json::json!({
        "op": "browser_tab_suspend",
        "tab_index": tab_index,
        "engine": engine.wire(),
        "url": url,
        "title": title,
        "idle_after_ms": idle_after_ms,
        "source": "browser",
        "host": local_hostname(),
    })
    .to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalProtocol {
    Mailto,
    Tel,
    Magnet,
}

impl ExternalProtocol {
    fn from_url(url: &str) -> Option<Self> {
        let (scheme, _) = url.split_once(':')?;
        match scheme.to_ascii_lowercase().as_str() {
            "mailto" => Some(Self::Mailto),
            "tel" => Some(Self::Tel),
            "magnet" => Some(Self::Magnet),
            _ => None,
        }
    }

    const fn scheme(self) -> &'static str {
        match self {
            Self::Mailto => "mailto",
            Self::Tel => "tel",
            Self::Magnet => "magnet",
        }
    }

    const fn target(self) -> &'static str {
        match self {
            Self::Mailto => "email",
            Self::Tel => "voice",
            Self::Magnet => "transfers",
        }
    }

    const fn target_label(self) -> &'static str {
        match self {
            Self::Mailto => "Email",
            Self::Tel => "Voice",
            Self::Magnet => "Transfers",
        }
    }
}

fn browser_protocol_handoff_body(protocol: ExternalProtocol, url: &str) -> String {
    serde_json::json!({
        "op": "browser_protocol_handoff",
        "scheme": protocol.scheme(),
        "target": protocol.target(),
        "url": url,
    })
    .to_string()
}

fn voice_dial_body(url: &str) -> String {
    let number = url.split_once(':').map_or(url, |(_, rest)| rest).trim();
    serde_json::json!({
        "peer": number,
        "source": "browser",
        "url": url,
    })
    .to_string()
}

fn browser_notify_body(severity: Severity, summary: &str, detail: Option<&str>) -> String {
    let mut body = serde_json::json!({
        "severity": severity.tag(),
        "host": local_hostname(),
        "source": "browser",
        "summary": summary,
        "action": "action/shell/goto/browser",
    });
    if let Some(detail) = detail.filter(|s| !s.trim().is_empty()) {
        body["detail"] = serde_json::Value::String(detail.to_owned());
    }
    body.to_string()
}

/// Resolve an address-bar draft into the URL sent to the helper.
///
/// BROWSER-DD-2 asks for a real omnibox, not a strict URL field: explicit schemes
/// pass through, likely hostnames become HTTPS URLs, and free text searches the
/// mesh-hosted SearXNG instance. Empty/whitespace drafts stay inert.
fn omnibox_target(draft: &str) -> Option<String> {
    let draft = draft.trim();
    if draft.is_empty() {
        return None;
    }
    if has_url_scheme(draft) {
        return Some(draft.to_owned());
    }
    if looks_like_host(draft) {
        return Some(format!("https://{draft}"));
    }
    Some(format!(
        "{DEFAULT_SEARCH_URL}?q={}",
        percent_encode_query(draft)
    ))
}

fn should_fetch_suggestions(draft: &str) -> bool {
    let draft = draft.trim();
    !draft.is_empty() && !has_url_scheme(draft) && !looks_like_host(draft)
}

fn suggestions_url(query: &str) -> String {
    format!(
        "{DEFAULT_SUGGEST_URL}?q={}",
        percent_encode_query(query.trim())
    )
}

fn fetch_suggestions(query: &str) -> Result<Vec<String>, String> {
    let body = reqwest::blocking::Client::builder()
        .timeout(SUGGEST_TIMEOUT)
        .build()
        .map_err(|e| format!("Suggestions unavailable: {e}"))?
        .get(suggestions_url(query))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| format!("Suggestions unavailable: {e}"))?
        .text()
        .map_err(|e| format!("Suggestions unavailable: {e}"))?;
    parse_suggestions_json(query, &body)
}

fn parse_suggestions_json(query: &str, body: &str) -> Result<Vec<String>, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Invalid suggestions JSON: {e}"))?;
    let mut out = Vec::new();
    collect_suggestion_values(query.trim(), &value, &mut out);
    out.truncate(8);
    Ok(out)
}

fn collect_suggestion_values(query: &str, value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => push_suggestion(query, s, out),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_suggestion_values(query, item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for key in [
                "suggestions",
                "results",
                "completions",
                "value",
                "phrase",
                "text",
            ] {
                if let Some(v) = map.get(key) {
                    collect_suggestion_values(query, v, out);
                }
            }
        }
        _ => {}
    }
}

fn push_suggestion(query: &str, value: &str, out: &mut Vec<String>) {
    let value = value.trim();
    if value.is_empty() || value == query || out.iter().any(|s| s == value) {
        return;
    }
    out.push(value.to_owned());
}

fn has_url_scheme(s: &str) -> bool {
    if let Some((scheme, _rest)) = s.split_once("://") {
        return valid_scheme(scheme);
    }
    let Some((scheme, _rest)) = s.split_once(':') else {
        return false;
    };
    matches!(
        scheme,
        "about" | "data" | "file" | "mailto" | "tel" | "magnet" | "view-source"
    )
}

fn valid_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn looks_like_host(s: &str) -> bool {
    if s.contains(char::is_whitespace) {
        return false;
    }
    let host = s.split('/').next().unwrap_or(s);
    host == "localhost"
        || host.contains('.')
        || host.contains(':')
        || host.chars().all(|c| c.is_ascii_digit() || c == '.')
}

fn percent_encode_query(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b));
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn is_plain_http(url: &str) -> bool {
    url.trim_start().starts_with("http://")
}

fn is_new_tab_url(url: &str) -> bool {
    matches!(url.trim(), "" | "about:blank")
}

fn https_upgrade(url: &str) -> String {
    let trimmed = url.trim();
    trimmed
        .strip_prefix("http://")
        .map_or_else(|| trimmed.to_owned(), |rest| format!("https://{rest}"))
}

/// Mint the transfer job a completed browser download or scraper output uses once
/// the helper has materialized the file locally. The browser does not crawl or move
/// bytes itself here; it hands each resulting file to the daemon-owned Transfers
/// queue, preserving one download manager and one ledger (TRANSFERS-10).
fn browser_output_transfer_job(source: &str, dest: &str) -> TransferJob {
    TransferJob::new(
        source.trim(),
        dest.trim(),
        TransferMethod::BrowserDownload,
        TransferPolicy {
            bwlimit: None,
            verify: true,
        },
    )
}

/// Enqueue one materialized browser output into the shared Transfers queue.
///
/// # Errors
/// Returns an honest validation/dispatch error when either path is empty or the
/// transfer client cannot write the daemon inbox.
fn enqueue_browser_output(
    transfers: &dyn TransfersClient,
    source: &str,
    dest: &str,
) -> Result<String, String> {
    if source.trim().is_empty() || dest.trim().is_empty() {
        return Err("browser transfer enqueue requires source and destination paths".into());
    }
    let job = browser_output_transfer_job(source, dest);
    let id = job.id.clone();
    transfers.dispatch(&TransferVerb::Submit(job))?;
    Ok(id)
}

/// Enqueue every file produced by a Power-mode scrape/export batch. Each file
/// becomes its own `browser_download` job, so progress, verify, notify, pause, and
/// history stay in the same Transfers surface as ordinary downloads.
///
/// # Errors
/// Returns the first validation/dispatch error; any ids returned before that point
/// were already handed to the transfer worker.
fn enqueue_browser_output_batch(
    transfers: &dyn TransfersClient,
    sources: &[String],
    dest: &str,
) -> Result<Vec<String>, String> {
    let mut ids = Vec::with_capacity(sources.len());
    for source in sources {
        ids.push(enqueue_browser_output(transfers, source, dest)?);
    }
    Ok(ids)
}

/// Publish `body` on `topic` via the persist-first path (the same discipline as
/// the shell's Chat composer + Files' `chat_bridge`). Best-effort: no Bus on this
/// node / a transient open failure is a silent no-op — the honest solo-host
/// state, never a panic.
fn publish(topic: &str, body: &str) {
    publish_to_bus(mde_bus::client_data_dir().as_deref(), topic, body);
}

fn publish_to_bus(root: Option<&Path>, topic: &str, body: &str) {
    let Some(root) = root else { return };
    let Ok(persist) = Persist::open(root.to_path_buf()) else {
        return;
    };
    let _ = persist.write(topic, Priority::Default, None, Some(body));
}

/// The local hostname — the mesh identity a Send-in-Chat addresses (lock 2/21:
/// the hostname *is* the chat contact username). `$HOSTNAME` →
/// `/proc/sys/kernel/hostname` → `/etc/hostname` → `"localhost"`.
fn local_hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(h) = std::fs::read_to_string(path) {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_string();
            }
        }
    }
    "localhost".to_string()
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn browser_capture_dir() -> PathBuf {
    std::env::var_os("XDG_PICTURES_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Pictures")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("Magic Mesh Browser Captures")
}

fn browser_pdf_dir() -> PathBuf {
    std::env::var_os("XDG_DOCUMENTS_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Documents")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("Magic Mesh Browser PDFs")
}

fn browser_print_spool_dir() -> PathBuf {
    std::env::temp_dir().join("mde-browser-cups")
}

fn capture_filename_for(url: &str, title: &str, unix_ms: u64) -> String {
    output_filename_for("mde-browser", "png", url, title, unix_ms)
}

fn capture_full_page_filename_for(url: &str, title: &str, unix_ms: u64) -> String {
    output_filename_for("mde-browser-full-page", "png", url, title, unix_ms)
}

fn capture_mhtml_filename_for(url: &str, title: &str, unix_ms: u64) -> String {
    output_filename_for("mde-browser", "mhtml", url, title, unix_ms)
}

fn capture_annotated_filename_for(url: &str, title: &str, unix_ms: u64) -> String {
    output_filename_for("mde-browser-annotated", "png", url, title, unix_ms)
}

fn capture_callout_filename_for(url: &str, title: &str, unix_ms: u64) -> String {
    output_filename_for("mde-browser-callout", "png", url, title, unix_ms)
}

fn capture_freehand_filename_for(url: &str, title: &str, unix_ms: u64) -> String {
    output_filename_for("mde-browser-freehand", "png", url, title, unix_ms)
}

fn capture_region_filename_for(url: &str, title: &str, unix_ms: u64) -> String {
    output_filename_for("mde-browser-region", "png", url, title, unix_ms)
}

fn pdf_filename_for(url: &str, title: &str, unix_ms: u64) -> String {
    output_filename_for("mde-browser", "pdf", url, title, unix_ms)
}

fn print_pdf_filename_for(url: &str, title: &str, unix_ms: u64) -> String {
    output_filename_for("mde-browser-print", "pdf", url, title, unix_ms)
}

fn pdf_file_looks_readable(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    use std::io::Read;
    file.read_exact(&mut magic).is_ok() && magic == *b"%PDF"
}

fn file_url_for_path(path: &Path) -> Result<String, String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| format!("could not resolve current directory: {err}"))?
            .join(path)
    };
    let text = path.to_string_lossy();
    let mut out = String::from("file://");
    for byte in text.as_bytes() {
        match *byte {
            b'/' => out.push('/'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(*byte));
            }
            byte => out.push_str(&format!("%{byte:02X}")),
        }
    }
    Ok(out)
}

fn output_filename_for(prefix: &str, ext: &str, url: &str, title: &str, unix_ms: u64) -> String {
    let seed = host_of(url)
        .or_else(|| {
            let title = title.trim();
            (!title.is_empty()).then(|| title.to_owned())
        })
        .unwrap_or_else(|| "page".to_owned());
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in seed.chars() {
        let out = if ch.is_ascii_alphanumeric() {
            last_dash = false;
            Some(ch.to_ascii_lowercase())
        } else if !last_dash {
            last_dash = true;
            Some('-')
        } else {
            None
        };
        if let Some(ch) = out {
            slug.push(ch);
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "page" } else { slug };
    format!("{prefix}-{unix_ms}-{slug}.{ext}")
}

fn capture_annotation_caption(url: &str, title: &str, unix_ms: u64) -> String {
    let title = title.trim();
    let label = if title.is_empty() {
        host_of(url).unwrap_or_else(|| "page".to_owned())
    } else {
        title.to_owned()
    };
    format!("{label} | {url} | {unix_ms}")
}

fn mhtml_capture_document(url: &str, title: &str, unix_ms: u64, png: &[u8]) -> Vec<u8> {
    const BOUNDARY: &str = "----=_MagicMeshBrowserCapture";
    const IMAGE_LOCATION: &str = "mde-browser-capture.png";
    let title = title.trim();
    let label = if title.is_empty() {
        host_of(url).unwrap_or_else(|| "Browser Capture".to_owned())
    } else {
        title.to_owned()
    };
    let html = format!(
        concat!(
            "<!doctype html><html><head><meta charset=\"utf-8\">",
            "<title>{title}</title></head><body>",
            "<h1>{title}</h1>",
            "<p>Captured from <a href=\"{url}\">{url}</a></p>",
            "<p>Capture time: {unix_ms}</p>",
            "<img src=\"{image_location}\" alt=\"Browser capture\">",
            "</body></html>"
        ),
        title = html_escape(&label),
        url = html_escape(url),
        unix_ms = unix_ms,
        image_location = IMAGE_LOCATION
    );
    let encoded_png = base64::engine::general_purpose::STANDARD.encode(png);
    let mut out = String::new();
    out.push_str("MIME-Version: 1.0\r\n");
    out.push_str(&format!(
        "Content-Type: multipart/related; type=\"text/html\"; boundary=\"{BOUNDARY}\"\r\n"
    ));
    out.push_str(&format!(
        "Subject: Magic Mesh Browser Capture - {}\r\n\r\n",
        mhtml_header_value(&html_escape(&label))
    ));
    out.push_str(&format!("--{BOUNDARY}\r\n"));
    out.push_str("Content-Type: text/html; charset=\"utf-8\"\r\n");
    out.push_str("Content-Transfer-Encoding: 8bit\r\n");
    out.push_str(&format!(
        "Content-Location: {}\r\n\r\n",
        if url.trim().is_empty() {
            "about:blank"
        } else {
            url.trim()
        }
    ));
    out.push_str(&html);
    out.push_str("\r\n");
    out.push_str(&format!("--{BOUNDARY}\r\n"));
    out.push_str("Content-Type: image/png\r\n");
    out.push_str("Content-Transfer-Encoding: base64\r\n");
    out.push_str(&format!("Content-Location: {IMAGE_LOCATION}\r\n\r\n"));
    for chunk in encoded_png.as_bytes().chunks(76) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        out.push_str("\r\n");
    }
    out.push_str(&format!("--{BOUNDARY}--\r\n"));
    out.into_bytes()
}

fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn mhtml_header_value(text: &str) -> String {
    text.chars()
        .map(|ch| if ch == '\r' || ch == '\n' { ' ' } else { ch })
        .collect()
}

fn cups_job_title(url: &str, title: &str, unix_ms: u64) -> String {
    let seed = {
        let title = title.trim();
        if title.is_empty() {
            host_of(url).unwrap_or_else(|| "Browser page".to_owned())
        } else {
            title.to_owned()
        }
    };
    let mut out = String::new();
    let mut last_space = false;
    for ch in seed.chars() {
        let next = if ch.is_ascii_graphic() {
            last_space = false;
            Some(ch)
        } else if ch.is_whitespace() && !last_space {
            last_space = true;
            Some(' ')
        } else {
            None
        };
        if let Some(ch) = next {
            out.push(ch);
        }
        if out.len() >= 80 {
            break;
        }
    }
    let out = out.trim();
    if out.is_empty() {
        format!("Magic Mesh Browser {unix_ms}")
    } else {
        format!("Magic Mesh Browser - {out}")
    }
}

fn discover_cups_printers() -> Result<Vec<CupsPrinter>, String> {
    discover_cups_printers_with_runner(run_process_with_timeout)
}

fn discover_cups_printers_with_runner(
    runner: impl Fn(&str, &[String], Duration) -> Result<ProcessOutput, String>,
) -> Result<Vec<CupsPrinter>, String> {
    let names = runner("lpstat", &["-e".to_owned()], CUPS_PRINT_TIMEOUT)?;
    if !names.success {
        return Err(process_error("lpstat -e", &names));
    }
    let default = runner("lpstat", &["-d".to_owned()], CUPS_PRINT_TIMEOUT).ok();
    let default_name = default
        .as_ref()
        .filter(|output| output.success)
        .and_then(|output| parse_cups_default_destination(&output.stdout));
    let mut printers = parse_cups_printer_names(&names.stdout)
        .into_iter()
        .map(|name| CupsPrinter {
            is_default: default_name.as_deref() == Some(name.as_str()),
            name,
        })
        .collect::<Vec<_>>();
    printers.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(printers)
}

fn parse_cups_printer_names(stdout: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter(|name| seen.insert((*name).to_owned()))
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_cups_default_destination(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| {
            line.rsplit_once(':')
                .map(|(_, name)| name.trim().to_owned())
        })
        .filter(|name| !name.is_empty())
}

fn submit_pdf_to_cups(
    path: &Path,
    title: &str,
    settings: &CupsPrintSettings,
) -> Result<String, String> {
    submit_pdf_to_cups_with_runner(path, title, settings, run_process_with_timeout)
}

fn submit_pdf_to_cups_with_runner(
    path: &Path,
    title: &str,
    settings: &CupsPrintSettings,
    runner: impl FnOnce(&str, &[String], Duration) -> Result<ProcessOutput, String>,
) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    let path_arg = path.to_string_lossy().into_owned();
    let mut args = Vec::new();
    if let Some(destination) = settings
        .destination
        .as_deref()
        .map(str::trim)
        .filter(|destination| !destination.is_empty())
    {
        args.push("-d".to_owned());
        args.push(destination.to_owned());
    }
    if settings.copies > 1 {
        args.push("-n".to_owned());
        args.push(settings.copies.min(99).to_string());
    }
    if settings.duplex {
        args.push("-o".to_owned());
        args.push("sides=two-sided-long-edge".to_owned());
    }
    if settings.grayscale {
        args.push("-o".to_owned());
        args.push("ColorModel=Gray".to_owned());
    }
    args.push("-t".to_owned());
    args.push(title.to_owned());
    args.push(path_arg.clone());
    let output = runner("lp", &args, CUPS_PRINT_TIMEOUT)?;
    if output.success {
        let job = output.stdout.trim();
        if job.is_empty() {
            Ok(path_arg)
        } else {
            Ok(job.to_owned())
        }
    } else {
        let err = output.stderr.trim();
        if err.is_empty() {
            Err("lp failed without an error message".to_owned())
        } else {
            Err(err.to_owned())
        }
    }
}

fn process_error(command: &str, output: &ProcessOutput) -> String {
    let err = output.stderr.trim();
    if err.is_empty() {
        format!("{command} failed without an error message")
    } else {
        err.to_owned()
    }
}

fn run_process_with_timeout(
    program: &str,
    args: &[String],
    timeout: Duration,
) -> Result<ProcessOutput, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("{program} failed to start: {err}"))?;
    let started = Instant::now();
    while started.elapsed() < timeout {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|err| format!("{program} output failed: {err}"))?;
                return Ok(ProcessOutput {
                    success: output.status.success(),
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                });
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => return Err(format!("{program} status failed: {err}")),
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(format!("{program} timed out after {}s", timeout.as_secs()))
}

fn encode_color_image_png(img: &egui::ColorImage) -> Result<Vec<u8>, String> {
    let [w, h] = img.size;
    if w == 0 || h == 0 {
        return Err("empty frame".to_owned());
    }
    let expected = w
        .checked_mul(h)
        .ok_or_else(|| "frame dimensions overflow".to_owned())?;
    if img.pixels.len() != expected {
        return Err(format!(
            "frame has {} pixels but expected {expected}",
            img.pixels.len()
        ));
    }
    let mut rgba = Vec::with_capacity(expected * 4);
    for pixel in &img.pixels {
        rgba.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), pixel.a()]);
    }
    let mut bytes = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut bytes, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc
            .write_header()
            .map_err(|err| format!("could not write PNG header: {err}"))?;
        writer
            .write_image_data(&rgba)
            .map_err(|err| format!("could not write PNG pixels: {err}"))?;
    }
    Ok(bytes)
}

const ANNOTATION_BAR_HEIGHT: usize = 24;
const ANNOTATION_TEXT_SCALE: usize = 2;

fn annotate_capture_image(
    img: &egui::ColorImage,
    caption: &str,
) -> Result<egui::ColorImage, String> {
    let [w, h] = img.size;
    if w == 0 || h == 0 {
        return Err("empty frame".to_owned());
    }
    let out_h = h
        .checked_add(ANNOTATION_BAR_HEIGHT)
        .ok_or_else(|| "annotated frame dimensions overflow".to_owned())?;
    let expected = w
        .checked_mul(h)
        .ok_or_else(|| "frame dimensions overflow".to_owned())?;
    if img.pixels.len() != expected {
        return Err(format!(
            "frame has {} pixels but expected {expected}",
            img.pixels.len()
        ));
    }
    let mut out = egui::ColorImage::new([w, out_h], egui::Color32::from_rgb(15, 20, 25));
    out.pixels[..expected].copy_from_slice(&img.pixels);
    for y in h..out_h {
        for x in 0..w {
            out.pixels[y * w + x] = if y == h {
                Style::ACCENT
            } else {
                egui::Color32::from_rgb(21, 27, 34)
            };
        }
    }
    draw_tiny_text(
        &mut out,
        6,
        h + 6,
        &caption.to_ascii_uppercase(),
        Style::TEXT,
    );
    Ok(out)
}

fn annotate_callout_capture_image(
    img: &egui::ColorImage,
    caption: &str,
) -> Result<egui::ColorImage, String> {
    let [w, h] = img.size;
    let mut out = annotate_capture_image(img, caption)?;
    if w < 16 || h < 12 {
        draw_tiny_text(
            &mut out,
            6,
            h + 6,
            "CALLOUT",
            egui::Color32::from_rgb(255, 255, 255),
        );
        return Ok(out);
    }

    let box_w = (w / 3).clamp(12, 180);
    let box_h = (h / 3).clamp(8, 96);
    let x = (w.saturating_sub(box_w)) / 2;
    let y = (h.saturating_sub(box_h)) / 2;
    let accent = Style::ACCENT;
    draw_rect_outline(&mut out, x, y, box_w, box_h, accent);

    let leader_start_x = x.saturating_add(box_w);
    let leader_start_y = y;
    let leader_end_x = w.saturating_sub(3);
    let leader_end_y = 3;
    draw_diagonal_line(
        &mut out,
        leader_start_x,
        leader_start_y,
        leader_end_x,
        leader_end_y,
        accent,
    );
    draw_rect_outline(
        &mut out,
        leader_end_x.saturating_sub(10),
        leader_end_y,
        10,
        8,
        accent,
    );
    draw_tiny_text(
        &mut out,
        leader_end_x.saturating_sub(8),
        leader_end_y.saturating_add(1),
        "1",
        egui::Color32::from_rgb(255, 255, 255),
    );
    draw_tiny_text(
        &mut out,
        6,
        h + 6,
        "CALLOUT",
        egui::Color32::from_rgb(255, 255, 255),
    );
    Ok(out)
}

fn annotate_freehand_capture_image(
    img: &egui::ColorImage,
    caption: &str,
) -> Result<egui::ColorImage, String> {
    let [w, h] = img.size;
    let mut out = annotate_capture_image(img, caption)?;
    let stroke = egui::Color32::from_rgb(255, 255, 255);
    if w < 16 || h < 12 {
        draw_tiny_text(&mut out, 6, h + 6, "FREEHAND", stroke);
        return Ok(out);
    }

    let left = w / 5;
    let right = w.saturating_sub(left.max(1));
    let top = h / 4;
    let mid = h / 2;
    let bottom = h.saturating_sub(top.max(1));
    let points = [
        (left, mid),
        (left.saturating_add(w / 10), top),
        (left.saturating_add(w / 4), bottom),
        (left.saturating_add(w / 2), top.saturating_add(h / 8)),
        (right, mid),
    ];
    for segment in points.windows(2) {
        let [(x0, y0), (x1, y1)] = segment else {
            continue;
        };
        draw_thick_line(&mut out, *x0, *y0, *x1, *y1, stroke);
    }
    draw_tiny_text(&mut out, 6, h + 6, "FREEHAND", stroke);
    Ok(out)
}

fn draw_thick_line(
    img: &mut egui::ColorImage,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    color: egui::Color32,
) {
    for (dx, dy) in [(0isize, 0isize), (1, 0), (-1, 0), (0, 1), (0, -1)] {
        let sx0 = offset_coord(x0, dx);
        let sy0 = offset_coord(y0, dy);
        let sx1 = offset_coord(x1, dx);
        let sy1 = offset_coord(y1, dy);
        draw_diagonal_line(img, sx0, sy0, sx1, sy1, color);
    }
}

fn offset_coord(value: usize, delta: isize) -> usize {
    if delta.is_negative() {
        value.saturating_sub(delta.unsigned_abs())
    } else {
        value.saturating_add(delta as usize)
    }
}

fn draw_rect_outline(
    img: &mut egui::ColorImage,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    color: egui::Color32,
) {
    if width == 0 || height == 0 {
        return;
    }
    let right = x.saturating_add(width.saturating_sub(1));
    let bottom = y.saturating_add(height.saturating_sub(1));
    for px in x..=right {
        set_pixel(img, px, y, color);
        set_pixel(img, px, bottom, color);
    }
    for py in y..=bottom {
        set_pixel(img, x, py, color);
        set_pixel(img, right, py, color);
    }
}

fn draw_diagonal_line(
    img: &mut egui::ColorImage,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    color: egui::Color32,
) {
    let mut x0 = isize::try_from(x0).unwrap_or(isize::MAX);
    let mut y0 = isize::try_from(y0).unwrap_or(isize::MAX);
    let x1 = isize::try_from(x1).unwrap_or(isize::MAX);
    let y1 = isize::try_from(y1).unwrap_or(isize::MAX);
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        if let (Ok(x), Ok(y)) = (usize::try_from(x0), usize::try_from(y0)) {
            set_pixel(img, x, y, color);
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = err.saturating_mul(2);
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn set_pixel(img: &mut egui::ColorImage, x: usize, y: usize, color: egui::Color32) {
    let [w, h] = img.size;
    if x < w && y < h {
        img.pixels[y * w + x] = color;
    }
}

fn draw_tiny_text(
    img: &mut egui::ColorImage,
    mut x: usize,
    y: usize,
    text: &str,
    color: egui::Color32,
) {
    for ch in text.chars() {
        let glyph = tiny_glyph(ch);
        draw_tiny_glyph(img, x, y, glyph, color);
        x = x.saturating_add(6 * ANNOTATION_TEXT_SCALE);
        if x + 5 * ANNOTATION_TEXT_SCALE >= img.size[0] {
            break;
        }
    }
}

fn draw_tiny_glyph(
    img: &mut egui::ColorImage,
    x: usize,
    y: usize,
    glyph: [&'static str; 7],
    color: egui::Color32,
) {
    let [w, h] = img.size;
    for (gy, row) in glyph.iter().enumerate() {
        for (gx, bit) in row.as_bytes().iter().enumerate() {
            if *bit != b'1' {
                continue;
            }
            for sy in 0..ANNOTATION_TEXT_SCALE {
                for sx in 0..ANNOTATION_TEXT_SCALE {
                    let px = x + gx * ANNOTATION_TEXT_SCALE + sx;
                    let py = y + gy * ANNOTATION_TEXT_SCALE + sy;
                    if px < w && py < h {
                        img.pixels[py * w + px] = color;
                    }
                }
            }
        }
    }
}

fn tiny_glyph(ch: char) -> [&'static str; 7] {
    match ch {
        'A' => [
            "01110", "10001", "10001", "11111", "10001", "10001", "10001",
        ],
        'B' => [
            "11110", "10001", "10001", "11110", "10001", "10001", "11110",
        ],
        'C' => [
            "01111", "10000", "10000", "10000", "10000", "10000", "01111",
        ],
        'D' => [
            "11110", "10001", "10001", "10001", "10001", "10001", "11110",
        ],
        'E' => [
            "11111", "10000", "10000", "11110", "10000", "10000", "11111",
        ],
        'F' => [
            "11111", "10000", "10000", "11110", "10000", "10000", "10000",
        ],
        'G' => [
            "01111", "10000", "10000", "10111", "10001", "10001", "01111",
        ],
        'H' => [
            "10001", "10001", "10001", "11111", "10001", "10001", "10001",
        ],
        'I' => [
            "11111", "00100", "00100", "00100", "00100", "00100", "11111",
        ],
        'J' => [
            "00111", "00010", "00010", "00010", "00010", "10010", "01100",
        ],
        'K' => [
            "10001", "10010", "10100", "11000", "10100", "10010", "10001",
        ],
        'L' => [
            "10000", "10000", "10000", "10000", "10000", "10000", "11111",
        ],
        'M' => [
            "10001", "11011", "10101", "10101", "10001", "10001", "10001",
        ],
        'N' => [
            "10001", "11001", "10101", "10011", "10001", "10001", "10001",
        ],
        'O' => [
            "01110", "10001", "10001", "10001", "10001", "10001", "01110",
        ],
        'P' => [
            "11110", "10001", "10001", "11110", "10000", "10000", "10000",
        ],
        'Q' => [
            "01110", "10001", "10001", "10001", "10101", "10010", "01101",
        ],
        'R' => [
            "11110", "10001", "10001", "11110", "10100", "10010", "10001",
        ],
        'S' => [
            "01111", "10000", "10000", "01110", "00001", "00001", "11110",
        ],
        'T' => [
            "11111", "00100", "00100", "00100", "00100", "00100", "00100",
        ],
        'U' => [
            "10001", "10001", "10001", "10001", "10001", "10001", "01110",
        ],
        'V' => [
            "10001", "10001", "10001", "10001", "10001", "01010", "00100",
        ],
        'W' => [
            "10001", "10001", "10001", "10101", "10101", "10101", "01010",
        ],
        'X' => [
            "10001", "10001", "01010", "00100", "01010", "10001", "10001",
        ],
        'Y' => [
            "10001", "10001", "01010", "00100", "00100", "00100", "00100",
        ],
        'Z' => [
            "11111", "00001", "00010", "00100", "01000", "10000", "11111",
        ],
        '0' => [
            "01110", "10001", "10011", "10101", "11001", "10001", "01110",
        ],
        '1' => [
            "00100", "01100", "00100", "00100", "00100", "00100", "01110",
        ],
        '2' => [
            "01110", "10001", "00001", "00010", "00100", "01000", "11111",
        ],
        '3' => [
            "11110", "00001", "00001", "01110", "00001", "00001", "11110",
        ],
        '4' => [
            "00010", "00110", "01010", "10010", "11111", "00010", "00010",
        ],
        '5' => [
            "11111", "10000", "10000", "11110", "00001", "00001", "11110",
        ],
        '6' => [
            "01110", "10000", "10000", "11110", "10001", "10001", "01110",
        ],
        '7' => [
            "11111", "00001", "00010", "00100", "01000", "01000", "01000",
        ],
        '8' => [
            "01110", "10001", "10001", "01110", "10001", "10001", "01110",
        ],
        '9' => [
            "01110", "10001", "10001", "01111", "00001", "00001", "01110",
        ],
        ':' => [
            "00000", "00100", "00100", "00000", "00100", "00100", "00000",
        ],
        '/' => [
            "00001", "00010", "00010", "00100", "01000", "01000", "10000",
        ],
        '.' => [
            "00000", "00000", "00000", "00000", "00000", "01100", "01100",
        ],
        '-' => [
            "00000", "00000", "00000", "11111", "00000", "00000", "00000",
        ],
        '_' => [
            "00000", "00000", "00000", "00000", "00000", "00000", "11111",
        ],
        '|' => [
            "00100", "00100", "00100", "00100", "00100", "00100", "00100",
        ],
        ' ' => [
            "00000", "00000", "00000", "00000", "00000", "00000", "00000",
        ],
        _ => [
            "00000", "00000", "00000", "01110", "00000", "00000", "00000",
        ],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PixelRegion {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

impl PixelRegion {
    fn from_points(a: egui::Pos2, b: egui::Pos2, frame_size: [usize; 2]) -> Option<Self> {
        let [frame_w, frame_h] = frame_size;
        if frame_w == 0 || frame_h == 0 {
            return None;
        }
        let min_x = a.x.min(b.x).floor().clamp(0.0, frame_w as f32) as usize;
        let min_y = a.y.min(b.y).floor().clamp(0.0, frame_h as f32) as usize;
        let max_x = a.x.max(b.x).ceil().clamp(0.0, frame_w as f32) as usize;
        let max_y = a.y.max(b.y).ceil().clamp(0.0, frame_h as f32) as usize;
        let width = max_x.saturating_sub(min_x);
        let height = max_y.saturating_sub(min_y);
        (width > 1 && height > 1).then_some(Self {
            x: min_x,
            y: min_y,
            width,
            height,
        })
    }

    fn rect_on_image(self, image_rect: egui::Rect, frame_size: [usize; 2]) -> egui::Rect {
        let [frame_w, frame_h] = frame_size;
        let sx = image_rect.width() / frame_w.max(1) as f32;
        let sy = image_rect.height() / frame_h.max(1) as f32;
        egui::Rect::from_min_size(
            image_rect.min + egui::vec2(self.x as f32 * sx, self.y as f32 * sy),
            egui::vec2(self.width as f32 * sx, self.height as f32 * sy),
        )
    }
}

fn crop_color_image(
    img: &egui::ColorImage,
    region: PixelRegion,
) -> Result<egui::ColorImage, String> {
    let [w, h] = img.size;
    if region.x >= w
        || region.y >= h
        || region.width == 0
        || region.height == 0
        || region.x + region.width > w
        || region.y + region.height > h
    {
        return Err("capture region is outside the active frame".to_owned());
    }
    let mut pixels = Vec::with_capacity(region.width * region.height);
    for row in region.y..region.y + region.height {
        let start = row * w + region.x;
        let end = start + region.width;
        pixels.extend_from_slice(&img.pixels[start..end]);
    }
    let mut out = egui::ColorImage::new([region.width, region.height], egui::Color32::TRANSPARENT);
    out.pixels = pixels;
    Ok(out)
}

/// The Browser page-actions menu (BOOKMARKS-10): the three mesh-integration verbs
/// on the current page. Rendered by BOTH the toolbar menu button and the address
/// bar's right-click context menu (one body, two entry points). Each item greys
/// out with no live URL to act on. §4 Carbon tokens on the chrome.
fn page_actions_menu(
    ui: &mut egui::Ui,
    bus_root: Option<&Path>,
    engine: Option<BrowserEngine>,
    url: &str,
    title: &str,
) {
    let has_page = !url.trim().is_empty();
    // Add-from-page → the mesh-synced bookmarks store (via the worker's add verb).
    if ui
        .add_enabled(
            has_page,
            egui::Button::new(RichText::new("\u{2606}  Add bookmark").color(Style::TEXT)),
        )
        .clicked()
    {
        publish(ACTION_BOOKMARKS_ADD, &bookmark_add_body(url, title));
        ui.close_menu();
    }
    // Copy-URL → the shell clipboard (egui's output command).
    if ui
        .add_enabled(
            has_page,
            egui::Button::new(RichText::new("\u{29C9}  Copy URL").color(Style::TEXT)),
        )
        .clicked()
    {
        ui.ctx().copy_text(url.to_string());
        ui.close_menu();
    }
    // Send-in-Chat → the NOTIFY-CHAT send verb.
    if ui
        .add_enabled(
            has_page,
            egui::Button::new(RichText::new("\u{1F4AC}  Send in Chat").color(Style::TEXT)),
        )
        .clicked()
    {
        publish(
            ACTION_CHAT_SEND,
            &chat_share_body(&local_hostname(), url, title),
        );
        ui.close_menu();
    }
    for target in [
        BrowserShareTarget::Peer,
        BrowserShareTarget::Email,
        BrowserShareTarget::Qr,
    ] {
        if ui
            .add_enabled(
                has_page,
                egui::Button::new(
                    RichText::new(format!("{}  Share to {}", "\u{21AA}", target.label()))
                        .color(Style::TEXT),
                ),
            )
            .clicked()
        {
            publish_browser_share(bus_root, target, url, title);
            ui.close_menu();
        }
    }
    for target in [BrowserSendTabTarget::Node, BrowserSendTabTarget::Phone] {
        if ui
            .add_enabled(
                has_page,
                egui::Button::new(
                    RichText::new(format!("{}  Send tab to {}", "\u{21E5}", target.label()))
                        .color(Style::TEXT),
                ),
            )
            .clicked()
        {
            if let Some(engine) = engine {
                publish_browser_send_tab(bus_root, target, engine, url, title);
            }
            ui.close_menu();
        }
    }
}

/// The toolbar star that opens the BOOKMARKS-10 [`page_actions_menu`]; the glyph
/// dims with no live page (the menu items disable themselves too). Split out of
/// [`nav_chrome`] to keep that toolbar within its line budget.
fn page_actions_button(
    ui: &mut egui::Ui,
    has_page: bool,
    bus_root: Option<&Path>,
    engine: Option<BrowserEngine>,
    url: &str,
    title: &str,
) {
    let color = if has_page {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    ui.menu_button(
        RichText::new("\u{2606}").size(CHROME_FONT).color(color),
        |ui| {
            page_actions_menu(ui, bus_root, engine, url, title);
        },
    )
    .response
    .on_hover_text("Page actions \u{2014} bookmark, copy URL, share");
}

/// The navigation chrome bar — a §4-token toolbar. Back / forward / reload act on
/// the active session; the address bar loads on submit. On a crashed tab, Reload
/// becomes a respawn request. The page-actions menu (BOOKMARKS-10) hangs off both
/// the toolbar star button and the address bar's right-click.
fn nav_chrome(ui: &mut egui::Ui, state: &mut WebState) {
    let crashed = state
        .tabs
        .get(state.active)
        .is_some_and(|t| t.session.is_crashed());
    let nav = state
        .tabs
        .get(state.active)
        .map(|t| t.session.nav().clone())
        .unwrap_or_default();
    let active_engine = state.tabs.get(state.active).map(|t| t.engine);
    let has_tab = !state.tabs.is_empty();
    // BOOKMARKS-7 — the per-page ad-filter blocked count the active session tracks.
    let blocked = state
        .tabs
        .get(state.active)
        .map_or(0, |t| t.session.blocked_count());
    // BOOKMARKS-10 — the live page's URL + title, owned clones so the page-actions
    // closures never borrow `state`; empty when there is no live tab to act on.
    let page_url = state
        .tabs
        .get(state.active)
        .map(|t| t.session.nav().url.clone())
        .unwrap_or_default();
    let page_title = state
        .tabs
        .get(state.active)
        .map(|t| t.session.title().to_string())
        .unwrap_or_default();
    let has_page = has_tab && !crashed && !page_url.trim().is_empty();

    let mut accepted_suggestion: Option<String> = None;
    ui.horizontal(|ui| {
        // Back / forward — enabled only when the live session offers the history.
        if nav_button(ui, "\u{2039}", "Back", has_tab && !crashed && nav.can_back) {
            if let Some(tab) = state.active_tab() {
                tab.session.go_back();
            }
        }
        if nav_button(
            ui,
            "\u{203A}",
            "Forward",
            has_tab && !crashed && nav.can_forward,
        ) {
            if let Some(tab) = state.active_tab() {
                tab.session.go_forward();
            }
        }
        // Stop while CEF is loading; otherwise Reload respawns crashed tabs or
        // reloads the page. Servo currently has no real cancel-load hook, so its
        // compact chrome keeps the honest Reload control while loading.
        let can_stop =
            has_tab && !crashed && nav.loading && active_engine == Some(BrowserEngine::Cef);
        let (nav_label, nav_tip) = if can_stop {
            ("\u{00D7}", "Stop loading")
        } else if crashed {
            ("\u{21BB}", "Reload (restart page)")
        } else {
            ("\u{21BB}", "Reload")
        };
        if nav_button(ui, nav_label, nav_tip, has_tab) {
            if crashed {
                state.respawn_requested = true;
            } else if can_stop {
                if let Some(tab) = state.active_tab() {
                    tab.session.stop();
                }
            } else if let Some(tab) = state.active_tab() {
                tab.session.reload();
            }
        }

        // BOOKMARKS-10 — the page-actions menu (bookmark this page / copy its URL /
        // send it in Chat). The SAME three verbs also hang off the address bar's
        // right-click (below), so both the toolbar and the context menu reach them.
        page_actions_button(
            ui,
            has_page,
            state.bus_root.as_deref(),
            active_engine,
            &page_url,
            &page_title,
        );

        if nav_button(
            ui,
            "\u{25A3}",
            if state.capture_region_mode {
                "Select capture region"
            } else {
                "Capture viewport"
            },
            state.active_tab_has_frame(),
        ) {
            if state.capture_region_mode {
                state.cancel_region_capture();
            } else {
                state.capture_active_viewport();
            }
        }

        let (active_downloads, total_downloads) = state.download_counts();
        let downloads_label = if active_downloads > 0 {
            format!("\u{2193} {active_downloads}")
        } else {
            "\u{2193}".to_owned()
        };
        let downloads_tip = if total_downloads == 0 {
            "Downloads"
        } else {
            "Downloads from the shared Transfers ledger"
        };
        if ui
            .button(RichText::new(downloads_label).size(CHROME_FONT).color(
                if state.downloads_open {
                    Style::ACCENT
                } else {
                    Style::TEXT
                },
            ))
            .on_hover_text(downloads_tip)
            .clicked()
        {
            state.downloads_open = !state.downloads_open;
            if state.downloads_open {
                state.refresh_downloads();
            }
        }

        // BOOKMARKS-7 — a compact "N blocked" shield when the ad-filter has dropped
        // requests on this page (honest 0 stays hidden). Reads the session's
        // per-page counter; the engine is compiled from the mackesd `adfilter` blob.
        if blocked > 0 {
            ui.add_space(CHROME_GAP);
            ui.label(
                RichText::new(format!("\u{2298} {blocked}"))
                    .size(CHROME_FONT)
                    .color(Style::TEXT_DIM),
            )
            .on_hover_text(format!(
                "Ad-filter blocked {blocked} request{} on this page",
                if blocked == 1 { "" } else { "s" }
            ));
        }

        ui.add_space(CHROME_GAP);

        // The address bar fills the rest of the row.
        let field = egui::TextEdit::singleline(&mut state.address)
            .desired_width(ui.available_width() - Style::SP_XL * 2.0)
            .hint_text("Enter an address")
            .text_color(Style::TEXT)
            .font(egui::TextStyle::Small)
            .min_size(egui::vec2(160.0, CHROME_OMNIBOX_H));
        let resp = ui.add_enabled(has_tab && !crashed, field);
        if resp.changed() && has_tab && !crashed {
            state.update_suggestions_for_address();
        }
        // BOOKMARKS-10 — right-click the address bar for the same page actions
        // (bookmark / copy URL / Send-in-Chat) the toolbar star exposes.
        resp.context_menu(|ui| {
            page_actions_menu(
                ui,
                state.bus_root.as_deref(),
                active_engine,
                &page_url,
                &page_title,
            );
        });
        let submit = resp.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            && has_tab
            && !crashed;

        let go = ui
            .add_enabled(
                has_tab && !crashed && !state.address.trim().is_empty(),
                egui::Button::new(
                    RichText::new("\u{2192}")
                        .size(CHROME_FONT)
                        .color(Style::TEXT),
                )
                .min_size(egui::vec2(CHROME_BUTTON, CHROME_BUTTON)),
            )
            .on_hover_text("Go")
            .clicked();

        if submit || go {
            state.submit_address();
        }
    });
    if has_tab && !crashed {
        accepted_suggestion = suggestions_panel(ui, state);
    }
    if let Some(suggestion) = accepted_suggestion {
        state.accept_suggestion(suggestion);
    }
}

fn find_chrome(ui: &mut egui::Ui, state: &mut WebState) {
    if !state.find_open {
        return;
    }
    let enabled = state.can_drive_page_tools();
    let mut submit_forward = false;
    let mut submit_backward = false;
    egui::Frame::NONE
        .fill(Style::SURFACE)
        .inner_margin(egui::Margin::symmetric(4, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Find")
                        .size(CHROME_FONT)
                        .color(Style::TEXT_DIM),
                );
                let resp = ui.add_enabled(
                    enabled,
                    egui::TextEdit::singleline(&mut state.find_query)
                        .desired_width(220.0)
                        .hint_text("Find in page")
                        .text_color(Style::TEXT)
                        .font(egui::TextStyle::Small)
                        .min_size(egui::vec2(160.0, CHROME_OMNIBOX_H)),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if enter && ui.input(|i| i.modifiers.shift) {
                    submit_backward = true;
                } else if enter {
                    submit_forward = true;
                }
                if nav_button(ui, "\u{2191}", "Previous match", enabled) {
                    submit_backward = true;
                }
                if nav_button(ui, "\u{2193}", "Next match", enabled) {
                    submit_forward = true;
                }
                if nav_button(ui, "\u{00D7}", "Close find", true) {
                    state.close_find_bar();
                }
            });
        });
    if submit_backward {
        state.submit_find(true);
    } else if submit_forward {
        state.submit_find(false);
    }
}

fn print_settings_drawer(ui: &mut egui::Ui, state: &mut WebState) {
    if !state.print_settings_open {
        return;
    }

    let printers = state.cups_printers.clone();
    egui::Frame::NONE
        .fill(Style::SURFACE)
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Print").size(CHROME_FONT).color(Style::TEXT));
                ui.label(
                    RichText::new("CUPS destination")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("\u{00D7}")
                        .on_hover_text("Close print settings")
                        .clicked()
                    {
                        state.print_settings_open = false;
                    }
                    if ui
                        .small_button("\u{21BB}")
                        .on_hover_text("Refresh CUPS destinations")
                        .clicked()
                    {
                        state.refresh_cups_printers();
                    }
                });
            });

            if let Some(notice) = &state.cups_notice {
                ui.colored_label(
                    Style::DANGER,
                    RichText::new(format!("! {notice}")).size(Style::SMALL),
                );
            }

            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Destination").size(Style::SMALL));
                egui::ComboBox::from_id_salt("browser-cups-destination")
                    .selected_text(
                        state
                            .cups_settings
                            .destination
                            .as_deref()
                            .unwrap_or("System default"),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut state.cups_settings.destination,
                            None,
                            "System default",
                        );
                        for printer in &printers {
                            let label = if printer.is_default {
                                format!("{} (default)", printer.name)
                            } else {
                                printer.name.clone()
                            };
                            ui.selectable_value(
                                &mut state.cups_settings.destination,
                                Some(printer.name.clone()),
                                label,
                            );
                        }
                    });

                ui.separator();
                ui.label(RichText::new("Copies").size(Style::SMALL));
                ui.add(
                    egui::DragValue::new(&mut state.cups_settings.copies)
                        .range(1..=99)
                        .speed(1),
                );
                ui.checkbox(&mut state.cups_settings.duplex, "Duplex");
                ui.checkbox(&mut state.cups_settings.grayscale, "Grayscale");
                if ui
                    .add_enabled(
                        state.can_drive_page_tools(),
                        egui::Button::new(RichText::new("Print").size(Style::SMALL)),
                    )
                    .on_hover_text("Queue this page PDF and submit it to CUPS")
                    .clicked()
                {
                    state.print_active_page();
                }
            });

            if printers.is_empty() {
                muted_note(
                    ui,
                    "No CUPS destinations discovered; system default is still usable",
                );
            }
        });
}

fn downloads_drawer(ui: &mut egui::Ui, state: &mut WebState) {
    if !state.downloads_open {
        return;
    }

    let mut action: Option<TransferVerb> = None;
    let worker_present = state.transfers.worker_present();
    let jobs = state.download_jobs.clone();
    egui::Frame::NONE
        .fill(Style::SURFACE)
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Downloads")
                        .size(CHROME_FONT)
                        .color(Style::TEXT),
                );
                ui.label(
                    RichText::new("browser_download ledger")
                        .size(Style::SMALL)
                        .color(Style::TEXT_DIM),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("\u{00D7}")
                        .on_hover_text("Close downloads")
                        .clicked()
                    {
                        state.downloads_open = false;
                    }
                    if ui
                        .small_button("\u{21BB}")
                        .on_hover_text("Refresh downloads")
                        .clicked()
                    {
                        state.refresh_downloads();
                    }
                });
            });

            if let Some(notice) = &state.download_notice {
                ui.colored_label(
                    Style::DANGER,
                    RichText::new(format!("! {notice}")).size(Style::SMALL),
                );
            }

            if jobs.is_empty() {
                let message = if worker_present {
                    "No browser downloads yet"
                } else {
                    "Transfers worker ledger is not present on this node"
                };
                muted_note(ui, message);
                return;
            }

            for job in jobs.iter().take(6) {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(short_transfer_name(job))
                                    .size(Style::SMALL)
                                    .color(Style::TEXT),
                            );
                            ui.label(
                                RichText::new(job.state.label())
                                    .size(Style::SMALL)
                                    .color(download_state_color(job.state)),
                            );
                            if job.policy.verify {
                                ui.label(
                                    RichText::new("verify")
                                        .size(Style::SMALL)
                                        .color(Style::TEXT_DIM),
                                );
                            }
                        });
                        ui.label(
                            RichText::new(job.route())
                                .size(Style::SMALL)
                                .color(Style::TEXT_DIM),
                        );
                        if let Some(progress) = job.progress {
                            ui.add(
                                egui::ProgressBar::new(f32::from(progress) / 100.0)
                                    .desired_width((ui.available_width() * 0.55).max(120.0))
                                    .text(format!("{progress}%")),
                            );
                        }
                        if let Some(err) = &job.error {
                            ui.colored_label(
                                Style::DANGER,
                                RichText::new(format!("! {err}")).size(Style::SMALL),
                            );
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if !job.state.is_terminal()
                            && ui.small_button("Cancel").on_hover_text("Cancel").clicked()
                        {
                            action = Some(TransferVerb::Cancel(job.id.clone()));
                        }
                        if job.state.can_resume() && ui.small_button("Resume").clicked() {
                            action = Some(TransferVerb::Resume(job.id.clone()));
                        }
                        if job.state.can_pause() && ui.small_button("Pause").clicked() {
                            action = Some(TransferVerb::Pause(job.id.clone()));
                        }
                    });
                });
            }

            let hidden = jobs.len().saturating_sub(6);
            if hidden > 0 {
                muted_note(
                    ui,
                    &format!("{hidden} older browser download{} hidden", plural(hidden)),
                );
            }
        });

    if let Some(verb) = action {
        state.dispatch_download_verb(verb);
    }
}

fn short_transfer_name(job: &TransferJob) -> String {
    job.source
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .map_or_else(|| job.id.clone(), ToOwned::to_owned)
}

const fn download_state_color(state: TransferState) -> egui::Color32 {
    match state {
        TransferState::Done => Style::OK,
        TransferState::Failed => Style::DANGER,
        TransferState::Paused => Style::WARN,
        TransferState::Queued | TransferState::Running => Style::TEXT_DIM,
    }
}

fn suggestions_panel(ui: &mut egui::Ui, state: &WebState) -> Option<String> {
    if state.suggestions.items.is_empty() && state.suggestions.notice.is_none() {
        return None;
    }
    let mut accepted = None;
    ui.horizontal_wrapped(|ui| {
        ui.add_space(Style::SP_XL * 4.0);
        for suggestion in &state.suggestions.items {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(ellipsize(suggestion, 36))
                            .size(CHROME_FONT)
                            .color(Style::TEXT),
                    )
                    .fill(Style::SURFACE)
                    .min_size(egui::vec2(96.0, CHROME_BUTTON)),
                )
                .on_hover_text(format!("Search for {suggestion}"))
                .clicked()
            {
                accepted = Some(suggestion.clone());
            }
        }
        if state.suggestions.items.is_empty() {
            if let Some(notice) = state.suggestions.notice.as_deref() {
                muted_note(ui, notice);
            }
        }
    });
    accepted
}

fn insecure_prompt(ui: &mut egui::Ui, state: &mut WebState) {
    let Some(url) = state.insecure_prompt.clone() else {
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new("HTTP connection")
                .size(CHROME_FONT)
                .color(Style::WARN),
        );
        ui.label(RichText::new(ellipsize(&url, 64)).color(Style::TEXT_DIM));
        if ui
            .add(egui::Button::new(
                RichText::new("Use HTTPS")
                    .size(CHROME_FONT)
                    .color(Style::TEXT),
            ))
            .on_hover_text("Upgrade this navigation to HTTPS")
            .clicked()
        {
            state.upgrade_insecure_load();
        }
        if ui
            .add(egui::Button::new(
                RichText::new("Continue HTTP")
                    .size(CHROME_FONT)
                    .color(Style::WARN),
            ))
            .on_hover_text("Continue with the insecure HTTP URL")
            .clicked()
        {
            state.continue_insecure_load();
        }
        if ui
            .add(egui::Button::new(
                RichText::new("Cancel")
                    .size(CHROME_FONT)
                    .color(Style::TEXT_DIM),
            ))
            .clicked()
        {
            state.cancel_insecure_load();
        }
    });
}

fn new_tab_dashboard(ui: &mut egui::Ui, state: &mut WebState) {
    let mut submit_search = false;
    let mut open_service: Option<String> = None;
    centered(ui, |ui| {
        ui.label(
            RichText::new("Quasar Browser")
                .size(Style::HEADING)
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_M);
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut state.dashboard_query)
                    .desired_width(420.0)
                    .hint_text("Search the mesh")
                    .text_color(Style::TEXT),
            );
            submit_search = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui
                .add(egui::Button::new(
                    RichText::new("Search").color(Style::TEXT),
                ))
                .clicked()
            {
                submit_search = true;
            }
        });
        ui.add_space(Style::SP_M);
        ui.horizontal_wrapped(|ui| {
            for service in &state.speed_dial {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(service.label.as_str())
                                .size(Style::BODY)
                                .color(Style::TEXT),
                        )
                        .min_size(egui::vec2(112.0, Style::SP_XL)),
                    )
                    .on_hover_text(service.hint.as_str())
                    .clicked()
                {
                    open_service = Some(service.url.clone());
                }
            }
        });
    });
    if submit_search {
        state.submit_dashboard_search();
    }
    if let Some(url) = open_service {
        state.open_mesh_service(url);
    }
}

/// A compact chrome button in the §4 palette, returning whether it was clicked.
fn nav_button(ui: &mut egui::Ui, glyph: &str, tip: &str, enabled: bool) -> bool {
    let color = if enabled {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(glyph).size(CHROME_FONT).color(color))
            .min_size(egui::vec2(CHROME_BUTTON, CHROME_BUTTON)),
    )
    .on_hover_text(tip)
    .clicked()
}

/// Paint the active tab's decoded frame to fill the body and forward this frame's
/// egui input to the session (scaled by `pixels_per_point`).
fn paint_body(ui: &mut egui::Ui, state: &mut WebState, active: usize) {
    let Some((tex_id, texture_size, frame_size)) = state.tabs.get(active).and_then(|tab| {
        let texture = tab.texture.as_ref()?;
        Some((
            texture.id(),
            texture.size_vec2(),
            tab.last_frame.as_ref().map_or([0, 0], |frame| frame.size),
        ))
    }) else {
        return;
    };
    let size = ui.available_size();
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let image_rect = fit_rect_preserving_aspect(rect, texture_size);
    ui.painter().rect_filled(rect, 0.0, Style::SURFACE);
    egui::Image::new(egui::load::SizedTexture::new(tex_id, image_rect.size()))
        .paint_at(ui, image_rect);
    if state.capture_region_mode {
        handle_region_capture_drag(ui, state, &resp, image_rect, frame_size);
    }
    if resp.clicked() {
        resp.request_focus();
    }

    if state.capture_region_mode {
        return;
    }
    // Forward only page-owned input. Pointer geometry must be page-local before
    // the client scales it to helper device pixels; keyboard/text belongs to the
    // page only after the image has focus, so address-bar/chrome typing does not
    // leak into the helper.
    let ppp = ui.ctx().pixels_per_point();
    let browser_focused = resp.has_focus() || resp.clicked() || resp.dragged();
    for event in ui.input(|i| i.events.clone()) {
        if let Some(event) = browser_input_event(&event, image_rect, browser_focused) {
            if let Some(tab) = state.tabs.get_mut(active) {
                tab.last_activity = Instant::now();
                tab.idle_suspended = false;
                tab.session.send_input(&event, ppp);
            }
        }
    }
}

fn handle_region_capture_drag(
    ui: &mut egui::Ui,
    state: &mut WebState,
    resp: &egui::Response,
    image_rect: egui::Rect,
    frame_size: [usize; 2],
) {
    if frame_size[0] == 0 || frame_size[1] == 0 {
        state.cancel_region_capture();
        state.capture_notice = Some("Capture failed: no painted page".to_owned());
        return;
    }
    let pointer_to_frame = |pos: egui::Pos2| -> egui::Pos2 {
        let clamped = pos.clamp(image_rect.min, image_rect.max);
        let rel = clamped - image_rect.min;
        egui::pos2(
            rel.x * frame_size[0] as f32 / image_rect.width().max(1.0),
            rel.y * frame_size[1] as f32 / image_rect.height().max(1.0),
        )
    };
    if resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let pos = pointer_to_frame(pos);
            state.capture_region_start = Some(pos);
            state.capture_region_current = Some(pos);
        }
    } else if resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            state.capture_region_current = Some(pointer_to_frame(pos));
        }
    }

    if let (Some(start), Some(current)) = (state.capture_region_start, state.capture_region_current)
    {
        if let Some(region) = PixelRegion::from_points(start, current, frame_size) {
            let overlay = region.rect_on_image(image_rect, frame_size);
            ui.painter().rect_filled(
                overlay,
                0.0,
                egui::Color32::from_rgba_unmultiplied(15, 98, 254, 42),
            );
            ui.painter().rect_stroke(
                overlay,
                0.0,
                egui::Stroke::new(1.0, Style::ACCENT),
                egui::StrokeKind::Inside,
            );
        }
    }

    if resp.drag_stopped() {
        let result = state
            .capture_region_start
            .zip(state.capture_region_current)
            .and_then(|(start, current)| PixelRegion::from_points(start, current, frame_size))
            .ok_or_else(|| "selection is too small".to_owned())
            .and_then(|region| state.capture_active_region_to_dir(browser_capture_dir(), region));
        match result {
            Ok(path) => state.record_capture_success("Captured region", &path),
            Err(err) => state.capture_notice = Some(format!("Capture failed: {err}")),
        }
        state.cancel_region_capture();
    }
}

fn capture_notice(ui: &mut egui::Ui, state: &mut WebState) {
    let Some(notice) = state.capture_notice.clone() else {
        return;
    };
    let tone = if notice.starts_with("Capture failed:")
        || notice.starts_with("PDF failed")
        || notice.starts_with("PDF viewer failed:")
        || notice.starts_with("Print failed:")
    {
        Style::DANGER
    } else {
        Style::ACCENT
    };
    egui::Frame::NONE
        .fill(Style::SURFACE)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(tone, RichText::new(notice).size(Style::SMALL));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("\u{00D7}")
                        .on_hover_text("Dismiss capture notice")
                        .clicked()
                    {
                        state.capture_notice = None;
                    }
                });
            });
        });
}

fn fit_rect_preserving_aspect(outer: egui::Rect, content_size: egui::Vec2) -> egui::Rect {
    if content_size.x <= 0.0
        || content_size.y <= 0.0
        || outer.width() <= 0.0
        || outer.height() <= 0.0
    {
        return outer;
    }
    let scale = (outer.width() / content_size.x).min(outer.height() / content_size.y);
    let size = content_size * scale;
    egui::Rect::from_center_size(outer.center(), size)
}

fn browser_input_event(
    event: &egui::Event,
    rect: egui::Rect,
    browser_focused: bool,
) -> Option<egui::Event> {
    match event {
        egui::Event::PointerMoved(pos) => {
            if rect.contains(*pos) {
                Some(egui::Event::PointerMoved(*pos - rect.min.to_vec2()))
            } else if browser_focused {
                Some(egui::Event::PointerGone)
            } else {
                None
            }
        }
        egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers,
        } => {
            if rect.contains(*pos) || browser_focused {
                let local = (*pos - rect.min.to_vec2()).clamp(
                    egui::pos2(0.0, 0.0),
                    egui::pos2(rect.width(), rect.height()),
                );
                Some(egui::Event::PointerButton {
                    pos: local,
                    button: *button,
                    pressed: *pressed,
                    modifiers: *modifiers,
                })
            } else {
                None
            }
        }
        egui::Event::MouseWheel {
            unit,
            delta,
            modifiers,
        } => browser_focused.then_some(egui::Event::MouseWheel {
            unit: *unit,
            delta: *delta,
            modifiers: *modifiers,
        }),
        egui::Event::Key {
            key,
            physical_key,
            pressed,
            repeat,
            modifiers,
        } => browser_focused.then_some(egui::Event::Key {
            key: *key,
            physical_key: *physical_key,
            pressed: *pressed,
            repeat: *repeat,
            modifiers: *modifiers,
        }),
        egui::Event::Text(text) => browser_focused.then_some(egui::Event::Text(text.clone())),
        _ => None,
    }
}

/// The honest "page crashed" body: a danger caption naming the reason plus a
/// Reload that respawns the tab (setting `respawn_requested`). Never a fake page.
fn crashed_body(ui: &mut egui::Ui, reason: String, respawn_requested: &mut bool) {
    centered(ui, |ui| {
        ui.label(
            RichText::new("This page crashed")
                .size(Style::HEADING)
                .color(Style::DANGER),
        );
        ui.add_space(Style::SP_S);
        if !reason.is_empty() {
            muted_note(ui, reason);
        }
        ui.add_space(Style::SP_M);
        if ui
            .add(egui::Button::new(
                RichText::new("\u{21BB} Reload").color(Style::TEXT),
            ))
            .clicked()
        {
            *respawn_requested = true;
        }
    });
}

/// The no-session `EmptyState` — an honest gated caption (or the NAMED live-path
/// notice when one is passed), never a placeholder page.
fn empty_body(ui: &mut egui::Ui, notice: Option<&str>) {
    centered(ui, |ui| {
        ui.label(
            RichText::new("Sandboxed browser")
                .size(Style::HEADING)
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);
        muted_note(
            ui,
            notice.unwrap_or(
                "The sandboxed Servo browser renders here in the shell. A live session \
                 attaches on a GPU seat (BOOKMARKS-5/6 live path is gated).",
            ),
        );
    });
}

/// Center `content` vertically + horizontally in the remaining body.
fn centered(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.5 - Style::SP_XL);
        content(ui);
    });
}

/// The Browser surface's shared **MENUBAR-ALL** top bar (design: `menubar-all.md`).
///
/// The UPPERCASE `BROWSER` title in the Terminals-group accent, the real
/// [`WebSession`] menus, and a live status cluster — the one shared
/// [`mde_egui::menubar::MenuBar`] every surface embeds. **Page** carries the
/// address-bar's open seam; Edit / View / History / Bookmarks bind to the session
/// + page-actions seams the toolbar chrome already drives (§6 glue, no new
/// behaviour). Engine choice lives in the tab strip as explicit `+ Servo` and
/// `+ CEF` buttons. A context-gated item renders **disabled** and an absent
/// capability is **omitted** (§7): no page-text Copy, no keyboard chord table —
/// and the BROWSER-DD-8 **Power mode** is a named honest gate in View until its
/// toggle lands. The
/// status cluster shows the active engine, committed URL, session lifecycle,
/// http/https security state, and ad-filter shield (BOOKMARKS-7).
mod menubar {
    use super::{
        bookmark_add_body, chat_share_body, local_hostname, publish, publish_browser_send_tab,
        publish_browser_share, BrowserEngine, BrowserSendTabTarget, BrowserShareTarget,
        ContainerProfile, CupsPrintSettings, DisplayTarget, WebState, ACTION_BOOKMARKS_ADD,
        ACTION_CHAT_SEND, DEFAULT_DENIED_PERMISSIONS,
    };
    use mde_egui::egui;
    use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
    use mde_egui::{ChipTone, StatusChip, Style};
    use mde_web_preview_client::SessionState;

    /// The lock glyph a secure (https) page wears in the security chip.
    const LOCK: &str = "\u{1F512}";
    /// The open-lock glyph a plain (http) page wears in the security chip.
    const UNLOCK: &str = "\u{1F513}";
    /// The ad-filter shield glyph (matches the toolbar "N blocked" readout).
    const SHIELD: &str = "\u{2298}";
    /// The committed-URL chip truncates to this many characters so a long address
    /// never crowds the status cluster.
    const URL_MAX: usize = 42;

    fn print_options_active(settings: &CupsPrintSettings) -> bool {
        settings.destination.is_some()
            || settings.copies > 1
            || settings.duplex
            || settings.grayscale
    }

    /// One Browser menu action — each maps to a real [`WebSession`]/page seam in
    /// [`apply`]. `Copy`, so the menu model stays a plain value tree.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum MenuAction {
        /// Navigate back (`WebSession::go_back`).
        Back,
        /// Navigate forward (`WebSession::go_forward`).
        Forward,
        /// Reload the page, or respawn a crashed tab (`WebSession::reload` /
        /// `respawn_requested` — the exact toolbar Reload behaviour).
        Reload,
        /// Load the address-bar draft on the active tab (`WebSession::load` —
        /// the toolbar Go button's exact seam, MENU-3).
        OpenAddress,
        /// Toggle the BROWSER-DD-2 vertical tab layout.
        ToggleVerticalTabs,
        /// Toggle the browser download manager drawer.
        ToggleDownloads,
        /// Cycle the active tab through the built-in container profiles.
        CycleContainer,
        /// Cycle the active tab through display-placement targets.
        CycleDisplayTarget,
        /// Increase page zoom.
        ZoomIn,
        /// Decrease page zoom.
        ZoomOut,
        /// Reset page zoom to 100%.
        ResetZoom,
        /// Open the compact find-in-page bar.
        OpenFind,
        /// Toggle the active tab's audio mute state.
        ToggleAudioMute,
        /// Toggle forced dark styling for the active tab.
        ToggleForceDark,
        /// Toggle reader-mode styling for the active tab.
        ToggleReaderMode,
        /// Capture the latest painted browser viewport to a PNG file.
        CaptureViewport,
        /// Capture the current full helper surface to a PNG file.
        CaptureFullPage,
        /// Capture the current page metadata plus rendered frame as MHTML.
        CaptureMhtml,
        /// Capture the latest viewport with a visible metadata caption band.
        CaptureAnnotatedViewport,
        /// Capture the latest viewport with a visible callout annotation.
        CaptureCalloutViewport,
        /// Capture the latest viewport with a visible freehand stroke.
        CaptureFreehandViewport,
        /// Arm a drag-to-select region capture over the latest painted viewport.
        CaptureRegion,
        /// Print the active page.
        PrintPage,
        /// Toggle the CUPS destination/options drawer.
        TogglePrintSettings,
        /// Save the active page as a PDF.
        SavePdf,
        /// Open the last saved PDF in a CEF tab using Chromium's built-in viewer.
        OpenLastPdf,
        /// Reset the active tab's transient browser state to the new-tab surface.
        ClearCurrentTabData,
        /// Toggle the current first-party site's ad/tracker blocking policy.
        ToggleSiteBlocking,
        /// Forget the current site's permission decisions while preserving default-deny.
        ForgetSitePermissions,
        /// Copy the committed URL to the shell clipboard (the page-actions seam).
        CopyUrl,
        /// Bookmark the live page (`action/bookmarks/add`, BOOKMARKS-10).
        AddBookmark,
        /// Open the full Bookmarks manager surface.
        OpenBookmarksManager,
        /// Share the live page into Chat (`action/chat/send`, BOOKMARKS-10).
        SendInChat,
        /// Hand the live page to the platform peer-share owner.
        ShareToPeer,
        /// Hand the live page to the platform email owner.
        ShareToEmail,
        /// Hand the live page to the platform QR owner.
        ShareToQr,
        /// Hand the live tab to the session-sync owner for a target node.
        SendTabToNode,
        /// Hand the live tab to the phone bridge owner for a paired phone.
        SendTabToPhone,
    }

    /// A per-frame read-out of the active tab's live state — the single immutable
    /// borrow the menu model + status cluster are both built from, so the render is
    /// a pure function of it (unit-testable without a driven session).
    #[derive(Default)]
    #[allow(
        clippy::struct_excessive_bools,
        reason = "a flat read-out of the active tab's nav flags (can_back/can_forward/\
                  loading, mirroring NavState) plus has_tab/crashed and the address-bar \
                  draft flag — not a state machine"
    )]
    struct Snapshot {
        /// A tab is attached.
        has_tab: bool,
        /// Engine that owns the active tab, if any.
        active_engine: Option<BrowserEngine>,
        /// The active tab has crashed.
        crashed: bool,
        /// A back-history entry exists.
        can_back: bool,
        /// A forward-history entry exists.
        can_forward: bool,
        /// A load is in progress.
        loading: bool,
        /// The address bar holds a non-empty draft (gates Page → Open, MENU-3).
        typed_address: bool,
        /// Vertical tab chrome is enabled.
        vertical_tabs: bool,
        /// Active tab container identity.
        container: ContainerProfile,
        /// Active tab display-placement intent.
        display_target: DisplayTarget,
        /// Current page zoom percent.
        page_zoom_percent: u16,
        /// Find bar is open.
        find_open: bool,
        /// Download manager drawer is open.
        downloads_open: bool,
        /// Active browser-originated transfer count.
        active_downloads: usize,
        /// Total browser-originated transfer count.
        total_downloads: usize,
        /// Active tab audio is muted.
        audio_muted: bool,
        /// Active tab force-dark styling is enabled.
        force_dark: bool,
        /// Active tab reader-mode styling is enabled.
        reader_mode: bool,
        /// Active tab has a retained helper frame that can be captured.
        can_capture: bool,
        /// Drag-to-select region capture is armed.
        capture_region_mode: bool,
        /// CUPS print destination/options drawer is open.
        print_settings_open: bool,
        /// A non-default destination/options set is active.
        print_options_active: bool,
        /// A user save-as-PDF completed successfully and is still readable.
        has_saved_pdf: bool,
        /// The ad-filter blocked-request count for this page (BOOKMARKS-7).
        blocked: u32,
        /// The current first-party host, if the committed URL has one.
        current_site: Option<String>,
        /// Effective permission manager state for the current first-party host.
        current_site_permissions: Option<String>,
        /// Whether the native blocker is enabled for the current first-party host.
        site_blocking_enabled: bool,
        /// Safe-browsing mesh blocklist status.
        safe_browsing: String,
        /// Per-site data manager status.
        site_data: String,
        /// The committed URL.
        url: String,
        /// The session lifecycle, or `None` with no tab.
        state: Option<SessionState>,
    }

    impl Snapshot {
        /// Whether there is a live page (a non-crashed tab with a URL) to act on —
        /// the gate the page-family items (Copy URL / bookmark / share) share.
        fn has_page(&self) -> bool {
            self.has_tab && !self.crashed && !self.url.trim().is_empty()
        }
    }

    /// Read the active tab's live state into a [`Snapshot`] (one immutable borrow).
    fn snapshot(state: &WebState) -> Snapshot {
        let mut snap = state
            .tabs
            .get(state.active)
            .map_or_else(Snapshot::default, |tab| {
                let nav = tab.session.nav();
                let (active_downloads, total_downloads) = state.download_counts();
                Snapshot {
                    has_tab: true,
                    active_engine: Some(tab.engine),
                    crashed: tab.session.is_crashed(),
                    can_back: nav.can_back,
                    can_forward: nav.can_forward,
                    loading: nav.loading,
                    typed_address: false,
                    blocked: tab.session.blocked_count(),
                    current_site: state.active_first_party(),
                    current_site_permissions: state.active_site_permission_summary(),
                    site_blocking_enabled: state.active_site_blocking_enabled(),
                    safe_browsing: state.safe_browsing_summary(),
                    site_data: state.site_data_summary(),
                    url: nav.url.clone(),
                    state: Some(tab.session.state().clone()),
                    vertical_tabs: state.vertical_tabs,
                    container: tab.container,
                    display_target: tab.display_target,
                    page_zoom_percent: state.page_zoom_percent,
                    find_open: state.find_open,
                    downloads_open: state.downloads_open,
                    active_downloads,
                    total_downloads,
                    audio_muted: tab.muted,
                    force_dark: tab.force_dark,
                    reader_mode: tab.reader_mode,
                    can_capture: tab.last_frame.is_some(),
                    capture_region_mode: state.capture_region_mode,
                    print_settings_open: state.print_settings_open,
                    print_options_active: print_options_active(&state.cups_settings),
                    has_saved_pdf: state.last_saved_pdf.is_some(),
                }
            });
        let (active_downloads, total_downloads) = state.download_counts();
        snap.typed_address = !state.address.trim().is_empty();
        snap.vertical_tabs = state.vertical_tabs;
        snap.page_zoom_percent = state.page_zoom_percent;
        snap.find_open = state.find_open;
        snap.downloads_open = state.downloads_open;
        snap.capture_region_mode = state.capture_region_mode;
        snap.print_settings_open = state.print_settings_open;
        snap.print_options_active = print_options_active(&state.cups_settings);
        snap.has_saved_pdf = state.last_saved_pdf.is_some();
        snap.active_downloads = active_downloads;
        snap.total_downloads = total_downloads;
        snap.safe_browsing = state.safe_browsing_summary();
        snap.site_data = state.site_data_summary();
        snap
    }

    /// The Reload item's label — "Restart Page" on a crashed tab (it respawns),
    /// "Reload" otherwise (mirrors the toolbar tooltip).
    const fn reload_label(crashed: bool) -> &'static str {
        if crashed {
            "Restart Page"
        } else {
            "Reload"
        }
    }

    /// Build the Browser menus from live state (§6/§7): Page (the address-bar open
    /// seam), Edit (Copy URL), View (Reload, zoom, find, and the named BROWSER-DD-8
    /// Power-mode gate), History (Back/Forward, gated on the live history),
    /// Privacy, and Bookmarks (add plus share). New-tab engine choice is handled by
    /// the tab strip's explicit `+ Servo` and `+ CEF` buttons.
    fn build_menus(s: &Snapshot) -> Vec<Menu<MenuAction>> {
        let has_page = s.has_page();
        let can_tools = s.has_tab && !s.crashed;
        vec![
            Menu::new(
                "Page",
                vec![Entry::Item(
                    Item::new(MenuAction::OpenAddress, "Open Typed Address")
                        .shortcut("Enter")
                        .enabled(s.typed_address && s.has_tab && !s.crashed),
                )],
            ),
            Menu::new(
                "Edit",
                vec![Entry::Item(
                    Item::new(MenuAction::CopyUrl, "Copy URL").enabled(has_page),
                )],
            ),
            Menu::new(
                "View",
                vec![
                    Entry::Item(
                        Item::new(MenuAction::Reload, reload_label(s.crashed)).enabled(s.has_tab),
                    ),
                    Entry::Item(Item::new(
                        MenuAction::ToggleVerticalTabs,
                        if s.vertical_tabs {
                            "Horizontal Tabs"
                        } else {
                            "Vertical Tabs"
                        },
                    )),
                    Entry::Item(Item::new(
                        MenuAction::ToggleDownloads,
                        if s.downloads_open {
                            "Hide Downloads"
                        } else {
                            "Show Downloads"
                        },
                    )),
                    Entry::Item(
                        Item::new(
                            MenuAction::CycleContainer,
                            format!("Container: {}", s.container.label()),
                        )
                        .enabled(can_tools),
                    ),
                    Entry::Item(
                        Item::new(
                            MenuAction::CycleDisplayTarget,
                            format!("Display Target: {}", s.display_target.label()),
                        )
                        .enabled(can_tools),
                    ),
                    Entry::Separator,
                    Entry::Item(
                        Item::new(MenuAction::ZoomIn, "Zoom In")
                            .shortcut("Ctrl++")
                            .enabled(can_tools && s.page_zoom_percent < super::PAGE_ZOOM_MAX),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::ZoomOut, "Zoom Out")
                            .shortcut("Ctrl+-")
                            .enabled(can_tools && s.page_zoom_percent > super::PAGE_ZOOM_MIN),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::ResetZoom, "Actual Size")
                            .shortcut("Ctrl+0")
                            .enabled(can_tools && s.page_zoom_percent != 100),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::OpenFind, "Find in Page")
                            .shortcut("Ctrl+F")
                            .enabled(can_tools),
                    ),
                    Entry::Item(
                        Item::new(
                            MenuAction::ToggleAudioMute,
                            if s.audio_muted {
                                "Unmute Tab"
                            } else {
                                "Mute Tab"
                            },
                        )
                        .enabled(can_tools),
                    ),
                    Entry::Item(
                        Item::new(
                            MenuAction::ToggleForceDark,
                            if s.force_dark {
                                "Disable Force Dark"
                            } else {
                                "Enable Force Dark"
                            },
                        )
                        .enabled(can_tools),
                    ),
                    Entry::Item(
                        Item::new(
                            MenuAction::ToggleReaderMode,
                            if s.reader_mode {
                                "Disable Reader Mode"
                            } else {
                                "Enable Reader Mode"
                            },
                        )
                        .enabled(can_tools),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::CaptureViewport, "Capture Viewport")
                            .enabled(can_tools && s.can_capture),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::CaptureFullPage, "Capture Full Page")
                            .enabled(can_tools && s.can_capture),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::CaptureMhtml, "Capture MHTML")
                            .enabled(can_tools && s.can_capture),
                    ),
                    Entry::Item(
                        Item::new(
                            MenuAction::CaptureAnnotatedViewport,
                            "Capture with Annotation",
                        )
                        .enabled(can_tools && s.can_capture),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::CaptureCalloutViewport, "Capture with Callout")
                            .enabled(can_tools && s.can_capture),
                    ),
                    Entry::Item(
                        Item::new(
                            MenuAction::CaptureFreehandViewport,
                            "Capture Freehand Markup",
                        )
                        .enabled(can_tools && s.can_capture),
                    ),
                    Entry::Item(
                        Item::new(
                            MenuAction::CaptureRegion,
                            if s.capture_region_mode {
                                "Cancel Region Capture"
                            } else {
                                "Capture Region"
                            },
                        )
                        .enabled(can_tools && s.can_capture),
                    ),
                    Entry::Item(Item::new(MenuAction::PrintPage, "Print Page").enabled(can_tools)),
                    Entry::Item(
                        Item::new(
                            MenuAction::TogglePrintSettings,
                            if s.print_settings_open {
                                "Hide Print Settings"
                            } else {
                                "Print Settings"
                            },
                        )
                        .enabled(can_tools),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::SavePdf, "Save Page as PDF").enabled(can_tools),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::OpenLastPdf, "Open Last PDF")
                            .enabled(s.has_saved_pdf),
                    ),
                    Entry::Separator,
                    Entry::Caption(
                        "Power mode \u{2014} not yet available (BROWSER-DD-8: the dev/power \
                         toolset lands behind one toggle)."
                            .to_owned(),
                    ),
                ],
            ),
            Menu::new(
                "History",
                vec![
                    Entry::Item(
                        Item::new(MenuAction::Back, "Back")
                            .enabled(s.has_tab && !s.crashed && s.can_back),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::Forward, "Forward")
                            .enabled(s.has_tab && !s.crashed && s.can_forward),
                    ),
                ],
            ),
            Menu::new("Privacy", {
                let mut entries = vec![
                    Entry::Caption("Cookies: blocked by the sandboxed engine".to_owned()),
                    Entry::Caption("Third-party cookies: blocked (no cookie store)".to_owned()),
                    Entry::Caption("Session data: cleared on tab close".to_owned()),
                    Entry::Caption(s.site_data.clone()),
                    Entry::Caption("Filter lists: bundled seed + synced/custom rules".to_owned()),
                    Entry::Caption(s.safe_browsing.clone()),
                    Entry::Caption(format!(
                        "Permissions: default deny ({DEFAULT_DENIED_PERMISSIONS})"
                    )),
                    Entry::Separator,
                    Entry::Item(
                        Item::new(
                            MenuAction::ToggleSiteBlocking,
                            if s.site_blocking_enabled {
                                "Disable Blocking for This Site"
                            } else {
                                "Enable Blocking for This Site"
                            },
                        )
                        .enabled(s.current_site.is_some() && s.has_tab && !s.crashed),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::ForgetSitePermissions, "Forget Site Permissions")
                            .enabled(s.current_site.is_some() && s.has_tab && !s.crashed),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::ClearCurrentTabData, "Clear Current Tab Data")
                            .enabled(s.has_tab && !s.crashed),
                    ),
                ];
                if let Some(site) = &s.current_site {
                    entries.insert(4, Entry::Caption(format!("This site: {site}")));
                }
                if let Some(summary) = &s.current_site_permissions {
                    entries.insert(5, Entry::Caption(format!("Site permissions: {summary}")));
                }
                entries
            }),
            Menu::new(
                "Bookmarks",
                vec![
                    Entry::Item(Item::new(
                        MenuAction::OpenBookmarksManager,
                        "Open Bookmarks Manager",
                    )),
                    Entry::Separator,
                    Entry::Item(
                        Item::new(MenuAction::AddBookmark, "Add Bookmark").enabled(has_page),
                    ),
                    Entry::Separator,
                    Entry::Item(
                        Item::new(MenuAction::SendInChat, "Send in Chat").enabled(has_page),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::ShareToPeer, "Share to Peer").enabled(has_page),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::ShareToEmail, "Share to Email").enabled(has_page),
                    ),
                    Entry::Item(Item::new(MenuAction::ShareToQr, "Share as QR").enabled(has_page)),
                    Entry::Separator,
                    Entry::Item(
                        Item::new(MenuAction::SendTabToNode, "Send Tab to Node").enabled(has_page),
                    ),
                    Entry::Item(
                        Item::new(MenuAction::SendTabToPhone, "Send Tab to Phone")
                            .enabled(has_page),
                    ),
                ],
            ),
        ]
    }

    /// The lifecycle status chip: Loading (a load in flight or the pre-first-frame
    /// state), Live (a painted, settled page), Crashed, or an idle "No session"
    /// with no tab.
    fn state_chip(s: &Snapshot) -> StatusChip {
        match &s.state {
            None => StatusChip::new("No session", ChipTone::Neutral),
            Some(SessionState::Crashed { .. }) => StatusChip::new("Crashed", ChipTone::Danger),
            Some(SessionState::Loading) => StatusChip::new("Loading", ChipTone::Info),
            Some(SessionState::Live) => {
                if s.loading {
                    StatusChip::new("Loading", ChipTone::Info)
                } else {
                    StatusChip::new("Live", ChipTone::Ok)
                }
            }
        }
    }

    /// The http/https security chip for the committed URL — a lock (Ok) for https,
    /// an open lock (Warn) for http, or `None` for a schemeless address
    /// (`about:blank`, empty) with no security state to report.
    fn security_chip(s: &Snapshot) -> Option<StatusChip> {
        if !s.has_tab {
            return None;
        }
        let url = s.url.trim();
        if url.starts_with("https://") {
            Some(StatusChip::with_icon(LOCK, "https", ChipTone::Ok))
        } else if url.starts_with("http://") {
            Some(StatusChip::with_icon(UNLOCK, "http", ChipTone::Warn))
        } else {
            None
        }
    }

    /// Truncate a URL to [`URL_MAX`] characters (an ellipsis tail) so the chip
    /// stays compact; a short URL is verbatim.
    fn truncate_url(url: &str) -> String {
        let url = url.trim();
        if url.chars().count() <= URL_MAX {
            return url.to_owned();
        }
        let head: String = url.chars().take(URL_MAX - 1).collect();
        format!("{head}\u{2026}")
    }

    /// Build the live status cluster: the active engine (MENU-3 — every live
    /// session is the sandboxed Servo helper today, so the chip appears only when
    /// a tab actually runs one, §7), the committed URL, the lifecycle state, the
    /// http/https security state, and the ad-filter shield (a `0` count stays
    /// hidden, §7).
    fn build_status(s: &Snapshot) -> Vec<StatusChip> {
        let mut chips = Vec::new();
        if let Some(engine) = s.active_engine {
            chips.push(StatusChip::new(engine.label(), ChipTone::Info));
        }
        if s.has_tab && !s.url.trim().is_empty() {
            chips.push(StatusChip::new(truncate_url(&s.url), ChipTone::Neutral));
        }
        if s.has_tab && s.page_zoom_percent != 100 {
            chips.push(StatusChip::new(
                format!("{}%", s.page_zoom_percent),
                ChipTone::Neutral,
            ));
        }
        if s.has_tab {
            let container = s.container.chip();
            if !container.is_empty() {
                chips.push(StatusChip::new(container, ChipTone::Info));
            }
            let display = s.display_target.chip();
            if !display.is_empty() {
                chips.push(StatusChip::new(display, ChipTone::Neutral));
            }
        }
        if s.has_tab && s.find_open {
            chips.push(StatusChip::new("Find", ChipTone::Info));
        }
        if s.active_downloads > 0 {
            chips.push(StatusChip::with_icon(
                "\u{2193}",
                s.active_downloads.to_string(),
                ChipTone::Info,
            ));
        } else if s.downloads_open && s.total_downloads > 0 {
            chips.push(StatusChip::with_icon(
                "\u{2193}",
                s.total_downloads.to_string(),
                ChipTone::Neutral,
            ));
        }
        if s.has_tab && s.audio_muted {
            chips.push(StatusChip::new("Muted", ChipTone::Warn));
        }
        if s.has_tab && s.force_dark {
            chips.push(StatusChip::new("Dark", ChipTone::Info));
        }
        if s.has_tab && s.reader_mode {
            chips.push(StatusChip::new("Reader", ChipTone::Info));
        }
        if s.print_settings_open || s.print_options_active {
            chips.push(StatusChip::new("Print", ChipTone::Neutral));
        }
        chips.push(state_chip(s));
        if let Some(chip) = security_chip(s) {
            chips.push(chip);
        }
        if s.blocked > 0 {
            chips.push(StatusChip::with_icon(
                SHIELD,
                s.blocked.to_string(),
                ChipTone::Warn,
            ));
        }
        chips
    }

    /// Render the BROWSER bar and return the action the operator picked this frame.
    pub(super) fn show(state: &WebState, ui: &mut egui::Ui) -> Option<MenuAction> {
        let snap = snapshot(state);
        let menus = build_menus(&snap);
        let status = build_status(&snap);
        let model = MenuBarModel {
            // The dock groups Browser under **Terminals** (teal), so the title
            // wears that categorical accent (lock 2).
            title: "Browser",
            accent: Style::ACCENT_TERMINALS,
            menus: &menus,
            status: &status,
        };
        MenuBar::show(ui, &model)
    }

    /// The active tab's committed URL, or empty with no tab.
    fn page_url(state: &WebState) -> String {
        state
            .tabs
            .get(state.active)
            .map(|t| t.session.nav().url.clone())
            .unwrap_or_default()
    }

    /// The active tab's committed URL + title, or empties with no tab.
    fn page_url_title(state: &WebState) -> (String, String) {
        state.tabs.get(state.active).map_or_else(
            || (String::new(), String::new()),
            |t| (t.session.nav().url.clone(), t.session.title().to_owned()),
        )
    }

    /// Dispatch a picked action to its real seam (§6, no new behaviour) — the SAME
    /// seams the toolbar chrome + page-actions menu already drive.
    pub(super) fn apply(ctx: &egui::Context, state: &mut WebState, action: MenuAction) {
        match action {
            MenuAction::Back => {
                if let Some(tab) = state.active_tab() {
                    tab.session.go_back();
                }
            }
            MenuAction::Forward => {
                if let Some(tab) = state.active_tab() {
                    tab.session.go_forward();
                }
            }
            MenuAction::Reload => {
                let crashed = state
                    .tabs
                    .get(state.active)
                    .is_some_and(|t| t.session.is_crashed());
                if crashed {
                    state.respawn_requested = true;
                } else if let Some(tab) = state.active_tab() {
                    tab.session.reload();
                }
            }
            MenuAction::OpenAddress => {
                // The toolbar Go button's exact seam (MENU-3): load the address
                // draft on a live (non-crashed) active tab, including the
                // HTTPS-only prompt for explicit http:// targets.
                state.submit_address();
            }
            MenuAction::ToggleVerticalTabs => state.toggle_vertical_tabs(),
            MenuAction::ToggleDownloads => {
                state.downloads_open = !state.downloads_open;
                if state.downloads_open {
                    state.refresh_downloads();
                }
            }
            MenuAction::CycleContainer => state.cycle_active_tab_container(),
            MenuAction::CycleDisplayTarget => state.cycle_active_tab_display_target(),
            MenuAction::ZoomIn => state.zoom_in(),
            MenuAction::ZoomOut => state.zoom_out(),
            MenuAction::ResetZoom => state.reset_zoom(),
            MenuAction::OpenFind => state.open_find_bar(),
            MenuAction::ToggleAudioMute => state.toggle_active_tab_mute(),
            MenuAction::ToggleForceDark => state.toggle_active_tab_force_dark(),
            MenuAction::ToggleReaderMode => state.toggle_active_tab_reader_mode(),
            MenuAction::CaptureViewport => state.capture_active_viewport(),
            MenuAction::CaptureFullPage => state.capture_active_full_page(),
            MenuAction::CaptureMhtml => state.capture_active_mhtml(),
            MenuAction::CaptureAnnotatedViewport => state.capture_active_annotated_viewport(),
            MenuAction::CaptureCalloutViewport => state.capture_active_callout_viewport(),
            MenuAction::CaptureFreehandViewport => state.capture_active_freehand_viewport(),
            MenuAction::CaptureRegion => {
                if state.capture_region_mode {
                    state.cancel_region_capture();
                } else {
                    state.start_region_capture();
                }
            }
            MenuAction::PrintPage => state.print_active_page(),
            MenuAction::TogglePrintSettings => state.toggle_print_settings(),
            MenuAction::SavePdf => state.save_active_page_pdf(),
            MenuAction::OpenLastPdf => state.open_last_saved_pdf(),
            MenuAction::ClearCurrentTabData => state.clear_active_session_data(),
            MenuAction::ToggleSiteBlocking => {
                let enabled = !state.active_site_blocking_enabled();
                state.set_active_site_blocking(enabled);
            }
            MenuAction::ForgetSitePermissions => state.forget_active_site_permissions(),
            MenuAction::CopyUrl => {
                let url = page_url(state);
                if !url.trim().is_empty() {
                    ctx.copy_text(url);
                }
            }
            MenuAction::AddBookmark => {
                let (url, title) = page_url_title(state);
                if !url.trim().is_empty() {
                    publish(ACTION_BOOKMARKS_ADD, &bookmark_add_body(&url, &title));
                }
            }
            MenuAction::OpenBookmarksManager => state.request_bookmarks_manager(),
            MenuAction::SendInChat => {
                let (url, title) = page_url_title(state);
                if !url.trim().is_empty() {
                    publish(
                        ACTION_CHAT_SEND,
                        &chat_share_body(&local_hostname(), &url, &title),
                    );
                }
            }
            MenuAction::ShareToPeer => {
                let (url, title) = page_url_title(state);
                if !url.trim().is_empty() {
                    publish_browser_share(
                        state.bus_root.as_deref(),
                        BrowserShareTarget::Peer,
                        &url,
                        &title,
                    );
                }
            }
            MenuAction::ShareToEmail => {
                let (url, title) = page_url_title(state);
                if !url.trim().is_empty() {
                    publish_browser_share(
                        state.bus_root.as_deref(),
                        BrowserShareTarget::Email,
                        &url,
                        &title,
                    );
                }
            }
            MenuAction::ShareToQr => {
                let (url, title) = page_url_title(state);
                if !url.trim().is_empty() {
                    publish_browser_share(
                        state.bus_root.as_deref(),
                        BrowserShareTarget::Qr,
                        &url,
                        &title,
                    );
                }
            }
            MenuAction::SendTabToNode => {
                let Some(engine) = state.tabs.get(state.active).map(|tab| tab.engine) else {
                    return;
                };
                let (url, title) = page_url_title(state);
                if !url.trim().is_empty() {
                    publish_browser_send_tab(
                        state.bus_root.as_deref(),
                        BrowserSendTabTarget::Node,
                        engine,
                        &url,
                        &title,
                    );
                }
            }
            MenuAction::SendTabToPhone => {
                let Some(engine) = state.tabs.get(state.active).map(|tab| tab.engine) else {
                    return;
                };
                let (url, title) = page_url_title(state);
                if !url.trim().is_empty() {
                    publish_browser_send_tab(
                        state.bus_root.as_deref(),
                        BrowserSendTabTarget::Phone,
                        engine,
                        &url,
                        &title,
                    );
                }
            }
        }
    }

    #[cfg(test)]
    #[allow(
        clippy::panic,
        reason = "a let-else in a model test names the expected menu shape (house style, \
                  mirroring the shared menubar.rs tests)"
    )]
    mod tests {
        use super::{
            apply, build_menus, build_status, reload_label, security_chip, show, state_chip,
            truncate_url, BrowserEngine, ContainerProfile, DisplayTarget, MenuAction, Snapshot,
            WebState, URL_MAX,
        };
        use mde_egui::egui;
        use mde_egui::menubar::Entry;
        use mde_egui::{ChipTone, Style};
        use mde_web_preview_client::SessionState;

        /// A live, navigable https page (a non-crashed tab, one back entry, three
        /// blocked requests) — the model tests read their gating off it.
        fn https_page() -> Snapshot {
            Snapshot {
                has_tab: true,
                active_engine: Some(BrowserEngine::Servo),
                crashed: false,
                can_back: true,
                can_forward: false,
                loading: false,
                typed_address: false,
                vertical_tabs: false,
                container: ContainerProfile::None,
                display_target: DisplayTarget::Current,
                page_zoom_percent: 100,
                find_open: false,
                downloads_open: false,
                active_downloads: 0,
                total_downloads: 0,
                audio_muted: false,
                force_dark: false,
                reader_mode: false,
                can_capture: true,
                capture_region_mode: false,
                print_settings_open: false,
                print_options_active: false,
                has_saved_pdf: false,
                blocked: 3,
                current_site: Some("example.com".to_owned()),
                current_site_permissions: Some(
                    "example.com: all sensitive prompts denied by default".to_owned(),
                ),
                site_blocking_enabled: true,
                safe_browsing: "Safe browsing: 2 mesh-hosted unsafe hosts loaded".to_owned(),
                site_data: "Site data: 1 tracked site · 1 open tab · example.com cleared 0 times"
                    .to_owned(),
                url: "https://example.com/path".to_owned(),
                state: Some(SessionState::Live),
            }
        }

        #[test]
        fn the_menus_cover_the_real_browser_seams() {
            let menus = build_menus(&https_page());
            let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
            assert_eq!(
                titles,
                ["Page", "Edit", "View", "History", "Privacy", "Bookmarks"]
            );
            // Engine selection is not hidden in a menu; the tab strip exposes
            // explicit + Servo and + CEF new-tab buttons. File/Help are also
            // honestly omitted rather than present-but-dead menus (§7).
            assert!(!titles.contains(&"Engine"));
            assert!(!titles.contains(&"File"));
            assert!(!titles.contains(&"Help"));
        }

        #[test]
        fn open_typed_address_gates_on_a_draft_and_a_live_tab() {
            // A typed draft + a live tab → enabled; no draft, or a crashed tab,
            // or no tab → the honest disable (§7).
            let ready = Snapshot {
                typed_address: true,
                ..https_page()
            };
            let open = |menus: Vec<mde_egui::menubar::Menu<MenuAction>>| {
                menus
                    .into_iter()
                    .find(|m| m.title == "Page")
                    .and_then(|m| {
                        m.entries.into_iter().find_map(|e| match e {
                            Entry::Item(i) if i.id == MenuAction::OpenAddress => Some(i.enabled),
                            _ => None,
                        })
                    })
                    .expect("Page → Open Typed Address is present")
            };
            assert!(open(build_menus(&ready)), "draft + live tab enables");
            assert!(!open(build_menus(&https_page())), "no draft disables");
            let crashed = Snapshot {
                typed_address: true,
                crashed: true,
                ..https_page()
            };
            assert!(!open(build_menus(&crashed)), "a crashed tab disables");
            let no_tab = Snapshot {
                typed_address: true,
                ..Snapshot::default()
            };
            assert!(!open(build_menus(&no_tab)), "no tab disables");
        }

        #[test]
        fn the_view_menu_names_the_power_mode_gate() {
            let view = build_menus(&https_page())
                .into_iter()
                .find(|m| m.title == "View")
                .expect("View menu present");
            assert!(
                view.entries.iter().any(|e| matches!(
                    e,
                    Entry::Caption(c) if c.contains("Power mode") && c.contains("BROWSER-DD-8")
                )),
                "the BROWSER-DD-8 Power-mode gate is a named honest caption"
            );
        }

        #[test]
        fn the_view_menu_exposes_real_zoom_and_find_actions() {
            let view = build_menus(&https_page())
                .into_iter()
                .find(|m| m.title == "View")
                .expect("View menu present");
            let item = |id: MenuAction| {
                view.entries
                    .iter()
                    .find_map(|e| match e {
                        Entry::Item(i) if i.id == id => Some(i),
                        _ => None,
                    })
                    .expect("view item present")
            };
            assert!(item(MenuAction::ZoomIn).enabled);
            assert!(item(MenuAction::ZoomOut).enabled);
            assert!(
                !item(MenuAction::ResetZoom).enabled,
                "100% zoom has nothing to reset"
            );
            assert!(item(MenuAction::OpenFind).enabled);
            assert_eq!(item(MenuAction::ToggleDownloads).label, "Show Downloads");
            assert!(item(MenuAction::ToggleDownloads).enabled);
            assert_eq!(item(MenuAction::ToggleAudioMute).label, "Mute Tab");
            assert!(item(MenuAction::ToggleAudioMute).enabled);
            assert_eq!(item(MenuAction::ToggleForceDark).label, "Enable Force Dark");
            assert!(item(MenuAction::ToggleForceDark).enabled);
            assert_eq!(
                item(MenuAction::ToggleReaderMode).label,
                "Enable Reader Mode"
            );
            assert!(item(MenuAction::ToggleReaderMode).enabled);
            assert!(item(MenuAction::CaptureViewport).enabled);
            assert!(item(MenuAction::CaptureFullPage).enabled);
            assert!(item(MenuAction::CaptureMhtml).enabled);
            assert!(item(MenuAction::CaptureAnnotatedViewport).enabled);
            assert!(item(MenuAction::CaptureCalloutViewport).enabled);
            assert!(item(MenuAction::CaptureFreehandViewport).enabled);
            assert!(item(MenuAction::CaptureRegion).enabled);
            assert!(item(MenuAction::PrintPage).enabled);
            assert_eq!(
                item(MenuAction::TogglePrintSettings).label,
                "Print Settings"
            );
            assert!(item(MenuAction::TogglePrintSettings).enabled);
            assert!(item(MenuAction::SavePdf).enabled);
            assert!(!item(MenuAction::OpenLastPdf).enabled);
            assert_eq!(
                item(MenuAction::CycleContainer).label,
                "Container: No Container"
            );
            assert!(item(MenuAction::CycleContainer).enabled);
            assert_eq!(
                item(MenuAction::CycleDisplayTarget).label,
                "Display Target: Current Display"
            );
            assert!(item(MenuAction::CycleDisplayTarget).enabled);

            let zoomed = Snapshot {
                container: ContainerProfile::Work,
                display_target: DisplayTarget::Secondary,
                page_zoom_percent: 150,
                find_open: true,
                downloads_open: true,
                active_downloads: 2,
                total_downloads: 3,
                audio_muted: true,
                force_dark: true,
                reader_mode: true,
                ..https_page()
            };
            let texts = build_status(&zoomed)
                .into_iter()
                .map(|c| c.text)
                .collect::<Vec<_>>();
            assert!(texts.contains(&"150%".to_owned()));
            assert!(texts.contains(&"Work".to_owned()));
            assert!(texts.contains(&"Display 2".to_owned()));
            assert!(texts.contains(&"Find".to_owned()));
            assert!(texts.contains(&"2".to_owned()));
            assert!(texts.contains(&"Muted".to_owned()));
            assert!(texts.contains(&"Dark".to_owned()));
            assert!(texts.contains(&"Reader".to_owned()));

            let muted = Snapshot {
                audio_muted: true,
                force_dark: true,
                reader_mode: true,
                ..https_page()
            };
            let view = build_menus(&muted)
                .into_iter()
                .find(|m| m.title == "View")
                .expect("View menu present");
            let unmute = view
                .entries
                .iter()
                .find_map(|e| match e {
                    Entry::Item(i) if i.id == MenuAction::ToggleAudioMute => Some(i),
                    _ => None,
                })
                .expect("mute item present");
            assert_eq!(unmute.label, "Unmute Tab");
            let disable_dark = view
                .entries
                .iter()
                .find_map(|e| match e {
                    Entry::Item(i) if i.id == MenuAction::ToggleForceDark => Some(i),
                    _ => None,
                })
                .expect("force-dark item present");
            assert_eq!(disable_dark.label, "Disable Force Dark");
            let disable_reader = view
                .entries
                .iter()
                .find_map(|e| match e {
                    Entry::Item(i) if i.id == MenuAction::ToggleReaderMode => Some(i),
                    _ => None,
                })
                .expect("reader item present");
            assert_eq!(disable_reader.label, "Disable Reader Mode");
        }

        #[test]
        fn the_engine_chip_reads_the_live_helper() {
            // A tab runs the sandboxed Servo helper → the engine chip shows; with
            // no session there is no engine to claim (§7).
            let chips = build_status(&https_page());
            assert_eq!(chips[0].text, "Servo", "the engine chip leads the cluster");
            assert_eq!(chips[0].tone, ChipTone::Info);
            assert!(
                !build_status(&Snapshot::default())
                    .iter()
                    .any(|c| c.text == "Servo"),
                "no tab ⇒ no engine chip"
            );
        }

        #[test]
        fn active_engine_chip_does_not_depend_on_future_tab_default() {
            let snap = Snapshot {
                active_engine: Some(BrowserEngine::Servo),
                ..https_page()
            };
            let chips = build_status(&snap);
            assert_eq!(
                chips[0].text, "Servo",
                "the status chip reads the actual active tab engine"
            );
            assert!(
                build_menus(&snap).iter().all(|m| m.title != "Engine"),
                "future-tab engine choice lives in the tab strip, not a menu"
            );
        }

        #[test]
        fn back_and_forward_gate_on_the_live_history() {
            // can_back = true, can_forward = false → Back enabled, Forward greyed —
            // the §7 disable, never an omission.
            let history = build_menus(&https_page())
                .into_iter()
                .find(|m| m.title == "History")
                .expect("History menu present");
            let items: Vec<(String, bool)> = history
                .entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Item(i) => Some((i.label.clone(), i.enabled)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                items,
                [("Back".to_owned(), true), ("Forward".to_owned(), false)]
            );
        }

        #[test]
        fn the_page_family_items_disable_without_a_live_page() {
            // No tab → page/session items grey (Copy URL / Reload / Back / Forward /
            // Add Bookmark / Send in Chat / Share), while the pure chrome layout
            // toggle remains usable.
            let menus = build_menus(&Snapshot::default());
            for menu in &menus {
                for entry in &menu.entries {
                    if let Entry::Item(item) = entry {
                        assert_eq!(
                            item.enabled,
                            matches!(
                                item.label.as_str(),
                                "Vertical Tabs" | "Show Downloads" | "Open Bookmarks Manager"
                            ),
                            "{} has the expected no-page gate",
                            item.label
                        );
                    }
                }
            }
        }

        #[test]
        fn the_bookmarks_menu_exposes_platform_share_handoffs() {
            let bookmarks = build_menus(&https_page())
                .into_iter()
                .find(|m| m.title == "Bookmarks")
                .expect("Bookmarks menu present");
            let items: Vec<(&str, bool)> = bookmarks
                .entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Item(i) => Some((i.label.as_str(), i.enabled)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                items,
                [
                    ("Open Bookmarks Manager", true),
                    ("Add Bookmark", true),
                    ("Send in Chat", true),
                    ("Share to Peer", true),
                    ("Share to Email", true),
                    ("Share as QR", true),
                    ("Send Tab to Node", true),
                    ("Send Tab to Phone", true),
                ]
            );
        }

        #[test]
        fn the_privacy_menu_exposes_the_enforced_cookie_policy() {
            let menus = build_menus(&https_page());
            let privacy = menus
                .into_iter()
                .find(|m| m.title == "Privacy")
                .expect("Privacy menu present");
            let captions: Vec<&str> = privacy
                .entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Caption(c) => Some(c.as_str()),
                    _ => None,
                })
                .collect();
            assert!(
                captions.iter().any(|c| c.contains("Cookies: blocked")),
                "cookie store policy is visible"
            );
            assert!(
                captions
                    .iter()
                    .any(|c| c.contains("Third-party cookies: blocked")),
                "third-party cookie policy is visible"
            );
            assert!(
                captions
                    .iter()
                    .any(|c| c.contains("Session data: cleared on tab close")),
                "clear-on-close policy is visible"
            );
            assert!(
                captions
                    .iter()
                    .any(|c| c.contains("Site data: 1 tracked site")),
                "the per-site data manager summary is visible"
            );
            assert!(
                captions
                    .iter()
                    .any(|c| c.contains("Filter lists: bundled seed + synced/custom rules")),
                "the filter-list policy source is visible"
            );
            assert!(
                captions
                    .iter()
                    .any(|c| c.contains("Safe browsing: 2 mesh-hosted unsafe hosts loaded")),
                "the safe-browsing mesh blocklist status is visible"
            );
            assert!(
                captions
                    .iter()
                    .any(|c| c.contains("Permissions: default deny")),
                "the default-deny permission manager policy is visible"
            );
            assert!(
                captions
                    .iter()
                    .any(|c| c.contains("This site: example.com")),
                "the current first-party site is visible"
            );
            assert!(
                captions
                    .iter()
                    .any(|c| c.contains("Site permissions: example.com")),
                "the current site's effective permission policy is visible"
            );
            let site_toggle = privacy
                .entries
                .iter()
                .find_map(|e| match e {
                    Entry::Item(i) if i.id == MenuAction::ToggleSiteBlocking => Some(i),
                    _ => None,
                })
                .expect("site-blocking item present");
            assert!(
                site_toggle.enabled,
                "live pages can toggle per-site blocking"
            );
            assert_eq!(site_toggle.label, "Disable Blocking for This Site");
            let forget = privacy
                .entries
                .iter()
                .find_map(|e| match e {
                    Entry::Item(i) if i.id == MenuAction::ForgetSitePermissions => Some(i),
                    _ => None,
                })
                .expect("permission manager item present");
            assert!(
                forget.enabled,
                "live pages can forget current-site permission decisions"
            );
            let clear = privacy
                .entries
                .iter()
                .find_map(|e| match e {
                    Entry::Item(i) if i.id == MenuAction::ClearCurrentTabData => Some(i),
                    _ => None,
                })
                .expect("clear item present");
            assert!(clear.enabled, "live non-crashed tabs can be cleared");
        }

        #[test]
        fn the_privacy_menu_reenables_blocking_for_an_allowlisted_site() {
            let snap = Snapshot {
                site_blocking_enabled: false,
                ..https_page()
            };
            let privacy = build_menus(&snap)
                .into_iter()
                .find(|m| m.title == "Privacy")
                .expect("Privacy menu present");
            let toggle = privacy
                .entries
                .iter()
                .find_map(|e| match e {
                    Entry::Item(i) if i.id == MenuAction::ToggleSiteBlocking => Some(i),
                    _ => None,
                })
                .expect("site toggle present");
            assert_eq!(toggle.label, "Enable Blocking for This Site");
            assert!(toggle.enabled);
        }

        #[test]
        fn reload_relabels_to_restart_and_stays_enabled_on_a_crashed_tab() {
            assert_eq!(reload_label(false), "Reload");
            assert_eq!(reload_label(true), "Restart Page");
            let crashed = Snapshot {
                has_tab: true,
                crashed: true,
                ..Snapshot::default()
            };
            let view = build_menus(&crashed)
                .into_iter()
                .find(|m| m.title == "View")
                .expect("View menu present");
            let Entry::Item(reload) = &view.entries[0] else {
                panic!("View[0] is Reload");
            };
            assert_eq!(reload.label, "Restart Page");
            assert!(
                reload.enabled,
                "Reload stays enabled on a crashed tab (it respawns)"
            );
        }

        #[test]
        fn the_status_cluster_reflects_the_live_page() {
            let chips = build_status(&https_page());
            let texts: Vec<&str> = chips.iter().map(|c| c.text.as_str()).collect();
            // Engine · URL · Live · https · 3 blocked, left→right (MENU-3 leads
            // with the active engine).
            assert_eq!(texts[0], "Servo");
            assert_eq!(texts[1], "https://example.com/path");
            assert!(texts.contains(&"Live"), "the lifecycle chip is present");
            assert!(texts.contains(&"https"), "the security chip is present");
            assert!(texts.contains(&"3"), "the ad-filter shield shows the count");
        }

        #[test]
        fn the_state_chip_tracks_the_session_lifecycle() {
            let live = state_chip(&https_page());
            assert_eq!(live.text, "Live");
            assert_eq!(live.tone, ChipTone::Ok);
            let loading = Snapshot {
                has_tab: true,
                loading: true,
                state: Some(SessionState::Live),
                ..Snapshot::default()
            };
            assert_eq!(state_chip(&loading).text, "Loading");
            let crashed = Snapshot {
                has_tab: true,
                state: Some(SessionState::Crashed {
                    reason: "boom".to_owned(),
                }),
                ..Snapshot::default()
            };
            assert_eq!(state_chip(&crashed).tone, ChipTone::Danger);
            // No tab → an honest neutral idle chip, never a fake "Live".
            assert_eq!(state_chip(&Snapshot::default()).tone, ChipTone::Neutral);
        }

        #[test]
        fn the_security_chip_reads_the_url_scheme() {
            assert_eq!(
                security_chip(&https_page()).expect("https chip").tone,
                ChipTone::Ok
            );
            let http = Snapshot {
                has_tab: true,
                url: "http://plain.example/".to_owned(),
                state: Some(SessionState::Live),
                ..Snapshot::default()
            };
            assert_eq!(
                security_chip(&http).expect("http chip").tone,
                ChipTone::Warn
            );
            // A schemeless address (about:blank) and no-tab both omit the chip.
            let blank = Snapshot {
                has_tab: true,
                url: "about:blank".to_owned(),
                state: Some(SessionState::Live),
                ..Snapshot::default()
            };
            assert!(security_chip(&blank).is_none());
            assert!(security_chip(&Snapshot::default()).is_none());
        }

        #[test]
        fn a_long_url_truncates_but_a_short_one_is_verbatim() {
            assert_eq!(truncate_url("https://a.co/"), "https://a.co/");
            let long = "https://example.com/a/very/long/path/that/keeps/going/and/going/on";
            let out = truncate_url(long);
            assert!(out.chars().count() <= URL_MAX, "truncated within the cap");
            assert!(
                out.ends_with('\u{2026}'),
                "a truncated URL wears an ellipsis"
            );
        }

        #[test]
        fn apply_on_an_empty_state_is_a_safe_noop() {
            let ctx = egui::Context::default();
            let mut state = WebState {
                address: "https://example.com/".to_owned(),
                ..WebState::default()
            };
            for action in [
                MenuAction::Back,
                MenuAction::Forward,
                MenuAction::Reload,
                MenuAction::OpenAddress,
                MenuAction::CycleContainer,
                MenuAction::CycleDisplayTarget,
                MenuAction::ZoomIn,
                MenuAction::ZoomOut,
                MenuAction::ResetZoom,
                MenuAction::OpenFind,
                MenuAction::ToggleAudioMute,
                MenuAction::ToggleForceDark,
                MenuAction::ToggleReaderMode,
                MenuAction::CaptureViewport,
                MenuAction::CaptureFullPage,
                MenuAction::CaptureMhtml,
                MenuAction::CaptureAnnotatedViewport,
                MenuAction::CaptureCalloutViewport,
                MenuAction::CaptureFreehandViewport,
                MenuAction::CaptureRegion,
                MenuAction::PrintPage,
                MenuAction::SavePdf,
                MenuAction::ClearCurrentTabData,
                MenuAction::ToggleSiteBlocking,
                MenuAction::ForgetSitePermissions,
                MenuAction::CopyUrl,
                MenuAction::AddBookmark,
                MenuAction::OpenBookmarksManager,
                MenuAction::SendInChat,
                MenuAction::ShareToPeer,
                MenuAction::ShareToEmail,
                MenuAction::ShareToQr,
                MenuAction::SendTabToNode,
                MenuAction::SendTabToPhone,
            ] {
                apply(&ctx, &mut state, action);
            }
            assert!(!state.respawn_requested, "no tab → Reload is a no-op");
            assert!(state.tabs.is_empty(), "no action attaches or drops a tab");
            assert_eq!(state.page_zoom_percent, 100, "no tab → zoom is unchanged");
            assert!(!state.find_open, "no tab → find remains closed");
        }

        #[test]
        fn the_view_menu_toggles_vertical_tabs() {
            let ctx = egui::Context::default();
            let mut state = WebState::default();
            assert!(!state.vertical_tabs);
            apply(&ctx, &mut state, MenuAction::ToggleVerticalTabs);
            assert!(state.vertical_tabs);
            apply(&ctx, &mut state, MenuAction::ToggleVerticalTabs);
            assert!(!state.vertical_tabs);
        }

        #[test]
        fn the_view_menu_toggles_downloads() {
            let ctx = egui::Context::default();
            let mut state = WebState::default();
            assert!(!state.downloads_open);
            apply(&ctx, &mut state, MenuAction::ToggleDownloads);
            assert!(state.downloads_open);
            apply(&ctx, &mut state, MenuAction::ToggleDownloads);
            assert!(!state.downloads_open);
        }

        #[test]
        fn the_bookmarks_menu_requests_the_manager_surface() {
            let ctx = egui::Context::default();
            let mut state = WebState::default();
            assert!(!state.take_bookmarks_manager_request());
            apply(&ctx, &mut state, MenuAction::OpenBookmarksManager);
            assert!(state.take_bookmarks_manager_request());
            assert!(!state.take_bookmarks_manager_request());
        }

        #[test]
        fn the_browser_bar_renders_headless() {
            use egui::{pos2, vec2, Rect};
            let ctx = egui::Context::default();
            Style::install(&ctx);
            let state = WebState::default();
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 640.0))),
                ..Default::default()
            };
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = show(&state, ui);
                });
            });
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            assert!(!prims.is_empty(), "the Browser bar produced no primitives");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};
    use mde_web_preview_client::{scm, testkit, wire};
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    /// A headless 960×640 shell body, mirroring the VDI + shell render tests.
    fn body_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        }
    }

    /// Drive one headless frame of `web_panel` on the CPU tessellation path (the
    /// same `Context::run` → `tessellate` the DRM runner drives, minus the GPU).
    fn run_panel(state: &mut WebState) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let out = ctx.run(body_input(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| web_panel(ui, state));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        !prims.is_empty()
    }

    /// Run frames until the active tab uploads its texture (the fake helper's frame
    /// is already buffered, so this settles immediately).
    fn run_until_texture(state: &mut WebState) -> bool {
        run_until_texture_for(state, 50)
    }

    fn run_until_texture_for(state: &mut WebState, frames: usize) -> bool {
        for _ in 0..frames {
            run_panel(state);
            if state
                .tabs
                .get(state.active)
                .is_some_and(|t| t.texture.is_some())
            {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        false
    }

    fn run_panel_on_ctx(ctx: &egui::Context, state: &mut WebState, input: egui::RawInput) -> bool {
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| web_panel(ui, state));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        !prims.is_empty()
    }

    fn write_helper_event(stream: &UnixStream, msg: &mde_web_preview_client::EventMsg) {
        let mut stream = stream;
        stream
            .write_all(&wire::frame(&msg.encode()))
            .expect("write helper event");
    }

    fn live_page_session() -> (
        WebSession,
        UnixStream,
        mde_web_preview_client::testkit::FrameWriter,
    ) {
        let (shell, helper) = UnixStream::pair().expect("socketpair");
        let writer =
            testkit::FrameWriter::create(testkit::FAKE_W, testkit::FAKE_H).expect("frame writer");
        writer
            .emit(
                testkit::FAKE_W,
                testkit::FAKE_H,
                mde_web_preview_client::PixelFormat::Rgba8,
                &testkit::gradient(testkit::FAKE_W, testkit::FAKE_H),
            )
            .expect("emit frame");
        scm::send_frame_with_fd(
            &helper,
            &mde_web_preview_client::EventMsg::AttachFrame.encode(),
            writer.raw_fd(),
        )
        .expect("attach frame");
        write_helper_event(
            &helper,
            &mde_web_preview_client::EventMsg::NavState {
                can_back: false,
                can_forward: false,
                loading: false,
                url: "https://example.test/".to_owned(),
            },
        );
        write_helper_event(
            &helper,
            &mde_web_preview_client::EventMsg::Title("Example".to_owned()),
        );
        write_helper_event(
            &helper,
            &mde_web_preview_client::EventMsg::PaintReady {
                seq: writer.sequence(),
            },
        );
        helper.set_nonblocking(true).expect("nonblocking helper");
        (
            WebSession::from_stream(shell, None).expect("session"),
            helper,
            writer,
        )
    }

    fn drain_control_messages(stream: &UnixStream) -> Vec<mde_web_preview_client::ControlMsg> {
        let mut rbuf = Vec::new();
        let mut out = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline {
            match scm::recv(stream).expect("recv controls") {
                scm::RecvOutcome::Data { bytes, .. } => {
                    rbuf.extend_from_slice(&bytes);
                    while let Some(payload) = wire::take_frame(&mut rbuf).expect("control frame") {
                        out.push(
                            mde_web_preview_client::ControlMsg::decode(&payload)
                                .expect("control decode"),
                        );
                    }
                }
                scm::RecvOutcome::WouldBlock => {
                    if !out.is_empty() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                scm::RecvOutcome::Eof => break,
            }
        }
        out
    }

    fn wait_for_fresh_frame(state: &mut WebState) -> bool {
        for _ in 0..100 {
            let Some(tab) = state.tabs.get_mut(state.active) else {
                return false;
            };
            tab.session.poll();
            if tab.session.take_frame().is_some() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        false
    }

    #[test]
    fn browser_capture_filename_prefers_host_and_sanitizes() {
        assert_eq!(
            capture_filename_for("https://Example.COM/some/path", "Ignored", 123),
            "mde-browser-123-example-com.png"
        );
        assert_eq!(
            capture_filename_for("about:blank", "News: Top Stories / Today", 456),
            "mde-browser-456-news-top-stories-today.png"
        );
        assert_eq!(
            capture_full_page_filename_for("https://Example.COM/some/path", "Ignored", 678),
            "mde-browser-full-page-678-example-com.png"
        );
        assert_eq!(
            capture_mhtml_filename_for("https://Example.COM/some/path", "Ignored", 679),
            "mde-browser-679-example-com.mhtml"
        );
        assert_eq!(
            capture_annotated_filename_for("https://Example.COM/some/path", "Ignored", 789),
            "mde-browser-annotated-789-example-com.png"
        );
        assert_eq!(
            capture_callout_filename_for("https://Example.COM/some/path", "Ignored", 888),
            "mde-browser-callout-888-example-com.png"
        );
        assert_eq!(
            capture_freehand_filename_for("https://Example.COM/some/path", "Ignored", 889),
            "mde-browser-freehand-889-example-com.png"
        );
        assert_eq!(
            pdf_filename_for("https://Example.COM/some/path", "Ignored", 999),
            "mde-browser-999-example-com.pdf"
        );
    }

    #[test]
    fn browser_capture_writes_latest_frame_png() {
        let (session, _helper, _writer) = live_page_session();
        let mut state = WebState::default();
        state.push_session(session);
        assert!(
            run_until_texture(&mut state),
            "the fake helper frame should upload before capture"
        );
        assert!(
            state.active_tab_has_frame(),
            "capture is gated on a retained helper frame"
        );

        let dir = tempfile::tempdir().expect("temp capture dir");
        let path = state
            .capture_active_viewport_to_dir(dir.path())
            .expect("capture writes PNG");
        assert_eq!(path.parent(), Some(dir.path()));
        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("example-test") && name.ends_with(".png")),
            "capture filename should include the current host: {}",
            path.display()
        );
        let bytes = std::fs::read(&path).expect("read capture");
        let image = crate::chooser::decode_png_rgba(&bytes).expect("capture decodes");
        assert_eq!(
            image.size,
            [testkit::FAKE_W as usize, testkit::FAKE_H as usize],
            "capture preserves the helper viewport dimensions"
        );
    }

    #[test]
    fn browser_full_page_capture_writes_named_png() {
        let (session, _helper, _writer) = live_page_session();
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));

        let dir = tempfile::tempdir().expect("temp capture dir");
        let path = state
            .capture_active_full_page_to_dir(dir.path())
            .expect("full-page capture writes PNG");

        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("mde-browser-full-page-")
                    && name.contains("example-test")
                    && name.ends_with(".png")),
            "full-page capture filename should include the current host: {}",
            path.display()
        );
        let bytes = std::fs::read(&path).expect("read capture");
        let image = crate::chooser::decode_png_rgba(&bytes).expect("capture decodes");
        assert_eq!(
            image.size,
            [testkit::FAKE_W as usize, testkit::FAKE_H as usize],
            "full-page capture preserves the current helper surface until stitching lands"
        );
    }

    #[test]
    fn browser_mhtml_capture_writes_related_archive() {
        let (session, _helper, _writer) = live_page_session();
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));

        let dir = tempfile::tempdir().expect("temp capture dir");
        let path = state
            .capture_active_mhtml_to_dir(dir.path())
            .expect("mhtml capture writes archive");

        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("mde-browser-")
                    && name.contains("example-test")
                    && name.ends_with(".mhtml")),
            "mhtml capture filename should include the current host: {}",
            path.display()
        );
        let archive = std::fs::read_to_string(&path).expect("read mhtml");
        assert!(archive.contains("Content-Type: multipart/related"));
        assert!(archive.contains("Content-Type: text/html; charset=\"utf-8\""));
        assert!(archive.contains("Content-Type: image/png"));
        assert!(archive.contains("Content-Transfer-Encoding: base64"));
        assert!(archive.contains("https://example.test/"));
        assert!(archive.contains("mde-browser-capture.png"));
        assert!(
            archive.contains("iVBORw0KGgo"),
            "the embedded related part should contain a base64 PNG payload"
        );
    }

    #[test]
    fn mhtml_capture_escapes_page_metadata() {
        let archive = mhtml_capture_document(
            "https://example.test/?q=<tag>&x=1",
            "A <Title> & \"Quote\"",
            42,
            b"not a real png for structure testing",
        );
        let archive = String::from_utf8(archive).expect("mhtml is utf8");
        assert!(archive.contains("A &lt;Title&gt; &amp; &quot;Quote&quot;"));
        assert!(archive.contains("https://example.test/?q=&lt;tag&gt;&amp;x=1"));
        assert!(!archive.contains("<Title>"));
    }

    #[test]
    fn browser_annotated_capture_writes_captioned_png() {
        let (session, _helper, _writer) = live_page_session();
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));

        let dir = tempfile::tempdir().expect("temp capture dir");
        let path = state
            .capture_active_annotated_viewport_to_dir(dir.path())
            .expect("annotated capture writes PNG");

        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("mde-browser-annotated-")
                    && name.contains("example-test")
                    && name.ends_with(".png")),
            "annotated capture filename should include the current host: {}",
            path.display()
        );
        let bytes = std::fs::read(&path).expect("read capture");
        let image = crate::chooser::decode_png_rgba(&bytes).expect("capture decodes");
        assert_eq!(
            image.size,
            [
                testkit::FAKE_W as usize,
                testkit::FAKE_H as usize + ANNOTATION_BAR_HEIGHT
            ],
            "annotated capture appends a visible caption band"
        );
        let frame_pixels = (testkit::FAKE_W * testkit::FAKE_H) as usize;
        assert!(
            image.pixels[frame_pixels..]
                .iter()
                .any(|pixel| *pixel == Style::TEXT),
            "caption band should contain rendered annotation text"
        );
    }

    #[test]
    fn browser_callout_capture_writes_annotated_png() {
        let (session, _helper, _writer) = live_page_session();
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));

        let dir = tempfile::tempdir().expect("temp capture dir");
        let path = state
            .capture_active_callout_viewport_to_dir(dir.path())
            .expect("callout capture writes PNG");

        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("mde-browser-callout-")
                    && name.contains("example-test")
                    && name.ends_with(".png")),
            "callout capture filename should include the current host: {}",
            path.display()
        );
        let bytes = std::fs::read(&path).expect("read capture");
        let image = crate::chooser::decode_png_rgba(&bytes).expect("capture decodes");
        assert_eq!(
            image.size,
            [
                testkit::FAKE_W as usize,
                testkit::FAKE_H as usize + ANNOTATION_BAR_HEIGHT
            ],
            "callout capture appends a visible caption band"
        );
        let frame_pixels = (testkit::FAKE_W * testkit::FAKE_H) as usize;
        assert!(
            image.pixels[frame_pixels..]
                .iter()
                .any(|pixel| *pixel == egui::Color32::from_rgb(255, 255, 255)),
            "callout capture should render a callout label into the caption band"
        );
    }

    #[test]
    fn browser_freehand_capture_writes_annotated_png() {
        let (session, _helper, _writer) = live_page_session();
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));

        let dir = tempfile::tempdir().expect("temp capture dir");
        let path = state
            .capture_active_freehand_viewport_to_dir(dir.path())
            .expect("freehand capture writes PNG");

        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("mde-browser-freehand-")
                    && name.contains("example-test")
                    && name.ends_with(".png")),
            "freehand capture filename should include the current host: {}",
            path.display()
        );
        let bytes = std::fs::read(&path).expect("read capture");
        let image = crate::chooser::decode_png_rgba(&bytes).expect("capture decodes");
        assert_eq!(
            image.size,
            [
                testkit::FAKE_W as usize,
                testkit::FAKE_H as usize + ANNOTATION_BAR_HEIGHT
            ],
            "freehand capture appends a visible caption band"
        );
        let frame_pixels = (testkit::FAKE_W * testkit::FAKE_H) as usize;
        assert!(
            image.pixels[frame_pixels..]
                .iter()
                .any(|pixel| *pixel == egui::Color32::from_rgb(255, 255, 255)),
            "freehand capture should render a freehand label into the caption band"
        );
    }

    #[test]
    fn annotated_capture_preserves_frame_and_adds_caption_band() {
        let mut img = egui::ColorImage::new([32, 4], egui::Color32::from_rgb(1, 2, 3));
        img.pixels[3] = egui::Color32::from_rgb(9, 8, 7);

        let annotated = annotate_capture_image(&img, "Example | https://example.test | 123")
            .expect("valid annotation");

        assert_eq!(annotated.size, [32, 4 + ANNOTATION_BAR_HEIGHT]);
        assert_eq!(&annotated.pixels[..img.pixels.len()], &img.pixels[..]);
        assert!(
            annotated.pixels[img.pixels.len()..]
                .iter()
                .any(|pixel| *pixel == Style::TEXT),
            "caption text should be painted into the appended band"
        );
    }

    #[test]
    fn callout_capture_draws_overlay_and_preserves_frame_area() {
        let img = egui::ColorImage::new([64, 48], egui::Color32::from_rgb(1, 2, 3));

        let annotated =
            annotate_callout_capture_image(&img, "Example | https://example.test | 123")
                .expect("valid callout annotation");

        assert_eq!(annotated.size, [64, 48 + ANNOTATION_BAR_HEIGHT]);
        assert_eq!(
            annotated.pixels[0], img.pixels[0],
            "unannotated corners of the captured frame remain intact"
        );
        assert!(
            annotated.pixels[..(64 * 48)]
                .iter()
                .any(|pixel| *pixel == Style::ACCENT),
            "callout overlay should paint an accent rectangle or leader line"
        );
        assert!(
            annotated.pixels[(64 * 48)..]
                .iter()
                .any(|pixel| *pixel == egui::Color32::from_rgb(255, 255, 255)),
            "callout label should be painted into the appended band"
        );
    }

    #[test]
    fn freehand_capture_draws_stroke_and_preserves_frame_area() {
        let img = egui::ColorImage::new([64, 48], egui::Color32::from_rgb(1, 2, 3));

        let annotated =
            annotate_freehand_capture_image(&img, "Example | https://example.test | 123")
                .expect("valid freehand annotation");

        assert_eq!(annotated.size, [64, 48 + ANNOTATION_BAR_HEIGHT]);
        assert_eq!(
            annotated.pixels[0], img.pixels[0],
            "unannotated corners of the captured frame remain intact"
        );
        assert!(
            annotated.pixels[..(64 * 48)]
                .iter()
                .any(|pixel| *pixel == egui::Color32::from_rgb(255, 255, 255)),
            "freehand overlay should paint a visible white stroke"
        );
        assert!(
            annotated.pixels[(64 * 48)..]
                .iter()
                .any(|pixel| *pixel == egui::Color32::from_rgb(255, 255, 255)),
            "freehand label should be painted into the appended band"
        );
    }

    #[test]
    fn browser_region_capture_crops_latest_frame_png() {
        let (session, _helper, _writer) = live_page_session();
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));

        let dir = tempfile::tempdir().expect("temp capture dir");
        let path = state
            .capture_active_region_to_dir(
                dir.path(),
                PixelRegion {
                    x: 1,
                    y: 1,
                    width: 3,
                    height: 2,
                },
            )
            .expect("region capture writes PNG");

        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("mde-browser-region-")
                    && name.contains("example-test")
                    && name.ends_with(".png")),
            "region capture filename should include the current host: {}",
            path.display()
        );
        let bytes = std::fs::read(&path).expect("read capture");
        let image = crate::chooser::decode_png_rgba(&bytes).expect("capture decodes");
        assert_eq!(image.size, [3, 2]);
    }

    #[test]
    fn browser_region_crop_validates_and_preserves_pixels() {
        let mut img = egui::ColorImage::new([4, 3], egui::Color32::TRANSPARENT);
        for y in 0..3 {
            for x in 0..4 {
                img.pixels[y * 4 + x] =
                    egui::Color32::from_rgba_unmultiplied(x as u8, y as u8, (x + y) as u8, 255);
            }
        }

        let cropped = crop_color_image(
            &img,
            PixelRegion {
                x: 1,
                y: 1,
                width: 2,
                height: 2,
            },
        )
        .expect("valid crop");

        assert_eq!(cropped.size, [2, 2]);
        assert_eq!(cropped.pixels[0], img.pixels[5]);
        assert_eq!(cropped.pixels[1], img.pixels[6]);
        assert_eq!(cropped.pixels[2], img.pixels[9]);
        assert_eq!(cropped.pixels[3], img.pixels[10]);
        assert!(
            crop_color_image(
                &img,
                PixelRegion {
                    x: 3,
                    y: 2,
                    width: 2,
                    height: 1,
                },
            )
            .is_err(),
            "out-of-bounds regions are rejected"
        );
    }

    #[test]
    fn pdf_completion_events_update_the_browser_notice() {
        let (session, helper, _writer) = live_page_session();
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));

        write_helper_event(
            &helper,
            &mde_web_preview_client::EventMsg::PdfSaved {
                path: "/tmp/mde-page.pdf".to_owned(),
                ok: true,
            },
        );
        run_panel(&mut state);
        assert_eq!(
            state.capture_notice.as_deref(),
            Some("PDF saved /tmp/mde-page.pdf")
        );

        write_helper_event(
            &helper,
            &mde_web_preview_client::EventMsg::PdfSaved {
                path: "/tmp/mde-page.pdf".to_owned(),
                ok: false,
            },
        );
        run_panel(&mut state);
        assert_eq!(
            state.capture_notice.as_deref(),
            Some("PDF failed /tmp/mde-page.pdf")
        );
    }

    #[test]
    fn saved_pdf_opens_in_a_cef_viewer_tab() {
        let dir = tempfile::tempdir().expect("pdf dir");
        let path = dir.path().join("report one.pdf");
        std::fs::write(&path, b"%PDF-1.7\n% viewer fixture\n").expect("pdf fixture");
        let path_text = path.to_string_lossy().into_owned();
        let mut state = WebState::default();

        assert_eq!(
            state.handle_pdf_event(path_text.clone(), true),
            format!("PDF saved {path_text}")
        );
        state.open_last_saved_pdf();

        assert_eq!(
            state.take_open_request(),
            Some(TabOpenIntent::NewForegroundUrl {
                engine: BrowserEngine::Cef,
                url: format!("file://{}", path_text.replace(' ', "%20")),
            })
        );
        assert_eq!(
            state.capture_notice.as_deref(),
            Some("Opening PDF in CEF viewer")
        );
    }

    #[test]
    fn pdf_viewer_refuses_missing_or_non_pdf_output() {
        let dir = tempfile::tempdir().expect("pdf dir");
        let path = dir.path().join("not-pdf.pdf");
        std::fs::write(&path, b"not a pdf\n").expect("bad fixture");
        let mut state = WebState::default();
        state.last_saved_pdf = Some(path.clone());

        state.open_last_saved_pdf();

        assert_eq!(state.take_open_request(), None);
        assert!(
            state
                .capture_notice
                .as_deref()
                .is_some_and(|notice| notice.starts_with("PDF viewer failed:")),
            "viewer should explain the refused file: {:?}",
            state.capture_notice
        );
    }

    #[test]
    fn browser_body_input_is_localized_and_keyboard_is_focus_gated() {
        let rect = Rect::from_min_size(pos2(20.0, 40.0), vec2(320.0, 200.0));

        let moved = browser_input_event(&egui::Event::PointerMoved(pos2(70.0, 90.0)), rect, false)
            .expect("pointer inside page");
        assert_eq!(moved, egui::Event::PointerMoved(pos2(50.0, 50.0)));

        let key = egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        };
        assert_eq!(
            browser_input_event(&key, rect, false),
            None,
            "address-bar/chrome keystrokes must not leak into the page"
        );
        assert_eq!(
            browser_input_event(&key, rect, true),
            Some(key),
            "click-focused page canvas receives keyboard events"
        );
        assert_eq!(
            browser_input_event(&egui::Event::Text("mesh".to_owned()), rect, true),
            Some(egui::Event::Text("mesh".to_owned())),
            "committed text reaches the focused page canvas"
        );
    }

    #[test]
    fn browser_panel_click_focuses_page_and_sends_keyboard_input_to_helper() {
        let (session, helper, _writer) = live_page_session();
        let mut state = WebState::default();
        state.push_session(session);
        let ctx = egui::Context::default();
        Style::install(&ctx);
        assert!(
            run_panel_on_ctx(&ctx, &mut state, body_input()),
            "the live page frame must upload before body input can be painted"
        );

        let page_point = pos2(480.0, 420.0);
        let mut click_input = body_input();
        click_input.events = vec![
            egui::Event::PointerMoved(page_point),
            egui::Event::PointerButton {
                pos: page_point,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::PointerButton {
                pos: page_point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            },
        ];
        assert!(run_panel_on_ctx(&ctx, &mut state, click_input));
        let mut key_input = body_input();
        key_input.events = vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            },
            egui::Event::Text("mesh".to_owned()),
        ];
        assert!(run_panel_on_ctx(&ctx, &mut state, key_input));

        let controls = drain_control_messages(&helper);
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::Input(
                    mde_web_preview_client::InputEvent::PointerButton { pressed: true, .. }
                )
            )),
            "clicking the Browser body must send a page pointer press: {controls:?}"
        );
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::Input(
                    mde_web_preview_client::InputEvent::Key {
                        key: mde_web_preview_client::wire::KeyCode::A,
                        pressed: true,
                        ..
                    }
                )
            )),
            "a focused Browser body must forward key input: {controls:?}"
        );
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::Input(
                    mde_web_preview_client::InputEvent::Text(text)
                ) if text == "mesh"
            )),
            "a focused Browser body must forward committed text: {controls:?}"
        );
    }

    #[test]
    fn page_zoom_and_find_actions_send_helper_controls() {
        let (session, helper, _writer) = live_page_session();
        let mut state = WebState::default();
        state.push_session(session);
        let ctx = egui::Context::default();

        menubar::apply(&ctx, &mut state, menubar::MenuAction::ZoomIn);
        menubar::apply(&ctx, &mut state, menubar::MenuAction::ZoomOut);
        menubar::apply(&ctx, &mut state, menubar::MenuAction::OpenFind);
        assert!(
            state.find_open,
            "OpenFind keeps the compact find chrome visible until close"
        );
        state.find_query = "mesh".to_owned();
        state.submit_find(false);
        state.submit_find(true);
        state.close_find_bar();
        menubar::apply(&ctx, &mut state, menubar::MenuAction::ToggleAudioMute);
        assert!(state.tabs[state.active].muted);
        menubar::apply(&ctx, &mut state, menubar::MenuAction::ToggleAudioMute);
        assert!(!state.tabs[state.active].muted);
        menubar::apply(&ctx, &mut state, menubar::MenuAction::ToggleForceDark);
        assert!(state.tabs[state.active].force_dark);
        menubar::apply(&ctx, &mut state, menubar::MenuAction::ToggleForceDark);
        assert!(!state.tabs[state.active].force_dark);
        menubar::apply(&ctx, &mut state, menubar::MenuAction::ToggleReaderMode);
        assert!(state.tabs[state.active].reader_mode);
        menubar::apply(&ctx, &mut state, menubar::MenuAction::ToggleReaderMode);
        assert!(!state.tabs[state.active].reader_mode);
        let print_dir = tempfile::tempdir().expect("print spool dir");
        let print_path = state
            .queue_active_page_cups_print_to_dir(print_dir.path())
            .expect("cups print path queued");
        let print_path_text = print_path.to_string_lossy().into_owned();
        menubar::apply(&ctx, &mut state, menubar::MenuAction::PrintPage);
        let pdf_dir = tempfile::tempdir().expect("pdf output dir");
        let pdf_path = state
            .save_active_page_pdf_to_dir(pdf_dir.path())
            .expect("pdf path queued");
        let pdf_path_text = pdf_path.to_string_lossy().into_owned();

        let controls = drain_control_messages(&helper);
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::SetZoom { percent: 110 }
            )),
            "zoom in must send a helper zoom control: {controls:?}"
        );
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::SetZoom { percent: 100 }
            )),
            "zoom out must restore a helper zoom control: {controls:?}"
        );
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::FindInPage {
                    query,
                    backwards: false,
                } if query == "mesh"
            )),
            "forward find must reach the helper: {controls:?}"
        );
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::FindInPage {
                    query,
                    backwards: true,
                } if query == "mesh"
            )),
            "backward find must reach the helper: {controls:?}"
        );
        assert!(
            controls
                .iter()
                .any(|msg| matches!(msg, mde_web_preview_client::ControlMsg::ClearFind)),
            "closing find must clear the helper selection: {controls:?}"
        );
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::SetAudioMuted { muted: true }
            )),
            "muting the tab must reach the helper: {controls:?}"
        );
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::SetAudioMuted { muted: false }
            )),
            "unmuting the tab must reach the helper: {controls:?}"
        );
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::SetForceDark { enabled: true }
            )),
            "enabling force-dark must reach the helper: {controls:?}"
        );
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::SetForceDark { enabled: false }
            )),
            "disabling force-dark must reach the helper: {controls:?}"
        );
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::SetReaderMode { enabled: true }
            )),
            "enabling reader mode must reach the helper: {controls:?}"
        );
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::SetReaderMode { enabled: false }
            )),
            "disabling reader mode must reach the helper: {controls:?}"
        );
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::SavePdf { path }
                    if path == &print_path_text
            )),
            "CUPS print must request a helper PDF before lp submission: {controls:?}"
        );
        assert!(
            controls.iter().any(|msg| matches!(
                msg,
                mde_web_preview_client::ControlMsg::SavePdf { path } if path == &pdf_path_text
            )),
            "save-as-PDF must reach the helper with the chosen path: {controls:?}"
        );
    }

    #[test]
    fn cups_print_submission_uses_lp_title_and_pdf_path() {
        let dir = tempfile::tempdir().expect("cups pdf dir");
        let path = dir.path().join("page.pdf");
        std::fs::write(&path, b"%PDF-1.7\n").expect("pdf fixture");
        let mut seen_program = String::new();
        let mut seen_args = Vec::new();

        let job = submit_pdf_to_cups_with_runner(
            &path,
            "Magic Mesh Browser - Example",
            &CupsPrintSettings::default(),
            |program, args, timeout| {
                seen_program = program.to_owned();
                seen_args = args.to_vec();
                assert_eq!(timeout, CUPS_PRINT_TIMEOUT);
                Ok(ProcessOutput {
                    success: true,
                    stdout: "request id is Office-42 (1 file)\n".to_owned(),
                    stderr: String::new(),
                })
            },
        )
        .expect("lp submission succeeds");

        assert_eq!(job, "request id is Office-42 (1 file)");
        assert_eq!(seen_program, "lp");
        assert_eq!(
            seen_args,
            vec![
                "-t".to_owned(),
                "Magic Mesh Browser - Example".to_owned(),
                path.to_string_lossy().into_owned(),
            ]
        );
    }

    #[test]
    fn cups_print_submission_surfaces_lp_errors_without_a_printer() {
        let dir = tempfile::tempdir().expect("cups pdf dir");
        let path = dir.path().join("page.pdf");
        std::fs::write(&path, b"%PDF-1.7\n").expect("pdf fixture");

        let err = submit_pdf_to_cups_with_runner(
            &path,
            "Example",
            &CupsPrintSettings::default(),
            |_program, _args, _timeout| {
                Ok(ProcessOutput {
                    success: false,
                    stdout: String::new(),
                    stderr: "lp: Error - no default destination available\n".to_owned(),
                })
            },
        )
        .expect_err("lp failure is surfaced");

        assert_eq!(err, "lp: Error - no default destination available");
    }

    #[test]
    fn cups_print_submission_applies_destination_and_options() {
        let dir = tempfile::tempdir().expect("cups pdf dir");
        let path = dir.path().join("page.pdf");
        std::fs::write(&path, b"%PDF-1.7\n").expect("pdf fixture");
        let settings = CupsPrintSettings {
            destination: Some("Office".to_owned()),
            copies: 3,
            duplex: true,
            grayscale: true,
        };
        let mut seen_args = Vec::new();

        submit_pdf_to_cups_with_runner(&path, "Example", &settings, |_program, args, _timeout| {
            seen_args = args.to_vec();
            Ok(ProcessOutput {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            })
        })
        .expect("lp submission succeeds");

        assert_eq!(
            seen_args,
            vec![
                "-d".to_owned(),
                "Office".to_owned(),
                "-n".to_owned(),
                "3".to_owned(),
                "-o".to_owned(),
                "sides=two-sided-long-edge".to_owned(),
                "-o".to_owned(),
                "ColorModel=Gray".to_owned(),
                "-t".to_owned(),
                "Example".to_owned(),
                path.to_string_lossy().into_owned(),
            ]
        );
    }

    #[test]
    fn cups_printer_discovery_marks_the_default_destination() {
        let printers = discover_cups_printers_with_runner(|program, args, timeout| {
            assert_eq!(program, "lpstat");
            assert_eq!(timeout, CUPS_PRINT_TIMEOUT);
            if args == ["-e"] {
                Ok(ProcessOutput {
                    success: true,
                    stdout: "Lab\nOffice\nLab\n".to_owned(),
                    stderr: String::new(),
                })
            } else if args == ["-d"] {
                Ok(ProcessOutput {
                    success: true,
                    stdout: "system default destination: Office\n".to_owned(),
                    stderr: String::new(),
                })
            } else {
                panic!("unexpected lpstat args: {args:?}");
            }
        })
        .expect("printer discovery succeeds");

        assert_eq!(
            printers,
            vec![
                CupsPrinter {
                    name: "Office".to_owned(),
                    is_default: true,
                },
                CupsPrinter {
                    name: "Lab".to_owned(),
                    is_default: false,
                },
            ]
        );
    }

    #[test]
    fn container_tabs_are_named_per_tab_and_visible_in_chrome() {
        let (first, _first_helper, _first_writer) = live_page_session();
        let (second, _second_helper, _second_writer) = live_page_session();
        let mut state = WebState::default();
        state.push_session(first);
        state.push_session(second);

        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.active, 1);
        assert_eq!(state.tabs[0].container, ContainerProfile::None);
        assert_eq!(state.tabs[1].container, ContainerProfile::None);

        state.set_active_tab_container(ContainerProfile::Work);
        assert_eq!(state.tabs[1].container, ContainerProfile::Work);
        assert_eq!(
            state.tabs[0].container,
            ContainerProfile::None,
            "container identity stays per-tab"
        );
        assert!(
            tab_label(&state.tabs[1]).contains("W "),
            "the tab pill carries the Work marker"
        );
        assert!(
            tab_hover(&state.tabs[1]).contains("Container: Work"),
            "the hover text names the container"
        );

        state.cycle_active_tab_container();
        assert_eq!(state.tabs[1].container, ContainerProfile::Banking);
        state.select_tab(0);
        state.cycle_active_tab_container();
        assert_eq!(state.tabs[0].container, ContainerProfile::Personal);
        assert_eq!(state.tabs[1].container, ContainerProfile::Banking);
    }

    #[test]
    fn display_targets_are_per_tab_and_visible_in_chrome() {
        let (first, _first_helper, _first_writer) = live_page_session();
        let (second, _second_helper, _second_writer) = live_page_session();
        let mut state = WebState::default();
        state.push_session(first);
        state.push_session(second);

        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.active, 1);
        assert_eq!(state.tabs[0].display_target, DisplayTarget::Current);
        assert_eq!(state.tabs[1].display_target, DisplayTarget::Current);

        state.set_active_tab_display_target(DisplayTarget::Secondary);
        assert_eq!(state.tabs[1].display_target, DisplayTarget::Secondary);
        assert_eq!(
            state.tabs[0].display_target,
            DisplayTarget::Current,
            "display target intent stays per-tab"
        );
        assert!(
            tab_label(&state.tabs[1]).contains("D2 "),
            "the tab pill carries the Display 2 marker"
        );
        assert!(
            tab_hover(&state.tabs[1]).contains("Display target: Secondary Display"),
            "the hover text names the display target"
        );

        state.cycle_active_tab_display_target();
        assert_eq!(state.tabs[1].display_target, DisplayTarget::AllDisplays);
        state.select_tab(0);
        state.cycle_active_tab_display_target();
        assert_eq!(state.tabs[0].display_target, DisplayTarget::Primary);
        assert_eq!(state.tabs[1].display_target, DisplayTarget::AllDisplays);
    }

    #[test]
    fn display_target_changes_publish_platform_handoff() {
        let bus = tempfile::tempdir().expect("temp bus");
        let (session, _helper, _writer) = live_page_session();
        let mut state = WebState::default().with_bus_root(Some(bus.path().to_path_buf()));
        state.push_session_with_engine(session, BrowserEngine::Cef);
        run_until_texture(&mut state);

        state.set_active_tab_display_target(DisplayTarget::Secondary);

        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let msgs = persist
            .list_since(ACTION_BROWSER_DISPLAY_TARGET, None)
            .expect("list display target actions");
        assert_eq!(msgs.len(), 1);
        let body = msgs[0].body.as_deref().expect("handoff body");
        let v: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
        assert_eq!(v["op"], "browser_display_target");
        assert_eq!(v["tab_index"], 0);
        assert_eq!(v["engine"], "cef");
        assert_eq!(v["target"], "secondary");
        assert_eq!(v["url"], "https://example.test/");
        assert_eq!(v["title"], "Example");

        state.set_active_tab_display_target(DisplayTarget::AllDisplays);
        let msgs = persist
            .list_since(ACTION_BROWSER_DISPLAY_TARGET, None)
            .expect("list display target actions");
        assert_eq!(msgs.len(), 2);
        let body = msgs[1].body.as_deref().expect("second handoff body");
        let v: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
        assert_eq!(v["target"], "all_displays");
    }

    #[test]
    fn inactive_idle_tabs_publish_suspend_handoff_and_stop_once() {
        let bus = tempfile::tempdir().expect("temp bus");
        let (first, first_helper, _first_writer) = live_page_session();
        let (second, _second_helper, _second_writer) = live_page_session();
        let mut state = WebState::default().with_bus_root(Some(bus.path().to_path_buf()));
        state.push_session_with_engine(first, BrowserEngine::Servo);
        state.push_session_with_engine(second, BrowserEngine::Cef);
        assert_eq!(state.active, 1, "second tab is active");
        state.tabs[0].session.poll();
        state.tabs[1].session.poll();
        let _ = drain_control_messages(&first_helper);

        let now = Instant::now();
        state.tabs[0].last_activity = now - IDLE_TAB_SUSPEND_AFTER - Duration::from_secs(1);
        state.suspend_idle_tabs(now);
        state.suspend_idle_tabs(now + Duration::from_secs(1));

        assert!(state.tabs[0].idle_suspended);
        assert!(!state.tabs[1].idle_suspended, "active tab is not suspended");
        let controls = drain_control_messages(&first_helper);
        assert!(
            controls
                .iter()
                .any(|msg| matches!(msg, mde_web_preview_client::ControlMsg::Stop)),
            "inactive idle tab received a Stop control: {controls:?}"
        );

        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let msgs = persist
            .list_since(ACTION_BROWSER_TAB_SUSPEND, None)
            .expect("list browser suspend actions");
        assert_eq!(msgs.len(), 1, "suspend handoff is once per idle period");
        let body = msgs[0].body.as_deref().expect("suspend body");
        let v: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
        assert_eq!(v["op"], "browser_tab_suspend");
        assert_eq!(v["tab_index"], 0);
        assert_eq!(v["engine"], "servo");
        assert_eq!(v["url"], "https://example.test/");
        assert_eq!(v["source"], "browser");
        assert_eq!(
            v["idle_after_ms"],
            u64::try_from(IDLE_TAB_SUSPEND_AFTER.as_millis()).unwrap()
        );
        assert!(
            tab_label(&state.tabs[0]).contains('\u{25D2}'),
            "suspended tabs wear the idle marker"
        );
        assert!(tab_hover(&state.tabs[0]).contains("Idle suspended"));
    }

    #[test]
    fn selecting_a_suspended_tab_reactivates_idle_state() {
        let (first, _first_helper, _first_writer) = live_page_session();
        let (second, _second_helper, _second_writer) = live_page_session();
        let mut state = WebState::default();
        state.push_session(first);
        state.push_session(second);
        state.tabs[0].idle_suspended = true;
        let old_activity = state.tabs[0].last_activity;

        state.select_tab(0);

        assert_eq!(state.active, 0);
        assert!(!state.tabs[0].idle_suspended);
        assert!(state.tabs[0].last_activity >= old_activity);
    }

    #[test]
    fn no_session_paints_the_gated_empty_state() {
        let mut state = WebState::default();
        assert!(run_panel(&mut state), "the gated EmptyState drew nothing");
        assert!(state.tabs.is_empty());
    }

    #[test]
    fn a_paint_ready_frame_uploads_to_a_texture_and_paints() {
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        assert!(
            run_until_texture(&mut state),
            "no frame uploaded to a texture"
        );
        assert!(state.tabs[0].texture.is_some());
        assert!(run_panel(&mut state), "the browser image produced no draw");
    }

    #[test]
    fn tab_strip_state_switches_closes_and_requests_new_tabs() {
        let (first, _helper1) = testkit::connect().expect("connect 1");
        let (second, _helper2) = testkit::connect().expect("connect 2");
        let mut state = WebState::default();
        state.push_session(first);
        state.push_session(second);
        assert!(run_until_texture(&mut state));
        assert_eq!(state.active, 1, "new pushed tabs become foreground");

        state.select_tab(0);
        assert_eq!(state.active, 0);
        assert_eq!(state.address, "about:blank");

        state.close_tab(0);
        assert_eq!(state.tabs.len(), 1);
        assert_eq!(state.active, 0, "active index stays valid after close");

        state.request_new_tab(BrowserEngine::Cef);
        assert_eq!(
            state.take_open_request(),
            Some(TabOpenIntent::NewForeground(BrowserEngine::Cef))
        );
        assert_eq!(state.take_open_request(), None, "request drains once");
    }

    #[test]
    fn tab_reorder_preserves_the_active_session_identity() {
        let (first, _helper1) = testkit::connect().expect("connect 1");
        let (second, _helper2) = testkit::connect().expect("connect 2");
        let (third, _helper3) = testkit::connect().expect("connect 3");
        let mut state = WebState::default();
        state.push_session(first);
        state.push_session(second);
        state.push_session(third);
        assert!(run_until_texture(&mut state));
        state.select_tab(1);
        let active_title = state.tabs[state.active].session.title().to_owned();

        state.move_tab(1, 0);
        assert_eq!(state.active, 0);
        assert_eq!(state.tabs[state.active].session.title(), active_title);

        state.move_tab(0, 2);
        assert_eq!(state.active, 2);
        assert_eq!(state.tabs[state.active].session.title(), active_title);
    }

    #[test]
    fn closing_the_last_tab_returns_to_the_honest_empty_state() {
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));
        state.close_tab(0);
        assert!(state.tabs.is_empty());
        assert_eq!(state.active, 0);
        assert!(state.address.is_empty());
        assert!(run_panel(&mut state), "empty browser state draws honestly");
    }

    #[test]
    fn tab_strip_renders_with_multiple_tabs() {
        let (first, _helper1) = testkit::connect().expect("connect 1");
        let (second, _helper2) = testkit::connect().expect("connect 2");
        let mut state = WebState::default();
        state.push_session(first);
        state.push_session(second);
        assert!(run_panel(&mut state), "tab strip produced no primitives");
    }

    #[test]
    fn vertical_tab_strip_renders_with_the_same_sessions() {
        let (first, _helper1) = testkit::connect().expect("connect 1");
        let (second, _helper2) = testkit::connect().expect("connect 2");
        let mut state = WebState::default();
        state.push_session(first);
        state.push_session(second);
        state.set_vertical_tabs(true);
        assert!(
            run_panel(&mut state),
            "vertical tabs produced no primitives"
        );
        assert_eq!(state.tabs.len(), 2);
        assert!(state.vertical_tabs);
    }

    #[test]
    fn new_tab_dashboard_renders_for_about_blank() {
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));
        assert_eq!(state.tabs[0].session.nav().url, "about:blank");
        assert!(run_panel(&mut state), "new-tab dashboard draws");
    }

    #[test]
    fn new_tab_dashboard_search_loads_mesh_searxng() {
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));
        state.dashboard_query = "mesh browser".to_owned();

        state.submit_dashboard_search();

        assert_eq!(state.address, "https://search.mesh/search?q=mesh+browser");
        assert_eq!(state.insecure_prompt, None);
        assert!(
            wait_for_fresh_frame(&mut state),
            "dashboard search reached the helper"
        );
    }

    #[test]
    fn new_tab_dashboard_mesh_service_shortcuts_use_the_same_load_gate() {
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));
        let music = NEW_TAB_SERVICES
            .iter()
            .find(|svc| svc.label == "Music")
            .expect("music shortcut");

        state.open_mesh_service(music.url.to_owned());

        assert_eq!(state.address, "http://music.mesh:4533/");
        assert_eq!(
            state.insecure_prompt.as_deref(),
            Some("http://music.mesh:4533/")
        );
        assert!(
            !state.tabs[0].session.nav().loading,
            "HTTP mesh services pause at the same HTTPS prompt"
        );
    }

    #[test]
    fn explicit_http_address_prompts_before_loading() {
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));
        state.address = "http://plain.example/path".to_owned();

        state.submit_address();

        assert_eq!(
            state.insecure_prompt.as_deref(),
            Some("http://plain.example/path")
        );
        assert_eq!(state.address, "http://plain.example/path");
        assert!(
            !state.tabs[0].session.nav().loading,
            "HTTP prompt pauses before sending Load to the helper"
        );
        assert!(run_panel(&mut state), "the HTTP prompt renders");
    }

    #[test]
    fn http_prompt_can_upgrade_or_continue() {
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));
        state.address = "http://plain.example/path".to_owned();
        state.submit_address();
        state.upgrade_insecure_load();
        assert_eq!(state.insecure_prompt, None);
        assert_eq!(state.address, "https://plain.example/path");
        assert!(
            wait_for_fresh_frame(&mut state),
            "upgraded HTTPS load reached the helper"
        );

        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));
        state.address = "http://plain.example/keep-http".to_owned();
        state.submit_address();
        state.continue_insecure_load();
        assert_eq!(state.insecure_prompt, None);
        assert_eq!(state.address, "http://plain.example/keep-http");
        assert!(
            wait_for_fresh_frame(&mut state),
            "continued HTTP load reached the helper"
        );
    }

    #[test]
    fn a_crashed_tab_paints_the_honest_crashed_state() {
        let (session, helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        run_until_texture(&mut state);

        helper.crash();
        // Poll it to observe the crash, then render the crashed body.
        assert!(run_panel(&mut state));
        assert!(state.tabs[0].session.is_crashed());
        assert!(run_panel(&mut state), "the crashed body produced no draw");
    }

    #[test]
    fn one_tabs_crash_does_not_disturb_another() {
        let (a, helper_a) = testkit::connect().expect("connect a");
        let (b, _helper_b) = testkit::connect().expect("connect b");
        let mut state = WebState::default();
        state.push_session(a); // tab 0
        state.push_session(b); // tab 1 (active)
        run_until_texture(&mut state);

        helper_a.crash();
        run_panel(&mut state); // polls all tabs
        assert!(state.tabs[0].session.is_crashed(), "tab 0 crashed");
        assert!(!state.tabs[1].session.is_crashed(), "tab 1 unaffected");
    }

    #[test]
    fn the_ad_filter_blocked_count_surfaces_on_the_active_tab() {
        use mde_web_preview_client::{
            wire, EventMsg, FilterListStore, RequestFilter, ResourceType, WebSession,
        };
        use std::io::Write as _;
        use std::os::unix::net::UnixStream;

        // A bundled-filter session over a bare socketpair (no shm needed — we only
        // drive the request-policy protocol to bump the per-page counter).
        let (shell, helper) = UnixStream::pair().expect("socketpair");
        let filter = RequestFilter::from_store(&FilterListStore::with_bundled());
        let mut session = WebSession::from_stream(shell, None)
            .expect("session")
            .with_filter(filter);

        let mut peer: &UnixStream = &helper;
        let nav = EventMsg::NavState {
            can_back: false,
            can_forward: false,
            loading: false,
            url: "https://news.example.com/".to_owned(),
        };
        peer.write_all(&wire::frame(&nav.encode())).expect("nav");
        let req = EventMsg::ResourceRequest {
            id: 1,
            url: "https://doubleclick.net/ad".to_owned(),
            resource: mde_web_preview_client::resource_to_wire(ResourceType::Image),
        };
        peer.write_all(&wire::frame(&req.encode())).expect("req");
        session.poll();
        assert_eq!(
            session.blocked_count(),
            1,
            "the tracker was blocked + counted"
        );

        let mut state = WebState::default();
        state.tabs.push(Tab {
            session,
            engine: BrowserEngine::Servo,
            container: ContainerProfile::None,
            display_target: DisplayTarget::Current,
            muted: false,
            force_dark: false,
            reader_mode: false,
            last_activity: Instant::now(),
            idle_suspended: false,
            texture: None,
            last_frame: None,
        });
        // The nav chrome (with the "N blocked" shield) renders without panicking.
        assert!(run_panel(&mut state), "the browser chrome produced no draw");
        assert_eq!(state.tabs[0].session.blocked_count(), 1);
    }

    #[test]
    fn per_site_privacy_toggle_changes_real_resource_verdicts() {
        use mde_web_preview_client::{ControlMsg, EventMsg, ResourceType};

        let (shell, helper) = UnixStream::pair().expect("socketpair");
        helper.set_nonblocking(true).expect("helper nonblocking");
        let mut state = WebState::default();
        state.push_session(WebSession::from_stream(shell, None).expect("session"));

        let mut peer: &UnixStream = &helper;
        peer.write_all(&wire::frame(
            &EventMsg::NavState {
                can_back: false,
                can_forward: false,
                loading: false,
                url: "https://news.example.com/".to_owned(),
            }
            .encode(),
        ))
        .expect("nav");
        peer.write_all(&wire::frame(
            &EventMsg::ResourceRequest {
                id: 1,
                url: "https://doubleclick.net/ad".to_owned(),
                resource: mde_web_preview_client::resource_to_wire(ResourceType::Image),
            }
            .encode(),
        ))
        .expect("blocked request");
        state.tabs[0].session.poll();
        assert!(
            drain_control_messages(&helper)
                .into_iter()
                .any(|m| matches!(
                    m,
                    ControlMsg::ResourceVerdict {
                        id: 1,
                        allow: false
                    }
                )),
            "the bundled blocker rejects the tracker before network"
        );

        state.set_active_site_blocking(false);
        peer.write_all(&wire::frame(
            &EventMsg::ResourceRequest {
                id: 2,
                url: "https://doubleclick.net/ad".to_owned(),
                resource: mde_web_preview_client::resource_to_wire(ResourceType::Image),
            }
            .encode(),
        ))
        .expect("allowlisted request");
        state.tabs[0].session.poll();
        assert!(
            drain_control_messages(&helper)
                .into_iter()
                .any(|m| matches!(m, ControlMsg::ResourceVerdict { id: 2, allow: true })),
            "allowlisting the current site changes the actual helper verdict"
        );

        state.set_active_site_blocking(true);
        peer.write_all(&wire::frame(
            &EventMsg::ResourceRequest {
                id: 3,
                url: "https://doubleclick.net/ad".to_owned(),
                resource: mde_web_preview_client::resource_to_wire(ResourceType::Image),
            }
            .encode(),
        ))
        .expect("reblocked request");
        state.tabs[0].session.poll();
        assert!(
            drain_control_messages(&helper)
                .into_iter()
                .any(|m| matches!(
                    m,
                    ControlMsg::ResourceVerdict {
                        id: 3,
                        allow: false
                    }
                )),
            "re-enabling site blocking restores the block verdict"
        );
    }

    #[test]
    fn safe_browsing_hosts_change_real_resource_verdicts() {
        use mde_web_preview_client::{ControlMsg, EventMsg, ResourceType};

        let (shell, helper) = UnixStream::pair().expect("socketpair");
        helper.set_nonblocking(true).expect("helper nonblocking");
        let mut state = WebState::default();
        state.push_session(WebSession::from_stream(shell, None).expect("session"));

        let mut peer: &UnixStream = &helper;
        peer.write_all(&wire::frame(
            &EventMsg::NavState {
                can_back: false,
                can_forward: false,
                loading: false,
                url: "https://news.example.com/".to_owned(),
            }
            .encode(),
        ))
        .expect("nav");
        state.tabs[0].session.poll();
        let _ = drain_control_messages(&helper);

        peer.write_all(&wire::frame(
            &EventMsg::ResourceRequest {
                id: 11,
                url: "https://malware.test/payload.js".to_owned(),
                resource: mde_web_preview_client::resource_to_wire(ResourceType::Script),
            }
            .encode(),
        ))
        .expect("unlisted request");
        state.tabs[0].session.poll();
        assert!(
            drain_control_messages(&helper)
                .into_iter()
                .any(|m| matches!(
                    m,
                    ControlMsg::ResourceVerdict {
                        id: 11,
                        allow: true
                    }
                )),
            "an unlisted host is allowed"
        );

        state.set_safe_browsing_hosts(["malware.test"]);
        peer.write_all(&wire::frame(
            &EventMsg::ResourceRequest {
                id: 12,
                url: "https://cdn.malware.test/payload.js".to_owned(),
                resource: mde_web_preview_client::resource_to_wire(ResourceType::Script),
            }
            .encode(),
        ))
        .expect("listed request");
        state.tabs[0].session.poll();
        assert!(
            drain_control_messages(&helper)
                .into_iter()
                .any(|m| matches!(
                    m,
                    ControlMsg::ResourceVerdict {
                        id: 12,
                        allow: false
                    }
                )),
            "loading the mesh-hosted safe-browsing host blocks the real helper request"
        );
    }

    #[test]
    fn site_data_manager_tracks_committed_first_party_hosts() {
        use mde_web_preview_client::EventMsg;

        let (shell_a, helper_a) = UnixStream::pair().expect("socketpair a");
        let (shell_b, helper_b) = UnixStream::pair().expect("socketpair b");
        let mut state = WebState::default();
        state.push_session(WebSession::from_stream(shell_a, None).expect("session a"));
        state.push_session(WebSession::from_stream(shell_b, None).expect("session b"));

        let mut peer_a: &UnixStream = &helper_a;
        peer_a
            .write_all(&wire::frame(
                &EventMsg::NavState {
                    can_back: false,
                    can_forward: false,
                    loading: false,
                    url: "https://alpha.example/path".to_owned(),
                }
                .encode(),
            ))
            .expect("nav a");
        let mut peer_b: &UnixStream = &helper_b;
        peer_b
            .write_all(&wire::frame(
                &EventMsg::NavState {
                    can_back: false,
                    can_forward: false,
                    loading: false,
                    url: "https://beta.example/path".to_owned(),
                }
                .encode(),
            ))
            .expect("nav b");

        state.tabs[0].session.poll();
        state.tabs[1].session.poll();
        state.update_site_data_from_tabs();

        let summary = state.site_data_summary();
        assert!(summary.contains("2 tracked sites"), "summary = {summary}");
        assert!(summary.contains("2 open tabs"), "summary = {summary}");
    }

    #[test]
    fn clearing_current_tab_records_the_active_site_data_reset() {
        use mde_web_preview_client::EventMsg;

        let (shell, helper) = UnixStream::pair().expect("socketpair");
        let mut state = WebState::default();
        state.push_session(WebSession::from_stream(shell, None).expect("session"));

        let mut peer: &UnixStream = &helper;
        peer.write_all(&wire::frame(
            &EventMsg::NavState {
                can_back: false,
                can_forward: false,
                loading: false,
                url: "https://news.example.com/path".to_owned(),
            }
            .encode(),
        ))
        .expect("nav");
        state.tabs[0].session.poll();
        state.update_site_data_from_tabs();

        state.clear_active_session_data();
        let summary = state.site_data_summary();
        assert!(
            summary.contains("news.example.com cleared 1 time"),
            "summary = {summary}"
        );
    }

    #[test]
    fn custom_filter_rules_compile_into_open_tabs() {
        use mde_web_preview_client::{ControlMsg, EventMsg, ResourceType};

        let (shell, helper) = UnixStream::pair().expect("socketpair");
        helper.set_nonblocking(true).expect("helper nonblocking");
        let mut state = WebState::default();
        state.push_session(WebSession::from_stream(shell, None).expect("session"));
        state.add_custom_filter_rules("TestCustom", "||ads.custom.test^");

        let mut peer: &UnixStream = &helper;
        peer.write_all(&wire::frame(
            &EventMsg::NavState {
                can_back: false,
                can_forward: false,
                loading: false,
                url: "https://publisher.test/".to_owned(),
            }
            .encode(),
        ))
        .expect("nav");
        peer.write_all(&wire::frame(
            &EventMsg::ResourceRequest {
                id: 41,
                url: "https://ads.custom.test/banner.js".to_owned(),
                resource: mde_web_preview_client::resource_to_wire(ResourceType::Script),
            }
            .encode(),
        ))
        .expect("custom request");
        state.tabs[0].session.poll();

        assert!(
            drain_control_messages(&helper)
                .into_iter()
                .any(|m| matches!(
                    m,
                    ControlMsg::ResourceVerdict {
                        id: 41,
                        allow: false
                    }
                )),
            "custom EasyList-style rules are compiled into active tab verdicts"
        );
    }

    #[test]
    fn reload_on_a_crashed_tab_respawns_it() {
        let (session, helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        run_until_texture(&mut state);
        helper.crash();
        run_panel(&mut state);
        assert!(state.tabs[0].session.is_crashed());

        // The Reload button on a crashed tab requests a respawn; the shell swaps in
        // a fresh session (here a new fake helper) and the new page flows again.
        state.respawn_requested = true;
        assert!(state.take_respawn_request());
        let (fresh, _helper2) = testkit::connect().expect("respawn connect");
        state.respawn_active_with(fresh);
        assert!(
            !state.tabs[0].session.is_crashed(),
            "respawned session is live-ish"
        );
        assert!(
            run_until_texture(&mut state),
            "the respawned tab never uploaded a frame"
        );
    }

    // ── MENUBAR-ALL: the Browser bar dispatches its actions to real seams ───────

    #[test]
    fn the_menu_reload_on_a_live_tab_reloads_without_a_respawn() {
        // The View→Reload item on a live tab drives `WebSession::reload` (the same
        // seam the toolbar Reload button uses) and is NOT a respawn.
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        run_until_texture(&mut state);
        let ctx = egui::Context::default();
        super::menubar::apply(&ctx, &mut state, super::menubar::MenuAction::Reload);
        assert!(!state.respawn_requested, "a live reload is not a respawn");
        assert!(!state.tabs[0].session.is_crashed());
    }

    #[test]
    fn the_menu_open_address_loads_the_typed_draft_on_the_live_tab() {
        // Page → Open Typed Address drives `WebSession::load` (the toolbar Go
        // button's exact seam). The fake helper answers a Load with a fresh
        // frame + PaintReady, so observing a new frame proves the menu action
        // reached the live session — not a parallel path (§6).
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        run_until_texture(&mut state); // drain the initial frame
        state.address = "https://example.com/".to_owned();
        let ctx = egui::Context::default();
        super::menubar::apply(&ctx, &mut state, super::menubar::MenuAction::OpenAddress);
        assert!(
            wait_for_fresh_frame(&mut state),
            "OpenAddress reached the helper (a fresh frame arrived for the load)"
        );
    }

    #[test]
    fn the_menu_open_address_uses_the_http_prompt() {
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        run_until_texture(&mut state);
        state.address = "http://plain.example/".to_owned();
        let ctx = egui::Context::default();

        super::menubar::apply(&ctx, &mut state, super::menubar::MenuAction::OpenAddress);

        assert_eq!(
            state.insecure_prompt.as_deref(),
            Some("http://plain.example/")
        );
        assert!(
            !state.tabs[0].session.nav().loading,
            "menu OpenAddress pauses on explicit HTTP just like toolbar Go"
        );
    }

    #[test]
    fn the_privacy_menu_clear_current_tab_returns_to_the_dashboard() {
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        run_until_texture(&mut state);
        state.address = "https://example.com/".to_owned();
        state.submit_address();
        assert!(
            wait_for_fresh_frame(&mut state),
            "precondition: loaded page reached helper"
        );
        state.dashboard_query = "leftover".to_owned();
        state.insecure_prompt = Some("http://plain.example/".to_owned());
        let ctx = egui::Context::default();

        super::menubar::apply(
            &ctx,
            &mut state,
            super::menubar::MenuAction::ClearCurrentTabData,
        );

        assert_eq!(state.address, NEW_TAB_URL);
        assert!(state.dashboard_query.is_empty());
        assert_eq!(state.insecure_prompt, None);
        assert!(
            wait_for_fresh_frame(&mut state),
            "clear action loaded about:blank into the helper"
        );
        assert!(
            run_panel(&mut state),
            "new-tab dashboard renders after clear"
        );
    }

    #[test]
    fn the_menu_reload_on_a_crashed_tab_requests_a_respawn() {
        // On a crashed tab the SAME View→Reload item becomes a respawn request —
        // byte-identical to the toolbar Reload's crashed-tab behaviour.
        let (session, helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        run_until_texture(&mut state);
        helper.crash();
        run_panel(&mut state); // the poll observes the crash
        assert!(state.tabs[0].session.is_crashed());
        let ctx = egui::Context::default();
        super::menubar::apply(&ctx, &mut state, super::menubar::MenuAction::Reload);
        assert!(
            state.respawn_requested,
            "menu Reload on a crashed tab requests a respawn (like the toolbar)"
        );
    }

    // ── BOOKMARKS-10: mesh integration ─────────────────────────────────────────

    #[test]
    fn bookmark_add_body_is_the_workers_add_verb_shape() {
        let body = bookmark_add_body("https://example.com/", "Example Domain");
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["url"], "https://example.com/");
        assert_eq!(v["title"], "Example Domain");
        // `source` is omitted so the worker mints the default `Source::Manual`.
        assert!(v.get("source").is_none());
    }

    #[test]
    fn adfilter_domain_body_matches_the_worker_allow_block_shape() {
        assert_eq!(ACTION_ADFILTER_ALLOW, "action/adfilter/allow");
        assert_eq!(ACTION_ADFILTER_BLOCK, "action/adfilter/block");
        let body = adfilter_domain_body(" news.example.com ");
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["domain"], "news.example.com");
    }

    #[test]
    fn chat_share_body_round_trips_into_a_clipboard_message_kind() {
        let body = chat_share_body("eagle", "https://example.com/", "Example Domain");
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["scope"], "peer");
        assert_eq!(v["to"], "eagle");
        // Prove it's the REAL NOTIFY-CHAT message-kind (snake_case-tagged), not a
        // hand-rolled shape: the `kind` deserializes straight back into MessageKind.
        let kind: MessageKind = serde_json::from_value(v["kind"].clone()).expect("a MessageKind");
        assert!(matches!(kind, MessageKind::Clipboard { .. }));
        assert_eq!(v["kind"]["clipboard"]["preview"], "Example Domain");
        assert_eq!(v["kind"]["clipboard"]["full"], "https://example.com/");
    }

    #[test]
    fn chat_share_preview_falls_back_to_the_url_when_the_title_is_blank() {
        let body = chat_share_body("eagle", "https://example.com/", "   ");
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["kind"]["clipboard"]["preview"], "https://example.com/");
    }

    #[test]
    fn browser_share_body_is_the_platform_handoff_shape() {
        let body = browser_share_body(
            BrowserShareTarget::Email,
            "https://example.com/",
            "Example Domain",
        );
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["op"], "browser_share");
        assert_eq!(v["target"], "email");
        assert_eq!(v["url"], "https://example.com/");
        assert_eq!(v["title"], "Example Domain");
        assert_eq!(v["preview"], "Example Domain");
        assert_eq!(v["source"], "browser");
        assert!(v["host"].as_str().is_some_and(|host| !host.is_empty()));
    }

    #[test]
    fn browser_share_preview_falls_back_to_the_url_when_the_title_is_blank() {
        let body = browser_share_body(BrowserShareTarget::Qr, "https://example.com/", "   ");
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["target"], "qr");
        assert_eq!(v["title"], "");
        assert_eq!(v["preview"], "https://example.com/");
    }

    #[test]
    fn browser_send_tab_body_is_the_follow_me_handoff_shape() {
        assert_eq!(ACTION_BROWSER_SEND_TAB, "action/browser/send-tab");
        let body = browser_send_tab_body(
            BrowserSendTabTarget::Phone,
            BrowserEngine::Cef,
            "https://example.com/",
            "Example Domain",
        );
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["op"], "browser_send_tab");
        assert_eq!(v["target"], "phone");
        assert_eq!(v["engine"], "cef");
        assert_eq!(v["url"], "https://example.com/");
        assert_eq!(v["title"], "Example Domain");
        assert_eq!(v["preview"], "Example Domain");
        assert_eq!(v["source"], "browser");
        assert!(v["host"].as_str().is_some_and(|host| !host.is_empty()));
    }

    #[test]
    fn browser_send_tab_preview_falls_back_to_the_url_when_the_title_is_blank() {
        let body = browser_send_tab_body(
            BrowserSendTabTarget::Node,
            BrowserEngine::Servo,
            "https://example.com/",
            "   ",
        );
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["target"], "node");
        assert_eq!(v["target_id"], local_hostname());
        assert_eq!(v["target_label"], local_hostname());
        assert_eq!(v["engine"], "servo");
        assert_eq!(v["title"], "");
        assert_eq!(v["preview"], "https://example.com/");
    }

    #[test]
    fn browser_session_sync_body_carries_tabs_settings_and_downloads() {
        assert_eq!(ACTION_BROWSER_SESSION_SYNC, "action/browser/session-sync");
        let (session, _helper, _writer) = live_page_session();
        let mut state = WebState::default();
        state.push_session(session);
        run_until_texture(&mut state);
        state.tabs[state.active].container = ContainerProfile::Work;
        state.tabs[state.active].display_target = DisplayTarget::Secondary;
        state.tabs[state.active].muted = true;
        state.tabs[state.active].force_dark = true;
        state.vertical_tabs = true;
        state.page_zoom_percent = 125;
        state.downloads_open = true;
        state.speed_dial = vec![SpeedDialEntry::new(
            "Ops",
            "https://ops.mesh/",
            "Open the mesh ops console",
        )];
        let mut job = browser_output_transfer_job("/tmp/source.bin", "/tmp/dest.bin");
        job.state = TransferState::Running;
        job.progress = Some(42);
        state.download_jobs.push(job);

        let body = browser_session_sync_body(&state);
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(v["op"], "browser_session_sync");
        assert_eq!(v["source"], "browser");
        assert_eq!(v["active_index"], 0);
        assert_eq!(v["settings"]["vertical_tabs"], true);
        assert_eq!(v["settings"]["page_zoom_percent"], 125);
        assert_eq!(v["settings"]["downloads_open"], true);
        assert_eq!(v["settings"]["speed_dial"][0]["label"], "Ops");
        assert_eq!(v["settings"]["speed_dial"][0]["url"], "https://ops.mesh/");
        assert_eq!(
            v["settings"]["speed_dial"][0]["hint"],
            "Open the mesh ops console"
        );
        assert_eq!(v["tabs"][0]["engine"], "servo");
        assert_eq!(v["tabs"][0]["container"], "work");
        assert_eq!(v["tabs"][0]["display_target"], "secondary");
        assert_eq!(v["tabs"][0]["url"], "https://example.test/");
        assert_eq!(v["tabs"][0]["muted"], true);
        assert_eq!(v["tabs"][0]["force_dark"], true);
        assert_eq!(v["downloads"][0]["method"], "browser_download");
        assert_eq!(v["downloads"][0]["state"], "running");
        assert_eq!(v["downloads"][0]["progress"], 42);
    }

    #[test]
    fn browser_session_restore_enqueues_tabs_with_the_active_tab_last() {
        let body = serde_json::json!({
            "op": "browser_session_sync",
            "active_index": 1,
            "settings": {
                "future_engine": "cef",
                "vertical_tabs": true,
                "page_zoom_percent": 135,
                "find_open": true,
                "downloads_open": true,
                "speed_dial": [
                    {"label": "Ops", "url": "https://ops.mesh/", "hint": "Open ops"},
                    {"label": "", "url": "https://drop.example/", "hint": "drop"},
                    {"label": "No URL", "url": "", "hint": "drop"}
                ],
            },
            "tabs": [
                {"index": 0, "engine": "servo", "url": "https://first.example/"},
                {"index": 1, "engine": "cef", "url": "https://active.example/"},
                {"index": 2, "engine": "servo", "url": "https://last.example/"},
            ],
            "downloads": [],
        })
        .to_string();
        let mut state = WebState::default();

        let restored = state
            .restore_session_sync_snapshot(&body)
            .expect("restore snapshot");

        assert_eq!(restored, 3);
        assert_eq!(state.engine, BrowserEngine::Cef);
        assert!(state.vertical_tabs);
        assert_eq!(state.page_zoom_percent, 135);
        assert!(state.find_open);
        assert!(state.downloads_open);
        assert_eq!(
            state.speed_dial,
            vec![SpeedDialEntry::new("Ops", "https://ops.mesh/", "Open ops")]
        );
        assert_eq!(
            state.take_open_request(),
            Some(TabOpenIntent::NewForegroundUrl {
                engine: BrowserEngine::Servo,
                url: "https://first.example/".to_owned(),
            })
        );
        assert_eq!(
            state.take_open_request(),
            Some(TabOpenIntent::NewForegroundUrl {
                engine: BrowserEngine::Servo,
                url: "https://last.example/".to_owned(),
            })
        );
        assert_eq!(
            state.take_open_request(),
            Some(TabOpenIntent::NewForegroundUrl {
                engine: BrowserEngine::Cef,
                url: "https://active.example/".to_owned(),
            })
        );
        assert_eq!(state.take_open_request(), None);
    }

    #[test]
    fn browser_session_restore_rejects_the_wrong_snapshot_shape() {
        let mut state = WebState::default();
        assert!(state.restore_session_sync_snapshot("{}").is_err());
        assert!(state
            .restore_session_sync_snapshot(r#"{"op":"browser_send_tab","tabs":[]}"#)
            .is_err());
    }

    #[test]
    fn browser_startup_restore_reads_daemon_latest_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let host = local_hostname();
        let path = session_sync_latest_path(root.path(), &host);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::json!({
                "op": "browser_session_sync",
                "active_index": 0,
                "settings": {
                    "future_engine": "cef",
                    "speed_dial": [
                        {"label": "Ops", "url": "https://ops.mesh/", "hint": "Open ops"}
                    ],
                },
                "tabs": [
                    {"index": 0, "engine": "cef", "url": "https://restored.mesh/"}
                ],
                "downloads": [],
            })
            .to_string(),
        )
        .unwrap();
        let mut state =
            WebState::default().with_session_restore_roots(vec![root.path().to_path_buf()]);

        assert_eq!(state.restore_startup_session_once(), Some(1));

        assert_eq!(state.engine, BrowserEngine::Cef);
        assert_eq!(
            state.speed_dial,
            vec![SpeedDialEntry::new("Ops", "https://ops.mesh/", "Open ops")]
        );
        assert_eq!(
            state.take_open_request(),
            Some(TabOpenIntent::NewForegroundUrl {
                engine: BrowserEngine::Cef,
                url: "https://restored.mesh/".to_owned(),
            })
        );
        assert_eq!(state.restore_startup_session_once(), None);
    }

    #[test]
    fn browser_startup_restore_host_path_matches_the_daemon_sanitizer() {
        assert_eq!(sanitize_session_host("work station/1"), "work-station1");
        assert_eq!(
            session_sync_latest_path(Path::new("/mesh"), "work station/1"),
            PathBuf::from("/mesh/browser-session-sync/work-station1/latest.json")
        );
        assert_eq!(
            send_tab_inbox_dir(Path::new("/mesh"), "work station/1"),
            PathBuf::from("/mesh/browser-send-tab/node/work-station1")
        );
    }

    #[test]
    fn browser_send_tab_outbox_enqueues_local_node_tabs_and_unlinks_records() {
        let root = tempfile::tempdir().unwrap();
        let host = local_hostname();
        let path = send_tab_inbox_dir(root.path(), &host)
            .join("source-node")
            .join("01Send.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::json!({
                "op": "browser_send_tab",
                "target": "node",
                "target_id": host,
                "target_label": host,
                "engine": "cef",
                "url": "https://handoff.mesh/",
                "title": "Handoff",
                "preview": "Handoff",
                "source": "browser",
                "host": "source-node"
            })
            .to_string(),
        )
        .unwrap();
        let mut state =
            WebState::default().with_session_restore_roots(vec![root.path().to_path_buf()]);

        assert_eq!(state.drain_incoming_send_tabs(), 1);

        assert_eq!(
            state.take_open_request(),
            Some(TabOpenIntent::NewForegroundUrl {
                engine: BrowserEngine::Cef,
                url: "https://handoff.mesh/".to_owned(),
            })
        );
        assert!(
            !path.exists(),
            "consumed send-tab records are unlinked so they do not replay"
        );
    }

    #[test]
    fn browser_send_tab_outbox_rejects_phone_and_other_node_records() {
        let root = tempfile::tempdir().unwrap();
        let host = local_hostname();
        let local_dir = send_tab_inbox_dir(root.path(), &host).join("source-node");
        std::fs::create_dir_all(&local_dir).unwrap();
        std::fs::write(
            local_dir.join("phone.json"),
            serde_json::json!({
                "op": "browser_send_tab",
                "target": "phone",
                "target_id": host,
                "engine": "cef",
                "url": "https://phone.mesh/"
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            local_dir.join("other.json"),
            serde_json::json!({
                "op": "browser_send_tab",
                "target": "node",
                "target_id": "other-node",
                "engine": "servo",
                "url": "https://other.mesh/"
            })
            .to_string(),
        )
        .unwrap();
        let mut state =
            WebState::default().with_session_restore_roots(vec![root.path().to_path_buf()]);

        assert_eq!(state.drain_incoming_send_tabs(), 0);
        assert_eq!(state.take_open_request(), None);
        assert!(local_dir.join("phone.json").exists());
        assert!(local_dir.join("other.json").exists());
    }

    #[test]
    fn browser_send_tab_outbox_dedupes_local_and_shared_records() {
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        let host = local_hostname();
        let body = serde_json::json!({
            "op": "browser_send_tab",
            "target": "node",
            "target_id": host,
            "engine": "servo",
            "url": "https://dedupe.mesh/",
            "host": "source-node"
        })
        .to_string();
        let local_path = send_tab_inbox_dir(local.path(), &host)
            .join("source-node")
            .join("01Same.json");
        let share_path = send_tab_inbox_dir(share.path(), &host)
            .join("source-node")
            .join("01Same.json");
        std::fs::create_dir_all(local_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(share_path.parent().unwrap()).unwrap();
        std::fs::write(&local_path, &body).unwrap();
        std::fs::write(&share_path, &body).unwrap();
        let mut state = WebState::default().with_session_restore_roots(vec![
            local.path().to_path_buf(),
            share.path().to_path_buf(),
        ]);

        assert_eq!(state.drain_incoming_send_tabs(), 1);

        assert_eq!(
            state.take_open_request(),
            Some(TabOpenIntent::NewForegroundUrl {
                engine: BrowserEngine::Servo,
                url: "https://dedupe.mesh/".to_owned(),
            })
        );
        assert_eq!(state.take_open_request(), None);
        assert!(!local_path.exists());
        assert!(!share_path.exists());
    }

    #[test]
    fn omnibox_resolves_urls_hosts_and_searches() {
        assert_eq!(
            omnibox_target(" https://example.com/a "),
            Some("https://example.com/a".to_owned())
        );
        assert_eq!(
            omnibox_target("about:blank"),
            Some("about:blank".to_owned())
        );
        assert_eq!(
            omnibox_target("data:text/html,hi"),
            Some("data:text/html,hi".to_owned())
        );
        assert_eq!(
            omnibox_target("example.com"),
            Some("https://example.com".to_owned())
        );
        assert_eq!(
            omnibox_target("localhost:8080/admin"),
            Some("https://localhost:8080/admin".to_owned())
        );
        assert_eq!(
            omnibox_target("10.42.0.1:4533"),
            Some("https://10.42.0.1:4533".to_owned())
        );
        assert_eq!(
            omnibox_target("mesh browser status"),
            Some("https://search.mesh/search?q=mesh+browser+status".to_owned())
        );
        assert_eq!(
            omnibox_target("a+b & c"),
            Some("https://search.mesh/search?q=a%2Bb+%26+c".to_owned())
        );
        assert_eq!(omnibox_target("  "), None);
    }

    #[test]
    fn external_tel_urls_handoff_to_voice_without_helper_navigation() {
        let bus = tempfile::tempdir().expect("temp bus");
        let (session, helper, _writer) = live_page_session();
        let mut state = WebState::default().with_bus_root(Some(bus.path().to_path_buf()));
        state.push_session(session);
        run_until_texture(&mut state);
        let _ = drain_control_messages(&helper);

        state.address = "tel:+15551234567".to_owned();
        state.submit_address();

        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let msgs = persist
            .list_since(ACTION_VOICE_DIAL, None)
            .expect("list voice dial actions");
        assert_eq!(msgs.len(), 1);
        let body = msgs[0].body.as_deref().expect("dial body");
        let v: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
        assert_eq!(v["peer"], "+15551234567");
        assert_eq!(v["source"], "browser");
        assert_eq!(v["url"], "tel:+15551234567");
        assert_eq!(
            state.capture_notice.as_deref(),
            Some("Handed tel link to Voice")
        );

        let controls = drain_control_messages(&helper);
        assert!(
            controls
                .iter()
                .all(|msg| !matches!(msg, mde_web_preview_client::ControlMsg::Load(_))),
            "external protocol handoff must not navigate the helper: {controls:?}"
        );
    }

    #[test]
    fn mailto_and_magnet_urls_publish_browser_protocol_handoffs() {
        let bus = tempfile::tempdir().expect("temp bus");
        let (session, helper, _writer) = live_page_session();
        let mut state = WebState::default().with_bus_root(Some(bus.path().to_path_buf()));
        state.push_session(session);
        run_until_texture(&mut state);
        let _ = drain_control_messages(&helper);

        state.address = "mailto:ops@example.test?subject=mesh".to_owned();
        state.submit_address();
        state.address = "magnet:?xt=urn:btih:0123456789abcdef".to_owned();
        state.submit_address();

        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let msgs = persist
            .list_since(ACTION_BROWSER_PROTOCOL, None)
            .expect("list browser protocol actions");
        assert_eq!(msgs.len(), 2);

        let mail: serde_json::Value =
            serde_json::from_str(msgs[0].body.as_deref().expect("mailto body"))
                .expect("valid JSON");
        assert_eq!(mail["op"], "browser_protocol_handoff");
        assert_eq!(mail["scheme"], "mailto");
        assert_eq!(mail["target"], "email");
        assert_eq!(mail["url"], "mailto:ops@example.test?subject=mesh");

        let magnet: serde_json::Value =
            serde_json::from_str(msgs[1].body.as_deref().expect("magnet body"))
                .expect("valid JSON");
        assert_eq!(magnet["scheme"], "magnet");
        assert_eq!(magnet["target"], "transfers");
        assert_eq!(magnet["url"], "magnet:?xt=urn:btih:0123456789abcdef");
        assert_eq!(
            state.capture_notice.as_deref(),
            Some("Handed magnet link to Transfers")
        );

        let controls = drain_control_messages(&helper);
        assert!(
            controls
                .iter()
                .all(|msg| !matches!(msg, mde_web_preview_client::ControlMsg::Load(_))),
            "external protocol handoff must not navigate the helper: {controls:?}"
        );
    }

    #[test]
    fn browser_share_menu_actions_publish_platform_handoffs() {
        let bus = tempfile::tempdir().expect("temp bus");
        let (session, _helper, _writer) = live_page_session();
        let mut state = WebState::default().with_bus_root(Some(bus.path().to_path_buf()));
        state.push_session(session);
        run_until_texture(&mut state);
        let ctx = egui::Context::default();

        super::menubar::apply(&ctx, &mut state, super::menubar::MenuAction::ShareToPeer);
        super::menubar::apply(&ctx, &mut state, super::menubar::MenuAction::ShareToEmail);
        super::menubar::apply(&ctx, &mut state, super::menubar::MenuAction::ShareToQr);

        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let msgs = persist
            .list_since(ACTION_BROWSER_SHARE, None)
            .expect("list browser share actions");
        assert_eq!(msgs.len(), 3);
        let targets: Vec<String> = msgs
            .iter()
            .map(|msg| {
                let body = msg.body.as_deref().expect("share body");
                let v: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
                assert_eq!(v["op"], "browser_share");
                assert_eq!(v["source"], "browser");
                assert_eq!(v["url"], "https://example.test/");
                v["target"].as_str().expect("target").to_owned()
            })
            .collect();
        assert_eq!(targets, ["peer", "email", "qr"]);
    }

    #[test]
    fn browser_send_tab_menu_actions_publish_follow_me_handoffs() {
        let bus = tempfile::tempdir().expect("temp bus");
        let (session, _helper, _writer) = live_page_session();
        let mut state = WebState::default().with_bus_root(Some(bus.path().to_path_buf()));
        state.push_session(session);
        state.tabs[state.active].engine = BrowserEngine::Cef;
        run_until_texture(&mut state);
        let ctx = egui::Context::default();

        super::menubar::apply(&ctx, &mut state, super::menubar::MenuAction::SendTabToNode);
        super::menubar::apply(&ctx, &mut state, super::menubar::MenuAction::SendTabToPhone);

        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let msgs = persist
            .list_since(ACTION_BROWSER_SEND_TAB, None)
            .expect("list browser send-tab actions");
        assert_eq!(msgs.len(), 2);
        let targets: Vec<String> = msgs
            .iter()
            .map(|msg| {
                let body = msg.body.as_deref().expect("send-tab body");
                let v: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
                assert_eq!(v["op"], "browser_send_tab");
                assert_eq!(v["source"], "browser");
                assert_eq!(v["engine"], "cef");
                assert_eq!(v["url"], "https://example.test/");
                if v["target"] == "node" {
                    assert_eq!(v["target_id"], local_hostname());
                    assert_eq!(v["target_label"], local_hostname());
                }
                v["target"].as_str().expect("target").to_owned()
            })
            .collect();
        assert_eq!(targets, ["node", "phone"]);
    }

    #[test]
    fn browser_session_sync_publishes_once_until_the_snapshot_changes() {
        let bus = tempfile::tempdir().expect("temp bus");
        let (session, _helper, _writer) = live_page_session();
        let mut state = WebState::default().with_bus_root(Some(bus.path().to_path_buf()));
        state.push_session(session);
        state.publish_session_snapshot();
        state.publish_session_snapshot();

        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let msgs = persist
            .list_since(ACTION_BROWSER_SESSION_SYNC, None)
            .expect("list browser session sync");
        assert_eq!(msgs.len(), 1, "unchanged snapshots are de-duped");

        state.set_vertical_tabs(true);
        state.publish_session_snapshot();
        let msgs = persist
            .list_since(ACTION_BROWSER_SESSION_SYNC, None)
            .expect("list browser session sync after change");
        assert_eq!(msgs.len(), 2, "a changed setting emits a new snapshot");
        let latest: serde_json::Value =
            serde_json::from_str(msgs[1].body.as_deref().expect("sync body")).expect("valid JSON");
        assert_eq!(latest["settings"]["vertical_tabs"], true);
    }

    #[test]
    fn browser_capture_success_publishes_notify_feed_event() {
        let bus = tempfile::tempdir().expect("temp bus");
        let mut state = WebState::default().with_bus_root(Some(bus.path().to_path_buf()));
        let path = PathBuf::from("/tmp/mde-browser-capture.png");

        state.record_capture_success("Captured", &path);

        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let msgs = persist
            .list_since(EVENT_NOTIFY_BROWSER, None)
            .expect("list browser notify events");
        assert_eq!(msgs.len(), 1);
        let body = msgs[0].body.as_deref().expect("notify body");
        let v: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
        assert_eq!(v["severity"], "info");
        assert_eq!(v["source"], "browser");
        assert_eq!(v["summary"], "Captured /tmp/mde-browser-capture.png");
        assert_eq!(v["detail"], "/tmp/mde-browser-capture.png");
        assert_eq!(v["action"], "action/shell/goto/browser");
    }

    #[test]
    fn suggestions_only_fetch_for_plain_search_drafts() {
        assert!(should_fetch_suggestions("mesh browser"));
        assert!(!should_fetch_suggestions("https://example.com"));
        assert!(!should_fetch_suggestions("example.com"));
        assert!(!should_fetch_suggestions("localhost:8080"));
        assert!(!should_fetch_suggestions("about:blank"));
        assert!(!should_fetch_suggestions("   "));
        assert_eq!(
            suggestions_url("a+b & c"),
            "https://search.mesh/autocompleter?q=a%2Bb+%26+c"
        );
    }

    #[test]
    fn suggestions_parser_accepts_searxng_and_opensearch_shapes() {
        assert_eq!(
            parse_suggestions_json("mesh", r#"["mesh",["mesh network","mesh browser"]]"#)
                .expect("opensearch shape"),
            ["mesh network".to_owned(), "mesh browser".to_owned()]
        );
        assert_eq!(
            parse_suggestions_json(
                "mesh",
                r#"{"suggestions":["mesh browser",{"value":"mesh terminal"},{"text":"mesh files"}]}"#
            )
            .expect("object shape"),
            [
                "mesh browser".to_owned(),
                "mesh terminal".to_owned(),
                "mesh files".to_owned()
            ]
        );
        assert_eq!(
            parse_suggestions_json("mesh", r#"["mesh","mesh browser","mesh browser",""]"#)
                .expect("dedupe"),
            ["mesh browser".to_owned()]
        );
    }

    #[test]
    fn accepting_a_suggestion_uses_the_normal_omnibox_load_path() {
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        assert!(run_until_texture(&mut state));

        state.accept_suggestion("mesh browser".to_owned());

        assert_eq!(state.address, "https://search.mesh/search?q=mesh+browser");
        assert!(
            wait_for_fresh_frame(&mut state),
            "accepted suggestion reached the helper through submit_address"
        );
    }

    #[derive(Clone, Default)]
    struct RecordingTransfers {
        jobs: std::sync::Arc<std::sync::Mutex<Vec<TransferJob>>>,
        verbs: std::sync::Arc<std::sync::Mutex<Vec<TransferVerb>>>,
    }

    impl RecordingTransfers {
        fn with_jobs(jobs: Vec<TransferJob>) -> Self {
            Self {
                jobs: std::sync::Arc::new(std::sync::Mutex::new(jobs)),
                verbs: std::sync::Arc::default(),
            }
        }

        fn verbs(&self) -> Vec<TransferVerb> {
            self.verbs.lock().unwrap().clone()
        }

        fn set_jobs(&self, jobs: Vec<TransferJob>) {
            *self.jobs.lock().unwrap() = jobs;
        }
    }

    impl TransfersClient for RecordingTransfers {
        fn jobs(&self) -> Vec<TransferJob> {
            self.jobs.lock().unwrap().clone()
        }

        fn worker_present(&self) -> bool {
            true
        }

        fn dispatch(&self, verb: &TransferVerb) -> Result<(), String> {
            self.verbs.lock().unwrap().push(verb.clone());
            Ok(())
        }
    }

    #[test]
    fn browser_download_enqueue_submits_a_verified_browser_transfer() {
        let transfers = RecordingTransfers::default();
        let id = enqueue_browser_output(&transfers, "/tmp/helper/file.bin", "/home/mm/Downloads")
            .expect("enqueue");
        let verbs = transfers.verbs();
        assert_eq!(verbs.len(), 1);
        let TransferVerb::Submit(job) = &verbs[0] else {
            panic!("expected submit");
        };
        assert_eq!(job.id, id);
        assert_eq!(job.source, "/tmp/helper/file.bin");
        assert_eq!(job.dest, "/home/mm/Downloads");
        assert_eq!(job.method, TransferMethod::BrowserDownload);
        assert!(job.policy.verify, "browser outputs should be verified");
    }

    #[test]
    fn scraper_output_batch_enqueues_one_transfer_per_file() {
        let transfers = RecordingTransfers::default();
        let ids = enqueue_browser_output_batch(
            &transfers,
            &[
                "/tmp/scrape/page.json".to_owned(),
                "/tmp/scrape/page.md".to_owned(),
            ],
            "/home/mm/Exports",
        )
        .expect("enqueue batch");
        assert_eq!(ids.len(), 2);
        let verbs = transfers.verbs();
        assert_eq!(verbs.len(), 2);
        for verb in verbs.iter() {
            let TransferVerb::Submit(job) = verb else {
                panic!("expected submit");
            };
            assert_eq!(job.method, TransferMethod::BrowserDownload);
            assert_eq!(job.dest, "/home/mm/Exports");
            assert!(job.policy.verify);
        }
    }

    fn transfer_fixture(
        id: &str,
        method: TransferMethod,
        state: TransferState,
        updated_ms: u64,
    ) -> TransferJob {
        let mut job = TransferJob::new(
            format!("/tmp/{id}.bin"),
            "/home/mm/Downloads",
            method,
            TransferPolicy {
                bwlimit: None,
                verify: true,
            },
        );
        job.id = id.to_owned();
        job.state = state;
        job.progress = if state == TransferState::Running {
            Some(42)
        } else {
            None
        };
        job.created_ms = updated_ms.saturating_sub(10);
        job.updated_ms = updated_ms;
        job
    }

    #[test]
    fn browser_download_manager_filters_and_dispatches_shared_transfer_jobs() {
        let running = transfer_fixture(
            "browser-running",
            TransferMethod::BrowserDownload,
            TransferState::Running,
            30,
        );
        let paused = transfer_fixture(
            "browser-paused",
            TransferMethod::BrowserDownload,
            TransferState::Paused,
            40,
        );
        let done = transfer_fixture(
            "browser-done",
            TransferMethod::BrowserDownload,
            TransferState::Done,
            50,
        );
        let http = transfer_fixture("http", TransferMethod::Http, TransferState::Running, 60);
        let transfers = RecordingTransfers::with_jobs(vec![done, http, paused, running]);
        let mut state = WebState::default().with_transfers(Box::new(transfers.clone()));

        let ids = state
            .download_jobs
            .iter()
            .map(|job| job.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["browser-running", "browser-paused", "browser-done"]);
        assert_eq!(state.download_counts(), (2, 3));

        state.dispatch_download_verb(TransferVerb::Pause("browser-running".to_owned()));
        state.dispatch_download_verb(TransferVerb::Resume("browser-paused".to_owned()));
        state.dispatch_download_verb(TransferVerb::Cancel("browser-done".to_owned()));

        assert_eq!(
            transfers.verbs(),
            [
                TransferVerb::Pause("browser-running".to_owned()),
                TransferVerb::Resume("browser-paused".to_owned()),
                TransferVerb::Cancel("browser-done".to_owned())
            ]
        );
    }

    #[test]
    fn browser_download_completion_publishes_notify_feed_event_once() {
        let bus = tempfile::tempdir().expect("temp bus");
        let running = transfer_fixture(
            "browser-running",
            TransferMethod::BrowserDownload,
            TransferState::Running,
            10,
        );
        let done = transfer_fixture(
            "browser-running",
            TransferMethod::BrowserDownload,
            TransferState::Done,
            20,
        );
        let transfers = RecordingTransfers::with_jobs(vec![running]);
        let mut state = WebState::default()
            .with_bus_root(Some(bus.path().to_path_buf()))
            .with_transfers(Box::new(transfers.clone()));

        transfers.set_jobs(vec![done]);
        state.refresh_downloads();
        state.refresh_downloads();

        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let msgs = persist
            .list_since(EVENT_NOTIFY_BROWSER, None)
            .expect("list browser notify events");
        assert_eq!(msgs.len(), 1, "download completion is announced once");
        let body = msgs[0].body.as_deref().expect("notify body");
        let v: serde_json::Value = serde_json::from_str(body).expect("valid JSON");
        assert_eq!(v["severity"], "info");
        assert_eq!(v["source"], "browser");
        assert_eq!(
            v["summary"],
            "Browser download complete: browser-running.bin"
        );
        assert_eq!(
            v["detail"],
            "/tmp/browser-running.bin -> /home/mm/Downloads"
        );
        assert_eq!(v["action"], "action/shell/goto/browser");
    }

    #[test]
    fn the_live_page_url_and_title_feed_the_page_actions() {
        let (session, _helper) = testkit::connect().expect("connect");
        let mut state = WebState::default();
        state.push_session(session);
        run_until_texture(&mut state);
        // The three page-actions all read the active session's live nav + title
        // (the testkit helper reports `about:blank` for both).
        let tab = &state.tabs[0];
        assert_eq!(tab.session.nav().url, "about:blank");
        assert_eq!(tab.session.title(), "about:blank");
        // The chrome now carrying the page-actions star menu still renders.
        assert!(
            run_panel(&mut state),
            "the page-actions chrome produced no draw"
        );
    }

    // ── live-helper: the real spawn/attach/pump glue ────────────────────────────
    //
    // Exercised through the SAME `open_with` seam the shell's Browser arm drives,
    // but with a `testkit` factory injected in place of the real `Command::spawn`,
    // so the spawn→attach→pump path runs hermetically — no Servo, no real process.

    #[cfg(feature = "live-helper")]
    #[test]
    fn live_open_spawns_attaches_and_pumps_a_frame() {
        use std::cell::RefCell;
        // Hold the fake helpers so the attached session stays live through the pump.
        let helpers: RefCell<Vec<testkit::FakeHelper>> = RefCell::new(Vec::new());
        let mut state = WebState::default();
        // A real, existing path passes the "helper installed" gate; the injected
        // factory returns a testkit session instead of exec'ing Servo.
        let bin = std::env::current_exe().expect("test exe path");
        state.open_with(
            true,
            BrowserEngine::Servo,
            START_URL.to_owned(),
            bin,
            |_spec| {
                let (session, helper) = testkit::connect()?;
                helpers.borrow_mut().push(helper);
                Ok(session)
            },
        );
        assert_eq!(
            state.tabs.len(),
            1,
            "the live open attached exactly one tab"
        );
        assert!(
            state.gate_notice.is_none(),
            "a successful open clears the gate notice"
        );
        assert!(
            run_until_texture(&mut state),
            "the live tab pumped no frame into the texture path"
        );
        assert!(state.tabs[0].texture.is_some());
    }

    #[cfg(feature = "live-helper")]
    #[test]
    fn live_open_with_no_seat_stays_the_honest_empty_state() {
        use std::cell::Cell;
        let spawned = Cell::new(false);
        let mut state = WebState::default();
        let bin = std::env::current_exe().expect("test exe path");
        state.open_with(
            false,
            BrowserEngine::Servo,
            START_URL.to_owned(),
            bin,
            |_spec| {
                spawned.set(true);
                Err(std::io::Error::other(
                    "factory must not be called without a seat",
                ))
            },
        );
        assert!(!spawned.get(), "no seat must not spawn a helper");
        assert!(state.tabs.is_empty(), "no tab attaches without a seat");
        assert!(
            state.gate_notice.is_some(),
            "the no-seat gate is named honestly"
        );
        // The panel draws the honest gated EmptyState, never a fake page.
        assert!(run_panel(&mut state));
    }

    #[cfg(feature = "live-helper")]
    #[test]
    fn live_open_with_an_absent_helper_stays_the_honest_empty_state() {
        use std::cell::Cell;
        let spawned = Cell::new(false);
        let mut state = WebState::default();
        let missing = std::path::PathBuf::from("/nonexistent/mde-web-preview-xyz");
        state.open_with(
            true,
            BrowserEngine::Servo,
            START_URL.to_owned(),
            missing,
            |_spec| {
                spawned.set(true);
                Err(std::io::Error::other(
                    "factory must not run with an absent helper",
                ))
            },
        );
        assert!(!spawned.get(), "an absent helper binary must not spawn");
        assert!(state.tabs.is_empty());
        let notice = state.gate_notice.as_deref().unwrap_or_default();
        assert!(
            notice.contains("not installed"),
            "the absent-helper gate names it honestly: {notice}"
        );
        assert!(run_panel(&mut state));
    }

    #[cfg(feature = "live-helper")]
    #[test]
    fn a_spawn_failure_surfaces_an_honest_reason_not_a_hang() {
        let mut state = WebState::default();
        let bin = std::env::current_exe().expect("test exe path");
        state.open_with(
            true,
            BrowserEngine::Servo,
            START_URL.to_owned(),
            bin,
            |_spec| Err(std::io::Error::other("exec denied by sandbox")),
        );
        assert!(state.tabs.is_empty(), "a failed spawn attaches no tab");
        let notice = state.gate_notice.as_deref().unwrap_or_default();
        assert!(
            notice.contains("failed to start") && notice.contains("exec denied"),
            "a spawn failure surfaces its reason: {notice}"
        );
        assert!(run_panel(&mut state), "the honest failure notice draws");
    }

    #[cfg(feature = "live-helper")]
    #[test]
    fn helper_bin_path_defaults_and_honors_engine_env_overrides() {
        use std::path::PathBuf;
        std::env::remove_var(SERVO_HELPER_BIN_ENV);
        std::env::remove_var(CEF_HELPER_BIN_ENV);
        assert_eq!(
            helper_bin_path(BrowserEngine::Servo),
            PathBuf::from(DEFAULT_SERVO_HELPER_BIN)
        );
        assert_eq!(
            helper_bin_path(BrowserEngine::Cef),
            PathBuf::from(DEFAULT_CEF_HELPER_BIN)
        );
        std::env::set_var(SERVO_HELPER_BIN_ENV, "/opt/mde/mde-web-preview");
        std::env::set_var(CEF_HELPER_BIN_ENV, "/opt/mde/mde-web-cef");
        assert_eq!(
            helper_bin_path(BrowserEngine::Servo),
            PathBuf::from("/opt/mde/mde-web-preview")
        );
        assert_eq!(
            helper_bin_path(BrowserEngine::Cef),
            PathBuf::from("/opt/mde/mde-web-cef")
        );
        std::env::remove_var(SERVO_HELPER_BIN_ENV);
        std::env::remove_var(CEF_HELPER_BIN_ENV);
    }

    #[cfg(feature = "live-helper")]
    #[test]
    fn default_engine_prefers_cef_only_when_helper_and_runtime_are_installed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let helper = dir.path().join("mde-web-cef");
        let runtime = dir.path().join("cef");
        std::fs::write(&helper, b"helper").expect("helper marker");
        std::fs::create_dir_all(runtime.join(CEF_RELEASE_DIR)).expect("release dir");
        std::fs::create_dir_all(runtime.join(CEF_RESOURCES_DIR)).expect("resources dir");
        std::fs::write(runtime.join(CEF_RELEASE_DIR).join(CEF_LIB_NAME), b"cef")
            .expect("libcef marker");
        std::fs::write(runtime.join(CEF_RESOURCES_DIR).join(CEF_ICU_DATA), b"icu")
            .expect("icu marker");
        std::fs::write(
            runtime.join(CEF_RESOURCES_DIR).join(CEF_RESOURCES_PAK),
            b"pak",
        )
        .expect("pak marker");

        std::env::set_var(CEF_HELPER_BIN_ENV, &helper);
        std::env::set_var(CEF_ROOT_ENV, &runtime);
        assert_eq!(
            WebState::default().engine,
            BrowserEngine::Cef,
            "a Workstation with the packaged CEF helper/runtime should default to Chromium"
        );

        std::fs::remove_file(runtime.join(CEF_RESOURCES_DIR).join(CEF_RESOURCES_PAK))
            .expect("remove resources marker");
        assert_eq!(
            WebState::default().engine,
            BrowserEngine::Servo,
            "a partial CEF runtime must fall back to Servo instead of selecting a broken default"
        );
        std::env::remove_var(CEF_HELPER_BIN_ENV);
        std::env::remove_var(CEF_ROOT_ENV);
    }

    #[cfg(feature = "live-helper")]
    #[test]
    fn cef_open_requires_the_real_cef_runtime_before_spawn() {
        use std::cell::Cell;
        let spawned = Cell::new(false);
        let mut state = WebState::default();
        let bin = std::env::current_exe().expect("test exe path");
        std::env::remove_var(CEF_ROOT_ENV);
        state.open_with(
            true,
            BrowserEngine::Cef,
            START_URL.to_owned(),
            bin,
            |_spec| {
                spawned.set(true);
                Err(std::io::Error::other(
                    "factory must not run without the CEF runtime",
                ))
            },
        );
        assert!(!spawned.get(), "missing CEF runtime must gate before spawn");
        assert!(state.tabs.is_empty());
        let notice = state.gate_notice.as_deref().unwrap_or_default();
        assert!(
            notice.contains("Chromium/CEF runtime") && notice.contains(CEF_LIB_NAME),
            "the CEF runtime gate names the missing library: {notice}"
        );
    }

    #[cfg(feature = "live-helper")]
    #[test]
    fn cef_live_open_uses_the_browser_ui_spawn_path_and_pumps_a_frame() {
        use std::cell::RefCell;
        let dir = make_fake_cef_runtime("mde-shell-cef-open-test");
        std::env::set_var(CEF_ROOT_ENV, &dir);

        let helpers: RefCell<Vec<testkit::FakeHelper>> = RefCell::new(Vec::new());
        let mut state = WebState::default();
        state.select_engine(BrowserEngine::Cef);
        let bin = std::env::current_exe().expect("test exe path");
        let expected_bin = bin.clone();
        state.open_with(
            true,
            BrowserEngine::Cef,
            START_URL.to_owned(),
            bin,
            |spec| {
                assert_eq!(spec.helper_bin, expected_bin);
                assert_eq!(spec.url, START_URL);
                assert_eq!((spec.width, spec.height), (INIT_W, INIT_H));
                let (session, helper) = testkit::connect()?;
                helpers.borrow_mut().push(helper);
                Ok(session)
            },
        );

        assert_eq!(state.tabs.len(), 1, "CEF live open attached one tab");
        assert_eq!(state.tabs[0].engine, BrowserEngine::Cef);
        assert!(
            state.gate_notice.is_none(),
            "successful CEF open clears the gate"
        );
        assert!(
            run_until_texture(&mut state),
            "CEF-selected Browser UI path did not pump a frame"
        );
        assert!(state.tabs[0].texture.is_some());

        std::env::remove_var(CEF_ROOT_ENV);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(feature = "live-helper")]
    #[test]
    fn cef_live_browser_ui_renders_a_real_site_when_farm_smoke_is_enabled() {
        if std::env::var_os("MDE_CEF_LIVE_UI_SMOKE").is_none() {
            return;
        }

        let helper_bin = helper_bin_path(BrowserEngine::Cef);
        assert!(
            helper_bin.exists(),
            "MDE_WEB_CEF_BIN must point at a built mde-web-cef helper for the live smoke: {}",
            helper_bin.display()
        );
        assert_eq!(
            cef_runtime_missing_path(),
            None,
            "MDE_CEF_ROOT must point at a complete pinned CEF runtime"
        );

        let server = LiveHttpServer::start();
        let url = server.url.clone();
        let mut state = WebState::default();
        state.select_engine(BrowserEngine::Cef);
        state.open_with(
            true,
            BrowserEngine::Cef,
            START_URL.to_owned(),
            helper_bin,
            WebSession::spawn,
        );

        assert_eq!(state.tabs.len(), 1, "CEF live smoke attached one tab");
        assert_eq!(state.tabs[0].engine, BrowserEngine::Cef);
        assert!(
            run_until_texture(&mut state),
            "CEF did not produce the initial Browser UI frame"
        );

        state.tabs[0].texture = None;
        state.address = url.clone();
        state.submit_address();
        assert!(
            state.insecure_prompt.is_some(),
            "the live HTTP smoke should exercise the Browser HTTPS prompt seam"
        );
        state.continue_insecure_load();
        assert!(
            run_until_texture_for(&mut state, 300),
            "CEF did not render the live HTTP page through the Browser UI texture path"
        );
        assert!(
            server.hits() > 0,
            "CEF did not fetch the live smoke page at {url}"
        );

        if let Some(public_url) = std::env::var("MDE_CEF_LIVE_UI_PUBLIC_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
        {
            state.tabs[0].texture = None;
            state.address = public_url.clone();
            state.submit_address();
            assert!(
                state.insecure_prompt.is_none(),
                "public CEF smoke URLs must be HTTPS or otherwise pre-approved: {public_url}"
            );
            assert!(
                run_until_texture_for(&mut state, 600),
                "CEF did not render the public Browser UI smoke URL: {public_url}"
            );
        }
    }

    #[cfg(feature = "live-helper")]
    #[test]
    fn cef_runtime_gate_accepts_the_upstream_bundle_layout() {
        let dir = make_fake_cef_runtime("mde-shell-cef-runtime-test");
        std::env::set_var(CEF_ROOT_ENV, &dir);
        assert_eq!(
            cef_runtime_lib(),
            dir.join(CEF_RELEASE_DIR).join(CEF_LIB_NAME)
        );
        assert_eq!(cef_runtime_missing_path(), None);
        std::env::remove_var(CEF_ROOT_ENV);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(feature = "live-helper")]
    fn make_fake_cef_runtime(prefix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(CEF_RELEASE_DIR)).expect("mkdir");
        std::fs::write(dir.join(CEF_RELEASE_DIR).join(CEF_LIB_NAME), b"test")
            .expect("libcef marker");
        std::fs::create_dir_all(dir.join(CEF_RESOURCES_DIR)).expect("resources");
        std::fs::write(dir.join(CEF_RESOURCES_DIR).join(CEF_ICU_DATA), b"icu").expect("icu marker");
        std::fs::write(dir.join(CEF_RESOURCES_DIR).join(CEF_RESOURCES_PAK), b"pak")
            .expect("pak marker");
        dir
    }

    #[cfg(feature = "live-helper")]
    struct LiveHttpServer {
        url: String,
        addr: std::net::SocketAddr,
        hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        done: std::sync::Arc<std::sync::atomic::AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    #[cfg(feature = "live-helper")]
    impl LiveHttpServer {
        fn start() -> Self {
            use std::io::{Read, Write};
            use std::net::TcpListener;
            use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
            use std::sync::Arc;
            use std::time::Duration;

            let listener = TcpListener::bind("127.0.0.1:0").expect("bind live smoke server");
            listener
                .set_nonblocking(true)
                .expect("nonblocking live smoke server");
            let addr = listener.local_addr().expect("live smoke addr");
            let hits = Arc::new(AtomicUsize::new(0));
            let done = Arc::new(AtomicBool::new(false));
            let server_hits = Arc::clone(&hits);
            let server_done = Arc::clone(&done);
            let handle = std::thread::spawn(move || {
                let body = b"<!doctype html><html><body><h1>CEF Browser UI live smoke</h1><p>real HTTP page</p></body></html>";
                while !server_done.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let mut buf = [0_u8; 1024];
                            let _ = stream.read(&mut buf);
                            server_hits.fetch_add(1, Ordering::SeqCst);
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = stream.write_all(response.as_bytes());
                            let _ = stream.write_all(body);
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                url: format!("http://{addr}/"),
                addr,
                hits,
                done,
                handle: Some(handle),
            }
        }

        fn hits(&self) -> usize {
            self.hits.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[cfg(feature = "live-helper")]
    impl Drop for LiveHttpServer {
        fn drop(&mut self) {
            use std::net::TcpStream;
            use std::sync::atomic::Ordering;
            self.done.store(true, Ordering::SeqCst);
            let _ = TcpStream::connect(self.addr);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }
}
