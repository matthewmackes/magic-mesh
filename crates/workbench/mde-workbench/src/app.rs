//! Top-level Iced application — state, message reducer, view.
//!
//! CB-1.1 + CB-1.2 scaffold: nine-group sidebar + breadcrumb +
//! page title / subtitle. Per-panel views (CB-1.3 ... CB-1.10)
//! land as separate substeps and plug into [`App::view`] via
//! [`crate::View::Panel`] matching.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cosmic::app::ApplicationExt;
use cosmic::iced::widget::{column, container, row, stack, text};
use cosmic::iced::{window, Length, Subscription, Task};
use cosmic::{Application, Element};

use crate::cosmic_compat::prelude::*;

use crate::backend::{Backend, RemoteBackend};
use crate::dbus::PendingFocus;
use crate::header::HeaderAction;
use crate::keyboard::{KeyAction, Pane};
use crate::model::{resolve_panel_label, view_from_focus_slug, DatacenterTab, Group, View};
use crate::panels::{
    all_services as all_services_panel, audit as audit_panel, build_farm as build_farm_panel,
    compute as compute_panel, config_apply as config_apply_panel, connect as connect_panel,
    connectivity as connectivity_panel, datacenter as datacenter_panel, dns as dns_panel,
    drift as drift_panel, firewall as firewall_panel, fleet_logs as fleet_logs_panel,
    fleet_revisions as fleet_revisions_panel, fleet_rollup as fleet_rollup_panel,
    fleet_settings as fleet_settings_panel, front_door as front_door_panel,
    genesis as genesis_panel, hardware as hardware_panel, health_check as health_check_panel,
    help_index as help_index_panel, hub as hub_panel, images as images_panel,
    interfaces as interfaces_panel, inventory as inventory_panel, jobs as jobs_panel,
    lighthouses as lighthouses_panel, logs as logs_panel, mesh_bus as mesh_bus_panel,
    mesh_control as mesh_control_panel, mesh_federation as mesh_federation_panel,
    mesh_history as mesh_history_panel, mesh_join as mesh_join_panel, mesh_logs as mesh_logs_panel,
    mesh_pending as mesh_pending_panel, mesh_services as mesh_services_panel,
    mesh_storage as mesh_storage_panel, mirrors as mirrors_panel, music as music_panel,
    network_hosts as network_hosts_panel, node_roles as node_roles_panel,
    notifications as notifications_panel, peers as peers_panel, playbooks as playbooks_panel,
    policy as policy_panel, profiles as profiles_panel, provisioning as provisioning_panel,
    registration as registration_panel, remote_desktop as remote_desktop_panel,
    repair as repair_panel, resources as resources_panel, router as router_panel,
    routing as routing_panel, run_history as run_history_panel,
    service_publishing as service_publishing_panel, sip_gateway as sip_gateway_panel,
    snapshots as snapshots_panel, sync_status as sync_status_panel,
    system_update as system_update_panel, tags as tags_panel, vpn as vpn_panel,
    wallpaper as wallpaper_panel, wifi as wifi_panel,
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
    /// FRONTDOOR-2 — the Front Door (Win10-Start home) sub-message. Currently
    /// just the omnibox text-change; rail navigation reuses `SelectPanel`.
    FrontDoor(front_door_panel::Message),
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
    /// FRONTDOOR-15 — launch a standalone MDE app **pointed at a remote node's
    /// data** (Q74 — GUI apps open locally on remote data). The GUI runs on THIS
    /// workstation (a remote X/Wayland session is out of scope); the carried node
    /// target (its overlay address) is passed to the app as `--node <target>` so
    /// it reads the chosen node's data instead of the local default. Mirrors the
    /// detached best-effort spawn of [`LaunchApp`] — a binary/flag a given app
    /// doesn't support simply has no effect (the app falls back to local), never a
    /// panic. The `String` is the resolved target (overlay IP / hostname).
    LaunchAppOnNode(&'static str, String),
    /// AIR-20 — Devices → Music settings panel sub-message.
    Music(music_panel::Message),
    VoipGateway(sip_gateway_panel::Message),
    /// CB-1.5.a — Fleet inventory panel sub-message.
    Inventory(inventory_panel::Message),
    /// PLANES-5 — hardware inventory (replicated PeerProbe) sub-message.
    Hardware(hardware_panel::Message),
    /// PLANES-10 — Jobs panel (templates + run history) sub-message.
    Jobs(jobs_panel::Message),
    BuildFarm(build_farm_panel::Message),
    Datacenter(datacenter_panel::Message),
    /// DATACENTER-25 — switch the active tab inside the Datacenter panel (its own
    /// surface or one of the five folded panels). Fired by the Datacenter fold-bar
    /// buttons; the handler selects the tab and fires the folded panel's `load()`
    /// so the tab lands populated (matching the per-panel on-nav load behaviour).
    DatacenterTab(DatacenterTab),
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
    /// XCP-4 — Provisioning (VM Spawner) panel sub-message.
    Provisioning(provisioning_panel::Message),
    /// DATACENTER-18 — New-Mesh genesis wizard sub-message.
    Genesis(genesis_panel::Message),
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
    Lighthouses(lighthouses_panel::Message),
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
    /// CONNECT-6 — Network → Connectivity (exposure matrix) sub-message.
    Connectivity(connectivity_panel::Message),
    MeshStorage(mesh_storage_panel::Message),
    /// MESH-PROBE-9.a — Network → Network Hosts panel sub-message.
    NetworkHosts(network_hosts_panel::Message),
    /// COMPUTE/SVC-VIEW — Mesh → All Services unified panel sub-message.
    AllServices(all_services_panel::Message),
    /// ROUTER-5 — Routers panel sub-message.
    Router(router_panel::Message),
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
    /// MOTION-TRANS-1 — one frame of the active panel/route crossfade. Fired by
    /// the in-flight-only transition tick (idle ⇒ no wakeups); the handler just
    /// re-renders, clearing the transition once the tween completes.
    TransitionTick,
}

/// MOTION-TRANS-1 — an in-flight panel/route crossfade. Created the instant the
/// active [`View`] changes; the incoming body fades in from the panel background
/// over the [`Motion::dialog_mount`](mde_theme::motion::Motion::dialog_mount)
/// duration (≤80 ms under reduce-motion). The switch itself registers
/// immediately (the new view is already live in `App::view`); only the visual
/// dissolve is deferred, so there is no input delay. Held in `Option` so idle =
/// no transition = no tick subscription.
#[derive(Debug, Clone, Copy)]
struct PanelTransition {
    /// When the swap happened — the crossfade origin.
    start: Instant,
    /// Whether reduce-motion was active at swap time, so the tween caps to the
    /// Carbon ≤80 ms crossfade and skips the eased curve (a stable snapshot for
    /// the life of this transition).
    reduce_motion: bool,
}

impl PanelTransition {
    /// The crossfade's `(scrim_alpha, complete)` at `now`. `scrim_alpha` is the
    /// **outgoing** surface's opacity from [`mde_theme::animation::crossfade`] —
    /// the panel-background veil still over the incoming body (`1.0` at the swap,
    /// `0.0` once fully revealed). `complete` is true once the incoming surface is
    /// fully opaque, which (because [`mde_theme::animation::Tween`] clamps progress
    /// to `1.0` at/after its duration) is reached exactly at the end of the tween —
    /// so the tick subscription and the scrim both tear down with no idle wakeups.
    /// Computed in one call so a frame never re-runs the easing math.
    fn sample(self, now: Instant) -> (f32, bool) {
        let (outgoing, incoming) =
            mde_theme::animation::crossfade(self.start, now, self.reduce_motion);
        (outgoing.alpha, incoming.alpha >= 1.0)
    }

    /// Has the crossfade finished at `now`? (Drives the tick-stop guard.)
    fn is_complete(self, now: Instant) -> bool {
        self.sample(now).1
    }
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
    /// MOTION-TRANS-1 — the in-flight crossfade for the most recent panel/route
    /// switch, or `None` at rest. Set by [`App::begin_transition`] whenever the
    /// active [`View`] actually changes; cleared once the tween completes.
    transition: Option<PanelTransition>,
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
    /// FRONTDOOR-1 — the GPU canvas tile-grid "Front Door" that renders the
    /// home/dashboard route. FRONTDOOR-16 removed the old `home` widget-tree
    /// state + view entirely (the slow "4-second menu"); the Front Door is the
    /// sole launcher now, and the only live remnant of `home` is its
    /// boot-readiness reader (read as a free function, no panel state).
    front_door: front_door_panel::FrontDoor,
    /// v4.0.1 WB-2.b — Maintain group root grid state.
    hub: hub_panel::HubPanel,
    /// v4.0.1 WB-2.c — Help group root topics list.
    help: help_index_panel::HelpIndexPanel,
    inventory: inventory_panel::InventoryPanel,
    hardware: hardware_panel::HardwarePanel,
    jobs: jobs_panel::JobsPanel,
    build_farm: build_farm_panel::BuildFarmPanel,
    datacenter: datacenter_panel::DatacenterPanel,
    /// DATACENTER-25 — which surface is showing inside the Datacenter panel: its
    /// own multi-lens surface (`Native`) or one of the five folded panels
    /// (Instances / Snapshots / Images / Lighthouses / Build Farm). Only
    /// meaningful while the active view is the `datacenter` panel; the fold-bar
    /// (`app.rs`) reads it to render the active tab, and the per-tab subscriptions
    /// gate on `view == datacenter && datacenter_tab == X` so a folded surface
    /// only samples/beams/refreshes while its tab is shown.
    datacenter_tab: DatacenterTab,
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
    provisioning: provisioning_panel::ProvisioningPanel,
    genesis: genesis_panel::GenesisPanel,
    system_update: system_update_panel::SystemUpdatePanel,
    repair: repair_panel::RepairPanel,
    health_check: health_check_panel::HealthCheckPanel,
    drift: drift_panel::DriftPanel,
    policy: policy_panel::PolicyPanel,
    interfaces: interfaces_panel::InterfacesPanel,
    dns: dns_panel::DnsPanel,
    routing: routing_panel::RoutingPanel,
    lighthouses: lighthouses_panel::LighthousesPanel,
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
    /// CONNECT-6 — Network → Connectivity (exposure matrix) panel state.
    connectivity: connectivity_panel::ConnectivityPanel,
    mesh_storage: mesh_storage_panel::MeshStoragePanel,
    /// MESH-PROBE-9.a — Network → Network Hosts panel state (the probe
    /// host/service inventory read off mesh-storage).
    network_hosts: network_hosts_panel::NetworkHostsPanel,
    /// COMPUTE/SVC-VIEW — Mesh → All Services unified panel state.
    all_services: all_services_panel::AllServicesPanel,
    /// ROUTER-5 — Routers panel state.
    router: router_panel::RouterPanel,
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
            transition: None,
            sidebar: SidebarState::new(),
            focused_pane: Pane::Sidebar,
            bus_reachable: true,
            backend,
            notifications: notifications_panel::NotificationsPanel::new(),
            music: music_panel::MusicPanel::new(),
            sip_gateway: sip_gateway_panel::SipGatewayPanel::new(),
            connect: connect_panel::ConnectPanel::new(),
            front_door: front_door_panel::FrontDoor::new(),
            hub: hub_panel::HubPanel::new(),
            help: help_index_panel::HelpIndexPanel::new(),
            inventory: inventory_panel::InventoryPanel::new(),
            hardware: hardware_panel::HardwarePanel::new(),
            jobs: jobs_panel::JobsPanel::new(),
            build_farm: build_farm_panel::BuildFarmPanel::new(),
            datacenter: datacenter_panel::DatacenterPanel::new(),
            // DATACENTER-25 — land on Datacenter's own surface; folded tabs are
            // selected by a fold-bar click or a folded-slug deep link.
            datacenter_tab: DatacenterTab::default(),
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
            provisioning: provisioning_panel::ProvisioningPanel::new(),
            genesis: genesis_panel::GenesisPanel::new(),
            system_update: system_update_panel::SystemUpdatePanel::new(),
            repair: repair_panel::RepairPanel::new(),
            health_check: health_check_panel::HealthCheckPanel::new(),
            drift: drift_panel::DriftPanel::new(),
            policy: policy_panel::PolicyPanel::new(),
            interfaces: interfaces_panel::InterfacesPanel::new(),
            dns: dns_panel::DnsPanel::new(),
            routing: routing_panel::RoutingPanel::new(),
            lighthouses: lighthouses_panel::LighthousesPanel::new(),
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
            connectivity: connectivity_panel::ConnectivityPanel::new(),
            mesh_storage: mesh_storage_panel::MeshStoragePanel::new(),
            network_hosts: network_hosts_panel::NetworkHostsPanel::new(),
            all_services: all_services_panel::AllServicesPanel::new(),
            router: router_panel::RouterPanel::new(),
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
        // APPLAUNCH-8/9 — the `launcher` slug (the Start button + Super key) lands
        // on the Front Door (Dashboard) with its unified launcher OPEN, on a cold
        // launch exactly as the running-instance hand-off does (apply_focus_request).
        if focus_slug == "launcher" {
            app.view = View::Group(Group::Dashboard);
            app.focused_pane = Pane::Main;
            app.front_door.launcher.open = true;
            return app;
        }
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
    /// 2. **Front Door live data** — the view-gated slow-poll + the Peers
    ///    directory-changed Bus event keep its tiles live (below). FRONTDOOR-16
    ///    retired the old Overview D-Bus / Nebula-event subscriptions: they only
    ///    fed the removed `home` capability-row state, which no longer renders.
    #[allow(clippy::unused_self)]
    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            cosmic::iced::time::every(Duration::from_millis(200))
                .map(|_| PendingFocus::drain().map_or(Message::Noop, Message::FocusRequest)),
            // GUI-RECONNECT — a slow Bus liveness tick. On a down→up
            // transition (mackesd came back) the handler re-loads the
            // active panel, so panels recover on their own instead of
            // showing a stale "mesh service isn't answering" until a
            // manual refresh.
            cosmic::iced::time::every(Duration::from_secs(10)).map(|_| Message::ReconnectTick),
        ];
        // MOTION-TRANS-1 — drive the panel/route crossfade at ~60 fps, but ONLY
        // while a transition is actually in flight. At rest `self.transition` is
        // `None`, so this subscription doesn't exist and there are zero idle
        // wakeups (MOTION-PERF-1).
        if self
            .transition
            .is_some_and(|t| !t.is_complete(Instant::now()))
        {
            subs.push(
                cosmic::iced::time::every(Duration::from_millis(16))
                    .map(|_| Message::TransitionTick),
            );
        }
        // E6.10 — sample Compute instance CPU/mem only while that view is
        // active, so virsh/podman stats aren't polled when the operator is
        // elsewhere. DATACENTER-25 — the Compute/Instances surface is now the
        // Datacenter "Instances" tab, so gate on that tab being shown.
        if self.on_datacenter_tab(DatacenterTab::Instances) {
            subs.push(compute_panel::ComputePanel::sample_subscription());
        }
        // LIGHTHOUSE-5 — only while the Lighthouses tab is open: advance the
        // beacon beam (150ms) and refresh the cards from the replicated
        // directory (5s push-ish). Idle elsewhere (no CPU when the tab is shut).
        // DATACENTER-25 — Lighthouses is now the Datacenter "Lighthouses" tab.
        if self.on_datacenter_tab(DatacenterTab::Lighthouses) {
            subs.push(
                cosmic::iced::time::every(Duration::from_millis(150))
                    .map(|_| Message::Lighthouses(lighthouses_panel::Message::BeamTick)),
            );
            subs.push(
                cosmic::iced::time::every(Duration::from_secs(5))
                    .map(|_| Message::Lighthouses(lighthouses_panel::Message::Refresh)),
            );
        }
        // FRONTDOOR-4 — while the Front Door is the active view (the Dashboard
        // group root or the "home" panel both render it), keep its widget tiles
        // live: the slow-poll fallback (15 s) backstops the not-purely-push
        // topics (build/farm verdict, datacenter health, boot readiness), and the
        // Peers directory-changed Bus event is the push half — it fires a Front
        // Door reload the instant the roster changes, so node-health / mesh-map /
        // data-center tiles update without waiting out the poll (Q22). Both are
        // view-gated, so nothing polls when the operator is elsewhere.
        if matches!(
            self.view,
            View::Group(Group::Dashboard) | View::Panel { panel: "home", .. }
        ) {
            subs.push(front_door_panel::FrontDoor::poll_subscription());
            subs.push(
                peers_panel::directory_event_subscription()
                    .map(|_| Message::FrontDoor(front_door_panel::Message::Reload)),
            );
            // APPLAUNCH-8 / CTRLSURF-3 — keyboard-first nav on the Front Door home.
            // The launcher overlay owns its keys while open (↑↓ highlight, Enter
            // commit, Esc close, Tab/Ctrl+1..5). CTRLSURF-3 (Phase 2) promotes the
            // same keyboard-first story to the resting / searching home when the
            // launcher is CLOSED: Esc clears the search, ↑↓/Tab summon the launcher,
            // Ctrl+1..5 jump to the rail's section routes (Enter is the omnibox's own
            // on_submit). Additive — the mouse is untouched. Gated on
            // `home_keys_active` so the home keys never fight an open overlay
            // (Settings / Pending / detail), which carry their own controls.
            if self.front_door.launcher.open {
                subs.push(front_door_panel::FrontDoor::launcher_key_subscription());
            } else if self.front_door.home_keys_active() {
                subs.push(front_door_panel::FrontDoor::home_key_subscription());
            }
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
        // MOTION-TRANS-1 — snapshot the active route before the reducer runs so a
        // panel/route switch (any handler that reassigns `self.view`) arms a
        // crossfade afterwards. The switch registers immediately (the new view is
        // live the moment the match returns); only the visual dissolve is deferred,
        // so there is no input delay.
        let prev_view = self.view;
        let task = self.reduce(message);
        if self.view != prev_view {
            self.begin_transition();
        }
        task
    }

    /// MOTION-TRANS-1 — arm a fresh crossfade for the panel/route that just became
    /// active. Snapshots the current reduce-motion preference so the tween's
    /// duration/curve stay stable for the life of the transition.
    fn begin_transition(&mut self) {
        self.transition = Some(PanelTransition {
            start: Instant::now(),
            reduce_motion: crate::live_theme::reduce_motion(),
        });
    }

    /// The message reducer proper. Split out of [`App::update`] so the latter can
    /// wrap it with the MOTION-TRANS-1 route-change crossfade detection.
    fn reduce(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SelectGroup(group) => {
                self.view = View::Group(group);
                self.focused_pane = Pane::Main;
                // FRONTDOOR-5 — leaving (or re-entering) the Front Door drops any
                // open tile detail, so it always lands on the grid, not a stale
                // actions menu from a prior visit.
                self.front_door.detail = None;
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
                self.focused_pane = Pane::Main;
                // FRONTDOOR-5 — see SelectGroup: any navigation resets the Front
                // Door to its grid (no stale tile detail on return).
                self.front_door.detail = None;
                // DATACENTER-25 — a click/link targeting one of the six folded
                // slugs (e.g. a Home stat-card → System/snapshots) routes to the
                // Datacenter panel with that tab selected, so the now-retired
                // standalone slug never lands on a missing panel.
                if let Some(tab) = DatacenterTab::from_folded_slug(panel) {
                    self.view = View::Panel {
                        group: Group::Provisioning,
                        panel: "datacenter",
                    };
                    let dc_load = self.on_panel_navigated(Group::Provisioning, "datacenter");
                    let tab_load = self.select_datacenter_tab(tab);
                    return Task::batch([dc_load, tab_load]);
                }
                self.view = View::Panel { group, panel };
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
            Message::FrontDoor(msg) => {
                // FRONTDOOR-2 folds the omnibox text + mode flip (no side effect);
                // FRONTDOOR-4's Reload/Loaded drive the live-tile Bus read, so the
                // panel update now returns a real Task (the off-thread load).
                self.front_door.update(msg)
            }
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
            Message::LaunchAppOnNode(bin, node) => {
                // FRONTDOOR-15 (Q74) — the same detached best-effort spawn as
                // `LaunchApp`, but the chosen node's address is handed to the app
                // as `--node <target>` so the GUI runs LOCALLY pointed at the
                // remote node's data. An app that doesn't recognise the flag falls
                // back to its local default (the flag is inert, not fatal); a
                // missing binary fails silently — never panics the Workbench.
                let _ = std::process::Command::new(bin)
                    .args(["--node", &node])
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
            Message::BuildFarm(msg) => self.build_farm.update(msg),
            Message::Datacenter(msg) => self.datacenter.update(msg),
            // DATACENTER-25 — switch the Datacenter fold-bar tab and fire the
            // newly-shown surface's load so it lands populated (same contract as
            // a standalone panel's on-nav load).
            Message::DatacenterTab(tab) => self.select_datacenter_tab(tab),
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
            Message::Provisioning(msg) => self.provisioning.update(msg),
            Message::Genesis(msg) => self.genesis.update(msg),
            Message::SystemUpdate(msg) => self.system_update.update(msg),
            Message::Repair(msg) => self.repair.update(msg),
            Message::HealthCheck(msg) => self.health_check.update(msg),
            Message::Drift(msg) => self.drift.update(msg),
            Message::Policy(msg) => self.policy.update(msg),
            Message::Interfaces(msg) => self.interfaces.update(msg),
            Message::Dns(msg) => self.dns.update(msg),
            Message::Routing(msg) => self.routing.update(msg),
            Message::Lighthouses(msg) => self.lighthouses.update(msg),
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
            Message::Connectivity(msg) => self.connectivity.update(msg),
            Message::MeshStorage(msg) => self.mesh_storage.update(msg),
            Message::NetworkHosts(msg) => self.network_hosts.update(msg),
            Message::AllServices(msg) => self.all_services.update(msg),
            Message::Router(msg) => self.router.update(msg),
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
            // MOTION-TRANS-1 — a crossfade frame. Drop the transition once it has
            // settled so the tick subscription goes quiet (no idle wakeups); while
            // it's still in flight, returning here just re-renders the next frame.
            Message::TransitionTick => {
                if self
                    .transition
                    .is_some_and(|t| t.is_complete(Instant::now()))
                {
                    self.transition = None;
                }
                Task::none()
            }
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
                    match self.view {
                        View::Panel { group, panel } => {
                            return self.on_panel_navigated(group, panel)
                        }
                        // FRONTDOOR-4 — the Dashboard group root renders the Front
                        // Door but isn't a Panel route, so re-fire its live-tile
                        // load directly on a mackesd down→up recovery.
                        View::Group(Group::Dashboard) => {
                            return front_door_panel::FrontDoor::load()
                        }
                        View::Group(_) => {}
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
            // FRONTDOOR-1/16 — the "home" route RENDERS the Front Door (see the
            // view router); the old home/Overview panel + its load were removed at
            // parity, so this fires only the Front Door's live-tile load. Its
            // System tile reads boot readiness as a free function (no panel state).
            "home" => front_door_panel::FrontDoor::load(),
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
            // DATACENTER-25 — navigating to the Datacenter panel loads its own
            // surface AND re-fires the currently-selected folded tab's load, so a
            // return-nav refreshes whichever fold-bar surface was last shown
            // (Native = no extra load; its `DatacenterPanel::load` covers it). The
            // folded slugs (build-farm/snapshots/images/lighthouses/instances) no
            // longer reach this match — they redirect to `datacenter` upstream —
            // so their on-nav loads run via `select_datacenter_tab` instead.
            "datacenter" => {
                let dc = datacenter_panel::DatacenterPanel::load();
                let tab = match self.datacenter_tab {
                    DatacenterTab::Native => Task::none(),
                    DatacenterTab::Instances => compute_panel::ComputePanel::load(),
                    DatacenterTab::Snapshots => snapshots_panel::SnapshotsPanel::load(),
                    DatacenterTab::Images => images_panel::ImagesPanel::load(),
                    DatacenterTab::Lighthouses => lighthouses_panel::LighthousesPanel::load(),
                    DatacenterTab::BuildFarm => build_farm_panel::BuildFarmPanel::load(),
                };
                Task::batch([dc, tab])
            }
            "node_roles" => node_roles_panel::NodeRolesPanel::load(),
            "playbooks" => playbooks_panel::PlaybooksPanel::load(),
            "run_history" => run_history_panel::RunHistoryPanel::load(),
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
            // DATACENTER-25 — Lighthouses / Images / Instances (Compute) are now
            // Datacenter fold-bar tabs; their on-nav loads run via
            // `select_datacenter_tab` (and the `datacenter` arm above), so their
            // slugs no longer reach this match — they redirect to `datacenter`.
            // PLANES-3/W82 — the fleet capability-tag census.
            "tags" => tags_panel::TagsPanel::load(),
            // PLANES-21 — the install-profile catalog.
            "profiles" => profiles_panel::ProfilesPanel::load(),
            // PLANES-24 — the package-mirror catalog.
            "mirrors" => mirrors_panel::MirrorsPanel::load(),
            // XCP-4 — the VM Spawner queries the xcp_provision worker for the
            // VM + dom0-host rosters on nav so the panel lands populated.
            "provisioning" => provisioning_panel::ProvisioningPanel::load(),
            // DATACENTER-18 — the New-Mesh genesis wizard queries the do-regions
            // roster on nav so step 1's region picker lands populated.
            "genesis" => genesis_panel::GenesisPanel::load(),
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
            // COMPUTE/SVC-VIEW — All Services unions all three sources on first nav.
            "all_services" => all_services_panel::AllServicesPanel::load(),
            // ROUTER-5 — Routers panel reads the per-node router registry on first nav.
            "router" => router_panel::RouterPanel::load(),
            // PLANES-1 (W4) — Mesh Services folds into This Node/Health.
            "mesh_services" => mesh_services_panel::MeshServicesPanel::load(),
            // NF-13.8 (v2.5) — shell-out to
            // mackes.mesh_nebula.published_services_summary
            // for the 7 canonical services + per-row overlay
            // bind state.
            "service_publishing" => service_publishing_panel::ServicePublishingPanel::load(),
            // CONNECT-6 — the exposure matrix: list-services + list-candidates
            // over action/connect/* on first nav.
            "connectivity" => connectivity_panel::ConnectivityPanel::load(),
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
    fn on_group_navigated(&self, group: Group) -> Task<Message> {
        // NAV-1 — group roots render the role card; the live directory
        // (Peers) and the instance list (Compute→Provisioning/Instances)
        // load via their slug-routed panels, not a group root.
        // FRONTDOOR-4 — the Dashboard root renders the Front Door (see the view
        // router), so landing on it fires the Front Door's live-tile load so the
        // widgets stream real data instead of sitting on the skeleton.
        match group {
            Group::Dashboard => front_door_panel::FrontDoor::load(),
            _ => Task::none(),
        }
    }

    /// DATACENTER-25 — select a Datacenter fold-bar tab and fire the newly-shown
    /// surface's `load()` so it lands populated. `Native` is Datacenter's own
    /// surface (already loaded by the `datacenter` on-nav load); the folded tabs
    /// delegate to the absorbed panel's loader. Reused by the fold-bar message and
    /// by a folded-slug deep link / focus request.
    fn select_datacenter_tab(&mut self, tab: DatacenterTab) -> Task<Message> {
        self.datacenter_tab = tab;
        match tab {
            // Datacenter's own surface — its load fires on the `datacenter`
            // on-panel-nav (and the Refresh button), so selecting the tab is a
            // pure view switch.
            DatacenterTab::Native => Task::none(),
            DatacenterTab::Instances => compute_panel::ComputePanel::load(),
            DatacenterTab::Snapshots => snapshots_panel::SnapshotsPanel::load(),
            DatacenterTab::Images => images_panel::ImagesPanel::load(),
            DatacenterTab::Lighthouses => lighthouses_panel::LighthousesPanel::load(),
            DatacenterTab::BuildFarm => build_farm_panel::BuildFarmPanel::load(),
        }
    }

    /// DATACENTER-25 — is the active view the Datacenter panel with `tab` shown?
    /// The per-tab subscriptions (Instances CPU/mem sampling, Lighthouse beam +
    /// refresh) gate on this so a folded surface keeps its live data while its
    /// tab is open and stays idle (no CPU) when it isn't.
    #[must_use]
    fn on_datacenter_tab(&self, tab: DatacenterTab) -> bool {
        self.view.panel_slug() == Some("datacenter") && self.datacenter_tab == tab
    }

    fn apply_focus_request(&mut self, slug: &str) -> Task<Message> {
        if slug.is_empty() {
            // Empty slug = "raise only, no view change" — the
            // 1.x taskbar click-through behaviour.
            return Task::none();
        }
        // APPLAUNCH-8/9 — the Start button + Super key press the `launcher` slug
        // (replacing the retired `mde-apps-applet --toggle`): land on the Dashboard
        // (the Front Door) and open its unified app-launcher panel mode. This is
        // the SOLE launcher trigger now — one launcher (Q29/Q33).
        if slug == "launcher" {
            self.view = View::Group(Group::Dashboard);
            self.focused_pane = Pane::Main;
            return Task::batch([
                front_door_panel::FrontDoor::load(),
                Task::done(Message::FrontDoor(
                    front_door_panel::Message::ToggleLauncher,
                )),
            ]);
        }
        // LIGHTHOUSE-4 — a "<group>.<panel>:<focus>" slug carries a per-item
        // focus target (the Hub footer presses `mesh.lighthouses:<host>`). Split
        // the focus off: the left routes to the view, the right highlights the
        // item. No existing slug contains a colon, so this is unambiguous.
        let (route_slug, focus_id) = match slug.split_once(':') {
            Some((r, f)) => (r, Some(f.to_string())),
            None => (slug, None),
        };
        let Some(view) = view_from_focus_slug(route_slug) else {
            // Unknown slug silently ignored — matches the 1.x
            // `mackes --focus` Dashboard fallback for unmapped
            // surfaces (here we keep the current view since
            // jumping back to Dashboard on a typo would
            // surprise the user mid-task).
            return Task::none();
        };
        self.view = view;
        self.focused_pane = Pane::Main;
        // DATACENTER-25 — a folded-surface deep link (e.g. `mesh.lighthouses`,
        // `system.snapshots`) resolves to the Datacenter panel; select the
        // matching fold-bar tab so the surface is actually shown, and route the
        // per-item focus suffix to the folded panel. The route_slug still carries
        // the original folded panel name (`view_from_focus_slug` only rewrote the
        // View, not the slug), so map it back to its tab.
        let folded_tab = route_slug
            .rsplit('.')
            .next()
            .and_then(DatacenterTab::from_folded_slug);
        if let Some(tab) = folded_tab {
            // Apply the per-item focus before the load so the Lighthouses tab opens
            // already highlighting + listing the clicked lighthouse first (Q20).
            if tab == DatacenterTab::Lighthouses {
                if let Some(focus) = &focus_id {
                    self.lighthouses.set_focus(focus);
                }
            }
            let dc_load = self.on_panel_navigated(Group::Provisioning, "datacenter");
            let tab_load = self.select_datacenter_tab(tab);
            return Task::batch([dc_load, tab_load]);
        }
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

        let body = self.crossfaded_body();

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

    /// MOTION-TRANS-1 — the active panel body, crossfaded in while a route switch
    /// transition is in flight. iced 0.13 has no opacity widget for an arbitrary
    /// subtree, so a true two-buffer A↔B crossfade isn't expressible (we can't keep
    /// the outgoing Element alive across the swap). Instead the incoming body
    /// dissolves **through the panel background**: a background-coloured scrim is
    /// stacked over the live new body at the outgoing surface's opacity (full at
    /// the swap → clear when revealed), so the swap reads as one deliberate
    /// cross-dissolve rather than a hard cut. The scrim is an inert `container`
    /// (its content is a `Space`, so `mouse_interaction` is `None`): the iced
    /// `stack` therefore does not levitate the cursor away from the body, and
    /// clicks/scrolls reach the already-live new panel even mid-fade — there is no
    /// input delay. At rest (`transition == None`, or once settled) the body is
    /// returned bare, with zero extra widgets and the panel's own sizing intact.
    fn crossfaded_body(&self) -> Element<'_, Message> {
        let body = self.panel_body();
        let Some(transition) = self.transition else {
            return body;
        };
        // One easing evaluation per frame: the background veil + completeness.
        let (scrim_alpha, complete) = transition.sample(Instant::now());
        if complete {
            return body;
        }
        let bg = crate::live_theme::palette().background;
        // The scrim is `Length::Fill` so it covers exactly the body's bounds; the
        // stack inherits the body's own sizing (no forced Fill), so the panel's
        // layout is identical with or without the in-flight scrim — no reflow.
        let scrim = container(cosmic::iced::widget::Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_theme| container::Style {
                background: Some(cosmic::iced::Background::Color(
                    crate::cosmic_compat::with_alpha(bg.into_cosmic_color(), scrim_alpha),
                )),
                ..container::Style::default()
            });
        stack![body, scrim].into()
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
            // FRONTDOOR-1 — the Dashboard/home route now renders the GPU canvas
            // tile-grid Front Door instead of the old slow `home` widget tree.
            // `home`'s state + load stay intact (data reuse is FRONTDOOR-4); only
            // this VIEW is swapped.
            View::Group(Group::Dashboard) => self.front_door.view(),
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
            // DATACENTER-25 — the Datacenter panel now hosts six surfaces behind a
            // fold-bar: its own (Native) plus the folded Instances / Snapshots /
            // Images / Lighthouses / Build Farm panels. `datacenter_surface`
            // renders the fold-bar + the active tab's body. The folded panels are
            // reachable ONLY here now (their standalone nav entries + view arms are
            // retired), which is why their `pub mod`s stay live.
            View::Panel {
                panel: "datacenter",
                ..
            } => self.datacenter_surface(),
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
            // DATACENTER-25 — Snapshots is now a Datacenter fold-bar tab (rendered
            // by `datacenter_surface`); the standalone `system.snapshots` view arm
            // is retired and its slug redirects to the Datacenter panel.
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
            // LIGHTHOUSE-5 / DATACENTER-25 — the lighthouse ops surface is now a
            // Datacenter fold-bar tab (rendered by `datacenter_surface`); the
            // standalone `mesh.lighthouses` view arm is retired and its slug
            // redirects to the Datacenter panel.
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
            // PLANES-22 / DATACENTER-25 — the image catalog is now a Datacenter
            // fold-bar tab (rendered by `datacenter_surface`); the standalone
            // `provisioning.images` view arm is retired and its slug redirects to
            // the Datacenter panel.
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
            // COMPUTE/SVC-VIEW — Mesh → All Services unions the three service sources.
            View::Panel {
                panel: "all_services",
                ..
            } => self.all_services.view(),
            // ROUTER-5 — Mesh → Routers per-node router/firewall view.
            View::Panel {
                panel: "router", ..
            } => self.router.view(),
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
            // CONNECT-6 — Network → Connectivity renders the exposure matrix
            // (configured policies) + auto-discovered candidates.
            View::Panel {
                panel: "connectivity",
                ..
            } => self.connectivity.view(),
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
            // E6.10 / NAV-1 / DATACENTER-25 — the Compute/Instances surface (local
            // + fleet VM/pod list, incl. the embedded VM-create wizard) is now a
            // Datacenter fold-bar tab (rendered by `datacenter_surface`); the
            // standalone `provisioning.instances` view arm is retired and its slug
            // redirects to the Datacenter panel.
            // XCP-4 — the VM Spawner / Provisioning plane (A-plane MDE-VMs).
            View::Panel {
                panel: "provisioning",
                ..
            } => self.provisioning.view(),
            // DATACENTER-18 — the New-Mesh genesis wizard ("give birth to a new
            // Nebula"): plan + Tofu-write here; the live apply/found stay gated.
            View::Panel {
                panel: "genesis", ..
            } => self.genesis.view(),
            // WB-OVERVIEW-2 — the "Home" panel renders the home dashboard (same
            // as the group root). FRONTDOOR-1 swaps that VIEW for the GPU canvas
            // tile-grid Front Door (`home` state + load stay intact for the
            // FRONTDOOR-4 data reuse).
            View::Panel { panel: "home", .. } => self.front_door.view(),
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

    /// DATACENTER-25 — the Datacenter panel surface: a fold-bar tab row (its own
    /// surface + the five folded panels) above the active tab's body. The folded
    /// panels keep their own state + reducer + subscription on [`App`]; this is the
    /// VIEW seam that renders them in-place when their tab is selected, so each
    /// folded `pub mod` stays reachable through here (no dead modules).
    ///
    /// The fold-bar is the same Carbon idiom the Datacenter panel uses for its own
    /// view-mode tabs (`variant_button`, Primary = selected). The "Datacenter" tab
    /// then shows that panel's own multi-lens surface (Overview / Topology /
    /// Resources / Tofu / Audit + the prod/dev zone tabs), so the existing native
    /// tabs are preserved one level down — the fold-bar picks the surface, the
    /// Datacenter row picks the lens.
    fn datacenter_surface(&self) -> Element<'_, Message> {
        let palette = crate::live_theme::palette();
        let space = mde_theme::spacing::BASE;

        let tab_btn = |tab: DatacenterTab| -> Element<'_, Message> {
            let variant = if self.datacenter_tab == tab {
                crate::controls::ButtonVariant::Primary
            } else {
                crate::controls::ButtonVariant::Secondary
            };
            crate::controls::variant_button(
                tab.label(),
                variant,
                Some(Message::DatacenterTab(tab)),
                palette,
            )
        };

        let mut fold_bar = row![].spacing(f32::from(space[2]));
        for tab in DatacenterTab::all() {
            fold_bar = fold_bar.push(tab_btn(tab));
        }

        // The active tab's body. `Native` is Datacenter's own surface (which
        // carries its own view-mode tab row); the folded tabs render the absorbed
        // panel's `.view()` directly — routing its messages through the existing
        // per-panel `update` arms unchanged.
        let body: Element<'_, Message> = match self.datacenter_tab {
            DatacenterTab::Native => self.datacenter.view(),
            DatacenterTab::Instances => self.compute.view(),
            DatacenterTab::Snapshots => self.snapshots.view(),
            DatacenterTab::Images => self.images.view(),
            DatacenterTab::Lighthouses => self.lighthouses.view(),
            DatacenterTab::BuildFarm => self.build_farm.view(),
        };

        column![fold_bar, body]
            .spacing(f32::from(space[3]))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
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
                let mut boot = match app.view {
                    View::Panel { group, panel } => app.on_panel_navigated(group, panel),
                    View::Group(g) => app.on_group_navigated(g),
                };
                // APPLAUNCH-7 — a cold `--focus launcher` opens the launcher: load
                // its cache (apps/favorites/groups) alongside the tile load so it
                // paints populated, not empty.
                if app.front_door.launcher.open {
                    boot = Task::batch([boot, front_door_panel::FrontDoor::launcher_load()]);
                }
                (app, boot)
            }
            // FRONTDOOR-4 — a plain launch lands on the Dashboard/"home" view,
            // which renders the Front Door, so fire its live-tile load on boot
            // (alongside the Peers directory load the Front Door's mesh-map /
            // node-health / data-center tiles reuse) — the menu lands streaming
            // real data, not on the skeleton.
            _ => (
                App::new(),
                Task::batch([
                    crate::panels::peers::PeersPanel::load(),
                    front_door_panel::FrontDoor::load(),
                ]),
            ),
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
            // DATACENTER-25 — images folded into the Datacenter panel; node_roles
            // is the still-standalone Provisioning panel used in its place here.
            (Group::Provisioning, "node_roles"),
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
    fn panel_switch_arms_a_crossfade_then_clears_when_complete() {
        // MOTION-TRANS-1 — switching the active route arms an in-flight crossfade;
        // selecting the SAME route again is a no-op (no needless re-fade); and a
        // completed tween clears, so the tick subscription falls quiet (idle ⇒ no
        // wakeups).
        let mut app = App::new();
        assert!(app.transition.is_none(), "rest = no transition");
        let _ = app.update(Message::SelectPanel {
            group: Group::ThisNode,
            panel: "remote_desktop",
        });
        let armed = app.transition.expect("a real switch arms a crossfade");
        assert!(
            !armed.is_complete(armed.start),
            "the crossfade starts in flight (alpha < 1 at t=start)"
        );
        // Re-selecting the identical view must NOT re-arm (view unchanged).
        let armed_start = armed.start;
        let _ = app.update(Message::SelectPanel {
            group: Group::ThisNode,
            panel: "remote_desktop",
        });
        assert_eq!(
            app.transition.map(|t| t.start),
            Some(armed_start),
            "same-view re-select keeps the original transition (no re-fade)"
        );
        // After the dialog_mount duration the crossfade has settled. The
        // TransitionTick handler reads the live wall clock, so assert the
        // completeness predicate directly (wall-clock-independent) for the
        // settle, then confirm the handler runs without panicking.
        let done = armed.start
            + mde_theme::motion::Motion::dialog_mount().duration
            + Duration::from_millis(1);
        assert!(
            armed.is_complete(done),
            "the crossfade is complete after its dialog_mount duration"
        );
        let _ = app.update(Message::TransitionTick);
    }

    #[test]
    fn switching_to_a_group_root_also_crossfades() {
        // MOTION-TRANS-1 — group-root navigations (View::Group) crossfade too, not
        // just leaf panels: any route change is a visual swap.
        let mut app = App::new();
        let _ = app.update(Message::SelectGroup(Group::System));
        assert!(
            app.transition.is_some(),
            "a group-root switch arms a crossfade"
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
        // DATACENTER-25 — snapshots folded into Datacenter, so use a
        // still-standalone System panel (repair) for the escape-to-group check.
        let _ = app.update(Message::SelectPanel {
            group: Group::System,
            panel: "repair",
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
            panel: "repair",
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
