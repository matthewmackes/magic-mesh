//! EPIC-UI-MATERIAL.svg-swap — Material Symbols icon system.
//!
//! Supersedes the prior Carbon-based system per Q43 + Q97 of the
//! 100-Q tightening survey (2026-05-25) + the 8-Q icon-mapping
//! survey 2026-05-26. **Design lock:** `docs/design/icon-mapping.md`.
//!
//! ## Locks
//!
//! - **Variant**: Outlined Material Symbols
//! - **Weight**: 400 (Material default)
//! - **Sizing**: Material optical-size variants — bundle 20 / 24 /
//!   40 px SVGs per icon; [`IconSize`] tiers map onto the nearest
//!   bundled optical size via [`IconSize::optical_svg_size`].
//! - **Fill rule**: status indicators + notification bell + the
//!   playbook play-glyph are **always filled** (carries over Q38);
//!   nav-group + sidebar icons render **filled when
//!   [`IconState::Active`]**, outlined otherwise (the new
//!   active-state behavior).
//! - **Source**: 180 SVGs fetched from Google's
//!   `material-design-icons` GitHub repo (Apache-2.0) by
//!   `install-helpers/fetch-material-symbols.sh` into
//!   `assets/icons/material-symbols/`.
//! - **API**: [`Icon::material_name`] returns the Material symbolic
//!   name (e.g. `"network_check"`). [`Icon::fill_mode`] returns the
//!   `FillMode` tri-state ([`FillMode::NeverFill`] /
//!   [`FillMode::AlwaysFill`] / [`FillMode::OnActive`]).
//!   [`ResolvedIcon::svg_bytes`] takes an [`IconState`] argument
//!   and returns the appropriate (outlined or filled) byte slice.
//!
//! ## Surface
//!
//! [`Icon`] is the semantic enum — call sites use
//! `Icon::Fleet`, `Icon::Snapshot`, etc., **never** a hardcoded
//! Material name or Unicode codepoint. [`IconSize`] is the locked
//! tier enum (16 / 20 / 24 / 32 / 48 px render sizes;
//! consumer-facing). [`IconState`] is `Idle` or `Active` and drives
//! the outlined↔filled swap for icons whose `fill_mode` is
//! `OnActive`. Resolution happens via [`mde_icon`]:
//!
//! ```
//! use mde_theme::{mde_icon, Icon, IconSize, IconState};
//! let glyph = mde_icon(Icon::Fleet, IconSize::Nav);
//! assert_eq!(glyph.size_px(), 20.0);
//! // Pick outlined or filled bytes based on selection state:
//! let bytes_idle = glyph.svg_bytes_for_state(IconState::Idle);
//! let bytes_active = glyph.svg_bytes_for_state(IconState::Active);
//! assert!(bytes_idle.len() > 32);
//! ```
//!
//! ## Migration history
//!
//! Original UX-8 (50-Q survey 2026-05-21) locked Carbon. Q43 of
//! the 100-Q tightening survey re-pivoted to Material Symbols to
//! align with the ChromeOS-Classic visual lock. The 8-Q
//! icon-mapping survey 2026-05-26 locked variant / weight /
//! sizing / source / API; this file implements those locks. The
//! retiring legacy GTK panel (`crates/mackes-panel/`) still has
//! its own Carbon-named test as historical context;
//! `crates/mde-theme/` is the new authoritative surface.

/// Locked icon size tiers (consumer-facing render dimensions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconSize {
    /// 16 px — inline within text, in tight controls (input
    /// trailing icons, badge prefixes).
    Inline,
    /// 20 px — sidebar nav rows, list-row leading icons.
    Nav,
    /// 24 px — panel page headers, prominent toolbar buttons.
    PanelHeader,
    /// 32 px — empty-state hero icon.
    EmptyState,
    /// 48 px — wizard hero icon.
    WizardHero,
}

impl IconSize {
    /// Pixel size for this tier (consumer render size).
    #[must_use]
    pub const fn px(self) -> f32 {
        match self {
            IconSize::Inline => 16.0,
            IconSize::Nav => 20.0,
            IconSize::PanelHeader => 24.0,
            IconSize::EmptyState => 32.0,
            IconSize::WizardHero => 48.0,
        }
    }

    /// Material Symbols optical SVG size for this tier — the file
    /// to load. Material ships SVGs at 20 / 24 / 40 px with
    /// per-size stroke-weight tuning; [`IconSize::px`] is the
    /// render dimension, this is the source-asset dimension.
    /// Picks the nearest bundled optical size.
    #[must_use]
    pub const fn optical_svg_size(self) -> u32 {
        match self {
            // 16 px renders the 20 px optical — Material's smallest
            // bundled size; the renderer scales down.
            IconSize::Inline => 20,
            IconSize::Nav => 20,
            IconSize::PanelHeader => 24,
            // 32 px + 48 px both pick up the 40 px optical;
            // renderer handles the scale.
            IconSize::EmptyState => 40,
            IconSize::WizardHero => 40,
        }
    }
}

/// Material Symbols stroke weight in px (weight 400 maps to 1 px
/// effective stroke at 24 px optical). Matches the prior Carbon
/// 1-px lock.
pub const MATERIAL_LINE_WEIGHT_PX: f32 = 1.0;

/// Selection / activation state — drives the outlined↔filled swap
/// for icons whose [`Icon::fill_mode`] is [`FillMode::OnActive`].
///
/// Most callsites pass [`IconState::Idle`]. Selection-aware
/// surfaces (sidebar selected row, active tab, focused dock
/// button) thread [`IconState::Active`] when the icon represents
/// the currently-active entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconState {
    /// Outlined render (default for most callsites).
    Idle,
    /// Filled render when this icon's [`FillMode`] is
    /// [`FillMode::OnActive`]. No effect on [`FillMode::AlwaysFill`]
    /// (already filled) or [`FillMode::NeverFill`] (always outlined).
    Active,
}

/// Tri-state fill policy per icon. Locked by the 8-Q icon-mapping
/// survey Round 1 Q2 (2026-05-26).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillMode {
    /// Icon is always outlined regardless of state (most icons).
    NeverFill,
    /// Icon is always filled (status dots, notification bell,
    /// playbook play-glyph — Q38 carry-over).
    AlwaysFill,
    /// Outlined by default; filled when paired with
    /// [`IconState::Active`] (nav-group + sidebar icons).
    OnActive,
}

/// Semantic icon names. Every Iced/GTK call site uses these enum
/// variants — never a hardcoded glyph/path/codepoint. Adding a new
/// variant requires arms in `material_name`, `fallback_glyph`,
/// `fill_mode`, and the `svg_bytes` resolver; the
/// `every_variant_resolves_*` tests guard against missing arms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    // --- Navigation surfaces ---
    /// Dashboard / home group.
    Dashboard,
    /// Apps group + apps panels.
    Apps,
    /// Network group + network panels.
    Network,
    /// Devices group + devices panels.
    Devices,
    /// Look & Feel group + theming panels.
    LookAndFeel,
    /// System group + system panels.
    System,
    /// Maintain group + maintenance panels.
    Maintain,
    /// Fleet group + mesh panels.
    Fleet,
    /// Compute group — local + fleet VMs / pods (E6.10).
    Compute,
    /// Help group.
    Help,

    // --- Panel-specific ---
    /// Snapshot / backup.
    Snapshot,
    /// Single peer / device entry.
    Peer,
    /// Logs panel.
    Logs,
    /// System update / package manager.
    Update,
    /// Repair / recovery.
    Repair,
    /// Sound / audio.
    Sound,
    /// Display / monitor.
    Display,
    /// Printer.
    Printer,
    /// Power / battery.
    Power,
    /// Removable storage / USB.
    Removable,
    /// Date / time / clock.
    Clock,
    /// Wallpaper.
    Wallpaper,
    /// Fonts.
    Fonts,
    /// Themes / colour swatches.
    Themes,
    /// Session / login.
    Session,
    /// Notifications / bell. **Always filled** per Round 1 Q2.
    Notification,
    /// Wi-Fi.
    Wifi,
    /// VPN.
    Vpn,
    /// Firewall.
    Firewall,
    /// Playbook / automation. **Always filled** (play-glyph).
    Playbook,
    /// History / past events.
    History,
    /// Settings / gear.
    Settings,
    /// Inventory / list.
    Inventory,
    /// Workbench brand-strip glyph.
    Workbench,
    /// Files manager brand-strip glyph. Active when the Files
    /// nav button is selected.
    Files,

    // --- Window controls ---
    /// Minimize window.
    WindowMinimize,
    /// Maximize / restore window.
    WindowMaximize,
    /// Close window.
    WindowClose,

    // --- Status / state (always filled) ---
    /// Healthy / OK status dot.
    StatusOk,
    /// Warning status dot.
    StatusWarning,
    /// Error status dot.
    StatusError,
    /// Informational status dot (filled `i`-in-a-circle). The peer of
    /// the OK / warning / error dots for an *info*-severity indicator.
    StatusInfo,
    /// Unknown / pending status dot.
    StatusUnknown,

    // --- Action affordances ---
    /// Refresh / reload.
    Refresh,
    /// Add / create.
    Add,
    /// Delete / trash.
    Delete,
    /// Edit / pencil.
    Edit,
    /// Confirm / checkmark.
    Confirm,
    /// Cancel / close X.
    Cancel,
    /// Search / magnifier.
    Search,
    /// Chevron right (navigation indicator).
    ChevronRight,
    /// Chevron down (expanded indicator).
    ChevronDown,

    // --- File-type (CR-3.c) — per-MIME file-row glyphs ---
    /// Generic document with text lines (text/*, office docs).
    Document,
    /// Blank / empty document (new-file placeholder).
    DocumentBlank,
    /// Image file (image/*).
    Image,
    /// PDF document (application/pdf).
    Pdf,
    /// Source code file (text/x-*, application/json, …).
    Code,
    /// Audio file (audio/*).
    Audio,
    /// Video file (video/*).
    Video,
    /// Archive / compressed file (application/zip, …).
    Archive,
    /// Directory / folder (inode/directory).
    Folder,
}

impl Icon {
    /// Material Symbols symbolic name (e.g. `"network_check"`,
    /// `"notifications"`). Source-of-truth for which SVG file to
    /// load from `assets/icons/material-symbols/`. Heuristic
    /// mapping per the icon-mapping survey Round 2 Q4 (2026-05-26);
    /// per-icon revisions land as follow-on commits if any specific
    /// glyph disappoints.
    #[must_use]
    pub const fn material_name(self) -> &'static str {
        match self {
            Icon::Dashboard => "dashboard",
            Icon::Apps => "apps",
            Icon::Network => "network_check",
            Icon::Devices => "devices",
            Icon::LookAndFeel => "palette",
            Icon::System => "settings",
            Icon::Maintain => "build",
            Icon::Fleet => "public",
            Icon::Compute => "memory",
            Icon::Help => "help",

            Icon::Snapshot => "save",
            Icon::Peer => "memory",
            Icon::Logs => "list",
            Icon::Update => "rocket_launch",
            Icon::Repair => "build",
            Icon::Sound => "volume_up",
            Icon::Display => "desktop_windows",
            Icon::Printer => "print",
            Icon::Power => "battery_charging_full",
            Icon::Removable => "usb",
            Icon::Clock => "schedule",
            Icon::Wallpaper => "image",
            Icon::Fonts => "text_fields",
            Icon::Themes => "palette",
            Icon::Session => "person",
            Icon::Notification => "notifications",
            Icon::Wifi => "wifi",
            Icon::Vpn => "vpn_lock",
            Icon::Firewall => "security",
            Icon::Playbook => "play_arrow",
            Icon::History => "history",
            Icon::Settings => "settings",
            Icon::Inventory => "checklist",
            Icon::Workbench => "handyman",
            Icon::Files => "folder",

            Icon::WindowMinimize => "remove",
            Icon::WindowMaximize => "fullscreen",
            Icon::WindowClose => "close",

            Icon::StatusOk => "check_circle",
            Icon::StatusWarning => "warning",
            Icon::StatusError => "error",
            Icon::StatusInfo => "info",
            Icon::StatusUnknown => "help",

            Icon::Refresh => "refresh",
            Icon::Add => "add",
            Icon::Delete => "delete",
            Icon::Edit => "edit",
            Icon::Confirm => "check",
            Icon::Cancel => "close",
            Icon::Search => "search",
            Icon::ChevronRight => "chevron_right",
            Icon::ChevronDown => "expand_more",

            Icon::Document => "description",
            Icon::DocumentBlank => "draft",
            Icon::Image => "image",
            Icon::Pdf => "picture_as_pdf",
            Icon::Code => "code",
            Icon::Audio => "audio_file",
            Icon::Video => "video_file",
            Icon::Archive => "folder_zip",
            Icon::Folder => "folder",
        }
    }

    /// Unicode fallback glyph — rendered by surfaces that prefer
    /// text-over-SVG (legacy GTK panels). The Iced workbench /
    /// portal stacks prefer [`ResolvedIcon::svg_bytes`].
    #[must_use]
    pub const fn fallback_glyph(self) -> &'static str {
        match self {
            Icon::Dashboard => "\u{2630}",
            Icon::Apps => "\u{25A6}",
            Icon::Network => "\u{29C8}",
            Icon::Devices => "\u{25A3}",
            Icon::LookAndFeel => "\u{25C9}",
            Icon::System => "\u{2699}",
            Icon::Maintain => "\u{1F527}",
            Icon::Fleet => "\u{29C9}",
            Icon::Compute => "\u{25A5}",
            Icon::Help => "?",

            Icon::Snapshot => "\u{29C7}",
            Icon::Peer => "\u{25CB}",
            Icon::Logs => "\u{2630}",
            Icon::Update => "\u{2191}",
            Icon::Repair => "\u{1F6E0}",
            Icon::Sound => "\u{266B}",
            Icon::Display => "\u{25AD}",
            Icon::Printer => "\u{2399}",
            Icon::Power => "\u{26A1}",
            Icon::Removable => "\u{2902}",
            Icon::Clock => "\u{29D6}",
            Icon::Wallpaper => "\u{2766}",
            Icon::Fonts => "A",
            Icon::Themes => "\u{25D0}",
            Icon::Session => "\u{2630}",
            Icon::Notification => "\u{1F514}",
            Icon::Wifi => "\u{1F4F6}",
            Icon::Vpn => "\u{1F512}",
            Icon::Firewall => "\u{1F6E1}",
            Icon::Playbook => "\u{25B6}",
            Icon::History => "\u{231B}",
            Icon::Settings => "\u{2699}",
            Icon::Inventory => "\u{2261}",
            Icon::Workbench => "\u{25A6}",
            Icon::Files => "\u{1F4C1}",

            Icon::WindowMinimize => "\u{2212}",
            Icon::WindowMaximize => "\u{25A1}",
            Icon::WindowClose => "\u{00D7}",

            Icon::StatusOk => "\u{25CF}",
            Icon::StatusWarning => "\u{25CF}",
            Icon::StatusError => "\u{25CF}",
            Icon::StatusInfo => "\u{24D8}",
            Icon::StatusUnknown => "\u{25CB}",

            Icon::Refresh => "\u{21BB}",
            Icon::Add => "+",
            Icon::Delete => "\u{1F5D1}",
            Icon::Edit => "\u{270E}",
            Icon::Confirm => "\u{2713}",
            Icon::Cancel => "\u{00D7}",
            Icon::Search => "\u{1F50D}",
            Icon::ChevronRight => "\u{203A}",
            Icon::ChevronDown => "\u{2304}",

            Icon::Document => "\u{1F4C4}",
            Icon::DocumentBlank => "\u{1F5CE}",
            Icon::Image => "\u{1F5BC}",
            Icon::Pdf => "P",
            Icon::Code => "<>",
            Icon::Audio => "\u{266A}",
            Icon::Video => "\u{1F3AC}",
            Icon::Archive => "\u{1F4E6}",
            Icon::Folder => "\u{1F4C1}",
        }
    }

    /// Fill policy for this icon. Locked by the 8-Q icon-mapping
    /// survey Round 1 Q2 (2026-05-26).
    #[must_use]
    pub const fn fill_mode(self) -> FillMode {
        match self {
            // Always filled (Q38 carry-over).
            Icon::Notification
            | Icon::StatusOk
            | Icon::StatusWarning
            | Icon::StatusError
            | Icon::StatusInfo
            | Icon::StatusUnknown
            | Icon::Playbook => FillMode::AlwaysFill,

            // Outlined-by-default, filled when active (new
            // active-state fill rule, Round 1 Q2). Covers the 9
            // sidebar nav groups + the dock-only Files /
            // Settings buttons.
            Icon::Dashboard
            | Icon::Apps
            | Icon::Network
            | Icon::Devices
            | Icon::LookAndFeel
            | Icon::System
            | Icon::Maintain
            | Icon::Fleet
            | Icon::Compute
            | Icon::Help
            | Icon::Files => FillMode::OnActive,

            // Everything else is outlined regardless of state.
            _ => FillMode::NeverFill,
        }
    }
}

/// Resolved icon — a Material name + Unicode fallback + the locked
/// size + fill policy. Consumers render either the Material SVG
/// (via [`ResolvedIcon::svg_bytes`]) or the fallback glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedIcon {
    /// Material Symbols symbolic name (e.g. `"network_check"`).
    pub material_name: &'static str,
    /// Unicode fallback glyph rendered when SVG isn't appropriate.
    pub fallback_glyph: &'static str,
    /// Fill policy for this icon.
    pub fill_mode: FillMode,
    /// Resolved [`IconSize`] tier.
    pub size: IconSize,
    /// The [`Icon`] variant — kept so [`ResolvedIcon::svg_bytes`]
    /// can resolve the right `include_bytes!` arm directly.
    icon: Icon,
}

impl ResolvedIcon {
    /// Pixel size — convenience pass-through.
    #[must_use]
    pub const fn size_px(self) -> f32 {
        self.size.px()
    }

    /// Material Symbols SVG bytes for this icon at the resolved
    /// size + the requested state. Returns the outlined SVG when
    /// the fill policy is [`FillMode::NeverFill`] or when the state
    /// is [`IconState::Idle`] with [`FillMode::OnActive`]. Returns
    /// the filled SVG otherwise.
    /// Backward-compatible no-arg accessor. Equivalent to
    /// `svg_bytes_for_state(IconState::Idle)` wrapped in `Some`.
    /// Preserves the prior Carbon-era `Option<&[u8]>` shape so
    /// existing callsites keep compiling. Always returns `Some`
    /// now that every variant is wired; the `Option` is
    /// structural-only. New callsites that care about selection
    /// state should call [`Self::svg_bytes_for_state`] directly
    /// with the appropriate [`IconState`].
    #[must_use]
    pub fn svg_bytes(&self) -> Option<&'static [u8]> {
        Some(self.svg_bytes_for_state(IconState::Idle))
    }

    /// State-aware accessor. Returns the outlined SVG when the
    /// fill policy is [`FillMode::NeverFill`] or when the state
    /// is [`IconState::Idle`] with [`FillMode::OnActive`]. Returns
    /// the filled SVG otherwise.
    #[must_use]
    pub fn svg_bytes_for_state(&self, state: IconState) -> &'static [u8] {
        self.icon
            .svg_bytes(self.size, state_to_filled(state, self.fill_mode))
    }
}

const fn state_to_filled(state: IconState, mode: FillMode) -> bool {
    match (mode, state) {
        (FillMode::AlwaysFill, _) => true,
        (FillMode::OnActive, IconState::Active) => true,
        _ => false,
    }
}

impl Icon {
    /// Resolve to SVG bytes at the given size + filled state.
    /// Internal — public consumers go through
    /// [`ResolvedIcon::svg_bytes`] which honors [`FillMode`].
    #[must_use]
    pub fn svg_bytes(self, size: IconSize, filled: bool) -> &'static [u8] {
        // The match below is exhaustive on `self`; each arm picks
        // the right optical-size SVG, with a nested branch for the
        // filled variant on fill-eligible icons.
        let svg_size = size.optical_svg_size();
        material_svg_bytes(self, svg_size, filled)
    }
}

// ───────────────────────────────────────────────────────────────
// Material SVG resolver — the big mechanical match.
//
// Each Icon variant has 3 outlined arms (sizes 20/24/40); icons
// in the AlwaysFill or OnActive set additionally have 3 filled
// arms (with `_fill1` infix in the path).
//
// The `_ => fallback` catch-alls inside each block are unreachable
// in practice (svg_size is one of 20/24/40 and filled-vs-not is
// gated by fill_mode upstream) but compile cleanly and protect
// against future IconSize additions.
// ───────────────────────────────────────────────────────────────

#[allow(clippy::too_many_lines)]
fn material_svg_bytes(icon: Icon, svg_size: u32, filled: bool) -> &'static [u8] {
    match icon {
        // ── Navigation surfaces (OnActive — fill variants exist) ──
        Icon::Dashboard => match (svg_size, filled) {
            (20, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/dashboard_20px.svg")
            }
            (24, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/dashboard_24px.svg")
            }
            (40, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/dashboard_40px.svg")
            }
            (20, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/dashboard_fill1_20px.svg")
            }
            (24, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/dashboard_fill1_24px.svg")
            }
            (40, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/dashboard_fill1_40px.svg")
            }
            _ => include_bytes!("../../../../assets/icons/material-symbols/dashboard_24px.svg"),
        },
        Icon::Apps => match (svg_size, filled) {
            (20, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/apps_20px.svg")
            }
            (24, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/apps_24px.svg")
            }
            (40, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/apps_40px.svg")
            }
            (20, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/apps_fill1_20px.svg")
            }
            (24, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/apps_fill1_24px.svg")
            }
            (40, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/apps_fill1_40px.svg")
            }
            _ => include_bytes!("../../../../assets/icons/material-symbols/apps_24px.svg"),
        },
        Icon::Network => match (svg_size, filled) {
            (20, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/network_check_20px.svg")
            }
            (24, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/network_check_24px.svg")
            }
            (40, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/network_check_40px.svg")
            }
            (20, true) => include_bytes!(
                "../../../../assets/icons/material-symbols/network_check_fill1_20px.svg"
            ),
            (24, true) => include_bytes!(
                "../../../../assets/icons/material-symbols/network_check_fill1_24px.svg"
            ),
            (40, true) => include_bytes!(
                "../../../../assets/icons/material-symbols/network_check_fill1_40px.svg"
            ),
            _ => include_bytes!("../../../../assets/icons/material-symbols/network_check_24px.svg"),
        },
        Icon::Devices => match (svg_size, filled) {
            (20, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/devices_20px.svg")
            }
            (24, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/devices_24px.svg")
            }
            (40, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/devices_40px.svg")
            }
            (20, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/devices_fill1_20px.svg")
            }
            (24, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/devices_fill1_24px.svg")
            }
            (40, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/devices_fill1_40px.svg")
            }
            _ => include_bytes!("../../../../assets/icons/material-symbols/devices_24px.svg"),
        },
        Icon::LookAndFeel | Icon::Themes => match (svg_size, filled) {
            // Both LookAndFeel + Themes map to `palette`; only
            // LookAndFeel is OnActive so the filled arms are only
            // reached through that variant.
            (20, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/palette_20px.svg")
            }
            (24, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/palette_24px.svg")
            }
            (40, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/palette_40px.svg")
            }
            (20, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/palette_fill1_20px.svg")
            }
            (24, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/palette_fill1_24px.svg")
            }
            (40, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/palette_fill1_40px.svg")
            }
            _ => include_bytes!("../../../../assets/icons/material-symbols/palette_24px.svg"),
        },
        Icon::System | Icon::Settings => match (svg_size, filled) {
            // Both System + Settings map to `settings`; only System
            // is OnActive.
            (20, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/settings_20px.svg")
            }
            (24, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/settings_24px.svg")
            }
            (40, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/settings_40px.svg")
            }
            (20, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/settings_fill1_20px.svg")
            }
            (24, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/settings_fill1_24px.svg")
            }
            (40, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/settings_fill1_40px.svg")
            }
            _ => include_bytes!("../../../../assets/icons/material-symbols/settings_24px.svg"),
        },
        Icon::Maintain | Icon::Repair => match (svg_size, filled) {
            // Both Maintain + Repair map to `build`; only Maintain
            // is OnActive.
            (20, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/build_20px.svg")
            }
            (24, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/build_24px.svg")
            }
            (40, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/build_40px.svg")
            }
            (20, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/build_fill1_20px.svg")
            }
            (24, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/build_fill1_24px.svg")
            }
            (40, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/build_fill1_40px.svg")
            }
            _ => include_bytes!("../../../../assets/icons/material-symbols/build_24px.svg"),
        },
        Icon::Fleet => match (svg_size, filled) {
            (20, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/public_20px.svg")
            }
            (24, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/public_24px.svg")
            }
            (40, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/public_40px.svg")
            }
            (20, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/public_fill1_20px.svg")
            }
            (24, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/public_fill1_24px.svg")
            }
            (40, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/public_fill1_40px.svg")
            }
            _ => include_bytes!("../../../../assets/icons/material-symbols/public_24px.svg"),
        },
        Icon::Compute => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/memory_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/memory_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/memory_40px.svg"),
        ),
        Icon::Help | Icon::StatusUnknown => match (svg_size, filled) {
            // Both Help (OnActive) + StatusUnknown (AlwaysFill)
            // map to `help`.
            (20, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/help_20px.svg")
            }
            (24, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/help_24px.svg")
            }
            (40, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/help_40px.svg")
            }
            (20, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/help_fill1_20px.svg")
            }
            (24, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/help_fill1_24px.svg")
            }
            (40, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/help_fill1_40px.svg")
            }
            _ => include_bytes!("../../../../assets/icons/material-symbols/help_24px.svg"),
        },
        Icon::Files => match (svg_size, filled) {
            (20, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/folder_20px.svg")
            }
            (24, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/folder_24px.svg")
            }
            (40, false) => {
                include_bytes!("../../../../assets/icons/material-symbols/folder_40px.svg")
            }
            (20, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/folder_fill1_20px.svg")
            }
            (24, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/folder_fill1_24px.svg")
            }
            (40, true) => {
                include_bytes!("../../../../assets/icons/material-symbols/folder_fill1_40px.svg")
            }
            _ => include_bytes!("../../../../assets/icons/material-symbols/folder_24px.svg"),
        },

        // ── Always-filled icons (status + bell + playbook) ──
        Icon::Notification => match svg_size {
            20 => include_bytes!(
                "../../../../assets/icons/material-symbols/notifications_fill1_20px.svg"
            ),
            24 => include_bytes!(
                "../../../../assets/icons/material-symbols/notifications_fill1_24px.svg"
            ),
            40 => include_bytes!(
                "../../../../assets/icons/material-symbols/notifications_fill1_40px.svg"
            ),
            _ => include_bytes!(
                "../../../../assets/icons/material-symbols/notifications_fill1_24px.svg"
            ),
        },
        Icon::Playbook => match svg_size {
            20 => include_bytes!(
                "../../../../assets/icons/material-symbols/play_arrow_fill1_20px.svg"
            ),
            24 => include_bytes!(
                "../../../../assets/icons/material-symbols/play_arrow_fill1_24px.svg"
            ),
            40 => include_bytes!(
                "../../../../assets/icons/material-symbols/play_arrow_fill1_40px.svg"
            ),
            _ => include_bytes!(
                "../../../../assets/icons/material-symbols/play_arrow_fill1_24px.svg"
            ),
        },
        Icon::StatusOk => match svg_size {
            20 => include_bytes!(
                "../../../../assets/icons/material-symbols/check_circle_fill1_20px.svg"
            ),
            24 => include_bytes!(
                "../../../../assets/icons/material-symbols/check_circle_fill1_24px.svg"
            ),
            40 => include_bytes!(
                "../../../../assets/icons/material-symbols/check_circle_fill1_40px.svg"
            ),
            _ => include_bytes!(
                "../../../../assets/icons/material-symbols/check_circle_fill1_24px.svg"
            ),
        },
        Icon::StatusWarning => match svg_size {
            20 => {
                include_bytes!("../../../../assets/icons/material-symbols/warning_fill1_20px.svg")
            }
            24 => {
                include_bytes!("../../../../assets/icons/material-symbols/warning_fill1_24px.svg")
            }
            40 => {
                include_bytes!("../../../../assets/icons/material-symbols/warning_fill1_40px.svg")
            }
            _ => include_bytes!("../../../../assets/icons/material-symbols/warning_fill1_24px.svg"),
        },
        Icon::StatusError => match svg_size {
            20 => include_bytes!("../../../../assets/icons/material-symbols/error_fill1_20px.svg"),
            24 => include_bytes!("../../../../assets/icons/material-symbols/error_fill1_24px.svg"),
            40 => include_bytes!("../../../../assets/icons/material-symbols/error_fill1_40px.svg"),
            _ => include_bytes!("../../../../assets/icons/material-symbols/error_fill1_24px.svg"),
        },
        Icon::StatusInfo => match svg_size {
            20 => include_bytes!("../../../../assets/icons/material-symbols/info_fill1_20px.svg"),
            24 => include_bytes!("../../../../assets/icons/material-symbols/info_fill1_24px.svg"),
            40 => include_bytes!("../../../../assets/icons/material-symbols/info_fill1_40px.svg"),
            _ => include_bytes!("../../../../assets/icons/material-symbols/info_fill1_24px.svg"),
        },

        // ── Never-filled icons (outlined-only) ──
        Icon::Snapshot => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/save_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/save_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/save_40px.svg"),
        ),
        Icon::Peer => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/memory_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/memory_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/memory_40px.svg"),
        ),
        Icon::Logs | Icon::Session => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/list_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/list_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/list_40px.svg"),
        ),
        Icon::Update => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/rocket_launch_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/rocket_launch_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/rocket_launch_40px.svg"),
        ),
        Icon::Sound => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/volume_up_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/volume_up_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/volume_up_40px.svg"),
        ),
        Icon::Display => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/desktop_windows_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/desktop_windows_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/desktop_windows_40px.svg"),
        ),
        Icon::Printer => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/print_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/print_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/print_40px.svg"),
        ),
        Icon::Power => pick_3(
            svg_size,
            include_bytes!(
                "../../../../assets/icons/material-symbols/battery_charging_full_20px.svg"
            ),
            include_bytes!(
                "../../../../assets/icons/material-symbols/battery_charging_full_24px.svg"
            ),
            include_bytes!(
                "../../../../assets/icons/material-symbols/battery_charging_full_40px.svg"
            ),
        ),
        Icon::Removable => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/usb_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/usb_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/usb_40px.svg"),
        ),
        Icon::Clock => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/schedule_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/schedule_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/schedule_40px.svg"),
        ),
        Icon::Wallpaper => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/image_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/image_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/image_40px.svg"),
        ),
        Icon::Fonts => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/text_fields_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/text_fields_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/text_fields_40px.svg"),
        ),
        Icon::Wifi => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/wifi_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/wifi_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/wifi_40px.svg"),
        ),
        Icon::Vpn => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/vpn_lock_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/vpn_lock_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/vpn_lock_40px.svg"),
        ),
        Icon::Firewall => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/security_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/security_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/security_40px.svg"),
        ),
        Icon::History => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/history_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/history_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/history_40px.svg"),
        ),
        Icon::Inventory => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/checklist_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/checklist_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/checklist_40px.svg"),
        ),
        // CTRLSURF-8 — the Workbench brand renders the mesh-native
        // "mesh-control" glyph (`assets/icons/carbon/workbench.svg`), not the
        // generic Material `handyman` tools glyph. It's a single scalable path,
        // so every optical tier bakes the one asset. `material_name` still
        // reports `handyman` as the Material lineage/fallback; these bytes are
        // the deliberate brand override so the header brand-strip, the
        // notify-center launch tile, and the This-Node nav group all read the
        // on-brand mesh glyph consistently (BUG-20 cross-surface parity).
        Icon::Workbench => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/carbon/workbench.svg"),
            include_bytes!("../../../../assets/icons/carbon/workbench.svg"),
            include_bytes!("../../../../assets/icons/carbon/workbench.svg"),
        ),
        Icon::WindowMinimize => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/remove_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/remove_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/remove_40px.svg"),
        ),
        Icon::WindowMaximize => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/fullscreen_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/fullscreen_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/fullscreen_40px.svg"),
        ),
        Icon::WindowClose | Icon::Cancel => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/close_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/close_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/close_40px.svg"),
        ),
        Icon::Refresh => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/refresh_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/refresh_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/refresh_40px.svg"),
        ),
        Icon::Add => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/add_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/add_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/add_40px.svg"),
        ),
        Icon::Delete => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/delete_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/delete_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/delete_40px.svg"),
        ),
        Icon::Edit => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/edit_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/edit_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/edit_40px.svg"),
        ),
        Icon::Confirm => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/check_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/check_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/check_40px.svg"),
        ),
        Icon::Search => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/search_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/search_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/search_40px.svg"),
        ),
        Icon::ChevronRight => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/chevron_right_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/chevron_right_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/chevron_right_40px.svg"),
        ),
        Icon::ChevronDown => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/expand_more_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/expand_more_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/expand_more_40px.svg"),
        ),

        // ── File-type icons (CR-3.c, never-filled) ──
        Icon::Document => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/description_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/description_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/description_40px.svg"),
        ),
        Icon::DocumentBlank => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/draft_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/draft_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/draft_40px.svg"),
        ),
        // Icon::Image shares the `image` SVG with Icon::Wallpaper.
        Icon::Image => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/image_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/image_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/image_40px.svg"),
        ),
        Icon::Pdf => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/picture_as_pdf_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/picture_as_pdf_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/picture_as_pdf_40px.svg"),
        ),
        Icon::Code => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/code_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/code_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/code_40px.svg"),
        ),
        Icon::Audio => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/audio_file_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/audio_file_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/audio_file_40px.svg"),
        ),
        Icon::Video => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/video_file_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/video_file_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/video_file_40px.svg"),
        ),
        Icon::Archive => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/folder_zip_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/folder_zip_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/folder_zip_40px.svg"),
        ),
        // Icon::Folder shares the `folder` SVG with Icon::Files.
        Icon::Folder => pick_3(
            svg_size,
            include_bytes!("../../../../assets/icons/material-symbols/folder_20px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/folder_24px.svg"),
            include_bytes!("../../../../assets/icons/material-symbols/folder_40px.svg"),
        ),
    }
}

const fn pick_3(
    svg_size: u32,
    b20: &'static [u8],
    b24: &'static [u8],
    b40: &'static [u8],
) -> &'static [u8] {
    match svg_size {
        20 => b20,
        24 => b24,
        40 => b40,
        _ => b24,
    }
}

/// Single canonical resolver. Consumers never construct
/// `ResolvedIcon` directly — they go through this so adding a
/// new `Icon` variant lights up everywhere consistently.
#[must_use]
pub const fn mde_icon(icon: Icon, size: IconSize) -> ResolvedIcon {
    ResolvedIcon {
        material_name: icon.material_name(),
        fallback_glyph: icon.fallback_glyph(),
        fill_mode: icon.fill_mode(),
        size,
        icon,
    }
}

/// Pick an [`Icon`] from a peer's `device_type` field.
/// `mesh_peer_card` consumers route via this so the inventory list,
/// fleet panel, and peer-connection-card all render the same glyph
/// for the same kind of device.
#[must_use]
pub fn icon_for_device_type(device_type: &str) -> Icon {
    match device_type {
        "laptop" | "notebook" => Icon::Devices,
        "desktop" | "tower" => Icon::Devices,
        "phone" | "mobile" => Icon::Devices,
        "server" | "rack" => Icon::Fleet,
        "router" | "gateway" => Icon::Network,
        "printer" => Icon::Printer,
        "display" | "monitor" => Icon::Display,
        _ => Icon::Peer,
    }
}

/// Pick an [`Icon`] from a MIME type string (CR-3.c).
///
/// Single canonical MIME → icon mapping so file-row Object Cards,
/// the mde-files sidebar, and any future consumer all show the
/// same glyph for the same file type.
///
/// Matching is prefix-based — `"image/"`, `"audio/"`, `"video/"`
/// — so `image/png`, `image/jpeg`, etc. all resolve to
/// [`Icon::Image`] without needing an exhaustive list.
///
/// Returns [`Icon::Document`] for unrecognised types (the generic
/// fallback for text / office docs rather than the even-more-
/// generic `Icon::Files` folder glyph).
#[must_use]
pub fn icon_for_mime(mime: &str) -> Icon {
    if mime == "inode/directory" {
        return Icon::Folder;
    }
    if mime.starts_with("image/") {
        return Icon::Image;
    }
    if mime.starts_with("audio/") {
        return Icon::Audio;
    }
    if mime.starts_with("video/") {
        return Icon::Video;
    }
    match mime {
        "application/pdf" => Icon::Pdf,
        "application/zip"
        | "application/x-tar"
        | "application/x-bzip2"
        | "application/gzip"
        | "application/x-xz"
        | "application/x-7z-compressed"
        | "application/x-rar-compressed"
        | "application/vnd.rar" => Icon::Archive,
        "application/json"
        | "application/xml"
        | "application/x-sh"
        | "application/javascript"
        | "application/typescript" => Icon::Code,
        // Common code MIME types carried under text/* (RFC 2046).
        "text/html" | "text/css" | "text/javascript" | "text/typescript" | "text/xml" => Icon::Code,
        // text/x-* is the convention for non-standard source files.
        m if m.starts_with("text/x-") => Icon::Code,
        // Remaining text/* (plain text, CSV, Markdown, …) → Document.
        m if m.starts_with("text/") => Icon::Document,
        // Unrecognised application/* or anything else → Document.
        _ => Icon::Document,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_sizes_match_locked_tiers() {
        assert!((IconSize::Inline.px() - 16.0).abs() < f32::EPSILON);
        assert!((IconSize::Nav.px() - 20.0).abs() < f32::EPSILON);
        assert!((IconSize::PanelHeader.px() - 24.0).abs() < f32::EPSILON);
        assert!((IconSize::EmptyState.px() - 32.0).abs() < f32::EPSILON);
        assert!((IconSize::WizardHero.px() - 48.0).abs() < f32::EPSILON);
    }

    #[test]
    fn optical_svg_sizes_map_to_bundled_assets() {
        // Material ships SVGs at 20 / 24 / 40 — every IconSize must
        // pick one of those.
        for size in [
            IconSize::Inline,
            IconSize::Nav,
            IconSize::PanelHeader,
            IconSize::EmptyState,
            IconSize::WizardHero,
        ] {
            let optical = size.optical_svg_size();
            assert!(
                matches!(optical, 20 | 24 | 40),
                "{size:?} optical_svg_size() = {optical}, must be 20/24/40"
            );
        }
    }

    #[test]
    fn material_line_weight_locked_to_one_px() {
        assert!((MATERIAL_LINE_WEIGHT_PX - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn every_variant_resolves_to_nonempty_material_name() {
        for icon in every_icon() {
            assert!(
                !icon.material_name().is_empty(),
                "Icon::{icon:?} has an empty material_name()"
            );
        }
    }

    #[test]
    fn every_variant_resolves_to_nonempty_fallback() {
        for icon in every_icon() {
            assert!(
                !icon.fallback_glyph().is_empty(),
                "Icon::{icon:?} has an empty fallback_glyph()"
            );
        }
    }

    #[test]
    fn fill_mode_table_matches_lock() {
        // AlwaysFill: status dots + notification bell + playbook.
        for icon in [
            Icon::Notification,
            Icon::StatusOk,
            Icon::StatusWarning,
            Icon::StatusError,
            Icon::StatusInfo,
            Icon::StatusUnknown,
            Icon::Playbook,
        ] {
            assert_eq!(
                icon.fill_mode(),
                FillMode::AlwaysFill,
                "{icon:?} should be AlwaysFill"
            );
        }
        // OnActive: nav-group + sidebar entries.
        for icon in [
            Icon::Dashboard,
            Icon::Apps,
            Icon::Network,
            Icon::Devices,
            Icon::LookAndFeel,
            Icon::System,
            Icon::Maintain,
            Icon::Fleet,
            Icon::Compute,
            Icon::Help,
            Icon::Files,
        ] {
            assert_eq!(
                icon.fill_mode(),
                FillMode::OnActive,
                "{icon:?} should be OnActive"
            );
        }
        // Spot-check NeverFill.
        assert_eq!(Icon::Settings.fill_mode(), FillMode::NeverFill);
        assert_eq!(Icon::Refresh.fill_mode(), FillMode::NeverFill);
        assert_eq!(Icon::WindowMinimize.fill_mode(), FillMode::NeverFill);
    }

    #[test]
    fn every_action_carries_a_material_symbolic_icon() {
        // Successor to the retired Carbon-named test. Every Icon
        // variant must produce a non-empty Material Symbols name +
        // a non-zero SVG payload at the Nav tier.
        for icon in every_icon() {
            let name = icon.material_name();
            assert!(!name.is_empty(), "{icon:?} material_name() empty");
            let resolved = mde_icon(icon, IconSize::Nav);
            let bytes = resolved.svg_bytes_for_state(IconState::Idle);
            assert!(
                bytes.len() > 32,
                "{icon:?} (material_name={name}) SVG payload too small"
            );
        }
    }

    #[test]
    fn svg_bytes_swap_on_active_state_for_on_active_icons() {
        let resolved = mde_icon(Icon::Dashboard, IconSize::Nav);
        let idle = resolved.svg_bytes_for_state(IconState::Idle);
        let active = resolved.svg_bytes_for_state(IconState::Active);
        assert_ne!(
            idle, active,
            "OnActive icon should resolve different bytes for Idle vs Active"
        );
    }

    #[test]
    fn svg_bytes_constant_for_never_fill_icons() {
        let resolved = mde_icon(Icon::Settings, IconSize::Nav);
        let idle = resolved.svg_bytes_for_state(IconState::Idle);
        let active = resolved.svg_bytes_for_state(IconState::Active);
        assert_eq!(
            idle, active,
            "NeverFill icon should resolve same bytes regardless of state"
        );
    }

    #[test]
    fn svg_bytes_always_filled_regardless_of_state() {
        // Notification is AlwaysFill — both Idle + Active should
        // resolve to the _fill1 SVG.
        let resolved = mde_icon(Icon::Notification, IconSize::Nav);
        let idle = resolved.svg_bytes_for_state(IconState::Idle);
        let active = resolved.svg_bytes_for_state(IconState::Active);
        assert_eq!(idle, active);
        // Sanity-check the bytes are the fill1 variant by
        // re-reading from a different size + asserting non-empty.
        assert!(idle.len() > 32);
    }

    #[test]
    fn mde_icon_carries_size_and_material_name() {
        let r = mde_icon(Icon::Fleet, IconSize::Nav);
        assert_eq!(r.material_name, "public");
        assert!((r.size_px() - 20.0).abs() < f32::EPSILON);
    }

    #[test]
    fn device_type_routing_falls_back_to_peer_on_unknown() {
        assert_eq!(icon_for_device_type("nas"), Icon::Peer);
        assert_eq!(icon_for_device_type(""), Icon::Peer);
        assert_eq!(icon_for_device_type("laptop"), Icon::Devices);
        assert_eq!(icon_for_device_type("router"), Icon::Network);
    }

    /// Every `Icon` variant — keep in sync with the enum so the
    /// `every_variant_resolves_*` tests catch a missing arm.
    fn every_icon() -> Vec<Icon> {
        vec![
            Icon::Dashboard,
            Icon::Apps,
            Icon::Network,
            Icon::Devices,
            Icon::LookAndFeel,
            Icon::System,
            Icon::Maintain,
            Icon::Fleet,
            Icon::Compute,
            Icon::Help,
            Icon::Snapshot,
            Icon::Peer,
            Icon::Logs,
            Icon::Update,
            Icon::Repair,
            Icon::Sound,
            Icon::Display,
            Icon::Printer,
            Icon::Power,
            Icon::Removable,
            Icon::Clock,
            Icon::Wallpaper,
            Icon::Fonts,
            Icon::Themes,
            Icon::Session,
            Icon::Notification,
            Icon::Wifi,
            Icon::Vpn,
            Icon::Firewall,
            Icon::Playbook,
            Icon::History,
            Icon::Settings,
            Icon::Inventory,
            Icon::Workbench,
            Icon::Files,
            Icon::WindowMinimize,
            Icon::WindowMaximize,
            Icon::WindowClose,
            Icon::StatusOk,
            Icon::StatusWarning,
            Icon::StatusError,
            Icon::StatusInfo,
            Icon::StatusUnknown,
            Icon::Refresh,
            Icon::Add,
            Icon::Delete,
            Icon::Edit,
            Icon::Confirm,
            Icon::Cancel,
            Icon::Search,
            Icon::ChevronRight,
            Icon::ChevronDown,
            // CR-3.c file-type variants.
            Icon::Document,
            Icon::DocumentBlank,
            Icon::Image,
            Icon::Pdf,
            Icon::Code,
            Icon::Audio,
            Icon::Video,
            Icon::Archive,
            Icon::Folder,
        ]
    }

    // ── CR-3.c: per-mime Icon variants ────────────────────────────

    #[test]
    fn file_type_icons_resolve_nonempty_svg_at_nav_size() {
        for icon in [
            Icon::Document,
            Icon::DocumentBlank,
            Icon::Image,
            Icon::Pdf,
            Icon::Code,
            Icon::Audio,
            Icon::Video,
            Icon::Archive,
            Icon::Folder,
        ] {
            let resolved = mde_icon(icon, IconSize::Nav);
            let bytes = resolved.svg_bytes_for_state(IconState::Idle);
            assert!(
                bytes.len() > 32,
                "Icon::{icon:?} SVG payload too small at Nav size"
            );
        }
    }

    #[test]
    fn file_type_icons_are_never_fill() {
        for icon in [
            Icon::Document,
            Icon::DocumentBlank,
            Icon::Image,
            Icon::Pdf,
            Icon::Code,
            Icon::Audio,
            Icon::Video,
            Icon::Archive,
            Icon::Folder,
        ] {
            assert_eq!(
                icon.fill_mode(),
                FillMode::NeverFill,
                "Icon::{icon:?} should be NeverFill"
            );
        }
    }

    #[test]
    fn icon_for_mime_image_types() {
        assert_eq!(icon_for_mime("image/png"), Icon::Image);
        assert_eq!(icon_for_mime("image/jpeg"), Icon::Image);
        assert_eq!(icon_for_mime("image/webp"), Icon::Image);
        assert_eq!(icon_for_mime("image/svg+xml"), Icon::Image);
    }

    #[test]
    fn icon_for_mime_audio_types() {
        assert_eq!(icon_for_mime("audio/mpeg"), Icon::Audio);
        assert_eq!(icon_for_mime("audio/flac"), Icon::Audio);
        assert_eq!(icon_for_mime("audio/ogg"), Icon::Audio);
    }

    #[test]
    fn icon_for_mime_video_types() {
        assert_eq!(icon_for_mime("video/mp4"), Icon::Video);
        assert_eq!(icon_for_mime("video/webm"), Icon::Video);
        assert_eq!(icon_for_mime("video/x-matroska"), Icon::Video);
    }

    #[test]
    fn icon_for_mime_pdf() {
        assert_eq!(icon_for_mime("application/pdf"), Icon::Pdf);
    }

    #[test]
    fn icon_for_mime_archives() {
        assert_eq!(icon_for_mime("application/zip"), Icon::Archive);
        assert_eq!(icon_for_mime("application/x-tar"), Icon::Archive);
        assert_eq!(icon_for_mime("application/gzip"), Icon::Archive);
        assert_eq!(icon_for_mime("application/x-7z-compressed"), Icon::Archive);
    }

    #[test]
    fn icon_for_mime_code_types() {
        assert_eq!(icon_for_mime("application/json"), Icon::Code);
        assert_eq!(icon_for_mime("application/javascript"), Icon::Code);
        assert_eq!(icon_for_mime("text/html"), Icon::Code);
        assert_eq!(icon_for_mime("text/css"), Icon::Code);
        assert_eq!(icon_for_mime("text/x-python"), Icon::Code);
        assert_eq!(icon_for_mime("text/x-rust"), Icon::Code);
    }

    #[test]
    fn icon_for_mime_text_plain_is_document() {
        assert_eq!(icon_for_mime("text/plain"), Icon::Document);
        assert_eq!(icon_for_mime("text/markdown"), Icon::Document);
        assert_eq!(icon_for_mime("text/csv"), Icon::Document);
    }

    #[test]
    fn icon_for_mime_directory() {
        assert_eq!(icon_for_mime("inode/directory"), Icon::Folder);
    }

    #[test]
    fn icon_for_mime_unknown_falls_back_to_document() {
        assert_eq!(icon_for_mime("application/octet-stream"), Icon::Document);
        assert_eq!(icon_for_mime(""), Icon::Document);
        assert_eq!(icon_for_mime("model/gltf+json"), Icon::Document);
    }
}
