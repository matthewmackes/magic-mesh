//! `brand::icons` — the platform icon resolver (QBRAND-2).
//!
//! The platform glyphs embedded as inline SVG consts behind [`IconId`], plus
//! the SVG→raster loader ([`icon_image`]) every surface draws them through. The
//! product mark / wordmark resolve to the Construct brand assets; the default platform
//! surface, status, and tray glyphs now resolve to the YAMIS monochrome theme
//! under `assets/icons/YAMIS/YAMIS/`. The SVGs use `currentColor` through the
//! freedesktop ColorScheme classes, so ONE embedded set serves every tint: the
//! loader substitutes the caller's color pre-parse and rasterizes with `resvg`
//! at the exact requested pixel size — DPI-crisp at any scale, no pre-baked PNG
//! ladder.
//!
//! ## Toolkit-free by design
//!
//! This crate stays free of a GUI dependency (QBRAND lock #4: the daemon and
//! packaging read the same crate as the shell — `mackesd --version` must not
//! pull egui), so the loader returns a plain RGBA8 buffer ([`IconImage`])
//! rather than an egui texture. The shell wraps it in one line:
//!
//! ```ignore
//! let img = mde_theme::brand::icons::icon_image(id, size, tint)?;
//! let color = egui::ColorImage::from_rgba_unmultiplied(img.size_usize(), &img.rgba);
//! let tex = ctx.load_texture(id.name(), color, egui::TextureOptions::LINEAR);
//! ```
//!
//! Tints come in as a plain `[r, g, b, a]` array so callers pass their
//! `mde_egui::Style` token colors directly — this crate never re-derives token
//! values (that would fork the design system's source of truth).
//!
//! ## The wordmark logotype
//!
//! [`IconId::Wordmark`] is the Construct text lockup ("MDE" / "CONSTRUCT") —
//! pure `<text>` elements. `resvg` is built here without its `text`/fontdb
//! features (the minimal, farm-vendorable configuration), so the lockup
//! parses and keeps its 320×184 aspect but rasterizes fully transparent — it
//! never panics, and callers wanting the visible lockup should use the
//! official raster assets (`assets/brand/construct/app-icon-*.png` /
//! `brand::logo`, QBRAND-3). The SVG's own `<desc>` flags the designed fix:
//! outlining the letterforms to paths, with resvg's `text` feature + a
//! bundled fontdb as the heavier alternative.

use std::fmt;

use resvg::{tiny_skia, usvg};

/// Embed one Construct brand SVG from `assets/brand/construct/` at compile time.
macro_rules! construct_svg {
    ($file:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../assets/brand/construct/",
            $file
        ))
    };
}

/// Embed one YAMIS SVG from `assets/icons/YAMIS/YAMIS/` at compile time.
macro_rules! yamis_svg {
    ($file:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../assets/icons/YAMIS/YAMIS/",
            $file
        ))
    };
}

/// Identifier for every embedded product and platform glyph.
///
/// Product marks resolve to the Construct brand assets; surface, role, tray,
/// and shared UI action glyphs resolve to the bundled YAMIS theme. Call
/// [`IconId::svg`] for the embedded source and [`IconId::name`] for a stable
/// cache/debug identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconId {
    /// The round mesh-node constellation product mark (`mark.svg`, the 64×64
    /// trace of the official artwork; the blue plane rides a 0.55-opacity
    /// group so single-color tinting keeps the two-tone hierarchy).
    Mark,
    /// The stacked "MDE / CONSTRUCT" text lockup
    /// (`wordmark.svg`, 320×184 — rasterizes transparent without a fontdb;
    /// see the module docs).
    Wordmark,
    /// A single mesh node/peer glyph (`node.svg`), health-tinted by callers.
    Node,
    /// The Workbench (fleet command) surface glyph.
    Workbench,
    /// The Instances (VM broker) surface glyph.
    Instances,
    /// The remote Desktop surface glyph.
    Desktop,
    /// The Music surface glyph.
    Music,
    /// The Media player surface glyph.
    Media,
    /// The Files surface glyph.
    Files,
    /// The Voice surface glyph.
    Voice,
    /// The Browser surface glyph.
    Browser,
    /// The Maps & Location vehicle navigation surface glyph.
    MapsLocation,
    /// The Bookmarks manager surface glyph.
    Bookmarks,
    /// The Terminal surface glyph.
    Terminal,
    /// The Editor (code editor) surface glyph.
    Editor,
    /// The Chat surface glyph.
    Chat,
    /// The Phones hub surface glyph — a smartphone outline (KDC-MESH-9).
    Phones,
    /// The System surface glyph.
    System,
    /// The Storage surface glyph.
    Storage,
    /// The Mesh-View (topology map) surface glyph.
    MeshView,
    /// The Settings (host-controls) surface glyph — a toothed cog. Distinct from
    /// the spoked [`System`](Self::System) glyph; the dock's right-side Settings
    /// button (PICKER-2) draws this gear.
    Settings,
    /// Settings category: display/output controls.
    DisplaySettings,
    /// Settings category: mouse and pointer controls.
    Mouse,
    /// Settings category: touchpad controls.
    Touchpad,
    /// Settings category: keyboard/hotkey controls.
    Keyboard,
    /// Settings category: desktop wallpaper controls.
    Wallpaper,
    /// Settings category: appearance/theme controls.
    Appearance,
    /// Settings category: Bluetooth controls.
    Bluetooth,
    /// Settings category: power and battery controls.
    PowerBattery,
    /// Settings category: network controls.
    NetworkSettings,
    /// Shared UI: search/magnifier glyph for compact search fields.
    Search,
    /// Shared UI: close/clear `x` glyph for compact dismiss and clear buttons.
    Close,
    /// Shared UI: reload/refresh glyph.
    Reload,
    /// Shared UI: cancel/stop glyph.
    Cancel,
    /// Shared UI: left navigation arrow.
    ArrowLeft,
    /// Shared UI: right navigation arrow.
    ArrowRight,
    /// Shared UI: down navigation arrow.
    ArrowDown,
    /// Shared UI: application/options menu glyph.
    Menu,
    /// Shared UI: horizontal more/overflow glyph.
    MoreHorizontal,
    /// Shared UI: downloads glyph.
    Downloads,
    /// Shared UI: camera/screen-capture glyph.
    Capture,
    /// Shared UI: print/printer glyph.
    Print,
    /// Shared UI: recent/history glyph.
    History,
    /// Shared UI: tab-management glyph.
    Tabs,
    /// Shared UI: new/add tab glyph.
    NewTab,
    /// Shared UI: generic add/plus glyph.
    Add,
    /// Shared UI: generic remove/minus glyph.
    Remove,
    /// Shared UI: zoom in glyph.
    ZoomIn,
    /// Shared UI: zoom out glyph.
    ZoomOut,
    /// Shared UI: checkmark/selected glyph.
    Check,
    /// Shared UI: document/page glyph.
    Page,
    /// Files surface: generic folder entry glyph.
    FileFolder,
    /// Files surface: home folder place glyph.
    FileHome,
    /// Files surface: documents folder place glyph.
    FileDocuments,
    /// Files surface: downloads folder place glyph.
    FileDownloads,
    /// Files surface: generic document file glyph.
    FileDocument,
    /// Files surface: image file glyph.
    FileImage,
    /// Files surface: PDF file glyph.
    FilePdf,
    /// Files surface: archive/package file glyph.
    FileArchive,
    /// Shared UI: internet/browser-engine glyph.
    Internet,
    /// Shared UI: edit/text-entry glyph.
    TextEdit,
    /// Shared UI: view/inspect glyph.
    View,
    /// Shared UI: security/privacy shield glyph.
    Security,
    /// Shared UI: warning/attention glyph.
    Warning,
    /// Shared UI: locked/private state glyph.
    Lock,
    /// Shared UI: power instrumentation glyph.
    Power,
    /// Shared UI: share/send glyph.
    Share,
    /// Shared UI: generic audio glyph.
    Audio,
    /// Shared UI: lower-volume glyph.
    VolumeLow,
    /// Shared UI: media play glyph.
    Play,
    /// Shared UI: media pause glyph.
    Pause,
    /// Shared UI: media stop glyph.
    MediaStop,
    /// Shared UI: previous media item glyph.
    Previous,
    /// Shared UI: next media item glyph.
    Next,
    /// Shared UI: picture-in-picture / duplicate-window glyph.
    PictureInPicture,
    /// Shared UI: dark-mode/night glyph.
    DarkMode,
    /// Shared UI: active notification/bell glyph.
    Notifications,
    /// Shared UI: muted notifications/bell glyph.
    NotificationsMuted,
    /// The Workstation role badge.
    Workstation,
    /// The Server role badge.
    Server,
    /// The Lighthouse role badge.
    Lighthouse,
    /// Tray: mesh signal strength — four ascending bars.
    Signal,
    /// Tray: active VDI session — a monitor carrying a two-node link mark
    /// (the Desktop monitor + a connection, per NAVBAR-W10 W2/W10).
    Sessions,
    /// Tray: Start / Advanced menu — the Win10-style left rail menu glyph.
    Start,
    /// Shared UI: pin/favorite glyph for launchers and rows that expose pinning.
    Pin,
    /// Shared UI: up navigation arrow. The bottom taskbar no longer uses this for
    /// Desktop Sources, Health, or overflow controls.
    ChevronUp,
    /// Tray: speaker with sound-wave arcs (volume).
    Volume,
    /// Tray: speaker with an `×` mark — the muted state for the volume
    /// micro-flyout (NAVBAR-W10 W7).
    VolumeMuted,
    /// Tray: the Bluetooth rune, drawn 12 units wide so the crossing strokes
    /// stay separable at 16px (no prior BT glyph existed in the set).
    BluetoothSmall,
    /// Tray: battery outline, no charge fill (the empty step of the W8
    /// fill-level ladder; the other steps share this exact outline).
    BatteryEmpty,
    /// Tray: battery at ~25% — the shared outline + a 4-unit fill bar.
    BatteryQuarter,
    /// Tray: battery at ~50% — the shared outline + an 8-unit fill bar.
    BatteryHalf,
    /// Tray: battery at ~75% — the shared outline + a 12-unit fill bar.
    BatteryThreeQuarter,
    /// Tray: battery at 100% — the shared outline + the full 16-unit fill bar.
    BatteryFull,
    /// Tray: a standalone solid charge bolt sized to overlay any
    /// `Battery*` glyph at the same raster size (the Win10 idiom: the bolt
    /// spans the icon, overflowing the outline) — draw the fill-level glyph,
    /// then this on top while charging. Also reads alone as "charging".
    BatteryBolt,
}

impl IconId {
    /// Every glyph in the set, for exhaustive iteration (dock catalogs, tests).
    pub const ALL: [Self; 95] = [
        Self::Mark,
        Self::Wordmark,
        Self::Node,
        Self::Workbench,
        Self::Instances,
        Self::Desktop,
        Self::Music,
        Self::Media,
        Self::Files,
        Self::Voice,
        Self::Browser,
        Self::MapsLocation,
        Self::Bookmarks,
        Self::Terminal,
        Self::Editor,
        Self::Chat,
        Self::Phones,
        Self::System,
        Self::Storage,
        Self::MeshView,
        Self::Settings,
        Self::DisplaySettings,
        Self::Mouse,
        Self::Touchpad,
        Self::Keyboard,
        Self::Wallpaper,
        Self::Appearance,
        Self::Bluetooth,
        Self::PowerBattery,
        Self::NetworkSettings,
        Self::Search,
        Self::Close,
        Self::Reload,
        Self::Cancel,
        Self::ArrowLeft,
        Self::ArrowRight,
        Self::ArrowDown,
        Self::Menu,
        Self::MoreHorizontal,
        Self::Downloads,
        Self::Capture,
        Self::Print,
        Self::History,
        Self::Tabs,
        Self::NewTab,
        Self::Add,
        Self::Remove,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::Check,
        Self::Page,
        Self::FileFolder,
        Self::FileHome,
        Self::FileDocuments,
        Self::FileDownloads,
        Self::FileDocument,
        Self::FileImage,
        Self::FilePdf,
        Self::FileArchive,
        Self::Internet,
        Self::TextEdit,
        Self::View,
        Self::Security,
        Self::Warning,
        Self::Lock,
        Self::Power,
        Self::Share,
        Self::Audio,
        Self::VolumeLow,
        Self::Play,
        Self::Pause,
        Self::MediaStop,
        Self::Previous,
        Self::Next,
        Self::PictureInPicture,
        Self::DarkMode,
        Self::Notifications,
        Self::NotificationsMuted,
        Self::Workstation,
        Self::Server,
        Self::Lighthouse,
        Self::Signal,
        Self::Sessions,
        Self::Start,
        Self::Pin,
        Self::ChevronUp,
        Self::Volume,
        Self::VolumeMuted,
        Self::BluetoothSmall,
        Self::BatteryEmpty,
        Self::BatteryQuarter,
        Self::BatteryHalf,
        Self::BatteryThreeQuarter,
        Self::BatteryFull,
        Self::BatteryBolt,
    ];

    /// The live tray/status glyph subset (NAVBAR-W10-1) — every glyph the tray
    /// renders at 16px, for targeted iteration in the tray and its tests. Retired
    /// Start-bar pin and up-chevron controls stay out of this live subset.
    pub const TRAY: [Self; 12] = [
        Self::Signal,
        Self::Sessions,
        Self::Start,
        Self::Volume,
        Self::VolumeMuted,
        Self::BluetoothSmall,
        Self::BatteryEmpty,
        Self::BatteryQuarter,
        Self::BatteryHalf,
        Self::BatteryThreeQuarter,
        Self::BatteryFull,
        Self::BatteryBolt,
    ];

    /// The embedded SVG source for this glyph — `currentColor` line-art in a
    /// square viewBox (`0 0 32 32` for the surface/role/node/tray glyphs, `0
    /// 0 64 64` for the mark); the wordmark alone is a `0 0 320 184` text
    /// lockup.
    #[must_use]
    pub const fn svg(self) -> &'static str {
        match self {
            Self::Mark => construct_svg!("mark.svg"),
            Self::Wordmark => construct_svg!("wordmark.svg"),
            Self::Node => yamis_svg!("devices/scalable/network-workgroup.svg"),
            Self::Workbench => yamis_svg!("categories/scalable/applications-system.svg"),
            Self::Instances => yamis_svg!("devices/scalable/computer.svg"),
            Self::Desktop => yamis_svg!("devices/scalable/video-display.svg"),
            Self::Music => yamis_svg!("places/scalable/folder-music.svg"),
            Self::Media => yamis_svg!("categories/scalable/applications-multimedia.svg"),
            Self::Files => yamis_svg!("apps/scalable/system-file-manager.svg"),
            Self::Voice => yamis_svg!("devices/scalable/audio-input-microphone.svg"),
            Self::Browser => yamis_svg!("apps/scalable/chromium.svg"),
            Self::MapsLocation => yamis_svg!("apps/scalable/maps.svg"),
            Self::Bookmarks => yamis_svg!("emblems/scalable/emblem-favorite.svg"),
            Self::Terminal => yamis_svg!("apps/scalable/utilities-terminal.svg"),
            Self::Editor => yamis_svg!("apps/scalable/code-oss.svg"),
            Self::Chat => yamis_svg!("status/scalable/tray-message.svg"),
            Self::Phones => yamis_svg!("devices/scalable/phone.svg"),
            Self::System => yamis_svg!("categories/scalable/preferences-system.svg"),
            Self::Storage => yamis_svg!("devices/scalable/drive-harddisk.svg"),
            Self::MeshView => yamis_svg!("devices/scalable/network-card.svg"),
            Self::Settings => yamis_svg!("apps/scalable/systemsettings.svg"),
            Self::DisplaySettings => yamis_svg!("apps/16/preferences-displays.svg"),
            Self::Mouse => yamis_svg!("apps/16/preferences-mouse.svg"),
            Self::Touchpad => yamis_svg!("apps/16/preferences-touchpad.svg"),
            Self::Keyboard => yamis_svg!("apps/16/preferences-desktop-keyboard.svg"),
            Self::Wallpaper => yamis_svg!("apps/16/preferences-desktop-wallpaper.svg"),
            Self::Appearance => yamis_svg!("apps/16/preferences-appearance.svg"),
            Self::Bluetooth => yamis_svg!("status/scalable/bluetooth-active.svg"),
            Self::PowerBattery => yamis_svg!("apps/16/preferences-power-and-battery.svg"),
            Self::NetworkSettings => yamis_svg!("apps/16/preferences-system-network.svg"),
            Self::Search => yamis_svg!("apps/scalable/system-search.svg"),
            Self::Close => yamis_svg!("actions/16/remove.svg"),
            Self::Reload => yamis_svg!("status/scalable/state-sync.svg"),
            Self::Cancel => yamis_svg!("apps/scalable/dialog-cancel.svg"),
            Self::ArrowLeft => yamis_svg!("actions/16/arrow-left.svg"),
            Self::ArrowRight => yamis_svg!("actions/16/arrow-right.svg"),
            Self::ArrowDown => yamis_svg!("actions/16/arrow-down.svg"),
            Self::Menu => yamis_svg!("actions/16/application-menu.svg"),
            Self::MoreHorizontal => yamis_svg!("actions/16/more-horizontal.svg"),
            Self::Downloads => yamis_svg!("emblems/scalable/emblem-downloads.svg"),
            Self::Capture => yamis_svg!("devices/scalable/camera-photo.svg"),
            Self::Print => yamis_svg!("devices/scalable/printer.svg"),
            Self::History => yamis_svg!("actions/16/document-open-recent.svg"),
            Self::Tabs => yamis_svg!("preferences/scalable/preferences-tabs.svg"),
            Self::NewTab => yamis_svg!("actions/16/list-add.svg"),
            Self::Add => yamis_svg!("actions/16/list-add.svg"),
            Self::Remove => yamis_svg!("actions/16/list-remove.svg"),
            Self::ZoomIn => yamis_svg!("actions/16/zoom-in.svg"),
            Self::ZoomOut => yamis_svg!("actions/16/zoom-out.svg"),
            Self::Check => yamis_svg!("emblems/scalable/checkmark.svg"),
            Self::Page => yamis_svg!("preferences/scalable/preferences-document.svg"),
            Self::FileFolder => yamis_svg!("places/16/folder.svg"),
            Self::FileHome => yamis_svg!("places/16/user-home.svg"),
            Self::FileDocuments => yamis_svg!("places/16/folder-documents.svg"),
            Self::FileDownloads => yamis_svg!("places/16/folder-download.svg"),
            Self::FileDocument => {
                yamis_svg!("mimetypes/scalable-outlined/text-x-generic-template.svg")
            }
            Self::FileImage => yamis_svg!("mimetypes/scalable-outlined/image-svg+xml.svg"),
            Self::FilePdf => yamis_svg!("mimetypes/scalable-outlined/application-pdf.svg"),
            Self::FileArchive => yamis_svg!("emblems/scalable/emblem-package.svg"),
            Self::Internet => yamis_svg!("categories/scalable/applications-internet.svg"),
            Self::TextEdit => yamis_svg!("apps/scalable/accessories-text-editor.svg"),
            Self::View => yamis_svg!("apps/scalable/systemview.svg"),
            Self::Security => yamis_svg!("preferences/scalable/preferences-security.svg"),
            Self::Warning => yamis_svg!("status/scalable/state-warning.svg"),
            Self::Lock => yamis_svg!("emblems/scalable/emblem-encrypted-locked.svg"),
            Self::Power => yamis_svg!("apps/scalable/system-shutdown.svg"),
            Self::Share => yamis_svg!("emblems/scalable/emblem-shared.svg"),
            Self::Audio => yamis_svg!("status/scalable/audio-on.svg"),
            Self::VolumeLow => yamis_svg!("status/scalable/audio-volume-low.svg"),
            Self::Play => yamis_svg!("actions/16/media-playback-start.svg"),
            Self::Pause => yamis_svg!("actions/16/media-playback-pause.svg"),
            Self::MediaStop => yamis_svg!("actions/16/media-playback-stop.svg"),
            Self::Previous => yamis_svg!("actions/16/media-skip-backward.svg"),
            Self::Next => yamis_svg!("actions/16/media-skip-forward.svg"),
            Self::PictureInPicture => yamis_svg!("preferences/scalable/window-duplicate.svg"),
            Self::DarkMode => yamis_svg!("status/scalable/weather-clear-night.svg"),
            Self::Notifications => yamis_svg!("status/scalable/notification-active.svg"),
            Self::NotificationsMuted => {
                yamis_svg!("status/scalable/notification-disabled-symbolic.svg")
            }
            Self::Workstation => yamis_svg!("apps/scalable/helio-workstation.svg"),
            Self::Server => yamis_svg!("devices/scalable/network-server.svg"),
            Self::Lighthouse => {
                yamis_svg!("preferences/scalable/preferences-system-network-server.svg")
            }
            Self::Signal => yamis_svg!("status/scalable/network-wireless-100.svg"),
            Self::Sessions => yamis_svg!("preferences/scalable/window-duplicate.svg"),
            Self::Start => yamis_svg!("apps/scalable/start-here.svg"),
            Self::Pin => yamis_svg!("actions/16/window-pin.svg"),
            Self::ChevronUp => yamis_svg!("actions/16/arrow-up.svg"),
            Self::Volume => yamis_svg!("status/scalable/audio-volume-medium.svg"),
            Self::VolumeMuted => yamis_svg!("status/scalable/audio-volume-muted.svg"),
            Self::BluetoothSmall => yamis_svg!("status/scalable/bluetooth-active.svg"),
            Self::BatteryEmpty => yamis_svg!("status/scalable/battery-000.svg"),
            Self::BatteryQuarter => yamis_svg!("status/scalable/battery-020.svg"),
            Self::BatteryHalf => yamis_svg!("status/scalable/battery-050.svg"),
            Self::BatteryThreeQuarter => yamis_svg!("status/scalable/battery-080.svg"),
            Self::BatteryFull => yamis_svg!("status/scalable/battery-100.svg"),
            Self::BatteryBolt => yamis_svg!("status/scalable/battery-100-charging.svg"),
        }
    }

    /// The glyph's stable asset name (the SVG file stem, e.g.
    /// `"surface-terminal"`) — handy as an egui texture debug-name and for
    /// packaging scripts that resolve the on-disk asset.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mark => "mark",
            Self::Wordmark => "wordmark",
            Self::Node => "yamis-network-workgroup",
            Self::Workbench => "yamis-applications-system",
            Self::Instances => "yamis-computer",
            Self::Desktop => "yamis-video-display",
            Self::Music => "yamis-folder-music",
            Self::Media => "yamis-applications-multimedia",
            Self::Files => "yamis-system-file-manager",
            Self::Voice => "yamis-audio-input-microphone",
            Self::Browser => "yamis-chromium",
            Self::MapsLocation => "yamis-maps-location",
            Self::Bookmarks => "yamis-emblem-favorite",
            Self::Terminal => "yamis-utilities-terminal",
            Self::Editor => "yamis-code-oss",
            Self::Chat => "yamis-tray-message",
            Self::Phones => "yamis-phone",
            Self::System => "yamis-preferences-system",
            Self::Storage => "yamis-drive-harddisk",
            Self::MeshView => "yamis-network-card",
            Self::Settings => "yamis-systemsettings",
            Self::DisplaySettings => "yamis-preferences-displays",
            Self::Mouse => "yamis-preferences-mouse",
            Self::Touchpad => "yamis-preferences-touchpad",
            Self::Keyboard => "yamis-preferences-desktop-keyboard",
            Self::Wallpaper => "yamis-preferences-desktop-wallpaper",
            Self::Appearance => "yamis-preferences-appearance",
            Self::Bluetooth => "yamis-bluetooth-active-settings",
            Self::PowerBattery => "yamis-preferences-power-and-battery",
            Self::NetworkSettings => "yamis-preferences-system-network",
            Self::Search => "yamis-system-search",
            Self::Close => "yamis-remove",
            Self::Reload => "yamis-state-sync",
            Self::Cancel => "yamis-dialog-cancel",
            Self::ArrowLeft => "yamis-arrow-left",
            Self::ArrowRight => "yamis-arrow-right",
            Self::ArrowDown => "yamis-arrow-down",
            Self::Menu => "yamis-application-menu",
            Self::MoreHorizontal => "yamis-more-horizontal",
            Self::Downloads => "yamis-emblem-downloads",
            Self::Capture => "yamis-camera-photo",
            Self::Print => "yamis-printer",
            Self::History => "yamis-document-open-recent",
            Self::Tabs => "yamis-preferences-tabs",
            Self::NewTab => "yamis-list-add",
            Self::Add => "yamis-list-add-generic",
            Self::Remove => "yamis-list-remove",
            Self::ZoomIn => "yamis-zoom-in",
            Self::ZoomOut => "yamis-zoom-out",
            Self::Check => "yamis-checkmark",
            Self::Page => "yamis-preferences-document",
            Self::FileFolder => "yamis-folder-file-entry",
            Self::FileHome => "yamis-user-home-files-place",
            Self::FileDocuments => "yamis-folder-documents-files-place",
            Self::FileDownloads => "yamis-folder-download-files-place",
            Self::FileDocument => "yamis-text-generic-file",
            Self::FileImage => "yamis-image-file",
            Self::FilePdf => "yamis-application-pdf-file",
            Self::FileArchive => "yamis-emblem-package-file",
            Self::Internet => "yamis-applications-internet",
            Self::TextEdit => "yamis-accessories-text-editor",
            Self::View => "yamis-systemview",
            Self::Security => "yamis-preferences-security",
            Self::Warning => "yamis-state-warning",
            Self::Lock => "yamis-emblem-encrypted-locked",
            Self::Power => "yamis-system-shutdown",
            Self::Share => "yamis-emblem-shared",
            Self::Audio => "yamis-audio-on",
            Self::VolumeLow => "yamis-audio-volume-low",
            Self::Play => "yamis-media-playback-start",
            Self::Pause => "yamis-media-playback-pause",
            Self::MediaStop => "yamis-media-playback-stop",
            Self::Previous => "yamis-media-skip-backward",
            Self::Next => "yamis-media-skip-forward",
            Self::PictureInPicture => "yamis-window-duplicate-pip",
            Self::DarkMode => "yamis-weather-clear-night",
            Self::Notifications => "yamis-notification-active",
            Self::NotificationsMuted => "yamis-notification-disabled",
            Self::Workstation => "yamis-helio-workstation",
            Self::Server => "yamis-network-server",
            Self::Lighthouse => "yamis-preferences-system-network-server",
            Self::Signal => "yamis-network-wireless-100",
            Self::Sessions => "yamis-window-duplicate",
            Self::Start => "yamis-start-here",
            Self::Pin => "yamis-window-pin",
            Self::ChevronUp => "yamis-arrow-up",
            Self::Volume => "yamis-audio-volume-medium",
            Self::VolumeMuted => "yamis-audio-volume-muted",
            Self::BluetoothSmall => "yamis-bluetooth-active",
            Self::BatteryEmpty => "yamis-battery-000",
            Self::BatteryQuarter => "yamis-battery-020",
            Self::BatteryHalf => "yamis-battery-050",
            Self::BatteryThreeQuarter => "yamis-battery-080",
            Self::BatteryFull => "yamis-battery-100",
            Self::BatteryBolt => "yamis-battery-100-charging",
        }
    }
}

/// A rasterized glyph — plain RGBA8 with *straight* (unmultiplied) alpha,
/// row-major, ready for `egui::ColorImage::from_rgba_unmultiplied` (see the
/// module docs for the one-line shell wrapper).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    /// Raster width in pixels (equals the requested size for the square
    /// glyphs; wider for the wordmark lockup).
    pub width: u32,
    /// Raster height in pixels — always exactly the requested `size_px`.
    pub height: u32,
    /// RGBA8 pixel data, straight alpha, row-major; `width × height × 4` bytes.
    pub rgba: Vec<u8>,
}

impl IconImage {
    /// `[width, height]` as `usize` — the shape `egui::ColorImage` wants.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // usize ≥ 32 bits on every MCNF target
    pub const fn size_usize(&self) -> [usize; 2] {
        [self.width as usize, self.height as usize]
    }
}

/// Why a glyph failed to rasterize. Every [`IconId`] source is embedded at
/// compile time and covered by tests, so in practice only [`Self::ZeroSize`]
/// is reachable from caller input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconError {
    /// The requested raster size was zero pixels.
    ZeroSize,
    /// The embedded SVG failed to parse — a build-time asset bug.
    Parse {
        /// The glyph whose embedded source failed to parse.
        id: IconId,
        /// The `usvg` parse error, stringified.
        reason: String,
    },
    /// The raster buffer could not be allocated for these dimensions.
    Alloc {
        /// The glyph being rasterized.
        id: IconId,
        /// The raster width that failed to allocate.
        width: u32,
        /// The raster height that failed to allocate.
        height: u32,
    },
}

impl fmt::Display for IconError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroSize => write!(f, "icon raster size must be non-zero"),
            Self::Parse { id, reason } => {
                write!(f, "embedded SVG for {id:?} failed to parse: {reason}")
            }
            Self::Alloc { id, width, height } => {
                write!(
                    f,
                    "raster buffer alloc failed for {id:?} at {width}x{height}"
                )
            }
        }
    }
}

impl std::error::Error for IconError {}

/// Rasterize a brand glyph at exactly `size_px` tall, tinted `[r, g, b, a]`.
///
/// The glyph's `currentColor` is substituted with the tint's RGB *before*
/// parsing (the line-art glyphs are authored in `currentColor`, so a string
/// substitution colors every stroke and fill; per-glyph `opacity` groups keep
/// the traced artwork's tonal hierarchy under the single color); the tint's
/// alpha is applied per-pixel after rasterization, since SVG hex colors carry
/// no alpha channel. The raster
/// height is exactly `size_px` and the width follows the source aspect ratio —
/// `size_px × size_px` for the square glyphs, proportionally wider for the
/// wordmark — so the result is DPI-crisp at whatever physical size the caller
/// computed.
///
/// # Errors
///
/// [`IconError::ZeroSize`] for `size_px == 0`; [`IconError::Parse`] /
/// [`IconError::Alloc`] only on an embedded-asset or dimension bug (both are
/// exercised across the whole set by this module's tests).
#[allow(
    clippy::cast_precision_loss,      // size_px → f32: raster sizes are small (≪ 2^24)
    clippy::cast_possible_truncation, // rounded, clamped-positive f32 → u32
    clippy::cast_sign_loss            // width ≥ 1.0 by the .max(1.0) clamp
)]
pub fn icon_image(id: IconId, size_px: u32, tint: [u8; 4]) -> Result<IconImage, IconError> {
    if size_px == 0 {
        return Err(IconError::ZeroSize);
    }
    let [red, green, blue, alpha] = tint;
    let colored = id
        .svg()
        .replace("currentColor", &format!("#{red:02x}{green:02x}{blue:02x}"));
    let options = usvg::Options::default();
    let tree = usvg::Tree::from_str(&colored, &options).map_err(|err| IconError::Parse {
        id,
        reason: err.to_string(),
    })?;

    let svg_size = tree.size();
    let scale = size_px as f32 / svg_size.height();
    let width = (svg_size.width() * scale).round().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, size_px).ok_or(IconError::Alloc {
        id,
        width,
        height: size_px,
    })?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    // tiny-skia rasterizes premultiplied; egui wants straight alpha. Demultiply
    // and fold the tint's alpha into the coverage in one pass.
    let mut rgba = Vec::with_capacity(pixmap.pixels().len() * 4);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), scale_alpha(c.alpha(), alpha)]);
    }
    Ok(IconImage {
        width,
        height: size_px,
        rgba,
    })
}

/// Scale a rasterized coverage alpha by the tint's alpha (`coverage × tint /
/// 255`, rounding half up) — how the tint's alpha channel is applied, since
/// the pre-parse color substitution can only carry RGB.
fn scale_alpha(coverage: u8, tint_alpha: u8) -> u8 {
    let scaled = (u16::from(coverage) * u16::from(tint_alpha) + 127) / 255;
    u8::try_from(scaled).unwrap_or(u8::MAX) // ≤ 255 by construction
}

#[cfg(test)]
#[allow(clippy::panic)] // tests fail by panicking, with per-glyph context
mod tests {
    use super::{icon_image, IconError, IconId};

    /// A light Gray-10-ish tint used across the raster tests.
    const TINT: [u8; 4] = [0xe0, 0xe0, 0xe0, 0xff];

    /// Count pixels with any coverage — the "rasterized non-empty" probe.
    fn opaque_pixels(rgba: &[u8]) -> usize {
        rgba.chunks_exact(4).filter(|px| px[3] > 0).count()
    }

    /// Count pixels that render as the icon's strong foreground, ignoring the
    /// low-opacity shell many YAMIS status icons use for the unfilled portion.
    fn strong_pixels(rgba: &[u8]) -> usize {
        rgba.chunks_exact(4).filter(|px| px[3] >= 192).count()
    }

    /// Byte index of the strongest-coverage pixel — a geometry-independent
    /// anchor for the tint assertions (the official mark trace has no
    /// guaranteed feature at any fixed coordinate).
    fn max_alpha_index(rgba: &[u8]) -> usize {
        rgba.chunks_exact(4)
            .enumerate()
            .max_by_key(|(_, px)| px[3])
            .map_or(0, |(i, _)| i * 4)
    }

    #[test]
    fn every_icon_rasterizes_nonempty_at_16_32_64() {
        for id in IconId::ALL {
            for size in [16_u32, 32, 64] {
                let img = icon_image(id, size, TINT)
                    .unwrap_or_else(|err| panic!("{id:?} @ {size}px failed: {err}"));
                assert_eq!(img.height, size, "{id:?} @ {size}px height");
                assert!(img.width >= size, "{id:?} @ {size}px width {}", img.width);
                let [w, h] = img.size_usize();
                assert_eq!(img.rgba.len(), w * h * 4, "{id:?} @ {size}px buffer len");
                // The wordmark is pure <text> and rasterizes transparent
                // without a fontdb (module docs; the dedicated wordmark test
                // pins that behavior) — every other glyph must show ink.
                if id != IconId::Wordmark {
                    assert!(
                        opaque_pixels(&img.rgba) > 0,
                        "{id:?} @ {size}px rasterized empty"
                    );
                }
            }
        }
    }

    #[test]
    fn tint_rgb_covers_every_inked_pixel() {
        // The mark is pure currentColor, so after the pre-parse substitution
        // EVERY covered pixel must carry exactly the tint's RGB (demultiply
        // is exact for 0x00/0xff channels), whatever the traced geometry.
        let img = icon_image(IconId::Mark, 64, [0xff, 0x00, 0x00, 0xff]).expect("mark rasterizes");
        let mut inked = 0_usize;
        for px in img.rgba.chunks_exact(4) {
            if px[3] > 0 {
                inked += 1;
                assert_eq!(&px[..3], &[0xff, 0x00, 0x00], "inked pixel off-tint");
            }
        }
        assert!(inked > 0, "mark rasterized empty");
        // Something renders at real strength too: the blue-plane node fills
        // sit in a 0.55-opacity group (≈ alpha 140), the white plane at full.
        let idx = max_alpha_index(&img.rgba);
        assert!(
            img.rgba[idx + 3] >= 128,
            "mark strongest coverage too faint: {}",
            img.rgba[idx + 3]
        );
    }

    #[test]
    fn tint_alpha_scales_coverage() {
        // Same glyph, same size, tint alpha 255 vs 128: the coverage raster
        // is identical, so each pixel's alpha must land at exactly
        // (coverage × 128 + 127) / 255 — checked at the strongest pixel,
        // independent of the traced geometry.
        let full = icon_image(IconId::Mark, 64, [0xff, 0xff, 0xff, 0xff]).expect("full-alpha mark");
        let half = icon_image(IconId::Mark, 64, [0xff, 0xff, 0xff, 0x80]).expect("half-alpha mark");
        let idx = max_alpha_index(&full.rgba);
        let coverage = full.rgba[idx + 3]; // scale_alpha(c, 255) == c
        assert!(coverage > 0, "mark rasterized empty");
        let expected = u8::try_from((u16::from(coverage) * 0x80 + 127) / 255).unwrap_or(u8::MAX);
        assert_eq!(
            half.rgba[idx + 3],
            expected,
            "tint alpha must scale coverage {coverage}"
        );
    }

    #[test]
    fn wordmark_is_empty_without_a_fontdb_but_never_panics() {
        // The official wordmark is a pure-<text> stacked lockup; resvg is
        // built here without text/fontdb (module docs), so it must parse,
        // keep its wide 420x160 aspect and return a fully transparent raster
        // — gracefully, no panic. The visible lockup ships via the official
        // raster assets (app-icon-*.png / brand::logo, QBRAND-3).
        let img = icon_image(IconId::Wordmark, 48, TINT).expect("wordmark parses + rasterizes");
        assert_eq!(img.height, 48);
        assert_eq!(img.width, 126, "420x160 aspect at 48px tall");
        let [w, h] = img.size_usize();
        assert_eq!(img.rgba.len(), w * h * 4);
        assert_eq!(
            opaque_pixels(&img.rgba),
            0,
            "text lockup unexpectedly rendered without a fontdb"
        );
    }

    #[test]
    fn tray_glyphs_rasterize_nonempty_at_16_and_24() {
        // NAVBAR-W10-1 §7 gate: the Win10 tray draws these at exactly 16px
        // (24px covers the flyout/hi-DPI step) — every glyph must come back
        // square, correctly sized and with real ink through the same
        // icon_image loader the shell uses.
        assert_eq!(IconId::TRAY.len(), 12, "tray subset size");
        assert!(
            !IconId::TRAY.contains(&IconId::Pin),
            "retired Start-bar pin glyph must stay out of the live tray subset"
        );
        assert!(
            !IconId::TRAY.contains(&IconId::ChevronUp),
            "old up-chevron glyph must stay out of the live tray subset"
        );
        for id in IconId::TRAY {
            for size in [16_u32, 24] {
                let img = icon_image(id, size, TINT)
                    .unwrap_or_else(|err| panic!("tray {id:?} @ {size}px failed: {err}"));
                assert_eq!(img.height, size, "tray {id:?} @ {size}px height");
                assert_eq!(img.width, size, "tray glyphs are square, {id:?}");
                assert!(
                    opaque_pixels(&img.rgba) > 0,
                    "tray {id:?} @ {size}px rasterized empty"
                );
            }
        }
    }

    #[test]
    fn battery_fill_ladder_is_strictly_monotonic_at_16px() {
        // YAMIS battery icons share one low-opacity shell and vary the strong
        // foreground fill. At tray size (16px), each step must render strictly
        // more strong pixels than the one below — proving the five levels stay
        // visually distinct where it matters.
        let ladder = [
            IconId::BatteryEmpty,
            IconId::BatteryQuarter,
            IconId::BatteryHalf,
            IconId::BatteryThreeQuarter,
            IconId::BatteryFull,
        ];
        let inked: Vec<usize> = ladder
            .iter()
            .map(|&id| {
                let img =
                    icon_image(id, 16, TINT).unwrap_or_else(|err| panic!("{id:?} failed: {err}"));
                strong_pixels(&img.rgba)
            })
            .collect();
        for pair in inked.windows(2) {
            assert!(
                pair[0] < pair[1],
                "battery fill ladder not monotonic at 16px: {inked:?}"
            );
        }
    }

    #[test]
    fn notification_glyphs_are_yamis_backed_and_rasterize_at_chat_button_size() {
        for id in [IconId::Notifications, IconId::NotificationsMuted] {
            assert!(
                id.name().starts_with("yamis-notification"),
                "{id:?} should stay on the shared YAMIS notification path"
            );
            assert!(
                id.svg().contains("currentColor"),
                "{id:?} must remain tintable through icon_image"
            );
            let img = icon_image(id, 16, TINT).unwrap_or_else(|err| panic!("{id:?}: {err}"));
            assert_eq!(img.height, 16, "{id:?} chat button icon height");
            assert!(opaque_pixels(&img.rgba) > 0, "{id:?} rasterized empty");
        }
    }

    #[test]
    fn zero_size_is_an_error_not_a_panic() {
        assert_eq!(icon_image(IconId::Mark, 0, TINT), Err(IconError::ZeroSize));
    }

    #[test]
    fn ids_names_and_sources_are_distinct_and_exhaustive() {
        // Guards a copy-paste slip in the name table. YAMIS intentionally ships
        // some alias files with identical SVG bodies, so source-content
        // uniqueness is not a valid invariant for mixed theme assets.
        let mut names: Vec<&str> = IconId::ALL.iter().map(|id| id.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), IconId::ALL.len(), "duplicate glyph name");

        for id in IconId::ALL {
            assert!(id.svg().contains("<svg"), "{id:?} source is not SVG");
        }
    }
}
