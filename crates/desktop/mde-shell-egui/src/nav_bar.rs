//! The Construct Springboard Dock: persistent shell navigation plus chooser pins.
//!
//! The dock is shell-owned and always reserves its footprint. In the default
//! bottom-left placement it is a solid-black springboard pill; Pin moves it into
//! the 56px left rail that the central layout reserves below the top strip.
//! Placement is persisted per seat, while the visual transition is kept in
//! memory so a restart never waits on animation state. Chooser-pinned remote
//! desktop sources are rendered as additional dock targets from the chooser's
//! existing read model.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mde_egui::egui;
use mde_egui::{Elevation, MotionMode, Style, TypographyRole};
use mde_theme::brand::icons::IconId;
use serde::{Deserialize, Serialize};

use crate::status_bar::STATUS_BAR_H;
use crate::surfaces::icon_texture;

/// The reserved left-rail width in docked mode.
pub(crate) const DOCKED_W: f32 = 56.0;
/// The fixed floating pill width selected in the navigation-bar design review.
pub(crate) const FLOATING_W: f32 = 240.0;
/// The fixed floating pill height selected in the navigation-bar design review.
pub(crate) const FLOATING_H: f32 = 56.0;
/// The bottom/left breathing room around the undocked pill.
const FLOATING_MARGIN: f32 = 16.0;
/// Bottom space reserved by the horizontal Springboard Dock, including its
/// breathing room below the black pill.
pub(crate) const SPRINGBOARD_DOCK_RESERVED_H: f32 = FLOATING_H + FLOATING_MARGIN;
/// The icon controls keep a comfortable 48px target in the compact dock.
const CONTROL_EDGE: f32 = 48.0;
/// Maximum number of chooser-pinned sources shown in the dock. The full
/// chooser remains the unbounded discovery surface; the dock is a quick rail.
const MAX_PINNED_SOURCES: usize = 8;
/// The transition first slides left, then melts into the vertical rail.
const SLIDE_FRACTION: f32 = 0.34;
/// Total transition length: short enough to feel direct, long enough to read.
const TRANSITION: Duration = Duration::from_millis(360);
/// Persisted per-seat preference.
const CONFIG_FILE: &str = "settings-nav-bar.json";

/// One action emitted by the painted controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// Return to the previously active app or Fleet & Mesh tab.
    Back,
    /// Open the untitled all-icons Desktop.
    Home,
    /// Toggle between the floating pill and the left rail.
    ToggleDock,
    /// Open a chooser-pinned remote desktop source through the normal chooser
    /// authentication and VDI hand-off path.
    DesktopSource(String),
}

/// The persisted placement choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum DockMode {
    /// The undocked bottom-left pill.
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
            let mode = fs::read_to_string(path)
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
        self.transition.as_ref().map_or(
            matches!(self.mode, DockMode::Floating),
            |transition| {
                matches!(transition.from, DockMode::Floating)
                    || matches!(transition.to, DockMode::Floating)
            },
        )
    }

    /// Whether the central workspace must reserve the vertical pinned rail.
    #[must_use]
    pub(crate) fn reserves_left_space(&self) -> bool {
        self.transition.as_ref().map_or(
            matches!(self.mode, DockMode::Docked),
            |transition| {
                matches!(transition.from, DockMode::Docked)
                    || matches!(transition.to, DockMode::Docked)
            },
        )
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
        egui::Area::new(egui::Id::new("construct-navigation-bar"))
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
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_min_size(geometry.outer.size());
                let painter = ui.painter().clone();
                paint_backing(&painter, &geometry);
                for control in &geometry.controls {
                    // egui interaction rectangles are screen-space, just like
                    // the painter and AccessKit rectangles. Translating these
                    // into Area-local coordinates makes every control miss on
                    // any non-zero-position bar.
                    let response = ui.interact(control.rect, control_id(*control), egui::Sense::click());
                    let hovered = response.hovered();
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
                    install_accessibility(ctx, *control, label.as_str(), self.is_docked());
                    let response = response.on_hover_ui(move |ui| {
                        nav_bar_tooltip(ui, label.as_str());
                    });
                    if response.clicked() {
                        action = Some(control_action(*control, pinned_sources));
                    }
                }
            });
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
            let staging = translate_geometry(&floating, egui::vec2(-FLOATING_MARGIN, 0.0));
            if raw < SLIDE_FRACTION {
                let t = smoothstep(raw / SLIDE_FRACTION);
                return interpolate_geometry(&floating, &staging, t, false);
            }
            let t = smoothstep((raw - SLIDE_FRACTION) / (1.0 - SLIDE_FRACTION));
            return interpolate_geometry(&staging, &docked, t, false);
        }
        let staging = translate_geometry(&floating, egui::vec2(-FLOATING_MARGIN, 0.0));
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlKind {
    Back,
    Home,
    Pin,
    PinnedDesktop,
}

impl ControlKind {
    const fn id_suffix(self) -> &'static str {
        match self {
            Self::Back => "back",
            Self::Home => "home",
            Self::Pin => "pin",
            Self::PinnedDesktop => "pinned-desktop",
        }
    }

    const fn icon(self) -> IconId {
        match self {
            Self::Back => IconId::ArrowLeft,
            Self::Home => IconId::FileHome,
            Self::Pin => IconId::Pin,
            Self::PinnedDesktop => IconId::Desktop,
        }
    }

    const fn tooltip(self) -> &'static str {
        match self {
            Self::Back => "Back",
            Self::Home => "Home",
            Self::Pin => "Pin Springboard Dock",
            Self::PinnedDesktop => "Open pinned desktop",
        }
    }

    const fn action(self) -> Action {
        match self {
            Self::Back => Action::Back,
            Self::Home => Action::Home,
            Self::Pin => Action::ToggleDock,
            Self::PinnedDesktop => Action::Home,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Control {
    kind: ControlKind,
    rect: egui::Rect,
    source_index: Option<usize>,
}

#[derive(Debug, Clone)]
struct Geometry {
    outer: egui::Rect,
    radius: egui::CornerRadius,
    controls: Vec<Control>,
    finished: bool,
}

fn floating_geometry(screen: egui::Rect) -> Geometry {
    floating_geometry_for(screen, 0)
}

fn floating_geometry_for(screen: egui::Rect, pinned_count: usize) -> Geometry {
    let gap = 4.0;
    let base_count = 3.0;
    let pinned_count_f = pinned_count as f32;
    let total_controls = base_count + pinned_count_f;
    let separators = if pinned_count > 0 { 1.0 } else { 0.0 };
    let content_w = total_controls.mul_add(CONTROL_EDGE, (total_controls - 1.0) * gap)
        + separators * (Style::SP_S - gap);
    let width = FLOATING_W.max(content_w + Style::SP_L * 2.0);
    let outer = egui::Rect::from_min_size(
        egui::pos2(
            screen.left() + FLOATING_MARGIN,
            screen.bottom() - FLOATING_MARGIN - FLOATING_H,
        ),
        egui::vec2(width.min((screen.width() - 2.0 * FLOATING_MARGIN).max(1.0)), FLOATING_H),
    );
    let first_x = outer.left() + Style::SP_L;
    let y = outer.center().y - CONTROL_EDGE / 2.0;
    let mut controls = Vec::with_capacity(3 + pinned_count);
    for (idx, kind) in [ControlKind::Back, ControlKind::Home, ControlKind::Pin]
        .into_iter()
        .enumerate()
    {
        controls.push(Control {
            kind,
            rect: egui::Rect::from_min_size(
                egui::pos2(first_x + idx as f32 * (CONTROL_EDGE + gap), y),
                egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
            ),
            source_index: None,
        });
    }
    let mut source_x = controls
        .last()
        .map_or(first_x, |control| control.rect.right() + Style::SP_S);
    for source_index in 0..pinned_count {
        controls.push(Control {
            kind: ControlKind::PinnedDesktop,
            rect: egui::Rect::from_min_size(
                egui::pos2(source_x, y),
                egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
            ),
            source_index: Some(source_index),
        });
        source_x += CONTROL_EDGE + gap;
    }
    Geometry {
        outer,
        radius: egui::CornerRadius::same((FLOATING_H / 2.0) as u8),
        controls,
        finished: true,
    }
}

fn docked_geometry(screen: egui::Rect) -> Geometry {
    docked_geometry_for(screen, 0)
}

fn docked_geometry_for(screen: egui::Rect, pinned_count: usize) -> Geometry {
    let outer = egui::Rect::from_min_size(screen.left_top(), egui::vec2(DOCKED_W, screen.height()));
    let mut controls = Vec::with_capacity(3 + pinned_count);
    let first_y = screen.top() + STATUS_BAR_H + Style::SP_S;
    for (idx, kind) in [ControlKind::Back, ControlKind::Home, ControlKind::Pin]
        .into_iter()
        .enumerate()
    {
        controls.push(Control {
            kind,
            rect: egui::Rect::from_min_size(
                egui::pos2(
                    outer.center().x - CONTROL_EDGE / 2.0,
                    first_y + idx as f32 * (CONTROL_EDGE + Style::SP_XS),
                ),
                egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
            ),
            source_index: None,
        });
    }
    for source_index in 0..pinned_count {
        controls.push(Control {
            kind: ControlKind::PinnedDesktop,
            rect: egui::Rect::from_min_size(
                egui::pos2(
                    outer.center().x - CONTROL_EDGE / 2.0,
                    first_y + (3 + source_index) as f32 * (CONTROL_EDGE + Style::SP_XS),
                ),
                egui::vec2(CONTROL_EDGE, CONTROL_EDGE),
            ),
            source_index: Some(source_index),
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
            source_index: from.source_index,
        })
        .collect();
    Geometry {
        outer,
        radius: lerp_radius(from.radius, to.radius, t),
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
                source_index: control.source_index,
            })
            .collect(),
        finished: geometry.finished,
    }
}

fn paint_backing(painter: &egui::Painter, geometry: &Geometry) {
    let shadow = geometry.outer.translate(egui::vec2(0.0, 3.0));
    painter.rect_filled(shadow, geometry.radius, Elevation::Overlay.shadow().umbra);
    painter.rect_filled(geometry.outer, geometry.radius, Style::NAV_BAR_BG);
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

fn control_id(control: Control) -> egui::Id {
    match control.source_index {
        Some(index) => egui::Id::new(("construct-navigation-bar", "pinned", index)),
        None => egui::Id::new(("construct-navigation-bar", control.kind.id_suffix())),
    }
}

fn control_icon(control: Control) -> IconId {
    control.kind.icon()
}

fn control_label(
    control: Control,
    pinned_sources: &[crate::surfaces::DesktopRailSource],
) -> String {
    match control.source_index.and_then(|index| pinned_sources.get(index)) {
        Some(source) => format!("Open pinned desktop {} on {}", source.label, source.node),
        None if control.kind == ControlKind::Pin => "Pin Springboard Dock".to_owned(),
        None => control.kind.tooltip().to_owned(),
    }
}

fn control_action(
    control: Control,
    pinned_sources: &[crate::surfaces::DesktopRailSource],
) -> Action {
    match control.source_index.and_then(|index| pinned_sources.get(index)) {
        Some(source) => Action::DesktopSource(source.id.clone()),
        None => control.kind.action(),
    }
}

fn install_accessibility(ctx: &egui::Context, control: Control, label: &str, _docked: bool) {
    let _ = ctx.accesskit_node_builder(
        control_id(control),
        |node| {
            node.set_role(egui::accesskit::Role::Button);
            node.set_label(label.to_owned());
            node.set_bounds(egui::accesskit::Rect {
                x0: control.rect.left().into(),
                y0: control.rect.top().into(),
                x1: control.rect.right().into(),
                y1: control.rect.bottom().into(),
            });
            node.add_action(egui::accesskit::Action::Click);
        },
    );
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&prefs)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_an_unpinned_springboard_dock() {
        assert_eq!(State::default().mode, DockMode::Floating);
        assert_eq!(FLOATING_W, 240.0);
        assert_eq!(FLOATING_H, 56.0);
        assert_eq!(DOCKED_W, 56.0);
        assert_eq!(SPRINGBOARD_DOCK_RESERVED_H, 72.0);
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
    fn floating_and_docked_geometry_have_the_locked_edges() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let floating = floating_geometry(screen);
        let docked = docked_geometry(screen);
        assert_eq!(floating.outer.size(), egui::vec2(FLOATING_W, FLOATING_H));
        assert_eq!(docked.outer.width(), DOCKED_W);
        assert_eq!(docked.outer.top(), screen.top());
        assert_eq!(docked.controls[0].rect.top(), STATUS_BAR_H + Style::SP_S);
        assert_eq!(docked.controls.len(), 3);
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
        let loaded = fs::read_to_string(&path).expect("read nav-bar prefs");
        let prefs: NavBarPrefs = serde_json::from_str(&loaded).expect("decode nav-bar prefs");
        assert_eq!(prefs.mode, DockMode::Docked);
        fs::write(&path, "not json").expect("write malformed nav-bar prefs");
        let fallback = fs::read_to_string(&path)
            .ok()
            .and_then(|json| serde_json::from_str::<NavBarPrefs>(&json).ok())
            .map_or(DockMode::Floating, |prefs| prefs.mode);
        assert_eq!(fallback, DockMode::Floating);
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
                (ControlKind::Back, "Back"),
                (ControlKind::Home, "Home"),
                (ControlKind::Pin, "Pin Springboard Dock"),
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
        let home = floating_geometry(screen).controls[1].rect.center();

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
        assert_eq!(geometry.controls.len(), 4);
        assert_eq!(geometry.controls[2].kind, ControlKind::Pin);
        assert_eq!(geometry.controls[3].kind, ControlKind::PinnedDesktop);
        assert_eq!(geometry.controls[3].source_index, Some(0));
        assert!(
            geometry.outer.width() >= FLOATING_W,
            "the dock must grow to retain the pinned source target"
        );

        let ctx = egui::Context::default();
        let mut state = State::with_mode(DockMode::Floating);
        let target = geometry.controls[3].rect.center();
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
    fn headless_geometry_proves_black_pill_and_slide_then_melt_rail() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let floating = floating_geometry(screen);
        let docked = docked_geometry(screen);

        assert_eq!(
            floating.outer,
            egui::Rect::from_min_max(egui::pos2(16.0, 728.0), egui::pos2(256.0, 784.0)),
            "undocked navigation must be a fixed bottom-left pill"
        );
        assert_eq!(
            floating.radius,
            egui::CornerRadius::same((FLOATING_H / 2.0) as u8),
            "the undocked backing must stay pill-shaped"
        );
        assert_eq!(
            floating
                .controls
                .iter()
                .map(|control| control.kind)
                .collect::<Vec<_>>(),
            vec![ControlKind::Back, ControlKind::Home, ControlKind::Pin],
            "the dock order must remain Back, Home, Pin"
        );
        assert_eq!(
            floating
                .controls
                .iter()
                .map(|control| control.kind.icon())
                .collect::<Vec<_>>(),
            vec![IconId::ArrowLeft, IconId::FileHome, IconId::Pin,]
        );
        assert!(
            floating
                .controls
                .iter()
                .all(|control| control.rect.center().y == floating.outer.center().y),
            "the pill controls must share one horizontal row"
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
            "the undocked pill backing must paint an opaque black mesh"
        );

        let start = Instant::now();
        let mut state = State::default();
        state.toggle_mode(start, MotionMode::Normal);

        let at_start = state.geometry(screen, start);
        assert_eq!(at_start.outer, floating.outer);
        assert_eq!(at_start.radius, floating.radius);

        // The first phase is a horizontal slide into the left edge: size,
        // vertical position, and pill radius are unchanged before the melt.
        let during_slide = state.geometry(screen, start + Duration::from_millis(90));
        assert!(during_slide.outer.left() < floating.outer.left());
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
        assert!(during_melt.radius.nw < floating.radius.nw);
        assert!(during_melt.controls[0].rect.top() < during_slide.controls[0].rect.top());

        let settled = state.geometry(screen, start + TRANSITION);
        assert_eq!(settled.outer, docked.outer);
        assert_eq!(
            settled
                .controls
                .iter()
                .map(|control| control.kind)
                .collect::<Vec<_>>(),
            vec![ControlKind::Back, ControlKind::Home, ControlKind::Pin]
        );
        assert!(settled.controls[0].rect.top() < settled.controls[1].rect.top());
        assert!(settled.controls[1].rect.top() < settled.controls[2].rect.top());
        assert_eq!(settled.controls[2].kind, ControlKind::Pin);
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
}
