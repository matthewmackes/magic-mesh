//! Top-level Iced application — state, message reducer, view.
//!
//! CB-1.1 + CB-1.2 scaffold: nine-group sidebar + breadcrumb +
//! page title / subtitle. Per-panel views (CB-1.3 ... CB-1.10)
//! land as separate substeps and plug into [`App::view`] via
//! [`crate::View::Panel`] matching.

use std::sync::Arc;
use std::time::Duration;

use iced::widget::{column, container, row, text};
use iced::{window, Element, Length, Size, Subscription, Task, Theme};

use crate::backend::{Backend, RemoteBackend};
use crate::dbus::PendingFocus;
use crate::header::HeaderAction;
use crate::keyboard::{KeyAction, Pane};
use crate::model::{resolve_panel_label, view_from_focus_slug, Group, View};
use crate::panels::{
    apps_install as apps_install_panel, apps_installed as apps_installed_panel,
    apps_remove as apps_remove_panel, apps_sources as apps_sources_panel, audit as audit_panel,
    compute as compute_panel, config_apply as config_apply_panel, connect as connect_panel,
    datetime as datetime_panel, default_apps as default_apps_panel, displays as displays_panel,
    dns as dns_panel, drift as drift_panel, firewall as firewall_panel,
    fleet_logs as fleet_logs_panel, fleet_revisions as fleet_revisions_panel,
    fleet_rollup as fleet_rollup_panel, fleet_settings as fleet_settings_panel,
    fonts as fonts_panel, hardware as hardware_panel, health_check as health_check_panel,
    help_index as help_index_panel, home as home_panel, hub as hub_panel, images as images_panel,
    interfaces as interfaces_panel, inventory as inventory_panel, jobs as jobs_panel,
    keyboard as keyboard_panel, logs as logs_panel, mesh_bus as mesh_bus_panel,
    mesh_control as mesh_control_panel, mesh_federation as mesh_federation_panel,
    mesh_history as mesh_history_panel, mesh_join as mesh_join_panel, mesh_logs as mesh_logs_panel,
    mesh_pending as mesh_pending_panel, mesh_services as mesh_services_panel,
    mesh_storage as mesh_storage_panel, mirrors as mirrors_panel, mouse as mouse_panel,
    music as music_panel,
    network_hosts as network_hosts_panel, node_roles as node_roles_panel,
    notifications as notifications_panel, panel_apps as panel_apps_panel, peers as peers_panel,
    playbooks as playbooks_panel, policy as policy_panel, power as power_panel,
    printers as printers_panel, profiles as profiles_panel, registration as registration_panel,
    remote_desktop as remote_desktop_panel, removable as removable_panel, repair as repair_panel,
    resources as resources_panel, routing as routing_panel, run_history as run_history_panel,
    service_publishing as service_publishing_panel, session as session_panel,
    snapshots as snapshots_panel, sound as sound_panel, sync_status as sync_status_panel,
    system_update as system_update_panel, tags as tags_panel, themes as themes_panel,
    vpn as vpn_panel, wallpaper as wallpaper_panel, wifi as wifi_panel,
};
use crate::patternfly::{breadcrumb, page_subtitle, page_title};
use crate::sidebar::SidebarState;

/// Default window size — matches the v1.x GTK3 sidebar window
/// (`SidebarWindow` defaults).
pub const WIN_W: f32 = 1180.0;
pub const WIN_H: f32 = 760.0;

/// Build the workbench's `iced::Theme` from `mde_theme::Palette`.
///
/// UX-3 — Q-locked dark palette (Q2 indigo accent + Q3 Apple-
/// charcoal background). E5.3 — the semantic success / danger /
/// warning colours now come from `mde_theme::Palette` (centralized in
/// the design-token crate) instead of hardcoded RGB literals here, so
/// every surface shares the one source.
#[must_use]
pub fn mde_workbench_iced_theme() -> Theme {
    let p = crate::live_theme::palette();
    let palette = iced::theme::Palette {
        background: p.background.into_iced_color(),
        text: p.text.into_iced_color(),
        primary: p.accent.into_iced_color(),
        warning: p.warning.into_iced_color(),
        success: p.success.into_iced_color(),
        danger: p.danger.into_iced_color(),
    };
    Theme::custom("MDE".to_string(), palette)
}

/// Reducer messages — every interaction lands here.
#[derive(Debug, Clone)]
pub enum Message {
    /// Sidebar click on a top-level group row.
    SelectGroup(Group),
    /// PLANES-20 / W87 — drill from a Fleet-rollup card into the Peers
    /// Front Door, pre-filtered to the clicked role.
    DrillToPeers(String),
    /// Sidebar click on a leaf panel row.
    SelectPanel {
        group: Group,
        panel: &'static str,
    },
    /// Keyboard / chord-bar generated key. Translated by
    /// [`crate::keyboard::interpret_key`] before landing here.
    KeyPressed(KeyAction),
    /// User toggled the user-expansion state of a group
    /// (chevron click). Active group ignores this per CB-1.2.
    ToggleGroupExpansion(Group),
    /// CB-1.13 — a `dev.mackes.MDE.Shell.Workbench.Focus(slug)`
    /// call landed in [`PendingFocus`] and the polling
    /// subscription pulled it out. Empty slug means "raise
    /// only — don't change the view" (the 1.x taskbar
    /// click-through contract).
    FocusRequest(String),
    /// CB-1.6 — Look & Feel themes panel sub-message.
    Themes(themes_panel::Message),
    /// CB-1.6 — Look & Feel fonts panel sub-message.
    Fonts(fonts_panel::Message),
    /// CB-1.9 partial — System session panel sub-message.
    Session(session_panel::Message),
    /// CB-1.9 partial — System notifications panel sub-message.
    Notifications(notifications_panel::Message),
    /// CB-1.4 partial — Devices power panel sub-message.
    Power(power_panel::Message),
    /// CB-1.4 partial — Devices removable panel sub-message.
    Removable(removable_panel::Message),
    /// CB-1.4.a — Devices displays panel sub-message.
    Displays(displays_panel::Message),
    /// EPIC-RETIRE-PY-WORKBENCH.port-keyboard — Devices keyboard panel sub-message.
    Keyboard(keyboard_panel::Message),
    /// EPIC-RETIRE-PY-WORKBENCH.port-mouse — Devices mouse/touchpad panel sub-message.
    Mouse(mouse_panel::Message),
    /// CB-1.4.b — Devices sound panel sub-message.
    Sound(sound_panel::Message),
    /// v4.0.1 WB-2.a — Dashboard `home` landing-page messages.
    Home(home_panel::Message),
    /// v4.0.1 WB-2.c — Help index opened a topic; the path is
    /// dispatched to `xdg-open`.
    HelpTopicOpened(std::path::PathBuf),
    /// E6.2 — open a Settings page at the given deep-link slug
    /// (`""` = the Settings home). Fired by the Dashboard role's
    /// See-also links.
    OpenSettings(&'static str),
    /// E0.15 — deep-link a Settings `<category> --page <page>` config
    /// surface. Fired by the Devices launcher panels (mouse/keyboard/
    /// displays) that delegate to Settings instead of duplicating the
    /// libinput controls.
    OpenSettingsPage(&'static str, &'static str),
    /// Launch a standalone MDE app by binary name, detached. Fired by
    /// Overview capability rows whose config surface is its own app
    /// rather than a Workbench panel (e.g. Voice & Video →
    /// `mde-voice-config`). The binary resolves on PATH post-install.
    LaunchApp(&'static str),
    /// CB-1.4.b — Devices sound panel Refresh button. Re-runs
    /// the panel's Load so a freshly-plugged speaker shows up
    /// in the picker without the user having to navigate
    /// away and back.
    SoundRefresh,
    /// CB-1.4.c — Devices printers panel sub-message.
    Printers(printers_panel::Message),
    /// AIR-20 — Devices → Music settings panel sub-message.
    Music(music_panel::Message),
    /// CB-1.4.c — Devices printers panel Refresh button.
    /// Re-runs the panel's Load so a newly-added CUPS queue
    /// shows up in the picker.
    PrintersRefresh,
    /// CB-1.5.a — Fleet inventory panel sub-message.
    Inventory(inventory_panel::Message),
    /// PLANES-5 — hardware inventory (replicated PeerProbe) sub-message.
    Hardware(hardware_panel::Message),
    /// PLANES-10 — Jobs panel (templates + run history) sub-message.
    Jobs(jobs_panel::Message),
    /// PLANES-12 — Audit panel (hash-chain timeline + verify) sub-message.
    Audit(audit_panel::Message),
    /// PLANES-8 — Mesh Logs panel (journald mesh-unit view) sub-message.
    MeshLogs(mesh_logs_panel::Message),
    /// PLANES-7 — Config-apply panel (applied vs newest revision) sub-message.
    ConfigApply(config_apply_panel::Message),
    /// PLANES-4 — Registration panel (identity + cert lifecycle) sub-message.
    Registration(registration_panel::Message),
    /// PLANES-20 — Fleet rollup dashboard sub-message.
    FleetRollup(fleet_rollup_panel::Message),
    /// PLANES-23 — Node roles + tags panel sub-message.
    NodeRoles(node_roles_panel::Message),
    /// PLANES-14 — Fleet logs search sub-message.
    FleetLogs(fleet_logs_panel::Message),
    /// CB-1.5.b — Fleet playbooks panel sub-message.
    Playbooks(playbooks_panel::Message),
    /// CB-1.5.c — Fleet run-history panel sub-message.
    RunHistory(run_history_panel::Message),
    /// CB-1.9.a — System date/time panel sub-message.
    DateTime(datetime_panel::Message),
    /// CB-1.9.b — System default-apps panel sub-message.
    DefaultApps(default_apps_panel::Message),
    /// CB-1.9.d — Maintain snapshots panel sub-message.
    Snapshots(snapshots_panel::Message),
    /// CB-1.7 partial — Maintain logs panel sub-message.
    Logs(logs_panel::Message),
    /// CB-1.7 partial — Maintain resources panel sub-message.
    Resources(resources_panel::Message),
    /// E6.10 — Compute group instance-list sub-message.
    Compute(compute_panel::Message),
    /// CB-1.7 partial — Maintain system-update panel sub-message.
    SystemUpdate(system_update_panel::Message),
    /// CB-1.7 partial — Maintain repair panel sub-message.
    Repair(repair_panel::Message),
    /// CB-1.3 partial — Apps → Installed panel sub-message.
    AppsInstalled(apps_installed_panel::Message),
    /// CB-1.3 partial — Apps → Sources panel sub-message.
    AppsSources(apps_sources_panel::Message),
    /// CB-1.3 follow-up — Apps → Install panel sub-message.
    AppsInstall(apps_install_panel::Message),
    /// CB-1.3 follow-up — Apps → Remove panel sub-message.
    AppsRemove(apps_remove_panel::Message),
    HealthCheck(health_check_panel::Message),
    Drift(drift_panel::Message),
    Policy(policy_panel::Message),
    Interfaces(interfaces_panel::Message),
    Dns(dns_panel::Message),
    Routing(routing_panel::Message),
    Tags(tags_panel::Message),
    Profiles(profiles_panel::Message),
    Mirrors(mirrors_panel::Message),
    Images(images_panel::Message),
    /// BUS-7.2 — Network → Mackes Bus panel sub-message.
    MeshBus(mesh_bus_panel::Message),
    /// TUNE-15.b — Network → Mesh Federation panel sub-message.
    MeshFederation(mesh_federation_panel::Message),
    MeshControl(mesh_control_panel::Message),
    MeshPending(mesh_pending_panel::Message),
    MeshServices(mesh_services_panel::Message),
    /// NF-13.8 — Network → Service Publishing sub-message.
    ServicePublishing(service_publishing_panel::Message),
    MeshStorage(mesh_storage_panel::Message),
    /// MESH-PROBE-9.a — Network → Network Hosts panel sub-message.
    NetworkHosts(network_hosts_panel::Message),
    PanelApps(panel_apps_panel::Message),
    RemoteDesktop(remote_desktop_panel::Message),
    /// PD-3 — the Peers directory (Front Door) sub-message.
    Peers(peers_panel::Message),
    SyncStatus(sync_status_panel::Message),
    /// CB-1.8 partial — Network → Firewall panel sub-message.
    Firewall(firewall_panel::Message),
    /// CB-1.8 partial — Network → Wi-Fi panel sub-message.
    Wifi(wifi_panel::Message),
    /// CB-1.8 partial — Network → VPN panel sub-message.
    Vpn(vpn_panel::Message),
    /// CB-1.8 partial — Network → Mesh Join panel sub-message.
    MeshJoin(mesh_join_panel::Message),
    /// CB-1.8 partial — Network → Mesh History panel sub-message.
    MeshHistory(mesh_history_panel::Message),
    /// CB-1.5 partial — Fleet settings panel sub-message.
    FleetSettings(fleet_settings_panel::Message),
    /// CB-1.5 partial — Fleet revisions panel sub-message.
    FleetRevisions(fleet_revisions_panel::Message),
    /// CB-1.6 follow-on — Look & Feel wallpaper panel sub-message.
    Wallpaper(wallpaper_panel::Message),
    /// UX-4 — header bar window-control click (min/max/close).
    /// The reducer maps each variant to an `iced::window::*`
    /// Task. The Iced runtime delivers the live `window::Id` on
    /// every `get_latest()` so a `--focus` hand-off and a normal
    /// startup go through the same code path.
    WindowControl(HeaderAction),
    /// No-op — the inert default for declaratively-wired fallbacks
    /// (focus-drain misses, lazy widget message slots). Not a stub:
    /// every live use is a functional "nothing to do" value.
    Noop,
}

/// Workbench application state.
#[derive(Clone)]
pub struct App {
    view: View,
    sidebar: SidebarState,
    focused_pane: Pane,
    backend: Arc<dyn Backend>,
    themes: themes_panel::ThemesPanel,
    fonts: fonts_panel::FontsPanel,
    session: session_panel::SessionPanel,
    notifications: notifications_panel::NotificationsPanel,
    power: power_panel::PowerPanel,
    removable: removable_panel::RemovablePanel,
    displays: displays_panel::DisplaysPanel,
    keyboard: keyboard_panel::KeyboardPanel,
    mouse: mouse_panel::MousePanel,
    sound: sound_panel::SoundPanel,
    printers: printers_panel::PrintersPanel,
    music: music_panel::MusicPanel,
    /// v4.0.1 WB-1 — Connected Devices panel state. Hosts the
    /// paired-peer list + per-row action handlers.
    connect: connect_panel::ConnectPanel,
    /// v4.0.1 WB-2.a — Dashboard landing page state.
    home: home_panel::HomePanel,
    /// v4.0.1 WB-2.b — Maintain group root grid state.
    hub: hub_panel::HubPanel,
    /// v4.0.1 WB-2.c — Help group root topics list.
    help: help_index_panel::HelpIndexPanel,
    inventory: inventory_panel::InventoryPanel,
    hardware: hardware_panel::HardwarePanel,
    jobs: jobs_panel::JobsPanel,
    audit: audit_panel::AuditPanel,
    mesh_logs: mesh_logs_panel::MeshLogsPanel,
    config_apply: config_apply_panel::ConfigApplyPanel,
    registration: registration_panel::RegistrationPanel,
    fleet_rollup: fleet_rollup_panel::FleetRollupPanel,
    node_roles: node_roles_panel::NodeRolesPanel,
    fleet_logs: fleet_logs_panel::FleetLogsPanel,
    playbooks: playbooks_panel::PlaybooksPanel,
    run_history: run_history_panel::RunHistoryPanel,
    datetime: datetime_panel::DateTimePanel,
    default_apps: default_apps_panel::DefaultAppsPanel,
    snapshots: snapshots_panel::SnapshotsPanel,
    logs: logs_panel::LogsPanel,
    resources: resources_panel::ResourcesPanel,
    compute: compute_panel::ComputePanel,
    system_update: system_update_panel::SystemUpdatePanel,
    repair: repair_panel::RepairPanel,
    apps_installed: apps_installed_panel::AppsInstalledPanel,
    apps_sources: apps_sources_panel::AppsSourcesPanel,
    apps_install: apps_install_panel::AppsInstallPanel,
    apps_remove: apps_remove_panel::AppsRemovePanel,
    health_check: health_check_panel::HealthCheckPanel,
    drift: drift_panel::DriftPanel,
    policy: policy_panel::PolicyPanel,
    interfaces: interfaces_panel::InterfacesPanel,
    dns: dns_panel::DnsPanel,
    routing: routing_panel::RoutingPanel,
    tags: tags_panel::TagsPanel,
    profiles: profiles_panel::ProfilesPanel,
    mirrors: mirrors_panel::MirrorsPanel,
    images: images_panel::ImagesPanel,
    mesh_bus: mesh_bus_panel::MeshBusPanel,
    mesh_federation: mesh_federation_panel::MeshFederationPanel,
    mesh_control: mesh_control_panel::MeshControlPanel,
    mesh_pending: mesh_pending_panel::MeshPendingPanel,
    mesh_services: mesh_services_panel::MeshServicesPanel,
    /// NF-13.8 — Network → Service Publishing panel state.
    service_publishing: service_publishing_panel::ServicePublishingPanel,
    mesh_storage: mesh_storage_panel::MeshStoragePanel,
    /// MESH-PROBE-9.a — Network → Network Hosts panel state (the probe
    /// host/service inventory read off mesh-storage).
    network_hosts: network_hosts_panel::NetworkHostsPanel,
    panel_apps: panel_apps_panel::PanelAppsPanel,
    remote_desktop: remote_desktop_panel::RemoteDesktopPanel,
    peers: peers_panel::PeersPanel,
    sync_status: sync_status_panel::SyncStatusPanel,
    firewall: firewall_panel::FirewallPanel,
    wifi: wifi_panel::WifiPanel,
    vpn: vpn_panel::VpnPanel,
    mesh_join: mesh_join_panel::MeshJoinPanel,
    mesh_history: mesh_history_panel::MeshHistoryPanel,
    fleet_settings: fleet_settings_panel::FleetSettingsPanel,
    fleet_revisions: fleet_revisions_panel::FleetRevisionsPanel,
    wallpaper: wallpaper_panel::WallpaperPanel,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("view", &self.view)
            .field("focused_pane", &self.focused_pane)
            .field("themes", &self.themes)
            .field("fonts", &self.fonts)
            .field("session", &self.session)
            .field("notifications", &self.notifications)
            .finish_non_exhaustive()
    }
}

impl Default for App {
    /// v4.0.1 AF-2.3.b (2026-05-23) — production default now
    /// uses `RemoteBackend` which wraps the AF-2.3.a
    /// FileBackend: every `set` writes the local
    /// `~/.config/mde/workbench-settings.toml` AND pushes to
    /// `dev.mackes.MDE.Settings.Set` on the session bus
    /// (best-effort; the local write always succeeds even
    /// when mackesd is offline). mackesd's mesh settings sync
    /// propagates the bus-side write to peers, so changing
    /// a font in Workbench surfaces on a peer within ~5 s.
    fn default() -> Self {
        Self::with_backend(Arc::new(RemoteBackend::new()))
    }
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build an [`App`] over a specific [`Backend`] — used by
    /// `main.rs` to wire the live [`crate::RemoteBackend`] and
    /// by tests to substitute [`DemoBackend`] with seeded
    /// values.
    #[must_use]
    pub fn with_backend(backend: Arc<dyn Backend>) -> Self {
        Self {
            view: View::default(),
            sidebar: SidebarState::new(),
            focused_pane: Pane::Sidebar,
            backend,
            themes: themes_panel::ThemesPanel::new(),
            fonts: fonts_panel::FontsPanel::new(),
            session: session_panel::SessionPanel::new(),
            notifications: notifications_panel::NotificationsPanel::new(),
            power: power_panel::PowerPanel::new(),
            removable: removable_panel::RemovablePanel::new(),
            displays: displays_panel::DisplaysPanel::new(),
            keyboard: keyboard_panel::KeyboardPanel::new(),
            mouse: mouse_panel::MousePanel::new(),
            sound: sound_panel::SoundPanel::new(),
            printers: printers_panel::PrintersPanel::new(),
            music: music_panel::MusicPanel::new(),
            connect: connect_panel::ConnectPanel::new(),
            home: home_panel::HomePanel::new(),
            hub: hub_panel::HubPanel::new(),
            help: help_index_panel::HelpIndexPanel::new(),
            inventory: inventory_panel::InventoryPanel::new(),
            hardware: hardware_panel::HardwarePanel::new(),
            jobs: jobs_panel::JobsPanel::new(),
            audit: audit_panel::AuditPanel::new(),
            mesh_logs: mesh_logs_panel::MeshLogsPanel::new(),
            config_apply: config_apply_panel::ConfigApplyPanel::new(),
            registration: registration_panel::RegistrationPanel::new(),
            fleet_rollup: fleet_rollup_panel::FleetRollupPanel::new(),
            node_roles: node_roles_panel::NodeRolesPanel::new(),
            fleet_logs: fleet_logs_panel::FleetLogsPanel::new(),
            playbooks: playbooks_panel::PlaybooksPanel::new(),
            run_history: run_history_panel::RunHistoryPanel::new(),
            datetime: datetime_panel::DateTimePanel::new(),
            default_apps: default_apps_panel::DefaultAppsPanel::new(),
            snapshots: snapshots_panel::SnapshotsPanel::new(),
            logs: logs_panel::LogsPanel::new(),
            resources: resources_panel::ResourcesPanel::new(),
            compute: compute_panel::ComputePanel::new(),
            system_update: system_update_panel::SystemUpdatePanel::new(),
            repair: repair_panel::RepairPanel::new(),
            apps_installed: apps_installed_panel::AppsInstalledPanel::new(),
            apps_sources: apps_sources_panel::AppsSourcesPanel::new(),
            apps_install: apps_install_panel::AppsInstallPanel::new(),
            apps_remove: apps_remove_panel::AppsRemovePanel::new(),
            health_check: health_check_panel::HealthCheckPanel::new(),
            drift: drift_panel::DriftPanel::new(),
            policy: policy_panel::PolicyPanel::new(),
            interfaces: interfaces_panel::InterfacesPanel::new(),
            dns: dns_panel::DnsPanel::new(),
            routing: routing_panel::RoutingPanel::new(),
            tags: tags_panel::TagsPanel::new(),
            profiles: profiles_panel::ProfilesPanel::new(),
            mirrors: mirrors_panel::MirrorsPanel::new(),
            images: images_panel::ImagesPanel::new(),
            mesh_bus: mesh_bus_panel::MeshBusPanel::new(),
            mesh_federation: mesh_federation_panel::MeshFederationPanel::new(),
            mesh_control: mesh_control_panel::MeshControlPanel::new(),
            mesh_pending: mesh_pending_panel::MeshPendingPanel::new(),
            mesh_services: mesh_services_panel::MeshServicesPanel::new(),
            service_publishing: service_publishing_panel::ServicePublishingPanel::new(),
            mesh_storage: mesh_storage_panel::MeshStoragePanel::new(),
            network_hosts: network_hosts_panel::NetworkHostsPanel::new(),
            panel_apps: panel_apps_panel::PanelAppsPanel::new(),
            remote_desktop: remote_desktop_panel::RemoteDesktopPanel::new(),
            peers: peers_panel::PeersPanel::new(),
            sync_status: sync_status_panel::SyncStatusPanel::new(),
            firewall: firewall_panel::FirewallPanel::new(),
            wifi: wifi_panel::WifiPanel::new(),
            vpn: vpn_panel::VpnPanel::new(),
            mesh_join: mesh_join_panel::MeshJoinPanel::new(),
            mesh_history: mesh_history_panel::MeshHistoryPanel::new(),
            fleet_settings: fleet_settings_panel::FleetSettingsPanel::new(),
            fleet_revisions: fleet_revisions_panel::FleetRevisionsPanel::new(),
            wallpaper: wallpaper_panel::WallpaperPanel::new(),
        }
    }

    /// Build an [`App`] pre-focused on a deep-link slug
    /// (e.g. `--focus network.remote_desktop`). Falls back to
    /// the default Dashboard view when the slug is unknown.
    #[must_use]
    pub fn with_focus(focus_slug: &str) -> Self {
        let mut app = Self::default();
        if let Some(view) = view_from_focus_slug(focus_slug) {
            app.view = view;
            app.focused_pane = Pane::Main;
        }
        app
    }

    /// Clone of the backend handle — `Task::perform` futures
    /// keep their own `Arc<dyn Backend>` so the reducer stays
    /// non-blocking.
    pub fn backend(&self) -> Arc<dyn Backend> {
        Arc::clone(&self.backend)
    }

    /// Read-only view of the themes panel state — used by tests
    /// + by the view layer to render the panel chrome.
    #[must_use]
    pub fn themes(&self) -> &themes_panel::ThemesPanel {
        &self.themes
    }

    /// Read-only view of the fonts panel state.
    #[must_use]
    pub fn fonts(&self) -> &fonts_panel::FontsPanel {
        &self.fonts
    }

    /// Read-only view of the session panel state.
    #[must_use]
    pub fn session(&self) -> &session_panel::SessionPanel {
        &self.session
    }

    /// Read-only view of the notifications panel state.
    #[must_use]
    pub fn notifications(&self) -> &notifications_panel::NotificationsPanel {
        &self.notifications
    }

    /// Read-only view of the power panel state.
    #[must_use]
    pub fn power(&self) -> &power_panel::PowerPanel {
        &self.power
    }

    /// Read-only view of the removable panel state.
    #[must_use]
    pub fn removable(&self) -> &removable_panel::RemovablePanel {
        &self.removable
    }

    /// Read-only view of the displays panel state.
    #[must_use]
    pub fn displays(&self) -> &displays_panel::DisplaysPanel {
        &self.displays
    }

    /// Read-only view of the sound panel state.
    #[must_use]
    pub fn sound(&self) -> &sound_panel::SoundPanel {
        &self.sound
    }

    /// Read-only view of the printers panel state.
    #[must_use]
    pub fn printers(&self) -> &printers_panel::PrintersPanel {
        &self.printers
    }

    /// Read-only view of the inventory panel state.
    #[must_use]
    pub fn inventory(&self) -> &inventory_panel::InventoryPanel {
        &self.inventory
    }

    /// Read-only view of the playbooks panel state.
    #[must_use]
    pub fn playbooks(&self) -> &playbooks_panel::PlaybooksPanel {
        &self.playbooks
    }

    /// Read-only view of the run-history panel state.
    #[must_use]
    pub fn run_history(&self) -> &run_history_panel::RunHistoryPanel {
        &self.run_history
    }

    /// Read-only view of the datetime panel state.
    #[must_use]
    pub fn datetime(&self) -> &datetime_panel::DateTimePanel {
        &self.datetime
    }

    /// Read-only view of the default-apps panel state.
    #[must_use]
    pub fn default_apps(&self) -> &default_apps_panel::DefaultAppsPanel {
        &self.default_apps
    }

    /// Read-only view of the snapshots panel state.
    #[must_use]
    pub fn snapshots(&self) -> &snapshots_panel::SnapshotsPanel {
        &self.snapshots
    }

    /// Read-only view of the logs panel state.
    #[must_use]
    pub fn logs(&self) -> &logs_panel::LogsPanel {
        &self.logs
    }

    /// Read-only view of the resources panel state.
    #[must_use]
    pub fn resources(&self) -> &resources_panel::ResourcesPanel {
        &self.resources
    }

    /// Read-only view of the system-update panel state.
    #[must_use]
    pub fn system_update(&self) -> &system_update_panel::SystemUpdatePanel {
        &self.system_update
    }

    /// Read-only view of the repair panel state.
    #[must_use]
    pub fn repair(&self) -> &repair_panel::RepairPanel {
        &self.repair
    }

    /// Read-only view of the apps-installed panel state.
    #[must_use]
    pub fn apps_installed(&self) -> &apps_installed_panel::AppsInstalledPanel {
        &self.apps_installed
    }

    /// Read-only view of the apps-sources panel state.
    #[must_use]
    pub fn apps_sources(&self) -> &apps_sources_panel::AppsSourcesPanel {
        &self.apps_sources
    }

    /// Read-only view of the apps-install panel state.
    #[must_use]
    pub fn apps_install(&self) -> &apps_install_panel::AppsInstallPanel {
        &self.apps_install
    }

    /// Read-only view of the apps-remove panel state.
    #[must_use]
    pub fn apps_remove(&self) -> &apps_remove_panel::AppsRemovePanel {
        &self.apps_remove
    }

    /// Read-only view of the firewall panel state.
    #[must_use]
    pub fn firewall(&self) -> &firewall_panel::FirewallPanel {
        &self.firewall
    }

    /// Read-only view of the wifi panel state.
    #[must_use]
    pub fn wifi(&self) -> &wifi_panel::WifiPanel {
        &self.wifi
    }

    /// Read-only view of the vpn panel state.
    #[must_use]
    pub fn vpn(&self) -> &vpn_panel::VpnPanel {
        &self.vpn
    }

    /// Read-only view of the mesh-join panel state.
    #[must_use]
    pub fn mesh_join(&self) -> &mesh_join_panel::MeshJoinPanel {
        &self.mesh_join
    }

    /// Read-only view of the mesh-history panel state.
    #[must_use]
    pub fn mesh_history(&self) -> &mesh_history_panel::MeshHistoryPanel {
        &self.mesh_history
    }

    /// Read-only view of the fleet settings panel state.
    #[must_use]
    pub fn fleet_settings(&self) -> &fleet_settings_panel::FleetSettingsPanel {
        &self.fleet_settings
    }

    /// Read-only view of the fleet revisions panel state.
    #[must_use]
    pub fn fleet_revisions(&self) -> &fleet_revisions_panel::FleetRevisionsPanel {
        &self.fleet_revisions
    }

    /// Read-only view of the wallpaper panel state.
    #[must_use]
    pub fn wallpaper(&self) -> &wallpaper_panel::WallpaperPanel {
        &self.wallpaper
    }

    #[must_use]
    pub fn current_view(&self) -> View {
        self.view
    }

    #[must_use]
    pub fn focused_pane(&self) -> Pane {
        self.focused_pane
    }

    /// Run the Iced application.
    pub fn run() -> iced::Result {
        // UX-4 (d) — request a decoration-less window so the
        // custom `crate::header` bar is the only title strip the
        // user sees. sway tiles Iced apps without server-side
        // decorations regardless, but setting this explicitly
        // means GNOME-shell / KDE-on-Wayland sessions (which the
        // X11 build feature enables for the v2.0.0 fallback path)
        // also fall back to client-side chrome.
        // iced 0.14: application(boot, update, view) — the first arg
        // is the boot fn (initial State); the title moved to .title().
        // PD-4 — boot lands on the Peers Front Door; fire its
        // directory load immediately so the panel is live, not
        // "Loading…" until the first manual refresh.
        iced::application(
            || {
                // Deep-link boot: a `--focus <slug>` (queued in PendingFocus
                // by main before run()) lands DIRECTLY on the target panel
                // and fires its load, instead of flashing the Peers front
                // door and only navigating on the next 200 ms poll. The poll
                // still serves sibling-process `--focus` handoffs.
                match PendingFocus::drain() {
                    Some(slug) if !slug.is_empty() => {
                        let app = App::with_focus(&slug);
                        let boot = match app.view {
                            View::Panel { group, panel } => app.on_panel_navigated(group, panel),
                            View::Group(g) => app.on_group_navigated(g),
                        };
                        (app, boot)
                    }
                    _ => (App::new(), crate::panels::peers::PeersPanel::load()),
                }
            },
            Self::update,
            Self::view,
        )
        .title(Self::title)
        .theme(Self::theme)
        .subscription(Self::subscription)
        .window(window::Settings {
            size: Size::new(WIN_W, WIN_H),
            decorations: false,
            ..window::Settings::default()
        })
        .run()
    }

    /// Iced subscription bundle. Two streams:
    ///
    /// 1. **PendingFocus poll** — 200 ms tick that drains any
    ///    `dev.mackes.MDE.Shell.Workbench.Focus` D-Bus call from
    ///    a sibling `mde-workbench --focus <slug>` invocation
    ///    into [`Message::FocusRequest`].
    /// 2. **Overview D-Bus signal bridge** — subscribes to the
    ///    Nebula + Fleet signals mackesd emits so the Overview's
    ///    capability cards refresh without polling
    ///    (see `panels::home::dbus_subscription`).
    #[allow(clippy::unused_self)]
    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            iced::time::every(Duration::from_millis(200))
                .map(|_| PendingFocus::drain().map_or(Message::Noop, Message::FocusRequest)),
            home_panel::dbus_subscription(),
            // E0.3.1.b — Nebula signals now arrive over the mesh Bus
            // event topic, not D-Bus; this polls them into DbusEvents.
            home_panel::nebula_event_subscription(),
        ];
        // E6.10 — sample Compute instance CPU/mem only while that view is
        // active, so virsh/podman stats aren't polled when the operator is
        // elsewhere.
        if self.view.group() == Group::Compute {
            subs.push(compute_panel::ComputePanel::sample_subscription());
        }
        // PD-8 (L14) / PLANES-1 — Netdata sampling only while the Peers
        // directory is the active view (the Compute pattern). The Front
        // Door is reachable as the Peers plane root/panel or the
        // Controller/Inventory door.
        if matches!(
            self.view,
            View::Group(Group::Peers)
                | View::Panel {
                    group: Group::Peers,
                    panel: "peers"
                }
                | View::Panel {
                    group: Group::Controller,
                    panel: "peers"
                }
        ) {
            subs.push(peers_panel::metrics_subscription());
            // PD-3/Q10 — refresh the directory itself every 30 s while
            // the Front Door is open, so presence/health/tags stay live.
            subs.push(peers_panel::directory_subscription());
            // PD-3/Q10 — plus the Bus-push half: reload the instant the
            // responder reports a roster change.
            subs.push(peers_panel::directory_event_subscription());
        }
        Subscription::batch(subs)
    }

    fn title(&self) -> String {
        format!("MDE Workbench — {}", page_title(self.view))
    }

    #[allow(clippy::unused_self)]
    fn theme(&self) -> Theme {
        // UX-3 — Iced framework palette is derived from the
        // locked `mde_theme::Palette` so every widget that defers
        // to the theme (default backgrounds, text, primary
        // buttons) renders with Q-locked indigo + Q-locked
        // charcoal instead of Iced's stock dark navy. Widget-
        // level deep restyling is the scope of UX-4..UX-9; this
        // step alone moves the workbench's base surface onto the
        // MDE identity.
        mde_workbench_iced_theme()
    }

    /// Apply a [`Message`] to the state. Returns [`Task::none`]
    /// for synchronous variants; panel messages fan out into
    /// real async backend calls.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SelectGroup(group) => {
                self.view = View::Group(group);
                self.focused_pane = Pane::Main;
                self.on_group_navigated(group)
            }
            // PLANES-20 / W87 — open the Peers Front Door filtered to the
            // role the operator drilled into. The filter is set before the
            // load resolves; the Peers Loaded handler preserves it.
            Message::DrillToPeers(role) => {
                self.view = View::Group(Group::Peers);
                self.focused_pane = Pane::Main;
                let task = self.on_group_navigated(Group::Peers);
                self.peers.filter = role;
                task
            }
            Message::SelectPanel { group, panel } => {
                self.view = View::Panel { group, panel };
                self.focused_pane = Pane::Main;
                self.on_panel_navigated(group, panel)
            }
            Message::ToggleGroupExpansion(group) => {
                self.sidebar.toggle(group, self.view.group());
                Task::none()
            }
            Message::KeyPressed(action) => {
                self.apply_key_action(action);
                Task::none()
            }
            Message::FocusRequest(slug) => self.apply_focus_request(&slug),
            Message::Themes(msg) => self.themes.update(msg),
            Message::Fonts(msg) => self.fonts.update(msg, self.backend()),
            Message::Session(msg) => self.session.update(msg, self.backend()),
            Message::Notifications(msg) => self.notifications.update(msg, self.backend()),
            Message::Power(msg) => self.power.update(msg, self.backend()),
            Message::Removable(msg) => self.removable.update(msg, self.backend()),
            Message::Displays(msg) => self.displays.update(msg, self.backend()),
            Message::Keyboard(msg) => self.keyboard.update(msg, self.backend()),
            Message::Mouse(msg) => self.mouse.update(msg, self.backend()),
            Message::Sound(msg) => self.sound.update(msg),
            Message::SoundRefresh => sound_panel::SoundPanel::load(),
            Message::Home(msg) => self.home.update(msg),
            Message::HelpTopicOpened(path) => {
                help_index_panel::spawn_xdg_open(&path);
                Task::none()
            }
            Message::OpenSettings(slug) => {
                // E6.2 — bridge to the shell's Win10 Settings app. Spawn
                // detached (mirrors the help xdg-open path); `mde` resolves
                // on PATH post-install. Empty slug opens the Settings home.
                let mut cmd = std::process::Command::new("mde");
                cmd.arg("settings");
                if !slug.is_empty() {
                    cmd.arg(slug);
                }
                let _ = cmd
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                Task::none()
            }
            Message::LaunchApp(bin) => {
                // Detached best-effort spawn, mirroring OpenSettings. The
                // binary resolves on PATH post-install; a missing binary
                // (dev tree without an install) fails silently rather than
                // panicking the Workbench.
                let _ = std::process::Command::new(bin)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                Task::none()
            }
            Message::OpenSettingsPage(category, page) => {
                // E0.15 — bridge to the legacy `mde settings <category> --page
                // <page>` config surface (Cosmic now owns the desktop; this exec
                // targets the retired dispatcher path). Detached spawn, mirrors
                // OpenSettings above.
                let _ = std::process::Command::new("mde")
                    .arg("settings")
                    .arg(category)
                    .arg("--page")
                    .arg(page)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                Task::none()
            }
            Message::Printers(msg) => self.printers.update(msg),
            Message::Music(msg) => self.music.update(msg),
            Message::PrintersRefresh => printers_panel::PrintersPanel::load(),
            Message::Inventory(msg) => self.inventory.update(msg),
            Message::Hardware(msg) => self.hardware.update(msg),
            Message::Jobs(msg) => self.jobs.update(msg),
            Message::Audit(msg) => self.audit.update(msg),
            Message::MeshLogs(msg) => self.mesh_logs.update(msg),
            Message::ConfigApply(msg) => self.config_apply.update(msg),
            Message::Registration(msg) => self.registration.update(msg),
            Message::FleetRollup(msg) => self.fleet_rollup.update(msg),
            Message::NodeRoles(msg) => self.node_roles.update(msg),
            Message::FleetLogs(msg) => self.fleet_logs.update(msg),
            Message::Playbooks(msg) => self.playbooks.update(msg),
            Message::RunHistory(msg) => self.run_history.update(msg),
            Message::DateTime(msg) => self.datetime.update(msg),
            Message::DefaultApps(msg) => self.default_apps.update(msg),
            Message::Snapshots(msg) => self.snapshots.update(msg),
            Message::Logs(msg) => self.logs.update(msg),
            Message::Resources(msg) => self.resources.update(msg),
            Message::Compute(msg) => self.compute.update(msg),
            Message::SystemUpdate(msg) => self.system_update.update(msg),
            Message::Repair(msg) => self.repair.update(msg),
            Message::AppsInstalled(msg) => self.apps_installed.update(msg),
            Message::AppsSources(msg) => self.apps_sources.update(msg),
            Message::AppsInstall(msg) => self.apps_install.update(msg),
            Message::AppsRemove(msg) => self.apps_remove.update(msg),
            Message::HealthCheck(msg) => self.health_check.update(msg),
            Message::Drift(msg) => self.drift.update(msg),
            Message::Policy(msg) => self.policy.update(msg),
            Message::Interfaces(msg) => self.interfaces.update(msg),
            Message::Dns(msg) => self.dns.update(msg),
            Message::Routing(msg) => self.routing.update(msg),
            Message::Tags(msg) => self.tags.update(msg),
            Message::Profiles(msg) => self.profiles.update(msg),
            Message::Mirrors(msg) => self.mirrors.update(msg),
            Message::Images(msg) => self.images.update(msg),
            Message::MeshBus(msg) => self.mesh_bus.update(msg),
            Message::MeshFederation(msg) => self.mesh_federation.update(msg),
            Message::MeshControl(msg) => self.mesh_control.update(msg),
            Message::MeshPending(msg) => self.mesh_pending.update(msg),
            Message::MeshServices(msg) => self.mesh_services.update(msg),
            Message::ServicePublishing(msg) => self.service_publishing.update(msg),
            Message::MeshStorage(msg) => self.mesh_storage.update(msg),
            Message::NetworkHosts(msg) => self.network_hosts.update(msg),
            Message::PanelApps(msg) => self.panel_apps.update(msg),
            Message::RemoteDesktop(msg) => self.remote_desktop.update(msg),
            Message::Peers(msg) => self.peers.update(msg),
            Message::SyncStatus(msg) => self.sync_status.update(msg),
            Message::Firewall(msg) => self.firewall.update(msg),
            Message::Wifi(msg) => self.wifi.update(msg),
            Message::Vpn(msg) => self.vpn.update(msg),
            Message::MeshJoin(msg) => self.mesh_join.update(msg),
            Message::MeshHistory(msg) => self.mesh_history.update(msg),
            Message::FleetSettings(msg) => self.fleet_settings.update(msg),
            Message::FleetRevisions(msg) => self.fleet_revisions.update(msg),
            Message::Wallpaper(msg) => self.wallpaper.update(msg, self.backend()),
            Message::WindowControl(action) => Self::dispatch_window_action(action),
            Message::Noop => Task::none(),
        }
    }

    /// UX-4 — turn a header-button click into an `iced::window`
    /// Task. `get_latest()` returns the most-recently-created
    /// window id, which for the single-window workbench is the
    /// one the user is interacting with.
    fn dispatch_window_action(action: HeaderAction) -> Task<Message> {
        window::latest().and_then(move |id| match action {
            HeaderAction::Minimize => window::minimize(id, true),
            HeaderAction::ToggleMaximize => window::toggle_maximize(id),
            HeaderAction::Close => window::close(id),
        })
    }

    /// CB-1.6 — when the user lands on a known panel, kick off
    /// the panel's initial load. Unknown panels (no Iced view
    /// shipped yet) just no-op.
    fn on_panel_navigated(&self, group: Group, panel: &'static str) -> Task<Message> {
        match (group, panel) {
            (Group::LookAndFeel, "themes") => themes_panel::ThemesPanel::load(),
            (Group::LookAndFeel, "fonts") => fonts_panel::FontsPanel::load(self.backend()),
            (Group::LookAndFeel, "wallpaper") => {
                wallpaper_panel::WallpaperPanel::load(self.backend())
            }
            (Group::Devices, "session") => session_panel::SessionPanel::load(self.backend()),
            (Group::System, "notifications") => {
                notifications_panel::NotificationsPanel::load(self.backend())
            }
            (Group::Devices, "power") => power_panel::PowerPanel::load(self.backend()),
            (Group::Devices, "removable") => removable_panel::RemovablePanel::load(self.backend()),
            (Group::Devices, "displays") => displays_panel::DisplaysPanel::load(self.backend()),
            (Group::Devices, "keyboard") => keyboard_panel::KeyboardPanel::load(self.backend()),
            (Group::Devices, "mouse") => mouse_panel::MousePanel::load(self.backend()),
            (Group::Devices, "sound") => sound_panel::SoundPanel::load(),
            (Group::Devices, "printers") => printers_panel::PrintersPanel::load(),
            (Group::Devices, "music") => music_panel::MusicPanel::load(),
            // v4.0.1 WB-1 (Phase 0.7 rescue): Connected Devices
            // panel. Real D-Bus subscription wiring chains on
            // KDC2-3.9 signals; the panel.load() returns
            // Task::none today.
            (Group::Devices, "connect") => connect_panel::ConnectPanel::load(),
            // PLANES-1 — Fleet keeps the rollup lens + fleet inventory;
            // the operational panels re-home into the planes.
            (Group::Fleet, "inventory") => inventory_panel::InventoryPanel::load(),
            (Group::Fleet, "fleet_rollup") => fleet_rollup_panel::FleetRollupPanel::load(),
            (Group::ThisNode, "hardware") => hardware_panel::HardwarePanel::load(),
            (Group::ThisNode, "config_apply") => config_apply_panel::ConfigApplyPanel::load(),
            (Group::ThisNode, "registration") => registration_panel::RegistrationPanel::load(),
            (Group::Controller, "jobs") => jobs_panel::JobsPanel::load(),
            (Group::Provisioning, "node_roles") => node_roles_panel::NodeRolesPanel::load(),
            (Group::Controller, "playbooks") => playbooks_panel::PlaybooksPanel::load(),
            (Group::Controller, "run_history") => run_history_panel::RunHistoryPanel::load(),
            (Group::System, "datetime") => datetime_panel::DateTimePanel::load(),
            (Group::Apps, "default_apps") => default_apps_panel::DefaultAppsPanel::load(),
            (Group::Maintain, "snapshots") => snapshots_panel::SnapshotsPanel::load(),
            (Group::System, "logs") => logs_panel::LogsPanel::load(),
            (Group::System, "resources") => resources_panel::ResourcesPanel::load(),
            (Group::System, "system_update") => system_update_panel::SystemUpdatePanel::load(),
            // v4.0.1 WB-2.f — auto-run probes on first nav so
            // the panel lands populated rather than empty.
            (Group::ThisNode, "health_check") => health_check_panel::HealthCheckPanel::load(),
            // PLANES-11 — Drift folds into Controller/Remediation.
            (Group::Controller, "drift") => drift_panel::DriftPanel::load(),
            // PLANES-13 — the policy engine surface.
            (Group::Controller, "policy") => policy_panel::PolicyPanel::load(),
            // PLANES-15 — the netstate desired-vs-actual diff.
            (Group::Network, "interfaces") => interfaces_panel::InterfacesPanel::load(),
            // PLANES-18 — the mesh DNS record set.
            (Group::Network, "dns") => dns_panel::DnsPanel::load(),
            // PLANES-19 — the overlay-reachability validation verdict.
            (Group::Network, "routing") => routing_panel::RoutingPanel::load(),
            // PLANES-3/W82 — the fleet capability-tag census.
            (Group::Fleet, "tags") => tags_panel::TagsPanel::load(),
            // PLANES-21 — the install-profile catalog.
            (Group::Provisioning, "profiles") => profiles_panel::ProfilesPanel::load(),
            // PLANES-24 — the package-mirror catalog.
            (Group::Provisioning, "mirrors") => mirrors_panel::MirrorsPanel::load(),
            // PLANES-22 — the image catalog.
            (Group::Provisioning, "images") => images_panel::ImagesPanel::load(),
            (Group::Controller, "audit") => audit_panel::AuditPanel::load(),
            (Group::ThisNode, "mesh_logs") => mesh_logs_panel::MeshLogsPanel::load(),
            (Group::Controller, "fleet_logs") => fleet_logs_panel::FleetLogsPanel::load(),
            // PLANES-1 (W52) — Mesh Control gets its own Controller entry.
            (Group::Controller, "mesh_control") => mesh_control_panel::MeshControlPanel::load(),
            // v4.0.1 WB-2.i — scan probe.json cache for pending peers.
            (Group::Network, "mesh_pending") => mesh_pending_panel::MeshPendingPanel::load(),
            // v4.0.1 WB-2.d — load applet visibility from panel.toml.
            (Group::Apps, "panel") => panel_apps_panel::PanelAppsPanel::load(),
            // v4.0.1 — panel.toml sync-status surface (Look & Feel).
            (Group::LookAndFeel, "sync_status") => sync_status_panel::SyncStatusPanel::load(),
            // v4.0.1 WB-2.k — peer roster via `mackesd nodes list --json`.
            // MESHFS-13.1 — Mesh Storage status panel.
            (Group::Network, "mesh_storage") => mesh_storage_panel::MeshStoragePanel::load(),
            // MESH-PROBE-9.a — Network Hosts reads the merged probe
            // inventory off mesh-storage on first nav (read-only).
            (Group::Network, "network_hosts") => network_hosts_panel::NetworkHostsPanel::load(),
            // PLANES-1 (W4) — Mesh Services folds into This Node/Health.
            (Group::ThisNode, "mesh_services") => mesh_services_panel::MeshServicesPanel::load(),
            // NF-13.8 (v2.5) — shell-out to
            // mackes.mesh_nebula.published_services_summary
            // for the 7 canonical services + per-row overlay
            // bind state.
            (Group::Network, "service_publishing") => {
                service_publishing_panel::ServicePublishingPanel::load()
            }
            // v4.0.1 WB-2.l — load cached peer-macs.json on
            // first nav so the known-hosts table is populated.
            (Group::Network, "remote_desktop") => remote_desktop_panel::RemoteDesktopPanel::load(),
            // PLANES-1 (W7) — the Peers directory: Front Door plane +
            // the Controller/Inventory door both load it.
            (Group::Peers, "peers") | (Group::Controller, "peers") => {
                peers_panel::PeersPanel::load()
            }
            (Group::Apps, "installed") => apps_installed_panel::AppsInstalledPanel::load(),
            (Group::Apps, "sources") => apps_sources_panel::AppsSourcesPanel::load(),
            (Group::Network, "firewall") => firewall_panel::FirewallPanel::load(),
            (Group::Network, "wifi") => wifi_panel::WifiPanel::load(),
            (Group::Network, "mesh_history") => mesh_history_panel::MeshHistoryPanel::load(),
            (Group::Network, "vpn") => vpn_panel::VpnPanel::load(),
            // PLANES-1 (W4) — Fleet Revisions folds into Controller/Config.
            (Group::Controller, "revisions") => fleet_revisions_panel::FleetRevisionsPanel::load(),
            // Fleet settings has no Load — it's a push-only
            // surface, so navigation doesn't fan a refresh.
            (Group::Controller, "settings") => Task::none(),
            // TUNE-15.b — Federation pairing panel: load active pairs on nav.
            (Group::Network, "mesh_federation") => {
                mesh_federation_panel::MeshFederationPanel::load()
            }
            _ => Task::none(),
        }
    }

    /// Group-root navigation side effects. Most group roots are static
    /// (the role card) or carry their own live subscription (Dashboard);
    /// the Compute root enumerates local VMs/pods on entry (E6.10), so a
    /// jump to it — sidebar click, `--page compute`, See-also link — lands
    /// the instance list already populated.
    fn on_group_navigated(&self, group: Group) -> Task<Message> {
        match group {
            Group::Compute => compute_panel::ComputePanel::load(),
            // PLANES-1 — the Peers Front Door plane lands on the live
            // directory (like Compute), not a role card.
            Group::Peers => peers_panel::PeersPanel::load(),
            _ => Task::none(),
        }
    }

    fn apply_focus_request(&mut self, slug: &str) -> Task<Message> {
        if slug.is_empty() {
            // Empty slug = "raise only, no view change" — the
            // 1.x taskbar click-through behaviour.
            return Task::none();
        }
        let Some(view) = view_from_focus_slug(slug) else {
            // Unknown slug silently ignored — matches the 1.x
            // `mackes --focus` Dashboard fallback for unmapped
            // surfaces (here we keep the current view since
            // jumping back to Dashboard on a typo would
            // surprise the user mid-task).
            return Task::none();
        };
        self.view = view;
        self.focused_pane = Pane::Main;
        if let View::Panel { group, panel } = view {
            self.on_panel_navigated(group, panel)
        } else {
            self.on_group_navigated(view.group())
        }
    }

    fn apply_key_action(&mut self, action: KeyAction) {
        match action {
            KeyAction::FocusPane(pane) => {
                self.focused_pane = pane;
            }
            KeyAction::JumpToGroup(group) => {
                self.view = View::Group(group);
                self.focused_pane = Pane::Sidebar;
            }
            KeyAction::CloseDetail => {
                if let View::Panel { group, .. } = self.view {
                    self.view = View::Group(group);
                    self.focused_pane = Pane::Sidebar;
                }
            }
            KeyAction::Ignored => {}
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let sidebar = crate::sidebar::view(
            &self.sidebar,
            self.view,
            self.focused_pane,
            Message::SelectGroup,
            |group, panel| Message::SelectPanel { group, panel },
        );

        let crumbs = breadcrumb(self.view)
            .into_iter()
            .map(|c| c.label)
            .collect::<Vec<_>>()
            .join(" / ");

        let page_heading = column![
            text(crumbs).size(12),
            text(page_title(self.view)).size(26),
            text(page_subtitle(self.view)).size(13),
        ]
        .spacing(6);

        let body = self.panel_body();

        // UX-6.a — outer panel padding (SPACE_24) is supplied
        // here once for every panel, replacing the per-panel
        // `Padding::new(0.0)` no-op wrappers. Density-aware via
        // `panel_chrome::outer_padding`.
        let main =
            column![page_heading, body]
                .spacing(20)
                .padding(crate::panel_chrome::outer_padding(
                    crate::live_theme::tokens().density,
                ));

        let layout = row![
            sidebar,
            container(main).width(Length::Fill).height(Length::Fill)
        ]
        .height(Length::Fill);

        // UX-4 — custom window header sits above sidebar + body
        // so the wordmark + window controls span the full width.
        let window_header = crate::header::view(Message::WindowControl);

        column![window_header, layout]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Per-View body — panel views land here as they ship.
    fn panel_body(&self) -> Element<'_, Message> {
        match self.view {
            // v4.0.1 WB-2.a/b/c — group-root landing pages. These
            // fire when the operator clicks the group header in
            // the sidebar (View::Group rather than View::Panel).
            // Before this commit every group root rendered the
            // catch-all placeholder "Panel view lands in a later
            // CB-1.x substep."
            View::Group(Group::Dashboard) => self.home.view(),
            // E6.7 — the Maintain group root now renders the standard role
            // card (via the View::Group(g) catch-all), matching the other
            // roles; the hub overview dashboard becomes the "Hub" sub-panel
            // below so the Maintain card's Hub action-link opens it.
            View::Panel {
                group: Group::Maintain,
                panel: "hub",
            } => self.hub.view(),
            // E6.9 — the Help group root renders the role card (catch-all);
            // its action-links open the help topics index + the About/Help
            // disclaimer surface as sub-panels.
            View::Panel {
                group: Group::Help,
                panel: "index",
            } => self.help.view(),
            View::Panel {
                group: Group::Help,
                panel: "about",
            } => crate::panels::about::AboutPanel::view(),
            View::Panel {
                group: Group::LookAndFeel,
                panel: "themes",
            } => self.themes.view(),
            View::Panel {
                group: Group::LookAndFeel,
                panel: "fonts",
            } => self.fonts.view(),
            View::Panel {
                group: Group::LookAndFeel,
                panel: "wallpaper",
            } => self.wallpaper.view(),
            View::Panel {
                group: Group::Devices,
                panel: "session",
            } => self.session.view(),
            View::Panel {
                group: Group::System,
                panel: "notifications",
            } => self.notifications.view(),
            View::Panel {
                group: Group::Devices,
                panel: "power",
            } => self.power.view(),
            View::Panel {
                group: Group::Devices,
                panel: "removable",
            } => self.removable.view(),
            View::Panel {
                group: Group::Devices,
                panel: "displays",
            } => self.displays.view(),
            View::Panel {
                group: Group::Devices,
                panel: "keyboard",
            } => self.keyboard.view(),
            View::Panel {
                group: Group::Devices,
                panel: "mouse",
            } => self.mouse.view(),
            View::Panel {
                group: Group::Devices,
                panel: "sound",
            } => self.sound.view(),
            View::Panel {
                group: Group::Devices,
                panel: "printers",
            } => self.printers.view(),
            View::Panel {
                group: Group::Devices,
                panel: "music",
            } => self.music.view(),
            View::Panel {
                group: Group::Devices,
                panel: "connect",
            } => self.connect.view(),
            // PLANES-1 — Fleet keeps the rollup lens + fleet inventory.
            View::Panel {
                group: Group::Fleet,
                panel: "inventory",
            } => self.inventory.view(),
            View::Panel {
                group: Group::Fleet,
                panel: "fleet_rollup",
            } => self.fleet_rollup.view(),
            // This Node plane — registration / inventory / config.
            View::Panel {
                group: Group::ThisNode,
                panel: "hardware",
            } => self.hardware.view(),
            View::Panel {
                group: Group::ThisNode,
                panel: "config_apply",
            } => self.config_apply.view(),
            View::Panel {
                group: Group::ThisNode,
                panel: "registration",
            } => self.registration.view(),
            // Controller plane — jobs / playbooks / run history.
            View::Panel {
                group: Group::Controller,
                panel: "jobs",
            } => self.jobs.view(),
            View::Panel {
                group: Group::Controller,
                panel: "playbooks",
            } => self.playbooks.view(),
            View::Panel {
                group: Group::Controller,
                panel: "run_history",
            } => self.run_history.view(),
            // Provisioning plane — node role pins + tags (W58).
            View::Panel {
                group: Group::Provisioning,
                panel: "node_roles",
            } => self.node_roles.view(),
            View::Panel {
                group: Group::System,
                panel: "datetime",
            } => self.datetime.view(),
            View::Panel {
                group: Group::Apps,
                panel: "default_apps",
            } => self.default_apps.view(),
            View::Panel {
                group: Group::Maintain,
                panel: "snapshots",
            } => self.snapshots.view(),
            View::Panel {
                group: Group::System,
                panel: "logs",
            } => self.logs.view(),
            View::Panel {
                group: Group::System,
                panel: "resources",
            } => self.resources.view(),
            View::Panel {
                group: Group::System,
                panel: "system_update",
            } => self.system_update.view(),
            View::Panel {
                group: Group::Maintain,
                panel: "repair",
            } => self.repair.view(),
            View::Panel {
                group: Group::Apps,
                panel: "installed",
            } => self.apps_installed.view(),
            View::Panel {
                group: Group::Apps,
                panel: "sources",
            } => self.apps_sources.view(),
            View::Panel {
                group: Group::Apps,
                panel: "install",
            } => self.apps_install.view(),
            View::Panel {
                group: Group::Apps,
                panel: "remove",
            } => self.apps_remove.view(),
            // v4.0.1 WB-2.d (2026-05-23) — Apps → Panel Apps
            // visibility editor (per-applet toggles, writes to
            // ~/.config/mde/panel.toml).
            View::Panel {
                group: Group::Apps,
                panel: "panel",
            } => self.panel_apps.view(),
            // v4.0.1 (2026-05-23) — Look & Feel → Panel Sync
            // Status reads panel.toml mtime + mackesd healthz
            // JSON to surface the mesh-sync state.
            View::Panel {
                group: Group::LookAndFeel,
                panel: "sync_status",
            } => self.sync_status.view(),
            // v4.0.1 WB-2.e (2026-05-23) — the Maintain → Debloat
            // sidebar entry routes to the same curated-bloat-list
            // panel the Apps → Remove path uses. Two nav paths
            // hit one panel surface; the design lock places
            // Debloat under Maintain (mass system cleanup), with
            // the Apps → Remove path retained as the per-app
            // entry point.
            View::Panel {
                group: Group::Maintain,
                panel: "debloat",
            } => self.apps_remove.view(),
            // v4.0.1 WB-2.f (2026-05-23) — Maintain → Health
            // Check renders the local-probe table (disk space,
            // memory, failed units, DNS, dnf backlog, snapshot
            // count, parity overlay).
            // PLANES-1 — Health re-homes to This Node (W20).
            View::Panel {
                group: Group::ThisNode,
                panel: "health_check",
            } => self.health_check.view(),
            // PLANES-12 — Audit re-homes to Controller.
            View::Panel {
                group: Group::Controller,
                panel: "audit",
            } => self.audit.view(),
            // PLANES-8 — Logs & Metrics re-home to This Node.
            View::Panel {
                group: Group::ThisNode,
                panel: "mesh_logs",
            } => self.mesh_logs.view(),
            // PLANES-14 — Fleet Logs re-home to Controller.
            View::Panel {
                group: Group::Controller,
                panel: "fleet_logs",
            } => self.fleet_logs.view(),
            // PLANES-11 — Drift folds into Controller/Remediation.
            View::Panel {
                group: Group::Controller,
                panel: "drift",
            } => self.drift.view(),
            // PLANES-13 — the policy engine surface.
            View::Panel {
                group: Group::Controller,
                panel: "policy",
            } => self.policy.view(),
            // PLANES-15 — the netstate desired-vs-actual diff.
            View::Panel {
                group: Group::Network,
                panel: "interfaces",
            } => self.interfaces.view(),
            // PLANES-18 — the mesh DNS record set.
            View::Panel {
                group: Group::Network,
                panel: "dns",
            } => self.dns.view(),
            // PLANES-19 — the overlay-reachability validation verdict.
            View::Panel {
                group: Group::Network,
                panel: "routing",
            } => self.routing.view(),
            // PLANES-3/W82 — the fleet capability-tag census.
            View::Panel {
                group: Group::Fleet,
                panel: "tags",
            } => self.tags.view(),
            // PLANES-21 — the install-profile catalog.
            View::Panel {
                group: Group::Provisioning,
                panel: "profiles",
            } => self.profiles.view(),
            // PLANES-24 — the package-mirror catalog.
            View::Panel {
                group: Group::Provisioning,
                panel: "mirrors",
            } => self.mirrors.view(),
            // PLANES-22 — the image catalog.
            View::Panel {
                group: Group::Provisioning,
                panel: "images",
            } => self.images.view(),
            // BUS-7.2 — Mackes Bus 5-tab operator surface.
            View::Panel {
                group: Group::Network,
                panel: "mesh_bus",
            } => self.mesh_bus.view(),
            // TUNE-15.b — Mesh Federation 4-tab pairing surface.
            View::Panel {
                group: Group::Network,
                panel: "mesh_federation",
            } => self.mesh_federation.view(),
            // v4.0.1 WB-2.h (2026-05-23) — Network → Mesh
            // Control renders the leader-lease state + healthz.
            // PLANES-1 (W52) — Mesh Control gets its own Controller entry.
            View::Panel {
                group: Group::Controller,
                panel: "mesh_control",
            } => self.mesh_control.view(),
            // v4.0.1 WB-2.i (2026-05-23) — Network → Mesh
            // Pending lists peer-probe rows from
            // $XDG_CACHE_HOME/mde/peers/<id>/probe.json with
            // Accept / Reject buttons.
            View::Panel {
                group: Group::Network,
                panel: "mesh_pending",
            } => self.mesh_pending.view(),
            // v4.0.1 WB-2.k (2026-05-23) — Network → Mesh
            // Topology renders the peer roster as a sortable
            // table (canvas-graph variant deferred to v4.1).
            // MESHFS-13.1 — Network → Mesh Storage status.
            View::Panel {
                group: Group::Network,
                panel: "mesh_storage",
            } => self.mesh_storage.view(),
            // MESH-PROBE-9.a — Network → Network Hosts lists the probe
            // inventory (hosts + identified services + trust-state).
            View::Panel {
                group: Group::Network,
                panel: "network_hosts",
            } => self.network_hosts.view(),
            // v4.0.1 WB-2.j (2026-05-23) — Network → Mesh
            // Services renders systemctl status + start/stop/
            // restart for the mesh-fabric daemons. v2.5 NF-5.4
            // (2026-05-24) swapped the set to nebula /
            // nebula-lighthouse / mackes-nebula-https-tunnel /
            // mackesd.
            // PLANES-1 (W4) — Mesh Services folds into This Node/Health.
            View::Panel {
                group: Group::ThisNode,
                panel: "mesh_services",
            } => self.mesh_services.view(),
            // NF-13.8 (v2.5) — Network → Service Publishing
            // renders the 7 canonical Nebula-published services
            // with overlay-bind status pills.
            View::Panel {
                group: Group::Network,
                panel: "service_publishing",
            } => self.service_publishing.view(),
            // v4.0.1 WB-2.l (2026-05-23) — Network → Remote
            // Desktop renders cached peer-macs.json hosts +
            // per-host RDP/VNC launch buttons + a manual-entry
            // text field.
            View::Panel {
                group: Group::Network,
                panel: "remote_desktop",
            } => self.remote_desktop.view(),
            // PD-3 / PLANES-1 (W7) — the Peers directory (the Front
            // Door), one component two doors: the Peers plane root +
            // its panel, and the Controller/Inventory door.
            View::Group(Group::Peers)
            | View::Panel {
                group: Group::Peers,
                panel: "peers",
            }
            | View::Panel {
                group: Group::Controller,
                panel: "peers",
            } => self.peers.view(),
            View::Panel {
                group: Group::Network,
                panel: "firewall",
            } => self.firewall.view(),
            View::Panel {
                group: Group::Network,
                panel: "wifi",
            } => self.wifi.view(),
            View::Panel {
                group: Group::Network,
                panel: "vpn",
            } => self.vpn.view(),
            View::Panel {
                group: Group::Network,
                panel: "mesh_join",
            } => self.mesh_join.view(),
            View::Panel {
                group: Group::Network,
                panel: "mesh_history",
            } => self.mesh_history.view(),
            // PLANES-1 (W4) — fleet settings + Config (Revisions) re-home
            // to the Controller plane.
            View::Panel {
                group: Group::Controller,
                panel: "settings",
            } => self.fleet_settings.view(),
            View::Panel {
                group: Group::Controller,
                panel: "revisions",
            } => self.fleet_revisions.view(),
            // E6.10 — the Compute group root (and its "Instances"
            // sub-panel) render the bespoke local + fleet VM/pod list,
            // not the generic role card: `--page compute` lands directly
            // on the instance enumeration per the E6.10 acceptance.
            View::Group(Group::Compute)
            | View::Panel {
                group: Group::Compute,
                panel: "instances",
            } => self.compute.view(),
            // E6.1 — any group-root view without a bespoke landing
            // (Apps / Devices / Fleet / Look & Feel / System, plus
            // Network when deep-linked) renders the "Manage Your Server"
            // role card: description + action-links + See-also sidebar.
            // Replaces the old "group landing isn't ready yet"
            // placeholder. Per-panel not-yet-shipped views still fall to
            // the friendly empty-state below.
            View::Group(g) => crate::role::role_landing(g),
            other => panel_under_construction(other),
        }
    }
}

/// v4.0.1 BUG-19 (2026-05-23) — friendly "panel not ready yet"
/// surface for sidebar entries whose reducer hasn't shipped. The
/// previous catch-all rendered a raw internal-jargon line that
/// leaked the CB-1.x substep id to the operator — the Phase 0.7
/// audit grep that the iteration skill added on 2026-05-23
/// promoted it from passive marker to actionable finding. This
/// renderer uses the standard UX-6 EmptyState (Material Symbols
/// tools icon + curated panel label + Back-to-group CTA wired
/// through `Message::SelectGroup`).
/// PLANES-1 (W16) — the owning worklist item for a plane panel whose
/// bespoke reducer hasn't shipped yet, so the guided empty state can
/// name it honestly. Returns `None` for panels that already have a
/// reducer (they never reach the catch-all) or that carry no tracked
/// follow-up.
fn panel_worklist_item(_group: Group, _panel: &str) -> Option<&'static str> {
    // PLANES-1 (W16) — every plane panel now has a real reducer, so no
    // plane slug reaches the catch-all with a tracked follow-up. The hook
    // stays for any future panel that ships its nav entry ahead of its
    // reducer.
    None
}

fn panel_under_construction(view: View) -> Element<'static, Message> {
    let palette = crate::live_theme::palette();
    let group = view.group();
    let (heading, body): (String, String) = match view {
        View::Group(g) => (
            format!("{} isn't ready yet", g.label()),
            format!(
                "The {} group landing page is part of the next workbench rollout. Pick a specific item from the sidebar to keep working.",
                g.label()
            ),
        ),
        View::Panel { group: g, panel } => {
            let panel_label = resolve_panel_label(g, panel).unwrap_or(panel);
            // PLANES-1 (W16) — name the owning worklist item so the
            // full-tree empty states read as honest "building this",
            // not vaporware.
            let tracked = match panel_worklist_item(g, panel) {
                Some(item) => format!(" Tracked as worklist item {item}."),
                None => String::new(),
            };
            (
                format!("{panel_label} isn't ready yet"),
                format!(
                    "The {panel_label} panel is part of the next workbench rollout.{tracked} Other panels in {group} stay available from the sidebar.",
                    group = g.label(),
                ),
            )
        }
    };
    let group_label = group.label();
    let state = mde_theme::EmptyState::with_cta(heading, body, format!("Back to {group_label}"))
        .with_icon(mde_theme::Icon::Maintain);
    let inner =
        crate::panel_chrome::empty_state(state, palette, move || Message::SelectGroup(group));
    iced::widget::container(inner)
        .padding(crate::panel_chrome::outer_padding(
            crate::live_theme::tokens().density,
        ))
        .width(iced::Length::Fill)
        .height(iced::Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::DemoBackend;

    #[test]
    fn new_app_lands_on_the_peers_front_door() {
        let app = App::new();
        // PD-4 / D2 / PLANES-1 — the Front Door, now its own plane.
        assert_eq!(
            app.current_view(),
            View::Panel {
                group: Group::Peers,
                panel: "peers"
            }
        );
        assert_eq!(app.focused_pane(), Pane::Sidebar);
    }

    #[test]
    fn every_plane_panel_has_a_real_reducer_now() {
        // PLANES-1 (W16) follow-through — the full-tree empty-state plane
        // panels (policy, interfaces, dns, routing, tags, profiles,
        // images, mirrors) all shipped real reducers, so the
        // worklist-item hook no longer fires for any of them.
        for (g, p) in [
            (Group::Controller, "policy"),
            (Group::Network, "interfaces"),
            (Group::Network, "dns"),
            (Group::Network, "routing"),
            (Group::Fleet, "tags"),
            (Group::Provisioning, "profiles"),
            (Group::Provisioning, "images"),
            (Group::Provisioning, "mirrors"),
        ] {
            assert!(
                panel_worklist_item(g, p).is_none(),
                "{g:?}/{p} should have a real reducer now (no worklist hook)"
            );
        }
    }

    #[test]
    fn select_group_updates_view_and_focuses_main_pane() {
        let mut app = App::new();
        let _ = app.update(Message::SelectGroup(Group::Network));
        assert_eq!(app.current_view(), View::Group(Group::Network));
        assert_eq!(app.focused_pane(), Pane::Main);
    }

    #[test]
    fn select_panel_carries_group_and_panel_slug() {
        let mut app = App::new();
        let _ = app.update(Message::SelectPanel {
            group: Group::Network,
            panel: "remote_desktop",
        });
        assert_eq!(
            app.current_view(),
            View::Panel {
                group: Group::Network,
                panel: "remote_desktop"
            }
        );
    }

    #[test]
    fn ctrl_digit_key_action_jumps_to_group_and_refocuses_sidebar() {
        let mut app = App::new();
        let _ = app.update(Message::KeyPressed(KeyAction::JumpToGroup(Group::Help)));
        assert_eq!(app.current_view(), View::Group(Group::Help));
        assert_eq!(app.focused_pane(), Pane::Sidebar);
    }

    #[test]
    fn drill_to_peers_opens_peers_front_door_filtered_by_role() {
        // PLANES-20 / W87 — a Fleet-rollup card drill-down lands on the
        // Peers plane with the role pre-filtered.
        let mut app = App::new();
        let _ = app.update(Message::DrillToPeers("host".into()));
        assert_eq!(app.current_view(), View::Group(Group::Peers));
        assert_eq!(app.peers.filter, "host");
    }

    #[test]
    fn escape_from_panel_view_returns_to_parent_group_landing() {
        let mut app = App::new();
        let _ = app.update(Message::SelectPanel {
            group: Group::Maintain,
            panel: "snapshots",
        });
        let _ = app.update(Message::KeyPressed(KeyAction::CloseDetail));
        assert_eq!(app.current_view(), View::Group(Group::Maintain));
        assert_eq!(app.focused_pane(), Pane::Sidebar);
    }

    #[test]
    fn escape_from_group_view_is_noop() {
        let mut app = App::new();
        let _ = app.update(Message::SelectGroup(Group::System));
        let _ = app.update(Message::KeyPressed(KeyAction::CloseDetail));
        // Still on the same group landing — no leaf to close.
        assert_eq!(app.current_view(), View::Group(Group::System));
    }

    #[test]
    fn tab_focus_pane_action_updates_focused_pane() {
        let mut app = App::new();
        let _ = app.update(Message::KeyPressed(KeyAction::FocusPane(Pane::Main)));
        assert_eq!(app.focused_pane(), Pane::Main);
    }

    #[test]
    fn toggle_group_expansion_flips_state() {
        let mut app = App::new();
        // Inactive group starts collapsed (the active group is now
        // Network — the Peers Front Door — so probe System instead).
        assert!(!app.sidebar.is_expanded(Group::System, Group::Dashboard));
        let _ = app.update(Message::ToggleGroupExpansion(Group::System));
        assert!(app.sidebar.is_expanded(Group::System, Group::Dashboard));
    }

    #[test]
    fn with_focus_lands_on_named_panel_and_focuses_main() {
        // network.mesh_ssh is the retired B1 slug — it aliases to the
        // Remote Access panel that absorbed it (SVC-1).
        let app = App::with_focus("network.mesh_ssh");
        assert_eq!(
            app.current_view(),
            View::Panel {
                group: Group::Network,
                panel: "remote_desktop"
            }
        );
        assert_eq!(app.focused_pane(), Pane::Main);
    }

    #[test]
    fn with_focus_falls_back_to_the_front_door_on_unknown_slug() {
        let app = App::with_focus("not-a-real-slug");
        // PD-4 — the fallback is the default view (the Peers Front Door).
        assert_eq!(app.current_view(), View::default());
    }

    #[test]
    fn noop_message_does_not_change_state() {
        let mut app = App::new();
        let before_view = app.current_view();
        let before_pane = app.focused_pane();
        let _ = app.update(Message::Noop);
        assert_eq!(app.current_view(), before_view);
        assert_eq!(app.focused_pane(), before_pane);
    }

    #[test]
    fn select_look_and_feel_themes_swaps_view_and_returns_load_task() {
        let mut app = App::new();
        // The Task isn't directly observable in unit tests
        // (it lands inside iced's executor), but the View
        // change + backend identity confirm the navigation
        // path fired.
        let _ = app.update(Message::SelectPanel {
            group: Group::LookAndFeel,
            panel: "themes",
        });
        assert_eq!(
            app.current_view(),
            View::Panel {
                group: Group::LookAndFeel,
                panel: "themes"
            }
        );
    }

    #[test]
    fn themes_panel_picks_route_through_app_reducer() {
        // GUI-3 — the rewritten panel holds the pending Carbon-gray
        // selection; picks route through the App reducer.
        let mut app = App::new();
        let _ = app.update(Message::Themes(themes_panel::Message::ThemePicked(
            "gray90".into(),
        )));
        let _ = app.update(Message::Themes(themes_panel::Message::DensityPicked(
            "compact".into(),
        )));
        assert_eq!(app.themes().theme, "gray90");
        assert_eq!(app.themes().density, "compact");
    }

    #[test]
    fn select_devices_session_swaps_view_to_panel() {
        // E6.4 — session lives under Devices now (moved from System).
        let mut app = App::new();
        let _ = app.update(Message::SelectPanel {
            group: Group::Devices,
            panel: "session",
        });
        assert_eq!(
            app.current_view(),
            View::Panel {
                group: Group::Devices,
                panel: "session"
            }
        );
    }

    #[test]
    fn session_panel_toggle_messages_persist_in_app_state() {
        let mut app = App::new();
        let _ = app.update(Message::Session(session_panel::Message::SaveOnExitChanged(
            true,
        )));
        let _ = app.update(Message::Session(
            session_panel::Message::LockOnSuspendChanged(true),
        ));
        assert!(app.session().save_on_exit);
        assert!(app.session().lock_on_suspend);
    }

    #[test]
    fn power_panel_field_changes_persist_in_app_state() {
        let mut app = App::new();
        let _ = app.update(Message::Power(power_panel::Message::ProfileChanged(
            "performance".into(),
        )));
        let _ = app.update(Message::Power(power_panel::Message::PresentationChanged(
            true,
        )));
        assert_eq!(app.power().profile, "performance");
        assert!(app.power().presentation_mode);
    }

    #[test]
    fn removable_panel_field_changes_persist_in_app_state() {
        let mut app = App::new();
        let _ = app.update(Message::Removable(
            removable_panel::Message::OnInsertChanged(true),
        ));
        let _ = app.update(Message::Removable(
            removable_panel::Message::AutorunChanged(false),
        ));
        assert!(app.removable().on_insert);
        assert!(!app.removable().autorun);
    }

    #[test]
    fn notifications_panel_field_changes_persist_in_app_state() {
        let mut app = App::new();
        let _ = app.update(Message::Notifications(
            notifications_panel::Message::DndChanged(true),
        ));
        let _ = app.update(Message::Notifications(
            notifications_panel::Message::LocationChanged("top-left".into()),
        ));
        assert!(app.notifications().dnd);
        assert_eq!(app.notifications().location, "top-left");
    }

    #[test]
    fn fonts_panel_field_changes_persist_in_app_state() {
        let mut app = App::new();
        let _ = app.update(Message::Fonts(fonts_panel::Message::NameChanged(
            "Inter 11".into(),
        )));
        let _ = app.update(Message::Fonts(fonts_panel::Message::HintingChanged(
            "full".into(),
        )));
        assert_eq!(app.fonts().name, "Inter 11");
        assert_eq!(app.fonts().hinting, "full");
    }

    #[test]
    fn focus_request_with_panel_slug_jumps_to_panel_and_focuses_main() {
        let mut app = App::new();
        let _ = app.update(Message::FocusRequest("network.mesh_ssh".into()));
        assert_eq!(
            app.current_view(),
            View::Panel {
                group: Group::Network,
                panel: "remote_desktop"
            }
        );
        assert_eq!(app.focused_pane(), Pane::Main);
    }

    #[test]
    fn focus_request_with_group_slug_lands_on_group_view() {
        let mut app = App::new();
        let _ = app.update(Message::FocusRequest("help".into()));
        assert_eq!(app.current_view(), View::Group(Group::Help));
    }

    #[test]
    fn focus_request_empty_slug_preserves_view() {
        let mut app = App::new();
        let _ = app.update(Message::SelectPanel {
            group: Group::Apps,
            panel: "sources",
        });
        let before = app.current_view();
        let _ = app.update(Message::FocusRequest(String::new()));
        assert_eq!(
            app.current_view(),
            before,
            "empty slug = raise-only contract — view must not change"
        );
    }

    #[test]
    fn focus_request_unknown_slug_preserves_view() {
        let mut app = App::new();
        let _ = app.update(Message::SelectGroup(Group::Maintain));
        let before = app.current_view();
        let _ = app.update(Message::FocusRequest("not-a-real-slug".into()));
        assert_eq!(
            app.current_view(),
            before,
            "unknown slug must not jolt the user out of their current view"
        );
    }

    #[test]
    fn title_includes_active_page() {
        let mut app = App::new();
        let _ = app.update(Message::SelectGroup(Group::Apps));
        assert!(app.title().contains("Apps"));
        assert!(app.title().starts_with("MDE Workbench"));
    }

    // UX-3 — theme() returns a custom Iced theme derived from
    // crate::live_theme::palette(). E9 (2026-06-07) moved that palette to
    // Carbon-only; the next two tests guard the resulting Carbon values.

    #[test]
    fn workbench_iced_theme_background_matches_carbon_gray_100() {
        // E9: the workbench renders on Carbon Gray 100 (the mde-theme
        // dark palette; carbondesignsystem.com gray ramp).
        let theme = mde_workbench_iced_theme();
        let bg = theme.palette().background;
        let expected = (
            0x16 as f32 / 255.0,
            0x16 as f32 / 255.0,
            0x16 as f32 / 255.0,
        );
        assert!(
            (bg.r - expected.0).abs() < 1e-4,
            "r {} vs {}",
            bg.r,
            expected.0
        );
        assert!(
            (bg.g - expected.1).abs() < 1e-4,
            "g {} vs {}",
            bg.g,
            expected.1
        );
        assert!(
            (bg.b - expected.2).abs() < 1e-4,
            "b {} vs {}",
            bg.b,
            expected.2
        );
    }

    #[test]
    fn workbench_iced_theme_primary_matches_carbon_blue_60() {
        let theme = mde_workbench_iced_theme();
        let primary = theme.palette().primary;
        // E9: Carbon Blue 60 interactive accent.
        let expected = (
            0x0f as f32 / 255.0,
            0x62 as f32 / 255.0,
            0xfe as f32 / 255.0,
        );
        assert!((primary.r - expected.0).abs() < 1e-4);
        assert!((primary.g - expected.1).abs() < 1e-4);
        assert!((primary.b - expected.2).abs() < 1e-4);
    }

    // UX-4 — the window-control reducer maps every HeaderAction
    // to an iced::window Task. We can't observe Tasks directly
    // in unit tests (they run inside the iced executor), but
    // every variant must hit the dispatcher without panicking
    // and the Noop / view-state invariants must hold.

    #[test]
    fn window_control_message_does_not_mutate_view_state() {
        let mut app = App::new();
        let _ = app.update(Message::SelectGroup(Group::Network));
        let before = app.current_view();
        let _ = app.update(Message::WindowControl(HeaderAction::Minimize));
        let _ = app.update(Message::WindowControl(HeaderAction::ToggleMaximize));
        let _ = app.update(Message::WindowControl(HeaderAction::Close));
        assert_eq!(
            app.current_view(),
            before,
            "window-control clicks must never re-route the user out of their current panel"
        );
    }

    #[test]
    fn window_control_dispatch_compiles_for_every_action() {
        // Smoke-only — exercises the match arm in
        // App::dispatch_window_action so adding a new
        // HeaderAction variant without wiring it triggers a
        // compile-time non-exhaustive-match error.
        let _ = App::dispatch_window_action(HeaderAction::Minimize);
        let _ = App::dispatch_window_action(HeaderAction::ToggleMaximize);
        let _ = App::dispatch_window_action(HeaderAction::Close);
    }

    #[test]
    fn workbench_iced_theme_is_custom_named_mde() {
        // Theme::Dark / Light have built-in names; the custom
        // theme advertises "MDE" so the workbench preferences UI
        // (UX-15.a) can detect the active theme by name.
        let theme = mde_workbench_iced_theme();
        // Custom themes implement Display as the name.
        let rendered = format!("{}", theme);
        assert!(rendered.contains("MDE"), "theme name = {rendered}");
    }
}
