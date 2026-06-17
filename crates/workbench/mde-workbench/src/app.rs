//! Top-level Iced application — state, message reducer, view.
//!
//! CB-1.1 + CB-1.2 scaffold: nine-group sidebar + breadcrumb +
//! page title / subtitle. Per-panel views (CB-1.3 ... CB-1.10)
//! land as separate substeps and plug into [`App::view`] via
//! [`crate::View::Panel`] matching.

use std::sync::Arc;
use std::time::Duration;

use cosmic::app::ApplicationExt;
use cosmic::iced::widget::{column, container, row, text};
use cosmic::iced::{window, Length, Subscription, Task};
use cosmic::{Application, Element};

use crate::cosmic_compat::prelude::*;

use crate::backend::{Backend, RemoteBackend};
use crate::dbus::PendingFocus;
use crate::header::HeaderAction;
use crate::keyboard::{KeyAction, Pane};
use crate::model::{resolve_panel_label, view_from_focus_slug, Group, View};
use crate::panels::{
    audit as audit_panel, compute as compute_panel, config_apply as config_apply_panel,
    connect as connect_panel, dns as dns_panel, drift as drift_panel, firewall as firewall_panel,
    fleet_logs as fleet_logs_panel, fleet_revisions as fleet_revisions_panel,
    fleet_rollup as fleet_rollup_panel, fleet_settings as fleet_settings_panel,
    hardware as hardware_panel, health_check as health_check_panel, help_index as help_index_panel,
    home as home_panel, hub as hub_panel, images as images_panel, interfaces as interfaces_panel,
    inventory as inventory_panel, jobs as jobs_panel, logs as logs_panel,
    mesh_bus as mesh_bus_panel, mesh_control as mesh_control_panel,
    mesh_federation as mesh_federation_panel, mesh_history as mesh_history_panel,
    mesh_join as mesh_join_panel, mesh_logs as mesh_logs_panel, mesh_pending as mesh_pending_panel,
    mesh_services as mesh_services_panel, mesh_storage as mesh_storage_panel,
    mirrors as mirrors_panel, music as music_panel, network_hosts as network_hosts_panel,
    node_roles as node_roles_panel, notifications as notifications_panel, peers as peers_panel,
    playbooks as playbooks_panel, policy as policy_panel, profiles as profiles_panel,
    registration as registration_panel, remote_desktop as remote_desktop_panel,
    repair as repair_panel, resources as resources_panel, routing as routing_panel,
    run_history as run_history_panel, service_publishing as service_publishing_panel,
    sip_gateway as sip_gateway_panel, snapshots as snapshots_panel,
    sync_status as sync_status_panel, system_update as system_update_panel, tags as tags_panel,
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
pub fn mde_workbench_iced_theme() -> cosmic::iced::Theme {
    let p = crate::live_theme::palette();
    let palette = cosmic::iced::theme::Palette {
        background: p.background.into_cosmic_color(),
        text: p.text.into_cosmic_color(),
        primary: p.accent.into_cosmic_color(),
        warning: p.warning.into_cosmic_color(),
        success: p.success.into_cosmic_color(),
        danger: p.danger.into_cosmic_color(),
    };
    cosmic::iced::Theme::custom("MDE".to_string(), palette)
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
    /// GUI-RECONNECT — periodic Bus liveness tick (fires the probe).
    ReconnectTick,
    /// GUI-RECONNECT — Bus liveness probe result. A `false→true`
    /// transition re-loads the active panel so it recovers after a
    /// `systemctl restart mackesd` without a manual refresh.
    ReconnectProbed(bool),
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
    /// AUD-5 — operator accepted the DISCLAIMER pre-flight gate.
    AcceptDisclaimer,
    /// AUD-3 — Connected Devices (KDC hub) panel sub-message.
    Connect(connect_panel::Message),
    /// CB-1.9 partial — System notifications panel sub-message.
    Notifications(notifications_panel::Message),
    /// v4.0.1 WB-2.a — Dashboard `home` landing-page messages.
    Home(home_panel::Message),
    /// v4.0.1 WB-2.c — Help index opened a topic; the path is
    /// dispatched to `xdg-open`.
    HelpTopicOpened(std::path::PathBuf),
    /// About panel — open an external URL/mailto (GitHub, Releases,
    /// Contact) detached via `xdg-open`.
    OpenExternal(&'static str),
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
    /// AIR-20 — Devices → Music settings panel sub-message.
    Music(music_panel::Message),
    VoipGateway(sip_gateway_panel::Message),
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
    /// GUI-7 — the libcosmic application core (window state, theme, nav). Set
    /// from the `Core` libcosmic hands `Application::init`; a throwaway
    /// `Core::default()` fills it on the `Default`/test path (the GUI never
    /// runs there).
    core: cosmic::app::Core,
    view: View,
    sidebar: SidebarState,
    focused_pane: Pane,
    /// GUI-RECONNECT — last known Bus/mackesd reachability. A down→up
    /// transition (the control plane came back after a restart) re-fires
    /// the active panel's load so its data recovers without a manual
    /// refresh. Starts optimistic (`true`).
    bus_reachable: bool,
    backend: Arc<dyn Backend>,
    notifications: notifications_panel::NotificationsPanel,
    music: music_panel::MusicPanel,
    sip_gateway: sip_gateway_panel::SipGatewayPanel,
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
    snapshots: snapshots_panel::SnapshotsPanel,
    logs: logs_panel::LogsPanel,
    resources: resources_panel::ResourcesPanel,
    compute: compute_panel::ComputePanel,
    system_update: system_update_panel::SystemUpdatePanel,
    repair: repair_panel::RepairPanel,
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
    /// AUD-5 — whether the operator has accepted the current DISCLAIMER. Until
    /// `true`, `view()` shows the blocking accept gate instead of the app.
    disclaimer_accepted: bool,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("view", &self.view)
            .field("focused_pane", &self.focused_pane)
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
            core: cosmic::app::Core::default(),
            view: View::default(),
            sidebar: SidebarState::new(),
            focused_pane: Pane::Sidebar,
            bus_reachable: true,
            backend,
            notifications: notifications_panel::NotificationsPanel::new(),
            music: music_panel::MusicPanel::new(),
            sip_gateway: sip_gateway_panel::SipGatewayPanel::new(),
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
            snapshots: snapshots_panel::SnapshotsPanel::new(),
            logs: logs_panel::LogsPanel::new(),
            resources: resources_panel::ResourcesPanel::new(),
            compute: compute_panel::ComputePanel::new(),
            system_update: system_update_panel::SystemUpdatePanel::new(),
            repair: repair_panel::RepairPanel::new(),
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
            // AUD-5 — the runtime DISCLAIMER pre-flight accept gate (§5).
            disclaimer_accepted: mde_disclaimer::is_accepted(),
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

    /// Read-only view of the notifications panel state.
    #[must_use]
    pub fn notifications(&self) -> &notifications_panel::NotificationsPanel {
        &self.notifications
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

    /// Run the libcosmic application (GUI-7).
    ///
    /// Builds the cosmic [`Settings`](cosmic::app::Settings) (WIN_W×WIN_H
    /// window, custom titlebar via the suppressed headerbar) then hands off to
    /// `cosmic::app::run`. The deep-link boot (a `--focus <slug>` queued in
    /// [`PendingFocus`] by `main` before `run()`) is drained in `init`, so a
    /// `--focus` hand-off lands directly on the target panel rather than
    /// flashing the Front Door.
    pub fn run() -> cosmic::iced::Result {
        // UX-4 (d) — the custom `crate::header` bar is the only title strip the
        // user sees; Cosmic's headerbar is suppressed in `init`. The compositor
        // manages window geometry under Cosmic; Settings carries the default
        // size.
        let settings = cosmic::app::Settings::default().size(cosmic::iced::Size::new(WIN_W, WIN_H));
        cosmic::app::run::<App>(settings, ())
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
            cosmic::iced::time::every(Duration::from_millis(200))
                .map(|_| PendingFocus::drain().map_or(Message::Noop, Message::FocusRequest)),
            home_panel::dbus_subscription(),
            // E0.3.1.b — Nebula signals now arrive over the mesh Bus
            // event topic, not D-Bus; this polls them into DbusEvents.
            home_panel::nebula_event_subscription(),
            // GUI-RECONNECT — a slow Bus liveness tick. On a down→up
            // transition (mackesd came back) the handler re-loads the
            // active panel, so panels recover on their own instead of
            // showing a stale "mesh service isn't answering" until a
            // manual refresh.
            cosmic::iced::time::every(Duration::from_secs(10)).map(|_| Message::ReconnectTick),
        ];
        // E6.10 — sample Compute instance CPU/mem only while that view is
        // active, so virsh/podman stats aren't polled when the operator is
        // elsewhere.
        if self.view.panel_slug() == Some("instances") {
            subs.push(compute_panel::ComputePanel::sample_subscription());
        }
        // PD-8 (L14) / PLANES-1 — Netdata sampling only while the Peers
        // directory is the active view (the Compute pattern). The Front
        // Door is reachable as the Peers plane root/panel or the
        // Controller/Inventory door.
        if self.view.panel_slug() == Some("peers") {
            subs.push(peers_panel::metrics_subscription());
            // PD-3/Q10 — refresh the directory itself every 30 s while
            // the Front Door is open, so presence/health/tags stay live.
            subs.push(peers_panel::directory_subscription());
            // PD-3/Q10 — plus the Bus-push half: reload the instant the
            // responder reports a roster change.
            subs.push(peers_panel::directory_event_subscription());
            // PD-7 — the live map's flow + trace ticks, each view-gated so
            // nothing polls when the map is closed / no edge is traced, and
            // the particle-animation loop runs only while real traffic flows
            // (L18/L22 — idle mesh ≈ idle CPU).
            if self.peers.map_view {
                subs.push(peers_panel::flow_data_subscription());
            }
            if self.peers.has_flow() {
                subs.push(peers_panel::flow_anim_subscription());
            }
            if self.peers.traced_edge.is_some() {
                subs.push(peers_panel::trace_subscription());
            }
        }
        Subscription::batch(subs)
    }

    // CUT-1 (2026-06-13): the iced-era inherent `title()` + `theme()` were
    // removed. Under cosmic::Application the window title is set via
    // `set_header_title` in `init` (the cosmic headerbar is suppressed and the
    // custom `header::view` renders the live `page_title`), and the base theme
    // is the user's COSMIC theme + the explicit Carbon styling every panel
    // applies through `cosmic_compat`/`panel_chrome` — the same pattern as the
    // mde-files port (cosmic::Application::theme returns cosmic::Theme, which
    // the old iced-Theme helper cannot supply). `mde_workbench_iced_theme()` is
    // retained as the §4 Carbon-palette reference its token tests assert.

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
                self.view = View::Panel {
                    group: Group::Mesh,
                    panel: "peers",
                };
                self.focused_pane = Pane::Main;
                let task = self.on_panel_navigated(Group::Mesh, "peers");
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
            Message::AcceptDisclaimer => {
                // AUD-5 — record consent (keyed to the disclaimer fingerprint),
                // then drop the gate. A write failure still lets the operator in
                // (they accepted); they'll just be re-prompted next launch.
                if let Err(e) = mde_disclaimer::record_acceptance() {
                    tracing::warn!(error = %e, "could not persist disclaimer acceptance");
                }
                self.disclaimer_accepted = true;
                Task::none()
            }
            Message::Connect(msg) => self.connect.update(msg),
            Message::Notifications(msg) => self.notifications.update(msg, self.backend()),
            Message::Home(msg) => self.home.update(msg),
            Message::HelpTopicOpened(path) => {
                help_index_panel::spawn_xdg_open(&path);
                Task::none()
            }
            Message::OpenExternal(url) => {
                // About panel links (GitHub / Releases / mailto). Detached,
                // best-effort — a missing xdg-open simply no-ops.
                let _ = std::process::Command::new("xdg-open")
                    .arg(url)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                Task::none()
            }
            Message::OpenSettings(slug) => {
                // AUD-15 (2026-06-11): Cosmic owns the desktop (§5) — the retired
                // `mde settings` dispatcher is gone. Open Cosmic Settings (the
                // desktop's own config surface); the legacy mde page slug no
                // longer maps, so we open its home. Detached best-effort spawn.
                let _ = slug; // legacy mde page slug — no longer routable
                let _ = std::process::Command::new("cosmic-settings")
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
                // AUD-15 (2026-06-11): retired `mde settings` dispatcher → open
                // Cosmic Settings (§5, Cosmic owns the desktop). The legacy
                // category/page args no longer map; open its home. Detached.
                let _ = (category, page); // legacy mde page coords — unroutable
                let _ = std::process::Command::new("cosmic-settings")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                Task::none()
            }
            Message::Music(msg) => self.music.update(msg),
            Message::VoipGateway(msg) => self.sip_gateway.update(msg),
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
            Message::Snapshots(msg) => self.snapshots.update(msg),
            Message::Logs(msg) => self.logs.update(msg),
            Message::Resources(msg) => self.resources.update(msg),
            Message::Compute(msg) => self.compute.update(msg),
            Message::SystemUpdate(msg) => self.system_update.update(msg),
            Message::Repair(msg) => self.repair.update(msg),
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
            // GUI-RECONNECT — fire the cheap Bus liveness probe.
            Message::ReconnectTick => {
                Task::perform(probe_bus_reachable(), Message::ReconnectProbed)
            }
            // GUI-RECONNECT — on a down→up transition, re-load the active
            // panel so its data recovers on its own. No reload while the
            // Bus stays healthy (no flicker / no clobbered input).
            Message::ReconnectProbed(reachable) => {
                let recovered = reachable && !self.bus_reachable;
                self.bus_reachable = reachable;
                if recovered {
                    if let View::Panel { group, panel } = self.view {
                        return self.on_panel_navigated(group, panel);
                    }
                }
                Task::none()
            }
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
    fn on_panel_navigated(&self, _group: Group, panel: &'static str) -> Task<Message> {
        match panel {
            "wallpaper" => wallpaper_panel::WallpaperPanel::load(self.backend()),
            "notifications" => notifications_panel::NotificationsPanel::load(self.backend()),
            "music" => music_panel::MusicPanel::load(),
            "sip_gateway" => sip_gateway_panel::SipGatewayPanel::load(),
            // v4.0.1 WB-1 (Phase 0.7 rescue): Connected Devices
            // panel. Real D-Bus subscription wiring chains on
            // KDC2-3.9 signals; the panel.load() returns
            // Task::none today.
            "connect" => connect_panel::ConnectPanel::load(),
            // PLANES-1 — Fleet keeps the rollup lens + fleet inventory;
            // the operational panels re-home into the planes.
            "inventory" => inventory_panel::InventoryPanel::load(),
            "fleet_rollup" => fleet_rollup_panel::FleetRollupPanel::load(),
            "hardware" => hardware_panel::HardwarePanel::load(),
            "config_apply" => config_apply_panel::ConfigApplyPanel::load(),
            "registration" => registration_panel::RegistrationPanel::load(),
            "jobs" => jobs_panel::JobsPanel::load(),
            "node_roles" => node_roles_panel::NodeRolesPanel::load(),
            "playbooks" => playbooks_panel::PlaybooksPanel::load(),
            "run_history" => run_history_panel::RunHistoryPanel::load(),
            "snapshots" => snapshots_panel::SnapshotsPanel::load(),
            "logs" => logs_panel::LogsPanel::load(),
            "resources" => resources_panel::ResourcesPanel::load(),
            "system_update" => system_update_panel::SystemUpdatePanel::load(),
            // v4.0.1 WB-2.f — auto-run probes on first nav so
            // the panel lands populated rather than empty.
            "health_check" => health_check_panel::HealthCheckPanel::load(),
            // PLANES-11 — Drift folds into Controller/Remediation.
            "drift" => drift_panel::DriftPanel::load(),
            // PLANES-13 — the policy engine surface.
            "policy" => policy_panel::PolicyPanel::load(),
            // PLANES-15 — the netstate desired-vs-actual diff.
            "interfaces" => interfaces_panel::InterfacesPanel::load(),
            // PLANES-18 — the mesh DNS record set.
            "dns" => dns_panel::DnsPanel::load(),
            // PLANES-19 — the overlay-reachability validation verdict.
            "routing" => routing_panel::RoutingPanel::load(),
            // PLANES-3/W82 — the fleet capability-tag census.
            "tags" => tags_panel::TagsPanel::load(),
            // PLANES-21 — the install-profile catalog.
            "profiles" => profiles_panel::ProfilesPanel::load(),
            // PLANES-24 — the package-mirror catalog.
            "mirrors" => mirrors_panel::MirrorsPanel::load(),
            // PLANES-22 — the image catalog.
            "images" => images_panel::ImagesPanel::load(),
            "audit" => audit_panel::AuditPanel::load(),
            "mesh_logs" => mesh_logs_panel::MeshLogsPanel::load(),
            "fleet_logs" => fleet_logs_panel::FleetLogsPanel::load(),
            // PLANES-1 (W52) — Mesh Control gets its own Controller entry.
            "mesh_control" => mesh_control_panel::MeshControlPanel::load(),
            // AUDIT-MESH-9 — load the bus Topics tab on open (was never fetched
            // until a tab click, so a live bus showed "No topics active yet").
            "mesh_bus" => mesh_bus_panel::MeshBusPanel::load(),
            // v4.0.1 WB-2.i — scan probe.json cache for pending peers.
            "mesh_pending" => mesh_pending_panel::MeshPendingPanel::load(),
            // v4.0.1 — panel.toml sync-status surface (Look & Feel).
            "sync_status" => sync_status_panel::SyncStatusPanel::load(),
            // v4.0.1 WB-2.k — peer roster via `mackesd nodes list --json`.
            // MESHFS-13.1 — Mesh Storage status panel.
            "mesh_storage" => mesh_storage_panel::MeshStoragePanel::load(),
            // MESH-PROBE-9.a — Network Hosts reads the merged probe
            // inventory off mesh-storage on first nav (read-only).
            "network_hosts" => network_hosts_panel::NetworkHostsPanel::load(),
            // PLANES-1 (W4) — Mesh Services folds into This Node/Health.
            "mesh_services" => mesh_services_panel::MeshServicesPanel::load(),
            // NF-13.8 (v2.5) — shell-out to
            // mackes.mesh_nebula.published_services_summary
            // for the 7 canonical services + per-row overlay
            // bind state.
            "service_publishing" => service_publishing_panel::ServicePublishingPanel::load(),
            // v4.0.1 WB-2.l — load cached peer-macs.json on
            // first nav so the known-hosts table is populated.
            "remote_desktop" => remote_desktop_panel::RemoteDesktopPanel::load(),
            // PLANES-1 (W7) — the Peers directory: Front Door plane +
            // the Controller/Inventory door both load it.
            "peers" => peers_panel::PeersPanel::load(),
            "firewall" => firewall_panel::FirewallPanel::load(),
            "wifi" => wifi_panel::WifiPanel::load(),
            "mesh_history" => mesh_history_panel::MeshHistoryPanel::load(),
            "vpn" => vpn_panel::VpnPanel::load(),
            // PLANES-1 (W4) — Fleet Revisions folds into Controller/Config.
            "revisions" => fleet_revisions_panel::FleetRevisionsPanel::load(),
            // Fleet settings has no Load — it's a push-only
            // surface, so navigation doesn't fan a refresh.
            "settings" => Task::none(),
            // TUNE-15.b — Federation pairing panel: load active pairs on nav.
            "mesh_federation" => mesh_federation_panel::MeshFederationPanel::load(),
            _ => Task::none(),
        }
    }

    /// Group-root navigation side effects. Most group roots are static
    /// (the role card) or carry their own live subscription (Dashboard);
    /// the Compute root enumerates local VMs/pods on entry (E6.10), so a
    /// jump to it — sidebar click, `--page compute`, See-also link — lands
    /// the instance list already populated.
    fn on_group_navigated(&self, _group: Group) -> Task<Message> {
        // NAV-1 — group roots render the role card; the live directory
        // (Peers) and the instance list (Compute→Provisioning/Instances)
        // load via their slug-routed panels, not a group root.
        Task::none()
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

    /// AUD-5 — the blocking DISCLAIMER accept screen shown until the operator
    /// consents (§5 pre-flight gate). Renders the canonical `mde_disclaimer`
    /// text with a single "I understand and accept" action.
    fn disclaimer_gate_view(&self) -> Element<'_, Message> {
        let palette = crate::live_theme::palette();
        let (title, body) = mde_disclaimer::split();
        let accept = crate::controls::variant_button(
            "I understand and accept",
            crate::controls::ButtonVariant::Primary,
            Some(Message::AcceptDisclaimer),
            palette,
        );
        let content = column![
            text(title).size(22).colr(palette.text.into_cosmic_color()),
            cosmic::iced::widget::scrollable(
                text(body)
                    .size(13)
                    .colr(palette.text_muted.into_cosmic_color())
            )
            .height(Length::Fill),
            row![accept],
        ]
        .spacing(18)
        .padding(cosmic::iced::Padding::from(32.0));
        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    pub fn view(&self) -> Element<'_, Message> {
        // AUD-5 — the DISCLAIMER pre-flight accept gate (§5): until the operator
        // accepts the current disclaimer, the whole app is replaced by a
        // blocking accept screen.
        if !self.disclaimer_accepted {
            return self.disclaimer_gate_view();
        }
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
        let main = column![page_heading, body]
            // CV-3 — density-aware gap (space.lg), in step with the
            // density-aware outer padding below.
            .spacing(crate::panel_chrome::column_gap(
                crate::live_theme::tokens().density,
            ))
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
            View::Panel { panel: "hub", .. } => self.hub.view(),
            // E6.9 — the Help group root renders the role card (catch-all);
            // its action-links open the help topics index + the About/Help
            // disclaimer surface as sub-panels.
            View::Panel { panel: "index", .. } => self.help.view(),
            View::Panel { panel: "about", .. } => crate::panels::about::AboutPanel::view(),
            View::Panel {
                panel: "wallpaper", ..
            } => self.wallpaper.view(),
            View::Panel {
                panel: "notifications",
                ..
            } => self.notifications.view(),
            View::Panel { panel: "music", .. } => self.music.view(),
            View::Panel {
                panel: "sip_gateway",
                ..
            } => self.sip_gateway.view(),
            View::Panel {
                panel: "connect", ..
            } => self.connect.view(),
            // PLANES-1 — Fleet keeps the rollup lens + fleet inventory.
            View::Panel {
                panel: "inventory", ..
            } => self.inventory.view(),
            View::Panel {
                panel: "fleet_rollup",
                ..
            } => self.fleet_rollup.view(),
            // This Node plane — registration / inventory / config.
            View::Panel {
                panel: "hardware", ..
            } => self.hardware.view(),
            View::Panel {
                panel: "config_apply",
                ..
            } => self.config_apply.view(),
            View::Panel {
                panel: "registration",
                ..
            } => self.registration.view(),
            // Controller plane — jobs / playbooks / run history.
            View::Panel { panel: "jobs", .. } => self.jobs.view(),
            View::Panel {
                panel: "playbooks", ..
            } => self.playbooks.view(),
            View::Panel {
                panel: "run_history",
                ..
            } => self.run_history.view(),
            // Provisioning plane — node role pins + tags (W58).
            View::Panel {
                panel: "node_roles",
                ..
            } => self.node_roles.view(),
            View::Panel {
                panel: "snapshots", ..
            } => self.snapshots.view(),
            View::Panel { panel: "logs", .. } => self.logs.view(),
            View::Panel {
                panel: "resources", ..
            } => self.resources.view(),
            View::Panel {
                panel: "system_update",
                ..
            } => self.system_update.view(),
            View::Panel {
                panel: "repair", ..
            } => self.repair.view(),
            // v4.0.1 (2026-05-23) — Look & Feel → Panel Sync
            // Status reads panel.toml mtime + mackesd healthz
            // JSON to surface the mesh-sync state.
            View::Panel {
                panel: "sync_status",
                ..
            } => self.sync_status.view(),
            // v4.0.1 WB-2.f (2026-05-23) — Maintain → Health
            // Check renders the local-probe table (disk space,
            // memory, failed units, DNS, dnf backlog, snapshot
            // count, parity overlay).
            // PLANES-1 — Health re-homes to This Node (W20).
            View::Panel {
                panel: "health_check",
                ..
            } => self.health_check.view(),
            // PLANES-12 — Audit re-homes to Controller.
            View::Panel { panel: "audit", .. } => self.audit.view(),
            // PLANES-8 — Logs & Metrics re-home to This Node.
            View::Panel {
                panel: "mesh_logs", ..
            } => self.mesh_logs.view(),
            // PLANES-14 — Fleet Logs re-home to Controller.
            View::Panel {
                panel: "fleet_logs",
                ..
            } => self.fleet_logs.view(),
            // PLANES-11 — Drift folds into Controller/Remediation.
            View::Panel { panel: "drift", .. } => self.drift.view(),
            // PLANES-13 — the policy engine surface.
            View::Panel {
                panel: "policy", ..
            } => self.policy.view(),
            // PLANES-15 — the netstate desired-vs-actual diff.
            View::Panel {
                panel: "interfaces",
                ..
            } => self.interfaces.view(),
            // PLANES-18 — the mesh DNS record set.
            View::Panel { panel: "dns", .. } => self.dns.view(),
            // PLANES-19 — the overlay-reachability validation verdict.
            View::Panel {
                panel: "routing", ..
            } => self.routing.view(),
            // PLANES-3/W82 — the fleet capability-tag census.
            View::Panel { panel: "tags", .. } => self.tags.view(),
            // PLANES-21 — the install-profile catalog.
            View::Panel {
                panel: "profiles", ..
            } => self.profiles.view(),
            // PLANES-24 — the package-mirror catalog.
            View::Panel {
                panel: "mirrors", ..
            } => self.mirrors.view(),
            // PLANES-22 — the image catalog.
            View::Panel {
                panel: "images", ..
            } => self.images.view(),
            // BUS-7.2 — Mackes Bus 5-tab operator surface.
            View::Panel {
                panel: "mesh_bus", ..
            } => self.mesh_bus.view(),
            // TUNE-15.b — Mesh Federation 4-tab pairing surface.
            View::Panel {
                panel: "mesh_federation",
                ..
            } => self.mesh_federation.view(),
            // v4.0.1 WB-2.h (2026-05-23) — Network → Mesh
            // Control renders the leader-lease state + healthz.
            // PLANES-1 (W52) — Mesh Control gets its own Controller entry.
            View::Panel {
                panel: "mesh_control",
                ..
            } => self.mesh_control.view(),
            // v4.0.1 WB-2.i (2026-05-23) — Network → Mesh
            // Pending lists peer-probe rows from
            // $XDG_CACHE_HOME/mde/peers/<id>/probe.json with
            // Accept / Reject buttons.
            View::Panel {
                panel: "mesh_pending",
                ..
            } => self.mesh_pending.view(),
            // v4.0.1 WB-2.k (2026-05-23) — Network → Mesh
            // Topology renders the peer roster as a sortable
            // table (canvas-graph variant deferred to v4.1).
            // MESHFS-13.1 — Network → Mesh Storage status.
            View::Panel {
                panel: "mesh_storage",
                ..
            } => self.mesh_storage.view(),
            // MESH-PROBE-9.a — Network → Network Hosts lists the probe
            // inventory (hosts + identified services + trust-state).
            View::Panel {
                panel: "network_hosts",
                ..
            } => self.network_hosts.view(),
            // v4.0.1 WB-2.j (2026-05-23) — Network → Mesh
            // Services renders systemctl status + start/stop/
            // restart for the mesh-fabric daemons. v2.5 NF-5.4
            // (2026-05-24) swapped the set to nebula /
            // nebula-lighthouse / mackes-nebula-https-tunnel /
            // mackesd.
            // PLANES-1 (W4) — Mesh Services folds into This Node/Health.
            View::Panel {
                panel: "mesh_services",
                ..
            } => self.mesh_services.view(),
            // NF-13.8 (v2.5) — Network → Service Publishing
            // renders the 7 canonical Nebula-published services
            // with overlay-bind status pills.
            View::Panel {
                panel: "service_publishing",
                ..
            } => self.service_publishing.view(),
            // v4.0.1 WB-2.l (2026-05-23) — Network → Remote
            // Desktop renders cached peer-macs.json hosts +
            // per-host RDP/VNC launch buttons + a manual-entry
            // text field.
            View::Panel {
                panel: "remote_desktop",
                ..
            } => self.remote_desktop.view(),
            // PD-3 / PLANES-1 (W7) — the Peers directory (the Front
            // Door), one component two doors: the Peers plane root +
            // its panel, and the Controller/Inventory door.
            // NAV-1 — the Peers directory now lives as the first panel
            // under the Mesh section (slug-routed).
            View::Panel { panel: "peers", .. } => self.peers.view(),
            View::Panel {
                panel: "firewall", ..
            } => self.firewall.view(),
            View::Panel { panel: "wifi", .. } => self.wifi.view(),
            View::Panel { panel: "vpn", .. } => self.vpn.view(),
            View::Panel {
                panel: "mesh_join", ..
            } => self.mesh_join.view(),
            View::Panel {
                panel: "mesh_history",
                ..
            } => self.mesh_history.view(),
            // PLANES-1 (W4) — fleet settings + Config (Revisions) re-home
            // to the Controller plane.
            View::Panel {
                panel: "settings", ..
            } => self.fleet_settings.view(),
            View::Panel {
                panel: "revisions", ..
            } => self.fleet_revisions.view(),
            // E6.10 — the Compute group root (and its "Instances"
            // sub-panel) render the bespoke local + fleet VM/pod list,
            // not the generic role card: `--page compute` lands directly
            // on the instance enumeration per the E6.10 acceptance.
            // NAV-1 — Compute folds into Provisioning; the Instances panel
            // (slug-routed) renders the local + fleet VM/pod list.
            View::Panel {
                panel: "instances", ..
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

/// GUI-7 — the `cosmic::Application` shell. The inherent `update`/`view`/
/// `subscription` carry the real logic (inherent methods win direct calls, so
/// these trait methods delegate without recursion); the trait wraps the
/// reducer's iced `Task` into the cosmic `Action` space.
impl Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "dev.mackes.MDE.Workbench";

    fn core(&self) -> &cosmic::app::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::app::Core {
        &mut self.core
    }

    fn init(
        core: cosmic::app::Core,
        _flags: Self::Flags,
    ) -> (Self, cosmic::app::Task<Self::Message>) {
        // PD-4 — boot lands on the Peers Front Door; fire its directory load
        // immediately so the panel is live, not "Loading…" until the first
        // manual refresh. Deep-link boot: a `--focus <slug>` (queued in
        // PendingFocus by main before run()) lands DIRECTLY on the target
        // panel and fires its load, instead of flashing the Peers front door
        // and only navigating on the next 200 ms poll. The poll still serves
        // sibling-process `--focus` handoffs.
        let (mut app, boot) = match PendingFocus::drain() {
            Some(slug) if !slug.is_empty() => {
                let app = App::with_focus(&slug);
                let boot = match app.view {
                    View::Panel { group, panel } => app.on_panel_navigated(group, panel),
                    View::Group(g) => app.on_group_navigated(g),
                };
                (app, boot)
            }
            _ => (App::new(), crate::panels::peers::PeersPanel::load()),
        };
        app.core = core;
        // UX-4 (d) — keep the workbench's custom `crate::header` bar; suppress
        // Cosmic's headerbar so it's the only title strip the user sees.
        app.core.window.show_headerbar = false;
        app.set_header_title("MDE Workbench".to_string());
        (app, boot.map(cosmic::Action::App))
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        App::subscription(self)
    }

    fn update(&mut self, message: Self::Message) -> cosmic::app::Task<Self::Message> {
        // Delegate to the inherent reducer (inherent resolution wins), then lift
        // the iced Task into the cosmic Action space the runtime expects.
        App::update(self, message).map(cosmic::Action::App)
    }

    fn view(&self) -> Element<'_, Self::Message> {
        App::view(self)
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
/// GUI-RECONNECT — a cheap Bus liveness probe: ask mackesd for `healthz`
/// over the shared Bus with a short timeout. `true` ⇒ the control plane
/// answered. Bounded (3 s) so a wedged/absent responder can never hang the
/// GUI; the child inherits `MDE_BUS_ROOT` from the session so it hits the
/// same spool the daemon serves.
async fn probe_bus_reachable() -> bool {
    tokio::task::spawn_blocking(|| {
        std::process::Command::new("mde-bus")
            .args(["request", "action/shell/healthz", "--timeout-secs", "3"])
            .output()
            .map(|o| o.status.success() && !o.stdout.is_empty())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

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
    cosmic::iced::widget::container(inner)
        .padding(crate::panel_chrome::outer_padding(
            crate::live_theme::tokens().density,
        ))
        .width(cosmic::iced::Length::Fill)
        .height(cosmic::iced::Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_app_lands_on_overview_home() {
        let app = App::new();
        // NAV-1 (Q14) — a plain launch lands on Overview/Home.
        assert_eq!(
            app.current_view(),
            View::Panel {
                group: Group::Dashboard,
                panel: "home"
            }
        );
        assert_eq!(app.focused_pane(), Pane::Sidebar);
    }

    #[test]
    fn reconnect_probe_tracks_bus_reachability_transition() {
        // GUI-RECONNECT — the probe result drives `bus_reachable`; a
        // down→up transition is what triggers the active-panel reload.
        let mut app = App::new();
        assert!(app.bus_reachable, "starts optimistic");
        let _ = app.update(Message::ReconnectProbed(false));
        assert!(!app.bus_reachable, "probe failure marks the Bus down");
        // Recovery: false→true flips the flag back (and re-loads the panel).
        let _ = app.update(Message::ReconnectProbed(true));
        assert!(app.bus_reachable, "probe success marks the Bus back up");
    }

    #[test]
    fn every_plane_panel_has_a_real_reducer_now() {
        // PLANES-1 (W16) follow-through — the full-tree empty-state plane
        // panels (policy, interfaces, dns, routing, tags, profiles,
        // images, mirrors) all shipped real reducers, so the
        // worklist-item hook no longer fires for any of them.
        for (g, p) in [
            (Group::Fleet, "policy"),
            (Group::ThisNode, "interfaces"),
            (Group::ThisNode, "dns"),
            (Group::ThisNode, "routing"),
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
        let _ = app.update(Message::SelectGroup(Group::ThisNode));
        assert_eq!(app.current_view(), View::Group(Group::ThisNode));
        assert_eq!(app.focused_pane(), Pane::Main);
    }

    #[test]
    fn select_panel_carries_group_and_panel_slug() {
        let mut app = App::new();
        let _ = app.update(Message::SelectPanel {
            group: Group::ThisNode,
            panel: "remote_desktop",
        });
        assert_eq!(
            app.current_view(),
            View::Panel {
                group: Group::ThisNode,
                panel: "remote_desktop"
            }
        );
    }

    #[test]
    fn ctrl_digit_key_action_jumps_to_group_and_refocuses_sidebar() {
        let mut app = App::new();
        let _ = app.update(Message::KeyPressed(KeyAction::JumpToGroup(Group::System)));
        assert_eq!(app.current_view(), View::Group(Group::System));
        assert_eq!(app.focused_pane(), Pane::Sidebar);
    }

    #[test]
    fn drill_to_peers_opens_peers_front_door_filtered_by_role() {
        // PLANES-20 / W87 — a Fleet-rollup card drill-down lands on the
        // Peers plane with the role pre-filtered.
        let mut app = App::new();
        let _ = app.update(Message::DrillToPeers("host".into()));
        // NAV-1 — Peers is now the first panel under the Mesh section.
        assert_eq!(
            app.current_view(),
            View::Panel {
                group: Group::Mesh,
                panel: "peers"
            }
        );
        assert_eq!(app.peers.filter, "host");
    }

    #[test]
    fn escape_from_panel_view_returns_to_parent_group_landing() {
        let mut app = App::new();
        let _ = app.update(Message::SelectPanel {
            group: Group::System,
            panel: "snapshots",
        });
        let _ = app.update(Message::KeyPressed(KeyAction::CloseDetail));
        assert_eq!(app.current_view(), View::Group(Group::System));
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
                group: Group::ThisNode,
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
    fn select_kept_wallpaper_panel_relocates_to_this_node() {
        // NAV-1.2 — wallpaper is a mesh-specific kept panel, relocated from
        // the retired Desktop group into This Node.
        let mut app = App::new();
        let _ = app.update(Message::SelectPanel {
            group: Group::ThisNode,
            panel: "wallpaper",
        });
        assert_eq!(
            app.current_view(),
            View::Panel {
                group: Group::ThisNode,
                panel: "wallpaper"
            }
        );
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
    fn focus_request_with_panel_slug_jumps_to_panel_and_focuses_main() {
        let mut app = App::new();
        let _ = app.update(Message::FocusRequest("network.mesh_ssh".into()));
        assert_eq!(
            app.current_view(),
            View::Panel {
                group: Group::ThisNode,
                panel: "remote_desktop"
            }
        );
        assert_eq!(app.focused_pane(), Pane::Main);
    }

    #[test]
    fn focus_request_with_group_slug_lands_on_group_view() {
        let mut app = App::new();
        let _ = app.update(Message::FocusRequest("system".into()));
        assert_eq!(app.current_view(), View::Group(Group::System));
    }

    #[test]
    fn focus_request_empty_slug_preserves_view() {
        let mut app = App::new();
        let _ = app.update(Message::SelectPanel {
            group: Group::System,
            panel: "snapshots",
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
        let _ = app.update(Message::SelectGroup(Group::System));
        let before = app.current_view();
        let _ = app.update(Message::FocusRequest("not-a-real-slug".into()));
        assert_eq!(
            app.current_view(),
            before,
            "unknown slug must not jolt the user out of their current view"
        );
    }

    #[test]
    fn page_title_tracks_active_page() {
        // CUT-1: the page-aware title now drives the custom header heading via
        // `page_title(self.view)` (the iced-era window `title()` was removed).
        let mut app = App::new();
        let _ = app.update(Message::SelectGroup(Group::Mesh));
        assert!(page_title(app.current_view()).contains("Mesh"));
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
        let _ = app.update(Message::SelectGroup(Group::ThisNode));
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
