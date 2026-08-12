//! The Construct Springboard Dock: persistent shell navigation plus chooser pins.
//!
//! The dock is shell-owned and always reserves its footprint. In the default
//! bottom placement it is a solid-black, full-width 48px taskbar; Pin moves it
//! into the 56px left rail that the central layout reserves below the top strip.
//! Placement is persisted per seat, while the visual transition is kept in
//! memory so a restart never waits on animation state. Chooser-pinned remote
//! desktop sources are rendered as additional dock targets from the chooser's
//! existing read model.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mde_egui::egui;
use mde_egui::{Elevation, Motion, MotionMode, MotionPreset, Style, TypographyRole};
use mde_theme::brand::icons::IconId;
use serde::{Deserialize, Serialize};

use crate::construct::ConstructChrome;
use crate::front_door::{FrontDoorPeerAppFavorites, FrontDoorPeerAppTarget};
use crate::status::StatusSegments;
use crate::status_bar::{self, bottom_tray_rect, StatusBarEnv, BOTTOM_TRAY_GAP, STATUS_BAR_H};
use crate::surfaces::{icon_texture, SessionRailEntry, Surface};

/// The reserved left-rail width in docked mode.
pub(crate) const DOCKED_W: f32 = 56.0;
/// The full-width Construct taskbar height in Bottom mode.
pub(crate) const TASKBAR_H: f32 = 48.0;
/// Bottom space reserved by the horizontal taskbar in normal workspace layout.
pub(crate) const SPRINGBOARD_DOCK_RESERVED_H: f32 = TASKBAR_H;
/// The icon controls use the taskbar's fixed 40px touch/keyboard target.
const CONTROL_EDGE: f32 = 40.0;
/// The focused workspace marker is deliberately smaller than its 40px target:
/// one centered accent, with a clear bottom breathing room, is the only
/// persistent focus signal in the taskbar.
const FOCUS_UNDERLINE_W: f32 = 18.0;
const FOCUS_UNDERLINE_H: f32 = 3.0;
const FOCUS_UNDERLINE_BOTTOM_GAP: f32 = 2.0;
/// The horizontal taskbar keeps fixed-size targets and moves lower-priority
/// chooser pins and launchers into the More flyout when a panel cannot fit the
/// whole catalog.
const FLOATING_GAP: f32 = 4.0;
/// Reserved right-side lane for the Windows-style clock and icon tray when the
/// taskbar is in its bottom configuration.
/// Width of the single-column taskbar overflow surface before screen clamping.
const OVERFLOW_W: f32 = 256.0;
/// Gap between the More anchor and its overflow surface.
const OVERFLOW_GAP: f32 = 4.0;
/// Maximum number of chooser-pinned sources shown in the dock. The full
/// chooser remains the unbounded discovery surface; the dock is a quick rail.
const MAX_PINNED_SOURCES: usize = 8;
/// The placement pin is intentionally visually subordinate to the navigation
/// glyphs while retaining the same full-size hit target and accessibility box.
const PIN_ICON_SCALE: f32 = 0.4;
/// The transition first slides left, then melts into the vertical rail.
const SLIDE_FRACTION: f32 = 0.34;
/// Total normal-mode transition length: short enough to feel direct, long enough
/// to read. The authoritative semantic timing lives in `MotionPreset::Whimsy`.
const TRANSITION: Duration = Duration::from_millis(280);
/// Persisted per-seat preference.
const CONFIG_FILE: &str = "settings-nav-bar.json";
/// Keep hostile or stale dock preferences bounded before serde materializes them.
const MAX_NAV_PREFS_BYTES: usize = 64 * 1024;
const NAV_PREFS_SCHEMA_VERSION: u16 = 2;
const MAX_PINNED_SURFACES: usize = Surface::ALL.len();
const MAX_PIN_SELECTOR_QUERY_CHARS: usize = 128;
const FIRST_BOOT_SELECTOR_W: f32 = 560.0;
const FIRST_BOOT_SELECTOR_H: f32 = 720.0;
static NAV_LAYER_ID_MAP_LOGGED: AtomicBool = AtomicBool::new(false);

/// Ordered first-boot and personalization catalog. Workers is the only
/// node-management entry; historical Fleet & Mesh / This Node keys are
/// decoded below and collapsed into it.
const PIN_CATALOG: [Surface; 9] = [
    Surface::Workers,
    Surface::InfraCode,
    Surface::Desktop,
    Surface::Terminal,
    Surface::MapsLocation,
    Surface::Communications,
    Surface::Music,
    Surface::Media,
    Surface::Browser,
];

/// One action emitted by the painted controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// Open the existing Front Door search overlay from Start.
    OpenFrontDoor,
    /// Focus the existing Front Door search field, opening its overlay if needed.
    FocusSearch,
    /// Return to the previously active app or Fleet & Mesh tab.
    Back,
    /// Open the untitled all-icons Desktop.
    Home,
    /// Open the real editor through Communications' Documents mode. The editor
    /// remains one owned workspace; this is its always-visible launch affordance.
    OpenEditor,
    /// Toggle between the bottom taskbar and the left rail.
    ToggleDock,
    /// Open one docked app surface.
    OpenSurface(Surface),
    /// Open the existing Maps & Location surface directly in its Weather mode.
    OpenWeather,
    /// Add a catalogued surface to the taskbar, preserving existing order.
    PinSurface(Surface),
    /// Remove a catalogued surface from the taskbar, preserving the survivors' order.
    UnpinSurface(Surface),
    /// Open a chooser-pinned remote desktop source through the normal chooser
    /// authentication and VDI hand-off path.
    DesktopSource(String),
    /// Focus one already-connected remote desktop or App VM session directly.
    RemoteSession(String),
}

/// Borrowed bottom-tray state painted inside the taskbar's foreground Area.
/// Navigation owns the one bar; status keeps its existing data and actions.
struct BottomTray<'a> {
    construct: &'a mut ConstructChrome,
    segments: &'a StatusSegments,
    opacity: f32,
    env: StatusBarEnv,
    active_surface: Option<Surface>,
    battery: Option<status_bar::LiveBatteryStatus>,
    weather: Option<status_bar::LiveWeatherStatus>,
}

/// The persisted placement choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum DockMode {
    /// The undocked bottom taskbar.
    #[serde(rename = "floating")]
    Floating,
    /// The reserved left rail below the status bar.
    #[serde(rename = "docked")]
    Docked,
}

impl Default for DockMode {
    fn default() -> Self {
        Self::Floating
    }
}

impl DockMode {
    const fn id_suffix(self) -> &'static str {
        match self {
            Self::Floating => "floating",
            Self::Docked => "docked",
        }
    }
}

fn default_taskbar_pins() -> Vec<Surface> {
    // This fallback is used only by deterministic headless/default state. A
    // new runtime profile starts empty and goes through the selector; a
    // migrated profile keeps its decoded ordered pins.
    PIN_CATALOG
        .into_iter()
        .filter(|surface| pin_catalog_surface(*surface))
        .take(10)
        .collect()
}

fn surface_key(surface: Surface) -> &'static str {
    match surface {
        Surface::Workers => "workers",
        Surface::FleetMesh => "fleet-mesh",
        Surface::InfraCode => "workloads",
        Surface::Desktop => "desktop",
        Surface::Music => "music",
        Surface::Media => "media",
        Surface::Files => "files",
        Surface::Browser => "browser",
        Surface::MapsLocation => "maps-location",
        Surface::Terminal => "terminal",
        Surface::Phones => "phones",
        Surface::ThisNode => "this-node",
        Surface::Communications => "mesh-teams",
        Surface::Workbench => "workbench",
        Surface::MeshView => "mesh-view",
        Surface::Explorer => "explorer",
        Surface::System => "system",
        Surface::Storage => "storage",
        Surface::About => "about",
        Surface::Clock => "clock",
        Surface::AutoHome => "auto-home",
    }
}

fn surface_from_key(key: &str) -> Option<Surface> {
    // Persisted profiles can predate the canonical Workers entry. Decode old
    // keys explicitly, then canonicalize aliases,
    // so migration preserves those pins instead of silently dropping them.
    PIN_CATALOG
        .into_iter()
        .find(|surface| surface_key(*surface) == key)
        .or_else(|| match key {
            "fleet-mesh" | "fleetmesh" => Some(Surface::FleetMesh),
            "workbench" => Some(Surface::Workbench),
            "mesh-view" => Some(Surface::MeshView),
            "explorer" => Some(Surface::Explorer),
            "this-node" | "thisnode" => Some(Surface::ThisNode),
            "phones" | "phone" => Some(Surface::Phones),
            "system" => Some(Surface::System),
            "storage" => Some(Surface::Storage),
            "about" => Some(Surface::About),
            _ => None,
        })
        .and_then(canonical_taskbar_surface)
}

fn canonical_taskbar_surface(surface: Surface) -> Option<Surface> {
    let canonical = match surface {
        Surface::Workers
        | Surface::FleetMesh
        | Surface::Workbench
        | Surface::MeshView
        | Surface::Explorer
        | Surface::ThisNode
        | Surface::System
        | Surface::Storage
        | Surface::About
        | Surface::Phones => Some(Surface::Workers),
        // Standalone Files is a compatibility deep link only; persisted pins
        // migrate to the one Mesh Teams collaboration destination.
        Surface::Files => Some(Surface::Communications),
        Surface::AutoHome | Surface::Clock => None,
        surface if Surface::ALL.contains(&surface) => Some(surface),
        _ => None,
    }?;
    // Tool-tray workspaces deliberately have no duplicate taskbar marker. The
    // fixed Workloads and merged Home/Sessions controls do, however, need the
    // canonical surface to reach `focused_control_index`.
    (!crate::surfaces::is_tool_tray_surface(canonical)).then_some(canonical)
}

/// Surfaces owned by the right-side tool tray are not valid center-nav pins.
/// Keeping this boundary in the catalog helpers also migrates old persisted
/// profiles instead of allowing duplicate workspace affordances to survive.
const fn pin_catalog_surface(surface: Surface) -> bool {
    !crate::surfaces::is_tool_tray_surface(surface)
        && !matches!(surface, Surface::InfraCode | Surface::Desktop)
}

fn decode_pinned_surfaces(keys: &[String]) -> Vec<Surface> {
    let mut surfaces = Vec::with_capacity(keys.len().min(MAX_PINNED_SURFACES));
    for surface in keys
        .iter()
        .filter_map(|key| surface_from_key(key))
        .filter_map(canonical_taskbar_surface)
    {
        if !surfaces.contains(&surface) {
            surfaces.push(surface);
        }
        if surfaces.len() == MAX_PINNED_SURFACES {
            break;
        }
    }
    surfaces
}

fn taskbar_surface_label(surface: Surface) -> &'static str {
    match surface {
        Surface::Workers => "Workers",
        Surface::FleetMesh => "Fleet & Mesh",
        Surface::InfraCode => "Workloads",
        surface => surface.label(),
    }
}

fn pin_catalog_index(surface: Surface) -> Option<usize> {
    PIN_CATALOG
        .iter()
        .position(|candidate| *candidate == surface && pin_catalog_surface(*candidate))
}

fn pin_catalog_contains(surface: Surface) -> bool {
    pin_catalog_index(surface).is_some()
}

fn taskbar_surface_search_terms(surface: Surface) -> &'static str {
    match surface {
        // These are navigation aliases, not additional taskbar entries. Keep
        // them on the canonical Workers result so a Start search or the
        // first-boot selector cannot strand the operator on a legacy deep link.
        // Query hyphens are normalized to spaces by `filtered_pin_catalog`.
        Surface::Workers => "workers fleet mesh fleet & mesh this node node workbench mesh map mesh view meshmap meshview explorer system storage about phones phone",
        Surface::FleetMesh => {
            "fleet mesh fleet & mesh workbench mesh map mesh view meshmap meshview explorer"
        }
        Surface::InfraCode => "infra code infra-code workloads",
        _ => "",
    }
}

fn filtered_pin_catalog(query: &str) -> Vec<Surface> {
    let query = bounded_pin_selector_query(query)
        .trim()
        .to_ascii_lowercase()
        .replace('-', " ");
    PIN_CATALOG
        .into_iter()
        .filter(|surface| pin_catalog_surface(*surface))
        .filter(|surface| {
            query.is_empty()
                || taskbar_surface_label(*surface)
                    .to_ascii_lowercase()
                    .contains(&query)
                || surface_key(*surface).replace('-', " ").contains(&query)
                || taskbar_surface_search_terms(*surface).contains(&query)
        })
        .collect()
}

fn bounded_pin_selector_query(query: &str) -> String {
    query.chars().take(MAX_PIN_SELECTOR_QUERY_CHARS).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ProfileState {
    /// A seat with no navigation preference has not completed first boot.
    #[serde(rename = "new")]
    New,
    /// A migrated or completed seat has an authoritative ordered pin list.
    #[serde(rename = "configured")]
    Configured,
}

fn default_persisted_profile_state() -> ProfileState {
    // Existing settings files predate this field and are migrated profiles;
    // absence must never be mistaken for a new profile during deserialization.
    ProfileState::Configured
}

#[derive(Debug, Clone, Default)]
struct PinSelectorState {
    query: String,
    selected: Vec<Surface>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct NavBarPrefs {
    #[serde(default)]
    schema_version: u16,
    #[serde(default)]
    mode: DockMode,
    #[serde(default)]
    pinned_surfaces: Vec<String>,
    #[serde(default = "default_persisted_profile_state")]
    profile_state: ProfileState,
    #[serde(default)]
    peer_app_favorites: FrontDoorPeerAppFavorites,
}

impl Default for NavBarPrefs {
    fn default() -> Self {
        Self {
            schema_version: NAV_PREFS_SCHEMA_VERSION,
            mode: DockMode::Floating,
            pinned_surfaces: default_taskbar_pins()
                .iter()
                .map(|surface| surface_key(*surface).to_owned())
                .collect(),
            profile_state: ProfileState::Configured,
            peer_app_favorites: FrontDoorPeerAppFavorites::empty(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TransitionState {
    from: DockMode,
    to: DockMode,
    started: Instant,
    motion: MotionMode,
}

/// Shell-owned state for the bar's persisted placement and transient motion.
#[derive(Debug)]
pub(crate) struct State {
    mode: DockMode,
    transition: Option<TransitionState>,
    pinned_surfaces: Vec<Surface>,
    profile_state: ProfileState,
    peer_app_favorites: FrontDoorPeerAppFavorites,
    pin_selector: PinSelectorState,
}

impl Default for State {
    fn default() -> Self {
        Self {
            mode: DockMode::Floating,
            transition: None,
            pinned_surfaces: default_taskbar_pins(),
            profile_state: ProfileState::Configured,
            peer_app_favorites: FrontDoorPeerAppFavorites::empty(),
            pin_selector: PinSelectorState::default(),
        }
    }
}

impl State {
    /// Build a deterministic placement state for layout and render proofs.
    ///
    /// This intentionally does not load or write seat preferences, so headless
    /// tests can exercise the docked rail without inheriting an operator's
    /// persisted navigation choice.
    #[must_use]
    pub(crate) fn with_mode(mode: DockMode) -> Self {
        Self {
            mode,
            transition: None,
            pinned_surfaces: default_taskbar_pins(),
            profile_state: ProfileState::Configured,
            peer_app_favorites: FrontDoorPeerAppFavorites::empty(),
            pin_selector: PinSelectorState::default(),
        }
    }

    fn new_profile(mode: DockMode) -> Self {
        Self {
            mode,
            transition: None,
            pinned_surfaces: Vec::new(),
            profile_state: ProfileState::New,
            peer_app_favorites: FrontDoorPeerAppFavorites::empty(),
            pin_selector: PinSelectorState::default(),
        }
    }

    fn from_prefs(prefs: NavBarPrefs) -> Self {
        // A newer writer may assign different meaning to placement or pin
        // keys.  Treating those bytes as this schema can restore a left rail
        // or launch targets the user did not configure under this build.  Keep
        // legacy (zero/older) migrations, but fail closed on future schemas.
        if prefs.schema_version > NAV_PREFS_SCHEMA_VERSION {
            return Self {
                mode: DockMode::Floating,
                transition: None,
                pinned_surfaces: Vec::new(),
                profile_state: ProfileState::Configured,
                peer_app_favorites: FrontDoorPeerAppFavorites::empty(),
                pin_selector: PinSelectorState::default(),
            };
        }
        let pinned_surfaces = decode_pinned_surfaces(&prefs.pinned_surfaces);
        let pin_selector = PinSelectorState {
            query: String::new(),
            selected: pinned_surfaces.clone(),
        };
        Self {
            mode: prefs.mode,
            transition: None,
            pinned_surfaces,
            profile_state: prefs.profile_state,
            peer_app_favorites: prefs.peer_app_favorites.bounded(),
            pin_selector,
        }
    }

    /// Load the persisted placement, degrading malformed or absent data to the
    /// floating default.
    #[must_use]
    pub(crate) fn load() -> Self {
        let Some(path) = Self::default_path() else {
            return Self::new_profile(DockMode::Floating);
        };
        match read_bounded_config(&path) {
            Ok(json) => serde_json::from_str::<NavBarPrefs>(&json)
                .map(Self::from_prefs)
                .unwrap_or_else(|_| {
                    Self::from_prefs(NavBarPrefs {
                        profile_state: ProfileState::Configured,
                        pinned_surfaces: Vec::new(),
                        ..NavBarPrefs::default()
                    })
                }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Self::new_profile(DockMode::Floating)
            }
            Err(_) => Self::from_prefs(NavBarPrefs {
                profile_state: ProfileState::Configured,
                pinned_surfaces: Vec::new(),
                ..NavBarPrefs::default()
            }),
        }
    }

    #[must_use]
    fn is_new_profile(&self) -> bool {
        self.profile_state == ProfileState::New
    }

    /// Whether the layout should reserve the left rail this frame.
    #[must_use]
    pub(crate) const fn is_docked(&self) -> bool {
        matches!(self.mode, DockMode::Docked)
    }

    /// Whether the central workspace must reserve the horizontal dock band.
    /// During the melt transition both endpoint rails are reserved so content
    /// never slides underneath either representation.
    #[must_use]
    pub(crate) fn reserves_bottom_space(&self) -> bool {
        self.transition
            .as_ref()
            .map_or(matches!(self.mode, DockMode::Floating), |transition| {
                matches!(transition.from, DockMode::Floating)
                    || matches!(transition.to, DockMode::Floating)
            })
    }

    /// Whether the central workspace must reserve the vertical pinned rail.
    #[must_use]
    pub(crate) fn reserves_left_space(&self) -> bool {
        self.transition
            .as_ref()
            .map_or(matches!(self.mode, DockMode::Docked), |transition| {
                matches!(transition.from, DockMode::Docked)
                    || matches!(transition.to, DockMode::Docked)
            })
    }

    /// Return the cross-fade weights for the two chrome treatments. The bottom
    /// tray and the top status strip share the same eased timeline as the
    /// navigation geometry, so the layout never snaps while the bar is moving.
    #[must_use]
    pub(crate) fn chrome_alphas(&self, now: Instant) -> (f32, f32) {
        let Some(transition) = self.transition else {
            return match self.mode {
                DockMode::Floating => (1.0, 0.0),
                DockMode::Docked => (0.0, 1.0),
            };
        };
        let elapsed = now.saturating_duration_since(transition.started);
        let t = transition_progress(elapsed, transition.motion);
        match (transition.from, transition.to) {
            (DockMode::Floating, DockMode::Docked) => (1.0 - t, t),
            (DockMode::Docked, DockMode::Floating) => (t, 1.0 - t),
            _ => (1.0, 0.0),
        }
    }

    /// Toggle placement and start the slide/melt transition.
    pub(crate) fn toggle(&mut self, now: Instant, motion: MotionMode) {
        self.toggle_mode(now, motion);
        self.save();
    }

    /// The current ordered taskbar catalog, suitable for the Front Door's
    /// personalization affordance. The slice is always bounded by the
    /// catalog decoder and the mutation methods below.
    pub(crate) fn pinned_surfaces(&self) -> &[Surface] {
        &self.pinned_surfaces
    }

    pub(crate) fn peer_app_favorites(&self) -> &FrontDoorPeerAppFavorites {
        &self.peer_app_favorites
    }

    pub(crate) fn toggle_peer_app_favorite(&mut self, target: &FrontDoorPeerAppTarget) {
        self.peer_app_favorites.toggle_target(target);
        self.save();
    }

    /// Pin one launchable catalog surface at the end of the existing order.
    /// Non-catalog surfaces (including protected chrome aliases) are rejected
    /// before they can reach persistence or geometry.
    pub(crate) fn pin_surface(&mut self, surface: Surface) -> bool {
        if !pin_catalog_contains(surface)
            || self.pinned_surfaces.contains(&surface)
            || self.pinned_surfaces.len() >= MAX_PINNED_SURFACES
        {
            return false;
        }
        self.pinned_surfaces.push(surface);
        self.save();
        true
    }

    /// Unpin one launchable catalog surface. Start/Back/Home/placement are
    /// represented by controls rather than surfaces, so they have no mutation
    /// path and cannot be removed by accident.
    pub(crate) fn unpin_surface(&mut self, surface: Surface) -> bool {
        if !pin_catalog_contains(surface) {
            return false;
        }
        let Some(index) = self
            .pinned_surfaces
            .iter()
            .position(|item| *item == surface)
        else {
            return false;
        };
        self.pinned_surfaces.remove(index);
        self.save();
        true
    }

    fn toggle_pin_selector_surface(&mut self, surface: Surface) {
        if !pin_catalog_contains(surface) {
            return;
        }
        if let Some(index) = self
            .pin_selector
            .selected
            .iter()
            .position(|item| *item == surface)
        {
            self.pin_selector.selected.remove(index);
        } else {
            self.pin_selector.selected.push(surface);
        }
        self.pin_selector
            .selected
            .sort_by_key(|item| pin_catalog_index(*item).unwrap_or(PIN_CATALOG.len()));
    }

    /// Return the user's first-boot selection in persisted order. The selector
    /// normally reaches this boundary only through `toggle_pin_selector_surface`,
    /// but canonicalizing again keeps a partially written or stale transient
    /// buffer from restoring defaults or persisting unsupported surfaces.
    fn bounded_first_boot_selection(&self) -> Vec<Surface> {
        self.pin_selector
            .selected
            .iter()
            .copied()
            .filter(|surface| pin_catalog_contains(*surface))
            .fold(
                Vec::with_capacity(MAX_PINNED_SURFACES),
                |mut selected, surface| {
                    if selected.len() < MAX_PINNED_SURFACES && !selected.contains(&surface) {
                        selected.push(surface);
                    }
                    selected
                },
            )
    }

    fn complete_first_boot(&mut self) {
        // An empty result is an explicit user choice. Never substitute the
        // deterministic headless defaults here: migrated pins remain intact
        // when carried through the selector, while a new profile may choose
        // no taskbar apps at all.
        self.pinned_surfaces = self.bounded_first_boot_selection();
        self.profile_state = ProfileState::Configured;
        self.pin_selector = PinSelectorState::default();
        self.save();
    }

    fn toggle_mode(&mut self, now: Instant, motion: MotionMode) {
        let from = self.mode;
        let to = match from {
            DockMode::Floating => DockMode::Docked,
            DockMode::Docked => DockMode::Floating,
        };
        self.mode = to;
        self.transition = (motion != MotionMode::Disabled).then(|| TransitionState {
            from,
            to,
            started: now,
            motion,
        });
    }

    /// Paint the dock and return the first clicked action, if any.
    pub(crate) fn mount(
        &mut self,
        ctx: &egui::Context,
        pinned_sources: &[crate::surfaces::DesktopRailSource],
    ) -> Option<Action> {
        self.mount_with_active(ctx, pinned_sources, None)
    }

    /// Paint the dock and underline the currently focused surface when the
    /// shell supplies one. The wrapper above keeps headless callers and older
    /// chrome tests independent of the shell's navigation state.
    pub(crate) fn mount_with_active(
        &mut self,
        ctx: &egui::Context,
        pinned_sources: &[crate::surfaces::DesktopRailSource],
        active_surface: Option<Surface>,
    ) -> Option<Action> {
        self.mount_with_active_inner(ctx, pinned_sources, active_surface, None, &[], None)
    }

    /// Render bottom-placement navigation and the clock/tray as one taskbar.
    pub(crate) fn mount_with_active_and_bottom_tray(
        &mut self,
        ctx: &egui::Context,
        pinned_sources: &[crate::surfaces::DesktopRailSource],
        active_surface: Option<Surface>,
        construct: &mut ConstructChrome,
        segments: &StatusSegments,
        tray_opacity: f32,
        tray_env: StatusBarEnv,
    ) -> Option<Action> {
        self.mount_with_active_inner(
            ctx,
            pinned_sources,
            active_surface,
            None,
            &[],
            Some(BottomTray {
                construct,
                segments,
                opacity: tray_opacity,
                env: tray_env,
                active_surface,
                battery: None,
                weather: None,
            }),
        )
    }

    /// Render the bottom taskbar with transient connected-session targets.
    /// These entries are supplied by the shell's live session projection and
    /// are deliberately excluded from nav-bar persistence and pin menus.
    pub(crate) fn mount_with_active_and_bottom_tray_and_sessions(
        &mut self,
        ctx: &egui::Context,
        pinned_sources: &[crate::surfaces::DesktopRailSource],
        active_surface: Option<Surface>,
        active_session_id: Option<&str>,
        connected_sessions: &[SessionRailEntry],
        construct: &mut ConstructChrome,
        segments: &StatusSegments,
        tray_opacity: f32,
        tray_env: StatusBarEnv,
        battery: Option<status_bar::LiveBatteryStatus>,
        weather: Option<status_bar::LiveWeatherStatus>,
    ) -> Option<Action> {
        self.mount_with_active_inner(
            ctx,
            pinned_sources,
            active_surface,
            active_session_id,
            connected_sessions,
            Some(BottomTray {
                construct,
                segments,
                opacity: tray_opacity,
                env: tray_env,
                active_surface,
                battery,
                weather,
            }),
        )
    }

    fn mount_with_active_inner(
        &mut self,
        ctx: &egui::Context,
        pinned_sources: &[crate::surfaces::DesktopRailSource],
        active_surface: Option<Surface>,
        active_session_id: Option<&str>,
        connected_sessions: &[SessionRailEntry],
        mut bottom_tray: Option<BottomTray<'_>>,
    ) -> Option<Action> {
        let active_surface = active_surface.and_then(canonical_taskbar_surface);
        if ctx.cumulative_pass_nr() == 0 {
            return None;
        }
        let screen = ctx.screen_rect();
        if nav_bar_proof_enabled() {
            tracing::info!(
                target: "mde_shell_egui::nav_bar",
                screen_width = screen.width(),
                screen_height = screen.height(),
                pixels_per_point = ctx.pixels_per_point(),
                zoom_factor = ctx.zoom_factor(),
                "springboard dock viewport proof"
            );
        }
        let pinned_sources = &pinned_sources[..pinned_sources.len().min(MAX_PINNED_SOURCES)];
        let geometry = self.geometry_for_with_sessions(
            screen,
            Instant::now(),
            pinned_sources.len(),
            connected_sessions,
        );
        // Resolve focus once against the rendered controls. This makes the
        // underline a singular semantic marker even if stale/corrupt state
        // ever presents duplicate canonical entries during a frame.
        let focused_index = focused_control_index_with_sessions(
            &geometry.controls,
            active_surface,
            connected_sessions,
            active_session_id,
        );
        if geometry.finished {
            self.transition = None;
        } else {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        let mut action = None;
        let overflow_popup_id = overflow_popup_id(self.mode);
        let mut overflow_response = None;
        let area = egui::Area::new(egui::Id::new("construct-navigation-bar"))
            .order(egui::Order::Foreground)
            // Keep the foreground Area limited to the visible bar footprint.
            // A full-screen interactive Area makes the navigation chrome a
            // transparent input shield: the home grid and every workspace
            // widget still paint, but nothing underneath can receive clicks.
            .fixed_pos(geometry.outer.min)
            // The child hit rectangles below are explicit screen-space
            // rectangles, so the parent does not need to participate in
            // hit-testing. Keeping it non-interactive prevents a stale Area
            // size from becoming a transparent input shield after an upgrade.
            .default_size(geometry.outer.size())
            .movable(false)
            // Keep the Area in egui's layer hit-test without giving it a click
            // sense. The child controls own clicks; a non-interactable Area is
            // omitted from `layer_id_at`, which makes the foreground controls
            // lose their layer during a move or dock/floating transition.
            .sense(egui::Sense::hover())
            .show(ctx, |ui| {
                ui.set_min_size(geometry.outer.size());
                let painter = ui.painter().clone();
                paint_backing(&painter, &geometry);
                for (control_index, control) in geometry.controls.iter().enumerate() {
                    // The Area's content UI is created with its absolute screen
                    // rect as max_rect, so these interaction rectangles stay in
                    // the same screen space as the painter and AccessKit tree.
                    let response = ui
                        .interact(
                            control.rect,
                            control_id(self.mode, *control),
                            egui::Sense::click(),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    let hovered = response.hovered();
                    let clicked = response.clicked();
                    if nav_bar_proof_enabled() {
                        log_foreground_layer_id_map_once();
                        let interact_pos = ctx.input(|i| i.pointer.interact_pos());
                        let top_layer = interact_pos
                            .and_then(|pos| ctx.layer_id_at(pos))
                            .map(|layer| format!("{layer:?}"));
                        let response_layer = format!("{:?}", response.layer_id);
                        let layer_contains_pointer =
                            ctx.rect_contains_pointer(response.layer_id, response.interact_rect);
                        tracing::info!(
                            target: "mde_shell_egui::nav_bar",
                            mode = self.mode.id_suffix(),
                            control = control.kind.id_suffix(),
                            rect_left = control.rect.left(),
                            rect_top = control.rect.top(),
                            rect_right = control.rect.right(),
                            rect_bottom = control.rect.bottom(),
                            hovered,
                            clicked,
                            interact_x = interact_pos.map(|pos| pos.x),
                            interact_y = interact_pos.map(|pos| pos.y),
                            top_layer = top_layer.as_deref(),
                            response_layer = response_layer.as_str(),
                            contains_pointer = response.contains_pointer(),
                            layer_contains_pointer,
                            enabled = response.enabled(),
                            "springboard dock control response proof"
                        );
                    }
                    if hovered {
                        painter.rect_filled(
                            control.rect.shrink(2.0),
                            egui::CornerRadius::same(Style::RADIUS_M as u8),
                            Style::NAV_BAR_HOVER,
                        );
                    }
                    let icon_size =
                        control_icon_size(*control, control.rect.height().min(Style::ICON_L));
                    let icon_rect = egui::Rect::from_center_size(
                        control.rect.center(),
                        egui::vec2(icon_size, icon_size),
                    );
                    if let Some(texture) =
                        icon_texture(ctx, control_icon(*control), icon_size, Style::NAV_BAR_ICON)
                    {
                        painter.image(
                            texture.id(),
                            icon_rect,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            Style::NAV_BAR_ICON,
                        );
                    }
                    if focused_index == Some(control_index) {
                        let underline = focus_underline_rect(control.rect);
                        painter.rect_filled(underline, egui::CornerRadius::ZERO, Style::ACCENT);
                    }
                    let label = if control.kind == ControlKind::Overflow {
                        geometry.overflow.as_ref().map_or_else(
                            || "More taskbar apps".to_owned(),
                            |overflow| overflow_label(overflow.items.len()),
                        )
                    } else {
                        control_label(*control, pinned_sources, connected_sessions)
                    };
                    install_accessibility(
                        ctx,
                        self.mode,
                        *control,
                        label.as_str(),
                        self.is_docked(),
                    );
                    let _response = response.clone().on_hover_ui(move |ui| {
                        nav_bar_tooltip(ui, label.as_str());
                    });
                    let keyboard_toggle = response.has_focus()
                        && ctx.input(|input| {
                            input.key_pressed(egui::Key::Enter)
                                || input.key_pressed(egui::Key::Space)
                        });
                    if control.kind == ControlKind::Overflow {
                        overflow_response = Some(response.clone());
                        if clicked || keyboard_toggle {
                            ctx.memory_mut(|memory| memory.toggle_popup(overflow_popup_id));
                        }
                    } else if clicked {
                        action = Some(control_action(*control, pinned_sources, connected_sessions));
                    }
                    if control.kind != ControlKind::Overflow {
                        if let Some(menu_action) =
                            taskbar_context_menu(&response, *control, &self.pinned_surfaces)
                        {
                            action = Some(menu_action);
                        }
                    }
                }

                // The tray is painted in this same Area after app controls, so
                // its clock and system targets remain anchored in the bar and
                // take the final hit-test priority in their reserved lane.
                if let Some(tray) = bottom_tray.as_mut() {
                    if status_bar::paint_bottom_tray_with_active(
                        ui,
                        screen,
                        tray.construct,
                        tray.segments,
                        tray.opacity,
                        tray.env,
                        tray.active_surface,
                        tray.battery,
                        tray.weather.as_ref(),
                    ) {
                        action = Some(Action::OpenWeather);
                    }
                }

                if let (Some(anchor), Some(overflow)) =
                    (overflow_response.as_ref(), geometry.overflow.as_ref())
                {
                    let items = overflow.items.clone();
                    let mode = self.mode;
                    let _ = egui::popup::popup_above_or_below_widget(
                        ui,
                        overflow_popup_id,
                        anchor,
                        egui::AboveOrBelow::Above,
                        egui::popup::PopupCloseBehavior::CloseOnClickOutside,
                        |ui| {
                            paint_overflow_popup(
                                ui,
                                mode,
                                anchor.rect,
                                &items,
                                pinned_sources,
                                connected_sessions,
                                &mut action,
                            );
                        },
                    );
                }
            });
        // egui retains Area ordering across frames. A foreground scrim or
        // modal that was previously moved to the top can remain above the
        // navigation bar even after it no longer visibly covers the dock. Keep
        // the global Springboard Dock as the top ordinary foreground Area; the
        // shell still mounts the lock curtain after this and raises it over all
        // chrome when engaged.
        ctx.move_to_top(area.response.layer_id);
        if self.is_new_profile() {
            self.mount_first_boot_selector(ctx);
            action = None;
        }
        action
    }

    fn mount_first_boot_selector(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        let card = first_boot_selector_rect(screen);
        let mut query = bounded_pin_selector_query(&self.pin_selector.query);
        let selected = self.pin_selector.selected.clone();
        let mut toggled = None;
        let mut complete = false;

        let area = egui::Area::new(egui::Id::new("construct-first-boot-pin-selector"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .default_size(screen.size())
            .movable(false)
            .sense(egui::Sense::hover())
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());
                ui.painter().rect_filled(
                    screen,
                    egui::CornerRadius::ZERO,
                    Style::resolve_color(ctx, Style::SCRIM_REGULAR),
                );
                let _ = ui.interact(
                    screen,
                    egui::Id::new("construct-first-boot-pin-selector-scrim"),
                    egui::Sense::click(),
                );
                ui.painter().rect_filled(
                    card,
                    egui::CornerRadius::same(Style::RADIUS_L as u8),
                    Style::resolve_color(ctx, Style::SURFACE),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(card), |ui| {
                    ui.add_space(Style::SP_L);
                    ui.label(
                        Style::typography_text("Choose your taskbar apps", TypographyRole::Title)
                            .color(Style::resolve_color(ctx, Style::TEXT)),
                    );
                    ui.label(
                        Style::typography_text(
                            "Select the surfaces you want pinned on this profile. You can change them later from the taskbar.",
                            TypographyRole::Body,
                        )
                        .color(Style::resolve_color(ctx, Style::TEXT_DIM)),
                    );
                    ui.add_space(Style::SP_S);
                    ui.add_sized(
                        egui::vec2(
                            (card.width() - 2.0 * Style::SP_L).max(1.0),
                            40.0,
                        ),
                        egui::TextEdit::singleline(&mut query)
                            .hint_text("Search taskbar apps"),
                    );
                    ui.add_space(Style::SP_XS);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .max_height((card.height() - 190.0).max(40.0))
                        .show(ui, |ui| {
                            for surface in filtered_pin_catalog(&query) {
                                let selected_row = selected.contains(&surface);
                                let (rect, response) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), CONTROL_EDGE),
                                    egui::Sense::click(),
                                );
                                let response =
                                    response.on_hover_cursor(egui::CursorIcon::PointingHand);
                                if selected_row {
                                    ui.painter().rect_filled(
                                        rect,
                                        egui::CornerRadius::same(Style::RADIUS_S as u8),
                                        Style::resolve_color(ctx, Style::SURFACE_HI),
                                    );
                                }
                                if let Some(texture) = icon_texture(
                                    ctx,
                                    surface.icon_id(),
                                    Style::ICON_L,
                                    Style::resolve_color(ctx, Style::TEXT),
                                ) {
                                    let icon_rect = egui::Rect::from_center_size(
                                        egui::pos2(rect.left() + Style::SP_L, rect.center().y),
                                        egui::vec2(Style::ICON_L, Style::ICON_L),
                                    );
                                    ui.painter().image(
                                        texture.id(),
                                        icon_rect,
                                        egui::Rect::from_min_max(
                                            egui::Pos2::ZERO,
                                            egui::pos2(1.0, 1.0),
                                        ),
                                        Style::resolve_color(ctx, Style::TEXT),
                                    );
                                }
                                let label = taskbar_surface_label(surface);
                                let galley = ui.painter().layout_job(Style::typography_job(
                                    label,
                                    TypographyRole::Label,
                                    Style::resolve_color(ctx, Style::TEXT),
                                    f32::INFINITY,
                                ));
                                ui.painter().galley(
                                    egui::pos2(
                                        rect.left() + Style::SP_XL,
                                        rect.center().y - galley.size().y / 2.0,
                                    ),
                                    galley,
                                    Style::resolve_color(ctx, Style::TEXT),
                                );
                                let control = Control {
                                    kind: ControlKind::SurfaceLauncher,
                                    rect,
                                    surface: Some(surface),
                                    source_index: None,
                                };
                                install_accessibility(ctx, self.mode, control, label, self.is_docked());
                                let _response = response.clone().on_hover_ui(|ui| {
                                    nav_bar_tooltip(ui, label);
                                });
                                if response.clicked() {
                                    toggled = Some(surface);
                                }
                            }
                        });
                    ui.add_space(Style::SP_S);
                    if ui
                        .add_sized(
                            egui::vec2(
                                (card.width() - 2.0 * Style::SP_L).max(1.0),
                                40.0,
                            ),
                            egui::Button::new("Continue with selected pins"),
                        )
                        .clicked()
                    {
                        complete = true;
                    }
                });
            });
        ctx.move_to_top(area.response.layer_id);
        self.pin_selector.query = bounded_pin_selector_query(&query);
        if let Some(surface) = toggled {
            self.toggle_pin_selector_surface(surface);
        }
        if complete {
            self.complete_first_boot();
        }
    }

    fn geometry(&self, screen: egui::Rect, now: Instant) -> Geometry {
        self.geometry_for(screen, now, 0)
    }

    fn geometry_for(&self, screen: egui::Rect, now: Instant, pinned_count: usize) -> Geometry {
        self.geometry_for_with_sessions(screen, now, pinned_count, &[])
    }

    fn geometry_for_with_sessions(
        &self,
        screen: egui::Rect,
        now: Instant,
        pinned_count: usize,
        connected_sessions: &[SessionRailEntry],
    ) -> Geometry {
        let floating = floating_geometry_for_catalog_with_sessions(
            screen,
            pinned_count,
            &self.pinned_surfaces,
            connected_sessions,
        );
        let docked = docked_geometry_for_catalog_with_sessions(
            screen,
            pinned_count,
            &self.pinned_surfaces,
            connected_sessions,
        );
        let Some(transition) = self.transition else {
            return match self.mode {
                DockMode::Floating => floating,
                DockMode::Docked => docked,
            };
        };
        let elapsed = now.saturating_duration_since(transition.started);
        let raw = (elapsed.as_secs_f32() / transition_duration(transition.motion).as_secs_f32())
            .clamp(0.0, 1.0);
        if raw >= 1.0 {
            return match transition.to {
                DockMode::Floating => floating,
                DockMode::Docked => docked,
            };
        }
        if transition.from == DockMode::Floating && transition.to == DockMode::Docked {
            let staging = floating_left_edge_staging(screen, &floating);
            if raw < SLIDE_FRACTION {
                let t = transition_progress(
                    Duration::from_secs_f32(
                        (raw / SLIDE_FRACTION)
                            * transition_duration(transition.motion).as_secs_f32(),
                    ),
                    transition.motion,
                );
                return interpolate_geometry(&floating, &staging, t, false);
            }
            let t = transition_progress(
                Duration::from_secs_f32(
                    ((raw - SLIDE_FRACTION) / (1.0 - SLIDE_FRACTION))
                        * transition_duration(transition.motion).as_secs_f32(),
                ),
                transition.motion,
            );
            return interpolate_geometry(&staging, &docked, t, false);
        }
        let staging = floating_left_edge_staging(screen, &floating);
        if raw < 1.0 - SLIDE_FRACTION {
            let t = transition_progress(
                Duration::from_secs_f32(
                    (raw / (1.0 - SLIDE_FRACTION))
                        * transition_duration(transition.motion).as_secs_f32(),
                ),
                transition.motion,
            );
            interpolate_geometry(&docked, &staging, t, false)
        } else {
            let t = transition_progress(
                Duration::from_secs_f32(
                    ((raw - (1.0 - SLIDE_FRACTION)) / SLIDE_FRACTION)
                        * transition_duration(transition.motion).as_secs_f32(),
                ),
                transition.motion,
            );
            interpolate_geometry(&staging, &floating, t, false)
        }
    }

    fn default_path() -> Option<PathBuf> {
        mde_bus::client_data_dir().map(|dir| dir.join(CONFIG_FILE))
    }

    fn save(&self) {
        if let Some(path) = Self::default_path() {
            let prefs = NavBarPrefs {
                schema_version: NAV_PREFS_SCHEMA_VERSION,
                mode: self.mode,
                pinned_surfaces: self
                    .pinned_surfaces
                    .iter()
                    .map(|surface| surface_key(*surface).to_owned())
                    .collect(),
                profile_state: self.profile_state,
                peer_app_favorites: self.peer_app_favorites.clone(),
            };
            let _ = save_to(&path, prefs);
        }
    }
}

/// Read the persisted dock preference without following a final symlink and
/// without allowing an unbounded local file to reach serde.
fn read_bounded_config(path: &Path) -> std::io::Result<String> {
    use std::io::Read as _;

    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        options.custom_flags(0o400000); // O_NOFOLLOW
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        options.custom_flags(0x100); // O_NOFOLLOW
        #[cfg(not(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios"
        )))]
        if !fs::symlink_metadata(path)?.file_type().is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "navigation preference is not a regular file",
            ));
        }
    }
    #[cfg(not(unix))]
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "navigation preference is not a regular file",
        ));
    }

    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "navigation preference is not a regular file",
        ));
    }
    if metadata.len() > MAX_NAV_PREFS_BYTES as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("navigation preference exceeds {MAX_NAV_PREFS_BYTES}-byte limit"),
        ));
    }

    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(MAX_NAV_PREFS_BYTES)
            .saturating_add(1),
    );
    file.take((MAX_NAV_PREFS_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_NAV_PREFS_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("navigation preference exceeds {MAX_NAV_PREFS_BYTES}-byte limit"),
        ));
    }
    String::from_utf8(bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlKind {
    Start,
    Search,
    Back,
    Home,
    /// The Documents editor lives in the Workspace lane, not as a separate
    /// top-level surface.
    Editor,
    Pin,
    Overflow,
    SurfaceLauncher,
    PinnedDesktop,
    RemoteSession,
}

impl ControlKind {
    const fn id_suffix(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Search => "search",
            Self::Back => "back",
            Self::Home => "home",
            Self::Editor => "editor",
            Self::Pin => "pin",
            Self::Overflow => "overflow",
            Self::SurfaceLauncher => "surface",
            Self::PinnedDesktop => "pinned-desktop",
            Self::RemoteSession => "remote-session",
        }
    }

    const fn icon(self) -> IconId {
        match self {
            Self::Start => IconId::Grid,
            Self::Search => IconId::Search,
            Self::Back => IconId::ArrowLeft,
            Self::Home => IconId::FileHome,
            Self::Editor => IconId::Editor,
            // The placement toggle lives at the far edge of both layouts; use
            // Carbon's canonical PIN glyph so the control communicates its
            // pinning action instead of borrowing the application-menu icon.
            Self::Pin => IconId::Pin,
            Self::Overflow => IconId::MoreHorizontal,
            Self::SurfaceLauncher => IconId::Mark,
            Self::PinnedDesktop => IconId::Desktop,
            Self::RemoteSession => IconId::Desktop,
        }
    }

    const fn tooltip(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Search => "Search",
            Self::Back => "Back",
            Self::Home => "Home",
            Self::Editor => "Editor - Workspace",
            Self::Pin => "Taskbar placement menu",
            Self::Overflow => "More taskbar apps",
            Self::SurfaceLauncher => "Open app",
            Self::PinnedDesktop => "Open pinned desktop",
            Self::RemoteSession => "Open remote session",
        }
    }

    const fn action(self) -> Action {
        match self {
            Self::Start => Action::OpenFrontDoor,
            Self::Search => Action::FocusSearch,
            Self::Back => Action::Back,
            Self::Home => Action::Home,
            Self::Editor => Action::OpenEditor,
            Self::Pin => Action::ToggleDock,
            // The More control is handled by the popup toggle in `mount`; it
            // never reaches this action mapping.
            Self::Overflow => Action::Home,
            Self::SurfaceLauncher => Action::Home,
            Self::PinnedDesktop => Action::Home,
            Self::RemoteSession => Action::Home,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Control {
    kind: ControlKind,
    rect: egui::Rect,
    surface: Option<Surface>,
    source_index: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverflowItem {
    Surface(Surface),
    RemoteSession(usize),
    PinnedDesktop(usize),
}

#[derive(Debug, Clone)]
struct OverflowGeometry {
    items: Vec<OverflowItem>,
}

#[derive(Debug, Clone)]
struct OverflowLayout {
    outer: egui::Rect,
    rows: Vec<egui::Rect>,
}

#[derive(Debug, Clone)]
struct Geometry {
    outer: egui::Rect,
    radius: egui::CornerRadius,
    controls: Vec<Control>,
    overflow: Option<OverflowGeometry>,
    finished: bool,
}

fn floating_geometry(screen: egui::Rect) -> Geometry {
    floating_geometry_for(screen, 0)
}

fn dock_launcher_count() -> usize {
    default_taskbar_pins().len()
}

fn dock_control_capacity(pinned_count: usize) -> usize {
    7 + dock_launcher_count() + pinned_count
}

fn dock_control_capacity_for(pinned_count: usize, surface_count: usize) -> usize {
    7 + surface_count + pinned_count
}

fn control_span(count: usize, edge: f32, gap: f32) -> f32 {
    if count == 0 {
        0.0
    } else {
        count as f32 * edge + (count - 1) as f32 * gap
    }
}

fn floating_center_bounds(screen: egui::Rect, gap: f32) -> (f32, f32) {
    let left_cluster_end = screen.left() + control_span(5, CONTROL_EDGE, gap);
    let left_safe_edge = left_cluster_end + gap;
    // Placement occupies the tray's reserved final 40px lane; do not reserve
    // a phantom second placement target to the left of that tray.
    let right_safe_edge = bottom_tray_rect(screen).left() - BOTTOM_TRAY_GAP;

    // The center lane is anchored to the physical screen, not to whatever
    // asymmetric space happens to remain between navigation and the tray.
    // Mirror the larger side reservation so either cluster can grow without
    // shifting the user's workspace strip away from the display midpoint.
    let left_reservation = (left_safe_edge - screen.left()).max(0.0);
    let right_reservation = (screen.right() - right_safe_edge).max(0.0);
    let symmetric_reservation = left_reservation
        .max(right_reservation)
        .min((screen.width() / 2.0).max(0.0));
    (
        screen.left() + symmetric_reservation,
        screen.right() - symmetric_reservation,
    )
}

fn floating_center_capacity(screen: egui::Rect, gap: f32) -> usize {
    let (center_left, center_right) = floating_center_bounds(screen, gap);
    let reserved_capacity =
        (((center_right - center_left).max(0.0) + gap) / (CONTROL_EDGE + gap)).floor() as usize;
    if reserved_capacity > 0 {
        return reserved_capacity;
    }

    // On compact screens the responsive status tray can consume its entire
    // preferred reservation even though one physical center target still fits
    // between the fixed Home cluster and placement control. Admit that one
    // slot only from exact hit geometry; at 320px it correctly remains zero
    // because a centered 40px target would intersect Home.
    let centered = egui::Rect::from_center_size(
        egui::pos2(screen.center().x, screen.bottom() - TASKBAR_H / 2.0),
        egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
    );
    let fixed_left_edge = screen.left() + control_span(5, CONTROL_EDGE, gap) + gap;
    let placement_left = screen.right() - Style::SP_S - CONTROL_EDGE - gap;
    usize::from(centered.left() >= fixed_left_edge && centered.right() <= placement_left)
}

/// Select the catalog entries that remain in the fixed rail and preserve the
/// rest, in catalog order, for the More flyout. The overflow control itself
/// consumes one center slot, so the visible entries remain centered with the
/// existing taskbar pin order intact.
fn catalog_selection(
    surfaces: &[Surface],
    pinned_count: usize,
    capacity: usize,
) -> (usize, usize, Option<OverflowGeometry>) {
    let (surface_count, _session_count, pinned_count, overflow) =
        catalog_selection_with_sessions(surfaces, 0, pinned_count, capacity);
    (surface_count, pinned_count, overflow)
}

fn catalog_selection_with_sessions(
    surfaces: &[Surface],
    session_count: usize,
    pinned_count: usize,
    capacity: usize,
) -> (usize, usize, usize, Option<OverflowGeometry>) {
    let pinned_count = pinned_count.min(MAX_PINNED_SOURCES);
    let total = surfaces.len() + session_count + pinned_count;
    let has_overflow = total > capacity && capacity > 0;
    let visible_capacity = capacity.saturating_sub(usize::from(has_overflow));
    let visible_surface_count = surfaces.len().min(visible_capacity);
    let visible_session_count = session_count.min(visible_capacity - visible_surface_count);
    let visible_pinned_count = pinned_count.min(
        visible_capacity
            .saturating_sub(visible_surface_count)
            .saturating_sub(visible_session_count),
    );

    let mut overflow_items = Vec::new();
    overflow_items.extend(
        surfaces
            .iter()
            .copied()
            .skip(visible_surface_count)
            .map(OverflowItem::Surface),
    );
    overflow_items.extend((visible_session_count..session_count).map(OverflowItem::RemoteSession));
    overflow_items.extend((visible_pinned_count..pinned_count).map(OverflowItem::PinnedDesktop));

    (
        visible_surface_count,
        visible_session_count,
        visible_pinned_count,
        has_overflow.then_some(OverflowGeometry {
            items: overflow_items,
        }),
    )
}

fn floating_geometry_for(screen: egui::Rect, pinned_count: usize) -> Geometry {
    floating_geometry_for_catalog(screen, pinned_count, &default_taskbar_pins())
}

fn floating_geometry_for_catalog(
    screen: egui::Rect,
    pinned_count: usize,
    surfaces: &[Surface],
) -> Geometry {
    floating_geometry_for_catalog_with_sessions(screen, pinned_count, surfaces, &[])
}

fn floating_geometry_for_catalog_with_sessions(
    screen: egui::Rect,
    pinned_count: usize,
    surfaces: &[Surface],
    connected_sessions: &[SessionRailEntry],
) -> Geometry {
    let gap = FLOATING_GAP;
    let center_capacity = floating_center_capacity(screen, gap);
    let has_catalog_items = !surfaces.is_empty()
        || !connected_sessions.is_empty()
        || pinned_count.min(MAX_PINNED_SOURCES) > 0;
    // Never emit the Editor when the fixed navigation and status reservations
    // leave no physical center slot: doing so aliases Home's hit region. Under
    // one-slot pressure, More takes priority so hidden sessions and apps remain
    // reachable; Editor remains available through Communications/Documents.
    let show_editor = center_capacity > usize::from(has_catalog_items);
    let (surface_count, session_count, pinned_count, overflow) = catalog_selection_with_sessions(
        surfaces,
        connected_sessions.len(),
        pinned_count,
        center_capacity.saturating_sub(usize::from(show_editor)),
    );
    let edge = CONTROL_EDGE;
    let outer = egui::Rect::from_min_size(
        egui::pos2(screen.left(), screen.bottom() - TASKBAR_H),
        egui::vec2(screen.width(), TASKBAR_H),
    );
    let y = outer.top() + (TASKBAR_H - edge) / 2.0;
    // Search is the leading taskbar affordance; do not reserve a decorative
    // blank lane before its fixed hit target.
    let left_start = outer.left();
    // Bottom placement reserves the rightmost taskbar slot for Show Desktop;
    // the status clock leaves this exact lane immediately to its right.
    let right_x = (outer.right() - Style::SP_S - edge).max(outer.left());
    let mut controls = Vec::with_capacity(dock_control_capacity_for(pinned_count, surface_count));
    let mut cursor_x = left_start;
    // Start and Search are distinct typed controls over the one Front Door.
    // Workloads is the one fixed workspace immediately after them, followed by
    // Back and Home. This keeps the high-frequency controls grouped and
    // identical in bottom and left-rail placements.
    for (kind, surface) in [
        (ControlKind::Start, None),
        (ControlKind::Search, None),
        (ControlKind::SurfaceLauncher, Some(Surface::InfraCode)),
        (ControlKind::Back, None),
        (ControlKind::Home, None),
    ] {
        let rect = egui::Rect::from_min_size(egui::pos2(cursor_x, y), egui::vec2(edge, edge));
        // At widths below the normal supported profile, keep the placement
        // escape reachable instead of letting the fixed left cluster steal
        // its hit region. The omitted controls remain available through
        // Front Door/search and the overflow catalog.
        if rect.right() <= right_x {
            controls.push(Control {
                kind,
                rect,
                surface,
                source_index: None,
            });
        }
        cursor_x += edge + gap;
    }
    let center_count = usize::from(show_editor)
        + surface_count
        + session_count
        + pinned_count
        + usize::from(overflow.is_some());
    let center_span = control_span(center_count, edge, gap);
    let (center_left, center_right) = floating_center_bounds(screen, gap);
    let center_start = (center_left + center_right) / 2.0 - center_span / 2.0;
    cursor_x = center_start;
    if show_editor {
        controls.push(Control {
            kind: ControlKind::Editor,
            rect: egui::Rect::from_min_size(egui::pos2(cursor_x, y), egui::vec2(edge, edge)),
            surface: None,
            source_index: None,
        });
        cursor_x += edge + gap;
    }
    for surface in surfaces.iter().copied().take(surface_count) {
        controls.push(Control {
            kind: ControlKind::SurfaceLauncher,
            rect: egui::Rect::from_min_size(egui::pos2(cursor_x, y), egui::vec2(edge, edge)),
            surface: Some(surface),
            source_index: None,
        });
        cursor_x += edge + gap;
    }
    for session_index in 0..session_count {
        controls.push(Control {
            kind: ControlKind::RemoteSession,
            rect: egui::Rect::from_min_size(egui::pos2(cursor_x, y), egui::vec2(edge, edge)),
            surface: None,
            source_index: Some(session_index),
        });
        cursor_x += edge + gap;
    }
    for source_index in 0..pinned_count {
        controls.push(Control {
            kind: ControlKind::PinnedDesktop,
            rect: egui::Rect::from_min_size(egui::pos2(cursor_x, y), egui::vec2(edge, edge)),
            surface: None,
            source_index: Some(source_index),
        });
        cursor_x += edge + gap;
    }
    if overflow.is_some() {
        controls.push(Control {
            kind: ControlKind::Overflow,
            rect: egui::Rect::from_min_size(egui::pos2(cursor_x, y), egui::vec2(edge, edge)),
            surface: None,
            source_index: None,
        });
    }
    controls.push(Control {
        kind: ControlKind::Pin,
        rect: egui::Rect::from_min_size(egui::pos2(right_x, y), egui::vec2(edge, edge)),
        surface: None,
        source_index: None,
    });
    Geometry {
        outer,
        radius: egui::CornerRadius::ZERO,
        controls,
        overflow,
        finished: true,
    }
}

fn floating_left_edge_staging(screen: egui::Rect, floating: &Geometry) -> Geometry {
    translate_geometry(
        floating,
        egui::vec2(screen.left() - floating.outer.left(), 0.0),
    )
}

fn docked_geometry(screen: egui::Rect) -> Geometry {
    docked_geometry_for(screen, 0)
}

fn docked_content_bottom(screen: egui::Rect) -> f32 {
    screen.bottom() - Style::SP_S
}

fn docked_pin_rect(screen: egui::Rect, outer: egui::Rect) -> Option<egui::Rect> {
    let y = docked_content_bottom(screen) - CONTROL_EDGE;
    let first_control_y = screen.top() + STATUS_BAR_H + Style::SP_S;
    (y >= first_control_y).then(|| {
        egui::Rect::from_min_size(
            egui::pos2(outer.center().x - CONTROL_EDGE / 2.0, y),
            egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
        )
    })
}

fn docked_control_fits_before_pin(
    screen: egui::Rect,
    cursor_y: f32,
    pin_rect: Option<egui::Rect>,
) -> bool {
    let content_bottom = pin_rect.map_or_else(
        || docked_content_bottom(screen),
        |pin| pin.top() - Style::SP_XS,
    );
    cursor_y + CONTROL_EDGE <= content_bottom
}

fn docked_geometry_for(screen: egui::Rect, pinned_count: usize) -> Geometry {
    docked_geometry_for_catalog(screen, pinned_count, &default_taskbar_pins())
}

fn docked_geometry_for_catalog(
    screen: egui::Rect,
    pinned_count: usize,
    surfaces: &[Surface],
) -> Geometry {
    docked_geometry_for_catalog_with_sessions(screen, pinned_count, surfaces, &[])
}

fn docked_geometry_for_catalog_with_sessions(
    screen: egui::Rect,
    pinned_count: usize,
    surfaces: &[Surface],
    connected_sessions: &[SessionRailEntry],
) -> Geometry {
    let outer = egui::Rect::from_min_size(
        egui::pos2(screen.left(), screen.top() + STATUS_BAR_H),
        egui::vec2(DOCKED_W, (screen.height() - STATUS_BAR_H).max(0.0)),
    );
    let mut controls = Vec::with_capacity(dock_control_capacity_for(
        pinned_count,
        surfaces.len() + connected_sessions.len(),
    ));
    // Placement is the escape hatch from a persisted Left rail. Reserve it at
    // the physical bottom edge before admitting any other controls so a short
    // remote viewport cannot strand the profile in Left mode after restart.
    let pin_rect = docked_pin_rect(screen, outer);
    let mut cursor_y = screen.top() + STATUS_BAR_H + Style::SP_S;
    let fixed_controls = if screen.height() <= 400.0 {
        [
            (ControlKind::Start, None),
            (ControlKind::SurfaceLauncher, Some(Surface::InfraCode)),
            (ControlKind::Back, None),
            (ControlKind::Search, None),
            (ControlKind::Home, None),
        ]
    } else {
        [
            (ControlKind::Start, None),
            (ControlKind::Search, None),
            (ControlKind::SurfaceLauncher, Some(Surface::InfraCode)),
            (ControlKind::Back, None),
            (ControlKind::Home, None),
        ]
    };
    for (kind, surface) in fixed_controls {
        // A short portrait/remote viewport may not have room for the complete
        // fixed cluster. Admit controls one at a time so the Left rail never
        // paints a hit target outside its owned display rect; the remaining
        // catalog is handled by the bounded More path below.
        if !docked_control_fits_before_pin(screen, cursor_y, pin_rect) {
            break;
        }
        controls.push(Control {
            kind,
            rect: egui::Rect::from_min_size(
                egui::pos2(outer.center().x - CONTROL_EDGE / 2.0, cursor_y),
                egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
            ),
            surface,
            source_index: None,
        });
        cursor_y += CONTROL_EDGE + Style::SP_XS;
    }
    cursor_y += Style::SP_S - Style::SP_XS;
    let mut available_slots: usize = 0;
    let mut slot_y = cursor_y;
    while docked_control_fits_before_pin(screen, slot_y, pin_rect) {
        available_slots += 1;
        slot_y += CONTROL_EDGE + Style::SP_XS;
    }
    // Editor consumes one of the remaining pre-placement slots. Placement was
    // already reserved outside this budget.
    let (surface_count, session_count, pinned_count, overflow) = catalog_selection_with_sessions(
        surfaces,
        connected_sessions.len(),
        pinned_count,
        available_slots.saturating_sub(1),
    );
    if docked_control_fits_before_pin(screen, cursor_y, pin_rect) {
        controls.push(Control {
            kind: ControlKind::Editor,
            rect: egui::Rect::from_min_size(
                egui::pos2(outer.center().x - CONTROL_EDGE / 2.0, cursor_y),
                egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
            ),
            surface: None,
            source_index: None,
        });
        cursor_y += CONTROL_EDGE + Style::SP_XS;
    }
    for surface in surfaces.iter().copied().take(surface_count) {
        if !docked_control_fits_before_pin(screen, cursor_y, pin_rect) {
            break;
        }
        controls.push(Control {
            kind: ControlKind::SurfaceLauncher,
            rect: egui::Rect::from_min_size(
                egui::pos2(outer.center().x - CONTROL_EDGE / 2.0, cursor_y),
                egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
            ),
            surface: Some(surface),
            source_index: None,
        });
        cursor_y += CONTROL_EDGE + Style::SP_XS;
    }
    for session_index in 0..session_count {
        controls.push(Control {
            kind: ControlKind::RemoteSession,
            rect: egui::Rect::from_min_size(
                egui::pos2(outer.center().x - CONTROL_EDGE / 2.0, cursor_y),
                egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
            ),
            surface: None,
            source_index: Some(session_index),
        });
        cursor_y += CONTROL_EDGE + Style::SP_XS;
    }
    for source_index in 0..pinned_count {
        if !docked_control_fits_before_pin(screen, cursor_y, pin_rect) {
            break;
        }
        controls.push(Control {
            kind: ControlKind::PinnedDesktop,
            rect: egui::Rect::from_min_size(
                egui::pos2(outer.center().x - CONTROL_EDGE / 2.0, cursor_y),
                egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
            ),
            surface: None,
            source_index: Some(source_index),
        });
        cursor_y += CONTROL_EDGE + Style::SP_XS;
    }
    if overflow.is_some() && docked_control_fits_before_pin(screen, cursor_y, pin_rect) {
        controls.push(Control {
            kind: ControlKind::Overflow,
            rect: egui::Rect::from_min_size(
                egui::pos2(outer.center().x - CONTROL_EDGE / 2.0, cursor_y),
                egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
            ),
            surface: None,
            source_index: None,
        });
    }
    if let Some(rect) = pin_rect {
        controls.push(Control {
            kind: ControlKind::Pin,
            rect,
            surface: None,
            source_index: None,
        });
    }
    Geometry {
        outer,
        radius: egui::CornerRadius {
            nw: 0,
            ne: 0,
            sw: 0,
            se: Style::RADIUS_L as u8,
        },
        controls,
        overflow,
        finished: true,
    }
}

fn interpolate_geometry(from: &Geometry, to: &Geometry, t: f32, finished: bool) -> Geometry {
    let outer = lerp_rect(from.outer, to.outer, t);
    let controls = from
        .controls
        .iter()
        .zip(&to.controls)
        .map(|(from, to)| Control {
            kind: from.kind,
            rect: lerp_rect(from.rect, to.rect, t),
            surface: from.surface,
            source_index: from.source_index,
        })
        .collect();
    Geometry {
        outer,
        radius: lerp_radius(from.radius, to.radius, t),
        controls,
        overflow: if t < 0.5 {
            from.overflow.clone()
        } else {
            to.overflow.clone()
        },
        finished,
    }
}

fn translate_geometry(geometry: &Geometry, delta: egui::Vec2) -> Geometry {
    Geometry {
        outer: geometry.outer.translate(delta),
        radius: geometry.radius,
        controls: geometry
            .controls
            .iter()
            .copied()
            .map(|control| Control {
                kind: control.kind,
                rect: control.rect.translate(delta),
                surface: control.surface,
                source_index: control.source_index,
            })
            .collect(),
        overflow: geometry.overflow.clone(),
        finished: geometry.finished,
    }
}

fn paint_backing(painter: &egui::Painter, geometry: &Geometry) {
    let shadow = geometry.outer.translate(egui::vec2(0.0, 3.0));
    painter.rect_filled(shadow, geometry.radius, Elevation::Overlay.shadow().umbra);
    painter.rect_filled(geometry.outer, geometry.radius, Style::NAV_BAR_BG);
    painter.line_segment(
        [geometry.outer.left_top(), geometry.outer.right_top()],
        Style::hairline(),
    );
}

#[cfg(test)]
pub(crate) fn floating_pin_center(screen: egui::Rect) -> egui::Pos2 {
    floating_geometry_for(screen, 0)
        .controls
        .iter()
        .find(|control| control.kind == ControlKind::Pin)
        .expect("floating taskbar placement control")
        .rect
        .center()
}

fn nav_bar_tooltip(ui: &mut egui::Ui, text: &str) {
    mde_egui::overlay()
        .corner_radius(mde_egui::corner(Style::RADIUS_S))
        .inner_margin(Style::tooltip_margin())
        .show(ui, |ui| {
            ui.set_max_width(Style::SP_XL * 12.0);
            ui.label(Style::typography_text(text, TypographyRole::Caption).color(Style::TEXT));
        });
}

fn control_id(mode: DockMode, control: Control) -> egui::Id {
    match (control.source_index, control.surface) {
        // Session and pinned-desktop indices come from independent projections.
        // Include the control kind so index zero in each projection cannot
        // alias egui's interaction, focus, or accessibility state.
        (Some(index), _) => egui::Id::new((
            "construct-navigation-bar",
            mode.id_suffix(),
            control.kind.id_suffix(),
            index,
        )),
        (None, Some(surface)) => egui::Id::new((
            "construct-navigation-bar",
            mode.id_suffix(),
            "surface",
            surface,
        )),
        (None, None) => egui::Id::new((
            "construct-navigation-bar",
            mode.id_suffix(),
            control.kind.id_suffix(),
        )),
    }
}

fn overflow_popup_id(mode: DockMode) -> egui::Id {
    egui::Id::new(("construct-navigation-bar", mode.id_suffix(), "overflow"))
}

fn first_boot_selector_rect(screen: egui::Rect) -> egui::Rect {
    // Keep the preferred inset and minimum card size when the viewport can
    // support them, but never let first boot create an off-screen modal on a
    // narrow seat or a small render target. The card may use the full viewport
    // at the smallest sizes; its controls remain bounded below as well.
    let width = FIRST_BOOT_SELECTOR_W
        .min((screen.width() - 2.0 * Style::SP_XL).max(280.0))
        .min(screen.width().max(1.0));
    let height = FIRST_BOOT_SELECTOR_H
        .min((screen.height() - 2.0 * Style::SP_XL).max(240.0))
        .min(screen.height().max(1.0));
    egui::Rect::from_center_size(screen.center(), egui::vec2(width, height))
}

fn overflow_label(item_count: usize) -> String {
    match item_count {
        1 => "More taskbar apps (1 hidden)".to_owned(),
        count => format!("More taskbar apps ({count} hidden)"),
    }
}

/// Return the single focused-workspace marker required by the Construct
/// taskbar contract. Keep this geometry independent of theme spacing so a
/// change to global padding cannot silently turn the marker into a bar.
fn focus_underline_rect(target: egui::Rect) -> egui::Rect {
    egui::Rect::from_center_size(
        egui::pos2(
            target.center().x,
            target.bottom() - FOCUS_UNDERLINE_BOTTOM_GAP - FOCUS_UNDERLINE_H / 2.0,
        ),
        egui::vec2(FOCUS_UNDERLINE_W, FOCUS_UNDERLINE_H),
    )
}

fn overflow_item_control(item: OverflowItem, rect: egui::Rect) -> Control {
    match item {
        OverflowItem::Surface(surface) => Control {
            kind: ControlKind::SurfaceLauncher,
            rect,
            surface: Some(surface),
            source_index: None,
        },
        OverflowItem::RemoteSession(session_index) => Control {
            kind: ControlKind::RemoteSession,
            rect,
            surface: None,
            source_index: Some(session_index),
        },
        OverflowItem::PinnedDesktop(source_index) => Control {
            kind: ControlKind::PinnedDesktop,
            rect,
            surface: None,
            source_index: Some(source_index),
        },
    }
}

/// Preferred geometry for the More surface. The popup container may flip below
/// the anchor when a display is too short; rows remain one 40px target column
/// and the whole preferred surface stays clamped inside the screen margins.
fn overflow_layout_for(
    anchor: egui::Rect,
    screen: egui::Rect,
    item_count: usize,
) -> OverflowLayout {
    let width = OVERFLOW_W.min((screen.width() - 2.0 * Style::SP_S).max(CONTROL_EDGE));
    let row_gap = Style::SP_XS;
    let height = 2.0 * Style::SP_S
        + item_count as f32 * CONTROL_EDGE
        + item_count.saturating_sub(1) as f32 * row_gap;
    let top_margin = Style::SP_S;
    let bottom_margin = Style::SP_S;
    let preferred_top = anchor.top() - OVERFLOW_GAP - height;
    let max_top = (screen.bottom() - bottom_margin - height).max(top_margin);
    let top = preferred_top.clamp(top_margin, max_top);
    let max_left = (screen.right() - Style::SP_S - width).max(screen.left() + Style::SP_S);
    let left = anchor.left().clamp(screen.left() + Style::SP_S, max_left);
    let outer = egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(width, height));
    let rows = (0..item_count)
        .map(|index| {
            egui::Rect::from_min_size(
                egui::pos2(
                    outer.left() + Style::SP_S,
                    outer.top() + Style::SP_S + index as f32 * (CONTROL_EDGE + row_gap),
                ),
                egui::vec2((width - 2.0 * Style::SP_S).max(CONTROL_EDGE), CONTROL_EDGE),
            )
        })
        .collect();
    OverflowLayout { outer, rows }
}

fn paint_overflow_popup(
    ui: &mut egui::Ui,
    mode: DockMode,
    anchor: egui::Rect,
    items: &[OverflowItem],
    pinned_sources: &[crate::surfaces::DesktopRailSource],
    connected_sessions: &[SessionRailEntry],
    action: &mut Option<Action>,
) {
    let layout = overflow_layout_for(anchor, ui.ctx().screen_rect(), items.len());
    ui.set_min_width(layout.outer.width());
    ui.set_min_height(layout.outer.height() - 2.0 * Style::SP_S);
    let text = Style::resolve_color(ui.ctx(), Style::TEXT);
    let hover = Style::resolve_color(ui.ctx(), Style::SURFACE_HI);

    for (item, row) in items.iter().copied().zip(layout.rows) {
        let (row, response) = ui.allocate_exact_size(row.size(), egui::Sense::click());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        let control = overflow_item_control(item, row);
        let label = control_label(control, pinned_sources, connected_sessions);
        if response.hovered() {
            ui.painter()
                .rect_filled(row, egui::CornerRadius::same(Style::RADIUS_S as u8), hover);
        }
        if let Some(texture) = icon_texture(
            ui.ctx(),
            control_icon(control),
            control_icon_size(control, row.height().min(Style::ICON_L)),
            text,
        ) {
            let icon_size = control_icon_size(control, row.height().min(Style::ICON_L));
            let icon_rect = egui::Rect::from_center_size(
                egui::pos2(row.left() + Style::SP_L, row.center().y),
                egui::vec2(icon_size, icon_size),
            );
            ui.painter().image(
                texture.id(),
                icon_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                text,
            );
        }
        let galley = ui.painter().layout_job(Style::typography_job(
            label.as_str(),
            TypographyRole::Label,
            text,
            (row.width() - Style::SP_XL * 2.0).max(0.0),
        ));
        ui.painter().galley(
            egui::pos2(
                row.left() + Style::SP_XL,
                row.center().y - galley.size().y / 2.0,
            ),
            galley,
            text,
        );
        install_accessibility(
            ui.ctx(),
            mode,
            control,
            label.as_str(),
            mode == DockMode::Docked,
        );
        let _response = response.clone().on_hover_ui(move |ui| {
            nav_bar_tooltip(ui, label.as_str());
        });
        let keyboard_activate = response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            });
        if response.clicked() || keyboard_activate {
            *action = Some(control_action(control, pinned_sources, connected_sessions));
            ui.memory_mut(|memory| memory.close_popup());
        }
    }
}

fn nav_bar_proof_enabled() -> bool {
    std::env::var_os("MDE_NAV_BAR_PROOF").is_some()
}

fn log_foreground_layer_id_map_once() {
    if NAV_LAYER_ID_MAP_LOGGED.swap(true, Ordering::Relaxed) {
        return;
    }
    let candidates = [
        (
            "construct-navigation-bar",
            egui::Id::new("construct-navigation-bar"),
        ),
        (
            "construct-status-bar",
            egui::Id::new("construct-status-bar"),
        ),
        (
            "notif-critical-edge-cue",
            egui::Id::new("notif-critical-edge-cue"),
        ),
        (
            "notif-critical-edge-cue/1",
            egui::Id::new(("notif-critical-edge-cue", 1_usize)),
        ),
        (
            "notif-critical-edge-cue/2",
            egui::Id::new(("notif-critical-edge-cue", 2_usize)),
        ),
        (
            "notif-critical-edge-cue/3",
            egui::Id::new(("notif-critical-edge-cue", 3_usize)),
        ),
        (
            "construct-notification-center",
            egui::Id::new("construct-notification-center"),
        ),
        (
            "shell-control-center",
            egui::Id::new("shell-control-center"),
        ),
        (
            "construct-switcher-area",
            egui::Id::new("construct-switcher-area"),
        ),
        (
            "shell-front-door-omnibox",
            egui::Id::new("shell-front-door-omnibox"),
        ),
        ("mcnf-osk-toggle", egui::Id::new("mcnf-osk-toggle")),
        ("mcnf-osk", egui::Id::new("mcnf-osk")),
        ("shell-curtain", egui::Id::new("shell-curtain")),
        (
            "shell-layout-profile-button",
            egui::Id::new("shell-layout-profile-button"),
        ),
        (
            "shell-layout-profile-menu",
            egui::Id::new("shell-layout-profile-menu"),
        ),
        (
            "shell-remote-sessions-fallback-button",
            egui::Id::new("shell-remote-sessions-fallback-button"),
        ),
        ("vdi-session-overlay", egui::Id::new("vdi-session-overlay")),
        (
            "browser-vm-search-suggestions-overlay",
            egui::Id::new("browser-vm-search-suggestions-overlay"),
        ),
        ("kiron-chyron-area", egui::Id::new("kiron-chyron-area")),
        ("kiron-osd-area", egui::Id::new("kiron-osd-area")),
    ];
    for (candidate, id) in candidates {
        let layer = egui::LayerId::new(egui::Order::Foreground, id);
        tracing::info!(
            target: "mde_shell_egui::nav_bar",
            candidate,
            layer = format!("{layer:?}").as_str(),
            "foreground layer candidate proof"
        );
    }
}

fn control_icon(control: Control) -> IconId {
    if let Some(surface) = control.surface {
        return surface.icon_id();
    }
    control.kind.icon()
}

fn control_icon_size(control: Control, standard: f32) -> f32 {
    if control.kind == ControlKind::Pin {
        standard * PIN_ICON_SCALE
    } else {
        standard
    }
}

/// Return the one rendered taskbar control that represents the active shell
/// surface. Legacy Fleet & Mesh views canonicalize to the same pinned entry;
/// choosing by position guarantees one underline rather than one per stale
/// duplicate in a malformed in-memory catalog.
fn focused_control_index(controls: &[Control], active_surface: Option<Surface>) -> Option<usize> {
    focused_control_index_with_sessions(controls, active_surface, &[], None)
}

fn focused_control_index_with_sessions(
    controls: &[Control],
    active_surface: Option<Surface>,
    connected_sessions: &[SessionRailEntry],
    active_session_id: Option<&str>,
) -> Option<usize> {
    if let Some(active_session_id) = active_session_id {
        if let Some(index) = controls.iter().position(|control| {
            control.kind == ControlKind::RemoteSession
                && control
                    .source_index
                    .and_then(|session_index| connected_sessions.get(session_index))
                    .and_then(SessionRailEntry::session_id)
                    == Some(active_session_id)
        }) {
            return Some(index);
        }
    }
    let active_surface = active_surface.and_then(canonical_taskbar_surface)?;
    if active_surface == Surface::Desktop {
        return controls
            .iter()
            .position(|control| control.kind == ControlKind::Home);
    }
    controls
        .iter()
        .position(|control| control.surface == Some(active_surface))
}

fn control_label(
    control: Control,
    pinned_sources: &[crate::surfaces::DesktopRailSource],
    connected_sessions: &[SessionRailEntry],
) -> String {
    if control.kind == ControlKind::RemoteSession {
        return control
            .source_index
            .and_then(|index| connected_sessions.get(index))
            .map_or_else(
                || "Open remote session".to_owned(),
                |session| format!("Open {}", session.label()),
            );
    }
    if let Some(source) = control
        .source_index
        .and_then(|index| pinned_sources.get(index))
    {
        return format!("Open pinned desktop {} on {}", source.label, source.node);
    }
    if let Some(surface) = control.surface {
        return format!("Open {}", taskbar_surface_label(surface));
    }
    if control.kind == ControlKind::Pin {
        "Taskbar placement menu".to_owned()
    } else {
        control.kind.tooltip().to_owned()
    }
}

fn control_action(
    control: Control,
    pinned_sources: &[crate::surfaces::DesktopRailSource],
    connected_sessions: &[SessionRailEntry],
) -> Action {
    if control.kind == ControlKind::RemoteSession {
        if let Some(id) = control
            .source_index
            .and_then(|index| connected_sessions.get(index))
            .and_then(SessionRailEntry::session_id)
        {
            return Action::RemoteSession(id.to_owned());
        }
        return Action::Home;
    }
    if let Some(source) = control
        .source_index
        .and_then(|index| pinned_sources.get(index))
    {
        return Action::DesktopSource(source.id.clone());
    }
    if let Some(surface) = control.surface {
        return Action::OpenSurface(surface);
    }
    control.kind.action()
}

fn taskbar_context_menu(
    response: &egui::Response,
    control: Control,
    pinned_surfaces: &[Surface],
) -> Option<Action> {
    let mut action = None;
    response.context_menu(|ui| {
        if control.kind == ControlKind::Pin {
            ui.label(Style::typography_text(
                "Taskbar apps",
                TypographyRole::Label,
            ));
            for surface in PIN_CATALOG
                .into_iter()
                .filter(|surface| pin_catalog_surface(*surface))
            {
                let pinned = pinned_surfaces.contains(&surface);
                let label = if pinned {
                    format!("Unpin {} from taskbar", surface.label())
                } else {
                    format!("Pin {} to taskbar", surface.label())
                };
                if ui.button(label).clicked() {
                    action = Some(if pinned {
                        Action::UnpinSurface(surface)
                    } else {
                        Action::PinSurface(surface)
                    });
                    ui.close_menu();
                }
            }
        } else if let Some(surface) = control.surface {
            if Surface::ALL.contains(&surface) && pinned_surfaces.contains(&surface) {
                if ui
                    .button(format!("Unpin {} from taskbar", surface.label()))
                    .clicked()
                {
                    action = Some(Action::UnpinSurface(surface));
                    ui.close_menu();
                }
            }
        }
    });
    action
}

fn install_accessibility(
    ctx: &egui::Context,
    mode: DockMode,
    control: Control,
    label: &str,
    _docked: bool,
) {
    let _ = ctx.accesskit_node_builder(control_id(mode, control), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(label.to_owned());
        node.set_bounds(egui::accesskit::Rect {
            x0: control.rect.left().into(),
            y0: control.rect.top().into(),
            x1: control.rect.right().into(),
            y1: control.rect.bottom().into(),
        });
        node.add_action(egui::accesskit::Action::Click);
    });
}

fn lerp_rect(from: egui::Rect, to: egui::Rect, t: f32) -> egui::Rect {
    egui::Rect::from_min_max(from.min.lerp(to.min, t), from.max.lerp(to.max, t))
}

fn lerp_radius(from: egui::CornerRadius, to: egui::CornerRadius, t: f32) -> egui::CornerRadius {
    let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    egui::CornerRadius {
        nw: lerp(from.nw, to.nw),
        ne: lerp(from.ne, to.ne),
        sw: lerp(from.sw, to.sw),
        se: lerp(from.se, to.se),
    }
}

fn transition_duration(mode: MotionMode) -> Duration {
    Duration::from_secs_f32(Motion::spec(MotionPreset::Whimsy).duration_for(mode))
}

fn transition_progress(elapsed: Duration, mode: MotionMode) -> f32 {
    Motion::spec(MotionPreset::Whimsy).progress_at(elapsed.as_secs_f32(), mode)
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn save_to(path: &Path, prefs: NavBarPrefs) -> std::io::Result<()> {
    use std::io::Write as _;

    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "navigation preference path has no parent",
        )
    })?;
    ensure_directory_tree(parent)?;
    let json = serde_json::to_string_pretty(&prefs)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let leaf = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "navigation preference path has an invalid filename",
            )
        })?;
    let (tmp, mut file) = (0..16)
        .find_map(|_| {
            let candidate = parent.join(format!(
                ".{leaf}.tmp.{}.{}",
                std::process::id(),
                rand::random::<u64>()
            ));
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            match options.open(&candidate) {
                Ok(file) => Some(Ok((candidate, file))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error)),
            }
        })
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "could not allocate a unique navigation preference temporary file",
            )
        })??;
    if let Err(error) = file
        .write_all(json.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    drop(file);
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    fs::File::open(parent)?.sync_all()
}

/// Create a preference directory only through real directory components. A
/// client-data path is local but still crosses a filesystem trust boundary;
/// never let a planted symlink redirect a navigation preference write.
fn ensure_directory_tree(path: &Path) -> std::io::Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "navigation preference parent is not a real directory: {}",
                        current.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
                let metadata = fs::symlink_metadata(&current)?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!(
                            "created navigation preference parent is unsafe: {}",
                            current.display()
                        ),
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_catalogued_springboard_dock() {
        assert_eq!(State::default().mode, DockMode::Floating);
        assert!((TASKBAR_H - 48.0).abs() < f32::EPSILON);
        assert_eq!(DOCKED_W, 56.0);
        assert!((SPRINGBOARD_DOCK_RESERVED_H - 48.0).abs() < f32::EPSILON);
    }

    #[test]
    fn dock_toggle_persists_and_reverses() {
        let mut state = State::default();
        let now = Instant::now();
        state.toggle_mode(now, MotionMode::Disabled);
        assert!(state.is_docked());
        state.toggle_mode(now, MotionMode::Disabled);
        assert!(!state.is_docked());
    }

    #[test]
    fn reduced_motion_keeps_a_brief_taskbar_transition() {
        let mut state = State::default();
        let now = Instant::now();
        state.toggle_mode(now, MotionMode::Reduced);
        assert!(state.is_docked());
        assert_eq!(state.chrome_alphas(now), (1.0, 0.0));
        let settled = now + Duration::from_millis(50);
        assert_eq!(state.chrome_alphas(settled), (0.0, 1.0));
    }

    #[test]
    fn chrome_alphas_crossfade_the_top_strip_and_bottom_tray() {
        let started = Instant::now();
        let state = State {
            mode: DockMode::Docked,
            transition: Some(TransitionState {
                from: DockMode::Floating,
                to: DockMode::Docked,
                started,
                motion: MotionMode::Normal,
            }),
            ..State::default()
        };
        let (bottom, top) = state.chrome_alphas(started + TRANSITION / 2);
        assert!((bottom - 0.5).abs() < 0.01);
        assert!((top - 0.5).abs() < 0.01);
        assert_eq!(state.chrome_alphas(started), (1.0, 0.0));
        assert_eq!(state.chrome_alphas(started + TRANSITION), (0.0, 1.0));
    }

    #[test]
    fn dock_launcher_accessibility_labels_use_operator_terms() {
        let desktop = Control {
            kind: ControlKind::SurfaceLauncher,
            rect: egui::Rect::NOTHING,
            surface: Some(Surface::Desktop),
            source_index: None,
        };
        let files = Control {
            kind: ControlKind::SurfaceLauncher,
            rect: egui::Rect::NOTHING,
            surface: Some(Surface::Files),
            source_index: None,
        };
        assert_eq!(control_label(desktop, &[], &[]), "Open Remote Sessions");
        assert_eq!(control_label(files, &[], &[]), "Open Files");
    }

    #[test]
    fn taskbar_s2_start_and_search_have_distinct_typed_front_door_actions() {
        let start = Control {
            kind: ControlKind::Start,
            rect: egui::Rect::NOTHING,
            surface: None,
            source_index: None,
        };
        let search = Control {
            kind: ControlKind::Search,
            rect: egui::Rect::NOTHING,
            surface: None,
            source_index: None,
        };
        let home = Control {
            kind: ControlKind::Home,
            rect: egui::Rect::NOTHING,
            surface: None,
            source_index: None,
        };
        assert_eq!(control_icon(start), IconId::Grid);
        assert_eq!(control_icon(search), IconId::Search);
        assert_eq!(control_icon(home), IconId::FileHome);
        assert_eq!(control_action(start, &[], &[]), Action::OpenFrontDoor);
        assert_eq!(control_action(search, &[], &[]), Action::FocusSearch);
    }

    #[test]
    fn placement_pin_uses_carbon_pin_and_is_sixty_percent_smaller() {
        let pin = Control {
            kind: ControlKind::Pin,
            rect: egui::Rect::NOTHING,
            surface: None,
            source_index: None,
        };
        let standard = Style::ICON_L;
        assert_eq!(control_icon(pin), IconId::Pin);
        assert!((control_icon_size(pin, standard) - standard * 0.4).abs() < f32::EPSILON);
        assert!(
            (control_icon_size(
                Control {
                    kind: ControlKind::Start,
                    ..pin
                },
                standard,
            ) - standard)
                .abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn floating_and_docked_geometry_have_the_locked_edges() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let floating = floating_geometry(screen);
        let docked = docked_geometry(screen);
        assert!((floating.outer.width() - screen.width()).abs() < 0.001);
        assert!((floating.outer.height() - TASKBAR_H).abs() < 0.001);
        assert_eq!(floating.outer.left(), screen.left());
        assert_eq!(floating.outer.right(), screen.right());
        assert_eq!(floating.outer.bottom(), screen.bottom());
        assert_eq!(docked.outer.width(), DOCKED_W);
        assert_eq!(docked.outer.top(), screen.top() + STATUS_BAR_H);
        assert_eq!(docked.controls[0].rect.top(), STATUS_BAR_H + Style::SP_S);
        assert_eq!(docked.controls.len(), dock_control_capacity(0));
    }

    #[test]
    fn floating_dock_is_bottom_centered_and_caps_pins_to_available_width() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let geometry = floating_geometry_for(screen, MAX_PINNED_SOURCES);

        assert_eq!(geometry.outer.left(), screen.left());
        assert_eq!(geometry.outer.right(), screen.right());
        assert_eq!(geometry.outer.center().x, screen.center().x);
        assert!(
            geometry
                .controls
                .iter()
                .filter(|control| control.kind == ControlKind::PinnedDesktop)
                .count()
                < MAX_PINNED_SOURCES,
            "bottom taskbar must drop excess chooser pins instead of painting controls past the screen"
        );
        assert_hit_targets_inside_backing("floating narrow screen".to_string(), &geometry);
    }

    #[test]
    fn bottom_workspace_strip_tracks_the_physical_screen_center() {
        for screen in [
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(480.0, 480.0)),
            egui::Rect::from_min_size(egui::pos2(73.0, 41.0), egui::vec2(800.0, 600.0)),
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0)),
            egui::Rect::from_min_size(egui::pos2(120.0, 80.0), egui::vec2(1920.0, 1080.0)),
        ] {
            let (center_left, center_right) = floating_center_bounds(screen, FLOATING_GAP);
            assert!(
                ((center_left - screen.left()) - (screen.right() - center_right)).abs() < 0.001,
                "center gutters must be symmetric for {screen:?}"
            );

            let geometry = floating_geometry_for(screen, MAX_PINNED_SOURCES);
            let centered_controls = geometry.controls.iter().filter(|control| {
                !matches!(
                    control.kind,
                    ControlKind::Start
                        | ControlKind::Search
                        | ControlKind::Back
                        | ControlKind::Home
                        | ControlKind::Pin
                ) && control.surface != Some(Surface::InfraCode)
            });
            let (strip_left, strip_right, count) = centered_controls.fold(
                (f32::INFINITY, f32::NEG_INFINITY, 0_usize),
                |(left, right, count), control| {
                    (
                        left.min(control.rect.left()),
                        right.max(control.rect.right()),
                        count + 1,
                    )
                },
            );
            assert!(
                count > 0,
                "the centered workspace lane must remain reachable"
            );
            assert!(
                ((strip_left + strip_right) / 2.0 - screen.center().x).abs() < 0.001,
                "workspace strip drifted away from the physical center for {screen:?}"
            );
        }
    }

    #[test]
    fn bottom_taskbar_keeps_fixed_targets_before_overflowing_the_catalog() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(480.0, 480.0));
        let geometry = floating_geometry_for(screen, MAX_PINNED_SOURCES);

        assert_eq!(geometry.outer.center().x, screen.center().x);
        assert_eq!(geometry.outer.left(), screen.left());
        assert_eq!(geometry.outer.right(), screen.right());
        assert!(
            geometry
                .controls
                .iter()
                .filter(|control| control.kind == ControlKind::SurfaceLauncher)
                .count()
                < dock_launcher_count(),
            "bottom placement must retain the launcher set when chooser pins are dropped"
        );
        assert_eq!(
            geometry
                .controls
                .iter()
                .filter(|control| control.kind == ControlKind::PinnedDesktop)
                .count(),
            0,
            "chooser pins are the first controls dropped on a narrow bottom Dock"
        );
        assert!(geometry
            .controls
            .iter()
            .all(|control| { (control.rect.width() - CONTROL_EDGE).abs() <= 0.001 }));
        assert_hit_targets_inside_backing("floating sub-640 screen".to_string(), &geometry);
    }

    #[test]
    fn bottom_taskbar_does_not_emit_center_controls_without_a_physical_slot() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 240.0));
        assert_eq!(floating_center_capacity(screen, FLOATING_GAP), 0);

        let geometry = floating_geometry_for_catalog_with_sessions(
            screen,
            MAX_PINNED_SOURCES,
            &default_taskbar_pins(),
            &[SessionRailEntry::with_session_id(
                "narrow-session",
                "Narrow desktop",
                "LIVE",
            )],
        );

        assert!(geometry.controls.iter().all(|control| {
            matches!(
                control.kind,
                ControlKind::Start
                    | ControlKind::Search
                    | ControlKind::SurfaceLauncher
                    | ControlKind::Back
                    | ControlKind::Home
                    | ControlKind::Pin
            )
        }));
        assert_eq!(
            geometry
                .controls
                .iter()
                .filter(|control| control.kind == ControlKind::SurfaceLauncher)
                .count(),
            1,
            "only the fixed Workloads launcher may remain outside the full center lane"
        );
        for (index, control) in geometry.controls.iter().enumerate() {
            assert!(geometry.outer.contains(control.rect.min));
            assert!(geometry.outer.contains(control.rect.max));
            for other in geometry.controls.iter().skip(index + 1) {
                assert!(
                    !control.rect.intersects(other.rect),
                    "narrow taskbar controls overlap: {control:?} and {other:?}"
                );
            }
        }
    }

    #[test]
    fn very_narrow_bottom_taskbar_keeps_placement_escape_disjoint() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(240.0, 240.0));
        let geometry = floating_geometry_for_catalog_with_sessions(
            screen,
            MAX_PINNED_SOURCES,
            &default_taskbar_pins(),
            &[],
        );

        let placement = geometry
            .controls
            .iter()
            .find(|control| control.kind == ControlKind::Pin)
            .copied()
            .expect("the narrow taskbar must retain a placement escape");
        assert!(geometry.outer.contains_rect(placement.rect));
        assert!(geometry.controls.iter().all(|control| {
            control.kind == ControlKind::Pin || !control.rect.intersects(placement.rect)
        }));
        assert!(geometry
            .controls
            .iter()
            .all(|control| { geometry.outer.contains_rect(control.rect) }));
    }

    #[test]
    fn short_left_rail_admits_only_controls_inside_its_display_rect() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 160.0));
        let geometry = docked_geometry_for_catalog_with_sessions(
            screen,
            MAX_PINNED_SOURCES,
            &default_taskbar_pins(),
            &[SessionRailEntry::with_session_id(
                "short-rail-session",
                "Short rail desktop",
                "LIVE",
            )],
        );

        assert!(
            geometry.controls.len() < 5,
            "a short Left rail must shed fixed controls instead of painting below the display"
        );
        for (index, control) in geometry.controls.iter().enumerate() {
            assert!(
                geometry.outer.contains(control.rect.min)
                    && geometry.outer.contains(control.rect.max),
                "control {control:?} escaped the Left rail {outer:?}",
                outer = geometry.outer
            );
            for other in geometry.controls.iter().skip(index + 1) {
                assert!(
                    !control.rect.intersects(other.rect),
                    "short Left rail controls overlap: {control:?} and {other:?}"
                );
            }
        }
    }

    #[test]
    fn persisted_left_rail_keeps_placement_escape_on_hostile_short_restart() {
        let state = State::from_prefs(NavBarPrefs {
            schema_version: NAV_PREFS_SCHEMA_VERSION,
            mode: DockMode::Docked,
            pinned_surfaces: PIN_CATALOG
                .iter()
                .map(|surface| surface_key(*surface).to_owned())
                .collect(),
            profile_state: ProfileState::Configured,
            ..NavBarPrefs::default()
        });
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 160.0));
        let geometry = state.geometry_for_with_sessions(
            screen,
            Instant::now(),
            MAX_PINNED_SOURCES,
            &[SessionRailEntry::with_session_id(
                "restart-session",
                "Restart desktop",
                "LIVE",
            )],
        );

        let placement = geometry
            .controls
            .iter()
            .find(|control| control.kind == ControlKind::Pin)
            .copied()
            .expect("persisted Left placement must retain its Bottom escape target");
        assert_eq!(control_action(placement, &[], &[]), Action::ToggleDock);
        assert!(geometry.outer.contains_rect(placement.rect));
        assert_eq!(
            placement.rect.bottom(),
            screen.bottom() - Style::SP_S,
            "placement must stay anchored to the bounded rail bottom"
        );
        assert!(geometry.controls.iter().all(|control| {
            control.kind == ControlKind::Pin || !control.rect.intersects(placement.rect)
        }));
    }

    #[test]
    fn focused_taskbar_marker_is_one_centered_18_by_3_accent_with_bottom_gap() {
        let target = egui::Rect::from_min_size(egui::pos2(120.0, 752.0), egui::vec2(40.0, 40.0));
        let marker = focus_underline_rect(target);

        assert_eq!(marker.size(), egui::vec2(18.0, 3.0));
        assert_eq!(marker.center().x, target.center().x);
        assert_eq!(marker.bottom(), target.bottom() - 2.0);
        assert_eq!(target.bottom() - marker.top(), 5.0);
        assert!(target.contains(marker.min));
        assert!(target.contains(marker.max));
    }

    #[test]
    fn taskbar_catalog_aliases_do_not_create_duplicate_workers_entries() {
        for query in [
            "Fleet & Mesh",
            "Workbench",
            "Mesh Map",
            "mesh-view",
            "Explorer",
        ] {
            assert_eq!(
                filtered_pin_catalog(query),
                Vec::<Surface>::new(),
                "{query:?} must not restore a tool-tray-owned center pin"
            );
        }
        for surface in [
            Surface::FleetMesh,
            Surface::Workbench,
            Surface::MeshView,
            Surface::Explorer,
        ] {
            assert_eq!(
                canonical_taskbar_surface(surface),
                None,
                "{surface:?} must not become a duplicate center-nav pin"
            );
        }
    }

    #[test]
    fn focused_taskbar_control_is_canonical_and_singular() {
        let controls = [
            Control {
                kind: ControlKind::SurfaceLauncher,
                rect: egui::Rect::NOTHING,
                surface: Some(Surface::FleetMesh),
                source_index: None,
            },
            Control {
                kind: ControlKind::SurfaceLauncher,
                rect: egui::Rect::NOTHING,
                surface: Some(Surface::FleetMesh),
                source_index: None,
            },
            Control {
                kind: ControlKind::SurfaceLauncher,
                rect: egui::Rect::NOTHING,
                surface: Some(Surface::Browser),
                source_index: None,
            },
        ];

        for alias in [
            Surface::FleetMesh,
            Surface::Workbench,
            Surface::MeshView,
            Surface::Explorer,
        ] {
            assert_eq!(
                focused_control_index(&controls, Some(alias)),
                None,
                "tool-tray-owned aliases must not focus a duplicate center control"
            );
        }
        assert_eq!(
            focused_control_index(&controls, Some(Surface::Browser)),
            Some(2)
        );
        assert_eq!(
            focused_control_index(&controls, Some(Surface::MapsLocation)),
            None
        );
    }

    #[test]
    fn docked_rail_drops_launcher_overflow_on_short_screens() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 400.0));
        let geometry = docked_geometry_for(screen, MAX_PINNED_SOURCES);

        assert_eq!(
            geometry
                .controls
                .iter()
                .take(3)
                .map(|control| control.kind)
                .collect::<Vec<_>>(),
            vec![
                ControlKind::Start,
                ControlKind::SurfaceLauncher,
                ControlKind::Back,
            ],
            "search, Workloads, and Back must remain first even when app launchers overflow"
        );
        assert!(
            geometry
                .controls
                .iter()
                .filter(|control| control.kind == ControlKind::SurfaceLauncher)
                .count()
                < dock_launcher_count(),
            "short vertical rails must omit lower-priority app launchers instead of painting below the rail"
        );
        assert_hit_targets_inside_backing("docked narrow screen".to_string(), &geometry);
    }

    #[test]
    fn overflow_preserves_catalog_order_after_visible_targets() {
        let surfaces = [Surface::Browser, Surface::Files, Surface::Terminal];
        let (visible_surfaces, visible_pins, overflow) = catalog_selection(&surfaces, 2, 2);
        assert_eq!(visible_surfaces, 1);
        assert_eq!(visible_pins, 0);
        assert_eq!(surfaces[0], Surface::Browser);
        let items = overflow.expect("overflow control").items;
        assert_eq!(
            items,
            vec![
                OverflowItem::Surface(Surface::Files),
                OverflowItem::Surface(Surface::Terminal),
                OverflowItem::PinnedDesktop(0),
                OverflowItem::PinnedDesktop(1),
            ]
        );
    }

    #[test]
    fn connected_remote_sessions_share_the_center_lane_and_overflow_cleanly() {
        let sessions = vec![
            SessionRailEntry::with_session_id("s1", "Oak desktop", "LIVE"),
            SessionRailEntry::with_session_id("s2", "Writer on Ash", "LIVE"),
            SessionRailEntry::with_session_id("s3", "Cedar desktop", "LIVE"),
            SessionRailEntry::with_session_id("s4", "Media on Birch", "LIVE"),
        ];
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(480.0, 480.0));
        let geometry = floating_geometry_for_catalog_with_sessions(screen, 0, &[], &sessions);
        let visible = geometry
            .controls
            .iter()
            .filter(|control| control.kind == ControlKind::RemoteSession)
            .count();
        let overflow = geometry.overflow.clone().expect("narrow taskbar overflow");

        assert!(visible < sessions.len());
        assert_eq!(
            overflow
                .items
                .iter()
                .filter(|item| matches!(item, OverflowItem::RemoteSession(_)))
                .count(),
            sessions.len() - visible
        );
        assert_hit_targets_inside_backing("connected remote sessions".to_owned(), &geometry);
    }

    #[test]
    fn connected_remote_session_controls_open_by_session_name_and_id() {
        let sessions = vec![
            SessionRailEntry::with_session_id("s1", "Oak desktop", "LIVE"),
            SessionRailEntry::with_session_id("s2", "Writer on Ash", "LIVE"),
        ];
        let control = Control {
            kind: ControlKind::RemoteSession,
            rect: egui::Rect::NOTHING,
            surface: None,
            source_index: Some(1),
        };

        assert_eq!(control_label(control, &[], &sessions), "Open Writer on Ash");
        assert_eq!(
            control_action(control, &[], &sessions),
            Action::RemoteSession("s2".to_owned())
        );
    }

    #[test]
    fn session_and_pinned_desktop_controls_have_disjoint_identity_and_hit_regions() {
        let sessions = [SessionRailEntry::with_session_id(
            "session-0",
            "Oak desktop",
            "LIVE",
        )];
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));

        for (mode, geometry) in [
            (
                DockMode::Floating,
                floating_geometry_for_catalog_with_sessions(screen, 1, &[], &sessions),
            ),
            (
                DockMode::Docked,
                docked_geometry_for_catalog_with_sessions(screen, 1, &[], &sessions),
            ),
        ] {
            let session = geometry
                .controls
                .iter()
                .find(|control| control.kind == ControlKind::RemoteSession)
                .copied()
                .expect("connected session target");
            let pinned = geometry
                .controls
                .iter()
                .find(|control| control.kind == ControlKind::PinnedDesktop)
                .copied()
                .expect("pinned desktop target");

            assert_eq!(session.source_index, pinned.source_index);
            assert_ne!(
                control_id(mode, session),
                control_id(mode, pinned),
                "independent projections must not alias egui state in {mode:?} placement"
            );
            assert!(
                !session.rect.intersects(pinned.rect),
                "session and pinned desktop hit regions must be disjoint in {mode:?} placement"
            );
        }
    }

    #[test]
    fn overflow_layout_keeps_rows_inside_the_screen_at_40px() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 240.0));
        let anchor = egui::Rect::from_min_size(egui::pos2(272.0, 192.0), egui::vec2(40.0, 40.0));
        let layout = overflow_layout_for(anchor, screen, 3);
        assert!(screen.contains(layout.outer.min));
        assert!(screen.contains(layout.outer.max));
        assert_eq!(layout.rows.len(), 3);
        assert!(layout.rows.iter().all(|row| {
            (row.width() - (OVERFLOW_W - 2.0 * Style::SP_S)).abs() < f32::EPSILON
                && (row.height() - CONTROL_EDGE).abs() < f32::EPSILON
                && layout.outer.contains(row.min)
                && layout.outer.contains(row.max)
        }));
    }

    #[test]
    fn first_boot_catalog_is_ordered_and_uses_operator_aliases() {
        assert_eq!(
            PIN_CATALOG[..9],
            [
                Surface::Workers,
                Surface::InfraCode,
                Surface::Desktop,
                Surface::Terminal,
                Surface::MapsLocation,
                Surface::Communications,
                Surface::Music,
                Surface::Media,
                Surface::Browser,
            ]
        );
        assert_eq!(taskbar_surface_label(Surface::Workers), "Workers");
        assert_eq!(taskbar_surface_label(Surface::FleetMesh), "Fleet & Mesh");
        assert_eq!(taskbar_surface_label(Surface::InfraCode), "Workloads");
        assert_eq!(filtered_pin_catalog("fleet & mesh"), Vec::<Surface>::new());
        assert_eq!(filtered_pin_catalog("workloads"), Vec::<Surface>::new());
        assert_eq!(filtered_pin_catalog("infra-code"), Vec::<Surface>::new());
    }

    #[test]
    fn pin_selector_query_is_utf8_safe_and_bounded_at_every_boundary() {
        let query = format!("{}🌐", "x".repeat(MAX_PIN_SELECTOR_QUERY_CHARS));
        let bounded = bounded_pin_selector_query(&query);
        assert_eq!(bounded.chars().count(), MAX_PIN_SELECTOR_QUERY_CHARS);
        assert!(bounded.is_char_boundary(bounded.len()));

        let mut state = State::new_profile(DockMode::Floating);
        state.pin_selector.query = query.clone();
        let rendered_query = bounded_pin_selector_query(&state.pin_selector.query);
        state.pin_selector.query = rendered_query.clone();
        assert_eq!(
            state.pin_selector.query,
            "x".repeat(MAX_PIN_SELECTOR_QUERY_CHARS)
        );
        assert_eq!(
            filtered_pin_catalog(&query),
            filtered_pin_catalog(&rendered_query)
        );
    }

    #[test]
    fn new_profile_selection_is_empty_and_commits_in_catalog_order() {
        let mut state = State::new_profile(DockMode::Floating);
        assert!(state.is_new_profile());
        assert!(state.pinned_surfaces().is_empty());
        assert!(state.pin_selector.selected.is_empty());

        state.toggle_pin_selector_surface(Surface::InfraCode);
        state.toggle_pin_selector_surface(Surface::Browser);
        assert_eq!(state.pin_selector.selected, vec![Surface::Browser]);

        state.complete_first_boot();
        assert!(!state.is_new_profile());
        assert_eq!(state.pinned_surfaces(), &[Surface::Browser]);
        assert!(state.pin_selector.selected.is_empty());
    }

    #[test]
    fn first_boot_keeps_existing_pins_when_selection_is_unchanged() {
        let mut state = State::from_prefs(NavBarPrefs {
            pinned_surfaces: vec!["browser".to_owned(), "maps-location".to_owned()],
            profile_state: ProfileState::New,
            ..NavBarPrefs::default()
        });

        assert!(state.is_new_profile());
        assert_eq!(
            state.pin_selector.selected,
            vec![Surface::Browser, Surface::MapsLocation]
        );

        state.complete_first_boot();

        assert!(!state.is_new_profile());
        assert_eq!(
            state.pinned_surfaces(),
            &[Surface::Browser, Surface::MapsLocation]
        );
    }

    #[test]
    fn first_boot_empty_selection_does_not_restore_defaults() {
        let mut state = State::new_profile(DockMode::Floating);
        state.complete_first_boot();

        assert!(!state.is_new_profile());
        assert!(state.pinned_surfaces().is_empty());
        assert_ne!(state.pinned_surfaces(), default_taskbar_pins().as_slice());
    }

    #[test]
    fn first_boot_commit_rejects_duplicate_and_unsupported_transient_entries() {
        let mut state = State::new_profile(DockMode::Floating);
        state.pin_selector.selected = vec![
            Surface::Browser,
            Surface::Workbench,
            Surface::Browser,
            Surface::MapsLocation,
            Surface::AutoHome,
        ];

        state.complete_first_boot();

        assert_eq!(
            state.pinned_surfaces(),
            &[Surface::Browser, Surface::MapsLocation]
        );
    }

    #[test]
    fn first_boot_selector_stays_inside_narrow_viewports() {
        for (screen, expected_size) in [
            (
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(240.0, 160.0)),
                egui::vec2(240.0, 160.0),
            ),
            (
                egui::Rect::from_min_size(egui::pos2(17.0, 11.0), egui::vec2(320.0, 180.0)),
                egui::vec2(280.0, 180.0),
            ),
        ] {
            let card = first_boot_selector_rect(screen);
            assert_eq!(card.size(), expected_size);
            assert!(card.left() >= screen.left());
            assert!(card.right() <= screen.right());
            assert!(card.top() >= screen.top());
            assert!(card.bottom() <= screen.bottom());
        }

        let desktop = first_boot_selector_rect(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 800.0),
        ));
        assert_eq!(
            desktop.size(),
            egui::vec2(FIRST_BOOT_SELECTOR_W, FIRST_BOOT_SELECTOR_H)
        );
    }

    #[test]
    fn migrated_preferences_preserve_pins_without_restoring_defaults() {
        let state = State::from_prefs(NavBarPrefs {
            schema_version: 1,
            mode: DockMode::Docked,
            pinned_surfaces: vec!["browser".to_owned(), "maps-location".to_owned()],
            profile_state: ProfileState::Configured,
            ..NavBarPrefs::default()
        });
        assert!(!state.is_new_profile());
        assert_eq!(
            state.pinned_surfaces(),
            &[Surface::Browser, Surface::MapsLocation]
        );
        assert_eq!(state.mode, DockMode::Docked);
        assert!(!state.pinned_surfaces().contains(&Surface::Desktop));

        let legacy_without_pins = State::from_prefs(NavBarPrefs {
            schema_version: 0,
            mode: DockMode::Floating,
            pinned_surfaces: Vec::new(),
            profile_state: ProfileState::Configured,
            ..NavBarPrefs::default()
        });
        assert!(legacy_without_pins.pinned_surfaces().is_empty());
        assert!(!legacy_without_pins
            .pinned_surfaces()
            .contains(&Surface::Desktop));
    }

    #[test]
    fn hit_targets_stay_inside_the_painted_navigation_chrome() {
        let screens = [
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0)),
            egui::Rect::from_min_size(egui::pos2(73.0, 41.0), egui::vec2(1280.0, 800.0)),
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0)),
        ];
        let pinned_counts = [0, 1, MAX_PINNED_SOURCES];
        let transition_offsets = [
            Duration::ZERO,
            Duration::from_millis(90),
            Duration::from_millis(180),
            Duration::from_millis(270),
        ];

        for screen in screens {
            for pinned_count in pinned_counts {
                assert_hit_targets_inside_backing(
                    format!("floating screen={screen:?} pinned={pinned_count}"),
                    &floating_geometry_for(screen, pinned_count),
                );
                assert_hit_targets_inside_backing(
                    format!("docked screen={screen:?} pinned={pinned_count}"),
                    &docked_geometry_for(screen, pinned_count),
                );

                let start = Instant::now();
                for (from, to) in [
                    (DockMode::Floating, DockMode::Docked),
                    (DockMode::Docked, DockMode::Floating),
                ] {
                    let state = State {
                        mode: to,
                        transition: Some(TransitionState {
                            from,
                            to,
                            started: start,
                            motion: MotionMode::Normal,
                        }),
                        pinned_surfaces: default_taskbar_pins(),
                        profile_state: ProfileState::Configured,
                        peer_app_favorites: FrontDoorPeerAppFavorites::empty(),
                        pin_selector: PinSelectorState::default(),
                    };
                    for offset in transition_offsets {
                        assert_hit_targets_inside_backing(
                            format!(
                                "{from:?}->{to:?} screen={screen:?} pinned={pinned_count} offset={offset:?}"
                            ),
                            &state.geometry_for(screen, start + offset, pinned_count),
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn smoothstep_has_stationary_endpoints() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(1.0), 1.0);
        assert!(smoothstep(0.5) > 0.4 && smoothstep(0.5) < 0.6);
    }

    #[test]
    fn preferences_round_trip_and_malformed_data_degrades() {
        let dir = tempfile_dir();
        let path = dir.join(CONFIG_FILE);
        save_to(
            &path,
            NavBarPrefs {
                mode: DockMode::Docked,
                ..NavBarPrefs::default()
            },
        )
        .expect("write nav-bar prefs");
        let loaded = read_bounded_config(&path).expect("read nav-bar prefs");
        let prefs: NavBarPrefs = serde_json::from_str(&loaded).expect("decode nav-bar prefs");
        assert_eq!(prefs.mode, DockMode::Docked);
        fs::write(&path, "not json").expect("write malformed nav-bar prefs");
        let fallback = read_bounded_config(&path)
            .ok()
            .and_then(|json| serde_json::from_str::<NavBarPrefs>(&json).ok())
            .map_or(DockMode::Floating, |prefs| prefs.mode);
        assert_eq!(fallback, DockMode::Floating);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn taskbar_surface_preferences_are_versioned_and_fail_closed() {
        let keys = vec![
            "browser".to_owned(),
            "browser".to_owned(),
            "not-a-surface".to_owned(),
            "maps-location".to_owned(),
            "this-node".to_owned(),
        ];
        assert_eq!(
            decode_pinned_surfaces(&keys),
            vec![Surface::Browser, Surface::MapsLocation]
        );

        let prefs = NavBarPrefs {
            schema_version: NAV_PREFS_SCHEMA_VERSION,
            mode: DockMode::Docked,
            pinned_surfaces: vec!["browser".to_owned(), "maps-location".to_owned()],
            profile_state: ProfileState::Configured,
            ..NavBarPrefs::default()
        };
        let json = serde_json::to_string(&prefs).expect("encode taskbar preferences");
        let decoded: NavBarPrefs = serde_json::from_str(&json).expect("decode taskbar preferences");
        assert_eq!(decoded.schema_version, NAV_PREFS_SCHEMA_VERSION);
        assert_eq!(
            decode_pinned_surfaces(&decoded.pinned_surfaces),
            vec![Surface::Browser, Surface::MapsLocation]
        );
    }

    #[test]
    fn future_preference_schema_cannot_restore_untrusted_placement_or_pins() {
        let state = State::from_prefs(NavBarPrefs {
            schema_version: NAV_PREFS_SCHEMA_VERSION + 1,
            mode: DockMode::Docked,
            pinned_surfaces: vec!["browser".to_owned(), "maps-location".to_owned()],
            profile_state: ProfileState::New,
            ..NavBarPrefs::default()
        });

        assert_eq!(state.mode, DockMode::Floating);
        assert!(state.pinned_surfaces().is_empty());
        assert!(!state.is_new_profile());
        assert!(state.peer_app_favorites().entries().is_empty());
    }

    #[test]
    fn migrated_alias_pins_canonicalize_without_reordering_or_restoring_defaults() {
        let state = State::from_prefs(NavBarPrefs {
            schema_version: 1,
            mode: DockMode::Floating,
            pinned_surfaces: vec![
                "workbench".to_owned(),
                "browser".to_owned(),
                "mesh-view".to_owned(),
                "system".to_owned(),
                "storage".to_owned(),
            ],
            profile_state: ProfileState::Configured,
            ..NavBarPrefs::default()
        });

        // Legacy Fleet & Mesh and This Node aliases collapse to one canonical
        // entry each, while the user's order and intentional profile state
        // remain authoritative.
        assert_eq!(state.pinned_surfaces(), &[Surface::Browser]);
        assert!(!state.is_new_profile());
        assert_ne!(state.pinned_surfaces(), default_taskbar_pins().as_slice());
    }

    #[test]
    fn taskbar_pin_actions_are_bounded_ordered_and_reject_non_catalog_surfaces() {
        let mut state = State::new_profile(DockMode::Floating);
        let original = state.pinned_surfaces().to_vec();
        assert!(!state.pin_surface(Surface::FleetMesh));
        assert!(!state.pin_surface(Surface::Workbench));
        assert!(!state.pin_surface(Surface::AutoHome));
        assert_eq!(state.pinned_surfaces(), original.as_slice());
        assert!(!state.unpin_surface(Surface::Workbench));
        assert!(!state.unpin_surface(Surface::AutoHome));
    }

    #[test]
    fn oversized_preferences_degrade_to_floating_without_json_materialization() {
        let dir = tempfile_dir();
        let path = dir.join(CONFIG_FILE);
        fs::write(&path, vec![b'x'; MAX_NAV_PREFS_BYTES + 1]).expect("oversized prefs");

        assert!(read_bounded_config(&path).is_err());
        let fallback = read_bounded_config(&path)
            .ok()
            .and_then(|json| serde_json::from_str::<NavBarPrefs>(&json).ok())
            .map_or(DockMode::Floating, |prefs| prefs.mode);
        assert_eq!(fallback, DockMode::Floating);
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn final_symlink_preferences_are_rejected() {
        use std::os::unix::fs::symlink;

        let dir = tempfile_dir();
        let target = dir.join("outside.json");
        let link = dir.join(CONFIG_FILE);
        fs::write(&target, r#"{"mode":"docked"}"#).expect("target prefs");
        symlink(&target, &link).expect("preference symlink");

        assert!(read_bounded_config(&link).is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn save_replaces_final_symlink_without_writing_through_it() {
        use std::os::unix::fs::symlink;

        let dir = tempfile_dir();
        let target = dir.join("outside.json");
        let link = dir.join(CONFIG_FILE);
        fs::write(&target, r#"{"mode":"floating"}"#).expect("target prefs");
        symlink(&target, &link).expect("preference symlink");

        save_to(
            &link,
            NavBarPrefs {
                mode: DockMode::Docked,
                ..NavBarPrefs::default()
            },
        )
        .expect("replace preference symlink atomically");

        assert_eq!(
            fs::read_to_string(&target).expect("read symlink target"),
            r#"{"mode":"floating"}"#
        );
        let saved: NavBarPrefs =
            serde_json::from_str(&fs::read_to_string(&link).expect("read replaced preference"))
                .expect("decode replaced preference");
        assert_eq!(saved.mode, DockMode::Docked);
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn save_rejects_symlinked_parent_without_writing_outside() {
        use std::os::unix::fs::symlink;

        let dir = tempfile_dir();
        let outside = dir.join("outside");
        let parent = dir.join("prefs");
        fs::create_dir(&outside).expect("outside directory");
        symlink(&outside, &parent).expect("preference parent symlink");

        let result = save_to(
            &parent.join(CONFIG_FILE),
            NavBarPrefs {
                mode: DockMode::Docked,
                ..NavBarPrefs::default()
            },
        );

        assert!(
            result.is_err(),
            "symlinked preference parent must be rejected"
        );
        assert!(!outside.join(CONFIG_FILE).exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn headless_mount_paints_and_exposes_all_controls_in_both_modes() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };

        for mode in [DockMode::Floating, DockMode::Docked] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let mut state = State {
                mode,
                transition: None,
                pinned_surfaces: default_taskbar_pins(),
                profile_state: ProfileState::Configured,
                peer_app_favorites: FrontDoorPeerAppFavorites::empty(),
                pin_selector: PinSelectorState::default(),
            };

            // The production mount waits for egui's initial pass, and Area adds
            // one invisible sizing pass, before its steady-state paint. Drive
            // those passes explicitly; no native window, display, or GPU is needed.
            let _ = ctx.run(input.clone(), |ctx| {
                assert_eq!(state.mount(ctx, &[]), None);
            });
            let _ = ctx.run(input.clone(), |ctx| {
                assert_eq!(state.mount(ctx, &[]), None);
            });
            let output = ctx.run(input.clone(), |ctx| {
                assert_eq!(state.mount(ctx, &[]), None);
            });

            let primitives = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
            assert!(
                !primitives.is_empty(),
                "{mode:?} navigation bar produced no visible paint"
            );
            let area_response = ctx
                .read_response(egui::Id::new("construct-navigation-bar").with("move"))
                .expect("navigation Area should expose its bounded hit rectangle");
            let expected_outer = match mode {
                DockMode::Floating => floating_geometry(screen).outer,
                DockMode::Docked => docked_geometry(screen).outer,
            };
            let rect_diffs = (
                area_response.rect.min.x - expected_outer.min.x,
                area_response.rect.min.y - expected_outer.min.y,
                area_response.rect.max.x - expected_outer.max.x,
                area_response.rect.max.y - expected_outer.max.y,
            );
            // egui snaps an Area to its pixel grid; the resulting sub-pixel
            // edge adjustment is not a change to the taskbar footprint.
            const EDGE_TOLERANCE: f32 = 0.05;
            assert!(
                rect_diffs.0.abs() <= EDGE_TOLERANCE
                    && rect_diffs.1.abs() <= EDGE_TOLERANCE
                    && rect_diffs.2.abs() <= EDGE_TOLERANCE
                    && rect_diffs.3.abs() <= EDGE_TOLERANCE,
                "the navigation Area must shield only the bar, not the home/workspace: mode={mode:?} actual={:?} expected={:?} diffs={:?}",
                area_response.rect,
                expected_outer,
                rect_diffs,
            );

            let update = output
                .platform_output
                .accesskit_update
                .as_ref()
                .expect("headless navigation bar should publish an AccessKit tree");
            for (kind, expected_label) in [
                (ControlKind::Start, "Start"),
                (ControlKind::Search, "Search"),
                (ControlKind::Back, "Back"),
                (ControlKind::Home, "Home"),
                (ControlKind::Pin, "Taskbar placement menu"),
            ] {
                let node = update
                    .nodes
                    .iter()
                    .find_map(|(_, node)| (node.label() == Some(expected_label)).then_some(node))
                    .unwrap_or_else(|| {
                        panic!(
                            "{mode:?} {:?} control missing from AccessKit tree: {expected_label:?}",
                            kind
                        )
                    });
                assert_eq!(node.role(), egui::accesskit::Role::Button);
                assert!(node.supports_action(egui::accesskit::Action::Click));
                let bounds = node
                    .bounds()
                    .expect("navigation control AccessKit node should have bounds");
                assert!(
                    bounds.x0 < bounds.x1 && bounds.y0 < bounds.y1,
                    "{mode:?} {:?} control has empty AccessKit bounds: {bounds:?}",
                    kind
                );
            }
        }
    }

    #[test]
    fn bottom_taskbar_composes_the_clock_tray_on_its_foreground_layer() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut state = State {
            mode: DockMode::Floating,
            transition: None,
            pinned_surfaces: default_taskbar_pins(),
            profile_state: ProfileState::Configured,
            peer_app_favorites: FrontDoorPeerAppFavorites::empty(),
            pin_selector: PinSelectorState::default(),
        };
        let mut construct = ConstructChrome::default();
        let segments = StatusSegments::default();
        let env = StatusBarEnv {
            curtain_engaged: false,
            car: false,
            immersive_app: false,
        };

        let mut output = egui::FullOutput::default();
        for _ in 0..3 {
            output = ctx.run(input.clone(), |ctx| {
                assert_eq!(
                    state.mount_with_active_and_bottom_tray(
                        ctx,
                        &[],
                        None,
                        &mut construct,
                        &segments,
                        1.0,
                        env,
                    ),
                    None
                );
            });
        }

        assert!(
            !ctx.tessellate(output.shapes, output.pixels_per_point)
                .is_empty(),
            "the unified taskbar must paint in a headless frame"
        );
        let taskbar = ctx
            .read_response(egui::Id::new("construct-navigation-bar").with("move"))
            .expect("taskbar Area response");
        let clock = ctx
            .read_response(egui::Id::new(("construct-bottom-system-tray", "clock")))
            .expect("clock remains reachable within the taskbar");
        let bell = ctx
            .read_response(status_bar::notification_bell_id("bottom"))
            .expect("notification bell remains reachable within the taskbar");
        let health = ctx
            .read_response(egui::Id::new(("system-mesh-health", "bottom")))
            .expect("mesh health remains reachable within the taskbar");
        assert_eq!(clock.layer_id, taskbar.layer_id);
        assert_eq!(bell.layer_id, taskbar.layer_id);
        assert_eq!(health.layer_id, taskbar.layer_id);
        assert!(
            clock.rect.intersect(bell.rect).width() <= f32::EPSILON,
            "clock and bell targets must remain disjoint"
        );
        let placement = floating_geometry(screen)
            .controls
            .into_iter()
            .find(|control| control.kind == ControlKind::Pin)
            .expect("bottom placement target");
        assert!(
            placement.rect.intersect(clock.rect).width() <= f32::EPSILON
                && placement.rect.intersect(bell.rect).width() <= f32::EPSILON,
            "placement, clock, and bell targets must remain disjoint"
        );
        assert!(
            bottom_tray_rect(screen).intersects(taskbar.rect),
            "the tray footprint must be part of the taskbar footprint"
        );
    }

    #[test]
    fn click_outside_bar_reaches_workspace_layer() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let workspace_click = egui::pos2(500.0, 400.0);
        let mut state = State::with_mode(DockMode::Floating);
        let ctx = egui::Context::default();
        let mut workspace_clicked = false;

        let mut frame = |ctx: &egui::Context, input: egui::RawInput, state: &mut State| {
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui.interact(
                        ui.max_rect(),
                        egui::Id::new("workspace-click-target"),
                        egui::Sense::click(),
                    );
                    if response.clicked() {
                        workspace_clicked = true;
                    }
                });
                assert_eq!(state.mount(ctx, &[]), None);
            });
        };

        let base = || egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        frame(&ctx, base(), &mut state);
        frame(&ctx, base(), &mut state);
        frame(&ctx, base(), &mut state);

        let mut press = base();
        press.events = vec![
            egui::Event::PointerMoved(workspace_click),
            egui::Event::PointerButton {
                pos: workspace_click,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        frame(&ctx, press, &mut state);

        let mut release = base();
        release.events = vec![
            egui::Event::PointerMoved(workspace_click),
            egui::Event::PointerButton {
                pos: workspace_click,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        frame(&ctx, release, &mut state);

        assert!(
            workspace_clicked,
            "the foreground navigation Area must not consume clicks outside its visible bar"
        );
    }

    #[test]
    fn floating_home_control_emits_action_after_a_real_pointer_click() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let mut state = State::with_mode(DockMode::Floating);
        let ctx = egui::Context::default();
        let home = floating_geometry(screen)
            .controls
            .iter()
            .find(|control| control.kind == ControlKind::Home)
            .expect("floating Home control")
            .rect
            .center();

        let base = || egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        for _ in 0..3 {
            let _ = ctx.run(base(), |ctx| {
                assert_eq!(state.mount(ctx, &[]), None);
            });
        }
        let mut press = base();
        press.events = vec![
            egui::Event::PointerMoved(home),
            egui::Event::PointerButton {
                pos: home,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        let _ = ctx.run(press, |ctx| {
            assert_eq!(state.mount(ctx, &[]), None);
        });

        let mut release = base();
        release.events = vec![
            egui::Event::PointerMoved(home),
            egui::Event::PointerButton {
                pos: home,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        let mut action = None;
        let _ = ctx.run(release, |ctx| action = state.mount(ctx, &[]));
        assert_eq!(action, Some(Action::Home));
    }

    #[test]
    fn floating_pin_control_emits_action_after_drm_touch_click() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0));
        let mut state = State::with_mode(DockMode::Floating);
        let ctx = egui::Context::default();
        let pin = floating_geometry(screen)
            .controls
            .iter()
            .find(|control| control.kind == ControlKind::Pin)
            .expect("floating taskbar placement control")
            .rect
            .center();

        let input = |events| egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        for _ in 0..3 {
            let _ = ctx.run(input(Vec::new()), |ctx| {
                assert_eq!(state.mount(ctx, &[]), None);
            });
        }

        let _ = ctx.run(
            input(vec![
                egui::Event::Touch {
                    device_id: egui::TouchDeviceId(0),
                    id: egui::TouchId(0),
                    phase: egui::TouchPhase::Start,
                    pos: pin,
                    force: None,
                },
                egui::Event::PointerMoved(pin),
                egui::Event::PointerButton {
                    pos: pin,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ctx| {
                assert_eq!(state.mount(ctx, &[]), None);
            },
        );
        let mut action = None;
        let _ = ctx.run(
            input(vec![
                egui::Event::Touch {
                    device_id: egui::TouchDeviceId(0),
                    id: egui::TouchId(0),
                    phase: egui::TouchPhase::End,
                    pos: pin,
                    force: None,
                },
                egui::Event::PointerButton {
                    pos: pin,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerGone,
            ]),
            |ctx| action = state.mount(ctx, &[]),
        );
        assert_eq!(action, Some(Action::ToggleDock));
    }

    #[test]
    fn floating_pin_recovers_from_stale_foreground_overlay_order() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0));
        let mut state = State::with_mode(DockMode::Floating);
        let ctx = egui::Context::default();
        let pin = floating_geometry(screen)
            .controls
            .iter()
            .find(|control| control.kind == ControlKind::Pin)
            .expect("floating taskbar placement control")
            .rect
            .center();

        let input = |events| egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        for _ in 0..3 {
            let _ = ctx.run(input(Vec::new()), |ctx| {
                assert_eq!(state.mount(ctx, &[]), None);
            });
        }

        let overlay_layer = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("stale-fullscreen-foreground-overlay"),
        );
        let _ = ctx.run(input(Vec::new()), |ctx| {
            assert_eq!(state.mount(ctx, &[]), None);
            let overlay = egui::Area::new(overlay_layer.id)
                .order(overlay_layer.order)
                .fixed_pos(screen.min)
                .show(ctx, |ui| {
                    ui.allocate_exact_size(screen.size(), egui::Sense::click());
                });
            ctx.move_to_top(overlay.response.layer_id);
        });
        assert_eq!(
            ctx.layer_id_at(pin),
            Some(overlay_layer),
            "fixture must place the stale overlay above the navigation Area before recovery"
        );

        let nav_layer = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("construct-navigation-bar"),
        );
        let _ = ctx.run(input(Vec::new()), |ctx| {
            assert_eq!(state.mount(ctx, &[]), None);
        });
        assert_eq!(
            ctx.layer_id_at(pin),
            Some(nav_layer),
            "mounting the navigation bar must re-raise its bounded Area above stale overlays"
        );

        let _ = ctx.run(
            input(vec![
                egui::Event::Touch {
                    device_id: egui::TouchDeviceId(0),
                    id: egui::TouchId(0),
                    phase: egui::TouchPhase::Start,
                    pos: pin,
                    force: None,
                },
                egui::Event::PointerMoved(pin),
                egui::Event::PointerButton {
                    pos: pin,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ctx| {
                assert_eq!(state.mount(ctx, &[]), None);
            },
        );
        let mut action = None;
        let _ = ctx.run(
            input(vec![
                egui::Event::Touch {
                    device_id: egui::TouchDeviceId(0),
                    id: egui::TouchId(0),
                    phase: egui::TouchPhase::End,
                    pos: pin,
                    force: None,
                },
                egui::Event::PointerButton {
                    pos: pin,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerGone,
            ]),
            |ctx| action = state.mount(ctx, &[]),
        );
        assert_eq!(action, Some(Action::ToggleDock));
    }

    #[test]
    fn non_zero_screen_pointer_hits_each_base_control() {
        // An egui Area's content UI, painter, and widget rectangles all use
        // screen coordinates. Keep the viewport origin non-zero so a local-
        // coordinate regression cannot pass by accident.
        let screen = egui::Rect::from_min_size(egui::pos2(73.0, 41.0), egui::vec2(1280.0, 800.0));

        for mode in [DockMode::Floating, DockMode::Docked] {
            let ctx = egui::Context::default();
            let mut state = State::with_mode(mode);
            let geometry = match mode {
                DockMode::Floating => floating_geometry(screen),
                DockMode::Docked => docked_geometry(screen),
            };

            let input = |events| egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            for _ in 0..3 {
                let _ = ctx.run(input(Vec::new()), |ctx| {
                    assert_eq!(state.mount(ctx, &[]), None);
                });
            }

            for (kind, expected) in [
                (ControlKind::Start, Action::OpenFrontDoor),
                (ControlKind::Search, Action::FocusSearch),
                (ControlKind::Back, Action::Back),
                (ControlKind::Home, Action::Home),
                (ControlKind::Pin, Action::ToggleDock),
            ] {
                let control = geometry
                    .controls
                    .iter()
                    .find(|control| control.kind == kind)
                    .copied()
                    .expect("base control must be present in both dock modes");
                let target = control.rect.center();
                let _ = ctx.run(
                    input(vec![
                        egui::Event::PointerMoved(target),
                        egui::Event::PointerButton {
                            pos: target,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]),
                    |ctx| {
                        assert_eq!(state.mount(ctx, &[]), None);
                    },
                );
                let mut action = None;
                let _ = ctx.run(
                    input(vec![
                        egui::Event::PointerMoved(target),
                        egui::Event::PointerButton {
                            pos: target,
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]),
                    |ctx| action = state.mount(ctx, &[]),
                );
                assert_eq!(action, Some(expected), "{mode:?} {kind:?} target");
            }
        }
    }

    #[test]
    fn non_zero_screen_pointer_hits_each_grouped_app_control() {
        let screen = egui::Rect::from_min_size(egui::pos2(73.0, 41.0), egui::vec2(1280.0, 800.0));

        for mode in [DockMode::Floating, DockMode::Docked] {
            let ctx = egui::Context::default();
            let mut state = State::with_mode(mode);
            let geometry = match mode {
                DockMode::Floating => floating_geometry(screen),
                DockMode::Docked => docked_geometry(screen),
            };
            let input = |events| egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            for _ in 0..3 {
                let _ = ctx.run(input(Vec::new()), |ctx| {
                    assert_eq!(state.mount(ctx, &[]), None);
                });
            }

            for surface in [
                Surface::InfraCode,
                Surface::MapsLocation,
                Surface::Communications,
                Surface::Browser,
            ] {
                let control = geometry
                    .controls
                    .iter()
                    .find(|control| control.surface == Some(surface))
                    .copied()
                    .unwrap_or_else(|| panic!("{mode:?} missing grouped app {surface:?}"));
                let target = control.rect.center();
                let _ = ctx.run(
                    input(vec![
                        egui::Event::PointerMoved(target),
                        egui::Event::PointerButton {
                            pos: target,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]),
                    |ctx| {
                        assert_eq!(state.mount(ctx, &[]), None);
                    },
                );
                let mut action = None;
                let _ = ctx.run(
                    input(vec![
                        egui::Event::PointerMoved(target),
                        egui::Event::PointerButton {
                            pos: target,
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]),
                    |ctx| action = state.mount(ctx, &[]),
                );
                assert_eq!(
                    action,
                    Some(Action::OpenSurface(surface)),
                    "{mode:?} {surface:?} target"
                );
            }
        }
    }

    #[test]
    fn stale_docked_hit_targets_do_not_fire_after_switching_to_floating() {
        // egui resolves a click from the widget IDs registered in the previous
        // frame, then reports `clicked()` on the current-frame widget with the
        // same ID. The Springboard Dock reuses Back/Home/Pin semantics in both
        // placements, so the IDs must include the placement: otherwise the old
        // top-left rail buttons can complete a click on the new bottom taskbar.
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let stale_home = docked_geometry(screen)
            .controls
            .iter()
            .find(|control| control.kind == ControlKind::Home)
            .expect("docked Home control")
            .rect
            .center();
        let floating = floating_geometry(screen);
        let floating_home_control = floating
            .controls
            .iter()
            .find(|control| control.kind == ControlKind::Home)
            .expect("floating Home control");
        let floating_home = floating_home_control.rect.center();
        assert!(
            !floating_home_control.rect.contains(stale_home),
            "the stale top-left Home target must be outside the floating taskbar"
        );

        let ctx = egui::Context::default();
        let mut state = State::with_mode(DockMode::Docked);
        let input = |events| egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };

        for _ in 0..3 {
            let _ = ctx.run(input(Vec::new()), |ctx| {
                assert_eq!(state.mount(ctx, &[]), None);
            });
        }

        state.toggle_mode(Instant::now(), MotionMode::Disabled);
        assert_eq!(state.mode, DockMode::Floating);

        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(stale_home),
                egui::Event::PointerButton {
                    pos: stale_home,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ctx| {
                assert_eq!(state.mount(ctx, &[]), None);
            },
        );
        let mut action = None;
        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(stale_home),
                egui::Event::PointerButton {
                    pos: stale_home,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ctx| action = state.mount(ctx, &[]),
        );
        assert_eq!(
            action, None,
            "a stale top-left rail hit box must not activate the bottom taskbar"
        );

        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(floating_home),
                egui::Event::PointerButton {
                    pos: floating_home,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ctx| {
                assert_eq!(state.mount(ctx, &[]), None);
            },
        );
        let mut action = None;
        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(floating_home),
                egui::Event::PointerButton {
                    pos: floating_home,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ctx| action = state.mount(ctx, &[]),
        );
        assert_eq!(
            action,
            Some(Action::Home),
            "the actual bottom taskbar Home target must remain clickable"
        );
    }

    #[test]
    fn chooser_pins_extend_the_reserved_springboard_dock_and_emit_source_action() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let sources = vec![crate::surfaces::DesktopRailSource::new(
            "peer:oak",
            "Oak Desktop",
            "oak",
            "RDP",
            true,
            true,
            false,
        )];
        let geometry = floating_geometry_for(screen, sources.len());
        assert_eq!(geometry.controls.len(), dock_control_capacity(1));
        let placement = geometry
            .controls
            .iter()
            .find(|control| control.kind == ControlKind::Pin)
            .copied()
            .expect("taskbar placement control");
        let pinned = geometry
            .controls
            .iter()
            .find(|control| control.kind == ControlKind::PinnedDesktop)
            .copied()
            .expect("chooser pin should append a dock target");
        assert_eq!(pinned.kind, ControlKind::PinnedDesktop);
        assert_eq!(pinned.source_index, Some(0));
        let tray = bottom_tray_rect(screen);
        let clock = egui::Rect::from_min_max(
            egui::pos2(
                tray.right() - 85.8 - CONTROL_EDGE - BOTTOM_TRAY_GAP,
                tray.top(),
            ),
            egui::pos2(tray.right() - CONTROL_EDGE - BOTTOM_TRAY_GAP, tray.bottom()),
        );
        assert!(
            placement.rect.left() >= clock.right() + BOTTOM_TRAY_GAP,
            "Show Desktop placement control must sit to the right of the bottom clock"
        );
        assert_eq!(geometry.outer.width(), screen.width());

        let ctx = egui::Context::default();
        let mut state = State::with_mode(DockMode::Floating);
        let target = pinned.rect.center();
        let input = |events| egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        for _ in 0..3 {
            let _ = ctx.run(input(Vec::new()), |ctx| {
                assert_eq!(state.mount(ctx, &sources), None);
            });
        }
        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(target),
                egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ctx| {
                let _ = state.mount(ctx, &sources);
            },
        );
        let mut action = None;
        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(target),
                egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ctx| action = state.mount(ctx, &sources),
        );
        assert_eq!(action, Some(Action::DesktopSource("peer:oak".to_owned())));
    }

    #[test]
    fn chooser_pin_click_keeps_the_foreground_layer_during_non_zero_transition() {
        let screen = egui::Rect::from_min_size(egui::pos2(73.0, 41.0), egui::vec2(1280.0, 800.0));
        let sources = vec![crate::surfaces::DesktopRailSource::new(
            "peer:cedar",
            "Cedar Desktop",
            "cedar",
            "RDP",
            true,
            true,
            false,
        )];
        let transition_started = Instant::now();
        let mut state = State {
            mode: DockMode::Docked,
            transition: Some(TransitionState {
                from: DockMode::Floating,
                to: DockMode::Docked,
                started: transition_started,
                motion: MotionMode::Normal,
            }),
            pinned_surfaces: default_taskbar_pins(),
            profile_state: ProfileState::Configured,
            peer_app_favorites: FrontDoorPeerAppFavorites::empty(),
            pin_selector: PinSelectorState::default(),
        };
        let ctx = egui::Context::default();
        let input = |events| egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };

        // Warm the Area so the pointer hit-test sees the same foreground layer
        // that the current frame will populate. The state remains in its
        // transition window because this loop is intentionally immediate.
        for _ in 0..3 {
            let _ = ctx.run(input(Vec::new()), |ctx| {
                assert_eq!(state.mount(ctx, &sources), None);
            });
        }
        let geometry = state.geometry_for(screen, Instant::now(), sources.len());
        let target = geometry
            .controls
            .iter()
            .find(|control| control.source_index == Some(0))
            .expect("transition geometry must retain the chooser pin")
            .rect
            .center();
        let layer = egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("construct-navigation-bar"),
        );
        assert_eq!(
            ctx.layer_id_at(target),
            Some(layer),
            "the navigation Area must remain in the foreground layer hit-test"
        );

        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(target),
                egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ctx| {
                let _ = state.mount(ctx, &sources);
            },
        );
        let mut action = None;
        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(target),
                egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ctx| action = state.mount(ctx, &sources),
        );
        assert_eq!(
            action,
            Some(Action::DesktopSource("peer:cedar".to_owned())),
            "a chooser-pinned target must emit its action during the transition"
        );
    }

    #[test]
    fn headless_geometry_proves_black_taskbar_and_slide_then_melt_rail() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let floating = floating_geometry(screen);
        let docked = docked_geometry(screen);

        assert_eq!(
            floating.outer,
            egui::Rect::from_min_max(egui::pos2(0.0, 752.0), egui::pos2(1280.0, 800.0)),
            "bottom navigation must be a full-width taskbar"
        );
        assert_eq!(floating.radius, egui::CornerRadius::ZERO);
        assert_eq!(
            floating
                .controls
                .iter()
                .take(5)
                .map(|control| control.kind)
                .collect::<Vec<_>>(),
            vec![
                ControlKind::Start,
                ControlKind::Search,
                ControlKind::SurfaceLauncher,
                ControlKind::Back,
                ControlKind::Home,
            ],
            "the taskbar must lead with search-first navigation controls"
        );
        assert_eq!(
            floating
                .controls
                .iter()
                .take(5)
                .map(|control| control_icon(*control))
                .collect::<Vec<_>>(),
            vec![
                IconId::Grid,
                IconId::Search,
                IconId::Server,
                IconId::ArrowLeft,
                IconId::FileHome,
            ]
        );
        assert_eq!(
            floating
                .controls
                .iter()
                .filter_map(|control| control.surface)
                .collect::<Vec<_>>(),
            vec![
                Surface::InfraCode,
                Surface::MapsLocation,
                Surface::Communications,
                Surface::Browser,
            ],
            "the taskbar must use the searchable pin catalog order"
        );
        assert!(
            floating
                .controls
                .iter()
                .all(|control| control.rect.center().y == floating.controls[0].rect.center().y),
            "the taskbar controls must share one horizontal row"
        );

        // Exercise the actual backing painter through egui's CPU tessellator so
        // this remains a headless render proof rather than only a rect assertion.
        let ctx = egui::Context::default();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("nav-bar-regression"),
                ));
                paint_backing(&painter, &floating);
            },
        );
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        assert!(
            primitives
                .iter()
                .any(|primitive| match &primitive.primitive {
                    egui::epaint::Primitive::Mesh(mesh) => {
                        mesh.vertices
                            .iter()
                            .any(|vertex| vertex.color == egui::Color32::BLACK)
                    }
                    egui::epaint::Primitive::Callback(_) => false,
                }),
            "the taskbar backing must paint an opaque black mesh"
        );

        let start = Instant::now();
        let mut state = State::default();
        state.toggle_mode(start, MotionMode::Normal);

        let at_start = state.geometry(screen, start);
        assert_eq!(at_start.outer, floating.outer);
        assert_eq!(at_start.radius, floating.radius);

        // The first phase is a horizontal slide into the left edge: size,
        // vertical position, and corner radius are unchanged before the melt.
        let during_slide = state.geometry(screen, start + Duration::from_millis(90));
        assert_eq!(during_slide.outer.left(), floating.outer.left());
        assert_eq!(during_slide.outer.width(), floating.outer.width());
        assert_eq!(during_slide.outer.top(), floating.outer.top());
        assert_eq!(during_slide.radius, floating.radius);

        // The second phase starts from that left-edge staging position and
        // melts into the narrower, taller rail with changing corner radii.
        let during_melt = state.geometry(screen, start + Duration::from_millis(180));
        assert_eq!(during_melt.outer.left(), screen.left());
        assert!(during_melt.outer.width() < floating.outer.width());
        assert!(during_melt.outer.top() < during_slide.outer.top());
        assert!(during_melt.outer.height() > floating.outer.height());
        assert!(during_melt.radius.nw <= floating.radius.nw);
        assert!(during_melt.controls[0].rect.top() < during_slide.controls[0].rect.top());

        let settled = state.geometry(screen, start + TRANSITION);
        assert_eq!(settled.outer, docked.outer);
        assert_eq!(
            settled
                .controls
                .iter()
                .take(5)
                .map(|control| control.kind)
                .collect::<Vec<_>>(),
            vec![
                ControlKind::Start,
                ControlKind::Search,
                ControlKind::SurfaceLauncher,
                ControlKind::Back,
                ControlKind::Home,
            ]
        );
        assert!(settled.controls[0].rect.top() < settled.controls[1].rect.top());
        assert!(settled.controls[1].rect.top() < settled.controls[2].rect.top());
        assert_eq!(settled.controls[4].kind, ControlKind::Home);
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mde-nav-bar-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create nav-bar test directory");
        dir
    }

    fn assert_hit_targets_inside_backing(label: String, geometry: &Geometry) {
        assert!(
            geometry.outer.is_positive(),
            "{label}: navigation backing must have a positive visual footprint"
        );
        for control in &geometry.controls {
            for point in hit_rect_probe_points(control.rect) {
                assert!(
                    point_inside_rounded_rect(point, geometry.outer, geometry.radius),
                    "{label}: {:?} hit target point {point:?} escapes painted backing {:?} radius {:?} rect {:?}",
                    control.kind,
                    geometry.outer,
                    geometry.radius,
                    control.rect
                );
            }
        }
    }

    fn hit_rect_probe_points(rect: egui::Rect) -> [egui::Pos2; 9] {
        [
            rect.left_top(),
            egui::pos2(rect.center().x, rect.top()),
            rect.right_top(),
            egui::pos2(rect.left(), rect.center().y),
            rect.center(),
            egui::pos2(rect.right(), rect.center().y),
            rect.left_bottom(),
            egui::pos2(rect.center().x, rect.bottom()),
            rect.right_bottom(),
        ]
    }

    fn point_inside_rounded_rect(
        point: egui::Pos2,
        rect: egui::Rect,
        radius: egui::CornerRadius,
    ) -> bool {
        const EPSILON: f32 = 0.01;
        if point.x < rect.left() - EPSILON
            || point.x > rect.right() + EPSILON
            || point.y < rect.top() - EPSILON
            || point.y > rect.bottom() + EPSILON
        {
            return false;
        }

        let in_corner = |radius: u8, corner_center: egui::Pos2| {
            let radius = f32::from(radius);
            if radius <= 0.0 {
                true
            } else {
                (point - corner_center).length_sq() <= (radius + EPSILON).powi(2)
            }
        };

        if point.x < rect.left() + f32::from(radius.nw)
            && point.y < rect.top() + f32::from(radius.nw)
        {
            return in_corner(
                radius.nw,
                egui::pos2(
                    rect.left() + f32::from(radius.nw),
                    rect.top() + f32::from(radius.nw),
                ),
            );
        }
        if point.x > rect.right() - f32::from(radius.ne)
            && point.y < rect.top() + f32::from(radius.ne)
        {
            return in_corner(
                radius.ne,
                egui::pos2(
                    rect.right() - f32::from(radius.ne),
                    rect.top() + f32::from(radius.ne),
                ),
            );
        }
        if point.x < rect.left() + f32::from(radius.sw)
            && point.y > rect.bottom() - f32::from(radius.sw)
        {
            return in_corner(
                radius.sw,
                egui::pos2(
                    rect.left() + f32::from(radius.sw),
                    rect.bottom() - f32::from(radius.sw),
                ),
            );
        }
        if point.x > rect.right() - f32::from(radius.se)
            && point.y > rect.bottom() - f32::from(radius.se)
        {
            return in_corner(
                radius.se,
                egui::pos2(
                    rect.right() - f32::from(radius.se),
                    rect.bottom() - f32::from(radius.se),
                ),
            );
        }

        true
    }
}
