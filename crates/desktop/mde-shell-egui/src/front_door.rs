//! Shell-owned unified search/omnibox front door.
//!
//! This is the runtime UI slice of the `SEARCH-omnibox` epic. It stays deliberately
//! thin: ranking is the shared `mde_egui::search_omnibox` core, and activation is
//! handed back to the owning shell surface so apps, Files, Explorer, and Browser
//! keep their existing local command paths.

use mde_egui::egui;
use mde_egui::search_omnibox::{ranked_hits, MatchTier, SearchDomain, SearchHit, SearchItem};
use mde_egui::Style;
use mde_files_egui::model::FileSearchTarget;
use mde_theme::brand::icons::IconId;

use crate::console::ConsoleSearchHit;
use crate::datacenter::{FrontDoorLifecycleCandidate, FrontDoorLifecycleKind};
use crate::dock::{icon_texture, launcher_group_label, Surface};
use crate::workbench::Plane;

const AREA_ID: &str = "shell-front-door-omnibox";
const INPUT_ID: &str = "shell-front-door-omnibox-input";
const RESULTS_SCROLL_ID: &str = "shell-front-door-results-scroll";
const MAX_HITS: usize = 12;
const PANEL_W: f32 = 720.0;
const ROW_H: f32 = 42.0;
const INPUT_H: f32 = 38.0;
const FILTER_CHIP_H: f32 = 24.0;
const FILTER_CHIP_GAP: f32 = 5.0;
const ACTION_PANEL_H: f32 = 44.0;
const PANEL_RADIUS: f32 = 8.0;
const PANEL_INSET: f32 = 10.0;
const EXPANSION_BUTTON_W: f32 = INPUT_H;
const EXPANDED_MIN_H: f32 = 320.0;
const PANEL_MIN_W: f32 = 320.0;
const ROW_ICON: f32 = 18.0;
const DOMAIN_W: f32 = 82.0;
const DOMAIN_MIN_W: f32 = 44.0;
const RESULT_TEXT_MIN_W: f32 = 72.0;
const SEARCH_MIN_W: f32 = 72.0;
const TOOLTIP_W: f32 = 260.0;
const ACTION_BUTTON_TEXT_MIN_W: f32 = 72.0;
const SEARCH_TEXT_SIZE: f32 = Style::BODY + 1.0;
const SEARCH_HINT: &str = "Search apps, workloads, services, commands, files, mesh, Browser";

/// Unified-search privacy policy (WL-FUNC-005): the omnibox is **ephemeral and
/// local-only**, matching the browser's private-by-default posture.
///
/// * *Ephemeral* — the live query lives only in [`FrontDoorState::query`] while
///   the panel is open. [`FrontDoorState`] is never serialized, keeps no
///   recent-search list, and [`FrontDoorState::close`] wipes the query, so no
///   persistent search-query history is ever written.
/// * *Local-only* — every candidate producer is a pure in-process scan of
///   already-local data (apps, discovered mesh units, files, Browser
///   bookmarks/history). Typing a query performs no network egress and no
///   telemetry send. A query can only leave the node when the user *explicitly*
///   activates the "Search web for …" row, and even then it navigates to the
///   mesh-local search endpoint (`search.mesh`), never an external provider.
///   There is no Assistant/AI producer that sends the query off-node.
///
/// This is the single documented policy point; it is asserted end-to-end by the
/// `front_door_search_is_ephemeral_and_local_only` test (and the browser-side
/// `omnibox_web_suggestion_stays_on_mesh_local_search_endpoint` test for the one
/// explicit-activation egress seam).
pub(crate) const SEARCH_PRIVACY_EPHEMERAL_LOCAL_ONLY: bool = true;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrontDoorMeshSourceStatus {
    Ready,
    Pending,
    Unavailable,
}

impl FrontDoorMeshSourceStatus {
    const fn allows_results(self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrontDoorSourceStatus {
    pub(crate) mesh: FrontDoorMeshSourceStatus,
}

impl Default for FrontDoorSourceStatus {
    fn default() -> Self {
        Self {
            mesh: FrontDoorMeshSourceStatus::Ready,
        }
    }
}

impl FrontDoorSourceStatus {
    pub(crate) const fn new(mesh: FrontDoorMeshSourceStatus) -> Self {
        Self { mesh }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrontDoorFilter {
    All,
    Apps,
    Mesh,
    Workloads,
    Services,
    Files,
    Browser,
    Commands,
    Web,
}

impl Default for FrontDoorFilter {
    fn default() -> Self {
        Self::All
    }
}

impl FrontDoorFilter {
    const ALL: [Self; 9] = [
        Self::All,
        Self::Apps,
        Self::Mesh,
        Self::Workloads,
        Self::Services,
        Self::Files,
        Self::Browser,
        Self::Commands,
        Self::Web,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Apps => "Apps",
            Self::Mesh => "Mesh",
            Self::Workloads => "Workloads",
            Self::Services => "Services",
            Self::Files => "Files",
            Self::Browser => "Browser",
            Self::Commands => "Commands",
            Self::Web => "Web",
        }
    }

    const fn width(self) -> f32 {
        match self {
            Self::All => 34.0,
            Self::Apps => 46.0,
            Self::Mesh => 46.0,
            Self::Workloads => 78.0,
            Self::Services => 66.0,
            Self::Files => 44.0,
            Self::Browser => 60.0,
            Self::Commands => 74.0,
            Self::Web => 38.0,
        }
    }

    fn matches_item(self, item: &SearchItem<FrontDoorTarget>) -> bool {
        match self {
            Self::All => true,
            Self::Apps => matches!(
                &item.payload,
                FrontDoorTarget::App(_) | FrontDoorTarget::PeerApp(_)
            ),
            Self::Mesh => {
                matches!(item.domain, SearchDomain::Mesh)
                    || matches!(&item.payload, FrontDoorTarget::PeerApp(_))
            }
            Self::Workloads => {
                matches!(
                    &item.payload,
                    FrontDoorTarget::Workflow(card) if card.kind == FrontDoorWorkflowKind::Workload
                ) || matches!(
                    &item.payload,
                    FrontDoorTarget::ServiceLifecycle(target)
                        if target.kind == FrontDoorLifecycleKind::Vm
                )
            }
            Self::Services => {
                matches!(
                    &item.payload,
                    FrontDoorTarget::Workflow(card) if card.kind == FrontDoorWorkflowKind::Service
                ) || matches!(&item.payload, FrontDoorTarget::ServiceLifecycle(_))
            }
            Self::Files => matches!(item.domain, SearchDomain::File),
            Self::Browser => matches!(
                item.domain,
                SearchDomain::BrowserBookmark | SearchDomain::BrowserHistory
            ),
            Self::Commands => matches!(&item.payload, FrontDoorTarget::ConsoleCommand(_)),
            Self::Web => matches!(
                item.domain,
                SearchDomain::WebSuggestion | SearchDomain::Assistant
            ),
        }
    }

    const fn empty_text(self, blank: bool) -> &'static str {
        match (blank, self) {
            (true, Self::All) => "Type to search",
            (true, Self::Apps) => "No app shortcuts",
            (true, Self::Mesh) => "No mesh shortcuts",
            (true, Self::Workloads) => "No workload shortcuts",
            (true, Self::Services) => "No service shortcuts",
            (true, Self::Files) => "No file shortcuts",
            (true, Self::Browser) => "No Browser shortcuts",
            (true, Self::Commands) => "No command shortcuts",
            (true, Self::Web) => "No web shortcuts",
            (false, Self::All) => "No local matches",
            (false, Self::Apps) => "No app matches",
            (false, Self::Mesh) => "No mesh matches",
            (false, Self::Workloads) => "No workload matches",
            (false, Self::Services) => "No service matches",
            (false, Self::Files) => "No file matches",
            (false, Self::Browser) => "No Browser matches",
            (false, Self::Commands) => "No command matches",
            (false, Self::Web) => "No web matches",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrontDoorFilterStep {
    Previous,
    Next,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrontDoorTarget {
    App(Surface),
    Workflow(FrontDoorWorkflowCard),
    ServiceLifecycle(FrontDoorServiceLifecycleTarget),
    PeerApp(FrontDoorPeerAppTarget),
    File(FileSearchTarget),
    Mesh(String),
    Browser(String),
    ConsoleCommand(ConsoleSearchHit),
    RunCommand(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FrontDoorRequest {
    Activate(FrontDoorTarget),
    LaunchPeerApp(FrontDoorPeerAppTarget),
    ConnectDesktopSource(String),
    InstanceLifecycle {
        unit_id: String,
        op: FrontDoorInstanceLifecycleOp,
    },
    ServiceLifecycle {
        target: FrontDoorServiceLifecycleTarget,
        op: FrontDoorServiceLifecycleOp,
    },
    OpenWorkbenchPlane(Plane),
    TogglePin(Surface),
    MovePin {
        surface: Surface,
        direction: FrontDoorPinMoveDirection,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrontDoorPeerApp {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) node: String,
    pub(crate) source: String,
    pub(crate) icon: String,
    pub(crate) health: String,
    pub(crate) state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrontDoorPeerAppTarget {
    pub(crate) node: String,
    pub(crate) app_id: String,
    pub(crate) name: String,
}

impl FrontDoorPeerAppTarget {
    pub(crate) fn desktop_source_id(&self) -> String {
        format!("peer:{}", self.node)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrontDoorServiceLifecycleTarget {
    pub(crate) host: String,
    pub(crate) kind: FrontDoorLifecycleKind,
    pub(crate) name: String,
    pub(crate) state: String,
}

impl FrontDoorServiceLifecycleTarget {
    fn target_line(&self) -> String {
        format!("{} on {} · {}", self.kind.label(), self.host, self.state)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FrontDoorServiceLifecycleOp {
    Start,
    Stop,
    Restart,
}

impl FrontDoorServiceLifecycleOp {
    const fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Restart => "Restart",
        }
    }

    const fn wire(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }

    const fn icon(self) -> IconId {
        match self {
            Self::Start => IconId::Play,
            Self::Stop => IconId::MediaStop,
            Self::Restart => IconId::Reload,
        }
    }

    const fn destructive(self) -> bool {
        matches!(self, Self::Stop | Self::Restart)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FrontDoorInstanceLifecycleOp {
    Start,
    Stop,
    Reboot,
}

impl FrontDoorInstanceLifecycleOp {
    const fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Reboot => "Reboot",
        }
    }

    const fn cloud_verb(self) -> &'static str {
        match self {
            Self::Start => "instance-start",
            Self::Stop => "instance-stop",
            Self::Reboot => "instance-reboot",
        }
    }

    const fn icon(self) -> IconId {
        match self {
            Self::Start => IconId::Play,
            Self::Stop => IconId::MediaStop,
            Self::Reboot => IconId::Reload,
        }
    }

    const fn destructive(self) -> bool {
        matches!(self, Self::Stop | Self::Reboot)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FrontDoorPinMoveDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrontDoorWorkflowKind {
    Workload,
    Service,
}

impl FrontDoorWorkflowKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Workload => "Workload",
            Self::Service => "Service",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrontDoorWorkflowCard {
    pub(crate) kind: FrontDoorWorkflowKind,
    pub(crate) surface: Surface,
    pub(crate) workbench_plane: Option<Plane>,
    title: &'static str,
    target: &'static str,
    terms: &'static [&'static str],
    icon: IconId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrontDoorWorkflowQuickAction {
    label: &'static str,
    plane: Plane,
    icon: IconId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrontDoorInstanceLifecycleTarget {
    unit_id: String,
    instance: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrontDoorLifecycleArm {
    unit_id: String,
    op: FrontDoorInstanceLifecycleOp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FrontDoorServiceLifecycleArm {
    target: FrontDoorServiceLifecycleTarget,
    op: FrontDoorServiceLifecycleOp,
}

const WORKLOAD_CARDS: [FrontDoorWorkflowCard; 2] = [
    FrontDoorWorkflowCard {
        kind: FrontDoorWorkflowKind::Workload,
        surface: Surface::InfraCode,
        workbench_plane: None,
        title: "Cloud workloads",
        target: "Instances, volumes, networks",
        terms: &[
            "workloads",
            "cloud",
            "instances",
            "volumes",
            "networks",
            "iaas",
        ],
        icon: IconId::Server,
    },
    FrontDoorWorkflowCard {
        kind: FrontDoorWorkflowKind::Workload,
        surface: Surface::Desktop,
        workbench_plane: None,
        title: "Desktop sessions",
        target: "Virtual desktops, remote seats, VDI",
        terms: &[
            "workloads",
            "desktop",
            "vdi",
            "remote",
            "sessions",
            "virtual machines",
        ],
        icon: IconId::Desktop,
    },
];

const SERVICE_CARDS: [FrontDoorWorkflowCard; 2] = [
    FrontDoorWorkflowCard {
        kind: FrontDoorWorkflowKind::Service,
        surface: Surface::Workbench,
        workbench_plane: Some(Plane::Provisioning),
        title: "Mesh services",
        target: "Fleet service health and controls",
        terms: &[
            "services", "mesh", "fleet", "health", "mackesd", "nebula", "systemd",
        ],
        icon: IconId::Workbench,
    },
    FrontDoorWorkflowCard {
        kind: FrontDoorWorkflowKind::Service,
        surface: Surface::InfraCode,
        workbench_plane: None,
        title: "Cloud API services",
        target: "Service catalog, endpoints, health",
        terms: &[
            "services",
            "cloud",
            "catalog",
            "api",
            "endpoints",
            "compute",
            "network",
        ],
        icon: IconId::Server,
    },
];

#[derive(Debug, Default)]
pub(crate) struct FrontDoorState {
    open: bool,
    query: String,
    selected: usize,
    filter: FrontDoorFilter,
    expanded: bool,
    focus_pending: bool,
    suppress_click_away_once: bool,
    lifecycle_arm: Option<FrontDoorLifecycleArm>,
    service_lifecycle_arm: Option<FrontDoorServiceLifecycleArm>,
}

impl FrontDoorState {
    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn open(&mut self) {
        self.open = true;
        self.focus_pending = true;
        self.selected = 0;
        self.filter = FrontDoorFilter::All;
        self.expanded = false;
        self.lifecycle_arm = None;
        self.service_lifecycle_arm = None;
        self.suppress_click_away_once = true;
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
        self.filter = FrontDoorFilter::All;
        self.expanded = false;
        self.focus_pending = false;
        self.lifecycle_arm = None;
        self.service_lifecycle_arm = None;
        self.suppress_click_away_once = false;
    }

    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    pub(crate) fn selected_peer_node(
        &self,
        items: Vec<SearchItem<FrontDoorTarget>>,
        sources: FrontDoorSourceStatus,
    ) -> Option<String> {
        if !self.open || run_command_mode(&self.query) {
            return None;
        }
        let hits = visible_front_door_hits_for_filter_with_sources(
            &self.query,
            self.filter,
            items,
            sources,
        );
        let hit = hits.get(self.selected.min(hits.len().saturating_sub(1)))?;
        match &hit.item.payload {
            FrontDoorTarget::Mesh(id) => peer_node_for_unit_id(id).map(str::to_owned),
            FrontDoorTarget::PeerApp(target) => Some(target.node.clone()),
            _ => None,
        }
    }
}

pub(crate) fn app_search_items() -> Vec<SearchItem<FrontDoorTarget>> {
    app_search_items_with_pins(&[])
}

pub(crate) fn app_search_items_with_pins(pinned: &[Surface]) -> Vec<SearchItem<FrontDoorTarget>> {
    let mut ordered = Vec::with_capacity(Surface::ALL.len());
    for &surface in pinned {
        if Surface::ALL.contains(&surface) && !ordered.contains(&surface) {
            ordered.push(surface);
        }
    }
    for surface in Surface::ALL {
        if !ordered.contains(&surface) {
            ordered.push(surface);
        }
    }

    ordered
        .into_iter()
        .enumerate()
        .map(|(idx, surface)| app_search_item(surface, idx))
        .collect()
}

pub(crate) fn workflow_search_items(rank_offset: usize) -> Vec<SearchItem<FrontDoorTarget>> {
    WORKLOAD_CARDS
        .iter()
        .chain(SERVICE_CARDS.iter())
        .enumerate()
        .map(|(idx, card)| workflow_search_item(*card, rank_offset + idx))
        .collect()
}

pub(crate) fn service_lifecycle_search_items(
    candidates: Vec<FrontDoorLifecycleCandidate>,
    rank_offset: usize,
) -> Vec<SearchItem<FrontDoorTarget>> {
    candidates
        .into_iter()
        .enumerate()
        .map(|(idx, candidate)| {
            let FrontDoorLifecycleCandidate {
                host,
                kind,
                name,
                state,
                detail,
            } = candidate;
            let title = format!("{} {}", name, kind.label());
            let target = FrontDoorServiceLifecycleTarget {
                host,
                kind,
                name,
                state,
            };
            let target_line = target.target_line();
            let terms = vec![
                "service lifecycle".to_owned(),
                target.host.clone(),
                target.kind.label().to_owned(),
                target.state.clone(),
                detail,
                idx.to_string(),
            ];
            SearchItem::new(
                SearchDomain::App,
                title,
                target_line,
                FrontDoorTarget::ServiceLifecycle(target),
            )
            .with_terms(terms)
            .with_source_rank(rank_offset + idx)
        })
        .collect()
}

pub(crate) fn peer_app_search_items(
    apps: impl IntoIterator<Item = FrontDoorPeerApp>,
    rank_offset: usize,
) -> Vec<SearchItem<FrontDoorTarget>> {
    apps.into_iter()
        .enumerate()
        .filter_map(|(idx, app)| {
            let node = app.node.trim();
            let name = app.name.trim();
            let id = app.id.trim();
            if node.is_empty() || name.is_empty() || id.is_empty() {
                return None;
            }
            let target = FrontDoorPeerAppTarget {
                node: node.to_owned(),
                app_id: id.to_owned(),
                name: name.to_owned(),
            };
            let target_line = format!("on {} · {}", node, peer_app_source_label(&app.source));
            let mut terms = vec![
                "peer app".to_owned(),
                "remote app".to_owned(),
                node.to_owned(),
                id.to_owned(),
                app.source,
                app.icon,
            ];
            if !app.health.trim().is_empty() {
                terms.push(app.health);
            }
            if !app.state.trim().is_empty() {
                terms.push(app.state);
            }
            Some(
                SearchItem::new(
                    SearchDomain::App,
                    name.to_owned(),
                    target_line,
                    FrontDoorTarget::PeerApp(target),
                )
                .with_terms(terms)
                .with_source_rank(rank_offset + idx),
            )
        })
        .collect()
}

fn peer_app_source_label(source: &str) -> &'static str {
    if source.trim().eq_ignore_ascii_case("flatpak") {
        "Flatpak"
    } else {
        "desktop app"
    }
}

fn app_search_item(surface: Surface, idx: usize) -> SearchItem<FrontDoorTarget> {
    SearchItem::new(
        SearchDomain::App,
        surface.label(),
        launcher_group_label(surface),
        FrontDoorTarget::App(surface),
    )
    .with_terms(vec![
        format!("{surface:?}"),
        launcher_group_label(surface).to_owned(),
        app_surface_keywords(surface).to_owned(),
    ])
    .with_source_rank(idx)
}

const fn app_surface_keywords(surface: Surface) -> &'static str {
    match surface {
        Surface::Workbench => "services provisioning fleet mesh control",
        Surface::InfraCode => "workloads services iaas cloud catalog",
        Surface::Desktop => "workloads sessions vdi virtual machines remote desktop",
        Surface::Communications => {
            "communications collaboration spaces activity messages chat threads calls presence comms"
        }
        _ => "",
    }
}

fn workflow_search_item(card: FrontDoorWorkflowCard, rank: usize) -> SearchItem<FrontDoorTarget> {
    SearchItem::new(
        SearchDomain::App,
        card.title,
        card.target,
        FrontDoorTarget::Workflow(card),
    )
    .with_terms(
        card.terms
            .iter()
            .copied()
            .chain([card.kind.label(), card.surface.label()]),
    )
    .with_source_rank(rank)
}

pub(crate) fn console_command_search_item(
    hit: ConsoleSearchHit,
    rank: usize,
) -> SearchItem<FrontDoorTarget> {
    SearchItem::new(
        SearchDomain::App,
        hit.label,
        hit.desc,
        FrontDoorTarget::ConsoleCommand(hit),
    )
    .with_terms(["Console", "Command", hit.group, hit.tool])
    .with_source_rank(rank)
}

fn initial_front_door_hits(
    items: Vec<SearchItem<FrontDoorTarget>>,
) -> Vec<SearchHit<FrontDoorTarget>> {
    items
        .into_iter()
        .take(MAX_HITS)
        .map(|item| SearchHit {
            item,
            // The renderer does not use lexical tier for the empty-query panel.
            // Keep the value deterministic without changing the shared ranker.
            tier: MatchTier::AuxiliarySubstring,
        })
        .collect()
}

fn filtered_front_door_items(
    items: Vec<SearchItem<FrontDoorTarget>>,
    filter: FrontDoorFilter,
    sources: FrontDoorSourceStatus,
) -> Vec<SearchItem<FrontDoorTarget>> {
    items
        .into_iter()
        .filter(|item| filter.matches_item(item) && source_allows_item(sources, item))
        .collect()
}

pub(crate) fn visible_front_door_hits(
    query: &str,
    items: Vec<SearchItem<FrontDoorTarget>>,
) -> Vec<SearchHit<FrontDoorTarget>> {
    visible_front_door_hits_for_filter(query, FrontDoorFilter::All, items)
}

pub(crate) fn visible_front_door_hits_for_filter(
    query: &str,
    filter: FrontDoorFilter,
    items: Vec<SearchItem<FrontDoorTarget>>,
) -> Vec<SearchHit<FrontDoorTarget>> {
    visible_front_door_hits_for_filter_with_sources(
        query,
        filter,
        items,
        FrontDoorSourceStatus::default(),
    )
}

fn visible_front_door_hits_for_filter_with_sources(
    query: &str,
    filter: FrontDoorFilter,
    items: Vec<SearchItem<FrontDoorTarget>>,
    sources: FrontDoorSourceStatus,
) -> Vec<SearchHit<FrontDoorTarget>> {
    let items = filtered_front_door_items(items, filter, sources);
    if query.trim().is_empty() {
        initial_front_door_hits(items)
    } else {
        ranked_front_door_hits(query, items)
    }
}

fn source_allows_item(sources: FrontDoorSourceStatus, item: &SearchItem<FrontDoorTarget>) -> bool {
    let needs_mesh = matches!(item.domain, SearchDomain::Mesh)
        || matches!(&item.payload, FrontDoorTarget::PeerApp(_));
    !needs_mesh || sources.mesh.allows_results()
}

pub(crate) fn ranked_front_door_hits(
    query: &str,
    items: Vec<SearchItem<FrontDoorTarget>>,
) -> Vec<SearchHit<FrontDoorTarget>> {
    ranked_hits(query, items, MAX_HITS)
}

fn run_command_query(query: &str) -> Option<&str> {
    let query = query.trim_start();
    let command = query.strip_prefix('>')?.trim();
    (!command.is_empty()).then_some(command)
}

fn run_command_mode(query: &str) -> bool {
    query.trim_start().starts_with('>')
}

fn front_door_screen_margin(screen: egui::Rect) -> f32 {
    Style::SP_L.min((screen.width() * 0.04).max(8.0))
}

fn front_door_panel_width(screen: egui::Rect, expanded: bool) -> f32 {
    let margin = front_door_screen_margin(screen);
    let available = (screen.width() - margin * 2.0).max(1.0);
    let min_width = PANEL_MIN_W.min(available);
    if expanded {
        available
    } else {
        PANEL_W.min(available).max(min_width)
    }
}

fn front_door_panel_top(screen: egui::Rect, expanded: bool) -> f32 {
    if expanded {
        screen.top() + front_door_screen_margin(screen)
    } else {
        screen.top() + (screen.height() * 0.14).max(Style::SP_XL)
    }
}

fn front_door_panel_pos(screen: egui::Rect, width: f32, expanded: bool) -> egui::Pos2 {
    if expanded {
        egui::pos2(
            screen.left() + front_door_screen_margin(screen),
            front_door_panel_top(screen, true),
        )
    } else {
        egui::pos2(
            screen.center().x - width / 2.0,
            front_door_panel_top(screen, false),
        )
    }
}

fn front_door_panel_min_height(screen: egui::Rect, expanded: bool) -> Option<f32> {
    expanded.then(|| front_door_panel_height(screen, true))
}

fn front_door_panel_height(screen: egui::Rect, expanded: bool) -> f32 {
    let top = front_door_panel_top(screen, expanded);
    let available = (screen.bottom() - top - front_door_screen_margin(screen)).max(1.0);
    if expanded {
        return available.max(EXPANDED_MIN_H.min(available)).min(available);
    }

    let minimum = (front_door_non_results_height() + ROW_H).min(available);
    let preferred = front_door_non_results_height() + front_door_results_content_height(MAX_HITS);
    preferred.min(available).max(minimum)
}

fn front_door_panel_rect(screen: egui::Rect, expanded: bool) -> egui::Rect {
    let width = front_door_panel_width(screen, expanded);
    egui::Rect::from_min_size(
        front_door_panel_pos(screen, width, expanded),
        egui::vec2(width, front_door_panel_height(screen, expanded)),
    )
}

fn front_door_non_results_height() -> f32 {
    PANEL_INSET * 2.0 + INPUT_H + Style::SP_XS + FILTER_CHIP_H + Style::SP_XS
}

fn front_door_results_content_height(hit_count: usize) -> f32 {
    let rows = ROW_H * hit_count.max(1) as f32;
    if hit_count == 0 {
        rows
    } else {
        rows + ACTION_PANEL_H
    }
}

fn front_door_results_max_height(screen: egui::Rect, expanded: bool) -> f32 {
    (front_door_panel_height(screen, expanded) - front_door_non_results_height()).max(ROW_H)
}

fn bounded_available_width(ui: &egui::Ui) -> f32 {
    let clip_remaining = (ui.clip_rect().right() - ui.next_widget_position().x).max(0.0);
    ui.available_width().max(0.0).min(clip_remaining)
}

pub(crate) fn front_door_panel(
    ctx: &egui::Context,
    state: &mut FrontDoorState,
    items: Vec<SearchItem<FrontDoorTarget>>,
    pinned: &[Surface],
) -> Option<FrontDoorRequest> {
    front_door_panel_with_sources(ctx, state, items, pinned, FrontDoorSourceStatus::default())
}

pub(crate) fn front_door_panel_with_sources(
    ctx: &egui::Context,
    state: &mut FrontDoorState,
    items: Vec<SearchItem<FrontDoorTarget>>,
    pinned: &[Surface],
    sources: FrontDoorSourceStatus,
) -> Option<FrontDoorRequest> {
    if !state.open {
        return None;
    }

    let screen = ctx.screen_rect();
    let panel_rect = front_door_panel_rect(screen, state.expanded);
    let results_max_h = front_door_results_max_height(screen, state.expanded);
    let mut action = None;

    let area = egui::Area::new(egui::Id::new(AREA_ID))
        .order(egui::Order::Foreground)
        .fixed_pos(panel_rect.min)
        .constrain(false)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(panel_rect.size(), egui::Sense::hover());
            paint_front_door_panel_frame(ui, rect);
            let inner_rect = rect.shrink(PANEL_INSET);
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(inner_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            child.set_clip_rect(inner_rect);
            {
                let ui = &mut child;
                let input_id = egui::Id::new(INPUT_ID);
                let response = search_and_expansion_row(ui, input_id, state);
                install_search_accessibility(ui.ctx(), response.rect, &state.query);
                if state.focus_pending {
                    response.request_focus();
                    state.focus_pending = false;
                }
                if response.changed() {
                    state.selected = 0;
                    state.lifecycle_arm = None;
                    state.service_lifecycle_arm = None;
                }

                ui.add_space(Style::SP_XS);
                let (filter_rect, filter_changed) = filter_chip_row(ui, &mut state.filter);
                if filter_changed {
                    state.selected = 0;
                    state.lifecycle_arm = None;
                    state.service_lifecycle_arm = None;
                }
                if let Some(step) = filter_keyboard_step(ui) {
                    state.filter = moved_filter(state.filter, step);
                    state.selected = 0;
                    state.lifecycle_arm = None;
                    state.service_lifecycle_arm = None;
                }

                let command_mode = run_command_mode(&state.query);
                let run_command = run_command_query(&state.query).map(str::to_owned);
                let hits = if command_mode {
                    Vec::new()
                } else {
                    visible_front_door_hits_for_filter_with_sources(
                        &state.query,
                        state.filter,
                        items,
                        sources,
                    )
                };
                if command_mode {
                    state.selected = 0;
                } else if !hits.is_empty() {
                    state.selected = state.selected.min(hits.len().saturating_sub(1));
                } else {
                    state.selected = 0;
                }

                let (escape, enter, up, down) = ui.input(|i| {
                    (
                        i.key_pressed(egui::Key::Escape),
                        i.key_pressed(egui::Key::Enter),
                        i.key_pressed(egui::Key::ArrowUp),
                        i.key_pressed(egui::Key::ArrowDown),
                    )
                });
                if escape {
                    state.close();
                    return;
                }
                if let Some(command) = run_command.as_ref() {
                    if enter {
                        action = Some(FrontDoorRequest::Activate(FrontDoorTarget::RunCommand(
                            command.clone(),
                        )));
                    }
                } else if !hits.is_empty() {
                    if down {
                        state.selected = (state.selected + 1) % hits.len();
                        state.lifecycle_arm = None;
                        state.service_lifecycle_arm = None;
                    }
                    if up {
                        state.selected = if state.selected == 0 {
                            hits.len() - 1
                        } else {
                            state.selected - 1
                        };
                        state.lifecycle_arm = None;
                        state.service_lifecycle_arm = None;
                    }
                    if enter {
                        action = hits.get(state.selected).map(activation_request_for_hit);
                    }
                }

                ui.add_space(Style::SP_XS);
                let results_top = filter_rect.bottom() + Style::SP_XS;
                let results_content_h = if command_mode {
                    if run_command.is_some() {
                        ROW_H + ACTION_PANEL_H
                    } else {
                        ROW_H
                    }
                } else {
                    front_door_results_content_height(hits.len())
                };
                let results_h = results_content_h.min(results_max_h);
                let results_rect = egui::Rect::from_min_max(
                    egui::pos2(filter_rect.left(), results_top),
                    egui::pos2(filter_rect.right(), results_top + results_h),
                );
                if let Some(command) = run_command.as_ref() {
                    install_run_command_accessibility(ui.ctx(), results_rect, command);
                    let response = run_command_row(ui, command);
                    if response.clicked() {
                        state.selected = 0;
                    }
                    if run_command_action_panel(ui, command).clicked() {
                        action = Some(FrontDoorRequest::Activate(FrontDoorTarget::RunCommand(
                            command.clone(),
                        )));
                    }
                } else if command_mode {
                    install_run_command_prompt_accessibility(ui.ctx(), results_rect);
                    command_empty_note(ui);
                } else {
                    install_results_announcement(
                        ui.ctx(),
                        results_rect,
                        &state.query,
                        &hits,
                        state.selected,
                        state.filter,
                        sources,
                    );
                    if hits.is_empty() {
                        if let Some(note) =
                            front_door_source_note(&state.query, state.filter, sources)
                        {
                            source_status_note(ui, note);
                            install_source_status_accessibility(ui.ctx(), results_rect, note);
                        } else {
                            empty_note(ui, state.query.trim().is_empty(), state.filter);
                        }
                    } else {
                        egui::ScrollArea::vertical()
                            .id_salt(RESULTS_SCROLL_ID)
                            .max_height(results_h)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.set_width(inner_rect.width());
                                for (idx, hit) in hits.iter().enumerate() {
                                    let selected = idx == state.selected;
                                    let response = option_row(ui, hit, idx, hits.len(), selected);
                                    if selected {
                                        response.scroll_to_me(Some(egui::Align::Center));
                                    }
                                    if response.clicked() {
                                        if state.selected != idx {
                                            state.lifecycle_arm = None;
                                            state.service_lifecycle_arm = None;
                                        }
                                        state.selected = idx;
                                    }
                                    if let Some(request) =
                                        result_context_menu_request(&response, hit, pinned)
                                    {
                                        action = Some(request);
                                    }
                                    if selected {
                                        if let Some(request) =
                                            result_action_panel(ui, state, hit, pinned)
                                        {
                                            action = Some(request);
                                        }
                                    }
                                }
                            });
                    }
                }
            }
        });

    if front_door_clicked_elsewhere_should_close(state, area.response.clicked_elsewhere()) {
        state.close();
    }
    if matches!(
        action,
        Some(
            FrontDoorRequest::Activate(_)
                | FrontDoorRequest::LaunchPeerApp(_)
                | FrontDoorRequest::ConnectDesktopSource(_)
                | FrontDoorRequest::OpenWorkbenchPlane(_)
        )
    ) {
        state.close();
    }
    action
}

fn paint_front_door_panel_frame(ui: &egui::Ui, rect: egui::Rect) {
    ui.painter().rect_filled(rect, PANEL_RADIUS, Style::SURFACE);
    ui.painter().rect_stroke(
        rect,
        PANEL_RADIUS,
        egui::Stroke::new(1.0, Style::BORDER),
        egui::StrokeKind::Inside,
    );
}

fn expansion_toggle_id() -> egui::Id {
    egui::Id::new("shell-front-door-expansion-toggle")
}

fn expansion_toggle_value(expanded: bool) -> &'static str {
    if expanded {
        "Full-screen"
    } else {
        "Panel"
    }
}

fn install_expansion_accessibility(ctx: &egui::Context, rect: egui::Rect, expanded: bool) {
    let _ = ctx.accesskit_node_builder(expansion_toggle_id(), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label("Front Door layout");
        node.set_value(expansion_toggle_value(expanded));
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
        if expanded {
            node.set_selected(true);
        }
    });
}

fn front_door_tooltip(ui: &mut egui::Ui, text: &str) {
    egui::Frame::NONE
        .fill(Style::SURFACE)
        .stroke(egui::Stroke::new(1.0, Style::BORDER))
        .corner_radius(8.0)
        .inner_margin(Style::tooltip_margin())
        .show(ui, |ui| {
            ui.set_max_width(TOOLTIP_W);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .size(Style::SMALL)
                        .color(Style::TEXT),
                )
                .wrap(),
            );
        });
}

fn front_door_hover_text(response: egui::Response, text: impl Into<String>) -> egui::Response {
    let text = text.into();
    response.on_hover_ui(move |ui| front_door_tooltip(ui, text.as_str()))
}

fn expansion_toggle_button(ui: &mut egui::Ui, expanded: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(EXPANSION_BUTTON_W, INPUT_H),
        egui::Sense::click(),
    );
    let selected = expanded;
    let fill = if selected {
        Style::ACCENT.linear_multiply(0.16)
    } else if response.hovered() {
        Style::SURFACE_HI
    } else {
        Style::SURFACE
    };
    let stroke = if selected {
        egui::Stroke::new(1.0, Style::ACCENT)
    } else {
        egui::Stroke::new(1.0, Style::BORDER)
    };
    ui.painter().rect_filled(rect, 6.0, fill);
    ui.painter()
        .rect_stroke(rect, 6.0, stroke, egui::StrokeKind::Inside);

    let icon = if expanded {
        IconId::Remove
    } else {
        IconId::Add
    };
    let tint = if selected || response.hovered() {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(16.0, 16.0));
    if let Some(tex) = icon_texture(ui.ctx(), icon, 16.0, tint) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        ui.painter()
            .image(tex.id(), icon_rect, uv, egui::Color32::WHITE);
    }

    install_expansion_accessibility(ui.ctx(), rect, expanded);
    let clicked = response.clicked();
    let hover = if expanded {
        "Return Front Door to panel mode"
    } else {
        "Expand Front Door"
    };
    let _ = front_door_hover_text(response, hover);
    clicked
}

fn search_and_expansion_row(
    ui: &mut egui::Ui,
    input_id: egui::Id,
    state: &mut FrontDoorState,
) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = Style::SP_XS;
        let available = bounded_available_width(ui);
        let show_expansion = show_expansion_control(available);
        let search_w = search_field_width(available, show_expansion);
        let response = front_door_search_field(ui, input_id, &mut state.query, search_w);
        if show_expansion && expansion_toggle_button(ui, state.expanded) {
            state.expanded = !state.expanded;
        }
        response
    })
    .inner
}

fn front_door_search_field(
    ui: &mut egui::Ui,
    input_id: egui::Id,
    query: &mut String,
    width: f32,
) -> egui::Response {
    let width = width.max(1.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, INPUT_H), egui::Sense::hover());
    let focused = ui.memory(|mem| mem.has_focus(input_id));
    let fill = if focused {
        Style::SURFACE_HI
    } else {
        Style::SURFACE
    };
    let stroke = if focused {
        egui::Stroke::new(1.0, Style::ACCENT)
    } else {
        egui::Stroke::new(1.0, Style::BORDER)
    };
    ui.painter().rect_filled(rect, Style::RADIUS_M, fill);
    ui.painter()
        .rect_stroke(rect, Style::RADIUS_M, stroke, egui::StrokeKind::Inside);

    let edit_rect = rect.shrink2(egui::vec2(Style::SP_S, Style::SP_XS));
    let response = ui
        .scope_builder(egui::UiBuilder::new().max_rect(edit_rect), |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            ui.add_sized(
                edit_rect.size(),
                egui::TextEdit::singleline(query)
                    .id(input_id)
                    .frame(false)
                    .font(egui::FontId::proportional(SEARCH_TEXT_SIZE))
                    .text_color(Style::TEXT)
                    .hint_text(
                        egui::RichText::new(SEARCH_HINT)
                            .size(SEARCH_TEXT_SIZE)
                            .color(Style::TEXT_DIM),
                    )
                    .desired_width(f32::INFINITY),
            )
        })
        .inner;
    mde_egui::focus::paint_focus_ring(ui.painter(), rect, response.has_focus());
    response
}

fn show_expansion_control(available_width: f32) -> bool {
    available_width >= EXPANSION_BUTTON_W + Style::SP_XS + SEARCH_MIN_W
}

fn search_field_width(available_width: f32, expansion_visible: bool) -> f32 {
    let reserve = if expansion_visible {
        EXPANSION_BUTTON_W + Style::SP_XS
    } else {
        0.0
    };
    (available_width.max(0.0) - reserve).max(1.0)
}

fn filter_chip_id(filter: FrontDoorFilter) -> egui::Id {
    egui::Id::new(("shell-front-door-filter-chip", filter.label()))
}

fn filter_chip_widths(available_width: f32) -> [f32; FrontDoorFilter::ALL.len()] {
    let available_width = available_width.max(0.0);
    let total_gap = FILTER_CHIP_GAP * (FrontDoorFilter::ALL.len().saturating_sub(1) as f32);
    let label_width: f32 = FrontDoorFilter::ALL
        .iter()
        .map(|filter| filter.width())
        .sum();
    let chip_budget = (available_width - total_gap).max(0.0);
    let scale = if label_width <= chip_budget || label_width <= 0.0 {
        1.0
    } else {
        chip_budget / label_width
    };
    let mut widths = [0.0; FrontDoorFilter::ALL.len()];
    for (idx, filter) in FrontDoorFilter::ALL.iter().enumerate() {
        widths[idx] = (filter.width() * scale).max(1.0);
    }
    widths
}

fn filter_index(filter: FrontDoorFilter) -> usize {
    FrontDoorFilter::ALL
        .iter()
        .position(|candidate| *candidate == filter)
        .expect("Front Door filter belongs to the static filter table")
}

fn moved_filter(filter: FrontDoorFilter, step: FrontDoorFilterStep) -> FrontDoorFilter {
    let len = FrontDoorFilter::ALL.len();
    let index = filter_index(filter);
    let next = match step {
        FrontDoorFilterStep::Previous => (index + len - 1) % len,
        FrontDoorFilterStep::Next => (index + 1) % len,
    };
    FrontDoorFilter::ALL[next]
}

fn filter_keyboard_step(ui: &egui::Ui) -> Option<FrontDoorFilterStep> {
    ui.input(|input| {
        if input.modifiers.ctrl && input.key_pressed(egui::Key::Tab) {
            return Some(if input.modifiers.shift {
                FrontDoorFilterStep::Previous
            } else {
                FrontDoorFilterStep::Next
            });
        }

        if input.modifiers.alt && input.key_pressed(egui::Key::ArrowLeft) {
            return Some(FrontDoorFilterStep::Previous);
        }

        if input.modifiers.alt && input.key_pressed(egui::Key::ArrowRight) {
            return Some(FrontDoorFilterStep::Next);
        }

        None
    })
}

fn filter_chip_row(ui: &mut egui::Ui, active: &mut FrontDoorFilter) -> (egui::Rect, bool) {
    let available_width = bounded_available_width(ui);
    let (row_rect, _) = ui.allocate_exact_size(
        egui::vec2(available_width, FILTER_CHIP_H),
        egui::Sense::hover(),
    );
    let mut changed = false;
    let mut x = row_rect.left();
    let widths = filter_chip_widths(row_rect.width());
    for (idx, filter) in FrontDoorFilter::ALL.into_iter().enumerate() {
        let chip_w = widths[idx].min((row_rect.right() - x).max(0.0));
        if chip_w <= 0.0 {
            break;
        }
        let rect = egui::Rect::from_min_size(
            egui::pos2(x, row_rect.top()),
            egui::vec2(chip_w, FILTER_CHIP_H),
        );
        let response = ui.interact(rect, filter_chip_id(filter), egui::Sense::click());
        if crate::dock::response_activated(ui, &response) && *active != filter {
            *active = filter;
            changed = true;
        }
        let selected = *active == filter;
        let fill = if selected {
            Style::ACCENT.linear_multiply(0.18)
        } else if response.hovered() {
            Style::SURFACE_HI
        } else {
            Style::SURFACE
        };
        let stroke = if selected {
            egui::Stroke::new(1.0, Style::ACCENT)
        } else {
            egui::Stroke::new(1.0, Style::BORDER)
        };
        ui.painter().rect_filled(rect, FILTER_CHIP_H * 0.5, fill);
        ui.painter()
            .rect_stroke(rect, FILTER_CHIP_H * 0.5, stroke, egui::StrokeKind::Inside);
        ui.painter()
            .with_clip_rect(rect.shrink2(egui::vec2(3.0, 0.0)))
            .text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                filter.label(),
                egui::FontId::proportional(11.0),
                if selected {
                    Style::TEXT
                } else {
                    Style::TEXT_DIM
                },
            );
        install_filter_accessibility(ui.ctx(), filter, rect, selected);
        mde_egui::focus::paint_focus_ring(ui.painter(), rect, response.has_focus());
        x = rect.right() + FILTER_CHIP_GAP;
    }
    (row_rect, changed)
}

fn empty_note(ui: &mut egui::Ui, blank: bool, filter: FrontDoorFilter) {
    let text = filter.empty_text(blank);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::hover(),
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(13.0),
        Style::TEXT_DIM,
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrontDoorSourceNote {
    label: &'static str,
    detail: &'static str,
}

impl FrontDoorSourceNote {
    fn value(self) -> String {
        format!("{}: {}", self.label, self.detail)
    }
}

fn front_door_source_note(
    query: &str,
    filter: FrontDoorFilter,
    sources: FrontDoorSourceStatus,
) -> Option<FrontDoorSourceNote> {
    let mesh_relevant = matches!(filter, FrontDoorFilter::Mesh)
        || (!query.trim().is_empty() && matches!(filter, FrontDoorFilter::All));
    if !mesh_relevant {
        return None;
    }
    match sources.mesh {
        FrontDoorMeshSourceStatus::Ready => None,
        FrontDoorMeshSourceStatus::Pending => Some(FrontDoorSourceNote {
            label: "Mesh discovery pending",
            detail: "Local launcher results remain available",
        }),
        FrontDoorMeshSourceStatus::Unavailable => Some(FrontDoorSourceNote {
            label: "Mesh results unavailable",
            detail: "Local launcher results remain available",
        }),
    }
}

fn source_status_accesskit_id() -> egui::Id {
    egui::Id::new("shell-front-door-source-status")
}

fn install_source_status_accessibility(
    ctx: &egui::Context,
    rect: egui::Rect,
    note: FrontDoorSourceNote,
) {
    let _ = ctx.accesskit_node_builder(source_status_accesskit_id(), |node| {
        node.set_role(egui::accesskit::Role::Status);
        node.set_live(egui::accesskit::Live::Polite);
        node.set_label(note.label);
        node.set_value(note.value());
        node.set_bounds(accesskit_rect(rect));
    });
}

fn source_status_note(ui: &mut egui::Ui, note: FrontDoorSourceNote) {
    let width = bounded_available_width(ui).max(1.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, ROW_H), egui::Sense::hover());
    let row_rect = rect.shrink2(egui::vec2(0.0, 3.0));
    ui.painter()
        .rect_filled(row_rect, 5.0, Style::WARN.linear_multiply(0.10));
    ui.painter().rect_stroke(
        row_rect,
        5.0,
        egui::Stroke::new(1.0, Style::WARN.linear_multiply(0.55)),
        egui::StrokeKind::Inside,
    );

    let icon_size = 16.0;
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(
            row_rect.left() + Style::SP_S + icon_size / 2.0,
            row_rect.center().y,
        ),
        egui::vec2(icon_size, icon_size),
    );
    if let Some(tex) = icon_texture(ui.ctx(), IconId::Node, icon_size, Style::WARN) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        ui.painter()
            .image(tex.id(), icon_rect, uv, egui::Color32::WHITE);
    }

    let text_rect = egui::Rect::from_min_max(
        egui::pos2(icon_rect.right() + Style::SP_S, row_rect.top()),
        row_rect.right_bottom() - egui::vec2(Style::SP_S, 0.0),
    );
    let painter = ui.painter().with_clip_rect(text_rect);
    painter.text(
        egui::pos2(text_rect.left(), text_rect.center().y - 7.0),
        egui::Align2::LEFT_CENTER,
        note.label,
        egui::FontId::proportional(12.0),
        Style::TEXT,
    );
    painter.text(
        egui::pos2(text_rect.left(), text_rect.center().y + 8.0),
        egui::Align2::LEFT_CENTER,
        note.detail,
        egui::FontId::proportional(11.0),
        Style::TEXT_DIM,
    );
}

fn command_empty_note(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(bounded_available_width(ui).max(1.0), ROW_H),
        egui::Sense::hover(),
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Type a command after >",
        egui::FontId::proportional(13.0),
        Style::TEXT_DIM,
    );
}

fn run_command_row(ui: &mut egui::Ui, command: &str) -> egui::Response {
    let row_width = bounded_available_width(ui).max(1.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(row_width, ROW_H), egui::Sense::click());
    ui.painter().rect_filled(
        rect.shrink2(egui::vec2(0.0, 2.0)),
        5.0,
        Style::ACCENT.linear_multiply(0.16),
    );

    let icon_rect = egui::Rect::from_center_size(
        rect.left_center() + egui::vec2(Style::SP_S + ROW_ICON / 2.0, 0.0),
        egui::vec2(ROW_ICON, ROW_ICON),
    );
    if let Some(tex) = icon_texture(ui.ctx(), IconId::Terminal, ROW_ICON, Style::TEXT) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        ui.painter()
            .image(tex.id(), icon_rect, uv, egui::Color32::WHITE);
    }

    let domain_w = result_domain_width(rect.width());
    let domain_rect = egui::Rect::from_min_size(
        egui::pos2(icon_rect.right() + Style::SP_XS, rect.center().y - 10.0),
        egui::vec2(domain_w, 20.0),
    );
    ui.painter().rect_stroke(
        domain_rect,
        4.0,
        egui::Stroke::new(1.0, Style::BORDER),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .with_clip_rect(domain_rect.shrink2(egui::vec2(3.0, 0.0)))
        .text(
            domain_rect.center(),
            egui::Align2::CENTER_CENTER,
            "Command",
            egui::FontId::proportional(11.0),
            Style::TEXT_DIM,
        );

    let text_left = domain_rect.right() + Style::SP_S;
    let text_right = rect.right() - Style::SP_S;
    if text_left < text_right {
        let title_rect = egui::Rect::from_min_max(
            egui::pos2(text_left, rect.top()),
            egui::pos2(text_right, rect.center().y),
        );
        ui.painter().with_clip_rect(title_rect).text(
            egui::pos2(title_rect.left(), rect.center().y - 8.0),
            egui::Align2::LEFT_CENTER,
            "Run command",
            egui::FontId::proportional(14.0),
            Style::TEXT,
        );
        let target_rect = egui::Rect::from_min_max(
            egui::pos2(text_left, rect.center().y),
            egui::pos2(text_right, rect.bottom()),
        );
        ui.painter().with_clip_rect(target_rect).text(
            egui::pos2(target_rect.left(), rect.center().y + 10.0),
            egui::Align2::LEFT_CENTER,
            command,
            egui::FontId::proportional(11.0),
            Style::TEXT_DIM,
        );
    }
    response
}

fn option_row(
    ui: &mut egui::Ui,
    hit: &SearchHit<FrontDoorTarget>,
    index: usize,
    total: usize,
    selected: bool,
) -> egui::Response {
    let row_width = bounded_available_width(ui).max(1.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(row_width, ROW_H), egui::Sense::click());
    let fill = if selected || response.hovered() {
        Style::ACCENT.linear_multiply(0.16)
    } else {
        Style::SURFACE_HI
    };
    let painter = ui.painter();
    painter.rect_filled(rect.shrink2(egui::vec2(0.0, 2.0)), 5.0, fill);

    let icon_tint = if selected {
        Style::TEXT
    } else {
        Style::TEXT_DIM
    };
    let icon_rect = egui::Rect::from_center_size(
        rect.left_center() + egui::vec2(Style::SP_S + ROW_ICON / 2.0, 0.0),
        egui::vec2(ROW_ICON, ROW_ICON),
    );
    if let Some(tex) = icon_texture(ui.ctx(), result_icon_id(hit), ROW_ICON, icon_tint) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(tex.id(), icon_rect, uv, egui::Color32::WHITE);
    }

    let domain_w = result_domain_width(rect.width());
    let domain_rect = egui::Rect::from_min_size(
        egui::pos2(icon_rect.right() + Style::SP_XS, rect.center().y - 10.0),
        egui::vec2(domain_w, 20.0),
    );
    painter.rect_stroke(
        domain_rect,
        4.0,
        egui::Stroke::new(1.0, Style::BORDER),
        egui::StrokeKind::Inside,
    );
    painter
        .with_clip_rect(domain_rect.shrink2(egui::vec2(3.0, 0.0)))
        .text(
            domain_rect.center(),
            egui::Align2::CENTER_CENTER,
            result_domain_label(hit),
            egui::FontId::proportional(11.0),
            Style::TEXT_DIM,
        );

    let text_left = domain_rect.right() + Style::SP_S;
    let text_right = rect.right() - Style::SP_S;
    if text_left < text_right {
        let title_rect = egui::Rect::from_min_max(
            egui::pos2(text_left, rect.top()),
            egui::pos2(text_right, rect.center().y),
        );
        painter.with_clip_rect(title_rect).text(
            egui::pos2(title_rect.left(), rect.center().y - 8.0),
            egui::Align2::LEFT_CENTER,
            &hit.item.title,
            egui::FontId::proportional(14.0),
            Style::TEXT,
        );
        let target_rect = egui::Rect::from_min_max(
            egui::pos2(text_left, rect.center().y),
            egui::pos2(text_right, rect.bottom()),
        );
        painter.with_clip_rect(target_rect).text(
            egui::pos2(target_rect.left(), rect.center().y + 10.0),
            egui::Align2::LEFT_CENTER,
            &hit.item.target,
            egui::FontId::proportional(11.0),
            Style::TEXT_DIM,
        );
    }
    install_result_accessibility(ui.ctx(), hit, rect, selected, index, total);
    response
}

fn result_action_button_width(available_width: f32) -> f32 {
    let available_width = available_width.max(0.0);
    if available_width <= 0.0 {
        return 0.0;
    }
    let preferred: f32 = 116.0;
    let min = 68.0_f32.min(available_width);
    preferred.min(available_width).max(min)
}

fn action_button_label_visible(button_width: f32) -> bool {
    button_width >= ACTION_BUTTON_TEXT_MIN_W
}

fn primary_action_label(hit: &SearchHit<FrontDoorTarget>) -> &'static str {
    match &hit.item.payload {
        FrontDoorTarget::App(_) => "Launch",
        FrontDoorTarget::PeerApp(_) => "Launch",
        FrontDoorTarget::Workflow(_) | FrontDoorTarget::ServiceLifecycle(_) => "Open",
        FrontDoorTarget::File(_)
        | FrontDoorTarget::Browser(_)
        | FrontDoorTarget::Mesh(_)
        | FrontDoorTarget::ConsoleCommand(_) => "Open",
        FrontDoorTarget::RunCommand(_) => "Run",
    }
}

fn activation_request_for_hit(hit: &SearchHit<FrontDoorTarget>) -> FrontDoorRequest {
    match &hit.item.payload {
        FrontDoorTarget::PeerApp(target) => FrontDoorRequest::LaunchPeerApp(target.clone()),
        _ => FrontDoorRequest::Activate(hit.item.payload.clone()),
    }
}

fn front_door_clicked_elsewhere_should_close(
    state: &mut FrontDoorState,
    clicked_elsewhere: bool,
) -> bool {
    if !clicked_elsewhere {
        state.suppress_click_away_once = false;
        return false;
    }
    if state.suppress_click_away_once {
        state.suppress_click_away_once = false;
        return false;
    }
    true
}

fn peer_node_for_unit_id(id: &str) -> Option<&str> {
    id.strip_prefix("peer:").filter(|node| !node.is_empty())
}

fn desktop_source_id_for_hit(hit: &SearchHit<FrontDoorTarget>) -> Option<String> {
    match &hit.item.payload {
        FrontDoorTarget::Mesh(id) if id.starts_with("peer:") || id.starts_with("peer-vm:") => {
            Some(id.clone())
        }
        FrontDoorTarget::PeerApp(target) => Some(target.desktop_source_id()),
        _ => None,
    }
}

fn cloud_instance_id(unit_id: &str) -> Option<&str> {
    unit_id
        .strip_prefix("cloud:instance:")
        .filter(|instance| !instance.is_empty())
}

fn instance_lifecycle_target_for_hit(
    hit: &SearchHit<FrontDoorTarget>,
) -> Option<FrontDoorInstanceLifecycleTarget> {
    let FrontDoorTarget::Mesh(unit_id) = &hit.item.payload else {
        return None;
    };
    Some(FrontDoorInstanceLifecycleTarget {
        unit_id: unit_id.clone(),
        instance: cloud_instance_id(unit_id)?.to_owned(),
    })
}

fn service_lifecycle_target_for_hit(
    hit: &SearchHit<FrontDoorTarget>,
) -> Option<&FrontDoorServiceLifecycleTarget> {
    match &hit.item.payload {
        FrontDoorTarget::ServiceLifecycle(target) => Some(target),
        _ => None,
    }
}

fn service_lifecycle_ops_for_target(
    target: &FrontDoorServiceLifecycleTarget,
) -> &'static [FrontDoorServiceLifecycleOp] {
    if target.state.trim().eq_ignore_ascii_case("running") {
        &[
            FrontDoorServiceLifecycleOp::Restart,
            FrontDoorServiceLifecycleOp::Stop,
        ]
    } else {
        &[FrontDoorServiceLifecycleOp::Start]
    }
}

pub(crate) fn cloud_instance_lifecycle_wire(
    unit_id: &str,
    op: FrontDoorInstanceLifecycleOp,
) -> Option<(String, String)> {
    let instance = cloud_instance_id(unit_id)?;
    let topic = format!("action/cloud/{}", op.cloud_verb());
    let body = serde_json::json!({ "instance": instance }).to_string();
    Some((topic, body))
}

pub(crate) fn service_lifecycle_wire(
    target: &FrontDoorServiceLifecycleTarget,
    op: FrontDoorServiceLifecycleOp,
) -> (String, String) {
    let body = serde_json::json!({
        "peer": target.host.as_str(),
        "kind": target.kind.label(),
        "name": target.name.as_str(),
        "op": op.wire(),
    })
    .to_string();
    ("action/services/lifecycle".to_owned(), body)
}

pub(crate) fn peer_app_launch_wire(target: &FrontDoorPeerAppTarget) -> Option<(String, String)> {
    let node = target.node.trim();
    let app_id = target.app_id.trim();
    if node.is_empty() || app_id.is_empty() {
        return None;
    }
    let body = serde_json::json!({
        "node": node,
        "app_id": app_id,
        "name": target.name.trim(),
    })
    .to_string();
    Some(("action/apps/launch".to_owned(), body))
}

fn primary_action_accesskit_id(hit: &SearchHit<FrontDoorTarget>) -> egui::Id {
    egui::Id::new((
        "shell-front-door-primary-action",
        result_domain_label(hit),
        hit.item.title.as_str(),
        hit.item.target.as_str(),
    ))
}

fn install_primary_action_accessibility(
    ctx: &egui::Context,
    hit: &SearchHit<FrontDoorTarget>,
    rect: egui::Rect,
) {
    let _ = ctx.accesskit_node_builder(primary_action_accesskit_id(hit), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(format!("{} {}", primary_action_label(hit), hit.item.title));
        node.set_value(format!(
            "Primary action: {}, {}",
            result_domain_label(hit),
            hit.item.target
        ));
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
}

fn connect_desktop_action_accesskit_id(hit: &SearchHit<FrontDoorTarget>) -> egui::Id {
    egui::Id::new((
        "shell-front-door-connect-desktop-action",
        hit.item.title.as_str(),
        hit.item.target.as_str(),
    ))
}

fn install_connect_desktop_action_accessibility(
    ctx: &egui::Context,
    hit: &SearchHit<FrontDoorTarget>,
    rect: egui::Rect,
    source_id: &str,
) {
    let _ = ctx.accesskit_node_builder(connect_desktop_action_accesskit_id(hit), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(format!("Connect desktop for {}", hit.item.title));
        node.set_value(format!(
            "Desktop source: {source_id}; uses Desktop chooser path"
        ));
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
}

fn instance_lifecycle_action_accesskit_id(
    hit: &SearchHit<FrontDoorTarget>,
    op: FrontDoorInstanceLifecycleOp,
) -> egui::Id {
    egui::Id::new((
        "shell-front-door-instance-lifecycle-action",
        hit.item.title.as_str(),
        hit.item.target.as_str(),
        op,
    ))
}

fn lifecycle_action_armed_for(
    state: &FrontDoorState,
    target: &FrontDoorInstanceLifecycleTarget,
    op: FrontDoorInstanceLifecycleOp,
) -> bool {
    state
        .lifecycle_arm
        .as_ref()
        .is_some_and(|arm| arm.unit_id == target.unit_id && arm.op == op)
}

fn lifecycle_action_label(
    state: &FrontDoorState,
    target: &FrontDoorInstanceLifecycleTarget,
    op: FrontDoorInstanceLifecycleOp,
) -> &'static str {
    if op.destructive() && lifecycle_action_armed_for(state, target, op) {
        "Confirm"
    } else {
        op.label()
    }
}

fn install_instance_lifecycle_action_accessibility(
    ctx: &egui::Context,
    hit: &SearchHit<FrontDoorTarget>,
    target: &FrontDoorInstanceLifecycleTarget,
    op: FrontDoorInstanceLifecycleOp,
    rect: egui::Rect,
    armed: bool,
) {
    let _ = ctx.accesskit_node_builder(instance_lifecycle_action_accesskit_id(hit, op), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(if armed {
            format!("Confirm {} for {}", op.label(), hit.item.title)
        } else {
            format!("{} instance {}", op.label(), hit.item.title)
        });
        let (topic, _) = cloud_instance_lifecycle_wire(&target.unit_id, op).unwrap_or_default();
        node.set_value(format!(
            "Cloud lifecycle: {topic}; instance {}",
            target.instance
        ));
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
        if armed {
            node.set_selected(true);
        }
    });
}

fn service_lifecycle_action_accesskit_id(
    hit: &SearchHit<FrontDoorTarget>,
    op: FrontDoorServiceLifecycleOp,
) -> egui::Id {
    egui::Id::new((
        "shell-front-door-service-lifecycle-action",
        hit.item.title.as_str(),
        hit.item.target.as_str(),
        op,
    ))
}

fn service_lifecycle_action_armed_for(
    state: &FrontDoorState,
    target: &FrontDoorServiceLifecycleTarget,
    op: FrontDoorServiceLifecycleOp,
) -> bool {
    state
        .service_lifecycle_arm
        .as_ref()
        .is_some_and(|arm| arm.target == *target && arm.op == op)
}

fn service_lifecycle_action_label(
    state: &FrontDoorState,
    target: &FrontDoorServiceLifecycleTarget,
    op: FrontDoorServiceLifecycleOp,
) -> &'static str {
    if op.destructive() && service_lifecycle_action_armed_for(state, target, op) {
        "Confirm"
    } else {
        op.label()
    }
}

fn install_service_lifecycle_action_accessibility(
    ctx: &egui::Context,
    hit: &SearchHit<FrontDoorTarget>,
    target: &FrontDoorServiceLifecycleTarget,
    op: FrontDoorServiceLifecycleOp,
    rect: egui::Rect,
    armed: bool,
) {
    let _ = ctx.accesskit_node_builder(service_lifecycle_action_accesskit_id(hit, op), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(if armed {
            format!("Confirm {} for {}", op.label(), hit.item.title)
        } else {
            format!("{} service {}", op.label(), hit.item.title)
        });
        let (topic, body) = service_lifecycle_wire(target, op);
        node.set_value(format!("Service lifecycle: {topic}; {body}"));
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
        if armed {
            node.set_selected(true);
        }
    });
}

fn lifecycle_action_clicked(
    state: &mut FrontDoorState,
    target: &FrontDoorInstanceLifecycleTarget,
    op: FrontDoorInstanceLifecycleOp,
) -> Option<FrontDoorRequest> {
    if op.destructive() && !lifecycle_action_armed_for(state, target, op) {
        state.lifecycle_arm = Some(FrontDoorLifecycleArm {
            unit_id: target.unit_id.clone(),
            op,
        });
        state.service_lifecycle_arm = None;
        return None;
    }
    state.lifecycle_arm = None;
    state.service_lifecycle_arm = None;
    Some(FrontDoorRequest::InstanceLifecycle {
        unit_id: target.unit_id.clone(),
        op,
    })
}

fn service_lifecycle_action_clicked(
    state: &mut FrontDoorState,
    target: &FrontDoorServiceLifecycleTarget,
    op: FrontDoorServiceLifecycleOp,
) -> Option<FrontDoorRequest> {
    if op.destructive() && !service_lifecycle_action_armed_for(state, target, op) {
        state.service_lifecycle_arm = Some(FrontDoorServiceLifecycleArm {
            target: target.clone(),
            op,
        });
        state.lifecycle_arm = None;
        return None;
    }
    state.service_lifecycle_arm = None;
    state.lifecycle_arm = None;
    Some(FrontDoorRequest::ServiceLifecycle {
        target: target.clone(),
        op,
    })
}

/// The related-Workbench-plane quick action for a workflow card, or `None` when
/// the card's home is a standalone surface rather than a Workbench plane.
///
/// WL-ARCH-006 — the cloud workflow cards (Cloud workloads / Cloud API services)
/// open the standalone **Workloads** surface (`Surface::InfraCode`); they have no
/// Workbench plane to deep-link to, so they export no plane quick action (§7 —
/// honest omission, never a dead affordance). Only the Desktop-session and mesh-
/// service cards relate to a live Workbench plane (Fleet).
fn workflow_quick_action_for_card(
    card: FrontDoorWorkflowCard,
) -> Option<FrontDoorWorkflowQuickAction> {
    match (card.kind, card.surface, card.workbench_plane) {
        (FrontDoorWorkflowKind::Workload, Surface::Desktop, _) => {
            Some(FrontDoorWorkflowQuickAction {
                label: "Fleet",
                plane: Plane::Fleet,
                icon: IconId::Desktop,
            })
        }
        (FrontDoorWorkflowKind::Service, Surface::Workbench, Some(Plane::Provisioning)) => {
            Some(FrontDoorWorkflowQuickAction {
                label: "Fleet",
                plane: Plane::Fleet,
                icon: IconId::Workbench,
            })
        }
        _ => None,
    }
}

fn workflow_quick_action_for_hit(
    hit: &SearchHit<FrontDoorTarget>,
) -> Option<FrontDoorWorkflowQuickAction> {
    match hit.item.payload {
        FrontDoorTarget::Workflow(card) => workflow_quick_action_for_card(card),
        _ => None,
    }
}

fn workflow_quick_action_accesskit_id(
    hit: &SearchHit<FrontDoorTarget>,
    action: FrontDoorWorkflowQuickAction,
) -> egui::Id {
    egui::Id::new((
        "shell-front-door-workflow-quick-action",
        hit.item.title.as_str(),
        action.label,
        action.plane.label(),
    ))
}

fn workflow_quick_action_accesskit_label(
    hit: &SearchHit<FrontDoorTarget>,
    action: FrontDoorWorkflowQuickAction,
) -> String {
    format!("Open {} plane for {}", action.label, hit.item.title)
}

fn workflow_quick_action_accesskit_value(
    hit: &SearchHit<FrontDoorTarget>,
    action: FrontDoorWorkflowQuickAction,
) -> String {
    format!(
        "Workflow action: Workbench {} plane; {}",
        action.plane.label(),
        hit.item.target
    )
}

fn install_workflow_quick_action_accessibility(
    ctx: &egui::Context,
    hit: &SearchHit<FrontDoorTarget>,
    action: FrontDoorWorkflowQuickAction,
    rect: egui::Rect,
) {
    let _ = ctx.accesskit_node_builder(workflow_quick_action_accesskit_id(hit, action), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(workflow_quick_action_accesskit_label(hit, action));
        node.set_value(workflow_quick_action_accesskit_value(hit, action));
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
}

fn pin_surface_for_hit(hit: &SearchHit<FrontDoorTarget>) -> Option<Surface> {
    match hit.item.payload {
        FrontDoorTarget::App(surface) => Some(surface),
        _ => None,
    }
}

fn pin_action_label(surface: Surface, pinned: bool) -> String {
    if pinned {
        format!("Unpin {}", surface.label())
    } else {
        format!("Pin {}", surface.label())
    }
}

fn pin_action_value(surface: Surface, pinned: bool) -> String {
    let state = if pinned { "Pinned" } else { "Not pinned" };
    format!("Favorite action: {}, {state}", surface.label())
}

fn pin_action_accesskit_id(surface: Surface) -> egui::Id {
    egui::Id::new(("shell-front-door-pin-action", surface))
}

fn move_pin_action_accesskit_id(
    surface: Surface,
    direction: FrontDoorPinMoveDirection,
) -> egui::Id {
    egui::Id::new(("shell-front-door-move-pin-action", surface, direction))
}

fn move_pin_action_label(surface: Surface, direction: FrontDoorPinMoveDirection) -> String {
    let direction = match direction {
        FrontDoorPinMoveDirection::Up => "up",
        FrontDoorPinMoveDirection::Down => "down",
    };
    format!("Move {} {direction}", surface.label())
}

fn move_pin_action_value(surface: Surface, index: usize, total: usize) -> String {
    format!(
        "Favorite order: {} of {total}, {}",
        index + 1,
        surface.label()
    )
}

fn install_pin_action_accessibility(
    ctx: &egui::Context,
    surface: Surface,
    rect: egui::Rect,
    pinned: bool,
) {
    let _ = ctx.accesskit_node_builder(pin_action_accesskit_id(surface), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(pin_action_label(surface, pinned));
        node.set_value(pin_action_value(surface, pinned));
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
        if pinned {
            node.set_selected(true);
        }
    });
}

fn install_move_pin_action_accessibility(
    ctx: &egui::Context,
    surface: Surface,
    direction: FrontDoorPinMoveDirection,
    rect: egui::Rect,
    index: usize,
    total: usize,
) {
    let _ = ctx.accesskit_node_builder(move_pin_action_accesskit_id(surface, direction), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(move_pin_action_label(surface, direction));
        node.set_value(move_pin_action_value(surface, index, total));
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
}

fn action_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    label: &str,
    icon: IconId,
    selected: bool,
) -> egui::Response {
    let response = ui.interact(rect, id, egui::Sense::click());
    let show_label = action_button_label_visible(rect.width());
    let button_fill = if response.hovered() {
        Style::ACCENT.linear_multiply(0.28)
    } else if selected {
        Style::ACCENT.linear_multiply(0.22)
    } else {
        Style::ACCENT.linear_multiply(0.18)
    };
    ui.painter().rect_filled(rect, 5.0, button_fill);
    ui.painter().rect_stroke(
        rect,
        5.0,
        egui::Stroke::new(1.0, Style::ACCENT),
        egui::StrokeKind::Inside,
    );

    let icon_size = 13.0;
    let icon_center = if show_label {
        egui::pos2(rect.left() + 10.0 + icon_size / 2.0, rect.center().y)
    } else {
        rect.center()
    };
    let icon_rect = egui::Rect::from_center_size(icon_center, egui::vec2(icon_size, icon_size));
    if let Some(tex) = icon_texture(ui.ctx(), icon, icon_size, Style::TEXT) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        ui.painter()
            .image(tex.id(), icon_rect, uv, egui::Color32::WHITE);
    }

    if show_label {
        let text_rect = egui::Rect::from_min_max(
            egui::pos2(icon_rect.right() + 4.0, rect.top()),
            rect.right_bottom(),
        );
        ui.painter()
            .with_clip_rect(text_rect.shrink2(egui::vec2(4.0, 0.0)))
            .text(
                text_rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(12.0),
                Style::TEXT,
            );
        response
    } else {
        front_door_hover_text(response, label.to_owned())
    }
}

fn icon_action_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    icon: IconId,
) -> egui::Response {
    let response = ui.interact(rect, id, egui::Sense::click());
    let button_fill = if response.hovered() {
        Style::ACCENT.linear_multiply(0.28)
    } else {
        Style::ACCENT.linear_multiply(0.18)
    };
    ui.painter().rect_filled(rect, 5.0, button_fill);
    ui.painter().rect_stroke(
        rect,
        5.0,
        egui::Stroke::new(1.0, Style::ACCENT),
        egui::StrokeKind::Inside,
    );
    let icon_size = 14.0;
    let icon_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(icon_size, icon_size));
    if let Some(tex) = icon_texture(ui.ctx(), icon, icon_size, Style::TEXT) {
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        ui.painter()
            .image(tex.id(), icon_rect, uv, egui::Color32::WHITE);
    }
    response
}

fn result_action_panel(
    ui: &mut egui::Ui,
    state: &mut FrontDoorState,
    hit: &SearchHit<FrontDoorTarget>,
    pinned: &[Surface],
) -> Option<FrontDoorRequest> {
    let width = bounded_available_width(ui).max(1.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, ACTION_PANEL_H), egui::Sense::hover());
    let panel_rect = rect.shrink2(egui::vec2(0.0, 4.0));
    ui.painter()
        .rect_filled(panel_rect, 5.0, Style::ACCENT.linear_multiply(0.08));
    ui.painter().rect_stroke(
        panel_rect,
        5.0,
        egui::Stroke::new(1.0, Style::ACCENT.linear_multiply(0.42)),
        egui::StrokeKind::Inside,
    );

    let pin_surface = pin_surface_for_hit(hit);
    let pin_index = pin_surface.and_then(|surface| {
        pinned
            .iter()
            .position(|&pinned_surface| pinned_surface == surface)
    });
    let pin_visible = pin_surface.is_some() && panel_rect.width() >= 176.0;
    let primary_budget = if pin_visible {
        panel_rect.width() * 0.32
    } else {
        panel_rect.width() * 0.36
    };
    let button_w = result_action_button_width(primary_budget);
    let primary_rect = egui::Rect::from_min_size(
        egui::pos2(
            panel_rect.right() - Style::SP_S - button_w,
            panel_rect.center().y - 13.0,
        ),
        egui::vec2(button_w, 26.0),
    );
    let primary_response = action_button(
        ui,
        primary_rect,
        primary_action_accesskit_id(hit),
        primary_action_label(hit),
        result_icon_id(hit),
        false,
    );

    let mut pin_response = None;
    let mut move_up_response = None;
    let mut move_down_response = None;
    let mut connect_response = None;
    let mut lifecycle_responses = Vec::new();
    let mut service_lifecycle_responses = Vec::new();
    let mut workflow_quick_response = None;
    let mut first_button_left = primary_rect.left();
    if let Some(service_target) = service_lifecycle_target_for_hit(hit) {
        for op in service_lifecycle_ops_for_target(service_target) {
            let available = (first_button_left - panel_rect.left() - Style::SP_S * 2.0).max(0.0);
            let lifecycle_w = 82.0_f32.min(available);
            if lifecycle_w < 60.0 {
                continue;
            }
            let action_rect = egui::Rect::from_min_size(
                egui::pos2(
                    first_button_left - Style::SP_XS - lifecycle_w,
                    panel_rect.center().y - 13.0,
                ),
                egui::vec2(lifecycle_w, 26.0),
            );
            let armed = service_lifecycle_action_armed_for(state, service_target, *op);
            let response = action_button(
                ui,
                action_rect,
                service_lifecycle_action_accesskit_id(hit, *op),
                service_lifecycle_action_label(state, service_target, *op),
                op.icon(),
                armed,
            );
            install_service_lifecycle_action_accessibility(
                ui.ctx(),
                hit,
                service_target,
                *op,
                action_rect,
                armed,
            );
            service_lifecycle_responses.push((service_target.clone(), *op, response));
            first_button_left = action_rect.left();
        }
    }
    if let Some(lifecycle_target) = instance_lifecycle_target_for_hit(hit) {
        for op in [
            FrontDoorInstanceLifecycleOp::Reboot,
            FrontDoorInstanceLifecycleOp::Stop,
            FrontDoorInstanceLifecycleOp::Start,
        ] {
            let available = (first_button_left - panel_rect.left() - Style::SP_S * 2.0).max(0.0);
            let lifecycle_w = 78.0_f32.min(available);
            if lifecycle_w < 58.0 {
                continue;
            }
            let action_rect = egui::Rect::from_min_size(
                egui::pos2(
                    first_button_left - Style::SP_XS - lifecycle_w,
                    panel_rect.center().y - 13.0,
                ),
                egui::vec2(lifecycle_w, 26.0),
            );
            let armed = lifecycle_action_armed_for(state, &lifecycle_target, op);
            let response = action_button(
                ui,
                action_rect,
                instance_lifecycle_action_accesskit_id(hit, op),
                lifecycle_action_label(state, &lifecycle_target, op),
                op.icon(),
                armed,
            );
            install_instance_lifecycle_action_accessibility(
                ui.ctx(),
                hit,
                &lifecycle_target,
                op,
                action_rect,
                armed,
            );
            lifecycle_responses.push((lifecycle_target.clone(), op, response));
            first_button_left = action_rect.left();
        }
    }
    if let Some(source_id) = desktop_source_id_for_hit(hit) {
        let available = (first_button_left - panel_rect.left() - Style::SP_S * 2.0).max(0.0);
        let connect_w = 96.0_f32.min(available);
        if connect_w >= 68.0 {
            let connect_rect = egui::Rect::from_min_size(
                egui::pos2(
                    first_button_left - Style::SP_XS - connect_w,
                    panel_rect.center().y - 13.0,
                ),
                egui::vec2(connect_w, 26.0),
            );
            let response = action_button(
                ui,
                connect_rect,
                connect_desktop_action_accesskit_id(hit),
                "Connect",
                IconId::Desktop,
                false,
            );
            install_connect_desktop_action_accessibility(ui.ctx(), hit, connect_rect, &source_id);
            connect_response = Some((source_id.to_owned(), response));
            first_button_left = connect_rect.left();
        }
    }
    if let Some(workflow_action) = workflow_quick_action_for_hit(hit) {
        let available = (first_button_left - panel_rect.left() - Style::SP_S * 2.0).max(0.0);
        let quick_w = 82.0_f32.min(available);
        if quick_w >= 62.0 {
            let quick_rect = egui::Rect::from_min_size(
                egui::pos2(
                    first_button_left - Style::SP_XS - quick_w,
                    panel_rect.center().y - 13.0,
                ),
                egui::vec2(quick_w, 26.0),
            );
            let response = action_button(
                ui,
                quick_rect,
                workflow_quick_action_accesskit_id(hit, workflow_action),
                workflow_action.label,
                workflow_action.icon,
                false,
            );
            install_workflow_quick_action_accessibility(ui.ctx(), hit, workflow_action, quick_rect);
            workflow_quick_response = Some((workflow_action, response));
            first_button_left = quick_rect.left();
        }
    }
    if let Some(surface) = pin_surface {
        let is_pinned = pinned.contains(&surface);
        if pin_visible {
            let pin_w =
                74.0_f32.min((first_button_left - panel_rect.left() - Style::SP_S * 2.0).max(0.0));
            if pin_w > 0.0 {
                let pin_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        first_button_left - Style::SP_XS - pin_w,
                        panel_rect.center().y - 13.0,
                    ),
                    egui::vec2(pin_w, 26.0),
                );
                let response = action_button(
                    ui,
                    pin_rect,
                    pin_action_accesskit_id(surface),
                    if is_pinned { "Unpin" } else { "Pin" },
                    IconId::Pin,
                    is_pinned,
                );
                install_pin_action_accessibility(ui.ctx(), surface, pin_rect, is_pinned);
                pin_response = Some((surface, response));
                let mut left = pin_rect.left();
                if let Some(index) = pin_index {
                    let total = pinned.len();
                    if index > 0 && left - Style::SP_XS - 28.0 >= panel_rect.left() {
                        let move_rect = egui::Rect::from_min_size(
                            egui::pos2(left - Style::SP_XS - 28.0, panel_rect.center().y - 13.0),
                            egui::vec2(28.0, 26.0),
                        );
                        let direction = FrontDoorPinMoveDirection::Up;
                        let response = icon_action_button(
                            ui,
                            move_rect,
                            move_pin_action_accesskit_id(surface, direction),
                            IconId::ChevronUp,
                        );
                        install_move_pin_action_accessibility(
                            ui.ctx(),
                            surface,
                            direction,
                            move_rect,
                            index,
                            total,
                        );
                        move_up_response = Some((surface, direction, response));
                        left = move_rect.left();
                    }
                    if index + 1 < total && left - Style::SP_XS - 28.0 >= panel_rect.left() {
                        let move_rect = egui::Rect::from_min_size(
                            egui::pos2(left - Style::SP_XS - 28.0, panel_rect.center().y - 13.0),
                            egui::vec2(28.0, 26.0),
                        );
                        let direction = FrontDoorPinMoveDirection::Down;
                        let response = icon_action_button(
                            ui,
                            move_rect,
                            move_pin_action_accesskit_id(surface, direction),
                            IconId::ArrowDown,
                        );
                        install_move_pin_action_accessibility(
                            ui.ctx(),
                            surface,
                            direction,
                            move_rect,
                            index,
                            total,
                        );
                        move_down_response = Some((surface, direction, response));
                        left = move_rect.left();
                    }
                }
                left
            } else {
                first_button_left
            }
        } else {
            first_button_left
        }
    } else {
        first_button_left
    };

    let text_left = panel_rect.left() + Style::SP_S;
    let text_right = first_button_left - Style::SP_S;
    if text_left < text_right {
        let text_rect = egui::Rect::from_min_max(
            egui::pos2(text_left, panel_rect.top()),
            egui::pos2(text_right, panel_rect.bottom()),
        );
        let instance_lifecycle_note = instance_lifecycle_target_for_hit(hit).and_then(|target| {
            state
                .lifecycle_arm
                .as_ref()
                .filter(|arm| arm.unit_id == target.unit_id)
                .map(|arm| {
                    format!(
                        "Armed {} for {}; click Confirm.",
                        arm.op.label(),
                        target.instance
                    )
                })
        });
        let service_lifecycle_note = service_lifecycle_target_for_hit(hit).and_then(|target| {
            state
                .service_lifecycle_arm
                .as_ref()
                .filter(|arm| arm.target == *target)
                .map(|arm| {
                    format!(
                        "Armed {} for {} on {}; click Confirm.",
                        arm.op.label(),
                        target.name,
                        target.host
                    )
                })
        });
        let lifecycle_note = service_lifecycle_note.or(instance_lifecycle_note);
        let (text, color) = lifecycle_note
            .as_deref()
            .map_or((hit.item.target.as_str(), Style::TEXT_DIM), |note| {
                (note, Style::WARN)
            });
        ui.painter().with_clip_rect(text_rect).text(
            egui::pos2(text_rect.left(), text_rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(12.0),
            color,
        );
    }

    install_primary_action_accessibility(ui.ctx(), hit, primary_rect);
    for (target, op, response) in service_lifecycle_responses {
        if response.clicked() {
            if let Some(request) = service_lifecycle_action_clicked(state, &target, op) {
                return Some(request);
            }
        }
    }
    for (target, op, response) in lifecycle_responses {
        if response.clicked() {
            if let Some(request) = lifecycle_action_clicked(state, &target, op) {
                return Some(request);
            }
        }
    }
    if let Some((surface, response)) = pin_response {
        if response.clicked() {
            return Some(FrontDoorRequest::TogglePin(surface));
        }
    }
    if let Some((surface, direction, response)) = move_up_response {
        if response.clicked() {
            return Some(FrontDoorRequest::MovePin { surface, direction });
        }
    }
    if let Some((surface, direction, response)) = move_down_response {
        if response.clicked() {
            return Some(FrontDoorRequest::MovePin { surface, direction });
        }
    }
    if let Some((source_id, response)) = connect_response {
        if response.clicked() {
            return Some(FrontDoorRequest::ConnectDesktopSource(source_id));
        }
    }
    if let Some((workflow_action, response)) = workflow_quick_response {
        if response.clicked() {
            return Some(FrontDoorRequest::OpenWorkbenchPlane(workflow_action.plane));
        }
    }
    primary_response
        .clicked()
        .then(|| activation_request_for_hit(hit))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ResultContextItem {
    Open,
    WorkflowPlane,
    ConnectDesktop,
    Pin,
    MoveUp,
    MoveDown,
}

fn result_context_item_id(hit: &SearchHit<FrontDoorTarget>, item: ResultContextItem) -> egui::Id {
    egui::Id::new((
        "shell-front-door-result-context",
        result_domain_label(hit),
        hit.item.title.as_str(),
        item,
    ))
}

fn context_menu_row(ui: &mut egui::Ui, id: egui::Id, label: &str) -> bool {
    let width = ui.available_width().max(Style::SP_XL * 4.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, Style::SP_L), egui::Sense::hover());
    let response = ui.interact(rect, id, egui::Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 5.0, Style::SURFACE_HI);
    }
    ui.painter().text(
        egui::pos2(rect.left() + Style::SP_S, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT,
    );
    mde_egui::focus::paint_focus_ring(ui.painter(), rect, response.has_focus());
    install_context_menu_row_accessibility(ui.ctx(), id, rect, label);
    crate::dock::response_activated(ui, &response)
}

fn install_context_menu_row_accessibility(
    ctx: &egui::Context,
    id: egui::Id,
    rect: egui::Rect,
    label: &str,
) {
    let _ = ctx.accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(label);
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
}

fn result_context_menu_request(
    response: &egui::Response,
    hit: &SearchHit<FrontDoorTarget>,
    pinned: &[Surface],
) -> Option<FrontDoorRequest> {
    let pin_surface = pin_surface_for_hit(hit);
    let mut request = None;
    let _ = front_door_context_menu(response, |ui| {
        if context_menu_row(
            ui,
            result_context_item_id(hit, ResultContextItem::Open),
            primary_action_label(hit),
        ) {
            request = Some(activation_request_for_hit(hit));
            ui.close_menu();
        }
        if let Some(workflow_action) = workflow_quick_action_for_hit(hit) {
            let label = format!("Open {} plane", workflow_action.label);
            if context_menu_row(
                ui,
                result_context_item_id(hit, ResultContextItem::WorkflowPlane),
                &label,
            ) {
                request = Some(FrontDoorRequest::OpenWorkbenchPlane(workflow_action.plane));
                ui.close_menu();
            }
        }
        if let Some(source_id) = desktop_source_id_for_hit(hit) {
            if context_menu_row(
                ui,
                result_context_item_id(hit, ResultContextItem::ConnectDesktop),
                "Connect desktop",
            ) {
                request = Some(FrontDoorRequest::ConnectDesktopSource(source_id.to_owned()));
                ui.close_menu();
            }
        }
        if let Some(surface) = pin_surface {
            let label = if pinned.contains(&surface) {
                "Unpin from top"
            } else {
                "Pin to top"
            };
            if context_menu_row(
                ui,
                result_context_item_id(hit, ResultContextItem::Pin),
                label,
            ) {
                request = Some(FrontDoorRequest::TogglePin(surface));
                ui.close_menu();
            }
            if let Some(index) = pinned.iter().position(|&candidate| candidate == surface) {
                if index > 0
                    && context_menu_row(
                        ui,
                        result_context_item_id(hit, ResultContextItem::MoveUp),
                        "Move up",
                    )
                {
                    request = Some(FrontDoorRequest::MovePin {
                        surface,
                        direction: FrontDoorPinMoveDirection::Up,
                    });
                    ui.close_menu();
                }
                if index + 1 < pinned.len()
                    && context_menu_row(
                        ui,
                        result_context_item_id(hit, ResultContextItem::MoveDown),
                        "Move down",
                    )
                {
                    request = Some(FrontDoorRequest::MovePin {
                        surface,
                        direction: FrontDoorPinMoveDirection::Down,
                    });
                    ui.close_menu();
                }
            }
        }
    });
    request
}

fn front_door_context_menu(
    response: &egui::Response,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> Option<egui::InnerResponse<()>> {
    let previous_style = response.ctx.style();
    let mut menu_style = (*previous_style).clone();
    apply_front_door_context_style(&mut menu_style);
    response.ctx.set_style(menu_style);
    let inner = response.context_menu(|ui| {
        apply_front_door_context_style(ui.style_mut());
        add_contents(ui);
    });
    response.ctx.set_style(previous_style);
    inner
}

fn apply_front_door_context_style(style: &mut egui::Style) {
    style.spacing.item_spacing = egui::vec2(Style::SP_XS, Style::SP_XS);
    let visuals = &mut style.visuals;
    visuals.override_text_color = Some(Style::TEXT);
    visuals.window_fill = Style::SURFACE;
    visuals.panel_fill = Style::SURFACE;
    visuals.extreme_bg_color = Style::SURFACE;
    visuals.faint_bg_color = Style::SURFACE_HI;
    visuals.widgets.noninteractive.bg_fill = Style::SURFACE;
    visuals.widgets.noninteractive.fg_stroke.color = Style::TEXT_DIM;
    visuals.widgets.noninteractive.bg_stroke.color = Style::BORDER;
    visuals.widgets.inactive.bg_fill = Style::SURFACE;
    visuals.widgets.inactive.fg_stroke.color = Style::TEXT;
    visuals.widgets.inactive.bg_stroke.color = Style::BORDER;
    visuals.widgets.hovered.bg_fill = Style::SURFACE_HI;
    visuals.widgets.hovered.fg_stroke.color = Style::TEXT;
    visuals.widgets.active.bg_fill = Style::SURFACE_HI;
    visuals.widgets.active.fg_stroke.color = Style::TEXT;
    visuals.widgets.open.bg_fill = Style::SURFACE_HI;
    visuals.widgets.open.weak_bg_fill = Style::SURFACE_HI;
    visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0, Style::TEXT);
    visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0, Style::BORDER);
}

fn run_command_accesskit_id() -> egui::Id {
    egui::Id::new("shell-front-door-run-command")
}

fn run_command_primary_action_id(command: &str) -> egui::Id {
    egui::Id::new(("shell-front-door-run-command-primary-action", command))
}

fn install_run_command_accessibility(ctx: &egui::Context, rect: egui::Rect, command: &str) {
    let _ = ctx.accesskit_node_builder(run_command_accesskit_id(), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label("Run command");
        node.set_value(format!("Command: {command}"));
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
        node.set_selected(true);
    });
}

fn install_run_command_prompt_accessibility(ctx: &egui::Context, rect: egui::Rect) {
    let _ = ctx.accesskit_node_builder(results_announcement_id(), |node| {
        node.set_role(egui::accesskit::Role::Status);
        node.set_live(egui::accesskit::Live::Polite);
        node.set_label("Shell search results");
        node.set_value("Type a command after >");
        node.set_bounds(accesskit_rect(rect));
    });
}

fn install_run_command_primary_action_accessibility(
    ctx: &egui::Context,
    rect: egui::Rect,
    command: &str,
) {
    let _ = ctx.accesskit_node_builder(run_command_primary_action_id(command), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label("Run command in Terminal");
        node.set_value(format!("Command: {command}"));
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
}

fn run_command_action_panel(ui: &mut egui::Ui, command: &str) -> egui::Response {
    let width = bounded_available_width(ui).max(1.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, ACTION_PANEL_H), egui::Sense::hover());
    let panel_rect = rect.shrink2(egui::vec2(0.0, 4.0));
    ui.painter()
        .rect_filled(panel_rect, 5.0, Style::ACCENT.linear_multiply(0.08));
    ui.painter().rect_stroke(
        panel_rect,
        5.0,
        egui::Stroke::new(1.0, Style::ACCENT.linear_multiply(0.42)),
        egui::StrokeKind::Inside,
    );

    let button_w = result_action_button_width(panel_rect.width() * 0.36);
    let button_rect = egui::Rect::from_min_size(
        egui::pos2(
            panel_rect.right() - Style::SP_S - button_w,
            panel_rect.center().y - 13.0,
        ),
        egui::vec2(button_w, 26.0),
    );
    let response = ui.interact(
        button_rect,
        run_command_primary_action_id(command),
        egui::Sense::click(),
    );
    let button_fill = if response.hovered() {
        Style::ACCENT.linear_multiply(0.28)
    } else {
        Style::ACCENT.linear_multiply(0.18)
    };
    ui.painter().rect_filled(button_rect, 5.0, button_fill);
    ui.painter().rect_stroke(
        button_rect,
        5.0,
        egui::Stroke::new(1.0, Style::ACCENT),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .with_clip_rect(button_rect.shrink2(egui::vec2(4.0, 0.0)))
        .text(
            button_rect.center(),
            egui::Align2::CENTER_CENTER,
            "Run",
            egui::FontId::proportional(12.0),
            Style::TEXT,
        );

    let text_left = panel_rect.left() + Style::SP_S;
    let text_right = button_rect.left() - Style::SP_S;
    if text_left < text_right {
        let text_rect = egui::Rect::from_min_max(
            egui::pos2(text_left, panel_rect.top()),
            egui::pos2(text_right, panel_rect.bottom()),
        );
        ui.painter().with_clip_rect(text_rect).text(
            egui::pos2(text_rect.left(), text_rect.center().y),
            egui::Align2::LEFT_CENTER,
            command,
            egui::FontId::proportional(12.0),
            Style::TEXT_DIM,
        );
    }

    install_run_command_primary_action_accessibility(ui.ctx(), button_rect, command);
    response
}

fn result_domain_width(row_width: f32) -> f32 {
    let row_width = row_width.max(0.0);
    let fixed = Style::SP_S + ROW_ICON + Style::SP_XS + Style::SP_S + RESULT_TEXT_MIN_W;
    (row_width - fixed).clamp(DOMAIN_MIN_W, DOMAIN_W)
}

pub(crate) fn result_icon_id(hit: &SearchHit<FrontDoorTarget>) -> IconId {
    match (&hit.item.payload, hit.item.domain) {
        (FrontDoorTarget::App(surface), _) => surface.icon_id(),
        (FrontDoorTarget::Workflow(card), _) => card.icon,
        (FrontDoorTarget::PeerApp(_), _) => IconId::Desktop,
        (FrontDoorTarget::ServiceLifecycle(target), _) => match target.kind {
            FrontDoorLifecycleKind::Container => IconId::Server,
            FrontDoorLifecycleKind::Vm => IconId::Desktop,
        },
        (FrontDoorTarget::ConsoleCommand(command), _) => command.icon,
        (FrontDoorTarget::RunCommand(_), _) => IconId::Terminal,
        (_, SearchDomain::File) => IconId::Files,
        (_, SearchDomain::Mesh) => IconId::Node,
        (_, SearchDomain::BrowserBookmark) => IconId::Bookmarks,
        (_, SearchDomain::BrowserHistory) => IconId::History,
        (_, SearchDomain::WebSuggestion | SearchDomain::Assistant) => IconId::Search,
        (_, SearchDomain::App) => IconId::Search,
    }
}

fn accesskit_rect(rect: egui::Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

fn search_accesskit_id() -> egui::Id {
    egui::Id::new("shell-front-door-search-accesskit")
}

fn install_search_accessibility(ctx: &egui::Context, rect: egui::Rect, query: &str) {
    let _ = ctx.accesskit_node_builder(search_accesskit_id(), |node| {
        node.set_role(egui::accesskit::Role::SearchInput);
        node.set_label("Shell search");
        node.set_value(query);
        node.set_bounds(accesskit_rect(rect));
    });
}

fn install_filter_accessibility(
    ctx: &egui::Context,
    filter: FrontDoorFilter,
    rect: egui::Rect,
    selected: bool,
) {
    let _ = ctx.accesskit_node_builder(filter_chip_id(filter), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(format!("Filter: {}", filter.label()));
        node.set_value(if selected { "Selected" } else { "Available" });
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
        if selected {
            node.set_selected(true);
        }
    });
}

fn results_announcement_id() -> egui::Id {
    egui::Id::new("shell-front-door-results-accesskit")
}

fn plural_suffix(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

fn results_announcement_value(
    query: &str,
    hits: &[SearchHit<FrontDoorTarget>],
    selected: usize,
    filter: FrontDoorFilter,
    sources: FrontDoorSourceStatus,
) -> String {
    let query = query.trim();
    if query.is_empty() {
        if hits.is_empty() {
            if let Some(note) = front_door_source_note(query, filter, sources) {
                return note.value();
            }
            "Type to search apps, commands, files, mesh, Browser".to_owned()
        } else {
            let highlighted = &hits[selected.min(hits.len() - 1)].item.title;
            format!(
                "{} local shortcut{} available, {highlighted} highlighted",
                hits.len(),
                plural_suffix(hits.len())
            )
        }
    } else if hits.is_empty() {
        if let Some(note) = front_door_source_note(query, filter, sources) {
            return note.value();
        }
        format!("No local matches for {query}")
    } else {
        let highlighted = &hits[selected.min(hits.len() - 1)].item.title;
        format!(
            "{} result{} for {query}, {highlighted} highlighted",
            hits.len(),
            plural_suffix(hits.len())
        )
    }
}

fn install_results_announcement(
    ctx: &egui::Context,
    rect: egui::Rect,
    query: &str,
    hits: &[SearchHit<FrontDoorTarget>],
    selected: usize,
    filter: FrontDoorFilter,
    sources: FrontDoorSourceStatus,
) {
    let _ = ctx.accesskit_node_builder(results_announcement_id(), |node| {
        node.set_role(egui::accesskit::Role::Status);
        node.set_live(egui::accesskit::Live::Polite);
        node.set_label("Shell search results");
        node.set_value(results_announcement_value(
            query, hits, selected, filter, sources,
        ));
        node.set_bounds(accesskit_rect(rect));
    });
}

fn result_accesskit_id(hit: &SearchHit<FrontDoorTarget>) -> egui::Id {
    egui::Id::new((
        "shell-front-door-result-accesskit",
        result_domain_label(hit),
        hit.item.title.as_str(),
        hit.item.target.as_str(),
    ))
}

fn result_accesskit_value(hit: &SearchHit<FrontDoorTarget>, index: usize, total: usize) -> String {
    format!(
        "Result {} of {total}: {}, {}",
        index + 1,
        result_domain_label(hit),
        hit.item.target
    )
}

fn install_result_accessibility(
    ctx: &egui::Context,
    hit: &SearchHit<FrontDoorTarget>,
    rect: egui::Rect,
    selected: bool,
    index: usize,
    total: usize,
) {
    let _ = ctx.accesskit_node_builder(result_accesskit_id(hit), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(hit.item.title.as_str());
        node.set_value(result_accesskit_value(hit, index, total));
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
        if selected {
            node.set_selected(true);
        }
    });
}

pub(crate) const fn domain_label(domain: SearchDomain) -> &'static str {
    match domain {
        SearchDomain::App => "App",
        SearchDomain::File => "File",
        SearchDomain::Mesh => "Mesh",
        SearchDomain::BrowserBookmark => "Bookmark",
        SearchDomain::BrowserHistory => "History",
        SearchDomain::WebSuggestion => "Web",
        SearchDomain::Assistant => "Assistant",
    }
}

fn result_domain_label(hit: &SearchHit<FrontDoorTarget>) -> &'static str {
    match &hit.item.payload {
        FrontDoorTarget::ConsoleCommand(_) => "Command",
        FrontDoorTarget::Workflow(card) => card.kind.label(),
        FrontDoorTarget::PeerApp(_) => "Peer App",
        FrontDoorTarget::ServiceLifecycle(_) => "Service",
        _ => domain_label(hit.item.domain),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::Surface;
    use crate::screenshot::Capture;
    use std::path::Path;

    fn fixture_front_door_items() -> Vec<SearchItem<FrontDoorTarget>> {
        let file_target = FileSearchTarget {
            pane: 0,
            row: 2,
            path: None,
        };
        vec![
            SearchItem::new(
                SearchDomain::App,
                "Browser",
                launcher_group_label(Surface::Browser),
                FrontDoorTarget::App(Surface::Browser),
            ),
            SearchItem::new(
                SearchDomain::File,
                "browser-notes.md",
                "/home/mde/browser-notes.md",
                FrontDoorTarget::File(file_target),
            ),
            SearchItem::new(
                SearchDomain::Mesh,
                "browser-node",
                "peer:browser-node",
                FrontDoorTarget::Mesh("peer:browser-node".to_owned()),
            ),
            SearchItem::new(
                SearchDomain::BrowserBookmark,
                "Browser docs",
                "https://docs.mesh/browser",
                FrontDoorTarget::Browser("https://docs.mesh/browser".to_owned()),
            ),
            SearchItem::new(
                SearchDomain::BrowserHistory,
                "Browser status",
                "https://status.mesh/browser",
                FrontDoorTarget::Browser("https://status.mesh/browser".to_owned()),
            ),
            SearchItem::new(
                SearchDomain::WebSuggestion,
                "Search web for browser",
                "https://search.mesh/search?q=browser",
                FrontDoorTarget::Browser("browser".to_owned()),
            ),
        ]
    }

    fn fixture_console_search_hit() -> ConsoleSearchHit {
        ConsoleSearchHit {
            flat: 4,
            label: "Live Logs",
            desc: "Follow the system journal live",
            group: "System",
            tool: "journalctl",
            icon: IconId::Editor,
        }
    }

    fn fixture_front_door_items_with_command() -> Vec<SearchItem<FrontDoorTarget>> {
        let mut items = fixture_front_door_items();
        let rank = items.len();
        items.push(console_command_search_item(
            fixture_console_search_hit(),
            rank,
        ));
        items
    }

    fn fixture_cloud_instance_item() -> SearchItem<FrontDoorTarget> {
        SearchItem::new(
            SearchDomain::Mesh,
            "web",
            "cloud:instance:i-9",
            FrontDoorTarget::Mesh("cloud:instance:i-9".to_owned()),
        )
    }

    fn fixture_service_lifecycle_item(
        name: &str,
        kind: FrontDoorLifecycleKind,
        state: &str,
    ) -> SearchItem<FrontDoorTarget> {
        service_lifecycle_search_items(
            vec![FrontDoorLifecycleCandidate {
                host: "oak".to_owned(),
                kind,
                name: name.to_owned(),
                state: state.to_owned(),
                detail: "construct/mesh-api:latest".to_owned(),
            }],
            0,
        )
        .pop()
        .expect("service lifecycle item")
    }

    fn accesskit_nodes(
        out: &egui::FullOutput,
    ) -> Vec<(egui::accesskit::NodeId, egui::accesskit::Node)> {
        out.platform_output
            .accesskit_update
            .as_ref()
            .expect("accesskit update")
            .nodes
            .clone()
    }

    fn accesskit_bounds_rect(node: &egui::accesskit::Node) -> egui::Rect {
        let bounds = node.bounds().expect("accesskit node has bounds");
        egui::Rect::from_min_max(
            egui::pos2(bounds.x0 as f32, bounds.y0 as f32),
            egui::pos2(bounds.x1 as f32, bounds.y1 as f32),
        )
    }

    fn painted_rect_bounds(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Rect> {
        fn walk(shape: &egui::Shape, clip_rect: egui::Rect, out: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, clip_rect, out);
                    }
                }
                _ => {
                    let visible = shape.visual_bounding_rect().intersect(clip_rect);
                    if visible.is_positive() {
                        out.push(visible);
                    }
                }
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, clipped.clip_rect, &mut out);
        }
        out
    }

    fn painted_fill_colors(shapes: &[egui::epaint::ClippedShape]) -> Vec<egui::Color32> {
        fn walk(shape: &egui::Shape, out: &mut Vec<egui::Color32>) {
            match shape {
                egui::Shape::Mesh(mesh) => {
                    out.extend(mesh.vertices.iter().map(|vertex| vertex.color));
                }
                egui::Shape::Path(path) => {
                    if path.fill != egui::Color32::TRANSPARENT {
                        out.push(path.fill);
                    }
                }
                egui::Shape::Rect(rect) => {
                    if rect.fill != egui::Color32::TRANSPARENT {
                        out.push(rect.fill);
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Color32)> {
        fn text_color(text: &egui::epaint::TextShape) -> egui::Color32 {
            if let Some(color) = text.override_text_color {
                return color;
            }
            text.galley
                .job
                .sections
                .iter()
                .find_map(|section| {
                    (section.format.color != egui::Color32::PLACEHOLDER)
                        .then_some(section.format.color)
                })
                .unwrap_or(text.fallback_color)
        }

        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push((text.galley.text().to_owned(), text_color(text)));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }

        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    fn assert_varied_fill_colors(colors: &[egui::Color32], surface: &str) {
        assert!(
            !colors.is_empty(),
            "{surface} should hand filled shapes to the egui backend"
        );
        let first = colors[0];
        assert!(
            colors.iter().any(|color| *color != first),
            "{surface} should paint more than one fill tone: {colors:?}"
        );
    }

    fn looks_like_warning_fill(color: egui::Color32) -> bool {
        color.a() > 0
            && color.r() >= color.g()
            && color.g() > color.b()
            && color.r().saturating_sub(color.b()) >= 8
    }

    fn assert_rects_inside_viewport(out: &egui::FullOutput, width: f32, surface: &str) {
        let rects = painted_rect_bounds(&out.shapes);
        assert!(
            rects
                .iter()
                .all(|rect| rect.left() >= -0.5 && rect.right() <= width + 0.5),
            "{surface} painted rects must stay inside {width}px viewport: {rects:?}"
        );
    }

    fn fixture_many_front_door_items() -> Vec<SearchItem<FrontDoorTarget>> {
        let mut items = fixture_front_door_items();
        let base_rank = items.len();
        for idx in 0..24 {
            let url = format!("https://example.test/browser/{idx:02}");
            items.push(
                SearchItem::new(
                    SearchDomain::BrowserHistory,
                    format!("Browser page {idx:02}"),
                    url.clone(),
                    FrontDoorTarget::Browser(url),
                )
                .with_source_rank(base_rank + idx),
            );
        }
        items
    }

    fn render_front_door_accesskit_frame_with(
        ctx: &egui::Context,
        query: &str,
        selected: usize,
        screen_size: egui::Vec2,
        items: Vec<SearchItem<FrontDoorTarget>>,
    ) -> egui::FullOutput {
        render_front_door_accesskit_frame_with_filter(
            ctx,
            query,
            selected,
            screen_size,
            items,
            FrontDoorFilter::All,
        )
    }

    fn render_front_door_accesskit_frame_with_filter(
        ctx: &egui::Context,
        query: &str,
        selected: usize,
        screen_size: egui::Vec2,
        items: Vec<SearchItem<FrontDoorTarget>>,
        filter: FrontDoorFilter,
    ) -> egui::FullOutput {
        render_front_door_accesskit_frame_with_layout(
            ctx,
            query,
            selected,
            screen_size,
            items,
            filter,
            false,
        )
    }

    fn render_front_door_accesskit_frame_with_layout(
        ctx: &egui::Context,
        query: &str,
        selected: usize,
        screen_size: egui::Vec2,
        items: Vec<SearchItem<FrontDoorTarget>>,
        filter: FrontDoorFilter,
        expanded: bool,
    ) -> egui::FullOutput {
        render_front_door_accesskit_frame_with_layout_and_pins(
            ctx,
            query,
            selected,
            screen_size,
            items,
            filter,
            expanded,
            &[],
        )
    }

    fn render_front_door_accesskit_frame_with_layout_and_pins(
        ctx: &egui::Context,
        query: &str,
        selected: usize,
        screen_size: egui::Vec2,
        items: Vec<SearchItem<FrontDoorTarget>>,
        filter: FrontDoorFilter,
        expanded: bool,
        pinned: &[Surface],
    ) -> egui::FullOutput {
        render_front_door_accesskit_frame_with_sources(
            ctx,
            query,
            selected,
            screen_size,
            items,
            filter,
            expanded,
            pinned,
            FrontDoorSourceStatus::default(),
        )
    }

    fn render_front_door_accesskit_frame_with_sources(
        ctx: &egui::Context,
        query: &str,
        selected: usize,
        screen_size: egui::Vec2,
        items: Vec<SearchItem<FrontDoorTarget>>,
        filter: FrontDoorFilter,
        expanded: bool,
        pinned: &[Surface],
        sources: FrontDoorSourceStatus,
    ) -> egui::FullOutput {
        let mut state = FrontDoorState::default();
        state.open();
        state.query = query.to_owned();
        state.selected = selected;
        state.filter = filter;
        state.expanded = expanded;
        let pinned = pinned.to_vec();
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen_size)),
                time: Some(0.0),
                ..Default::default()
            },
            move |ctx| {
                let _ =
                    front_door_panel_with_sources(ctx, &mut state, items.clone(), &pinned, sources);
            },
        )
    }

    fn render_front_door_accesskit_frame(ctx: &egui::Context, selected: usize) -> egui::FullOutput {
        render_front_door_accesskit_frame_with(
            ctx,
            "browser",
            selected,
            egui::vec2(900.0, 640.0),
            fixture_front_door_items(),
        )
    }

    fn capture_front_door_canvas(
        ctx: &egui::Context,
        query: &str,
        selected: usize,
        screen_size: egui::Vec2,
        items: Vec<SearchItem<FrontDoorTarget>>,
        filter: FrontDoorFilter,
        expanded: bool,
        sources: FrontDoorSourceStatus,
    ) -> crate::screenshot::Canvas {
        let mut state = FrontDoorState::default();
        state.open();
        state.query = query.to_owned();
        state.selected = selected;
        state.filter = filter;
        state.expanded = expanded;
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen_size)),
            time: Some(0.0),
            ..Default::default()
        };
        let mut cap = Capture::new();
        let _settle = cap.frame(ctx, input(), |ctx| {
            let _ = front_door_panel_with_sources(ctx, &mut state, items.clone(), &[], sources);
        });
        cap.frame(ctx, input(), |ctx| {
            let _ = front_door_panel_with_sources(ctx, &mut state, items.clone(), &[], sources);
        })
    }

    fn render_front_door_settled_frame(
        ctx: &egui::Context,
        query: &str,
        selected: usize,
        screen_size: egui::Vec2,
        items: Vec<SearchItem<FrontDoorTarget>>,
        filter: FrontDoorFilter,
        expanded: bool,
        sources: FrontDoorSourceStatus,
    ) -> egui::FullOutput {
        let mut state = FrontDoorState::default();
        state.open();
        state.query = query.to_owned();
        state.selected = selected;
        state.filter = filter;
        state.expanded = expanded;
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen_size)),
            time: Some(0.0),
            ..Default::default()
        };
        let _settle = ctx.run(input(), |ctx| {
            let _ = front_door_panel_with_sources(ctx, &mut state, items.clone(), &[], sources);
        });
        ctx.run(input(), |ctx| {
            let _ = front_door_panel_with_sources(ctx, &mut state, items.clone(), &[], sources);
        })
    }

    fn render_front_door_tooltip_frame(ctx: &egui::Context) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 96.0),
                )),
                time: Some(0.0),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        front_door_tooltip(ui, "Expand Front Door");
                    });
            },
        )
    }

    fn render_front_door_context_row_frame(ctx: &egui::Context) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 140.0),
                )),
                time: Some(0.0),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        apply_front_door_context_style(ui.style_mut());
                        let _ = context_menu_row(
                            ui,
                            egui::Id::new("front-door-context-test-open"),
                            "Launch Browser",
                        );
                        let _ = context_menu_row(
                            ui,
                            egui::Id::new("front-door-context-test-pin"),
                            "Pin to top",
                        );
                    });
            },
        )
    }

    fn write_front_door_proof(canvas: &crate::screenshot::Canvas, name: &str) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("screenshots")
            .join(name);
        canvas
            .write_png(&path)
            .expect("write the Front Door rendered proof screenshot");
        println!(
            "Front Door rendered proof screenshot written to {}",
            path.display()
        );
    }

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn key_with_modifiers(k: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn front_door_open_suppresses_only_the_opening_frame_click_away() {
        let mut state = FrontDoorState::default();
        state.open();

        assert!(
            !front_door_clicked_elsewhere_should_close(&mut state, true),
            "the click that opened Front Door must not immediately close it"
        );
        assert!(
            front_door_clicked_elsewhere_should_close(&mut state, true),
            "later outside clicks should still close Front Door"
        );

        state.open();
        assert!(
            !front_door_clicked_elsewhere_should_close(&mut state, false),
            "a normal inside frame clears the one-shot opening guard"
        );
        assert!(
            front_door_clicked_elsewhere_should_close(&mut state, true),
            "outside clicks after an inside frame should close normally"
        );
    }

    fn drive_front_door_action(
        ctx: &egui::Context,
        query: &str,
        events: Vec<egui::Event>,
    ) -> Option<FrontDoorTarget> {
        let mut state = FrontDoorState::default();
        state.open();
        state.query = query.to_owned();
        let mut action = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 640.0),
                )),
                events,
                time: Some(0.0),
                ..Default::default()
            },
            |ctx| {
                action = front_door_panel(ctx, &mut state, fixture_front_door_items(), &[]);
            },
        );
        action.and_then(|request| match request {
            FrontDoorRequest::Activate(target) => Some(target),
            FrontDoorRequest::LaunchPeerApp(_)
            | FrontDoorRequest::ConnectDesktopSource(_)
            | FrontDoorRequest::InstanceLifecycle { .. }
            | FrontDoorRequest::ServiceLifecycle { .. }
            | FrontDoorRequest::OpenWorkbenchPlane(_)
            | FrontDoorRequest::TogglePin(_)
            | FrontDoorRequest::MovePin { .. } => None,
        })
    }

    fn drive_front_door_filter_keyboard(
        ctx: &egui::Context,
        filter: FrontDoorFilter,
        events: Vec<egui::Event>,
    ) -> FrontDoorFilter {
        let mut state = FrontDoorState::default();
        state.open();
        state.query = "browser".to_owned();
        state.filter = filter;
        let modifiers = events
            .iter()
            .find_map(|event| match event {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or_default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 640.0),
                )),
                modifiers,
                events,
                time: Some(0.0),
                ..Default::default()
            },
            |ctx| {
                let _ = front_door_panel(
                    ctx,
                    &mut state,
                    fixture_front_door_items_with_command(),
                    &[],
                );
            },
        );
        state.filter
    }

    #[test]
    fn app_search_items_cover_the_shell_surface_inventory() {
        let items = app_search_items();
        assert_eq!(items.len(), Surface::ALL.len());
        assert!(items.iter().any(|item| {
            item.domain == SearchDomain::App
                && item.title == Surface::Browser.label()
                && item.payload == FrontDoorTarget::App(Surface::Browser)
        }));
        let browser = items
            .iter()
            .find(|item| item.payload == FrontDoorTarget::App(Surface::Browser))
            .expect("Browser app search row");
        let browser_group = launcher_group_label(Surface::Browser);
        assert_eq!(browser.target, browser_group);
        assert!(
            browser.terms.iter().any(|term| term == browser_group),
            "Front Door app rows should be searchable by the shared launcher group"
        );
    }

    #[test]
    fn app_search_items_with_pins_places_favorites_first_once() {
        let items = app_search_items_with_pins(&[
            Surface::Browser,
            Surface::Files,
            Surface::Browser,
            Surface::Timers,
        ]);
        assert_eq!(items.len(), Surface::ALL.len());
        assert_eq!(items[0].payload, FrontDoorTarget::App(Surface::Browser));
        assert_eq!(items[1].payload, FrontDoorTarget::App(Surface::Files));
        assert_eq!(
            items
                .iter()
                .filter(|item| item.payload == FrontDoorTarget::App(Surface::Browser))
                .count(),
            1,
            "pinned app rows should not duplicate their normal launcher row"
        );
        assert!(
            !items
                .iter()
                .any(|item| item.payload == FrontDoorTarget::App(Surface::Timers)),
            "Timers remains taskbar-clock owned, not a Front Door app row"
        );
    }

    #[test]
    fn workflow_search_items_expose_real_owner_cards_without_duplicating_apps() {
        let items = workflow_search_items(40);
        assert_eq!(items.len(), 4);
        assert!(items.iter().any(|item| {
            item.title == "Cloud workloads"
                && item.target == "Instances, volumes, networks"
                && matches!(
                    item.payload,
                    FrontDoorTarget::Workflow(FrontDoorWorkflowCard {
                        kind: FrontDoorWorkflowKind::Workload,
                        surface: Surface::InfraCode,
                        ..
                    })
                )
        }));
        assert!(items.iter().any(|item| {
            item.title == "Mesh services"
                && item.target == "Fleet service health and controls"
                && matches!(
                    item.payload,
                    FrontDoorTarget::Workflow(FrontDoorWorkflowCard {
                        kind: FrontDoorWorkflowKind::Service,
                        surface: Surface::Workbench,
                        workbench_plane: Some(Plane::Provisioning),
                        ..
                    })
                )
        }));
        assert!(
            items
                .iter()
                .all(|item| !matches!(item.payload, FrontDoorTarget::App(_))),
            "workflow cards must be distinct payloads, not renamed app shortcuts"
        );
    }

    #[test]
    fn blank_front_door_hits_show_initial_shortcuts_in_source_order() {
        let hits = visible_front_door_hits(
            "  ",
            app_search_items_with_pins(&[Surface::Browser, Surface::Files]),
        );
        assert_eq!(hits.len(), MAX_HITS);
        assert_eq!(hits[0].item.payload, FrontDoorTarget::App(Surface::Browser));
        assert_eq!(hits[0].item.target, launcher_group_label(Surface::Browser));
        assert_eq!(hits[1].item.payload, FrontDoorTarget::App(Surface::Files));
        assert_eq!(hits[1].item.target, "Files & Data");
    }

    #[test]
    fn front_door_filter_chips_keep_domains_separate() {
        let app_hits = visible_front_door_hits_for_filter(
            "  ",
            FrontDoorFilter::Apps,
            fixture_front_door_items_with_command(),
        );
        assert_eq!(app_hits.len(), 1);
        assert_eq!(app_hits[0].item.domain, SearchDomain::App);
        assert!(matches!(&app_hits[0].item.payload, FrontDoorTarget::App(_)));

        let file_hits = visible_front_door_hits_for_filter(
            "browser",
            FrontDoorFilter::Files,
            fixture_front_door_items(),
        );
        assert_eq!(file_hits.len(), 1);
        assert!(file_hits
            .iter()
            .all(|hit| hit.item.domain == SearchDomain::File));

        let browser_hits = visible_front_door_hits_for_filter(
            "browser",
            FrontDoorFilter::Browser,
            fixture_front_door_items(),
        );
        let browser_domains: Vec<SearchDomain> =
            browser_hits.iter().map(|hit| hit.item.domain).collect();
        assert_eq!(browser_domains.len(), 2);
        assert!(
            browser_domains.contains(&SearchDomain::BrowserBookmark)
                && browser_domains.contains(&SearchDomain::BrowserHistory),
            "Browser filter should mean Browser history/bookmarks, not the Browser app row"
        );

        let workflow_items = workflow_search_items(0);
        let workload_hits =
            visible_front_door_hits_for_filter("  ", FrontDoorFilter::Workloads, workflow_items);
        let workload_titles: Vec<&str> = workload_hits
            .iter()
            .map(|hit| hit.item.title.as_str())
            .collect();
        assert_eq!(
            workload_titles,
            vec!["Cloud workloads", "Desktop sessions"],
            "Workloads filter should expose current local workload cards"
        );
        assert!(workload_hits.iter().all(|hit| {
            matches!(
                &hit.item.payload,
                FrontDoorTarget::Workflow(card) if card.kind == FrontDoorWorkflowKind::Workload
            )
        }));

        let service_hits = visible_front_door_hits_for_filter(
            "  ",
            FrontDoorFilter::Services,
            workflow_search_items(0),
        );
        let service_titles: Vec<&str> = service_hits
            .iter()
            .map(|hit| hit.item.title.as_str())
            .collect();
        assert_eq!(
            service_titles,
            vec!["Mesh services", "Cloud API services"],
            "Services filter should expose current local service cards"
        );
        assert!(service_hits.iter().all(|hit| {
            matches!(
                &hit.item.payload,
                FrontDoorTarget::Workflow(card) if card.kind == FrontDoorWorkflowKind::Service
            )
        }));

        let command_hits = visible_front_door_hits_for_filter(
            "logs",
            FrontDoorFilter::Commands,
            fixture_front_door_items_with_command(),
        );
        assert_eq!(command_hits.len(), 1);
        assert!(matches!(
            &command_hits[0].item.payload,
            FrontDoorTarget::ConsoleCommand(hit)
                if hit.label == "Live Logs" && hit.group == "System"
        ));
        assert_eq!(
            result_domain_label(&command_hits[0]),
            "Command",
            "Console rows should render as command results, not app rows"
        );
    }

    #[test]
    fn front_door_filter_keyboard_traversal_cycles_filter_chips() {
        assert_eq!(
            moved_filter(FrontDoorFilter::All, FrontDoorFilterStep::Next),
            FrontDoorFilter::Apps
        );
        assert_eq!(
            moved_filter(FrontDoorFilter::All, FrontDoorFilterStep::Previous),
            FrontDoorFilter::Web
        );

        let ctx = egui::Context::default();
        mde_egui::fonts::install(&ctx);
        let ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let ctrl_shift = egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        let alt = egui::Modifiers {
            alt: true,
            ..Default::default()
        };

        assert_eq!(
            drive_front_door_filter_keyboard(
                &ctx,
                FrontDoorFilter::All,
                vec![key_with_modifiers(egui::Key::Tab, ctrl)]
            ),
            FrontDoorFilter::Apps,
            "Ctrl+Tab should advance Front Door filter chips without requiring mouse input"
        );
        assert_eq!(
            drive_front_door_filter_keyboard(
                &ctx,
                FrontDoorFilter::All,
                vec![key_with_modifiers(egui::Key::Tab, ctrl_shift)]
            ),
            FrontDoorFilter::Web,
            "Ctrl+Shift+Tab should move backward through Front Door filter chips"
        );
        assert_eq!(
            drive_front_door_filter_keyboard(
                &ctx,
                FrontDoorFilter::Files,
                vec![key_with_modifiers(egui::Key::ArrowLeft, alt)]
            ),
            FrontDoorFilter::Services,
            "Alt+Left should move to the previous Front Door filter chip"
        );
    }

    #[test]
    fn front_door_mesh_source_status_gates_mesh_rows_without_blocking_local_hits() {
        let unavailable = FrontDoorSourceStatus::new(FrontDoorMeshSourceStatus::Unavailable);

        let all_hits = visible_front_door_hits_for_filter_with_sources(
            "browser",
            FrontDoorFilter::All,
            fixture_front_door_items(),
            unavailable,
        );
        let domains: Vec<SearchDomain> = all_hits.iter().map(|hit| hit.item.domain).collect();
        assert!(
            !domains.contains(&SearchDomain::Mesh),
            "unavailable mesh source must not leave stale mesh rows activatable"
        );
        assert!(
            domains.contains(&SearchDomain::App)
                && domains.contains(&SearchDomain::File)
                && domains.contains(&SearchDomain::BrowserBookmark),
            "local launcher rows should remain immediate while mesh is gated: {domains:?}"
        );

        let mesh_hits = visible_front_door_hits_for_filter_with_sources(
            "browser",
            FrontDoorFilter::Mesh,
            fixture_front_door_items(),
            unavailable,
        );
        assert!(
            mesh_hits.is_empty(),
            "Mesh filter should show the degraded status row, not a stale activatable result"
        );

        let ready_hits = visible_front_door_hits_for_filter_with_sources(
            "browser",
            FrontDoorFilter::Mesh,
            fixture_front_door_items(),
            FrontDoorSourceStatus::default(),
        );
        assert!(ready_hits
            .iter()
            .any(|hit| hit.item.domain == SearchDomain::Mesh));
    }

    #[test]
    fn front_door_mesh_filter_reports_degraded_source_status() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let out = render_front_door_accesskit_frame_with_sources(
            &ctx,
            "browser",
            0,
            egui::vec2(900.0, 640.0),
            fixture_front_door_items(),
            FrontDoorFilter::Mesh,
            false,
            &[],
            FrontDoorSourceStatus::new(FrontDoorMeshSourceStatus::Unavailable),
        );
        let nodes = accesskit_nodes(&out);

        let source = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Mesh results unavailable"))
            .expect("degraded mesh source status row");
        assert_eq!(source.role(), egui::accesskit::Role::Status);
        assert_eq!(
            source.value(),
            Some("Mesh results unavailable: Local launcher results remain available")
        );
        assert!(
            !nodes
                .iter()
                .map(|(_, node)| node)
                .any(|node| node.label() == Some("browser-node")),
            "gated mesh rows must not remain exposed to keyboard/screen-reader activation"
        );

        let announcement = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Shell search results"))
            .expect("results live-region");
        assert_eq!(announcement.value(), source.value());
    }

    #[test]
    fn front_door_search_is_ephemeral_and_local_only() {
        // The policy point exists and is on.
        assert!(
            SEARCH_PRIVACY_EPHEMERAL_LOCAL_ONLY,
            "unified search must stay ephemeral + local-only (WL-FUNC-005)"
        );

        // Ephemeral: the live query lives only in FrontDoorState, and closing the
        // panel wipes it — there is no persistent search-query history to leak.
        let mut state = FrontDoorState::default();
        state.open();
        state.query = "secret internal codename".to_owned();
        assert_eq!(state.query(), "secret internal codename");
        state.close();
        assert!(
            state.query().is_empty(),
            "closing the omnibox must wipe the live query — no persistent search history"
        );
        // Re-opening starts blank; no recent-search list is restored.
        state.open();
        assert!(
            state.query().is_empty(),
            "re-opening the omnibox must not restore any prior query"
        );

        // Local-only + no hidden state: ranking a query is a pure transform over
        // already-local candidates. It is deterministic (no I/O, no side effects)
        // and the only egress-capable row — the explicit "Search web for …"
        // suggestion — stays on the mesh-local search endpoint, never an external
        // provider. Every other candidate resolves to an in-mesh / on-node target.
        let first = ranked_front_door_hits("browser", fixture_front_door_items());
        let second = ranked_front_door_hits("browser", fixture_front_door_items());
        assert_eq!(
            first.len(),
            second.len(),
            "ranking must be a pure, side-effect-free transform"
        );
        for hit in &first {
            let target = hit.item.target.to_ascii_lowercase();
            let external = target.contains("google.com")
                || target.contains("bing.com")
                || target.contains("duckduckgo.com")
                || target.contains("yahoo.com");
            assert!(
                !external,
                "no omnibox candidate may carry an off-mesh query egress target: {:?}",
                hit.item.target
            );
            if hit.item.domain == SearchDomain::WebSuggestion {
                assert!(
                    target.contains("search.mesh"),
                    "the web-search suggestion must target the mesh-local endpoint: {:?}",
                    hit.item.target
                );
            }
        }
    }

    #[test]
    fn front_door_mesh_rows_rank_healthy_source_over_degraded_over_down() {
        // Same title + tier, differing only in source health: the front door must
        // surface the healthy node first, then degraded, then down (WL-FUNC-005).
        use mde_egui::search_omnibox::SourceHealth;
        let items = vec![
            SearchItem::new(
                SearchDomain::Mesh,
                "oak-node",
                "peer:oak-down",
                FrontDoorTarget::Mesh("peer:oak-down".to_owned()),
            )
            .with_source_rank(0)
            .with_source_health(SourceHealth::Down),
            SearchItem::new(
                SearchDomain::Mesh,
                "oak-node",
                "peer:oak-healthy",
                FrontDoorTarget::Mesh("peer:oak-healthy".to_owned()),
            )
            .with_source_rank(1)
            .with_source_health(SourceHealth::Healthy),
            SearchItem::new(
                SearchDomain::Mesh,
                "oak-node",
                "peer:oak-degraded",
                FrontDoorTarget::Mesh("peer:oak-degraded".to_owned()),
            )
            .with_source_rank(2)
            .with_source_health(SourceHealth::Degraded),
        ];

        let payloads: Vec<String> = ranked_front_door_hits("oak", items)
            .into_iter()
            .filter_map(|hit| match hit.item.payload {
                FrontDoorTarget::Mesh(id) => Some(id),
                _ => None,
            })
            .collect();

        assert_eq!(
            payloads,
            ["peer:oak-healthy", "peer:oak-degraded", "peer:oak-down"]
        );
    }

    #[test]
    fn front_door_ranking_accepts_apps_files_mesh_browser_web_and_commands() {
        let hits = ranked_front_door_hits("browser", fixture_front_door_items());
        let domains: Vec<SearchDomain> = hits.iter().map(|hit| hit.item.domain).collect();
        assert!(domains.contains(&SearchDomain::App));
        assert!(domains.contains(&SearchDomain::File));
        assert!(domains.contains(&SearchDomain::Mesh));
        assert!(domains.contains(&SearchDomain::BrowserBookmark));
        assert!(domains.contains(&SearchDomain::BrowserHistory));
        assert!(domains.contains(&SearchDomain::WebSuggestion));

        let command_hits = ranked_front_door_hits("logs", fixture_front_door_items_with_command());
        assert!(command_hits.iter().any(|hit| {
            matches!(&hit.item.payload, FrontDoorTarget::ConsoleCommand(command) if command.label == "Live Logs")
        }));

        let workload_hits = ranked_front_door_hits("workloads", workflow_search_items(0));
        assert!(workload_hits.iter().any(|hit| {
            matches!(
                &hit.item.payload,
                FrontDoorTarget::Workflow(card)
                    if card.kind == FrontDoorWorkflowKind::Workload
                        && card.surface == Surface::InfraCode
            )
        }));
        assert!(workload_hits.iter().any(|hit| {
            matches!(
                &hit.item.payload,
                FrontDoorTarget::Workflow(card)
                    if card.kind == FrontDoorWorkflowKind::Workload
                        && card.surface == Surface::Desktop
            )
        }));

        let service_hits = ranked_front_door_hits("services", workflow_search_items(0));
        assert!(service_hits.iter().any(|hit| {
            matches!(
                &hit.item.payload,
                FrontDoorTarget::Workflow(card)
                    if card.kind == FrontDoorWorkflowKind::Service
                        && card.surface == Surface::Workbench
            )
        }));
        assert!(service_hits.iter().any(|hit| {
            matches!(
                &hit.item.payload,
                FrontDoorTarget::Workflow(card)
                    if card.kind == FrontDoorWorkflowKind::Service
                        && card.surface == Surface::InfraCode
            )
        }));
    }

    #[test]
    fn front_door_cloud_instance_lifecycle_uses_the_explorer_cloud_contract() {
        let (topic, body) = cloud_instance_lifecycle_wire(
            "cloud:instance:i-9",
            FrontDoorInstanceLifecycleOp::Reboot,
        )
        .expect("cloud instance lifecycle wire");

        assert_eq!(topic, "action/cloud/instance-reboot");
        assert_eq!(body, r#"{"instance":"i-9"}"#);
        assert!(
            cloud_instance_lifecycle_wire("peer:browser-node", FrontDoorInstanceLifecycleOp::Start)
                .is_none(),
            "Front Door must not mint lifecycle actions for non-instance mesh ids"
        );
    }

    #[test]
    fn front_door_instance_lifecycle_destructive_actions_are_armed_first() {
        let hit = ranked_front_door_hits("web", vec![fixture_cloud_instance_item()])
            .pop()
            .expect("cloud instance hit");
        let target = instance_lifecycle_target_for_hit(&hit).expect("instance target");
        let mut state = FrontDoorState::default();

        assert_eq!(
            lifecycle_action_clicked(&mut state, &target, FrontDoorInstanceLifecycleOp::Start),
            Some(FrontDoorRequest::InstanceLifecycle {
                unit_id: "cloud:instance:i-9".to_owned(),
                op: FrontDoorInstanceLifecycleOp::Start,
            }),
            "Start is the non-destructive lifecycle verb and publishes immediately"
        );
        assert!(state.lifecycle_arm.is_none());

        assert_eq!(
            lifecycle_action_clicked(&mut state, &target, FrontDoorInstanceLifecycleOp::Stop),
            None,
            "Stop must arm on first click"
        );
        assert!(lifecycle_action_armed_for(
            &state,
            &target,
            FrontDoorInstanceLifecycleOp::Stop
        ));
        assert_eq!(
            lifecycle_action_clicked(&mut state, &target, FrontDoorInstanceLifecycleOp::Stop),
            Some(FrontDoorRequest::InstanceLifecycle {
                unit_id: "cloud:instance:i-9".to_owned(),
                op: FrontDoorInstanceLifecycleOp::Stop,
            }),
            "The matching second click confirms the armed lifecycle action"
        );
        assert!(state.lifecycle_arm.is_none());
    }

    #[test]
    fn front_door_service_lifecycle_uses_the_directory_contract() {
        let item = fixture_service_lifecycle_item(
            "mesh-api",
            FrontDoorLifecycleKind::Container,
            "running",
        );
        let FrontDoorTarget::ServiceLifecycle(target) = item.payload else {
            panic!("expected service lifecycle payload");
        };

        let (topic, body) = service_lifecycle_wire(&target, FrontDoorServiceLifecycleOp::Restart);
        let body: serde_json::Value = serde_json::from_str(&body).expect("json lifecycle body");

        assert_eq!(topic, "action/services/lifecycle");
        assert_eq!(body["peer"], "oak");
        assert_eq!(body["kind"], "container");
        assert_eq!(body["name"], "mesh-api");
        assert_eq!(body["op"], "restart");
    }

    #[test]
    fn front_door_service_lifecycle_destructive_actions_are_armed_first() {
        let hit = ranked_front_door_hits(
            "mesh-api",
            vec![fixture_service_lifecycle_item(
                "mesh-api",
                FrontDoorLifecycleKind::Container,
                "running",
            )],
        )
        .pop()
        .expect("service lifecycle hit");
        let target = service_lifecycle_target_for_hit(&hit).expect("service target");
        let mut state = FrontDoorState::default();

        assert_eq!(
            service_lifecycle_action_clicked(
                &mut state,
                target,
                FrontDoorServiceLifecycleOp::Start
            ),
            Some(FrontDoorRequest::ServiceLifecycle {
                target: (*target).clone(),
                op: FrontDoorServiceLifecycleOp::Start,
            }),
            "Start remains a one-click request"
        );
        assert!(state.service_lifecycle_arm.is_none());

        assert_eq!(
            service_lifecycle_action_clicked(
                &mut state,
                target,
                FrontDoorServiceLifecycleOp::Restart
            ),
            None,
            "Restart must arm before dispatch"
        );
        assert!(service_lifecycle_action_armed_for(
            &state,
            target,
            FrontDoorServiceLifecycleOp::Restart
        ));
        assert_eq!(
            service_lifecycle_action_clicked(
                &mut state,
                target,
                FrontDoorServiceLifecycleOp::Restart
            ),
            Some(FrontDoorRequest::ServiceLifecycle {
                target: (*target).clone(),
                op: FrontDoorServiceLifecycleOp::Restart,
            }),
            "The matching second click confirms the service lifecycle action"
        );
        assert!(state.service_lifecycle_arm.is_none());
    }

    #[test]
    fn nonblank_front_door_hits_still_use_shared_ranking() {
        assert_eq!(
            visible_front_door_hits("  ", fixture_front_door_items()).len(),
            fixture_front_door_items().len(),
            "blank panel mode preserves source order"
        );
        assert_eq!(
            visible_front_door_hits("browser", fixture_front_door_items()),
            ranked_front_door_hits("browser", fixture_front_door_items()),
            "nonblank queries must keep the shared omnibox ranker"
        );
    }

    #[test]
    fn front_door_greater_than_input_enters_run_command_mode() {
        assert!(!run_command_mode("browser"));
        assert!(run_command_mode("> journalctl -xe"));
        assert!(run_command_mode("   >   "));
        assert_eq!(
            run_command_query("> journalctl -xe"),
            Some("journalctl -xe")
        );
        assert_eq!(run_command_query("   >   btop  "), Some("btop"));
        assert_eq!(run_command_query(">   "), None);

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let action = drive_front_door_action(&ctx, "> journalctl -xe", vec![key(egui::Key::Enter)]);
        assert_eq!(
            action,
            Some(FrontDoorTarget::RunCommand("journalctl -xe".to_owned())),
            "Enter in > mode should return a distinct run-command target, not a ranked search hit"
        );
    }

    #[test]
    fn front_door_run_command_mode_exports_explicit_action_not_ranked_results() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let out = render_front_door_accesskit_frame_with(
            &ctx,
            "> journalctl -xe",
            0,
            egui::vec2(900.0, 640.0),
            fixture_front_door_items(),
        );
        let nodes = accesskit_nodes(&out);
        let row = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Run command"))
            .expect("Front Door > mode should expose an explicit command row");
        assert_eq!(row.role(), egui::accesskit::Role::Button);
        assert_eq!(row.value(), Some("Command: journalctl -xe"));
        assert_eq!(row.is_selected(), Some(true));
        assert!(
            row.supports_action(egui::accesskit::Action::Click),
            "the command row should be actionable without masquerading as a search result"
        );

        let action = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Run command in Terminal"))
            .expect("Front Door > mode should expose an explicit primary Run action");
        assert_eq!(action.role(), egui::accesskit::Role::Button);
        assert_eq!(action.value(), Some("Command: journalctl -xe"));
        assert!(action.supports_action(egui::accesskit::Action::Click));
        assert!(
            !nodes
                .iter()
                .map(|(_, node)| node)
                .any(|node| node.label() == Some("Browser")),
            "> mode must bypass normal ranked app/file/browser result rows"
        );
    }

    #[test]
    fn front_door_domain_labels_cover_every_shared_domain() {
        for domain in [
            SearchDomain::App,
            SearchDomain::File,
            SearchDomain::Mesh,
            SearchDomain::BrowserBookmark,
            SearchDomain::BrowserHistory,
            SearchDomain::WebSuggestion,
            SearchDomain::Assistant,
        ] {
            assert!(!domain_label(domain).is_empty());
        }
    }

    #[test]
    fn front_door_result_icons_cover_apps_and_shared_domains() {
        let hits = ranked_front_door_hits("browser", fixture_front_door_items());
        let icons: Vec<(SearchDomain, IconId)> = hits
            .iter()
            .map(|hit| (hit.item.domain, result_icon_id(hit)))
            .collect();
        assert!(icons.contains(&(SearchDomain::App, IconId::Browser)));
        assert!(icons.contains(&(SearchDomain::File, IconId::Files)));
        assert!(icons.contains(&(SearchDomain::Mesh, IconId::Node)));
        assert!(icons.contains(&(SearchDomain::BrowserBookmark, IconId::Bookmarks)));
        assert!(icons.contains(&(SearchDomain::BrowserHistory, IconId::History)));
        assert!(icons.contains(&(SearchDomain::WebSuggestion, IconId::Search)));

        let assistant = SearchItem::new(
            SearchDomain::Assistant,
            "Ask about browser",
            "assistant:browser",
            FrontDoorTarget::Browser("browser".to_owned()),
        );
        let assistant_hit = ranked_front_door_hits("browser", vec![assistant])
            .into_iter()
            .next()
            .expect("assistant search hit");
        assert_eq!(result_icon_id(&assistant_hit), IconId::Search);

        let command_hit = ranked_front_door_hits(
            "logs",
            vec![console_command_search_item(fixture_console_search_hit(), 0)],
        )
        .into_iter()
        .next()
        .expect("console command search hit");
        assert_eq!(result_icon_id(&command_hit), IconId::Editor);
        assert_eq!(result_domain_label(&command_hit), "Command");

        let service_hit = ranked_front_door_hits("mesh services", workflow_search_items(0))
            .into_iter()
            .next()
            .expect("service workflow search hit");
        assert_eq!(result_domain_label(&service_hit), "Service");
        assert_eq!(result_icon_id(&service_hit), IconId::Workbench);

        let workload_hit = ranked_front_door_hits("desktop sessions", workflow_search_items(0))
            .into_iter()
            .next()
            .expect("workload workflow search hit");
        assert_eq!(result_domain_label(&workload_hit), "Workload");
        assert_eq!(result_icon_id(&workload_hit), IconId::Desktop);
    }

    #[test]
    fn front_door_results_height_is_viewport_bounded_on_short_screens() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 480.0));
        let max_height = front_door_results_max_height(screen, false);
        let full_content_height = front_door_results_content_height(MAX_HITS);

        assert_eq!(front_door_results_content_height(0), ROW_H);
        assert_eq!(
            front_door_results_content_height(1),
            ROW_H + ACTION_PANEL_H,
            "a selected Front Door row reserves room for its primary action strip"
        );
        assert!(
            max_height < full_content_height,
            "short screens should switch the result list to bounded scrolling"
        );
        assert!(max_height >= ROW_H);
        assert!(
            front_door_panel_top(screen, false)
                + front_door_non_results_height()
                + max_height
                + front_door_screen_margin(screen)
                <= screen.bottom() + 0.01,
            "front-door panel should leave a bottom gutter instead of overflowing"
        );
    }

    #[test]
    fn front_door_results_height_allows_full_list_on_tall_screens() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 900.0));

        assert!(
            front_door_results_max_height(screen, false)
                >= front_door_results_content_height(MAX_HITS),
            "tall screens should keep the normal full result list height"
        );
    }

    #[test]
    fn front_door_panel_mode_uses_one_bounded_outer_rect() {
        let screen = egui::Rect::from_min_size(egui::pos2(40.0, 20.0), egui::vec2(900.0, 480.0));
        let margin = front_door_screen_margin(screen);
        let compact = front_door_panel_rect(screen, false);

        assert!(
            (compact.center().x - screen.center().x).abs() < 0.01,
            "compact Front Door should be centered within the real screen rect: {compact:?}"
        );
        assert!(
            compact.left() >= screen.left() + margin - 0.01
                && compact.right() <= screen.right() - margin + 0.01,
            "compact Front Door width should stay inside the screen gutters: {compact:?}"
        );
        assert!(
            compact.bottom() <= screen.bottom() - margin + 0.01,
            "compact Front Door should not be sliced off the bottom edge: {compact:?}"
        );
        assert!(
            (front_door_results_max_height(screen, false) + front_door_non_results_height()
                - compact.height())
            .abs()
                < 0.01,
            "the result scroll cap should be derived from the same compact panel rect"
        );

        let ctx = egui::Context::default();
        mde_egui::fonts::install(&ctx);
        let mut state = FrontDoorState::default();
        state.open();
        state.query = "browser".to_owned();
        let items = fixture_many_front_door_items();
        let _out = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                time: Some(0.0),
                ..Default::default()
            },
            |ctx| {
                let _ = front_door_panel(ctx, &mut state, items.clone(), &[]);
            },
        );
        let area = ctx
            .read_response(egui::Id::new(AREA_ID))
            .expect("Front Door Area response registered")
            .rect;
        assert!(
            (area.left() - compact.left()).abs() < 0.5
                && (area.right() - compact.right()).abs() < 0.5
                && (area.top() - compact.top()).abs() < 0.5
                && (area.bottom() - compact.bottom()).abs() < 0.5,
            "the rendered Area must use the same compact outer rect, expected {compact:?}, got {area:?}"
        );
    }

    #[test]
    fn front_door_expanded_layout_uses_bounded_screen_geometry() {
        let desktop = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let portrait = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(430.0, 900.0));
        let narrow = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(280.0, 640.0));

        assert!(
            front_door_panel_width(desktop, true) > front_door_panel_width(desktop, false),
            "expanded mode should use the wide launcher canvas on desktop screens"
        );
        assert!(
            front_door_panel_top(desktop, true) < front_door_panel_top(desktop, false),
            "expanded mode should move toward the top of the workspace"
        );
        assert!(
            front_door_results_max_height(desktop, true)
                > front_door_results_max_height(desktop, false),
            "expanded mode should expose more vertical space for launcher rows"
        );

        let margin = front_door_screen_margin(portrait);
        let expanded_width = front_door_panel_width(portrait, true);
        let expanded_pos = front_door_panel_pos(portrait, expanded_width, true);
        assert!(expanded_pos.x >= portrait.left() + margin - 0.01);
        assert!(
            expanded_pos.x + expanded_width <= portrait.right() + 0.01,
            "expanded mode should stay within portrait/tablet width"
        );

        for expanded in [false, true] {
            let width = front_door_panel_width(narrow, expanded);
            let pos = front_door_panel_pos(narrow, width, expanded);
            assert!(
                pos.x >= narrow.left() - 0.01 && pos.x + width <= narrow.right() + 0.01,
                "Front Door {:?} mode must stay inside a narrow viewport: pos={pos:?} width={width}",
                if expanded { "expanded" } else { "panel" }
            );
        }
        let expanded_height =
            front_door_panel_min_height(narrow, true).expect("expanded height is set");
        assert!(
            front_door_panel_top(narrow, true) + expanded_height <= narrow.bottom() + 0.01,
            "expanded Front Door height must not push below the narrow viewport"
        );

        let ctx = egui::Context::default();
        mde_egui::fonts::install(&ctx);
        let out = render_front_door_accesskit_frame_with_layout(
            &ctx,
            "",
            0,
            egui::vec2(280.0, 640.0),
            fixture_front_door_items(),
            FrontDoorFilter::All,
            true,
        );
        assert_rects_inside_viewport(&out, 280.0, "narrow expanded Front Door");
    }

    #[test]
    fn front_door_narrow_panel_chips_and_rows_stay_bounded() {
        let search_threshold = EXPANSION_BUTTON_W + Style::SP_XS + SEARCH_MIN_W;
        assert!(!show_expansion_control(search_threshold - 1.0));
        assert!(show_expansion_control(search_threshold));
        assert_eq!(
            search_field_width(search_threshold - 1.0, false),
            search_threshold - 1.0
        );
        assert_eq!(search_field_width(search_threshold, true), SEARCH_MIN_W);

        let narrow_chip_widths = filter_chip_widths(260.0);
        let chip_total: f32 = narrow_chip_widths.iter().sum::<f32>()
            + FILTER_CHIP_GAP * (FrontDoorFilter::ALL.len() - 1) as f32;
        assert!(
            chip_total <= 260.0 + 0.01,
            "compact Front Door chips should fit their row budget: {narrow_chip_widths:?}"
        );
        let browser_idx = FrontDoorFilter::ALL
            .iter()
            .position(|filter| *filter == FrontDoorFilter::Browser)
            .expect("Browser filter is present");
        assert!(narrow_chip_widths[browser_idx] < FrontDoorFilter::Browser.width());
        assert_eq!(result_domain_width(320.0), DOMAIN_W);
        assert!(result_domain_width(180.0) < DOMAIN_W);

        let ctx = egui::Context::default();
        mde_egui::fonts::install(&ctx);
        let out = render_front_door_accesskit_frame_with_filter(
            &ctx,
            "browser",
            0,
            egui::vec2(280.0, 640.0),
            fixture_many_front_door_items(),
            FrontDoorFilter::All,
        );

        assert_rects_inside_viewport(&out, 280.0, "narrow Front Door panel");
    }

    #[test]
    fn front_door_narrow_action_buttons_use_icon_only_paint_without_losing_accesskit() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let out = render_front_door_accesskit_frame_with(
            &ctx,
            "browser",
            0,
            egui::vec2(220.0, 640.0),
            fixture_front_door_items(),
        );
        let nodes = accesskit_nodes(&out);
        let launch = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Launch Browser"))
            .expect("narrow app result should keep the primary action button accessible");
        let bounds = accesskit_bounds_rect(launch);

        assert!(
            bounds.width() < ACTION_BUTTON_TEXT_MIN_W,
            "test setup should exercise the cramped icon-only action width, got {bounds:?}"
        );
        assert_eq!(launch.role(), egui::accesskit::Role::Button);
        let expected_value = format!(
            "Primary action: App, {}",
            launcher_group_label(Surface::Browser)
        );
        assert_eq!(launch.value(), Some(expected_value.as_str()));
        assert!(launch.supports_action(egui::accesskit::Action::Click));

        let texts = painted_text(&out.shapes);
        assert!(
            !texts.iter().any(|(text, _)| text == "Launch"),
            "narrow Front Door action buttons should not paint clipped text labels: {texts:?}"
        );
    }

    #[test]
    fn front_door_results_accesskit_bounds_follow_the_scroll_cap() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let screen_size = egui::vec2(900.0, 480.0);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, screen_size);
        let expected_max = front_door_results_max_height(screen, false);
        let out = render_front_door_accesskit_frame_with(
            &ctx,
            "browser",
            MAX_HITS - 1,
            screen_size,
            fixture_many_front_door_items(),
        );
        let nodes = accesskit_nodes(&out);
        let results = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Shell search results"))
            .expect("front-door search results AccessKit status");
        let bounds = accesskit_bounds_rect(results);

        assert!(
            bounds.height() <= expected_max + 0.5,
            "status bounds should match the bounded visible result height, got {:?}",
            bounds
        );
        assert!(
            bounds.bottom() + Style::SP_L <= screen.bottom() + 0.5,
            "status bounds should stay inside the short viewport, got {:?}",
            bounds
        );
    }

    #[test]
    fn front_door_search_field_and_results_export_accesskit_state() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let out = render_front_door_accesskit_frame(&ctx, 1);
        let nodes = accesskit_nodes(&out);
        let search = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Shell search"))
            .expect("front-door search input AccessKit node");
        assert_eq!(search.role(), egui::accesskit::Role::SearchInput);
        assert_eq!(search.value(), Some("browser"));

        let results = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Shell search results"))
            .expect("front-door search results AccessKit status");
        assert_eq!(results.role(), egui::accesskit::Role::Status);
        assert_eq!(results.live(), Some(egui::accesskit::Live::Polite));
        let value = results.value().expect("front-door result summary value");
        assert!(value.contains("for browser"), "{value}");
        assert!(value.contains("highlighted"), "{value}");
    }

    #[test]
    fn front_door_search_field_uses_themed_hint_and_text() {
        let ctx = egui::Context::default();
        mde_egui::fonts::install(&ctx);

        let hint_out = render_front_door_settled_frame(
            &ctx,
            "",
            0,
            egui::vec2(900.0, 640.0),
            fixture_front_door_items(),
            FrontDoorFilter::All,
            false,
            FrontDoorSourceStatus::default(),
        );
        let hint_text = painted_text(&hint_out.shapes);
        assert!(
            hint_text
                .iter()
                .any(|(text, color)| text == SEARCH_HINT && *color == Style::TEXT_DIM),
            "Front Door search hint should paint themed dim text: {hint_text:?}"
        );
        assert!(
            !hint_text
                .iter()
                .any(|(text, color)| text == SEARCH_HINT && *color == egui::Color32::BLACK),
            "Front Door search hint leaked raw black text: {hint_text:?}"
        );

        let query_out = render_front_door_settled_frame(
            &ctx,
            "browser",
            0,
            egui::vec2(900.0, 640.0),
            fixture_front_door_items(),
            FrontDoorFilter::All,
            false,
            FrontDoorSourceStatus::default(),
        );
        let query_text = painted_text(&query_out.shapes);
        assert!(
            query_text
                .iter()
                .any(|(text, color)| text == "browser" && *color == Style::TEXT),
            "Front Door search query should paint themed primary text: {query_text:?}"
        );
    }

    #[test]
    fn blank_front_door_exports_shortcut_rows_and_live_status() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let out = render_front_door_accesskit_frame_with(
            &ctx,
            "",
            0,
            egui::vec2(900.0, 640.0),
            app_search_items_with_pins(&[Surface::Browser, Surface::Files]),
        );
        let nodes = accesskit_nodes(&out);
        let browser = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Browser"))
            .expect("blank Front Door should expose pinned Browser shortcut row");
        assert_eq!(browser.role(), egui::accesskit::Role::Button);
        assert!(
            browser
                .value()
                .is_some_and(|value| value.contains(launcher_group_label(Surface::Browser))),
            "blank shortcut row should expose the shared launcher category"
        );

        let results = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Shell search results"))
            .expect("blank Front Door search results status");
        let value = results.value().expect("blank Front Door status value");
        assert!(
            value.contains("local shortcut") && value.contains("Browser highlighted"),
            "blank Front Door should announce available shortcuts: {value}"
        );
    }

    #[test]
    fn front_door_filter_chips_export_accesskit_buttons() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let out = render_front_door_accesskit_frame_with_filter(
            &ctx,
            "",
            0,
            egui::vec2(900.0, 640.0),
            fixture_front_door_items(),
            FrontDoorFilter::Browser,
        );
        let nodes = accesskit_nodes(&out);
        for label in [
            "Filter: All",
            "Filter: Apps",
            "Filter: Mesh",
            "Filter: Workloads",
            "Filter: Services",
            "Filter: Files",
            "Filter: Browser",
            "Filter: Commands",
            "Filter: Web",
        ] {
            let chip = nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| node.label() == Some(label))
                .unwrap_or_else(|| panic!("missing Front Door filter chip {label}: {nodes:?}"));
            assert_eq!(chip.role(), egui::accesskit::Role::Button);
            assert!(
                chip.supports_action(egui::accesskit::Action::Click),
                "filter chips should be clickable AccessKit buttons"
            );
        }

        let browser_chip = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Filter: Browser"))
            .expect("Browser filter chip");
        assert_eq!(browser_chip.value(), Some("Selected"));
        assert_eq!(browser_chip.is_selected(), Some(true));
    }

    #[test]
    fn front_door_rendered_proof_covers_panel_expanded_and_degraded_states() {
        let ctx = egui::Context::default();
        Style::install(&ctx);

        let compact_size = egui::vec2(360.0, 640.0);
        let compact_out = render_front_door_settled_frame(
            &ctx,
            "browser",
            0,
            compact_size,
            fixture_many_front_door_items(),
            FrontDoorFilter::All,
            false,
            FrontDoorSourceStatus::default(),
        );
        assert_rects_inside_viewport(&compact_out, compact_size.x, "compact Front Door panel");
        let compact_canvas = capture_front_door_canvas(
            &ctx,
            "browser",
            0,
            compact_size,
            fixture_many_front_door_items(),
            FrontDoorFilter::All,
            false,
            FrontDoorSourceStatus::default(),
        );
        assert_eq!(
            (compact_canvas.width(), compact_canvas.height()),
            (360, 640),
            "the compact proof canvas should match the driven viewport"
        );
        assert!(
            !compact_canvas.is_blank(),
            "the compact Front Door render proof must paint real pixels"
        );
        let compact_fills = painted_fill_colors(&compact_out.shapes);
        assert_varied_fill_colors(&compact_fills, "compact Front Door panel");
        write_front_door_proof(&compact_canvas, "front-door-compact-panel.png");

        let expanded_size = egui::vec2(1200.0, 800.0);
        let expanded_out = render_front_door_settled_frame(
            &ctx,
            "",
            0,
            expanded_size,
            workflow_search_items(0),
            FrontDoorFilter::Workloads,
            true,
            FrontDoorSourceStatus::default(),
        );
        assert_rects_inside_viewport(&expanded_out, expanded_size.x, "expanded Front Door");
        let expanded_canvas = capture_front_door_canvas(
            &ctx,
            "",
            0,
            expanded_size,
            workflow_search_items(0),
            FrontDoorFilter::Workloads,
            true,
            FrontDoorSourceStatus::default(),
        );
        assert!(
            !expanded_canvas.is_blank(),
            "the expanded Front Door render proof must paint real pixels"
        );
        let expanded_fills = painted_fill_colors(&expanded_out.shapes);
        assert_varied_fill_colors(&expanded_fills, "expanded Front Door");
        write_front_door_proof(&expanded_canvas, "front-door-expanded-workflows.png");

        let degraded_size = egui::vec2(390.0, 640.0);
        let degraded_sources = FrontDoorSourceStatus::new(FrontDoorMeshSourceStatus::Unavailable);
        let degraded_out = render_front_door_settled_frame(
            &ctx,
            "browser",
            0,
            degraded_size,
            fixture_front_door_items(),
            FrontDoorFilter::Mesh,
            false,
            degraded_sources,
        );
        assert_rects_inside_viewport(&degraded_out, degraded_size.x, "degraded mesh Front Door");
        let degraded_fills = painted_fill_colors(&degraded_out.shapes);
        assert_varied_fill_colors(&degraded_fills, "degraded mesh Front Door");
        assert!(
            degraded_fills.iter().copied().any(looks_like_warning_fill),
            "the degraded mesh state should paint a warm warning row: {degraded_fills:?}"
        );
        let degraded_canvas = capture_front_door_canvas(
            &ctx,
            "browser",
            0,
            degraded_size,
            fixture_front_door_items(),
            FrontDoorFilter::Mesh,
            false,
            degraded_sources,
        );
        assert!(
            !degraded_canvas.is_blank(),
            "the degraded mesh Front Door render proof must paint real pixels"
        );
        write_front_door_proof(&degraded_canvas, "front-door-degraded-mesh.png");
    }

    #[test]
    fn front_door_expansion_control_exports_accesskit_state() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let panel_out = render_front_door_accesskit_frame_with_layout(
            &ctx,
            "",
            0,
            egui::vec2(900.0, 640.0),
            fixture_front_door_items(),
            FrontDoorFilter::All,
            false,
        );
        let panel_nodes = accesskit_nodes(&panel_out);
        let panel_toggle = panel_nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Front Door layout"))
            .expect("front-door panel layout toggle");
        assert_eq!(panel_toggle.role(), egui::accesskit::Role::Button);
        assert_eq!(panel_toggle.value(), Some("Panel"));
        assert!(
            panel_toggle.supports_action(egui::accesskit::Action::Click),
            "layout toggle should expose the click action"
        );
        assert_ne!(panel_toggle.is_selected(), Some(true));

        let expanded_out = render_front_door_accesskit_frame_with_layout(
            &ctx,
            "",
            0,
            egui::vec2(1200.0, 800.0),
            fixture_front_door_items(),
            FrontDoorFilter::All,
            true,
        );
        let expanded_nodes = accesskit_nodes(&expanded_out);
        let expanded_toggle = expanded_nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Front Door layout"))
            .expect("front-door expanded layout toggle");
        assert_eq!(expanded_toggle.value(), Some("Full-screen"));
        assert_eq!(expanded_toggle.is_selected(), Some(true));
    }

    #[test]
    fn front_door_expansion_tooltip_uses_themed_text() {
        let ctx = egui::Context::default();
        mde_egui::fonts::install(&ctx);

        let out = render_front_door_tooltip_frame(&ctx);
        let texts = painted_text(&out.shapes);
        assert!(
            texts
                .iter()
                .any(|(text, color)| text == "Expand Front Door" && *color == Style::TEXT),
            "Front Door expansion hover should paint themed tooltip text: {texts:?}"
        );
        assert!(
            !texts.iter().any(|(text, color)| {
                text == "Expand Front Door"
                    && matches!(*color, egui::Color32::BLACK | Style::BG | Style::TEXT_DIM)
            }),
            "Front Door expansion hover leaked an unreadable/shared shell text color: {texts:?}"
        );

        let fills = painted_fill_colors(&out.shapes);
        assert!(
            fills.contains(&Style::SURFACE),
            "Front Door tooltip should paint its own themed surface: {fills:?}"
        );
    }

    #[test]
    fn front_door_result_context_menu_rows_use_themed_text() {
        let ctx = egui::Context::default();
        mde_egui::fonts::install(&ctx);

        let out = render_front_door_context_row_frame(&ctx);
        let texts = painted_text(&out.shapes);
        for label in ["Launch Browser", "Pin to top"] {
            assert!(
                texts
                    .iter()
                    .any(|(text, color)| text == label && *color == Style::TEXT),
                "Front Door context row {label:?} should paint themed text: {texts:?}"
            );
            assert!(
                !texts
                    .iter()
                    .any(|(text, color)| text == label && *color == egui::Color32::BLACK),
                "Front Door context row {label:?} leaked raw black popup text: {texts:?}"
            );
        }
    }

    #[test]
    fn front_door_result_context_menu_visual_scope_uses_front_door_tokens() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut style = (*ctx.style()).clone();
        apply_front_door_context_style(&mut style);

        assert_eq!(style.visuals.window_fill, Style::SURFACE);
        assert_eq!(style.visuals.panel_fill, Style::SURFACE);
        assert_eq!(style.visuals.override_text_color, Some(Style::TEXT));
        assert_eq!(style.visuals.widgets.inactive.fg_stroke.color, Style::TEXT);
        assert_eq!(style.visuals.widgets.hovered.bg_fill, Style::SURFACE_HI);
        assert_eq!(style.visuals.widgets.open.bg_stroke.color, Style::BORDER);
    }

    #[test]
    fn front_door_result_rows_export_accesskit_buttons() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let expected_hits = ranked_front_door_hits("browser", fixture_front_door_items());
        assert!(
            expected_hits.len() >= 5,
            "fixture should cover app/file/mesh/browser/web results"
        );
        let out = render_front_door_accesskit_frame(&ctx, 1);
        let nodes = accesskit_nodes(&out);

        for hit in &expected_hits {
            let row = nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| node.label() == Some(hit.item.title.as_str()))
                .unwrap_or_else(|| {
                    panic!(
                        "missing front-door AccessKit result row {:?}: {nodes:?}",
                        hit.item.title
                    )
                });
            assert_eq!(row.role(), egui::accesskit::Role::Button);
            assert!(
                row.supports_action(egui::accesskit::Action::Click),
                "front-door result rows should expose the same click action as the painted row"
            );
            let value = row.value().expect("front-door row value");
            assert!(
                value.contains(domain_label(hit.item.domain)),
                "front-door row value should name the result domain: {value}"
            );
            assert!(
                value.contains(hit.item.target.as_str()),
                "front-door row value should expose the activation target: {value}"
            );
        }

        let selected = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.is_selected() == Some(true))
            .expect("selected front-door result row");
        assert!(
            selected
                .value()
                .is_some_and(|value| value.starts_with("Result 2 of ")),
            "selected row should match the keyboard highlight position"
        );
    }

    #[test]
    fn front_door_command_rows_export_command_accesskit_metadata() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let out = render_front_door_accesskit_frame_with_filter(
            &ctx,
            "logs",
            0,
            egui::vec2(900.0, 640.0),
            fixture_front_door_items_with_command(),
            FrontDoorFilter::Commands,
        );
        let nodes = accesskit_nodes(&out);
        let row = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Live Logs"))
            .expect("Console command row should be exposed in Front Door");
        assert_eq!(row.role(), egui::accesskit::Role::Button);
        assert!(
            row.value().is_some_and(|value| value.contains("Command")
                && value.contains("Follow the system journal live")),
            "command row value should name the Command domain and target: {:?}",
            row.value()
        );

        let action = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Open Live Logs"))
            .expect("selected Console command should expose a primary action");
        assert_eq!(action.role(), egui::accesskit::Role::Button);
        assert!(
            action
                .value()
                .is_some_and(|value| value.contains("Command")),
            "primary action should preserve the command domain: {:?}",
            action.value()
        );
    }

    #[test]
    fn front_door_selected_result_exports_primary_action_button() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let expected_hits = ranked_front_door_hits("browser", fixture_front_door_items());
        let selected_hit = &expected_hits[1];
        let out = render_front_door_accesskit_frame(&ctx, 1);
        let nodes = accesskit_nodes(&out);
        let label = format!(
            "{} {}",
            primary_action_label(selected_hit),
            selected_hit.item.title
        );
        let action = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some(label.as_str()))
            .unwrap_or_else(|| {
                panic!("missing selected Front Door primary action {label}: {nodes:?}")
            });

        assert_eq!(action.role(), egui::accesskit::Role::Button);
        assert!(
            action.supports_action(egui::accesskit::Action::Click),
            "the selected result's primary action must be the real clickable launch/open seam"
        );
        let value = action.value().expect("primary action value");
        assert!(
            value.contains(domain_label(selected_hit.item.domain))
                && value.contains(selected_hit.item.target.as_str()),
            "primary action value should preserve owner and target metadata: {value}"
        );
    }

    #[test]
    fn front_door_workflow_cards_export_real_workbench_quick_actions() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        // WL-ARCH-006 — the Cloud workloads card opens the standalone Workloads
        // surface (Infra as Code), not a Workbench plane, so it exports NO plane
        // quick action (honest omission, §7). Its primary open action still routes.
        let workload_out = render_front_door_accesskit_frame_with_filter(
            &ctx,
            "",
            0,
            egui::vec2(900.0, 640.0),
            workflow_search_items(0),
            FrontDoorFilter::Workloads,
        );
        let workload_nodes = accesskit_nodes(&workload_out);
        assert!(
            workload_nodes.iter().all(|(_, node)| node
                .label()
                .is_none_or(|label| !label.contains("plane for Cloud workloads"))),
            "the Cloud workloads card must expose no Workbench-plane quick action — it opens the Workloads surface"
        );

        let service_out = render_front_door_accesskit_frame_with_filter(
            &ctx,
            "",
            0,
            egui::vec2(900.0, 640.0),
            workflow_search_items(0),
            FrontDoorFilter::Services,
        );
        let service_nodes = accesskit_nodes(&service_out);
        let fleet = service_nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Open Fleet plane for Mesh services"))
            .expect("selected service card should expose the real Workbench Fleet plane");
        assert_eq!(fleet.role(), egui::accesskit::Role::Button);
        assert_eq!(
            fleet.value(),
            Some("Workflow action: Workbench Fleet plane; Fleet service health and controls")
        );
        assert!(fleet.supports_action(egui::accesskit::Action::Click));
    }

    #[test]
    fn front_door_mesh_peer_rows_export_desktop_connect_action() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let out = render_front_door_accesskit_frame_with(
            &ctx,
            "browser-node",
            0,
            egui::vec2(900.0, 640.0),
            fixture_front_door_items(),
        );
        let nodes = accesskit_nodes(&out);
        let open = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Open browser-node"))
            .expect("mesh peer should retain the normal Explorer Open action");
        assert_eq!(open.role(), egui::accesskit::Role::Button);

        let connect = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Connect desktop for browser-node"))
            .expect("selected mesh peer should expose Desktop Connect");
        assert_eq!(connect.role(), egui::accesskit::Role::Button);
        assert_eq!(
            connect.value(),
            Some("Desktop source: peer:browser-node; uses Desktop chooser path")
        );
        assert!(
            connect.supports_action(egui::accesskit::Action::Click),
            "Front Door peer Connect should be a real clickable action"
        );
    }

    #[test]
    fn front_door_selected_peer_app_reports_owning_node_for_lazy_load_context() {
        let mut state = FrontDoorState::default();
        state.open();
        let items = peer_app_search_items(
            [FrontDoorPeerApp {
                id: "org.mozilla.Firefox.desktop".to_owned(),
                name: "Firefox".to_owned(),
                node: "oak".to_owned(),
                source: "flatpak".to_owned(),
                icon: "firefox".to_owned(),
                health: "online".to_owned(),
                state: "installed".to_owned(),
            }],
            0,
        );

        assert_eq!(
            state.selected_peer_node(items, FrontDoorSourceStatus::default()),
            Some("oak".to_owned())
        );
    }

    #[test]
    fn front_door_peer_app_primary_action_uses_app_launch_and_keeps_desktop_connect() {
        let items = peer_app_search_items(
            [FrontDoorPeerApp {
                id: "org.mozilla.Firefox.desktop".to_owned(),
                name: "Firefox".to_owned(),
                node: "oak".to_owned(),
                source: "flatpak".to_owned(),
                icon: "firefox".to_owned(),
                health: "online".to_owned(),
                state: "installed".to_owned(),
            }],
            0,
        );
        let hit = ranked_front_door_hits("firefox", items.clone())
            .into_iter()
            .next()
            .expect("peer app hit");
        let FrontDoorTarget::PeerApp(target) = &hit.item.payload else {
            panic!("fixture should produce a peer app target");
        };

        assert_eq!(primary_action_label(&hit), "Launch");
        assert_eq!(
            activation_request_for_hit(&hit),
            FrontDoorRequest::LaunchPeerApp(target.clone())
        );
        assert_eq!(desktop_source_id_for_hit(&hit), Some("peer:oak".to_owned()));
        let (topic, body) = peer_app_launch_wire(target).expect("peer app launch wire");
        assert_eq!(topic, "action/apps/launch");
        let body: serde_json::Value = serde_json::from_str(&body).expect("launch body json");
        assert_eq!(body["node"], "oak");
        assert_eq!(body["app_id"], "org.mozilla.Firefox.desktop");
        assert_eq!(body["name"], "Firefox");

        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);
        let out = render_front_door_accesskit_frame_with(
            &ctx,
            "firefox",
            0,
            egui::vec2(900.0, 640.0),
            items,
        );
        let nodes = accesskit_nodes(&out);
        let launch = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Launch Firefox"))
            .expect("selected peer app should expose a Launch primary action");
        assert_eq!(launch.role(), egui::accesskit::Role::Button);
        assert!(launch.supports_action(egui::accesskit::Action::Click));

        let connect = nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Connect desktop for Firefox"))
            .expect("selected peer app should still expose Desktop Connect");
        assert_eq!(connect.role(), egui::accesskit::Role::Button);
        assert_eq!(
            connect.value(),
            Some("Desktop source: peer:oak; uses Desktop chooser path")
        );
    }

    #[test]
    fn front_door_mesh_instance_rows_export_lifecycle_actions() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let out = render_front_door_accesskit_frame_with(
            &ctx,
            "web",
            0,
            egui::vec2(900.0, 640.0),
            vec![fixture_cloud_instance_item()],
        );
        let nodes = accesskit_nodes(&out);

        for (label, topic) in [
            ("Start instance web", "action/cloud/instance-start"),
            ("Stop instance web", "action/cloud/instance-stop"),
            ("Reboot instance web", "action/cloud/instance-reboot"),
        ] {
            let action = nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| node.label() == Some(label))
                .unwrap_or_else(|| panic!("missing {label} lifecycle action: {nodes:?}"));
            let expected = format!("Cloud lifecycle: {topic}; instance i-9");
            assert_eq!(action.role(), egui::accesskit::Role::Button);
            assert_eq!(action.value(), Some(expected.as_str()));
            assert!(
                action.supports_action(egui::accesskit::Action::Click),
                "Front Door cloud instance lifecycle controls must be clickable"
            );
        }
    }

    #[test]
    fn front_door_service_rows_export_real_lifecycle_actions() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let out = render_front_door_accesskit_frame_with(
            &ctx,
            "mesh-api",
            0,
            egui::vec2(900.0, 640.0),
            vec![fixture_service_lifecycle_item(
                "mesh-api",
                FrontDoorLifecycleKind::Container,
                "running",
            )],
        );
        let nodes = accesskit_nodes(&out);

        for label in [
            "Stop service mesh-api container",
            "Restart service mesh-api container",
        ] {
            let action = nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| node.label() == Some(label))
                .unwrap_or_else(|| panic!("missing {label} lifecycle action: {nodes:?}"));
            let value = action.value().unwrap_or_default();
            assert_eq!(action.role(), egui::accesskit::Role::Button);
            assert!(value.contains("Service lifecycle: action/services/lifecycle"));
            assert!(value.contains(r#""peer":"oak""#), "{value}");
            assert!(value.contains(r#""kind":"container""#), "{value}");
            assert!(value.contains(r#""name":"mesh-api""#), "{value}");
            assert!(
                action.supports_action(egui::accesskit::Action::Click),
                "Front Door service lifecycle controls must be clickable"
            );
        }
    }

    #[test]
    fn front_door_selected_app_result_exports_pin_action_button() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        mde_egui::fonts::install(&ctx);

        let unpinned_out = render_front_door_accesskit_frame_with(
            &ctx,
            "browser",
            0,
            egui::vec2(900.0, 640.0),
            fixture_front_door_items(),
        );
        let unpinned_nodes = accesskit_nodes(&unpinned_out);
        let pin = unpinned_nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Pin Browser"))
            .expect("selected app result should expose Pin action");
        assert_eq!(pin.role(), egui::accesskit::Role::Button);
        assert_eq!(pin.value(), Some("Favorite action: Browser, Not pinned"));
        assert!(pin.supports_action(egui::accesskit::Action::Click));

        let pinned_out = render_front_door_accesskit_frame_with_layout_and_pins(
            &ctx,
            "browser",
            0,
            egui::vec2(900.0, 640.0),
            fixture_front_door_items(),
            FrontDoorFilter::All,
            false,
            &[Surface::Browser],
        );
        let pinned_nodes = accesskit_nodes(&pinned_out);
        let unpin = pinned_nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Unpin Browser"))
            .expect("selected pinned app result should expose Unpin action");
        assert_eq!(unpin.role(), egui::accesskit::Role::Button);
        assert_eq!(unpin.value(), Some("Favorite action: Browser, Pinned"));
        assert_eq!(unpin.is_selected(), Some(true));

        let ordered_pins = [Surface::Browser, Surface::Files];
        let reorder_out = render_front_door_accesskit_frame_with_layout_and_pins(
            &ctx,
            "",
            1,
            egui::vec2(900.0, 640.0),
            app_search_items_with_pins(&ordered_pins),
            FrontDoorFilter::All,
            false,
            &ordered_pins,
        );
        let reorder_nodes = accesskit_nodes(&reorder_out);
        let move_up = reorder_nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some("Move Files up"))
            .expect("selected pinned app result should expose Move up action");
        assert_eq!(move_up.role(), egui::accesskit::Role::Button);
        assert_eq!(move_up.value(), Some("Favorite order: 2 of 2, Files"));
        assert!(move_up.supports_action(egui::accesskit::Action::Click));
    }
}
