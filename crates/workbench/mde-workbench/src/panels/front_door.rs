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
//! SCOPE held to FRONTDOOR-1/2/3/4:
//! - `draw` only — tile click → detail view is FRONTDOOR-5, so the canvas keeps
//!   `type State = ()` and the default `update` / `mouse_interaction`.
//! - Omnibox is render + local text state only (search logic is FRONTDOOR-6).
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
use crate::panels::{build_farm, datacenter, home, peers};

/// FRONTDOOR-2/3 — the Front Door's own message set, threaded through
/// [`crate::Message::FrontDoor`]. Each variant is one we actually handle (§7):
/// the omnibox text-change and the panel ↔ full-screen toggle. Rail navigation
/// reuses the app-level [`crate::Message::SelectPanel`] directly (it drives the
/// real router), so it needs no variant here.
#[derive(Debug, Clone)]
pub enum Message {
    /// The omnibox text changed. FRONTDOOR-2 only records it into local state;
    /// the search behavior it will drive is FRONTDOOR-6.
    OmniboxChanged(String),
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
            Some(TileKey::DataCenter) => vec![
                TileAction::nav("Open Datacenter", Group::Provisioning, "datacenter"),
                TileAction::nav("Open Peers", Group::Mesh, "peers"),
            ],
            Some(TileKey::BuildFarm) => vec![
                TileAction::nav("Open Build Farm", Group::Provisioning, "build-farm"),
                TileAction::nav("Open Jobs", Group::Fleet, "jobs"),
            ],
            Some(TileKey::DevOps) => vec![
                TileAction::nav("Open Build Farm", Group::Provisioning, "build-farm"),
                TileAction::nav("Open Jobs", Group::Fleet, "jobs"),
            ],
            Some(TileKey::Alerts) => vec![
                TileAction::nav("Open Datacenter", Group::Provisioning, "datacenter"),
                TileAction::nav("Open Health", Group::Monitoring, "health_check"),
            ],
            Some(TileKey::System) => vec![
                TileAction::nav("Open Health", Group::Monitoring, "health_check"),
                TileAction::launch("Open Settings", "cosmic-settings"),
            ],
            // Copilot is seeded as a launcher (no key) — backend only
            // (FRONTDOOR-12), no route/app the workbench can open yet — so it
            // falls into the launcher arm below and yields an empty action list.
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
            // FRONTDOOR-12 backend only — no bus topic the workbench reads yet.
            TileKey::Copilot => None,
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

        Self {
            mesh_map: project::mesh_map(&peers),
            node_health: project::node_health(&peers),
            data_center: project::data_center(&peers),
            build_farm: project::build_farm(&farm),
            dev_ops: project::dev_ops(&farm),
            alerts: project::alerts(&health),
            system: Some(project::system(&boot)),
        }
    }
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
    /// controlled; the search it will drive is FRONTDOOR-6 (no behavior yet).
    pub query: String,
    /// FRONTDOOR-3 — which render mode the Front Door is in (panel default,
    /// flipped by the top-bar toggle). Default [`Mode::Panel`] (Q29).
    pub mode: Mode,
    /// FRONTDOOR-5 — the index of the tile whose detail **actions menu** is open
    /// (Q45/Q49), or `None` when the grid is showing. Set by a canvas tile click
    /// ([`Message::TileActivated`]); cleared by the menu's back control
    /// ([`Message::CloseDetail`]) or by leaving / reloading the view.
    pub detail: Option<usize>,
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
    /// shift). Copilot is seeded as a plain **launcher** (no key): FRONTDOOR-12
    /// landed its backend but no workbench-readable bus topic yet, so faking a
    /// value would violate §7 — it stays a launcher pending a publisher.
    #[must_use]
    pub fn new() -> Self {
        let tiles = vec![
            Tile::widget("Mesh Map", TileKey::MeshMap, TileTone::Accent),
            Tile::widget("Build / Farm", TileKey::BuildFarm, TileTone::Warning),
            Tile::widget("Alerts", TileKey::Alerts, TileTone::Danger),
            Tile::widget("Node Health", TileKey::NodeHealth, TileTone::Success),
            // Copilot — NO workbench source yet → plain launcher (§7), needs a
            // publisher (follow-up).
            Tile::launcher("Copilot", TileTone::Accent),
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
            mode: Mode::Panel,
            // FRONTDOOR-5 — start on the grid; a tile click opens a detail menu.
            detail: None,
        }
    }

    /// FRONTDOOR-4 — read the widget tiles' live data off the **existing**
    /// mde-bus data paths (Peers directory · Build Farm · Datacenter health ·
    /// boot readiness) on a blocking thread and fold it back in via
    /// [`Message::Loaded`]. Dispatched the same way the other panels load (a
    /// `Task` fired on nav / reconnect / the slow-poll tick), so the Front Door
    /// gets its data through the established subscription infra (§6) rather than
    /// a new mackesd publisher.
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
            Message::OmniboxChanged(q) => {
                self.query = q;
                Task::none()
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
        }
    }

    /// FRONTDOOR-4 — fold one [`FrontDoorData`] snapshot into the widget tiles:
    /// each keyed tile takes its `(value, tone)` from the snapshot, or keeps a
    /// `None` value (no source this round) — a launcher (`key == None`) is never
    /// touched. Pure given the snapshot, so it's unit-tested directly.
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
        if let Some(tile) = self.detail.and_then(|i| self.tiles.get(i)) {
            return self.detail_view(tile, palette);
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
    fn detail_view(&self, tile: &Tile, palette: Palette) -> Element<'_, crate::Message, Theme> {
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

        let body = column![
            back,
            Space::new().height(Length::Fixed(16.0)),
            header,
            Space::new().height(Length::Fixed(20.0)),
            menu,
        ]
        .spacing(8)
        .width(Length::Fill);

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

        // The full-screen rounded-icon grid: the same canvas program, told to lay
        // out at the larger full-screen scale. A scrollable wrapper gives the
        // "first cut" vertical paging when icons overflow the viewport.
        let grid = scrollable(self.icon_grid())
            .width(Length::Fill)
            .height(Length::Fill);

        let body = column![top_bar, grid]
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

        let pane = column![omnibox_bar, self.tile_grid()]
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
        let program = TileGrid {
            tiles: self.tiles.clone(),
            loading: self.loading,
            palette: crate::live_theme::palette(),
            layout,
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

/// The accent strip down the left edge of a card. Mode-independent (it's a hair
/// of color, not a sized element).
const STRIP_W: f32 = 5.0;

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
        // FRONTDOOR-4 — the design widgets are keyed (they take live data);
        // Copilot + the app launchers are keyless (no source → no faked value).
        let fd = FrontDoor::new();
        let key_of = |label: &str| fd.tiles.iter().find(|t| t.label == label).map(|t| t.key);
        assert_eq!(key_of("Mesh Map"), Some(Some(TileKey::MeshMap)));
        assert_eq!(key_of("Node Health"), Some(Some(TileKey::NodeHealth)));
        assert_eq!(key_of("Alerts"), Some(Some(TileKey::Alerts)));
        assert_eq!(key_of("Build / Farm"), Some(Some(TileKey::BuildFarm)));
        assert_eq!(key_of("System"), Some(Some(TileKey::System)));
        // Copilot has NO workbench source yet → it's a plain launcher (§7).
        assert_eq!(key_of("Copilot"), Some(None));
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
    fn omnibox_change_records_the_query_locally() {
        // FRONTDOOR-2 — the omnibox is a controlled field: a text-change updates
        // local state (so the field shows the typed text), with no other effect
        // (search is FRONTDOOR-6).
        let mut fd = FrontDoor::new();
        let _ = fd.update(Message::OmniboxChanged("build farm".to_string()));
        assert_eq!(fd.query, "build farm");
        let _ = fd.update(Message::OmniboxChanged(String::new()));
        assert!(fd.query.is_empty());
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
        // Copilot (a launcher) is never given a value (§7 — needs a publisher).
        assert!(tile("Copilot").value.is_none());
        assert_eq!(tile("Copilot").key, None);
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
    fn copilot_has_no_source_so_for_key_is_none() {
        // §7 — Copilot is the one widget with no workbench-readable source yet;
        // even a fully-populated snapshot yields nothing for it.
        let data = FrontDoorData {
            node_health: Some(("1/1 healthy".into(), TileTone::Success)),
            ..FrontDoorData::default()
        };
        assert!(data.for_key(TileKey::Copilot).is_none());
    }

    // ── FRONTDOOR-5: tile click → detail actions menu ──

    /// A `TileGrid` over the seeded tiles at the given layout, in the loaded
    /// (interactive) state — the shape the canvas sees once data has landed.
    fn grid(layout: Layout) -> TileGrid {
        TileGrid {
            tiles: FrontDoor::new().tiles,
            loading: false,
            palette: Palette::dark(),
            layout,
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
        // §7 — every detail action must be a REAL app message (a panel route or
        // an app launch), never an inert/no-op. Walk every seeded tile's menu and
        // assert each carries one of the two real message shapes.
        let fd = FrontDoor::new();
        for tile in &fd.tiles {
            for action in tile.actions() {
                match action.message {
                    crate::Message::SelectPanel { .. } | crate::Message::LaunchApp(_) => {}
                    other => panic!("tile {:?} has a non-real action: {other:?}", tile.label),
                }
            }
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
}
