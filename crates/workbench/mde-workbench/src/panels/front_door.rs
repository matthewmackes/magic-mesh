//! FRONTDOOR-1/2/3 — the "Front Door" home: a Win10-Start two-pane shell (panel
//! mode) and an iPadOS-home full-screen mode, both wrapping a GPU `canvas` tile
//! grid.
//!
//! FRONTDOOR-1 (the de-risk track, `docs/design/front-door.md`) replaced the old
//! deep-widget-tree home (the "4-second menu") with a tile grid drawn as flat GPU
//! 2D geometry on `cosmic::iced::widget::canvas` — the same lighter render path
//! Routing's path-graph and the Peers map use — so it paints immediately, with
//! skeleton placeholders while real data streams in.
//!
//! FRONTDOOR-2 (the panel layer) wraps that grid in the locked **Win10 Start**
//! shell (design Q1/Q5/Q98): a fixed left **rail** (identity · pinned · the
//! predominant DevOps + Data Center entries) and a right **pane** (a full-width
//! omnibox above the FRONTDOOR-1 tile grid). The rail's DevOps / Data Center
//! entries navigate to the real `build-farm` / `datacenter` panel routes (§7 — no
//! dead buttons); the omnibox renders + tracks its text locally but does NOT
//! search yet (that's FRONTDOOR-6). Carbon chrome: follow-OS theme, Blue 60
//! accent, comfortable density — all via `mde-theme` tokens, never raw hex (§4).
//!
//! FRONTDOOR-3 (this layer) adds the locked **iPadOS home** full-screen mode
//! (design Q86/Q89: a rounded-icon grid + widgets, **no dock**). A real toggle in
//! the top bar flips [`FrontDoor::mode`] between [`Mode::Panel`] (the FD-2 two-pane
//! shell, rail visible) and [`Mode::FullScreen`] (rail hidden; the same
//! [`TileGrid`] reused with full-screen layout params — larger rounded icons, more
//! columns — under a full-width omnibox). The full-screen render is a single
//! scrollable grid rather than true horizontal paging: paging is the design ideal,
//! but a scrollable grid is the accepted first cut (the rounded-icon aesthetic is
//! the required part), and it avoids a heavy custom pager on the canvas path.
//!
//! FRONTDOOR-4 (this layer) makes the **widget** tiles live: each one carries a
//! [`TileKey`] naming the design widget (Q99: mesh map, build/farm, alerts, node
//! health, Copilot, system, plus the DevOps + Data Center surfaces) and an
//! optional live `value` line drawn under the label. The values come from the
//! **existing** mde-bus data paths the other panels already read (§6 glue — no
//! new mackesd publisher, no `demo_data`): the Peers directory, the Build Farm
//! events, the Datacenter health checks, the boot-readiness snapshot. The Front
//! Door subscribes the **same way** the other panels do — a [`FrontDoor::load`]
//! Task fired on nav + reconnect, the Peers directory-changed Bus event for the
//! push half, and a slow-poll fallback ([`poll_subscription`]) for the topics
//! that aren't purely push (§7 — real data, event-driven + slow-poll fallback).
//! Until the first snapshot lands the FD-1 `loading` skeleton shows (Q92 — no
//! layout shift). The one widget with **no** existing workbench-readable source —
//! **Copilot** (FRONTDOOR-12 landed the backend, but no bus topic the workbench
//! can read yet) — stays a plain launcher with no faked value, noted as needing a
//! publisher (a follow-up), per §7 (better an honest launcher than a fake metric).
//!
//! FRONTDOOR-6 (this layer) makes the omnibox a real **unified search** (design
//! Q20/Q47/Q81). A non-empty query swaps the tile grid (in BOTH modes) for a
//! results surface: instant LOCAL hits first — matching app launchers + routable
//! panels, and live mesh entities (nodes + the services they publish, off the
//! same FD-4 Peers roster) — ranked best-first by an exact/prefix/word/substring
//! relevance ladder (the pure [`search`] engine). Below them, a distinct **AI
//! answer card**: the query is published to FD-9's `action/copilot/ask` topic and
//! the reply rendered when it lands ("Thinking…" until then), degrading quietly
//! to an "unavailable" note when Copilot can't answer (Q33 — no error spew). A
//! blank query restores the tile grid unchanged (FD-1..5 intact). Every local hit
//! activates a REAL app message (a panel route or app launch — §7, no demo data).
//! FILES search is a follow-up: there is no trivial existing filename source the
//! workbench reads, so faking it would violate §7 — it's deferred, not stubbed.
//!
//! SCOPE held to FRONTDOOR-1..6:
//! - `draw` only — tile click → detail view is FRONTDOOR-5, so the canvas keeps
//!   `type State = ()` and the default `update` / `mouse_interaction`.
//! - No wallpaper backdrop here.

use std::time::Duration;

use cosmic::iced::widget::canvas::{self, Frame, Path, Text};
use cosmic::iced::widget::text::Alignment;
use cosmic::iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
use cosmic::iced::{
    mouse, Background, Border, Element, Length, Padding, Pixels, Point, Rectangle, Size,
    Subscription, Task,
};
use cosmic::Theme;
use mde_theme::{FontSize, Palette, TypeRole};

use crate::cosmic_compat::prelude::*;
use crate::model::Group;
use crate::panels::{build_farm, datacenter, health_check, home, jobs, node_roles, peers};

/// FRONTDOOR-2/3 — the Front Door's own message set, threaded through
/// [`crate::Message::FrontDoor`]. Each variant is one we actually handle (§7):
/// the omnibox text-change and the panel ↔ full-screen toggle. Rail navigation
/// reuses the app-level [`crate::Message::SelectPanel`] directly (it drives the
/// real router), so it needs no variant here.
#[derive(Debug, Clone)]
pub enum Message {
    /// The omnibox text changed. FRONTDOOR-2 recorded it into local state;
    /// FRONTDOOR-6 makes it drive the real unified search — the instant LOCAL
    /// results (apps + mesh entities) are recomputed synchronously here, and a
    /// non-empty query also kicks off the async Copilot `ask` (the AI card).
    OmniboxChanged(String),
    /// FRONTDOOR-6 — a Copilot `ask` reply landed (or the publish/wait failed).
    /// Carries the generation the ask was fired under so a stale reply for a
    /// superseded query is dropped, plus the parsed [`CopilotAnswer`]. Boxed-free
    /// (the payload is small) — the generation guards ordering, not size.
    CopilotReplied(u64, CopilotAnswer),
    /// FRONTDOOR-6 — a local search hit was activated (clicked): fire its real
    /// app message (a panel route or an app launch) and clear the omnibox so the
    /// Front Door returns to the tile grid. The carried message is always one
    /// `App::update` already handles (§7 — no inert results).
    SearchHitActivated(Box<crate::Message>),
    /// FRONTDOOR-3 — the top-bar toggle was pressed: flip [`FrontDoor::mode`]
    /// between the Win10 panel and the iPadOS full-screen home.
    ToggleMode,
    /// FRONTDOOR-4 — the slow-poll tick (or a reconnect) asks for a fresh read.
    /// The handler returns [`FrontDoor::load`] so the actual Bus read happens
    /// off-thread; the result comes back as [`Message::Loaded`].
    Reload,
    /// FRONTDOOR-4 — a fresh widget-data snapshot read off the existing mde-bus
    /// data paths (the Peers directory + Build Farm + Datacenter health + boot
    /// readiness). Folded into the widget tiles' live `value` lines, and clears
    /// the [`FrontDoor::loading`] skeleton on the first arrival. Boxed so the
    /// enum stays small (clippy `large_enum_variant`).
    Loaded(Box<FrontDoorData>),
    /// FRONTDOOR-5 — a tile was left-clicked on the canvas (Q45). Carries the
    /// clicked tile's index into [`FrontDoor::tiles`]; the handler opens that
    /// tile's detail **actions menu** (Q49) over the grid.
    TileActivated(usize),
    /// FRONTDOOR-5 — the detail actions-menu's back/close control was pressed;
    /// return to the tile grid.
    CloseDetail,
    /// FRONTDOOR-7 — a one-click pipeline action from a DevOps / Build-Farm tile's
    /// detail menu fired. It carries the existing-panel navigation it routes to
    /// (`nav` — a real [`crate::Message::SelectPanel`]) and, for the **wired**
    /// actions, the panel's OWN trigger sub-message (`trigger` — e.g. the
    /// `BuildFarm`/`Jobs` `RefreshClicked` the build-farm/jobs panel already
    /// emits). Both fire in one click (`Task::batch`): the operator lands on the
    /// surface AND its real work runs (§7 — a real action, not a stub; §9 — the
    /// trigger is the existing typed verb path, never a raw shell). A
    /// navigate-only action carries `trigger == None` (the surface still refreshes
    /// via its own `on_panel_navigated` load — also a real action).
    PipelineAction {
        nav: Box<crate::Message>,
        trigger: Option<Box<crate::Message>>,
    },
    /// FRONTDOOR-10 — the operator clicked **Act** on a proactive suggestion that
    /// carries a typed proposal. Carries the suggestion's index into
    /// [`FrontDoor::suggestions`]. The handler re-publishes that suggestion's typed
    /// proposal to the copilot PROPOSE topic (`action/copilot/proposal`, FD-12's
    /// review queue) — it does NOT publish to FD-11's execution topic and does NOT
    /// execute anything (§9 — the GUI never auto-executes; the gated confirm/exec
    /// surface that drains the propose queue is a separate, out-of-scope UI).
    ProposeSuggestion(usize),
}

/// FRONTDOOR-3 — which of the two locked render modes the Front Door is in
/// (design Q29: panel default + a full-screen toggle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// The FRONTDOOR-2 **Win10 Start** two-pane shell: left rail + right pane
    /// (omnibox above the tile grid). The default summon form (Q29).
    #[default]
    Panel,
    /// The FRONTDOOR-3 **iPadOS home**: rail hidden, a full-screen rounded-icon
    /// grid + widgets, **no dock** (Q86/Q89).
    FullScreen,
}

/// The fixed width of the left rail (design Q5 — a Win10-Start identity/pinned/
/// surfaces column). Comfortable-density Start rails sit around this width.
const RAIL_WIDTH: f32 = 260.0;

/// FRONTDOOR-6 — how long the omnibox waits for the Copilot `ask` reply before
/// degrading to "unavailable". Set a touch above FD-9's per-request codex ceiling
/// ([`DEFAULT_CODEX_TIMEOUT`] = 120 s in the worker) so the worker's own graceful
/// "AI unavailable" reply (a timeout / absent codex / unsealed key) wins the race
/// and surfaces its reason path, rather than this client timing out first. A
/// no-worker / no-Bus environment still degrades quietly — `action_request_with_body`
/// returns `None` immediately when there's no Bus data-dir, so this ceiling only
/// bounds the genuinely-waiting-on-a-leader case.
const COPILOT_ASK_TIMEOUT: Duration = Duration::from_secs(130);

/// The Carbon token a tile's accent strip + label color reads from. Picked per
/// tile kind so DevOps/Data-Center/alert tiles read distinctly against the
/// background without any raw color. FRONTDOOR-4 will swap these for live status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileTone {
    /// The single interactive accent — mesh / Copilot / launchers.
    Accent,
    /// Healthy / system-nominal.
    Success,
    /// Pending / at-risk (build farm, node health caution).
    Warning,
    /// Errors / alerts.
    Danger,
    /// Neutral informational (system, generic app launchers).
    Neutral,
}

impl TileTone {
    /// Resolve this tone to its live Carbon color (§4 — token, never hex).
    fn color(self, p: &Palette) -> cosmic::iced::Color {
        match self {
            TileTone::Accent => p.accent.into_cosmic_color(),
            TileTone::Success => p.success.into_cosmic_color(),
            TileTone::Warning => p.warning.into_cosmic_color(),
            TileTone::Danger => p.danger.into_cosmic_color(),
            TileTone::Neutral => p.text_muted.into_cosmic_color(),
        }
    }
}

/// FRONTDOOR-4 — which design widget a tile *is* (Q99), so a fresh
/// [`FrontDoorData`] snapshot can target the right tile's live value + tone
/// without matching on its display label. A plain app launcher carries no key
/// (`Tile::key == None`) and never takes live data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileKey {
    /// Mesh map — online/total peer presence (Peers directory).
    MeshMap,
    /// Build / Farm — the latest farm + nightly-tier verdict (Build Farm events).
    BuildFarm,
    /// Alerts — non-ok datacenter health checks (`event/dc/health/*`).
    Alerts,
    /// Node health — healthy/total node count (Peers directory `health`).
    NodeHealth,
    /// Copilot — NO workbench-readable source yet (FRONTDOOR-12 backend only);
    /// stays a launcher, noted as needing a publisher.
    Copilot,
    /// System — boot-readiness + the running workbench version.
    System,
    /// Data Center — total datacenter-plane node count (Peers directory).
    DataCenter,
    /// DevOps — in-flight farm jobs (Build Farm events).
    DevOps,
}

/// One tile in the grid. A **widget** tile carries a [`TileKey`] and (once a
/// snapshot lands) a live `value` line under its label; a plain launcher has
/// `key == None` and never shows a value. FRONTDOOR-4 fills `value`/`tone` from
/// the existing mde-bus data paths — no static placeholder metric (§7).
#[derive(Debug, Clone)]
pub struct Tile {
    /// The card's short label, drawn centered on the tile.
    pub label: String,
    /// Which Carbon token the accent strip + label read.
    pub tone: TileTone,
    /// FRONTDOOR-4 — the widget this tile is (drives which live metric it takes),
    /// or `None` for a plain app launcher.
    pub key: Option<TileKey>,
    /// FRONTDOOR-4 — the live metric line drawn under the label (e.g.
    /// "5/5 healthy", "build: green", "2 alerts"). `None` until the first
    /// snapshot, or for a launcher / a widget whose source is still absent.
    pub value: Option<String>,
}

impl Tile {
    /// A plain app launcher: no live data, no key.
    fn launcher(label: &str, tone: TileTone) -> Self {
        Self {
            label: label.to_string(),
            tone,
            key: None,
            value: None,
        }
    }

    /// A live widget tile, identified by its [`TileKey`]. Starts with no `value`
    /// (the FD-1 skeleton covers the gap until the first snapshot) and a neutral
    /// resting tone; the snapshot supplies both.
    fn widget(label: &str, key: TileKey, tone: TileTone) -> Self {
        Self {
            label: label.to_string(),
            tone,
            key: Some(key),
            value: None,
        }
    }

    /// FRONTDOOR-5 — the **real** actions for this tile's detail menu (Q49), in
    /// menu order. Every entry navigates to an EXISTING panel via the app router
    /// ([`crate::Message::SelectPanel`]) or launches a real installed app
    /// ([`crate::Message::LaunchApp`]) — no stubs, no `todo!` (§7). The rich
    /// 1-click DevOps / Data-Center action *sets* are FRONTDOOR-7/8; here the
    /// detail simply routes to the surface that already owns those actions.
    ///
    /// A tile with no workbench-readable destination yet (Copilot — backend only,
    /// no route/app the workbench can open) returns an empty list: the detail
    /// still renders its live data + the always-real Back control, rather than a
    /// dead button.
    fn actions(&self) -> Vec<TileAction> {
        // Widget tiles route by their key; launchers route by their label.
        match self.key {
            Some(TileKey::MeshMap) => vec![
                TileAction::nav("Open Peers map", Group::Mesh, "peers"),
                TileAction::nav("Open Routing", Group::Mesh, "routing"),
            ],
            Some(TileKey::NodeHealth) => vec![
                TileAction::nav("Open Peers", Group::Mesh, "peers"),
                TileAction::nav("Open Fleet Roster", Group::Fleet, "inventory"),
            ],
            // FRONTDOOR-8 — the Data Center tile gets the locked Q51 node-lifecycle
            // one-click set (Q12 DC lead: 1-click node actions): join · drain ·
            // restart-service · view-health · cutover-helpers · provision · destroy.
            // Mirrors FD-7's wired-vs-navigate split (§6 glue, §9 typed — no raw
            // shell). WIRED where an existing parameterless verb exists: "View
            // health" re-probes via the Health panel's own `RunClicked`; "Drain"
            // lands on the Node Roles surface freshly re-read via its
            // `RefreshClicked` (the per-node role/drain edits live there). The
            // node/service/leader-scoped verbs whose action needs a target the
            // operator picks on the surface (join needs a passcode; restart needs a
            // unit+scope; cutover is a mutating leadership change we never fire blind
            // from a tile) NAVIGATE to the surface that owns them — also a real
            // action (the destination's `load()` fires). PROVISION + DESTROY are
            // navigate-ONLY to the Datacenter Tofu surface (Q54 — the
            // operator-gated apply/destroy with the prod-arm + typed-confirm lives
            // there); a destructive op is never triggered from a tile click.
            Some(TileKey::DataCenter) => vec![
                TileAction::pipeline_nav("Join node", Group::MeshProvisioning, "mesh_join"),
                TileAction::pipeline_wired(
                    "Drain node",
                    Group::Provisioning,
                    "node_roles",
                    crate::Message::NodeRoles(node_roles::Message::RefreshClicked),
                ),
                TileAction::pipeline_nav("Restart service", Group::ThisNode, "mesh_services"),
                TileAction::pipeline_wired(
                    "View health",
                    Group::Monitoring,
                    "health_check",
                    crate::Message::HealthCheck(health_check::Message::RunClicked),
                ),
                TileAction::pipeline_nav("Cutover helpers", Group::Mesh, "mesh_control"),
                // Provision / destroy — navigate-ONLY to the tofu/autoscaler surface
                // (the operator-gated apply); never a destructive op from a click.
                TileAction::pipeline_nav("Provision", Group::Provisioning, "datacenter"),
                TileAction::pipeline_nav("Destroy", Group::Provisioning, "datacenter"),
            ],
            // FRONTDOOR-7 — the Build/Farm + DevOps tiles get the locked one-click
            // pipeline action set (Q50d: build · deploy · rollback · view-logs ·
            // rerun-failed), each routed to the EXISTING build-farm / job infra
            // (§6 glue, §9 typed — no raw shell). "Build" is WIRED: it re-polls the
            // farm verdict off the Bus via the build-farm panel's OWN
            // `RefreshClicked` while routing there (a real trigger, not a stub).
            // The named verbs with no parameterless one-shot today (deploy /
            // rollback / view-logs / rerun-failed) navigate to the surface that
            // owns them — also a real action (the destination's `load()` fires).
            Some(TileKey::BuildFarm) => vec![
                TileAction::pipeline_wired(
                    "Build — refresh farm",
                    Group::Provisioning,
                    "build-farm",
                    crate::Message::BuildFarm(build_farm::Message::RefreshClicked),
                ),
                TileAction::pipeline_nav("Deploy", Group::Provisioning, "build-farm"),
                TileAction::pipeline_nav("Rollback", Group::System, "revisions"),
                TileAction::pipeline_nav("View logs", Group::Monitoring, "run_history"),
                TileAction::pipeline_wired(
                    "Rerun failed",
                    Group::Fleet,
                    "jobs",
                    crate::Message::Jobs(jobs::Message::RefreshClicked),
                ),
            ],
            // The DevOps tile fronts the everyday CI loop (Q11 DevOps lead): the
            // same locked pipeline action set, jobs-first (it tracks in-flight
            // farm jobs). "Rerun failed" is WIRED to the jobs panel's own
            // `RefreshClicked` re-read; the rest mirror the Build/Farm tile.
            Some(TileKey::DevOps) => vec![
                TileAction::pipeline_wired(
                    "Build — refresh farm",
                    Group::Provisioning,
                    "build-farm",
                    crate::Message::BuildFarm(build_farm::Message::RefreshClicked),
                ),
                TileAction::pipeline_nav("Deploy", Group::Provisioning, "build-farm"),
                TileAction::pipeline_nav("Rollback", Group::System, "revisions"),
                TileAction::pipeline_nav("View logs", Group::Monitoring, "run_history"),
                TileAction::pipeline_wired(
                    "Rerun failed",
                    Group::Fleet,
                    "jobs",
                    crate::Message::Jobs(jobs::Message::RefreshClicked),
                ),
            ],
            Some(TileKey::Alerts) => vec![
                TileAction::nav("Open Datacenter", Group::Provisioning, "datacenter"),
                TileAction::nav("Open Health", Group::Monitoring, "health_check"),
            ],
            Some(TileKey::System) => vec![
                TileAction::nav("Open Health", Group::Monitoring, "health_check"),
                TileAction::launch("Open Settings", "cosmic-settings"),
            ],
            // FRONTDOOR-10 — Copilot is a live widget now (status off
            // `state/copilot/status`), but it still has no NAV/launch surface the
            // workbench owns, so its nav-action list stays honestly empty. Its
            // detail view doesn't render a dead button: it renders the live status
            // + the proactive suggestion cards (each with a §9-safe "Act" that
            // re-publishes the proposal — never executes), built in `detail_view`.
            Some(TileKey::Copilot) => Vec::new(),
            // Plain app launchers route to their real installed app / in-app
            // surface. Each binary/route is one the workbench already opens
            // elsewhere (notify-center / launcher / the Music panel). Copilot
            // has no openable surface yet → honest empty list (§7 — needs a
            // publisher), still rendering its data + the Back control.
            None => match self.label.as_str() {
                "Files" => vec![TileAction::launch("Open Files", "mde-files")],
                "Terminal" => vec![TileAction::launch("Open Terminal", "cosmic-term")],
                "Settings" => vec![TileAction::launch("Open Settings", "cosmic-settings")],
                "Music" => vec![TileAction::nav("Open Music", Group::Mesh, "music")],
                // Copilot + any unrecognized launcher carry no faked action (§7).
                _ => Vec::new(),
            },
        }
    }
}

/// FRONTDOOR-5 — one entry in a tile's detail **actions menu** (Q49). Each is a
/// real, reachable action (§7): it carries the [`crate::Message`] the app already
/// handles — a router navigation ([`TileAction::nav`]) to an existing panel, or a
/// detached app launch ([`TileAction::launch`]). There is no inert variant.
#[derive(Debug, Clone)]
pub struct TileAction {
    /// The menu row's label.
    pub label: String,
    /// The app message this action fires — always one `App::update` handles.
    pub message: crate::Message,
}

impl TileAction {
    /// A navigation to an EXISTING panel route, via the app router
    /// ([`crate::Message::SelectPanel`]). `panel` must be a live panel slug.
    fn nav(label: &str, group: Group, panel: &'static str) -> Self {
        Self {
            label: label.to_string(),
            message: crate::Message::SelectPanel { group, panel },
        }
    }

    /// A detached launch of a real installed app ([`crate::Message::LaunchApp`]),
    /// the same spawn path the rail / notify-center launchers use.
    fn launch(label: &str, bin: &'static str) -> Self {
        Self {
            label: label.to_string(),
            message: crate::Message::LaunchApp(bin),
        }
    }

    /// FRONTDOOR-7 — a **navigate-only** pipeline action: a one-click action whose
    /// named verb has NO existing one-shot trigger (build / deploy / rollback /
    /// view-logs are not parameterless verbs the workbench owns), so it routes to
    /// the EXISTING panel that owns that capability. The nav is still a real action
    /// — `on_panel_navigated` fires the destination's own `load()` (a real Bus /
    /// CLI read) — never a stub (§7). Wrapped in [`Message::PipelineAction`] with
    /// no trigger so the detail menu is dismissed as it routes.
    fn pipeline_nav(label: &str, group: Group, panel: &'static str) -> Self {
        Self {
            label: label.to_string(),
            message: crate::Message::FrontDoor(Message::PipelineAction {
                nav: Box::new(crate::Message::SelectPanel { group, panel }),
                trigger: None,
            }),
        }
    }

    /// FRONTDOOR-7 — a **wired** pipeline action: routes to the existing surface
    /// AND fires that panel's OWN trigger sub-message (`trigger`) in the same
    /// click. Used where a real one-shot trigger exists — the build-farm / jobs
    /// panels' `RefreshClicked`, which re-polls the farm verdict / job store off
    /// the Bus (§9 — the existing typed verb, never a raw shell). The operator
    /// lands on the surface with its data freshly re-read.
    fn pipeline_wired(
        label: &str,
        group: Group,
        panel: &'static str,
        trigger: crate::Message,
    ) -> Self {
        Self {
            label: label.to_string(),
            message: crate::Message::FrontDoor(Message::PipelineAction {
                nav: Box::new(crate::Message::SelectPanel { group, panel }),
                trigger: Some(Box::new(trigger)),
            }),
        }
    }
}

/// FRONTDOOR-4 — one live-data snapshot for the widget tiles, read off the
/// **existing** mde-bus data paths in [`FrontDoor::load`]. Each field is a
/// pre-rendered `(value, tone)` for the matching [`TileKey`], or `None` when that
/// source had nothing to show (the tile keeps its skeleton / resting state rather
/// than display a fake metric). Built by [`FrontDoorData::read`] off-thread.
#[derive(Debug, Clone, Default)]
pub struct FrontDoorData {
    /// Mesh map — online/total presence.
    pub mesh_map: Option<(String, TileTone)>,
    /// Build / Farm — latest verdict.
    pub build_farm: Option<(String, TileTone)>,
    /// Alerts — non-ok datacenter health checks.
    pub alerts: Option<(String, TileTone)>,
    /// Node health — healthy/total.
    pub node_health: Option<(String, TileTone)>,
    /// System — boot readiness + version.
    pub system: Option<(String, TileTone)>,
    /// Data Center — total node count.
    pub data_center: Option<(String, TileTone)>,
    /// DevOps — in-flight farm jobs.
    pub dev_ops: Option<(String, TileTone)>,
    /// FRONTDOOR-10 — the Copilot tile's live value, read off the FD-10-backend
    /// `state/copilot/status` topic (ready/thinking/offline). `None` until the
    /// status topic has a body (the tile keeps its skeleton — never a fake value).
    /// Closes FD-4's honest gap: the tile was a launcher because NO topic existed.
    pub copilot: Option<(String, TileTone)>,
    /// FRONTDOOR-10 — the ranked proactive suggestions read off the FD-10-backend
    /// `action/copilot/suggestions` topic. Each is a PROPOSAL the operator can act
    /// on (re-published to the propose topic), NEVER executed from the GUI (§9).
    /// Empty when the topic is absent / the mesh is quiet (Q61).
    pub suggestions: Vec<copilot::Suggestion>,
    /// FRONTDOOR-6 — the raw Peers directory rows, carried through so the unified
    /// search has the live mesh entities (nodes + services) without a second Bus
    /// read. The widget projections above are derived from these same rows.
    pub peers: Vec<peers::PeerRow>,
}

impl FrontDoorData {
    /// The pre-rendered `(value, tone)` for one widget key, or `None` when this
    /// snapshot has nothing for it (incl. [`TileKey::Copilot`], which has no
    /// workbench source yet). Pure — the projection lives in [`Self::read`].
    #[must_use]
    pub fn for_key(&self, key: TileKey) -> Option<(String, TileTone)> {
        match key {
            TileKey::MeshMap => self.mesh_map.clone(),
            TileKey::BuildFarm => self.build_farm.clone(),
            TileKey::Alerts => self.alerts.clone(),
            TileKey::NodeHealth => self.node_health.clone(),
            TileKey::System => self.system.clone(),
            TileKey::DataCenter => self.data_center.clone(),
            TileKey::DevOps => self.dev_ops.clone(),
            // FRONTDOOR-10 — now a live widget: the `state/copilot/status` snapshot
            // (ready/thinking/offline). `None` until the topic has a body (the FD-1
            // skeleton covers the gap). Closes FD-4's launcher-only gap.
            TileKey::Copilot => self.copilot.clone(),
        }
    }

    /// Read every widget's source off the Bus and project it (blocking — runs on
    /// a `spawn_blocking` thread, like the other panels' loaders). Best-effort:
    /// a missing Bus / empty topic simply leaves that field `None` (the tile
    /// keeps its skeleton / resting state — never a fake value). The projection
    /// math is delegated to the pure [`project`] helpers so it's unit-testable
    /// without a live Bus.
    #[must_use]
    pub fn read() -> Self {
        let peers = peers::action_directory().unwrap_or_default();
        let farm = build_farm::read_farm_events().unwrap_or_default();
        let health = datacenter::read_health_checks();
        let boot = home::read_boot_readiness();

        // FRONTDOOR-10 — the two FD-10-backend topics, read off the Bus the same
        // way the other widget tiles read live state (the latest body on the topic
        // is the current snapshot). Best-effort: an absent Bus / empty topic leaves
        // the status `None` (skeleton) and the suggestions empty (§7 — no fake).
        let copilot_status = copilot::parse_status(latest_body(copilot::STATUS_TOPIC).as_deref());
        let suggestions =
            copilot::parse_suggestions(latest_body(copilot::SUGGESTIONS_TOPIC).as_deref());

        Self {
            mesh_map: project::mesh_map(&peers),
            node_health: project::node_health(&peers),
            data_center: project::data_center(&peers),
            build_farm: project::build_farm(&farm),
            dev_ops: project::dev_ops(&farm),
            alerts: project::alerts(&health),
            system: Some(project::system(&boot)),
            // FRONTDOOR-10 — the Copilot tile's live value + the ranked suggestions.
            copilot: copilot_status.map(|s| s.tile_value()),
            suggestions,
            // FRONTDOOR-6 — carry the raw roster for the unified mesh search.
            peers,
        }
    }
}

/// FRONTDOOR-10 — read the latest body on a `state/` (or current-snapshot) Bus
/// topic: the newest message's body, or `None` when there is no Bus data-dir, the
/// topic is empty, or the read faults. The canonical "latest snapshot" read the
/// other widget loaders use (`read_health_checks`, the notify-center voice-status
/// read): `list_since(topic, None)` returns the messages ULID-ascending, so the
/// last is the newest. Best-effort by design — every failure is a quiet `None`.
#[must_use]
fn latest_body(topic: &str) -> Option<String> {
    let dir = mde_bus::default_data_dir()?;
    let persist = mde_bus::persist::Persist::open(dir).ok()?;
    let msgs = persist.list_since(topic, None).ok()?;
    msgs.last().and_then(|m| m.body.clone())
}

/// FRONTDOOR-4 — the pure widget-value projections. Each maps already-parsed data
/// (from the existing panels' loaders) into a `(value, tone)` line. Kept as a
/// separate `pub(super)` module so the data-mapping is unit-tested without a live
/// Bus (§7 DoD — "tests for the data-mapping").
pub(super) mod project {
    use super::TileTone;
    use crate::panels::build_farm::{FarmSnapshot, TierOutcome};
    use crate::panels::datacenter::{health_summary, HealthCheck};
    use crate::panels::home::BootReadiness;
    use crate::panels::peers::PeerRow;

    /// Mesh map → online/total presence. Tone: success when all online, warning
    /// when some are not, neutral when the roster is empty.
    #[must_use]
    pub fn mesh_map(peers: &[PeerRow]) -> Option<(String, TileTone)> {
        if peers.is_empty() {
            return None;
        }
        let online = peers.iter().filter(|p| p.presence == "online").count();
        let total = peers.len();
        let tone = if online == total {
            TileTone::Success
        } else {
            TileTone::Warning
        };
        Some((format!("{online}/{total} online"), tone))
    }

    /// Node health → healthy/total. Tone: success when all healthy, danger when
    /// any node is degraded.
    #[must_use]
    pub fn node_health(peers: &[PeerRow]) -> Option<(String, TileTone)> {
        if peers.is_empty() {
            return None;
        }
        let healthy = peers.iter().filter(|p| p.health == "healthy").count();
        let total = peers.len();
        let tone = if healthy == total {
            TileTone::Success
        } else {
            TileTone::Danger
        };
        Some((format!("{healthy}/{total} healthy"), tone))
    }

    /// Data Center → total node count (the fleet the menu fronts).
    #[must_use]
    pub fn data_center(peers: &[PeerRow]) -> Option<(String, TileTone)> {
        if peers.is_empty() {
            return None;
        }
        let n = peers.len();
        let unit = if n == 1 { "node" } else { "nodes" };
        Some((format!("{n} {unit}"), TileTone::Accent))
    }

    /// Build / Farm → the latest combined verdict: any failing nightly tier or
    /// failed farm job is red; an all-passing build is green; an empty/queued
    /// farm is amber "building". Mirrors the Build Farm panel's own tier/job
    /// vocabulary so the tile reads the same as the panel it links to.
    #[must_use]
    pub fn build_farm(farm: &FarmSnapshot) -> Option<(String, TileTone)> {
        let any_tier_fail = farm.tiers.iter().any(|t| t.outcome == TierOutcome::Fail);
        let any_job_fail = farm
            .jobs
            .iter()
            .any(|j| j.phase == "done" && j.outcome == "fail");
        let any_tier_pass = farm.tiers.iter().any(|t| t.outcome == TierOutcome::Pass);

        if any_tier_fail || any_job_fail {
            Some(("build: red".to_string(), TileTone::Danger))
        } else if any_tier_pass {
            Some(("build: green".to_string(), TileTone::Success))
        } else if !farm.jobs.is_empty() {
            Some(("build: queued".to_string(), TileTone::Warning))
        } else {
            // No farm/tier activity on the Bus at all → no honest metric.
            None
        }
    }

    /// DevOps → in-flight (not-done) farm jobs, the everyday CI loop's pulse.
    /// Tone: accent while jobs are running, neutral when the queue is idle.
    #[must_use]
    pub fn dev_ops(farm: &FarmSnapshot) -> Option<(String, TileTone)> {
        if farm.jobs.is_empty() {
            return None;
        }
        let running = farm.jobs.iter().filter(|j| j.phase != "done").count();
        let (value, tone) = if running > 0 {
            (format!("{running} running"), TileTone::Accent)
        } else {
            ("queue idle".to_string(), TileTone::Neutral)
        };
        Some((value, tone))
    }

    /// Alerts → the count of non-ok datacenter health checks (`warn`+`fail`).
    /// Tone: danger when any alert fires, success on an all-clear. Empty health
    /// set → `None` (no datacenter plane reporting → no honest count).
    #[must_use]
    pub fn alerts(health: &[HealthCheck]) -> Option<(String, TileTone)> {
        if health.is_empty() {
            return None;
        }
        let (_ok, warn, fail) = health_summary(health);
        let n = warn + fail;
        if n == 0 {
            Some(("all clear".to_string(), TileTone::Success))
        } else {
            let unit = if n == 1 { "alert" } else { "alerts" };
            Some((format!("{n} {unit}"), TileTone::Danger))
        }
    }

    /// System → boot readiness + the running workbench version. Always available
    /// (the version is compiled in; an unready/absent boot snapshot still reads
    /// honestly as "booting").
    #[must_use]
    pub fn system(boot: &BootReadiness) -> (String, TileTone) {
        let version = env!("CARGO_PKG_VERSION");
        if boot.ready {
            (format!("ready · v{version}"), TileTone::Success)
        } else {
            (format!("booting · v{version}"), TileTone::Warning)
        }
    }
}

// ============================ FRONTDOOR-6: unified search ====================

/// FRONTDOOR-6 — what kind of thing a local search hit is, so the results list
/// can group + label them and the ranker can break score ties by a stable kind
/// priority (apps before mesh nodes before services — the "open this surface"
/// destinations rank above the "this thing exists" mentions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    /// A launchable app / in-app surface (a panel route or a real binary).
    App,
    /// A mesh node (a peer in the directory) → opens the Peers directory.
    MeshNode,
    /// A service a mesh node publishes → opens the Peers / Mesh Services view.
    MeshService,
}

impl HitKind {
    /// Stable tie-break priority (lower sorts first): apps are destinations the
    /// operator most often means, then nodes, then the services they run.
    fn rank(self) -> u8 {
        match self {
            HitKind::App => 0,
            HitKind::MeshNode => 1,
            HitKind::MeshService => 2,
        }
    }

    /// The muted section caption this kind renders under in the results list.
    fn section(self) -> &'static str {
        match self {
            HitKind::App => "Apps",
            HitKind::MeshNode => "Mesh nodes",
            HitKind::MeshService => "Services",
        }
    }
}

/// FRONTDOOR-6 — one instant LOCAL search result. Carries its display label, a
/// short context line (e.g. the node a service runs on, or the surface a panel
/// lives under), its [`HitKind`], the relevance `score` the ranker assigned, and
/// the REAL app message activating it fires (§7 — a panel route or an app launch,
/// never an inert hit). Built purely by [`search::local_results`] so the search +
/// rank is unit-tested without any live Bus or view.
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// The matched thing's name, drawn as the row's primary label.
    pub label: String,
    /// A muted secondary line (the node a service is on / the panel's section).
    pub context: String,
    /// What kind of hit this is (drives the section + the tie-break order).
    pub kind: HitKind,
    /// Relevance score (higher = better); see [`search::score_match`].
    pub score: u32,
    /// The app message activating this hit fires — always one `App::update`
    /// handles (a `SelectPanel` route or a `LaunchApp` spawn).
    pub message: crate::Message,
}

/// FRONTDOOR-6 — the state of the Copilot `ask` for the current query (Q47 —
/// instant local results, the AI answer streams in below). Drives the AI card's
/// rendering: a "thinking…" placeholder while the ask is in flight, the prose
/// answer when it lands, or a quiet "unavailable" note on graceful degrade (Q33 —
/// no error spew). `Idle` (empty query) renders no card at all.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CopilotState {
    /// No query / no ask outstanding — render no AI card.
    #[default]
    Idle,
    /// The ask was published; awaiting the reply. Renders the "thinking…" card.
    Thinking,
    /// Copilot answered — render the prose as the AI card.
    Answer(String),
    /// Graceful degrade (Q33): Copilot is unavailable (worker absent, codex not
    /// installed, key unsealed, timeout). Renders a quiet one-line note, never an
    /// error dump — the local results still stand on their own.
    Unavailable,
}

/// FRONTDOOR-6 — the parsed outcome of one Copilot `ask`, mapped from the
/// worker's `AskReply` JSON (`{answer?, error?}`) by [`search::parse_copilot_reply`].
/// Kept as a small owned enum (not the raw JSON) so the message stays cheap and
/// the parse is unit-testable without the Bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopilotAnswer {
    /// The model's prose answer text.
    Answer(String),
    /// Any degrade path (no reply, an `error` field, malformed JSON) → render the
    /// quiet "unavailable" note. The reason is dropped on purpose (Q33 — no spew).
    Unavailable,
}

/// FRONTDOOR-6 — the pure unified-search engine: the instant LOCAL match + rank,
/// the Copilot `ask` request encoding, and the reply parse. Split into its own
/// module with NO view / Bus dependency so the whole search + rank + message-flow
/// is unit-tested directly (DoD — the local search/rank + the message flow), and
/// so the Bus call sites stay a thin shell over tested logic.
pub(super) mod search {
    use super::{CopilotAnswer, HitKind, SearchHit, Tile};
    use crate::model::{nav_model, Group};
    use crate::panels::peers::PeerRow;

    /// The Bus action topic the AI answer is published to (FD-9's copilot worker
    /// contract: `action/copilot/ask`, reply on the generic `reply/<ulid>` lane).
    pub const COPILOT_ASK_TOPIC: &str = "action/copilot/ask";

    /// Score one candidate `haystack` against the lowercased `needle`. Higher is
    /// better; `0` means no match (the candidate is dropped). The ladder is the
    /// "exact / prefix / word-prefix / substring" relevance the design accepts
    /// (frequency/recency not needed — these are small, static-ish catalogs):
    /// an exact equality outranks a leading-prefix, which outranks a match at a
    /// word boundary, which outranks a bare substring. Pure + case-insensitive.
    #[must_use]
    pub fn score_match(haystack: &str, needle: &str) -> u32 {
        if needle.is_empty() {
            return 0;
        }
        let hay = haystack.to_lowercase();
        let need = needle.to_lowercase();
        if hay == need {
            return 100;
        }
        if hay.starts_with(&need) {
            return 80;
        }
        // A match at the start of any whitespace/`-`/`_`/`/`-delimited word.
        if hay
            .split(|c: char| c.is_whitespace() || c == '-' || c == '_' || c == '/')
            .any(|word| word.starts_with(&need))
        {
            return 60;
        }
        if hay.contains(&need) {
            return 40;
        }
        0
    }

    /// FRONTDOOR-6 — compute the instant LOCAL results for `query` over the real
    /// catalogs (§7 — no `demo_data`): the app launchers (`tiles` with a launch
    /// action + the routable `nav_model` panels) and the mesh entities (the live
    /// `peers` directory — nodes + the services they publish). Returns the hits
    /// ranked best-first, capped to a sane on-screen count. A blank query yields
    /// nothing (the caller restores the tile grid). Pure: no Bus, no view.
    #[must_use]
    pub fn local_results(query: &str, tiles: &[Tile], peers: &[PeerRow]) -> Vec<SearchHit> {
        let q = query.trim();
        if q.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<SearchHit> = Vec::new();

        // ── Apps: the seeded launcher tiles + the routable nav panels ──
        // A launcher tile carries a real `LaunchApp`/route action via `actions()`;
        // we take its FIRST action as the hit's activation (the tile's primary
        // open). Keyed widget tiles are mesh surfaces, not "apps", so skip them
        // here — they surface via the mesh entities + their own grid.
        for tile in tiles {
            if tile.key.is_some() {
                continue; // a live widget, not an app launcher
            }
            let score = score_match(&tile.label, q);
            if score == 0 {
                continue;
            }
            // The launcher's real open (its first detail action). A launcher with
            // no openable surface yet (e.g. the Copilot placeholder) is dropped —
            // an un-actionable hit would violate §7.
            let Some(action) = tile.actions().into_iter().next() else {
                continue;
            };
            hits.push(SearchHit {
                label: tile.label.clone(),
                context: "App".to_string(),
                kind: HitKind::App,
                score,
                message: action.message,
            });
        }
        // The routable in-app surfaces (every nav panel is a real `SelectPanel`
        // destination). Matching on the curated label so "build farm" finds the
        // Build Farm panel.
        for entry in nav_model() {
            for panel in &entry.panels {
                let score = score_match(panel.label(), q);
                if score == 0 {
                    continue;
                }
                hits.push(SearchHit {
                    label: panel.label().to_string(),
                    context: format!("{} surface", entry.group.label()),
                    kind: HitKind::App,
                    // Panels are surfaces, not first-class apps: nudge below a
                    // same-strength launcher match so "Music" the app beats the
                    // "Music" panel when both tie, but a strong panel-only match
                    // still ranks well.
                    score: score.saturating_sub(5),
                    message: crate::Message::SelectPanel {
                        group: entry.group,
                        panel: panel.slug(),
                    },
                });
            }
        }

        // ── Mesh entities: the live Peers directory (nodes + services) ──
        for peer in peers {
            let node_score = score_match(&peer.hostname, q);
            if node_score > 0 {
                hits.push(SearchHit {
                    label: peer.hostname.clone(),
                    context: mesh_node_context(peer),
                    kind: HitKind::MeshNode,
                    score: node_score,
                    // Open the Peers directory (the surface that owns node detail
                    // + the per-node actions) — a real route, not a stub.
                    message: peers_route(),
                });
            }
            // The services this node publishes — each a distinct hit pointing at
            // the same Peers surface (where the service's node is actionable).
            for svc in &peer.services {
                let svc_score = score_match(svc, q);
                if svc_score == 0 {
                    continue;
                }
                hits.push(SearchHit {
                    label: svc.clone(),
                    context: format!("on {}", peer.hostname),
                    kind: HitKind::MeshService,
                    // A service match is a hair weaker than the same-strength node
                    // match (the node is the thing you usually open).
                    score: svc_score.saturating_sub(3),
                    message: peers_route(),
                });
            }
        }

        rank(&mut hits);
        // Cap to a comfortable on-screen list — the AI card carries the long tail.
        hits.truncate(MAX_LOCAL_RESULTS);
        hits
    }

    /// The route every mesh hit opens: the Peers directory (Q20 — mesh entities
    /// navigate to the surface that owns them; Peers is where a node/service is
    /// actionable). A `&'static` slug, so it satisfies `SelectPanel`.
    fn peers_route() -> crate::Message {
        crate::Message::SelectPanel {
            group: Group::Mesh,
            panel: "peers",
        }
    }

    /// A node's secondary line: its role + presence, when known, so a node hit
    /// reads "anvil — peer · online" rather than a bare hostname.
    fn mesh_node_context(peer: &PeerRow) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !peer.role.is_empty() {
            parts.push(peer.role.clone());
        }
        if !peer.presence.is_empty() {
            parts.push(peer.presence.clone());
        }
        if parts.is_empty() {
            "mesh node".to_string()
        } else {
            parts.join(" · ")
        }
    }

    /// Rank hits best-first: by descending relevance score, then by the stable
    /// [`HitKind`] priority (apps before nodes before services), then alphabetical
    /// — so the order is deterministic (no view-time flicker on equal scores).
    fn rank(hits: &mut [SearchHit]) {
        hits.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then(a.kind.rank().cmp(&b.kind.rank()))
                .then(a.label.to_lowercase().cmp(&b.label.to_lowercase()))
        });
    }

    /// Cap on the instant-local list length; the AI card carries the rest (Q47).
    pub const MAX_LOCAL_RESULTS: usize = 12;

    /// FRONTDOOR-6 — encode the Copilot `ask` request body for `query` (FD-9's
    /// `AskRequest` JSON shape: `{prompt, context}`). The omnibox text is the
    /// prompt; a fixed context line tells the worker the ask came from the Front
    /// Door search box. Pure — the Bus publish is the caller's thin shell.
    #[must_use]
    pub fn ask_request_body(query: &str) -> String {
        // Build the JSON via serde so any quote/newline in the query is escaped
        // (a raw `format!` would mis-encode a query containing a `"`).
        serde_json::json!({
            "prompt": query.trim(),
            "context": "Asked from the Magic Mesh Front Door unified search box.",
        })
        .to_string()
    }

    /// FRONTDOOR-6 — parse the Copilot worker's reply body into a [`CopilotAnswer`].
    /// The reply is FD-9's `AskReply` JSON: `{answer?, error?}`. A present, non-
    /// empty `answer` is the prose; ANY other shape (an `error`, a null/empty
    /// answer, malformed JSON, or `None` for no-reply/timeout) degrades to
    /// `Unavailable` (Q33 — graceful degrade, no error spew). Pure + Bus-free.
    #[must_use]
    pub fn parse_copilot_reply(raw: Option<&str>) -> CopilotAnswer {
        let Some(body) = raw else {
            return CopilotAnswer::Unavailable; // no reply (timeout / no worker)
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(body.trim()) else {
            return CopilotAnswer::Unavailable;
        };
        match v.get("answer").and_then(serde_json::Value::as_str) {
            Some(answer) if !answer.trim().is_empty() => {
                CopilotAnswer::Answer(answer.trim().to_string())
            }
            // An `error` field, a null answer, or an empty answer → unavailable.
            _ => CopilotAnswer::Unavailable,
        }
    }
}

// ===================== FRONTDOOR-10 (GUI half): Copilot live =================

/// FRONTDOOR-10 — the Copilot live read off the two FD-10-backend bus topics
/// (`state/copilot/status` + `action/copilot/suggestions`). The workbench can't
/// depend on `mackesd` (the §6 mesh/desktop boundary — the copilot types live in
/// `mackesd::workers::copilot`), so this module mirrors the WIRE shapes the
/// backend serialized and parses them GUI-side — exactly as FD-6 parses FD-9's
/// `AskReply` JSON locally rather than importing it. Pure (no Bus / no view) so
/// the parse + the suggestion→tile mapping are unit-tested directly; the Bus read
/// in [`FrontDoorData::read`] is a thin shell over this.
pub(super) mod copilot {
    use super::{TileKey, TileTone};

    /// The bus topic the FD-10 backend publishes the compact Copilot STATUS to
    /// (`mackesd::workers::copilot::STATUS_TOPIC`). A `state/` snapshot the tile
    /// reads the same way the other widget tiles read live state — the latest body
    /// is the current status.
    pub const STATUS_TOPIC: &str = "state/copilot/status";

    /// The bus topic the FD-10 backend publishes its ranked proactive SUGGESTIONS
    /// to (`mackesd::workers::copilot::SUGGESTIONS_TOPIC`). The latest body is the
    /// current ranked set; each suggestion is a PROPOSAL the operator approves —
    /// never an instruction the GUI executes (§9).
    pub const SUGGESTIONS_TOPIC: &str = "action/copilot/suggestions";

    /// The bus topic the GUI re-publishes an approved suggestion's typed proposal
    /// to (`mackesd::workers::copilot::PROPOSAL_TOPIC`) — FD-12's propose-only
    /// review queue. The "Act" affordance writes HERE, never to FD-11's execution
    /// topic (`action/exec/request`): proposing is not executing, and the gated
    /// confirm/exec UI that drains this is a SEPARATE, out-of-scope surface (§9 —
    /// the GUI never auto-executes).
    pub const PROPOSAL_TOPIC: &str = "action/copilot/proposal";

    /// The coarse Copilot status the tile renders (FD-10 §1). Mirrors the backend
    /// `CopilotState` (serde `lowercase`): `ready` / `thinking` / `offline`. A
    /// missing/odd tag degrades to [`StatusState::Offline`] (the tile shows offline
    /// rather than guess — Q33 graceful degrade).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum StatusState {
        /// The leader has a usable codex key — Copilot can answer/suggest.
        Ready,
        /// A codex round-trip is in flight right now.
        Thinking,
        /// No codex here (not leader, key unsealed, store fault) — graceful degrade.
        Offline,
    }

    impl StatusState {
        /// The tile's live value line for this state.
        #[must_use]
        pub fn label(self) -> &'static str {
            match self {
                StatusState::Ready => "ready",
                StatusState::Thinking => "thinking",
                StatusState::Offline => "offline",
            }
        }

        /// The Carbon tone the value reads in (§4 token, never hex): a usable
        /// Copilot reads accent/success, an in-flight one warning, an offline one
        /// muted-neutral so the tile visibly degrades without an error color.
        #[must_use]
        pub fn tone(self) -> TileTone {
            match self {
                StatusState::Ready => TileTone::Success,
                StatusState::Thinking => TileTone::Warning,
                StatusState::Offline => TileTone::Neutral,
            }
        }
    }

    /// The parsed Copilot status (FD-10 §1) — just what the tile renders.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Status {
        /// The coarse state the value line shows.
        pub state: StatusState,
        /// Whether Copilot can actually serve here (leader + a usable codex key).
        pub available: bool,
    }

    impl Status {
        /// The tile's `(value, tone)` for this status — the pre-rendered live line
        /// the keyed Copilot tile takes, identical in shape to the other widgets'
        /// projections.
        #[must_use]
        pub fn tile_value(&self) -> (String, TileTone) {
            (self.state.label().to_string(), self.state.tone())
        }
    }

    /// Parse the latest `state/copilot/status` body (the backend `CopilotStatus`
    /// JSON: `{state, leader, available, model, last_activity_s?}`). Tolerant: we
    /// only need `state` + `available`; an unknown `state` tag, a missing field, or
    /// malformed/`None` JSON degrades to `None` (the tile keeps its skeleton /
    /// resting state — never a fake value, §7). Pure + Bus-free.
    #[must_use]
    pub fn parse_status(raw: Option<&str>) -> Option<Status> {
        let v: serde_json::Value = serde_json::from_str(raw?.trim()).ok()?;
        let state = match v.get("state").and_then(serde_json::Value::as_str)? {
            "ready" => StatusState::Ready,
            "thinking" => StatusState::Thinking,
            "offline" => StatusState::Offline,
            _ => return None,
        };
        let available = v
            .get("available")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        Some(Status { state, available })
    }

    /// One parsed proactive suggestion (FD-10 §2) — the prose the GUI renders plus,
    /// when actionable, the typed proposal the operator can ACT on. Mirrors the
    /// backend `Suggestion` wire shape. The proposal is carried as its raw JSON
    /// object body so the "Act" affordance can re-publish it to [`PROPOSAL_TOPIC`]
    /// verbatim (the propose-only path) WITHOUT the workbench needing the typed
    /// `ActionProposal`/`ActionRequest` enums (they live in `mackesd`, off-limits
    /// across the §6 boundary). It is NEVER executed here (§9).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Suggestion {
        /// A short operator-facing headline.
        pub title: String,
        /// The supporting rationale / what-to-do.
        pub detail: String,
        /// `high` | `medium` — carried so the GUI can badge/sort; the backend keeps
        /// it high-confidence-only (Q61).
        pub impact: String,
        /// The typed proposal's raw JSON object body (the backend `ActionProposal`:
        /// `{action, rationale}`), present only when the suggestion is actionable.
        /// `None` ⇒ advisory-only. Re-published verbatim to [`PROPOSAL_TOPIC`] on
        /// "Act" — never to the exec topic (§9).
        pub proposal_body: Option<String>,
    }

    impl Suggestion {
        /// Which tile this suggestion concerns (Q19 — inline on the relevant tile),
        /// inferred from the title+detail text against the widget vocabulary, or
        /// `None` when it names no tile (it then lands in the Copilot tile's general
        /// suggestions area rather than being dropped). A typed proposal's
        /// `service_lifecycle` target sharpens the map to the Data Center tile.
        /// Pure keyword classification — deterministic, no Bus.
        #[must_use]
        pub fn concerns_tile(&self) -> Option<TileKey> {
            let hay = format!("{} {}", self.title, self.detail).to_lowercase();
            // A typed service-lifecycle proposal is always a node/service op → the
            // Data Center tile owns the node-lifecycle actions (FD-8).
            if self
                .proposal_body
                .as_deref()
                .is_some_and(|b| b.contains("service_lifecycle"))
            {
                return Some(TileKey::DataCenter);
            }
            // Keyword buckets, most-specific first. Each phrase names a widget the
            // operator would open to act on that class of fix.
            const MAP: &[(&[&str], TileKey)] = &[
                (&["alert", "alarm", "incident"], TileKey::Alerts),
                (
                    &["build", "farm", "ci", "pipeline", "compile"],
                    TileKey::BuildFarm,
                ),
                (&["job", "deploy", "rollback", "rerun"], TileKey::DevOps),
                (
                    &[
                        "node",
                        "host",
                        "provision",
                        "drain",
                        "cutover",
                        "container",
                        "vm",
                        "service",
                        "restart",
                    ],
                    TileKey::DataCenter,
                ),
                (
                    &["health", "degraded", "unreachable", "down"],
                    TileKey::NodeHealth,
                ),
                (&["mesh", "peer", "routing", "latency"], TileKey::MeshMap),
                (&["disk", "memory", "boot", "version"], TileKey::System),
            ];
            for (needles, key) in MAP {
                if needles.iter().any(|n| hay.contains(n)) {
                    return Some(*key);
                }
            }
            None
        }
    }

    /// Parse the latest `action/copilot/suggestions` body (the backend
    /// `SuggestionSet` JSON: `{suggestions:[…], produced_at_s}`) into the ranked
    /// list the GUI renders. Tolerant: the proposal is kept as its raw JSON object
    /// (re-serialized so it round-trips to [`PROPOSAL_TOPIC`] cleanly); a missing
    /// title drops the entry; malformed / `None` JSON ⇒ empty (no panic, the tiles
    /// just carry no suggestions — §7). Order is preserved (the backend ranks it
    /// best-first). Pure + Bus-free.
    #[must_use]
    pub fn parse_suggestions(raw: Option<&str>) -> Vec<Suggestion> {
        let Some(body) = raw else {
            return Vec::new();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(body.trim()) else {
            return Vec::new();
        };
        let Some(arr) = v.get("suggestions").and_then(serde_json::Value::as_array) else {
            return Vec::new();
        };
        arr.iter()
            .filter_map(|s| {
                let title = s
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if title.is_empty() {
                    return None;
                }
                let detail = s
                    .get("detail")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let impact = match s
                    .get("impact")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("medium")
                {
                    "high" => "high".to_string(),
                    _ => "medium".to_string(),
                };
                // Keep the proposal as its own JSON object body so "Act" re-publishes
                // it verbatim to the propose topic — never re-derived, never executed.
                let proposal_body = s
                    .get("proposal")
                    .filter(|p| p.is_object())
                    .map(std::string::ToString::to_string);
                Some(Suggestion {
                    title,
                    detail,
                    impact,
                    proposal_body,
                })
            })
            .collect()
    }
}

/// The Front Door home state: the placeholder tile set + a loading flag, plus the
/// FRONTDOOR-2 omnibox query. While `loading`, the grid draws flat grey skeleton
/// cards instead of labeled tiles (Q92 — skeleton placeholders, no layout shift).
#[derive(Debug, Clone)]
pub struct FrontDoor {
    /// The tiles to draw (static placeholders for FRONTDOOR-1).
    pub tiles: Vec<Tile>,
    /// True → render skeletons; false → render labeled tiles.
    pub loading: bool,
    /// FRONTDOOR-2 — the omnibox's live text. Tracked here so the field is
    /// controlled; FRONTDOOR-6 makes a non-empty value drive the unified search.
    pub query: String,
    /// FRONTDOOR-6 — the live Peers directory rows, the source for the instant
    /// mesh-entity search (nodes + the services they publish). Folded in from the
    /// FD-4 [`FrontDoorData`] read (the same `peers::action_directory` snapshot
    /// the widget tiles already use — no new Bus path, §6). Empty until the first
    /// load; the search simply returns fewer hits until then (graceful).
    pub peers: Vec<peers::PeerRow>,
    /// FRONTDOOR-6 — the current instant LOCAL search results (apps + mesh
    /// entities), recomputed on every [`Message::OmniboxChanged`]. Empty when the
    /// query is blank (the tile grid shows instead).
    pub results: Vec<SearchHit>,
    /// FRONTDOOR-6 — the Copilot `ask` state for the current query (the AI card
    /// below the local results). `Idle` for a blank query (no card).
    pub copilot: CopilotState,
    /// FRONTDOOR-6 — a monotonic generation stamped on each Copilot ask. A reply
    /// is folded in ONLY if its generation still matches (so a slow reply for a
    /// query the operator has since changed/cleared is dropped, not shown stale).
    pub copilot_gen: u64,
    /// FRONTDOOR-3 — which render mode the Front Door is in (panel default,
    /// flipped by the top-bar toggle). Default [`Mode::Panel`] (Q29).
    pub mode: Mode,
    /// FRONTDOOR-5 — the index of the tile whose detail **actions menu** is open
    /// (Q45/Q49), or `None` when the grid is showing. Set by a canvas tile click
    /// ([`Message::TileActivated`]); cleared by the menu's back control
    /// ([`Message::CloseDetail`]) or by leaving / reloading the view.
    pub detail: Option<usize>,
    /// FRONTDOOR-10 — the live ranked proactive suggestions (off
    /// `action/copilot/suggestions`), folded in from the FD-4 [`FrontDoorData`]
    /// read. Each suggestion renders inline on the tile it concerns (Q19) — a
    /// count badge on the canvas tile + the card text in that tile's detail view,
    /// with the unmapped ones gathered on the Copilot tile. Empty until a set
    /// lands / when the mesh is quiet (Q61). A suggestion is a PROPOSAL: acting on
    /// it re-publishes to the propose topic, never executes (§9).
    pub suggestions: Vec<copilot::Suggestion>,
}

impl Default for FrontDoor {
    fn default() -> Self {
        Self::new()
    }
}

impl FrontDoor {
    /// Seed the home grid with the design's widget set (Q99: mesh map, build/
    /// farm, alerts, node health, Copilot, system) plus a few app launchers.
    /// FRONTDOOR-4 — the widget tiles carry a [`TileKey`] and take their live
    /// `value` + `tone` from the first [`FrontDoor::load`] snapshot; until then
    /// `loading` is true so the FD-1 skeleton covers them (Q92 — no layout
    /// shift). FRONTDOOR-10 — Copilot is now a live **widget** (the backend's
    /// `state/copilot/status` topic exists), reading ready/thinking/offline; its
    /// proactive suggestions render inline on the tiles they concern (Q19).
    #[must_use]
    pub fn new() -> Self {
        let tiles = vec![
            Tile::widget("Mesh Map", TileKey::MeshMap, TileTone::Accent),
            Tile::widget("Build / Farm", TileKey::BuildFarm, TileTone::Warning),
            Tile::widget("Alerts", TileKey::Alerts, TileTone::Danger),
            Tile::widget("Node Health", TileKey::NodeHealth, TileTone::Success),
            // FRONTDOOR-10 — now a LIVE widget: the FD-10-backend `state/copilot/
            // status` topic exists, so the tile reads ready/thinking/offline off it
            // (closing FD-4's launcher-only gap). The skeleton covers it until the
            // first snapshot (Q92).
            Tile::widget("Copilot", TileKey::Copilot, TileTone::Accent),
            Tile::widget("System", TileKey::System, TileTone::Neutral),
            Tile::widget("Data Center", TileKey::DataCenter, TileTone::Accent),
            Tile::widget("DevOps", TileKey::DevOps, TileTone::Warning),
            Tile::launcher("Files", TileTone::Neutral),
            Tile::launcher("Terminal", TileTone::Neutral),
            Tile::launcher("Settings", TileTone::Neutral),
            Tile::launcher("Music", TileTone::Neutral),
        ];
        Self {
            tiles,
            // FRONTDOOR-4 — start in the skeleton state; the first `load()`
            // snapshot flips it off without a layout shift.
            loading: true,
            query: String::new(),
            // FRONTDOOR-6 — no roster / results / AI ask until the operator
            // searches (a blank query renders the tile grid).
            peers: Vec::new(),
            results: Vec::new(),
            copilot: CopilotState::Idle,
            copilot_gen: 0,
            mode: Mode::Panel,
            // FRONTDOOR-5 — start on the grid; a tile click opens a detail menu.
            detail: None,
            // FRONTDOOR-10 — no suggestions until the first snapshot (a quiet mesh
            // keeps it empty — Q61).
            suggestions: Vec::new(),
        }
    }

    /// FRONTDOOR-4 — read the widget tiles' live data off the **existing**
    /// mde-bus data paths (Peers directory · Build Farm · Datacenter health ·
    /// boot readiness; FRONTDOOR-10 also reads the Copilot status + suggestions
    /// topics) on a blocking thread and fold it back in via [`Message::Loaded`].
    /// Dispatched the same way the other panels load (a `Task` fired on nav /
    /// reconnect / the slow-poll tick), so the Front Door gets its data through the
    /// established subscription infra (§6) — the FD-10 topics are already published
    /// by mackesd, so no new publisher is added on the GUI side.
    pub fn load() -> Task<crate::Message> {
        Task::perform(async { FrontDoorData::read() }, |data| {
            crate::Message::FrontDoor(Message::Loaded(Box::new(data)))
        })
    }

    /// FRONTDOOR-4 — the **slow-poll fallback** (design Q22: event-driven +
    /// slow-poll). The push half is the Peers directory-changed Bus event the
    /// app already subscribes to (it reloads the active view); this 15 s tick
    /// backstops the topics that aren't purely push — the Build Farm verdict,
    /// the Datacenter health checks, boot readiness — so the tiles stay live
    /// even with no roster change. Registered by `App::subscription` ONLY while
    /// the Front Door is the active view, so nothing polls when the operator is
    /// elsewhere (the view-gating pattern the other panels use).
    pub fn poll_subscription() -> Subscription<crate::Message> {
        cosmic::iced::time::every(Duration::from_secs(15))
            .map(|_| crate::Message::FrontDoor(Message::Reload))
    }

    /// FRONTDOOR-2/3/4 — fold a Front Door message into local state. The FD-2/3
    /// variants (omnibox text; the panel ↔ full-screen flip) are pure local
    /// edits with no follow-up; FRONTDOOR-4's [`Message::Reload`] kicks off the
    /// async Bus read (so it returns a `Task`), and [`Message::Loaded`] folds the
    /// snapshot into the widget tiles + clears the skeleton.
    pub fn update(&mut self, message: Message) -> Task<crate::Message> {
        match message {
            // FRONTDOOR-6 — the omnibox drives the real unified search. The
            // instant LOCAL results (apps + mesh entities) recompute synchronously
            // here (Q47 — local first); a non-empty query also fires the async
            // Copilot `ask` (the AI card streams in below). A blank query clears
            // everything back to the tile grid + cancels any pending AI ask.
            Message::OmniboxChanged(q) => {
                self.query = q;
                self.recompute_results();
                self.search_task()
            }
            // FRONTDOOR-6 — a Copilot reply landed. Fold it ONLY if it still
            // matches the generation the ask was fired under (a slow reply for a
            // since-changed query is dropped, never shown stale).
            Message::CopilotReplied(gen, answer) => {
                if gen == self.copilot_gen {
                    self.copilot = match answer {
                        CopilotAnswer::Answer(a) => CopilotState::Answer(a),
                        CopilotAnswer::Unavailable => CopilotState::Unavailable,
                    };
                }
                Task::none()
            }
            // FRONTDOOR-6 — a local search hit was clicked: fire its real app
            // message (a panel route / app launch) and clear the omnibox so the
            // Front Door returns to the grid behind the navigation.
            Message::SearchHitActivated(msg) => {
                self.query.clear();
                self.recompute_results();
                Task::done(*msg)
            }
            Message::ToggleMode => {
                self.mode = match self.mode {
                    Mode::Panel => Mode::FullScreen,
                    Mode::FullScreen => Mode::Panel,
                };
                Task::none()
            }
            // The slow-poll / reconnect tick: fire a fresh off-thread Bus read.
            Message::Reload => Self::load(),
            // A fresh snapshot landed — fold it into the widget tiles and lift
            // the skeleton (Q92 — the first real data clears `loading`). The
            // detail menu (if open) re-reads its tile by index, so a reload that
            // refreshes the live `value` line shows through without re-opening.
            Message::Loaded(data) => {
                self.apply(&data);
                self.loading = false;
                Task::none()
            }
            // FRONTDOOR-5 — a tile was clicked (Q45): open its detail actions
            // menu (Q49). Guard the index against a stale click landing after the
            // tile set changed, so a bad index never opens an empty menu.
            Message::TileActivated(i) => {
                if i < self.tiles.len() {
                    self.detail = Some(i);
                }
                Task::none()
            }
            // FRONTDOOR-5 — back out of the detail menu to the grid.
            Message::CloseDetail => {
                self.detail = None;
                Task::none()
            }
            // FRONTDOOR-7 — a one-click pipeline action: route to the existing
            // surface AND (for the wired actions) fire its own real trigger, in a
            // single click. Clear the detail first so the grid is restored behind
            // the navigation (mirrors `SearchHitActivated`). The nav alone already
            // refreshes the destination (its `on_panel_navigated` load); the
            // optional `trigger` adds the panel's own re-poll verb on top (§9 —
            // the existing typed message, never a raw shell). Both are dispatched
            // as app messages `App::update` already handles, batched together.
            Message::PipelineAction { nav, trigger } => {
                self.detail = None;
                let mut tasks = vec![Task::done(*nav)];
                if let Some(trigger) = trigger {
                    tasks.push(Task::done(*trigger));
                }
                Task::batch(tasks)
            }
            // FRONTDOOR-10 — Act on a suggestion's typed proposal. Re-publish it to
            // the PROPOSE topic (FD-12's review queue) — fire-and-forget, off the
            // iced thread (the Bus client owns its own runtime; `Persist` isn't
            // `Send`, so it MUST ride `spawn_blocking`). §9: this is a PROPOSE, not
            // an execute — it never touches `action/exec/request`, never runs the
            // action. A suggestion with no typed proposal (advisory-only) or a stale
            // index is a no-op (the "Act" affordance is only rendered when a
            // proposal exists, so this is just defence-in-depth).
            Message::ProposeSuggestion(i) => {
                let Some(body) = self
                    .suggestions
                    .get(i)
                    .and_then(|s| s.proposal_body.clone())
                else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        let _ = tokio::task::spawn_blocking(move || {
                            crate::dbus::action_publish(copilot::PROPOSAL_TOPIC, &body)
                        })
                        .await;
                    },
                    |()| crate::Message::Noop,
                )
            }
        }
    }

    /// FRONTDOOR-6 — recompute the instant LOCAL results for the current query
    /// (apps + mesh entities, ranked), and reset the Copilot state to match: a
    /// blank query clears the results AND parks Copilot at `Idle` (no AI card, no
    /// pending ask); a non-empty query sets it to `Thinking` (the placeholder
    /// shows until the reply lands or the next keystroke supersedes it). The
    /// actual ask is fired by [`Self::search_task`], called alongside this from
    /// the message handler. Pure local state mutation — no Bus.
    fn recompute_results(&mut self) {
        if self.query.trim().is_empty() {
            self.results.clear();
            self.copilot = CopilotState::Idle;
        } else {
            self.results = search::local_results(&self.query, &self.tiles, &self.peers);
            // The AI card shows "thinking…" the instant the operator types; the
            // bump to `copilot_gen` (in `search_task`) makes the in-flight reply
            // for any prior query a no-op when it lands.
            self.copilot = CopilotState::Thinking;
        }
    }

    /// FRONTDOOR-6 — fire the async Copilot `ask` for the current query (Q47 —
    /// the AI answer streams in below the local results). A blank query fires
    /// nothing (no AI card). Otherwise it bumps `copilot_gen` (so a slow reply for
    /// the previous query is dropped) and publishes the query to FD-9's
    /// `action/copilot/ask` topic on a blocking thread (the Bus client builds its
    /// own current-thread runtime — `Persist`/rusqlite isn't `Send` — so it MUST
    /// run under `spawn_blocking`, the same contract every other Bus read here
    /// follows). The reply (or a degrade) comes back as [`Message::CopilotReplied`]
    /// stamped with this generation. Graceful degrade (Q33): no Bus / no worker /
    /// timeout → `CopilotAnswer::Unavailable`, never an error or a hang.
    fn search_task(&mut self) -> Task<crate::Message> {
        let query = self.query.trim().to_string();
        if query.is_empty() {
            return Task::none();
        }
        self.copilot_gen = self.copilot_gen.wrapping_add(1);
        let generation = self.copilot_gen;
        Task::perform(
            async move {
                let body = search::ask_request_body(&query);
                // The Bus action/reply client blocks (it owns a runtime), so it
                // rides `spawn_blocking`. A join error / no Bus / timeout all
                // collapse to `None` → `Unavailable` (no spew).
                let raw = tokio::task::spawn_blocking(move || {
                    crate::dbus::action_request_with_body(
                        search::COPILOT_ASK_TOPIC,
                        Some(&body),
                        COPILOT_ASK_TIMEOUT,
                    )
                })
                .await
                .ok()
                .flatten();
                search::parse_copilot_reply(raw.as_deref())
            },
            move |answer| crate::Message::FrontDoor(Message::CopilotReplied(generation, answer)),
        )
    }

    /// FRONTDOOR-4 — fold one [`FrontDoorData`] snapshot into the widget tiles:
    /// each keyed tile takes its `(value, tone)` from the snapshot, or keeps a
    /// `None` value (no source this round) — a launcher (`key == None`) is never
    /// touched. FRONTDOOR-6 — also stores the raw roster for the unified search,
    /// and (if a search is live) re-ranks the results against the fresh roster so
    /// the mesh-entity hits track the directory. Pure given the snapshot.
    pub fn apply(&mut self, data: &FrontDoorData) {
        for tile in &mut self.tiles {
            let Some(key) = tile.key else { continue };
            if let Some((value, tone)) = data.for_key(key) {
                tile.value = Some(value);
                tile.tone = tone;
            } else {
                // No data for this widget this round (incl. Copilot): clear any
                // stale value rather than show a phantom metric (§7).
                tile.value = None;
            }
        }
        // FRONTDOOR-10 — fold the ranked proactive suggestions in (Q19 — they
        // render inline on the tile each concerns: a badge on the canvas card + the
        // card text in that tile's detail). Replaced wholesale each snapshot so a
        // resolved suggestion drops off rather than lingering (§7 — no stale card).
        self.suggestions = data.suggestions.clone();
        // FRONTDOOR-6 — refresh the search roster; re-rank live results so a
        // roster change while the operator is mid-search shows through.
        self.peers = data.peers.clone();
        if !self.query.trim().is_empty() {
            self.results = search::local_results(&self.query, &self.tiles, &self.peers);
        }
    }

    /// FRONTDOOR-10 — the suggestions that concern the tile at index `i` (Q19),
    /// in rank order. A suggestion maps to the tile its text/proposal names
    /// ([`copilot::Suggestion::concerns_tile`]); an UNMAPPED suggestion (it names
    /// no tile) is gathered onto the Copilot tile so it is never dropped. Returns
    /// the matching `(suggestion, global_index)` pairs — the global index is
    /// carried so the "Act" affordance can address the exact suggestion in
    /// `self.suggestions` regardless of the per-tile filtering. Pure.
    fn suggestions_for_tile(&self, i: usize) -> Vec<(usize, &copilot::Suggestion)> {
        let Some(tile) = self.tiles.get(i) else {
            return Vec::new();
        };
        let is_copilot = tile.key == Some(TileKey::Copilot);
        self.suggestions
            .iter()
            .enumerate()
            .filter(|(_, s)| match s.concerns_tile() {
                Some(key) => tile.key == Some(key),
                // Unmapped → home it on the Copilot tile (never dropped).
                None => is_copilot,
            })
            .collect()
    }

    /// FRONTDOOR-10 — the count of suggestions concerning the tile at index `i`,
    /// for the canvas badge (Q19 — the on-tile suggestion indicator). 0 ⇒ no badge.
    fn suggestion_count_for(&self, i: usize) -> usize {
        self.suggestions_for_tile(i).len()
    }

    /// FRONTDOOR-2/3 — the Front Door view, branching on [`Self::mode`]:
    /// [`Mode::Panel`] renders the FD-2 Win10-Start two-pane shell (rail + right
    /// pane); [`Mode::FullScreen`] renders the FD-3 iPadOS-home full-screen grid
    /// (rail hidden). The top-bar toggle (in each mode) flips between them.
    #[must_use]
    pub fn view(&self) -> Element<'_, crate::Message, Theme> {
        let palette = crate::live_theme::palette();
        // FRONTDOOR-5 — an open tile detail takes over the pane (Q45/Q49): the
        // actions menu for the clicked tile, with a Back control to the grid.
        // Validate the index against the live tile set so a stale index can never
        // panic the view.
        if let Some((i, tile)) = self
            .detail
            .and_then(|i| self.tiles.get(i).map(|t| (i, t)))
        {
            return self.detail_view(i, tile, palette);
        }
        match self.mode {
            Mode::Panel => self.panel_view(palette),
            Mode::FullScreen => self.fullscreen_view(palette),
        }
    }

    /// FRONTDOOR-5 — the **detail actions menu** for one tile (Q45 click→detail,
    /// Q49 detail = actions menu): the tile's label + its live data line, then a
    /// list of REAL actions (each navigates to an existing panel or launches a
    /// real app — §7, no stubs), under a Back control that returns to the grid.
    /// A tile with no openable surface yet (Copilot) shows its data + Back only —
    /// an honest empty menu rather than a dead button. Carbon chrome via tokens
    /// only (§4). Rendered for both modes (the grid mode is restored on Back).
    ///
    /// FRONTDOOR-10 — when proactive suggestions concern THIS tile (`i` is its
    /// index), they render inline above the actions (Q19): each a card with the
    /// title + rationale and, when the suggestion carries a typed proposal, a §9-
    /// safe **Act** button that re-publishes the proposal to the propose queue —
    /// never executes.
    fn detail_view(
        &self,
        i: usize,
        tile: &Tile,
        palette: Palette,
    ) -> Element<'_, crate::Message, Theme> {
        let sizes = FontSize::defaults();

        // Back control — always real (clears the detail), mirrors the mode toggle.
        let back = {
            let accent = palette.accent.into_cosmic_color();
            let raised = palette.raised.into_cosmic_color();
            let idle_bg = palette.hover_tint().into_cosmic_color();
            button(
                text("← Back")
                    .size(TypeRole::Body.size_in(sizes))
                    .colr(accent),
            )
            .padding(Padding::from([8u16, 14u16]))
            .sty(
                move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                    use cosmic::iced::widget::button::Status;
                    let bg = match status {
                        Status::Hovered | Status::Pressed => raised,
                        _ => idle_bg,
                    };
                    cosmic::iced::widget::button::Style {
                        snap: false,
                        background: Some(Background::Color(bg)),
                        text_color: accent,
                        border: Border {
                            color: cosmic::iced::Color::TRANSPARENT,
                            width: 0.0,
                            radius: 6.0.into(),
                        },
                        shadow: cosmic::iced::Shadow::default(),
                        ..cosmic::iced::widget::button::Style::default()
                    }
                },
            )
            .on_press(crate::Message::FrontDoor(Message::CloseDetail))
        };

        // Header: the tile label + its live data line (or a resting note when the
        // source had nothing this round — never a faked metric, §7).
        let value_line = match &tile.value {
            Some(v) => text(v.clone())
                .size(TypeRole::Body.size_in(sizes))
                .colr(tile.tone.color(&palette)),
            None => text("No live data")
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        };
        let header = column![
            text(tile.label.clone())
                .size(TypeRole::Heading.size_in(sizes))
                .colr(palette.text.into_cosmic_color()),
            value_line,
        ]
        .spacing(4);

        // FRONTDOOR-10 — the proactive suggestion cards concerning THIS tile (Q19),
        // rank-order, above the actions. Each shows the title + rationale and, when
        // it carries a typed proposal, a §9-safe "Act" that re-publishes the
        // proposal to the propose queue (never executes). Empty when no suggestion
        // concerns this tile (the section is omitted, not an empty header).
        let tile_suggestions = self.suggestions_for_tile(i);
        let suggestions_section: Option<Element<'_, crate::Message, Theme>> =
            if tile_suggestions.is_empty() {
                None
            } else {
                let mut sec = column![rail_section_label("Copilot suggestions", palette)].spacing(8);
                for (gi, s) in tile_suggestions {
                    sec = sec.push(suggestion_card(gi, s, palette));
                }
                Some(sec.into())
            };

        // The actions list — every row is a REAL navigation / launch (§7). An
        // empty list (Copilot) renders an honest note instead of a dead row.
        let actions = tile.actions();
        let mut menu = column![rail_section_label("Actions", palette)].spacing(6);
        if actions.is_empty() {
            menu = menu.push(
                text("No actions available yet for this tile.")
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            );
        } else {
            for action in actions {
                menu = menu.push(detail_action_row(action, palette));
            }
        }

        let mut body = column![
            back,
            Space::new().height(Length::Fixed(16.0)),
            header,
        ]
        .spacing(8)
        .width(Length::Fill);
        if let Some(section) = suggestions_section {
            body = body.push(Space::new().height(Length::Fixed(20.0)));
            body = body.push(section);
        }
        body = body.push(Space::new().height(Length::Fixed(20.0)));
        body = body.push(menu);

        let scroller = scrollable(container(body).padding(Padding::from([24u16, 24u16])))
            .width(Length::Fill)
            .height(Length::Fill);

        container(scroller)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_t: &Theme| container::Style {
                background: Some(Background::Color(palette.background.into_cosmic_color())),
                ..container::Style::default()
            })
            .into()
    }

    /// FRONTDOOR-2 — the Win10-Start two-pane view: a fixed left **rail** beside a
    /// right **pane** (the full-width omnibox above the FRONTDOOR-1 tile grid).
    fn panel_view(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        row![self.rail(palette), self.right_pane(palette)]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// FRONTDOOR-3 — the iPadOS-home full-screen view (Q86/Q89): the rail is
    /// hidden, leaving a full-width top bar (omnibox + the back-to-panel toggle)
    /// above a full-screen rounded-icon grid. **No dock** (the lock). The grid is
    /// the same [`TileGrid`] program reused with full-screen layout params (bigger
    /// rounded icons, more columns); it scrolls rather than paging (the accepted
    /// first cut — see the module note).
    fn fullscreen_view(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        let omnibox: Element<'_, crate::Message, Theme> =
            text_input("Search apps, files, mesh, or ask Copilot…", &self.query)
                .on_input(|s| crate::Message::FrontDoor(Message::OmniboxChanged(s)))
                .padding(Padding::from([10u16, 14u16]))
                .width(Length::Fill)
                .into();

        // Top bar: the omnibox stretches; the mode toggle sits at its right.
        let top_bar = container(
            row![omnibox, self.mode_toggle(palette)]
                .spacing(12)
                .align_y(cosmic::iced::Alignment::Center),
        )
        .width(Length::Fill)
        .padding(Padding::from([16u16, 16u16]));

        // FRONTDOOR-6 — a non-empty query REPLACES the icon grid with the unified
        // search results (instant local hits + the AI card streaming in below);
        // an empty query keeps the iPadOS rounded-icon grid (FD-1..5 unchanged).
        let content: Element<'_, crate::Message, Theme> = if self.searching() {
            self.search_results_view(palette)
        } else {
            // The full-screen rounded-icon grid: the same canvas program, told to
            // lay out at the larger full-screen scale. A scrollable wrapper gives
            // the "first cut" vertical paging when icons overflow the viewport.
            scrollable(self.icon_grid())
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };

        let body = column![top_bar, content]
            .width(Length::Fill)
            .height(Length::Fill);

        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_t: &Theme| container::Style {
                background: Some(Background::Color(palette.background.into_cosmic_color())),
                ..container::Style::default()
            })
            .into()
    }

    /// The left rail (design Q5): identity → Pinned → the predominant DevOps +
    /// Data Center surfaces. Fixed width, scrollable so a short window still
    /// reaches every entry. No power control: the Front Door has no existing
    /// local power/session action to call, so §7 says omit it (better an absent
    /// control than a dead button) — the mesh-power tile is FRONTDOOR-4 data.
    fn rail(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        let sizes = FontSize::defaults();

        // Identity — the account this Front Door belongs to. A static label for
        // now (live identity is FRONTDOOR-4); rendered, not interactive.
        let account = whoami_label();
        let identity = column![
            text(account)
                .size(TypeRole::Heading.size_in(sizes))
                .colr(palette.text.into_cosmic_color()),
            text("This node")
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(2);

        // Pinned — the launchers that have a real route today. Each entry
        // navigates somewhere real (§7); we don't list a pin we can't open yet.
        let pinned = column![
            rail_section_label("Pinned", palette),
            rail_link(
                "Peers",
                crate::Message::SelectPanel {
                    group: Group::Mesh,
                    panel: "peers",
                },
                palette,
                false,
            ),
            rail_link(
                "Mesh Bus",
                crate::Message::SelectPanel {
                    group: Group::Mesh,
                    panel: "mesh_bus",
                },
                palette,
                false,
            ),
        ]
        .spacing(4);

        // The predominant surfaces (the brief: DevOps + Data Center front-and-
        // center). Rendered as accent-emphasized rail links that navigate to the
        // real `build-farm` / `datacenter` panel routes (§7).
        let surfaces = column![
            rail_section_label("Surfaces", palette),
            rail_link(
                "DevOps",
                crate::Message::SelectPanel {
                    group: Group::Provisioning,
                    panel: "build-farm",
                },
                palette,
                true,
            ),
            rail_link(
                "Data Center",
                crate::Message::SelectPanel {
                    group: Group::Provisioning,
                    panel: "datacenter",
                },
                palette,
                true,
            ),
        ]
        .spacing(4);

        let body = column![
            identity,
            Space::new().height(Length::Fixed(16.0)),
            surfaces,
            Space::new().height(Length::Fixed(16.0)),
            pinned,
        ]
        .spacing(8)
        .width(Length::Fill);

        let scroller = scrollable(container(body).padding(Padding::from([20u16, 16u16])))
            .width(Length::Fill)
            .height(Length::Fill);

        container(scroller)
            .width(Length::Fixed(RAIL_WIDTH))
            .height(Length::Fill)
            .style(move |_t: &Theme| container::Style {
                background: Some(Background::Color(palette.surface.into_cosmic_color())),
                border: Border {
                    color: palette.border.into_cosmic_color(),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..container::Style::default()
            })
            .into()
    }

    /// The right pane: the full-width omnibox (FRONTDOOR-2 render + local text;
    /// search is FRONTDOOR-6) and the FRONTDOOR-3 full-screen toggle in the top
    /// bar, above the FRONTDOOR-1 canvas tile grid.
    fn right_pane(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        let omnibox: Element<'_, crate::Message, Theme> =
            text_input("Search apps, files, mesh, or ask Copilot…", &self.query)
                .on_input(|s| crate::Message::FrontDoor(Message::OmniboxChanged(s)))
                .padding(Padding::from([10u16, 14u16]))
                .width(Length::Fill)
                .into();

        let omnibox_bar = container(
            row![omnibox, self.mode_toggle(palette)]
                .spacing(12)
                .align_y(cosmic::iced::Alignment::Center),
        )
        .width(Length::Fill)
        .padding(Padding::from([16u16, 16u16]));

        // FRONTDOOR-6 — a non-empty query REPLACES the tile grid with the unified
        // search results (instant local hits + the AI card below); an empty query
        // keeps the FD-1 tile grid (the normal Win10-Start pane, unchanged).
        let content: Element<'_, crate::Message, Theme> = if self.searching() {
            self.search_results_view(palette)
        } else {
            self.tile_grid()
        };

        let pane = column![omnibox_bar, content]
            .width(Length::Fill)
            .height(Length::Fill);

        container(pane)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_t: &Theme| container::Style {
                background: Some(Background::Color(palette.background.into_cosmic_color())),
                ..container::Style::default()
            })
            .into()
    }

    /// FRONTDOOR-6 — is the omnibox driving a search right now? True when the
    /// query is non-blank; both modes then swap their tile grid for the unified
    /// results view. A blank query is false (the normal grid shows — FD-1..5
    /// unchanged).
    fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// FRONTDOOR-6 — the unified search results, rendered BELOW the omnibox in
    /// place of the tile grid (Q20 unified scope, Q47 instant-local-then-AI):
    /// the instant LOCAL hits first (apps + mesh nodes/services, grouped by kind,
    /// AI-ranked best-first), then a distinct **AI answer card** ("thinking…"
    /// until the Copilot reply lands, the prose when it does, a quiet note on
    /// graceful degrade). Every local row activates a REAL app message (§7 — a
    /// panel route or app launch). Carbon chrome via `mde-theme` tokens only (§4).
    fn search_results_view(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        let sizes = FontSize::defaults();
        let mut body = column![].spacing(8).width(Length::Fill);

        // ── Instant LOCAL results, grouped by kind (apps → nodes → services) ──
        if self.results.is_empty() {
            body = body.push(
                text("No local matches.")
                    .size(TypeRole::Body.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            );
        } else {
            // The results are already rank-sorted; the kind order within them is
            // the tie-break, so a simple section-header-on-change walk groups them
            // without re-sorting (and keeps the global rank order across sections).
            let mut last_section: Option<&'static str> = None;
            for hit in &self.results {
                let section = hit.kind.section();
                if last_section != Some(section) {
                    body = body.push(rail_section_label(section, palette));
                    last_section = Some(section);
                }
                body = body.push(search_hit_row(hit, palette));
            }
        }

        // ── The AI answer card — a DISTINCT card below the local results ──
        if let Some(card) = self.copilot_card(palette) {
            body = body.push(Space::new().height(Length::Fixed(16.0)));
            body = body.push(card);
        }

        scrollable(container(body).padding(Padding::from([12u16, 16u16])))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// FRONTDOOR-6 — the AI answer card (Q47 — the AI answer streams in below the
    /// local results, Q33 — graceful degrade). `None` when Copilot is `Idle` (a
    /// blank query — no card). Otherwise a distinct raised card: a "Copilot"
    /// caption over either a "thinking…" placeholder, the prose answer, or a quiet
    /// one-line "unavailable" note (never an error dump). Tokens only (§4).
    fn copilot_card(&self, palette: Palette) -> Option<Element<'_, crate::Message, Theme>> {
        let sizes = FontSize::defaults();
        let (line, tone) = match &self.copilot {
            CopilotState::Idle => return None,
            CopilotState::Thinking => (
                "Thinking…".to_string(),
                palette.text_muted.into_cosmic_color(),
            ),
            CopilotState::Answer(a) => (a.clone(), palette.text.into_cosmic_color()),
            CopilotState::Unavailable => (
                "Copilot is unavailable right now — local results still apply."
                    .to_string(),
                palette.text_muted.into_cosmic_color(),
            ),
        };
        let card = column![
            text("Copilot")
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.accent.into_cosmic_color()),
            text(line).size(TypeRole::Body.size_in(sizes)).colr(tone),
        ]
        .spacing(6)
        .width(Length::Fill);

        Some(
            container(card)
                .width(Length::Fill)
                .padding(Padding::from([14u16, 16u16]))
                .style(move |_t: &Theme| container::Style {
                    background: Some(Background::Color(palette.surface.into_cosmic_color())),
                    border: Border {
                        color: palette.border.into_cosmic_color(),
                        width: 1.0,
                        radius: 8.0.into(),
                    },
                    ..container::Style::default()
                })
                .into(),
        )
    }

    /// The FRONTDOOR-1 tile grid drawn on `canvas` (GPU 2D geometry, NOT a widget
    /// tree). The program paints from the live palette (it ignores the stock theme
    /// passed to `draw`), so `themer(None, ..)` bridges the stock-themed canvas
    /// back into the surrounding cosmic theme — same pattern as Routing's path
    /// graph and the Peers map. The panel-mode right-pane tile area: the compact
    /// [`Layout::Panel`] card scale.
    fn tile_grid(&self) -> Element<'_, crate::Message, Theme> {
        self.canvas_grid(Layout::Panel, Length::Fill)
    }

    /// FRONTDOOR-3 — the iPadOS-home full-screen icon grid: the same [`TileGrid`]
    /// canvas program at the larger [`Layout::FullScreen`] scale (bigger rounded
    /// icons, more columns). Its height is the natural grid height for the tile
    /// count so the enclosing `scrollable` can page through overflow (the accepted
    /// first cut in place of true horizontal paging).
    fn icon_grid(&self) -> Element<'_, crate::Message, Theme> {
        let rows = self
            .tiles
            .len()
            .div_ceil(Layout::FullScreen.nominal_columns());
        let height = Layout::FullScreen.grid_height(rows);
        self.canvas_grid(Layout::FullScreen, Length::Fixed(height))
    }

    /// Shared canvas-grid construction for both modes: build a [`TileGrid`] at the
    /// given [`Layout`] and bridge the stock-themed canvas back into the cosmic
    /// theme via `themer(None, ..)`.
    fn canvas_grid(&self, layout: Layout, height: Length) -> Element<'_, crate::Message, Theme> {
        // FRONTDOOR-10 — the per-tile suggestion count drives the on-tile badge
        // (Q19). Computed once here off the live suggestion set so `draw` stays
        // pure geometry over an owned snapshot.
        let suggestion_counts = (0..self.tiles.len())
            .map(|i| self.suggestion_count_for(i))
            .collect();
        let program = TileGrid {
            tiles: self.tiles.clone(),
            loading: self.loading,
            palette: crate::live_theme::palette(),
            layout,
            suggestion_counts,
        };
        let canvas_stock: cosmic::iced::Element<'_, crate::Message, cosmic::iced::Theme> =
            cosmic::iced::widget::canvas(program)
                .width(Length::Fill)
                .height(height)
                .into();
        cosmic::iced::widget::themer(None, canvas_stock).into()
    }

    /// FRONTDOOR-3 — the real panel ↔ full-screen toggle button (§7 — a real
    /// control wired to [`Message::ToggleMode`], no stub). Its glyph + label name
    /// the *target* mode: in panel mode it offers "⤢ Full screen"; in full-screen
    /// it offers "⤡ Panel". Carbon chrome via tokens only (§4).
    fn mode_toggle(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        let label = match self.mode {
            Mode::Panel => "⤢ Full screen",
            Mode::FullScreen => "⤡ Panel",
        };
        let accent = palette.accent.into_cosmic_color();
        let raised = palette.raised.into_cosmic_color();
        let idle_bg = palette.hover_tint().into_cosmic_color();

        button(
            text(label)
                .size(TypeRole::Body.size_in(FontSize::defaults()))
                .colr(accent),
        )
        .padding(Padding::from([8u16, 14u16]))
        .sty(
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                use cosmic::iced::widget::button::Status;
                let bg = match status {
                    Status::Hovered | Status::Pressed => raised,
                    _ => idle_bg,
                };
                cosmic::iced::widget::button::Style {
                    snap: false,
                    background: Some(Background::Color(bg)),
                    text_color: accent,
                    border: Border {
                        color: cosmic::iced::Color::TRANSPARENT,
                        width: 0.0,
                        radius: 6.0.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                    ..cosmic::iced::widget::button::Style::default()
                }
            },
        )
        .on_press(crate::Message::FrontDoor(Message::ToggleMode))
        .into()
    }
}

/// The rail's account identity. Best-effort from the environment (`$USER`),
/// falling back to a neutral label — no probe in `view()`. Live identity is
/// FRONTDOOR-4.
fn whoami_label() -> String {
    std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "Account".to_string())
}

/// A rail section header (Pinned / Surfaces), muted + caption-sized.
fn rail_section_label<'a>(label: &'a str, palette: Palette) -> Element<'a, crate::Message, Theme> {
    text(label)
        .size(TypeRole::Caption.size_in(FontSize::defaults()))
        .colr(palette.text_muted.into_cosmic_color())
        .into()
}

/// A full-width rail link. `emphasized` marks the predominant DevOps / Data
/// Center surfaces (design Q5): an accent-tinted fill + accent text so they read
/// front-and-center; ordinary pins read as quiet ghost rows. Every link carries a
/// REAL `on_press` route (§7 — no dead buttons).
fn rail_link<'a>(
    label: &'a str,
    msg: crate::Message,
    palette: Palette,
    emphasized: bool,
) -> Element<'a, crate::Message, Theme> {
    let accent = palette.accent.into_cosmic_color();
    let fg = if emphasized {
        accent
    } else {
        palette.text.into_cosmic_color()
    };
    let raised = palette.raised.into_cosmic_color();
    let hover_tint = palette.hover_tint().into_cosmic_color();
    let idle_bg = if emphasized {
        hover_tint
    } else {
        cosmic::iced::Color::TRANSPARENT
    };

    button(
        text(label)
            .size(TypeRole::Body.size_in(FontSize::defaults()))
            .colr(fg),
    )
    .width(Length::Fill)
    .padding(Padding::from([8u16, 12u16]))
    .sty(
        move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
            use cosmic::iced::widget::button::Status;
            let bg = match status {
                Status::Hovered | Status::Pressed => {
                    if emphasized {
                        accent_tint(accent)
                    } else {
                        raised
                    }
                }
                _ => idle_bg,
            };
            cosmic::iced::widget::button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: fg,
                border: Border {
                    color: cosmic::iced::Color::TRANSPARENT,
                    width: 0.0,
                    radius: 6.0.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
                ..cosmic::iced::widget::button::Style::default()
            }
        },
    )
    .on_press(msg)
    .into()
}

/// A stronger accent wash for an emphasized rail link's hover/press — the accent
/// at low alpha, so the row lifts without flipping to a full accent fill.
fn accent_tint(accent: cosmic::iced::Color) -> cosmic::iced::Color {
    cosmic::iced::Color { a: 0.28, ..accent }
}

/// FRONTDOOR-5 — one full-width row in a tile's detail actions menu. Carries the
/// action's REAL `on_press` message (a panel navigation or app launch — §7, never
/// a stub). Styled like an emphasized rail link (accent text + a quiet idle wash
/// that lifts on hover), so the menu reads as a list of live, clickable actions.
fn detail_action_row<'a>(
    action: TileAction,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let accent = palette.accent.into_cosmic_color();
    let idle_bg = palette.hover_tint().into_cosmic_color();

    button(
        text(action.label)
            .size(TypeRole::Body.size_in(FontSize::defaults()))
            .colr(accent),
    )
    .width(Length::Fill)
    .padding(Padding::from([10u16, 14u16]))
    .sty(
        move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
            use cosmic::iced::widget::button::Status;
            let bg = match status {
                Status::Hovered | Status::Pressed => accent_tint(accent),
                _ => idle_bg,
            };
            cosmic::iced::widget::button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: accent,
                border: Border {
                    color: cosmic::iced::Color::TRANSPARENT,
                    width: 0.0,
                    radius: 6.0.into(),
                },
                shadow: cosmic::iced::Shadow::default(),
                ..cosmic::iced::widget::button::Style::default()
            }
        },
    )
    .on_press(action.message)
    .into()
}

/// FRONTDOOR-10 — one proactive-suggestion card in a tile's detail view (Q19 —
/// the suggestion text, rendered inline on the tile it concerns). The title over a
/// muted impact + rationale, in a raised card; when the suggestion carries a typed
/// proposal, an **Act** button publishes it to the propose queue
/// ([`Message::ProposeSuggestion`] → `action/copilot/proposal`). §9: Act PROPOSES,
/// it does not execute — an advisory-only suggestion shows no Act button. `gi` is
/// the suggestion's index into [`FrontDoor::suggestions`] so the message addresses
/// the exact one regardless of per-tile filtering. Tokens only (§4).
fn suggestion_card<'a>(
    gi: usize,
    s: &copilot::Suggestion,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let sizes = FontSize::defaults();
    let accent = palette.accent.into_cosmic_color();
    // High-impact reads warning-toned, medium reads muted — the operator's eye goes
    // to the urgent ones first (§4 — token, never hex).
    let impact_tone = if s.impact == "high" {
        palette.warning.into_cosmic_color()
    } else {
        palette.text_muted.into_cosmic_color()
    };

    let mut card = column![
        text(s.title.clone())
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text.into_cosmic_color()),
        text(format!("{} impact", s.impact))
            .size(TypeRole::Caption.size_in(sizes))
            .colr(impact_tone),
    ]
    .spacing(4)
    .width(Length::Fill);

    if !s.detail.trim().is_empty() {
        card = card.push(
            text(s.detail.clone())
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        );
    }

    // §9 — the "Act" affordance ONLY when the suggestion carries a typed proposal.
    // It PROPOSES (re-publishes to the propose queue), never executes; the gated
    // confirm/exec surface that drains that queue is separate + out of scope.
    if s.proposal_body.is_some() {
        let idle_bg = palette.hover_tint().into_cosmic_color();
        let act = button(
            text("Act — queue this proposal for approval")
                .size(TypeRole::Caption.size_in(sizes))
                .colr(accent),
        )
        .padding(Padding::from([8u16, 12u16]))
        .sty(
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                use cosmic::iced::widget::button::Status;
                let bg = match status {
                    Status::Hovered | Status::Pressed => accent_tint(accent),
                    _ => idle_bg,
                };
                cosmic::iced::widget::button::Style {
                    snap: false,
                    background: Some(Background::Color(bg)),
                    text_color: accent,
                    border: Border {
                        color: cosmic::iced::Color::TRANSPARENT,
                        width: 0.0,
                        radius: 6.0.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                    ..cosmic::iced::widget::button::Style::default()
                }
            },
        )
        .on_press(crate::Message::FrontDoor(Message::ProposeSuggestion(gi)));
        card = card.push(act);
    }

    container(card)
        .width(Length::Fill)
        .padding(Padding::from([12u16, 14u16]))
        .style(move |_t: &Theme| container::Style {
            background: Some(Background::Color(palette.raised.into_cosmic_color())),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: 8.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// FRONTDOOR-6 — one full-width row in the unified search results list. The
/// primary label (the matched app / node / service) over a muted context line
/// (the node a service runs on / the surface a panel lives under). Clicking it
/// activates the hit's REAL app message via [`Message::SearchHitActivated`] (a
/// panel route or app launch — §7, never an inert row). Styled like the detail
/// action rows (a quiet idle wash that lifts on hover). Tokens only (§4).
fn search_hit_row<'a>(hit: &SearchHit, palette: Palette) -> Element<'a, crate::Message, Theme> {
    let sizes = FontSize::defaults();
    let accent = palette.accent.into_cosmic_color();
    let idle_bg = palette.hover_tint().into_cosmic_color();

    let label_block = column![
        text(hit.label.clone())
            .size(TypeRole::Body.size_in(sizes))
            .colr(accent),
        text(hit.context.clone())
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color()),
    ]
    .spacing(2);

    button(label_block)
        .width(Length::Fill)
        .padding(Padding::from([10u16, 14u16]))
        .sty(
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                use cosmic::iced::widget::button::Status;
                let bg = match status {
                    Status::Hovered | Status::Pressed => accent_tint(accent),
                    _ => idle_bg,
                };
                cosmic::iced::widget::button::Style {
                    snap: false,
                    background: Some(Background::Color(bg)),
                    text_color: accent,
                    border: Border {
                        color: cosmic::iced::Color::TRANSPARENT,
                        width: 0.0,
                        radius: 6.0.into(),
                    },
                    shadow: cosmic::iced::Shadow::default(),
                    ..cosmic::iced::widget::button::Style::default()
                }
            },
        )
        .on_press(crate::Message::FrontDoor(Message::SearchHitActivated(
            Box::new(hit.message.clone()),
        )))
        .into()
}

/// The accent strip down the left edge of a card. Mode-independent (it's a hair
/// of color, not a sized element).
const STRIP_W: f32 = 5.0;

/// FRONTDOOR-10 — the on-tile suggestion badge's radius (a small accent dot in the
/// card's top-right carrying the count — Q19). Mode-independent (a small fixed pip).
const BADGE_R: f32 = 9.0;

/// FRONTDOOR-3 — the snap-grid metrics for each render mode. FRONTDOOR-1's panel
/// numbers (~180 px comfortable-density cards, Q80) become [`Layout::Panel`];
/// [`Layout::FullScreen`] scales up to the iPadOS-home aesthetic — bigger, more
/// rounded "icon" tiles laid out with more columns and breathing room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Win10-Start panel cards: compact, lightly rounded.
    Panel,
    /// iPadOS-home full-screen icons: large, heavily rounded.
    FullScreen,
}

impl Layout {
    /// Tile width.
    fn tile_w(self) -> f32 {
        match self {
            Layout::Panel => 180.0,
            Layout::FullScreen => 220.0,
        }
    }

    /// Tile height.
    fn tile_h(self) -> f32 {
        match self {
            Layout::Panel => 96.0,
            Layout::FullScreen => 140.0,
        }
    }

    /// Inter-tile gap.
    fn gap(self) -> f32 {
        match self {
            Layout::Panel => 12.0,
            Layout::FullScreen => 28.0,
        }
    }

    /// Outer page padding.
    fn pad(self) -> f32 {
        match self {
            Layout::Panel => 16.0,
            Layout::FullScreen => 40.0,
        }
    }

    /// Corner radius — the full-screen tiles read as rounded "icons", so they
    /// round much harder than the lightly-rounded panel cards.
    fn radius(self) -> f32 {
        match self {
            Layout::Panel => 8.0,
            Layout::FullScreen => 28.0,
        }
    }

    /// The label point size for this scale.
    fn label_size(self) -> f32 {
        match self {
            Layout::Panel => 14.0,
            Layout::FullScreen => 18.0,
        }
    }

    /// A nominal column count used to pre-size the full-screen scroll area before
    /// the canvas knows its true width (the canvas itself recomputes columns from
    /// the real `bounds.width` at draw time).
    fn nominal_columns(self) -> usize {
        match self {
            Layout::Panel => 5,
            Layout::FullScreen => 6,
        }
    }

    /// The natural pixel height of a grid with `rows` rows at this scale — used to
    /// give the full-screen `scrollable` a content height it can page through.
    fn grid_height(self, rows: usize) -> f32 {
        let rows = rows.max(1) as f32;
        2.0 * self.pad() + rows * self.tile_h() + (rows - 1.0).max(0.0) * self.gap()
    }
}

/// The canvas program that draws the tile grid. Holds an owned snapshot of the
/// tiles + the live palette so `draw` is pure geometry (no global reads mid-paint).
/// FRONTDOOR-3 — `layout` selects the panel vs. full-screen scale.
#[derive(Debug)]
pub struct TileGrid {
    tiles: Vec<Tile>,
    loading: bool,
    palette: Palette,
    layout: Layout,
    /// FRONTDOOR-10 — per-tile proactive-suggestion count (parallel to `tiles`),
    /// painted as a small accent badge in the card's top-right (Q19 — the on-tile
    /// suggestion indicator). 0 ⇒ no badge.
    suggestion_counts: Vec<usize>,
}

impl TileGrid {
    /// Columns that fit in `width` at the given layout's tile size, clamped to at
    /// least one so a narrow panel still renders a single column.
    fn columns(width: f32, layout: Layout) -> usize {
        let (tile_w, gap, pad) = (layout.tile_w(), layout.gap(), layout.pad());
        let usable = (width - 2.0 * pad + gap).max(tile_w);
        ((usable / (tile_w + gap)).floor() as usize).max(1)
    }

    /// The top-left corner of tile `i` for a grid of `cols` columns at `layout`.
    fn tile_origin(i: usize, cols: usize, layout: Layout) -> Point {
        let (tile_w, tile_h, gap, pad) =
            (layout.tile_w(), layout.tile_h(), layout.gap(), layout.pad());
        let col = i % cols;
        let row = i / cols;
        Point::new(
            pad + col as f32 * (tile_w + gap),
            pad + row as f32 * (tile_h + gap),
        )
    }

    /// FRONTDOOR-5 — hit-test a canvas-local point against the tile rects, using
    /// the **same** column/origin/size math `draw` lays out with (so a click
    /// lands on exactly the card the operator sees). `width` is the canvas's real
    /// `bounds.width`, which drives the column count. Returns the index of the
    /// tile under `pos`, or `None` for a click in the gutter / padding / past the
    /// last tile. Pure + `width`-parameterized so it's unit-tested without a live
    /// canvas.
    fn tile_at(&self, pos: Point, width: f32) -> Option<usize> {
        let layout = self.layout;
        let (tile_w, tile_h) = (layout.tile_w(), layout.tile_h());
        let cols = Self::columns(width, layout);
        for i in 0..self.tiles.len() {
            let o = Self::tile_origin(i, cols, layout);
            if pos.x >= o.x && pos.x <= o.x + tile_w && pos.y >= o.y && pos.y <= o.y + tile_h {
                return Some(i);
            }
        }
        None
    }
}

impl canvas::Program<crate::Message> for TileGrid {
    type State = ();

    /// FRONTDOOR-5 — a left-click on a tile opens that tile's detail actions
    /// menu (Q45). Hit-tests the cursor against the same tile rects `draw` paints
    /// (via [`TileGrid::tile_at`], driven by the canvas's real `bounds.width`) and
    /// publishes [`Message::TileActivated`] with the clicked index. A miss (a
    /// click in the gutter or while the grid is a skeleton) is ignored. Mirrors
    /// the Peers-map canvas click handler's shape (same `position_in` + publish).
    fn update(
        &self,
        _state: &mut Self::State,
        event: &cosmic::iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<crate::Message>> {
        // Skeleton cards aren't interactive (no real tile to detail yet).
        if self.loading {
            return None;
        }
        if let cosmic::iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event
        {
            let pos = cursor.position_in(bounds)?;
            let i = self.tile_at(pos, bounds.width)?;
            return Some(canvas::Action::publish(crate::Message::FrontDoor(
                Message::TileActivated(i),
            )));
        }
        None
    }

    /// FRONTDOOR-5 — show the pointer cursor while hovering a tile, so the grid
    /// reads as clickable (the affordance for the Q45 click→detail). A skeleton
    /// grid or a cursor in the gutter keeps the default arrow.
    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if !self.loading {
            if let Some(pos) = cursor.position_in(bounds) {
                if self.tile_at(pos, bounds.width).is_some() {
                    return mouse::Interaction::Pointer;
                }
            }
        }
        mouse::Interaction::default()
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &cosmic::iced::Renderer,
        _theme: &cosmic::iced::Theme,
        bounds: cosmic::iced::Rectangle,
        _cursor: cosmic::iced::mouse::Cursor,
    ) -> Vec<cosmic::iced::widget::canvas::Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let p = &self.palette;

        // Carbon page background under the cards.
        frame.fill(
            &Path::rectangle(Point::ORIGIN, bounds.size()),
            p.background.into_cosmic_color(),
        );

        let layout = self.layout;
        let (tile_w, tile_h, radius, label_size) = (
            layout.tile_w(),
            layout.tile_h(),
            layout.radius(),
            layout.label_size(),
        );
        let cols = Self::columns(bounds.width, layout);
        let card_size = Size::new(tile_w, tile_h);
        let surface = p.surface.into_cosmic_color();
        // Skeleton fill: the raised surface token, a touch above `surface`, so a
        // loading card reads as a flat grey placeholder (no label, no strip).
        let skeleton = p.raised.into_cosmic_color();

        for (i, tile) in self.tiles.iter().enumerate() {
            let origin = Self::tile_origin(i, cols, layout);

            if self.loading {
                // Flat grey skeleton rounded-rect — Q92, no layout shift.
                frame.fill(
                    &Path::rounded_rectangle(origin, card_size, radius.into()),
                    skeleton,
                );
                continue;
            }

            // The card surface.
            frame.fill(
                &Path::rounded_rectangle(origin, card_size, radius.into()),
                surface,
            );

            // The tone-colored accent strip down the card's left edge.
            let strip_origin = Point::new(origin.x, origin.y);
            frame.fill(
                &Path::rounded_rectangle(strip_origin, Size::new(STRIP_W, tile_h), radius.into()),
                tile.tone.color(p),
            );

            // FRONTDOOR-4 — a widget tile with live data draws its metric line
            // under the label; a launcher (or a widget with no source this round)
            // draws the label centered as before. Nudge the label up a hair when
            // there's a value so the two lines straddle the card center.
            let cx = origin.x + tile_w / 2.0;
            let cy = origin.y + tile_h / 2.0;
            let value_size = (label_size - 2.0).max(10.0);

            if let Some(value) = &tile.value {
                // Label (slightly above center) + the live metric below it, the
                // metric tinted with the tile's live tone so "2 alerts" reads red
                // and "5/5 healthy" reads green (§4 — token color, never hex).
                frame.fill_text(Text {
                    content: tile.label.clone(),
                    position: Point::new(cx, cy - label_size),
                    color: p.text.into_cosmic_color(),
                    size: Pixels(label_size),
                    align_x: Alignment::Center,
                    ..Text::default()
                });
                frame.fill_text(Text {
                    content: value.clone(),
                    position: Point::new(cx, cy + 2.0),
                    color: tile.tone.color(p),
                    size: Pixels(value_size),
                    align_x: Alignment::Center,
                    ..Text::default()
                });
            } else {
                // The centered label (launchers + not-yet-loaded widgets).
                frame.fill_text(Text {
                    content: tile.label.clone(),
                    position: Point::new(cx, cy - 7.0),
                    color: p.text.into_cosmic_color(),
                    size: Pixels(label_size),
                    align_x: Alignment::Center,
                    ..Text::default()
                });
            }

            // FRONTDOOR-10 — the proactive-suggestion badge (Q19 — the on-tile
            // indicator a suggestion concerns THIS tile): a small accent pip in the
            // top-right corner carrying the count. The detail view holds the card
            // text + the §9-safe "Act"; the badge is just the affordance to look.
            let count = self.suggestion_counts.get(i).copied().unwrap_or(0);
            if count > 0 {
                let bcx = origin.x + tile_w - BADGE_R - 6.0;
                let bcy = origin.y + BADGE_R + 6.0;
                frame.fill(
                    &Path::circle(Point::new(bcx, bcy), BADGE_R),
                    p.accent.into_cosmic_color(),
                );
                frame.fill_text(Text {
                    content: count.to_string(),
                    position: Point::new(bcx, bcy - BADGE_R + 1.0),
                    color: p.background.into_cosmic_color(),
                    size: Pixels(BADGE_R + 3.0),
                    align_x: Alignment::Center,
                    ..Text::default()
                });
            }
        }

        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_seeds_the_design_widget_set() {
        // FRONTDOOR-1 — the home seeds ~12 tiles covering the Q99 widget set + a
        // few launchers. FRONTDOOR-2 — the omnibox query starts empty.
        // FRONTDOOR-4 — it starts in the skeleton state (`loading`), cleared by
        // the first snapshot.
        let fd = FrontDoor::new();
        assert_eq!(fd.tiles.len(), 12);
        assert!(fd.loading, "FRONTDOOR-4 starts on the skeleton");
        assert!(fd.query.is_empty());
        // The design's named widgets are present.
        for want in ["Mesh Map", "Build / Farm", "Alerts", "Copilot", "System"] {
            assert!(
                fd.tiles.iter().any(|t| t.label == want),
                "missing widget tile: {want}"
            );
        }
    }

    #[test]
    fn widget_tiles_carry_keys_and_launchers_do_not() {
        // FRONTDOOR-4 — the design widgets are keyed (they take live data); the
        // app launchers are keyless (no source → no faked value). FRONTDOOR-10 —
        // Copilot is now a KEYED widget too (its `state/copilot/status` topic).
        let fd = FrontDoor::new();
        let key_of = |label: &str| fd.tiles.iter().find(|t| t.label == label).map(|t| t.key);
        assert_eq!(key_of("Mesh Map"), Some(Some(TileKey::MeshMap)));
        assert_eq!(key_of("Node Health"), Some(Some(TileKey::NodeHealth)));
        assert_eq!(key_of("Alerts"), Some(Some(TileKey::Alerts)));
        assert_eq!(key_of("Build / Farm"), Some(Some(TileKey::BuildFarm)));
        assert_eq!(key_of("System"), Some(Some(TileKey::System)));
        // FRONTDOOR-10 — Copilot is keyed now (live status topic exists).
        assert_eq!(key_of("Copilot"), Some(Some(TileKey::Copilot)));
        assert_eq!(key_of("Files"), Some(None));
        assert_eq!(key_of("Terminal"), Some(None));
        // Every seeded widget tile starts with no value (the skeleton covers it).
        assert!(fd.tiles.iter().all(|t| t.value.is_none()));
    }

    #[test]
    fn columns_fit_the_width_and_never_zero() {
        // A wide panel packs several columns; a sliver still renders one. The
        // larger full-screen tiles pack fewer columns than the panel cards at the
        // same width (Layout::FullScreen is the bigger scale).
        assert!(TileGrid::columns(1200.0, Layout::Panel) >= 5);
        assert_eq!(TileGrid::columns(10.0, Layout::Panel), 1);
        assert_eq!(TileGrid::columns(0.0, Layout::Panel), 1);
        assert_eq!(TileGrid::columns(10.0, Layout::FullScreen), 1);
        assert!(
            TileGrid::columns(1600.0, Layout::FullScreen)
                <= TileGrid::columns(1600.0, Layout::Panel),
            "full-screen icons are larger, so fewer fit a given width"
        );
    }

    #[test]
    fn omnibox_change_records_the_query_and_drives_search() {
        // FRONTDOOR-2/6 — the omnibox is a controlled field that now DRIVES the
        // unified search: a text-change records the text AND recomputes the
        // instant local results + parks Copilot at "thinking…" (Q47); clearing it
        // restores the grid state (empty results, Copilot Idle).
        let mut fd = FrontDoor::new();
        let _ = fd.update(Message::OmniboxChanged("build farm".to_string()));
        assert_eq!(fd.query, "build farm");
        // The query matched a real surface (the Build Farm panel) — local hits.
        assert!(
            !fd.results.is_empty(),
            "a real query yields instant local hits"
        );
        assert_eq!(fd.copilot, CopilotState::Thinking, "the AI card is pending");
        // Each search keystroke bumps the generation so a stale reply is dropped.
        assert_eq!(fd.copilot_gen, 1);
        let _ = fd.update(Message::OmniboxChanged("build".to_string()));
        assert_eq!(fd.copilot_gen, 2);

        // Clearing the query restores the grid state.
        let _ = fd.update(Message::OmniboxChanged(String::new()));
        assert!(fd.query.is_empty());
        assert!(fd.results.is_empty(), "a blank query clears results");
        assert_eq!(fd.copilot, CopilotState::Idle, "and parks the AI card");
    }

    #[test]
    fn toggle_mode_flips_between_panel_and_fullscreen() {
        // FRONTDOOR-3 — the Front Door defaults to the panel; the toggle flips it
        // to full-screen and back (the real handler behind the top-bar button).
        let mut fd = FrontDoor::new();
        assert_eq!(fd.mode, Mode::Panel);
        let _ = fd.update(Message::ToggleMode);
        assert_eq!(fd.mode, Mode::FullScreen);
        let _ = fd.update(Message::ToggleMode);
        assert_eq!(fd.mode, Mode::Panel);
    }

    #[test]
    fn both_modes_view_constructs() {
        // FRONTDOOR-2/3 — both the two-pane panel view and the iPadOS full-screen
        // view (rail hidden + larger icon grid) build without panicking, in both
        // the loading and loaded states.
        let mut fd = FrontDoor::new();
        let _: Element<'_, crate::Message, Theme> = fd.view();
        fd.loading = true;
        let _: Element<'_, crate::Message, Theme> = fd.view();

        fd.mode = Mode::FullScreen;
        fd.loading = false;
        let _: Element<'_, crate::Message, Theme> = fd.view();
        fd.loading = true;
        let _: Element<'_, crate::Message, Theme> = fd.view();

        // FRONTDOOR-6 — the search-results surface (local hits + the AI card)
        // builds in BOTH modes, in every Copilot state, without panicking.
        fd.loading = false;
        for mode in [Mode::Panel, Mode::FullScreen] {
            fd.mode = mode;
            let _ = fd.update(Message::OmniboxChanged("mesh".to_string()));
            for state in [
                CopilotState::Thinking,
                CopilotState::Answer("restart mfsmaster on anvil".into()),
                CopilotState::Unavailable,
            ] {
                fd.copilot = state;
                let _: Element<'_, crate::Message, Theme> = fd.view();
            }
        }
    }

    #[test]
    fn tile_origins_advance_by_row_and_column() {
        // Tile 0 sits at the pad; the next column steps right by tile+gap; the
        // first tile of the second row steps down by tile+gap. Checked at the
        // panel scale.
        let l = Layout::Panel;
        let (pad, tile_w, tile_h, gap) = (l.pad(), l.tile_w(), l.tile_h(), l.gap());
        let o0 = TileGrid::tile_origin(0, 3, l);
        assert!((o0.x - pad).abs() < f32::EPSILON);
        assert!((o0.y - pad).abs() < f32::EPSILON);
        let o1 = TileGrid::tile_origin(1, 3, l);
        assert!((o1.x - (pad + tile_w + gap)).abs() < f32::EPSILON);
        assert!((o1.y - pad).abs() < f32::EPSILON);
        let o3 = TileGrid::tile_origin(3, 3, l);
        assert!((o3.x - pad).abs() < f32::EPSILON);
        assert!((o3.y - (pad + tile_h + gap)).abs() < f32::EPSILON);
    }

    #[test]
    fn fullscreen_icons_are_bigger_and_rounder_than_panel_cards() {
        // FRONTDOOR-3 — the iPadOS full-screen scale is the larger, more rounded
        // "icon" aesthetic: bigger tiles and a harder corner radius than the
        // Win10-Start panel cards.
        assert!(Layout::FullScreen.tile_w() > Layout::Panel.tile_w());
        assert!(Layout::FullScreen.tile_h() > Layout::Panel.tile_h());
        assert!(Layout::FullScreen.radius() > Layout::Panel.radius());
    }

    // ── FRONTDOOR-4: the data-mapping (DoD — "tests for the data-mapping") ──

    use crate::panels::build_farm::{FarmJobRow, FarmSnapshot, TestTierRow, TierOutcome};
    use crate::panels::datacenter::HealthCheck;
    use crate::panels::home::BootReadiness;
    use crate::panels::peers::PeerRow;

    /// A minimal `PeerRow` with just the presence/health fields the projections
    /// read; the rest default (the projections ignore them).
    fn peer(presence: &str, health: &str) -> PeerRow {
        PeerRow {
            hostname: "h".into(),
            presence: presence.into(),
            health: health.into(),
            version: String::new(),
            overlay_ip: String::new(),
            role: String::new(),
            currency: String::new(),
            last_seen_ms: 0,
            tags: Vec::new(),
            services: Vec::new(),
            ssh: false,
            rdp: false,
            vnc: false,
            lan_macs: Vec::new(),
            containers: Vec::new(),
            vms: Vec::new(),
        }
    }

    fn check(check: &str, status: &str) -> HealthCheck {
        HealthCheck {
            check: check.into(),
            status: status.into(),
            detail: String::new(),
        }
    }

    fn done_job(outcome: &str) -> FarmJobRow {
        FarmJobRow {
            jobid: "j".into(),
            phase: "done".into(),
            outcome: outcome.into(),
        }
    }

    fn tier(outcome: TierOutcome) -> TestTierRow {
        TestTierRow {
            tier: "install".into(),
            label: "L1".into(),
            outcome,
        }
    }

    #[test]
    fn mesh_map_and_node_health_count_presence_and_health() {
        // Empty roster → no honest metric (None), not "0/0".
        assert!(project::mesh_map(&[]).is_none());
        assert!(project::node_health(&[]).is_none());

        let rows = vec![
            peer("online", "healthy"),
            peer("online", "healthy"),
            peer("offline", "degraded"),
        ];
        let (mm_val, mm_tone) = project::mesh_map(&rows).unwrap();
        assert_eq!(mm_val, "2/3 online");
        assert_eq!(mm_tone, TileTone::Warning); // not all online

        let (nh_val, nh_tone) = project::node_health(&rows).unwrap();
        assert_eq!(nh_val, "2/3 healthy");
        assert_eq!(nh_tone, TileTone::Danger); // a degraded node

        // All healthy + online → success.
        let allgood = vec![peer("online", "healthy")];
        assert_eq!(project::mesh_map(&allgood).unwrap().1, TileTone::Success);
        assert_eq!(project::node_health(&allgood).unwrap().1, TileTone::Success);
    }

    #[test]
    fn data_center_counts_nodes_with_correct_pluralization() {
        assert!(project::data_center(&[]).is_none());
        assert_eq!(
            project::data_center(&[peer("online", "healthy")])
                .unwrap()
                .0,
            "1 node"
        );
        assert_eq!(
            project::data_center(&[peer("online", "healthy"), peer("idle", "healthy")])
                .unwrap()
                .0,
            "2 nodes"
        );
    }

    #[test]
    fn build_farm_verdict_prefers_failures_then_pass_then_queued() {
        // No activity at all → no metric.
        let empty = FarmSnapshot::default();
        assert!(project::build_farm(&empty).is_none());

        // A failing tier → red regardless of anything else.
        let red = FarmSnapshot {
            jobs: vec![done_job("pass")],
            tiers: vec![tier(TierOutcome::Fail), tier(TierOutcome::Pass)],
        };
        let (v, t) = project::build_farm(&red).unwrap();
        assert_eq!(v, "build: red");
        assert_eq!(t, TileTone::Danger);

        // All-passing tier, no failures → green.
        let green = FarmSnapshot {
            jobs: vec![done_job("pass")],
            tiers: vec![tier(TierOutcome::Pass)],
        };
        assert_eq!(project::build_farm(&green).unwrap().1, TileTone::Success);

        // Jobs present but no tier verdict yet → queued/amber.
        let queued = FarmSnapshot {
            jobs: vec![FarmJobRow {
                jobid: "q".into(),
                phase: "queued".into(),
                outcome: String::new(),
            }],
            tiers: vec![tier(TierOutcome::NoRuns)],
        };
        assert_eq!(project::build_farm(&queued).unwrap().1, TileTone::Warning);
    }

    #[test]
    fn dev_ops_counts_in_flight_jobs() {
        assert!(project::dev_ops(&FarmSnapshot::default()).is_none());
        let snap = FarmSnapshot {
            jobs: vec![
                done_job("pass"),
                FarmJobRow {
                    jobid: "r".into(),
                    phase: "queued".into(),
                    outcome: String::new(),
                },
            ],
            tiers: Vec::new(),
        };
        let (v, t) = project::dev_ops(&snap).unwrap();
        assert_eq!(v, "1 running");
        assert_eq!(t, TileTone::Accent);
    }

    #[test]
    fn alerts_count_non_ok_health_checks() {
        // No checks reporting → no count (not a fake "0 alerts").
        assert!(project::alerts(&[]).is_none());

        // All ok → all clear / success.
        let clear = vec![check("bus", "ok"), check("dom0", "ok")];
        let (v, t) = project::alerts(&clear).unwrap();
        assert_eq!(v, "all clear");
        assert_eq!(t, TileTone::Success);

        // A warn + a fail → 2 alerts / danger.
        let firing = vec![
            check("bus", "ok"),
            check("dom0", "warn"),
            check("doctl", "fail"),
        ];
        let (v, t) = project::alerts(&firing).unwrap();
        assert_eq!(v, "2 alerts");
        assert_eq!(t, TileTone::Danger);
    }

    #[test]
    fn system_reads_boot_readiness_and_is_always_present() {
        let ready = BootReadiness {
            ready: true,
            ..BootReadiness::default()
        };
        let (v, t) = project::system(&ready);
        assert!(v.starts_with("ready · v"));
        assert_eq!(t, TileTone::Success);

        let booting = BootReadiness::default();
        let (v, t) = project::system(&booting);
        assert!(v.starts_with("booting · v"));
        assert_eq!(t, TileTone::Warning);
    }

    #[test]
    fn loaded_folds_the_snapshot_into_widget_tiles_and_clears_skeleton() {
        // The end-to-end fold: a snapshot lands → keyed widget tiles take their
        // value+tone, launchers stay untouched, and the skeleton lifts (Q92).
        let mut fd = FrontDoor::new();
        assert!(fd.loading);

        let data = FrontDoorData {
            node_health: Some(("5/5 healthy".into(), TileTone::Success)),
            alerts: Some(("2 alerts".into(), TileTone::Danger)),
            ..FrontDoorData::default()
        };
        let _ = fd.update(Message::Loaded(Box::new(data)));

        assert!(!fd.loading, "first snapshot clears the skeleton");

        let tile = |label: &str| fd.tiles.iter().find(|t| t.label == label).unwrap();
        assert_eq!(tile("Node Health").value.as_deref(), Some("5/5 healthy"));
        assert_eq!(tile("Node Health").tone, TileTone::Success);
        assert_eq!(tile("Alerts").value.as_deref(), Some("2 alerts"));
        assert_eq!(tile("Alerts").tone, TileTone::Danger);
        // A keyed widget with no data this round stays value-less (no fake).
        assert!(tile("System").value.is_none());
        // FRONTDOOR-10 — Copilot is a keyed widget; with no status in THIS snapshot
        // it stays value-less (the skeleton/resting state), never a fake (§7).
        assert!(tile("Copilot").value.is_none());
        assert_eq!(tile("Copilot").key, Some(TileKey::Copilot));
    }

    #[test]
    fn apply_clears_a_stale_value_when_the_source_goes_away() {
        // If a widget had a value and the next snapshot has none for it, the tile
        // drops the value rather than showing a phantom metric.
        let mut fd = FrontDoor::new();
        let with = FrontDoorData {
            node_health: Some(("5/5 healthy".into(), TileTone::Success)),
            ..FrontDoorData::default()
        };
        fd.apply(&with);
        assert!(fd
            .tiles
            .iter()
            .any(|t| t.key == Some(TileKey::NodeHealth) && t.value.is_some()));

        fd.apply(&FrontDoorData::default());
        assert!(fd
            .tiles
            .iter()
            .find(|t| t.key == Some(TileKey::NodeHealth))
            .unwrap()
            .value
            .is_none());
    }

    #[test]
    fn copilot_for_key_reads_the_status_when_present_and_is_none_otherwise() {
        // FRONTDOOR-10 — Copilot now has a workbench source (`state/copilot/status`).
        // With no status in the snapshot it's `None` (the skeleton holds, no fake —
        // §7); with one it projects the live ready/thinking/offline value+tone.
        let empty = FrontDoorData {
            node_health: Some(("1/1 healthy".into(), TileTone::Success)),
            ..FrontDoorData::default()
        };
        assert!(empty.for_key(TileKey::Copilot).is_none());

        let live = FrontDoorData {
            copilot: Some(("ready".into(), TileTone::Success)),
            ..FrontDoorData::default()
        };
        assert_eq!(
            live.for_key(TileKey::Copilot),
            Some(("ready".to_string(), TileTone::Success))
        );
    }

    // ── FRONTDOOR-5: tile click → detail actions menu ──

    /// A `TileGrid` over the seeded tiles at the given layout, in the loaded
    /// (interactive) state — the shape the canvas sees once data has landed.
    fn grid(layout: Layout) -> TileGrid {
        let tiles = FrontDoor::new().tiles;
        let suggestion_counts = vec![0; tiles.len()];
        TileGrid {
            tiles,
            loading: false,
            palette: Palette::dark(),
            layout,
            suggestion_counts,
        }
    }

    #[test]
    fn tile_at_hits_the_card_under_the_cursor_and_misses_the_gutter() {
        // FRONTDOOR-5 — the hit-test reuses the SAME origin/size/column math
        // `draw` lays out with, so a point inside a card's rect resolves to that
        // card's index and a point in the inter-tile gutter resolves to nothing.
        let g = grid(Layout::Panel);
        let l = Layout::Panel;
        let width = 1200.0;
        let cols = TileGrid::columns(width, l);

        // The center of tile 0 hits tile 0.
        let o0 = TileGrid::tile_origin(0, cols, l);
        let c0 = Point::new(o0.x + l.tile_w() / 2.0, o0.y + l.tile_h() / 2.0);
        assert_eq!(g.tile_at(c0, width), Some(0));

        // The center of tile 1 (next column over) hits tile 1.
        let o1 = TileGrid::tile_origin(1, cols, l);
        let c1 = Point::new(o1.x + l.tile_w() / 2.0, o1.y + l.tile_h() / 2.0);
        assert_eq!(g.tile_at(c1, width), Some(1));

        // The gap between tile 0 and tile 1 is gutter → no hit.
        let gutter = Point::new(o0.x + l.tile_w() + l.gap() / 2.0, c0.y);
        assert_eq!(g.tile_at(gutter, width), None);

        // The outer padding (above the first row) is no hit.
        assert_eq!(g.tile_at(Point::new(c0.x, l.pad() / 2.0), width), None);

        // A point past the last tile is no hit.
        let last = g.tiles.len() - 1;
        let ol = TileGrid::tile_origin(last, cols, l);
        let beyond = Point::new(ol.x + l.tile_w() / 2.0, ol.y + l.tile_h() + l.gap() + 5.0);
        assert_eq!(g.tile_at(beyond, width), None);
    }

    #[test]
    fn tile_activated_opens_the_detail_and_close_returns_to_grid() {
        // FRONTDOOR-5 — a tile click (Q45) opens that tile's detail; the menu's
        // Back control (Q49) returns to the grid. Out-of-range indices are
        // ignored so a stale click never opens an empty menu.
        let mut fd = FrontDoor::new();
        assert!(fd.detail.is_none());

        let _ = fd.update(Message::TileActivated(2));
        assert_eq!(fd.detail, Some(2));

        // Re-activating swaps the open tile.
        let _ = fd.update(Message::TileActivated(0));
        assert_eq!(fd.detail, Some(0));

        // Back closes the detail.
        let _ = fd.update(Message::CloseDetail);
        assert!(fd.detail.is_none());

        // An out-of-range index is rejected (no panic, stays closed).
        let _ = fd.update(Message::TileActivated(9_999));
        assert!(fd.detail.is_none());
    }

    #[test]
    fn every_tile_action_is_a_real_navigation_or_launch() {
        // §7 — every detail action must be a REAL app message, never an inert/
        // no-op. Walk every seeded tile's menu and assert each carries one of the
        // real message shapes: a panel route, an app launch, or (FRONTDOOR-7) a
        // pipeline action whose inner `nav` is itself a real `SelectPanel` route
        // and whose `trigger` (if any) is a real existing-panel sub-message.
        let fd = FrontDoor::new();
        for tile in &fd.tiles {
            for action in tile.actions() {
                assert_real_action(&action.message, &tile.label);
            }
        }
    }

    /// Assert a tile-action message is a REAL, handled app message (§7). Recurses
    /// into a [`Message::PipelineAction`] so its `nav`/`trigger` are themselves
    /// verified as real (never an inert wrapper).
    fn assert_real_action(message: &crate::Message, label: &str) {
        match message {
            crate::Message::SelectPanel { .. } | crate::Message::LaunchApp(_) => {}
            // FRONTDOOR-7 — a pipeline action is real iff its nav is a real route
            // and its optional trigger is a real existing-panel sub-message.
            crate::Message::FrontDoor(Message::PipelineAction { nav, trigger }) => {
                assert!(
                    matches!(**nav, crate::Message::SelectPanel { .. }),
                    "tile {label} pipeline nav is not a real route: {nav:?}"
                );
                if let Some(trigger) = trigger {
                    // The wired triggers are the owning panels' OWN re-read/re-probe
                    // sub-messages — real existing verbs, never a stub. FD-7 wired
                    // the build-farm / jobs `RefreshClicked`; FD-8 adds the Node
                    // Roles `RefreshClicked` (drain surface re-read) + the Health
                    // panel's `RunClicked` (the per-node health re-probe).
                    assert!(
                        matches!(
                            **trigger,
                            crate::Message::BuildFarm(build_farm::Message::RefreshClicked)
                                | crate::Message::Jobs(jobs::Message::RefreshClicked)
                                | crate::Message::NodeRoles(node_roles::Message::RefreshClicked)
                                | crate::Message::HealthCheck(health_check::Message::RunClicked)
                        ),
                        "tile {label} pipeline trigger is not a real existing verb: {trigger:?}"
                    );
                }
            }
            other => panic!("tile {label} has a non-real action: {other:?}"),
        }
    }

    #[test]
    fn keyed_widget_tiles_have_real_actions_and_copilot_is_honestly_empty() {
        // The keyed widgets all route somewhere real (no empty menus); Copilot
        // (no workbench-readable surface yet) has an honest empty action list
        // rather than a dead button (§7).
        let fd = FrontDoor::new();
        let tile = |label: &str| fd.tiles.iter().find(|t| t.label == label).unwrap();

        for label in [
            "Mesh Map",
            "Build / Farm",
            "Alerts",
            "Node Health",
            "System",
            "Data Center",
            "DevOps",
        ] {
            assert!(
                !tile(label).actions().is_empty(),
                "widget tile {label} should expose at least one real action"
            );
        }
        // Copilot — empty, but never faked.
        assert!(tile("Copilot").actions().is_empty());
    }

    #[test]
    fn detail_view_builds_for_a_keyed_tile_and_for_copilot() {
        // The detail actions-menu view constructs for a tile with actions + live
        // data, and for the action-less Copilot (data + Back only) — neither
        // panics.
        let mut fd = FrontDoor::new();
        fd.loading = false;

        let mesh_idx = fd.tiles.iter().position(|t| t.label == "Mesh Map").unwrap();
        let _ = fd.update(Message::TileActivated(mesh_idx));
        let _: Element<'_, crate::Message, Theme> = fd.view();

        let copilot_idx = fd.tiles.iter().position(|t| t.label == "Copilot").unwrap();
        let _ = fd.update(Message::TileActivated(copilot_idx));
        let _: Element<'_, crate::Message, Theme> = fd.view();
    }

    // ── FRONTDOOR-7: the DevOps / Build-Farm one-click pipeline actions ──

    /// Pull the tile carrying a given [`TileKey`] from a fresh Front Door.
    fn keyed_tile(key: TileKey) -> Tile {
        FrontDoor::new()
            .tiles
            .into_iter()
            .find(|t| t.key == Some(key))
            .unwrap_or_else(|| panic!("no seeded tile for {key:?}"))
    }

    #[test]
    fn devops_and_build_farm_tiles_expose_the_locked_pipeline_action_set() {
        // FRONTDOOR-7 / Q50d — both the Build/Farm and DevOps tiles surface the
        // five locked one-click pipeline actions (build · deploy · rollback ·
        // view-logs · rerun-failed), in menu order, in BOTH tiles.
        for key in [TileKey::BuildFarm, TileKey::DevOps] {
            let labels: Vec<String> = keyed_tile(key)
                .actions()
                .into_iter()
                .map(|a| a.label)
                .collect();
            assert_eq!(labels.len(), 5, "{key:?} should expose all five verbs");
            assert!(labels[0].starts_with("Build"), "build is first: {labels:?}");
            assert!(
                labels.iter().any(|l| l == "Deploy"),
                "deploy present: {labels:?}"
            );
            assert!(
                labels.iter().any(|l| l == "Rollback"),
                "rollback present: {labels:?}"
            );
            assert!(
                labels.iter().any(|l| l == "View logs"),
                "view-logs present: {labels:?}"
            );
            assert!(
                labels.iter().any(|l| l == "Rerun failed"),
                "rerun-failed present: {labels:?}"
            );
        }
    }

    #[test]
    fn pipeline_actions_route_to_existing_panels_and_wire_real_triggers() {
        // FRONTDOOR-7 — every pipeline action is a real `PipelineAction` whose nav
        // routes to an EXISTING panel slug (§7), and the WIRED ones (Build, Rerun
        // failed) carry the build-farm / jobs panels' own `RefreshClicked` trigger
        // (§9 — the existing typed verb, no raw shell). Navigate-only verbs
        // (Deploy / Rollback / View logs) carry no trigger.
        let routes: std::collections::BTreeMap<String, (Group, &'static str, bool)> =
            keyed_tile(TileKey::BuildFarm)
                .actions()
                .into_iter()
                .map(|a| {
                    let crate::Message::FrontDoor(Message::PipelineAction { nav, trigger }) =
                        a.message
                    else {
                        panic!("action {} is not a PipelineAction", a.label);
                    };
                    let crate::Message::SelectPanel { group, panel } = *nav else {
                        panic!("action {} nav is not a SelectPanel route", a.label);
                    };
                    (a.label, (group, panel, trigger.is_some()))
                })
                .collect();

        // The named verbs land on the surface that owns them.
        assert_eq!(
            routes["Build — refresh farm"],
            (Group::Provisioning, "build-farm", true),
            "build is wired and routes to the Build Farm panel"
        );
        assert_eq!(routes["Deploy"], (Group::Provisioning, "build-farm", false));
        assert_eq!(
            routes["Rollback"],
            (Group::System, "revisions", false),
            "rollback routes to Fleet Config — the surface with the real Rollback button"
        );
        assert_eq!(
            routes["View logs"],
            (Group::Monitoring, "run_history", false),
            "view-logs routes to Run History — the real logs surface"
        );
        assert_eq!(
            routes["Rerun failed"],
            (Group::Fleet, "jobs", true),
            "rerun-failed is wired and routes to the Jobs panel"
        );
    }

    #[test]
    fn pipeline_action_routes_and_clears_the_detail() {
        // FRONTDOOR-7 — firing a pipeline action dismisses the open detail (the
        // grid restores behind the navigation) and dispatches the nav (+ trigger).
        // The handler returns a real Task batch; here we assert the state edit.
        let mut fd = FrontDoor::new();
        let build = keyed_tile(TileKey::BuildFarm)
            .actions()
            .into_iter()
            .next()
            .unwrap();
        let crate::Message::FrontDoor(inner) = build.message else {
            panic!("the build action is a Front Door pipeline message");
        };
        // Open a detail, then fire the pipeline action — the detail clears.
        let _ = fd.update(Message::TileActivated(0));
        assert!(fd.detail.is_some());
        let _ = fd.update(inner);
        assert!(fd.detail.is_none(), "a pipeline action restores the grid");
    }

    // ── FRONTDOOR-8: the Data Center tile's node-lifecycle one-click actions ──

    /// Pull a Data Center tile action's `(group, panel, wired)` route, panicking
    /// if it isn't a real [`Message::PipelineAction`] over a `SelectPanel` nav.
    fn dc_route(label: &str) -> (Group, &'static str, bool) {
        let action = keyed_tile(TileKey::DataCenter)
            .actions()
            .into_iter()
            .find(|a| a.label == label)
            .unwrap_or_else(|| panic!("no Data Center action labelled {label:?}"));
        let crate::Message::FrontDoor(Message::PipelineAction { nav, trigger }) = action.message
        else {
            panic!("action {label} is not a PipelineAction");
        };
        let crate::Message::SelectPanel { group, panel } = *nav else {
            panic!("action {label} nav is not a SelectPanel route");
        };
        (group, panel, trigger.is_some())
    }

    #[test]
    fn data_center_tile_exposes_the_locked_q51_lifecycle_action_set() {
        // FRONTDOOR-8 / Q51 — the Data Center tile surfaces the full locked
        // node-lifecycle set in menu order: join · drain · restart-service ·
        // view-health · cutover-helpers · provision · destroy.
        let labels: Vec<String> = keyed_tile(TileKey::DataCenter)
            .actions()
            .into_iter()
            .map(|a| a.label)
            .collect();
        assert_eq!(
            labels,
            vec![
                "Join node",
                "Drain node",
                "Restart service",
                "View health",
                "Cutover helpers",
                "Provision",
                "Destroy",
            ],
            "the Q51 lifecycle set, in order: {labels:?}"
        );
    }

    #[test]
    fn data_center_lifecycle_actions_route_to_real_panels_and_wire_real_triggers() {
        // FRONTDOOR-8 — every lifecycle action is a real `PipelineAction` whose nav
        // routes to an EXISTING panel slug (§7). WIRED where a parameterless verb
        // exists: Drain re-reads the Node Roles surface, View health re-probes the
        // Health panel (§9 — the existing typed verbs). The target-scoped verbs
        // (join/restart/cutover) and the gated provision/destroy are navigate-only.
        assert_eq!(
            dc_route("Join node"),
            (Group::MeshProvisioning, "mesh_join", false),
            "join navigates to the Mesh Join surface (it needs a passcode)"
        );
        assert_eq!(
            dc_route("Drain node"),
            (Group::Provisioning, "node_roles", true),
            "drain is wired to the Node Roles surface's own re-read"
        );
        assert_eq!(
            dc_route("Restart service"),
            (Group::ThisNode, "mesh_services", false),
            "restart navigates to Mesh Services (it needs a unit + scope)"
        );
        assert_eq!(
            dc_route("View health"),
            (Group::Monitoring, "health_check", true),
            "view-health is wired to the Health panel's own re-probe"
        );
        assert_eq!(
            dc_route("Cutover helpers"),
            (Group::Mesh, "mesh_control", false),
            "cutover navigates to Mesh Control — we never fire a takeover blind"
        );
    }

    #[test]
    fn data_center_provision_and_destroy_are_navigate_only_to_the_tofu_surface() {
        // FRONTDOOR-8 — provision + destroy are NAVIGATE-ONLY (Q54): they route to
        // the Datacenter Tofu surface (the operator-gated apply/destroy lives there)
        // and carry NO trigger, so a destructive op is never fired from a tile click.
        assert_eq!(
            dc_route("Provision"),
            (Group::Provisioning, "datacenter", false),
            "provision navigates to the gated tofu surface, no blind trigger"
        );
        assert_eq!(
            dc_route("Destroy"),
            (Group::Provisioning, "datacenter", false),
            "destroy navigates to the gated tofu surface, no blind trigger"
        );
    }

    // ── FRONTDOOR-6: the unified search (local search/rank + the message flow) ──

    /// A `PeerRow` with a chosen hostname + published services (the fields the
    /// mesh search reads); the rest default.
    fn mesh_peer(hostname: &str, services: &[&str]) -> PeerRow {
        PeerRow {
            hostname: hostname.into(),
            presence: "online".into(),
            health: "healthy".into(),
            role: "peer".into(),
            services: services.iter().map(|s| (*s).to_string()).collect(),
            ..PeerRow::default()
        }
    }

    #[test]
    fn score_match_ladder_orders_exact_prefix_word_substring() {
        use search::score_match;
        // Exact > leading-prefix > word-boundary-prefix > bare substring > miss.
        assert_eq!(score_match("Build Farm", "build farm"), 100);
        assert!(score_match("Build Farm", "build") > score_match("Build Farm", "farm"));
        // "farm" is a word-boundary prefix (after the space), not a leading one.
        assert_eq!(score_match("Build Farm", "build"), 80);
        assert_eq!(score_match("Build Farm", "farm"), 60);
        // A bare interior substring is the weakest real match.
        assert_eq!(score_match("Datacenter", "cent"), 40);
        // No match / empty needle → 0 (dropped).
        assert_eq!(score_match("Mesh", "zzz"), 0);
        assert_eq!(score_match("Mesh", ""), 0);
    }

    #[test]
    fn local_results_finds_apps_panels_and_mesh_entities() {
        // §7 — REAL unified scope over the real catalogs: a launcher app, a
        // routable panel, a mesh node, and a published service all surface, each
        // carrying a real activation message.
        let tiles = FrontDoor::new().tiles;
        let peers = vec![
            mesh_peer("anvil", &["mfsmaster", "nginx"]),
            mesh_peer("oak", &["etcd"]),
        ];

        // A node name → a MeshNode hit that routes to Peers.
        let r = search::local_results("anvil", &tiles, &peers);
        assert!(r.iter().any(|h| h.kind == HitKind::MeshNode && h.label == "anvil"));
        assert!(matches!(
            r.iter().find(|h| h.label == "anvil").unwrap().message,
            crate::Message::SelectPanel {
                group: Group::Mesh,
                panel: "peers"
            }
        ));

        // A service name → a MeshService hit, context naming its node.
        let r = search::local_results("mfsmaster", &tiles, &peers);
        let svc = r
            .iter()
            .find(|h| h.kind == HitKind::MeshService)
            .expect("a service hit");
        assert_eq!(svc.label, "mfsmaster");
        assert!(svc.context.contains("anvil"));

        // An app/panel name → an App hit. "Terminal" is a seeded launcher.
        let r = search::local_results("terminal", &tiles, &peers);
        assert!(r
            .iter()
            .any(|h| h.kind == HitKind::App && h.label == "Terminal"));

        // A panel label → an App hit routing via SelectPanel (e.g. Build Farm).
        let r = search::local_results("build farm", &tiles, &peers);
        assert!(r.iter().any(|h| {
            h.kind == HitKind::App
                && matches!(h.message, crate::Message::SelectPanel { .. })
                && h.label == "Build Farm"
        }));

        // A blank/whitespace query yields nothing (the grid shows).
        assert!(search::local_results("   ", &tiles, &peers).is_empty());
    }

    #[test]
    fn local_results_rank_orders_best_first_and_every_hit_is_actionable() {
        let tiles = FrontDoor::new().tiles;
        let peers = vec![mesh_peer("mesh-builder", &["mesh-agent"])];
        let r = search::local_results("mesh", &tiles, &peers);
        assert!(!r.is_empty());
        // Sorted by descending score (the ranker's first key).
        for w in r.windows(2) {
            assert!(
                w[0].score >= w[1].score,
                "results are rank-sorted best-first"
            );
        }
        // §7 — every hit carries a REAL app message (a route or a launch), never
        // an inert result.
        for hit in &r {
            match hit.message {
                crate::Message::SelectPanel { .. } | crate::Message::LaunchApp(_) => {}
                ref other => panic!("non-actionable search hit: {other:?}"),
            }
        }
        // The list is capped (Q47 — the AI card carries the long tail).
        assert!(r.len() <= search::MAX_LOCAL_RESULTS);
    }

    #[test]
    fn keyed_widget_tiles_are_not_treated_as_app_launchers() {
        // The live widget tiles (Mesh Map, Alerts, …) are mesh surfaces, not
        // "apps" — searching their label must NOT produce a launcher app hit from
        // the tile set (they surface via the mesh entities / their own grid).
        let tiles = FrontDoor::new().tiles;
        let r = search::local_results("alerts", &tiles, &[]);
        // No App hit whose label is the "Alerts" widget tile (a keyed widget).
        assert!(
            !r.iter()
                .any(|h| h.kind == HitKind::App && h.label == "Alerts"),
            "a keyed widget tile is not an app launcher hit"
        );
    }

    #[test]
    fn ask_request_body_is_valid_copilot_askrequest_json() {
        // The body must match FD-9's AskRequest shape ({prompt, context}) and
        // escape any quotes in the query (serde, not raw format!).
        let body = search::ask_request_body(r#"why is "anvil" down?"#);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["prompt"], r#"why is "anvil" down?"#);
        assert!(v["context"].as_str().unwrap().contains("Front Door"));
    }

    #[test]
    fn parse_copilot_reply_maps_answer_and_degrades_quietly() {
        use search::parse_copilot_reply;
        // A real answer → Answer(text), trimmed.
        assert_eq!(
            parse_copilot_reply(Some(r#"{"answer":"  restart mfsmaster  "}"#)),
            CopilotAnswer::Answer("restart mfsmaster".to_string())
        );
        // The worker's degrade reply ({error, no answer}) → Unavailable (Q33).
        assert_eq!(
            parse_copilot_reply(Some(r#"{"error":"AI unavailable: codex not available"}"#)),
            CopilotAnswer::Unavailable
        );
        // An empty/whitespace answer is not a real answer → Unavailable.
        assert_eq!(
            parse_copilot_reply(Some(r#"{"answer":"   "}"#)),
            CopilotAnswer::Unavailable
        );
        // No reply at all (timeout / no Bus / no worker) → Unavailable, no panic.
        assert_eq!(parse_copilot_reply(None), CopilotAnswer::Unavailable);
        // Malformed JSON → Unavailable.
        assert_eq!(
            parse_copilot_reply(Some("not json")),
            CopilotAnswer::Unavailable
        );
    }

    #[test]
    fn copilot_reply_only_folds_in_for_the_current_generation() {
        // The message flow: a reply stamped with a stale generation (the operator
        // changed the query since) is DROPPED, not shown; the matching generation
        // folds in.
        let mut fd = FrontDoor::new();
        let _ = fd.update(Message::OmniboxChanged("anvil".into())); // gen → 1
        assert_eq!(fd.copilot_gen, 1);
        assert_eq!(fd.copilot, CopilotState::Thinking);

        // A reply for an OLD generation is ignored (still thinking).
        let _ = fd.update(Message::CopilotReplied(
            0,
            CopilotAnswer::Answer("stale".into()),
        ));
        assert_eq!(fd.copilot, CopilotState::Thinking, "stale reply dropped");

        // The reply for the current generation folds in.
        let _ = fd.update(Message::CopilotReplied(
            1,
            CopilotAnswer::Answer("anvil is offline; restart nebula".into()),
        ));
        assert_eq!(
            fd.copilot,
            CopilotState::Answer("anvil is offline; restart nebula".into())
        );

        // A degrade reply for the current generation parks the quiet note.
        let _ = fd.update(Message::OmniboxChanged("oak".into())); // gen → 2
        let _ = fd.update(Message::CopilotReplied(2, CopilotAnswer::Unavailable));
        assert_eq!(fd.copilot, CopilotState::Unavailable);
    }

    #[test]
    fn search_hit_activation_clears_the_query_back_to_the_grid() {
        // Clicking a hit fires its real message and clears the omnibox so the
        // Front Door returns to the tile grid behind the navigation.
        let mut fd = FrontDoor::new();
        let _ = fd.update(Message::OmniboxChanged("peers".into()));
        assert!(!fd.results.is_empty());
        let msg = crate::Message::SelectPanel {
            group: Group::Mesh,
            panel: "peers",
        };
        let _ = fd.update(Message::SearchHitActivated(Box::new(msg)));
        assert!(fd.query.is_empty(), "activation clears the omnibox");
        assert!(fd.results.is_empty());
        assert_eq!(fd.copilot, CopilotState::Idle);
    }

    #[test]
    fn loaded_snapshot_refreshes_the_search_roster() {
        // FRONTDOOR-6 — the FD-4 load snapshot also carries the Peers roster into
        // the search; a live search re-ranks against the fresh roster so a node
        // that appears mid-search becomes searchable without a re-type.
        let mut fd = FrontDoor::new();
        let _ = fd.update(Message::OmniboxChanged("anvil".into()));
        // No roster yet → no mesh hit for "anvil".
        assert!(!fd.results.iter().any(|h| h.label == "anvil"));

        // A snapshot lands carrying the roster.
        let data = FrontDoorData {
            peers: vec![mesh_peer("anvil", &["mfsmaster"])],
            ..FrontDoorData::default()
        };
        let _ = fd.update(Message::Loaded(Box::new(data)));
        assert!(
            fd.results.iter().any(|h| h.label == "anvil"),
            "the fresh roster makes the node searchable mid-search"
        );
    }

    // ── FRONTDOOR-10 (GUI half): the live Copilot status + proactive suggestions ──

    #[test]
    fn copilot_status_parses_each_state_and_degrades_on_junk() {
        use copilot::{parse_status, StatusState};
        // The three backend states map to the three tile values; `available` is
        // carried through.
        let ready = parse_status(Some(
            r#"{"state":"ready","leader":true,"available":true,"model":"gpt-5-mini"}"#,
        ))
        .expect("ready parses");
        assert_eq!(ready.state, StatusState::Ready);
        assert!(ready.available);
        assert_eq!(ready.tile_value().0, "ready");
        assert_eq!(ready.tile_value().1, TileTone::Success);

        assert_eq!(
            parse_status(Some(r#"{"state":"thinking","available":true}"#))
                .unwrap()
                .state,
            StatusState::Thinking
        );
        let offline = parse_status(Some(
            r#"{"state":"offline","leader":false,"available":false,"model":"unknown"}"#,
        ))
        .unwrap();
        assert_eq!(offline.state, StatusState::Offline);
        assert!(!offline.available);
        assert_eq!(offline.tile_value().1, TileTone::Neutral);

        // Junk / unknown state / no body → None (the tile keeps its skeleton, §7).
        assert!(parse_status(None).is_none());
        assert!(parse_status(Some("not json")).is_none());
        assert!(parse_status(Some(r#"{"state":"wat"}"#)).is_none());
    }

    #[test]
    fn copilot_suggestions_parse_rank_order_and_keep_the_proposal_body() {
        use copilot::parse_suggestions;
        // A ranked set with one advisory + one actionable suggestion; order is
        // preserved (the backend ranks best-first) and the typed proposal rides
        // through as its own JSON body for the §9-safe "Act".
        let body = r#"{
            "suggestions":[
              {"title":"mfsmaster on oak is down","detail":"restart it","impact":"high",
               "proposal":{"action":{"kind":"service_lifecycle","target_host":"oak",
                 "service_kind":"container","name":"mfsmaster","op":"restart"},
                 "rationale":"oak lost its master"}},
              {"title":"farm queue is idle","detail":"nothing to do","impact":"medium"}
            ],
            "produced_at_s":123
        }"#;
        let sugg = parse_suggestions(Some(body));
        assert_eq!(sugg.len(), 2);
        assert_eq!(sugg[0].title, "mfsmaster on oak is down");
        assert_eq!(sugg[0].impact, "high");
        let proposal = sugg[0].proposal_body.as_deref().expect("actionable");
        assert!(proposal.contains("service_lifecycle"));
        assert!(proposal.contains("\"target_host\":\"oak\""));
        // The advisory one carries no proposal → no "Act" affordance.
        assert!(sugg[1].proposal_body.is_none());

        // A title-less entry is dropped; junk / empty set → empty.
        assert!(parse_suggestions(Some(r#"{"suggestions":[{"detail":"x"}]}"#)).is_empty());
        assert!(parse_suggestions(Some("not json")).is_empty());
        assert!(parse_suggestions(None).is_empty());
    }

    #[test]
    fn a_suggestion_maps_to_the_tile_it_concerns() {
        use copilot::Suggestion;
        let mk = |title: &str, detail: &str, proposal: Option<&str>| Suggestion {
            title: title.into(),
            detail: detail.into(),
            impact: "high".into(),
            proposal_body: proposal.map(str::to_string),
        };
        // A typed service-lifecycle proposal always maps to the Data Center tile.
        assert_eq!(
            mk("restart", "", Some(r#"{"action":{"kind":"service_lifecycle"}}"#)).concerns_tile(),
            Some(TileKey::DataCenter)
        );
        // Keyword buckets.
        assert_eq!(
            mk("Build farm is red", "a tier failed", None).concerns_tile(),
            Some(TileKey::BuildFarm)
        );
        assert_eq!(
            mk("Alert firing", "a health alarm", None).concerns_tile(),
            Some(TileKey::Alerts)
        );
        assert_eq!(
            mk("Mesh peer offline", "routing", None).concerns_tile(),
            Some(TileKey::MeshMap)
        );
        // Names no tile → unmapped (homed on the Copilot tile by the panel).
        assert!(mk("ponder the universe", "vague", None).concerns_tile().is_none());
    }

    #[test]
    fn loaded_folds_the_copilot_status_and_suggestions_inline() {
        // The end-to-end fold (Q19): a snapshot carries the live status + a ranked
        // set → the Copilot tile takes the status value, and each suggestion lands
        // on the tile it concerns (the canvas badge count proves the mapping).
        let mut fd = FrontDoor::new();
        let data = FrontDoorData {
            copilot: Some(("thinking".into(), TileTone::Warning)),
            suggestions: vec![
                copilot::Suggestion {
                    title: "Build farm red".into(),
                    detail: "a tier failed".into(),
                    impact: "high".into(),
                    proposal_body: None,
                },
                copilot::Suggestion {
                    title: "restart mfsmaster".into(),
                    detail: "oak lost it".into(),
                    impact: "high".into(),
                    proposal_body: Some(
                        r#"{"action":{"kind":"service_lifecycle","target_host":"oak","service_kind":"container","name":"mfsmaster","op":"restart"},"rationale":"x"}"#
                            .into(),
                    ),
                },
            ],
            ..FrontDoorData::default()
        };
        let _ = fd.update(Message::Loaded(Box::new(data)));

        let tile = |label: &str| fd.tiles.iter().find(|t| t.label == label).unwrap();
        // FD-4 gap closed — the Copilot tile shows the live status now.
        assert_eq!(tile("Copilot").value.as_deref(), Some("thinking"));
        assert_eq!(tile("Copilot").tone, TileTone::Warning);

        // The build suggestion lands on the Build/Farm tile; the lifecycle proposal
        // lands on the Data Center tile (badge count == 1 each).
        let idx = |label: &str| fd.tiles.iter().position(|t| t.label == label).unwrap();
        assert_eq!(fd.suggestion_count_for(idx("Build / Farm")), 1);
        assert_eq!(fd.suggestion_count_for(idx("Data Center")), 1);
        // A tile no suggestion concerns has no badge.
        assert_eq!(fd.suggestion_count_for(idx("System")), 0);
    }

    #[test]
    fn an_unmapped_suggestion_is_homed_on_the_copilot_tile_never_dropped() {
        let mut fd = FrontDoor::new();
        fd.suggestions = vec![copilot::Suggestion {
            title: "ponder the universe".into(),
            detail: "no tile named".into(),
            impact: "medium".into(),
            proposal_body: None,
        }];
        let copilot_idx = fd.tiles.iter().position(|t| t.label == "Copilot").unwrap();
        assert_eq!(fd.suggestion_count_for(copilot_idx), 1);
        // It isn't double-counted onto an arbitrary widget.
        let sys_idx = fd.tiles.iter().position(|t| t.label == "System").unwrap();
        assert_eq!(fd.suggestion_count_for(sys_idx), 0);
    }

    #[test]
    fn act_on_a_suggestion_targets_the_propose_topic_not_the_exec_topic() {
        // §9 — the "Act" path re-publishes to the PROPOSE queue, never the exec
        // topic, and never executes. The topic constant the handler writes to is
        // the FD-12 propose queue.
        assert_eq!(copilot::PROPOSAL_TOPIC, "action/copilot/proposal");
        assert_ne!(copilot::PROPOSAL_TOPIC, "action/exec/request");

        // ProposeSuggestion with no proposal body (advisory-only) is an inert
        // no-op Task — it never publishes anything.
        let mut fd = FrontDoor::new();
        fd.suggestions = vec![copilot::Suggestion {
            title: "advisory only".into(),
            detail: "no proposal".into(),
            impact: "medium".into(),
            proposal_body: None,
        }];
        // Doesn't panic; the handler short-circuits on the absent proposal body.
        let _ = fd.update(Message::ProposeSuggestion(0));
        // A stale index is also a no-op.
        let _ = fd.update(Message::ProposeSuggestion(99));
    }

    #[test]
    fn copilot_tile_detail_renders_its_suggestions_without_panicking() {
        // The detail view for the Copilot tile builds with its homed suggestions +
        // an actionable card (the §9 "Act" button) — neither path panics.
        let mut fd = FrontDoor::new();
        fd.loading = false;
        fd.suggestions = vec![copilot::Suggestion {
            title: "restart mfsmaster on oak".into(),
            detail: "oak lost its master".into(),
            impact: "high".into(),
            proposal_body: Some(
                r#"{"action":{"kind":"service_lifecycle","target_host":"oak","service_kind":"container","name":"mfsmaster","op":"restart"},"rationale":"x"}"#
                    .into(),
            ),
        }];
        // The lifecycle proposal homes on the Data Center tile — open its detail.
        let dc_idx = fd.tiles.iter().position(|t| t.label == "Data Center").unwrap();
        let _ = fd.update(Message::TileActivated(dc_idx));
        let _: Element<'_, crate::Message, Theme> = fd.view();
    }
}
