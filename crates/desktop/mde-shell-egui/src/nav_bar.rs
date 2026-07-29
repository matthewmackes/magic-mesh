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
use mde_egui::{Elevation, MotionMode, Style, TypographyRole};
use mde_theme::brand::icons::IconId;
use serde::{Deserialize, Serialize};

use crate::status_bar::STATUS_BAR_H;
use crate::surfaces::{
    dock_launcher_group_label, dock_launcher_surface_label, icon_texture, Surface,
    DOCK_LAUNCHER_GROUPS,
};

/// The reserved left-rail width in docked mode.
pub(crate) const DOCKED_W: f32 = 56.0;
/// The full-width Construct taskbar height in Bottom mode.
pub(crate) const TASKBAR_H: f32 = 48.0;
/// Bottom space reserved by the horizontal taskbar in normal workspace layout.
pub(crate) const SPRINGBOARD_DOCK_RESERVED_H: f32 = TASKBAR_H;
/// The icon controls keep a compact 40px target in the thin dock.
const CONTROL_EDGE: f32 = 40.0;
/// The horizontal taskbar keeps fixed-size targets and drops lower-priority
/// chooser pins and launchers when a panel cannot fit the whole catalog.
const FLOATING_GAP: f32 = 4.0;
/// Maximum number of chooser-pinned sources shown in the dock. The full
/// chooser remains the unbounded discovery surface; the dock is a quick rail.
const MAX_PINNED_SOURCES: usize = 8;
/// The transition first slides left, then melts into the vertical rail.
const SLIDE_FRACTION: f32 = 0.34;
/// Total transition length: short enough to feel direct, long enough to read.
const TRANSITION: Duration = Duration::from_millis(360);
/// Persisted per-seat preference.
const CONFIG_FILE: &str = "settings-nav-bar.json";
/// Keep hostile or stale dock preferences bounded before serde materializes them.
const MAX_NAV_PREFS_BYTES: usize = 64 * 1024;
static NAV_LAYER_ID_MAP_LOGGED: AtomicBool = AtomicBool::new(false);

/// One action emitted by the painted controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// Open and focus the existing Front Door search overlay.
    OpenSearch,
    /// Return to the previously active app or Fleet & Mesh tab.
    Back,
    /// Open the untitled all-icons Desktop.
    Home,
    /// Toggle between the bottom taskbar and the left rail.
    ToggleDock,
    /// Open one docked app surface.
    OpenSurface(Surface),
    /// Open a chooser-pinned remote desktop source through the normal chooser
    /// authentication and VDI hand-off path.
    DesktopSource(String),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct NavBarPrefs {
    #[serde(default)]
    mode: DockMode,
}

impl Default for NavBarPrefs {
    fn default() -> Self {
        Self {
            mode: DockMode::Floating,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TransitionState {
    from: DockMode,
    to: DockMode,
    started: Instant,
}

/// Shell-owned state for the bar's persisted placement and transient motion.
#[derive(Debug)]
pub(crate) struct State {
    mode: DockMode,
    transition: Option<TransitionState>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            mode: DockMode::Floating,
            transition: None,
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
    pub(crate) const fn with_mode(mode: DockMode) -> Self {
        Self {
            mode,
            transition: None,
        }
    }

    /// Load the persisted placement, degrading malformed or absent data to the
    /// floating default.
    #[must_use]
    pub(crate) fn load() -> Self {
        Self::default_path().map_or_else(Self::default, |path| {
            let mode = read_bounded_config(&path)
                .ok()
                .and_then(|json| serde_json::from_str::<NavBarPrefs>(&json).ok())
                .map_or(DockMode::Floating, |prefs| prefs.mode);
            Self {
                mode,
                transition: None,
            }
        })
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

    /// Toggle placement and start the slide/melt transition.
    pub(crate) fn toggle(&mut self, now: Instant, motion: MotionMode) {
        self.toggle_mode(now, motion);
        self.save();
    }

    fn toggle_mode(&mut self, now: Instant, motion: MotionMode) {
        let from = self.mode;
        let to = match from {
            DockMode::Floating => DockMode::Docked,
            DockMode::Docked => DockMode::Floating,
        };
        self.mode = to;
        self.transition =
            (motion != MotionMode::Disabled && motion != MotionMode::Reduced).then(|| {
                TransitionState {
                    from,
                    to,
                    started: now,
                }
            });
    }

    /// Paint the dock and return the first clicked action, if any.
    pub(crate) fn mount(
        &mut self,
        ctx: &egui::Context,
        pinned_sources: &[crate::surfaces::DesktopRailSource],
    ) -> Option<Action> {
        if ctx.cumulative_pass_nr() == 0 {
            return None;
        }
        let screen = ctx.screen_rect();
        let pinned_sources = &pinned_sources[..pinned_sources.len().min(MAX_PINNED_SOURCES)];
        let geometry = self.geometry_for(screen, Instant::now(), pinned_sources.len());
        if geometry.finished {
            self.transition = None;
        } else {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        let mut action = None;
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
                for group in &geometry.group_labels {
                    paint_group_label(ctx, &painter, *group);
                }
                for control in &geometry.controls {
                    // The Area's content UI is created with its absolute screen
                    // rect as max_rect, so these interaction rectangles stay in
                    // the same screen space as the painter and AccessKit tree.
                    let response = ui.interact(
                        control.rect,
                        control_id(self.mode, *control),
                        egui::Sense::click(),
                    );
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
                    if let Some(texture) = icon_texture(
                        ctx,
                        control_icon(*control),
                        control.rect.height().min(24.0),
                        Style::NAV_BAR_ICON,
                    ) {
                        let icon_rect = egui::Rect::from_center_size(
                            control.rect.center(),
                            egui::vec2(
                                control.rect.height().min(24.0),
                                control.rect.height().min(24.0),
                            ),
                        );
                        painter.image(
                            texture.id(),
                            icon_rect,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            Style::NAV_BAR_ICON,
                        );
                    }
                    let label = control_label(*control, pinned_sources);
                    install_accessibility(
                        ctx,
                        self.mode,
                        *control,
                        label.as_str(),
                        self.is_docked(),
                    );
                    let _response = response.on_hover_ui(move |ui| {
                        nav_bar_tooltip(ui, label.as_str());
                    });
                    if clicked {
                        action = Some(control_action(*control, pinned_sources));
                    }
                }
            });
        // egui retains Area ordering across frames. A foreground scrim or
        // modal that was previously moved to the top can remain above the
        // navigation bar even after it no longer visibly covers the dock. Keep
        // the global Springboard Dock as the top ordinary foreground Area; the
        // shell still mounts the lock curtain after this and raises it over all
        // chrome when engaged.
        ctx.move_to_top(area.response.layer_id);
        action
    }

    fn geometry(&self, screen: egui::Rect, now: Instant) -> Geometry {
        self.geometry_for(screen, now, 0)
    }

    fn geometry_for(&self, screen: egui::Rect, now: Instant, pinned_count: usize) -> Geometry {
        let floating = floating_geometry_for(screen, pinned_count);
        let docked = docked_geometry_for(screen, pinned_count);
        let Some(transition) = self.transition else {
            return match self.mode {
                DockMode::Floating => floating,
                DockMode::Docked => docked,
            };
        };
        let elapsed = now.saturating_duration_since(transition.started);
        let raw = (elapsed.as_secs_f32() / TRANSITION.as_secs_f32()).clamp(0.0, 1.0);
        if raw >= 1.0 {
            return match transition.to {
                DockMode::Floating => floating,
                DockMode::Docked => docked,
            };
        }
        if transition.from == DockMode::Floating && transition.to == DockMode::Docked {
            let staging = floating_left_edge_staging(screen, &floating);
            if raw < SLIDE_FRACTION {
                let t = smoothstep(raw / SLIDE_FRACTION);
                return interpolate_geometry(&floating, &staging, t, false);
            }
            let t = smoothstep((raw - SLIDE_FRACTION) / (1.0 - SLIDE_FRACTION));
            return interpolate_geometry(&staging, &docked, t, false);
        }
        let staging = floating_left_edge_staging(screen, &floating);
        if raw < 1.0 - SLIDE_FRACTION {
            let t = smoothstep(raw / (1.0 - SLIDE_FRACTION));
            interpolate_geometry(&docked, &staging, t, false)
        } else {
            let t = smoothstep((raw - (1.0 - SLIDE_FRACTION)) / SLIDE_FRACTION);
            interpolate_geometry(&staging, &floating, t, false)
        }
    }

    fn default_path() -> Option<PathBuf> {
        mde_bus::client_data_dir().map(|dir| dir.join(CONFIG_FILE))
    }

    fn save(&self) {
        if let Some(path) = Self::default_path() {
            let prefs = NavBarPrefs { mode: self.mode };
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
    Back,
    Home,
    Pin,
    SurfaceLauncher,
    PinnedDesktop,
}

impl ControlKind {
    const fn id_suffix(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Back => "back",
            Self::Home => "home",
            Self::Pin => "pin",
            Self::SurfaceLauncher => "surface",
            Self::PinnedDesktop => "pinned-desktop",
        }
    }

    const fn icon(self) -> IconId {
        match self {
            Self::Start => IconId::Mark,
            Self::Back => IconId::ArrowLeft,
            Self::Home => IconId::FileHome,
            Self::Pin => IconId::Pin,
            Self::SurfaceLauncher => IconId::Mark,
            Self::PinnedDesktop => IconId::Desktop,
        }
    }

    const fn tooltip(self) -> &'static str {
        match self {
            Self::Start => "Start - Search",
            Self::Back => "Back",
            Self::Home => "Home",
            Self::Pin => "Taskbar placement",
            Self::SurfaceLauncher => "Open app",
            Self::PinnedDesktop => "Open pinned desktop",
        }
    }

    const fn action(self) -> Action {
        match self {
            Self::Start => Action::OpenSearch,
            Self::Back => Action::Back,
            Self::Home => Action::Home,
            Self::Pin => Action::ToggleDock,
            Self::SurfaceLauncher => Action::Home,
            Self::PinnedDesktop => Action::Home,
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

#[derive(Debug, Clone, Copy)]
struct GroupLabel {
    label: &'static str,
    rect: egui::Rect,
    accent: egui::Color32,
}

#[derive(Debug, Clone)]
struct Geometry {
    outer: egui::Rect,
    radius: egui::CornerRadius,
    group_labels: Vec<GroupLabel>,
    controls: Vec<Control>,
    finished: bool,
}

fn floating_geometry(screen: egui::Rect) -> Geometry {
    floating_geometry_for(screen, 0)
}

fn dock_launcher_count() -> usize {
    DOCK_LAUNCHER_GROUPS
        .iter()
        .map(|group| group.surfaces.len())
        .sum()
}

fn dock_control_capacity(pinned_count: usize) -> usize {
    4 + dock_launcher_count() + pinned_count
}

fn control_span(count: usize, edge: f32, gap: f32) -> f32 {
    if count == 0 {
        0.0
    } else {
        count as f32 * edge + (count - 1) as f32 * gap
    }
}

fn effective_floating_pinned_count(screen: egui::Rect, requested: usize, gap: f32) -> usize {
    let requested = requested.min(MAX_PINNED_SOURCES);
    let visible_surfaces = effective_floating_surface_count(screen, gap);
    let center_capacity = floating_center_capacity(screen, gap);
    let available_pins = center_capacity.saturating_sub(visible_surfaces);
    for pinned_count in (0..=requested).rev() {
        if pinned_count <= available_pins {
            return pinned_count;
        }
    }
    0
}

fn effective_floating_surface_count(screen: egui::Rect, gap: f32) -> usize {
    floating_center_capacity(screen, gap).min(dock_launcher_count())
}

fn floating_center_bounds(screen: egui::Rect, gap: f32) -> (f32, f32) {
    let left_start = screen.left() + Style::SP_L;
    let left_cluster_end = left_start + control_span(3, CONTROL_EDGE, gap);
    let right_start = screen.right() - Style::SP_L - CONTROL_EDGE;
    (left_cluster_end + gap, right_start - gap)
}

fn floating_center_capacity(screen: egui::Rect, gap: f32) -> usize {
    let (center_left, center_right) = floating_center_bounds(screen, gap);
    (((center_right - center_left).max(0.0) + gap) / (CONTROL_EDGE + gap)).floor() as usize
}

fn floating_geometry_for(screen: egui::Rect, pinned_count: usize) -> Geometry {
    let gap = FLOATING_GAP;
    let surface_count = effective_floating_surface_count(screen, gap);
    let pinned_count = effective_floating_pinned_count(screen, pinned_count, gap);
    let edge = CONTROL_EDGE;
    let outer = egui::Rect::from_min_size(
        egui::pos2(screen.left(), screen.bottom() - TASKBAR_H),
        egui::vec2(screen.width(), TASKBAR_H),
    );
    let y = outer.top() + (TASKBAR_H - edge) / 2.0;
    let left_start = outer.left() + Style::SP_L;
    let right_x = outer.right() - Style::SP_L - edge;
    let mut controls = Vec::with_capacity(dock_control_capacity(pinned_count));
    let group_labels = Vec::new();
    let mut cursor_x = left_start;
    for kind in [ControlKind::Start, ControlKind::Back, ControlKind::Home] {
        controls.push(Control {
            kind,
            rect: egui::Rect::from_min_size(egui::pos2(cursor_x, y), egui::vec2(edge, edge)),
            surface: None,
            source_index: None,
        });
        cursor_x += edge + gap;
    }
    let center_count = surface_count + pinned_count;
    let center_span = control_span(center_count, edge, gap);
    let (center_left, center_right) = floating_center_bounds(screen, gap);
    let center_start = (center_left + center_right) / 2.0 - center_span / 2.0;
    cursor_x = center_start;
    for surface in DOCK_LAUNCHER_GROUPS
        .iter()
        .flat_map(|group| group.surfaces.iter().copied())
        .take(surface_count)
    {
        controls.push(Control {
            kind: ControlKind::SurfaceLauncher,
            rect: egui::Rect::from_min_size(egui::pos2(cursor_x, y), egui::vec2(edge, edge)),
            surface: Some(surface),
            source_index: None,
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
    controls.push(Control {
        kind: ControlKind::Pin,
        rect: egui::Rect::from_min_size(egui::pos2(right_x, y), egui::vec2(edge, edge)),
        surface: None,
        source_index: None,
    });
    Geometry {
        outer,
        radius: egui::CornerRadius::ZERO,
        group_labels,
        controls,
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

fn docked_control_fits(screen: egui::Rect, cursor_y: f32) -> bool {
    cursor_y + CONTROL_EDGE <= docked_content_bottom(screen)
}

fn docked_group_fits_one_control(screen: egui::Rect, cursor_y: f32, label_h: f32) -> bool {
    cursor_y + label_h + Style::SP_XS + CONTROL_EDGE <= docked_content_bottom(screen)
}

fn docked_geometry_for(screen: egui::Rect, pinned_count: usize) -> Geometry {
    let outer = egui::Rect::from_min_size(screen.left_top(), egui::vec2(DOCKED_W, screen.height()));
    let pinned_count = pinned_count.min(MAX_PINNED_SOURCES);
    let mut controls = Vec::with_capacity(dock_control_capacity(pinned_count));
    let mut group_labels = Vec::with_capacity(DOCK_LAUNCHER_GROUPS.len());
    let mut cursor_y = screen.top() + STATUS_BAR_H + Style::SP_S;
    let label_h = TypographyRole::Caption.line_height().ceil();
    for kind in [ControlKind::Start, ControlKind::Back, ControlKind::Home] {
        controls.push(Control {
            kind,
            rect: egui::Rect::from_min_size(
                egui::pos2(outer.center().x - CONTROL_EDGE / 2.0, cursor_y),
                egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
            ),
            surface: None,
            source_index: None,
        });
        cursor_y += CONTROL_EDGE + Style::SP_XS;
    }
    cursor_y += Style::SP_S - Style::SP_XS;
    for group in DOCK_LAUNCHER_GROUPS {
        if !docked_group_fits_one_control(screen, cursor_y, label_h) {
            break;
        }
        group_labels.push(GroupLabel {
            label: group.label,
            rect: egui::Rect::from_min_max(
                egui::pos2(outer.left() + Style::SP_XS, cursor_y),
                egui::pos2(outer.right() - Style::SP_XS, cursor_y + label_h),
            ),
            accent: group.accent,
        });
        cursor_y += label_h + Style::SP_XS;
        for surface in group.surfaces {
            if !docked_control_fits(screen, cursor_y) {
                break;
            }
            controls.push(Control {
                kind: ControlKind::SurfaceLauncher,
                rect: egui::Rect::from_min_size(
                    egui::pos2(outer.center().x - CONTROL_EDGE / 2.0, cursor_y),
                    egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
                ),
                surface: Some(*surface),
                source_index: None,
            });
            cursor_y += CONTROL_EDGE + Style::SP_XS;
        }
        cursor_y += Style::SP_S - Style::SP_XS;
    }
    for source_index in 0..pinned_count {
        if !docked_control_fits(screen, cursor_y) {
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
    if docked_control_fits(screen, cursor_y) {
        controls.push(Control {
            kind: ControlKind::Pin,
            rect: egui::Rect::from_min_size(
                egui::pos2(outer.center().x - CONTROL_EDGE / 2.0, cursor_y),
                egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
            ),
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
        group_labels,
        controls,
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
    let group_labels = from
        .group_labels
        .iter()
        .zip(&to.group_labels)
        .map(|(from, to)| GroupLabel {
            label: from.label,
            rect: lerp_rect(from.rect, to.rect, t),
            accent: from.accent,
        })
        .collect();
    Geometry {
        outer,
        radius: lerp_radius(from.radius, to.radius, t),
        group_labels,
        controls,
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
        group_labels: geometry
            .group_labels
            .iter()
            .copied()
            .map(|group| GroupLabel {
                label: group.label,
                rect: group.rect.translate(delta),
                accent: group.accent,
            })
            .collect(),
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

fn paint_group_label(ctx: &egui::Context, painter: &egui::Painter, group: GroupLabel) {
    let color = Style::resolve_color(ctx, group.accent);
    let label = painter.layout_job(Style::typography_job(
        group.label,
        TypographyRole::Caption,
        color,
        f32::INFINITY,
    ));
    painter.galley(
        egui::pos2(
            group.rect.center().x - label.size().x / 2.0,
            group.rect.center().y - label.size().y / 2.0,
        ),
        label,
        color,
    );
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
        (Some(index), _) => egui::Id::new((
            "construct-navigation-bar",
            mode.id_suffix(),
            "pinned",
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
            "mde-web-omnibox-suggestions-overlay",
            egui::Id::new("mde-web-omnibox-suggestions-overlay"),
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

fn control_label(
    control: Control,
    pinned_sources: &[crate::surfaces::DesktopRailSource],
) -> String {
    if let Some(source) = control
        .source_index
        .and_then(|index| pinned_sources.get(index))
    {
        return format!("Open pinned desktop {} on {}", source.label, source.node);
    }
    if let Some(surface) = control.surface {
        let group = dock_launcher_group_label(surface);
        let label = dock_launcher_surface_label(surface);
        return if group.is_empty() {
            format!("Open {label}")
        } else {
            format!("Open {label} from {group}")
        };
    }
    if control.kind == ControlKind::Pin {
        "Taskbar placement".to_owned()
    } else {
        control.kind.tooltip().to_owned()
    }
}

fn control_action(
    control: Control,
    pinned_sources: &[crate::surfaces::DesktopRailSource],
) -> Action {
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
    fn defaults_to_an_unpinned_springboard_dock() {
        assert_eq!(State::default().mode, DockMode::Floating);
        assert_eq!(TASKBAR_H, 48.0);
        assert_eq!(DOCKED_W, 56.0);
        assert_eq!(SPRINGBOARD_DOCK_RESERVED_H, 48.0);
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
        assert_eq!(control_label(desktop, &[]), "Open VMs from Infra");
        assert_eq!(control_label(files, &[]), "Open File Manager from Ops");
    }

    #[test]
    fn floating_and_docked_geometry_have_the_locked_edges() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let floating = floating_geometry(screen);
        let docked = docked_geometry(screen);
        assert_eq!(floating.outer.size(), egui::vec2(screen.width(), TASKBAR_H));
        assert_eq!(floating.outer.left(), screen.left());
        assert_eq!(floating.outer.right(), screen.right());
        assert_eq!(floating.outer.bottom(), screen.bottom());
        assert_eq!(docked.outer.width(), DOCKED_W);
        assert_eq!(docked.outer.top(), screen.top());
        assert_eq!(docked.controls[0].rect.top(), STATUS_BAR_H + Style::SP_S);
        assert_eq!(docked.controls.len(), dock_control_capacity(0));
        assert!(floating.group_labels.is_empty());
        assert_eq!(docked.group_labels.len(), DOCK_LAUNCHER_GROUPS.len());
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
            .all(|control| { (control.rect.width() - CONTROL_EDGE).abs() < f32::EPSILON }));
        assert_hit_targets_inside_backing("floating sub-640 screen".to_string(), &geometry);
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
            vec![ControlKind::Start, ControlKind::Back, ControlKind::Home],
            "rail safety controls must remain first even when app launchers overflow"
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
                        }),
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
            assert_eq!(
                area_response.rect, expected_outer,
                "the navigation Area must shield only the bar, not the home/workspace"
            );

            let update = output
                .platform_output
                .accesskit_update
                .as_ref()
                .expect("headless navigation bar should publish an AccessKit tree");
            for (kind, expected_label) in [
                (ControlKind::Start, "Start - Search"),
                (ControlKind::Back, "Back"),
                (ControlKind::Home, "Home"),
                (ControlKind::Pin, "Taskbar placement"),
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
                (ControlKind::Start, Action::OpenSearch),
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
                Surface::Desktop,
                Surface::Terminal,
                Surface::MapsLocation,
                Surface::Communications,
                Surface::Files,
                Surface::Music,
                Surface::Media,
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
        assert_eq!(placement.rect.right(), screen.right() - Style::SP_L);
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
            }),
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
                .take(3)
                .map(|control| control.kind)
                .collect::<Vec<_>>(),
            vec![ControlKind::Start, ControlKind::Back, ControlKind::Home],
            "the taskbar must lead with Start, Back, Home"
        );
        assert_eq!(
            floating
                .controls
                .iter()
                .take(3)
                .map(|control| control_icon(*control))
                .collect::<Vec<_>>(),
            vec![IconId::Mark, IconId::ArrowLeft, IconId::FileHome,]
        );
        assert_eq!(
            floating
                .controls
                .iter()
                .filter_map(|control| control.surface)
                .collect::<Vec<_>>(),
            vec![
                Surface::Desktop,
                Surface::Terminal,
                Surface::MapsLocation,
                Surface::Communications,
                Surface::Files,
                Surface::Music,
                Surface::Media,
                Surface::Browser,
            ],
            "the grouped dock must use the operator survey order"
        );
        assert!(floating.group_labels.is_empty());
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
                .take(3)
                .map(|control| control.kind)
                .collect::<Vec<_>>(),
            vec![ControlKind::Start, ControlKind::Back, ControlKind::Home]
        );
        assert!(settled.controls[0].rect.top() < settled.controls[1].rect.top());
        assert!(settled.controls[1].rect.top() < settled.controls[2].rect.top());
        assert_eq!(settled.controls[2].kind, ControlKind::Home);
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
