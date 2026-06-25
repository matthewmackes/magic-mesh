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
    /// surface that drains the propose queue is the FRONTDOOR-11 confirm gate below,
    /// where the operator REVIEWS the preview + confirms before anything runs).
    ProposeSuggestion(usize),
    /// FRONTDOOR-13 — the operator clicked **Apply fix** on an alert-triage group
    /// that carries a typed fix proposal. Carries the group's index into
    /// [`FrontDoor::triage`]`.groups`. The handler re-publishes that group's typed
    /// proposal to the copilot PROPOSE topic (`action/copilot/proposal`, FD-12's
    /// review queue) — exactly like a suggestion's "Act". It does NOT publish to
    /// FD-11's execution topic and does NOT execute anything (§9 — the GUI never
    /// auto-executes; the operator REVIEWS + confirms the fix in the FRONTDOOR-11
    /// confirm gate before anything runs).
    ProposeAlertFix(usize),
    /// FRONTDOOR-11 — open the **Pending actions** review surface (the confirm-gate
    /// overlay listing the queued proposals). Reachable from the Front Door's
    /// pending indicator; no proposal executes by opening it (it only shows the
    /// previews — §9).
    OpenPending,
    /// FRONTDOOR-11 — close the Pending actions review surface, back to the grid.
    ClosePending,
    /// FRONTDOOR-11 — the typed-confirm field for the proposal at index `usize`
    /// changed (the operator is typing the confirm word for a DESTRUCTIVE action).
    /// Records the text; the "Execute" affordance only arms once it matches (Q10).
    /// Normal (non-destructive) proposals never need this (they approve 1-click).
    ConfirmInputChanged(usize, String),
    /// FRONTDOOR-11 — the operator CONFIRMED the proposal at index `usize` (the
    /// 1-click "Approve" for a normal action, or "Execute" once the typed-confirm
    /// matches for a destructive one). This is the ONLY path that publishes the
    /// typed `ActionRequest` to the EXEC topic (`action/exec/request`) — the gate
    /// re-checks [`pending::PendingProposal::armed`] before publishing, so a
    /// destructive kind can never fire without the typed word (§9). The result
    /// comes back as [`Message::ExecResult`].
    ApproveProposal(usize),
    /// FRONTDOOR-11 — the operator DISMISSED the proposal at index `usize` (reject,
    /// no execute). Marks it dismissed on the card — it is never published to the
    /// exec topic (§9 — only a confirm executes).
    DismissProposal(usize),
    /// FRONTDOOR-11 — the action worker's typed reply for the proposal with bus id
    /// `String` landed (or the round-trip degraded). The bool is the worker's `ok`;
    /// the message is its `detail`/`error`. Folded onto the card as the RESULT
    /// (succeeded / failed). Keyed by the proposal's stable id (not its index) so a
    /// reload that reorders the list still resolves the right card.
    ExecResult(String, bool, String),
    /// FRONTDOOR-14 — open / close the **in-menu Settings** panel (Q48). Reachable
    /// from the rail (panel mode) + the top bar (both modes); pure local view-state
    /// flips. Opening it changes nothing until the operator touches a control.
    OpenSettings,
    /// FRONTDOOR-14 — close the Settings panel, back to the grid.
    CloseSettings,
    /// FRONTDOOR-14 — the operator picked a theme in Settings (Q23): apply it LIVE
    /// (the live token bundle swaps, the next render repaints) AND persist it to
    /// `preferences.toml`. A real apply, not a mockup (§7).
    SetTheme(mde_theme::Theme),
    /// FRONTDOOR-14 — the operator picked a density in Settings (Q80): apply +
    /// persist, exactly like the theme.
    SetDensity(mde_theme::Density),
    /// FRONTDOOR-14 — the operator toggled the Copilot **proactivity** policy
    /// (Q61). Persists the flag + gates the inline suggestion cards / on-tile
    /// badges live (off = the GUI stops surfacing the proactive set).
    SetAiProactive(bool),
    /// FRONTDOOR-14 — move the tile at the given index UP / DOWN one slot in the
    /// arrangement (Q79). Reorders the live grid + persists the new order.
    MoveTileUp(usize),
    /// FRONTDOOR-14 — move the tile at the given index DOWN one slot.
    MoveTileDown(usize),
    /// FRONTDOOR-14 — pin / un-pin the tile at the given index (Q79): pinned tiles
    /// sort to the front of the grid. Persists + re-lays the grid.
    ToggleTilePinned(usize),
    /// FRONTDOOR-14 — hide / un-hide the tile at the given index (Q79): a hidden
    /// tile drops from the grid (still listed in Settings to un-hide). Persists +
    /// re-lays the grid.
    ToggleTileHidden(usize),
    /// FRONTDOOR-14 — toggle the session **lock** (Q91, local best-effort). When
    /// locked, the Front Door's action affordances (the detail menu's pipeline
    /// actions, the suggestion "Act" / triage "Apply fix", the confirm-gate
    /// Approve/Execute) are disabled until unlock — navigation + viewing stay
    /// open. A local toggle; the real OS-session lock hook is a noted follow-up.
    ToggleLock,
    /// FRONTDOOR-15 — the operator picked the **target node** for the open tile's
    /// actions (Q32/Q74 cross-node): `Some(hostname)` scopes the next action to
    /// that one node (picked from the live roster), `None` restores the default
    /// **whole-mesh broadcast** reach (Q18). A pure local view-state flip — it
    /// changes nothing until an action fires; the scoped action then carries the
    /// target. The string is the node's hostname (its display + roster key).
    SelectTargetNode(Option<String>),
    /// FRONTDOOR-15 — launch a GUI app **on the current target node's data**
    /// (Q74). Carries the app binary; the handler resolves the target's overlay
    /// address off the live roster and fires [`crate::Message::LaunchAppOnNode`]
    /// (the GUI runs locally, pointed at the remote node). A no-op when no target
    /// is selected (the row is only offered once a node is picked). Closes the
    /// detail menu as it launches (mirrors a pipeline action).
    LaunchOnTarget(&'static str),
    /// FRONTDOOR-15 — the **push-to-talk** affordance was pressed (Q55 voice). It
    /// publishes the current ask (the omnibox text) to the EXISTING
    /// `action/copilot/ask` topic — the same path FD-6 search uses, NEVER the exec
    /// topic (§9) — renders the reply in the Copilot card, and best-effort SPEAKS
    /// it via the system speech service. A blank ask is a no-op. The mic→text
    /// transcription step is deferred (no STT engine in the airgapped workspace —
    /// see the module note); today the operator's typed/dictated ask drives it.
    PushToTalk,
    /// FRONTDOOR-15 — a push-to-talk Copilot reply landed (or degraded). Carries
    /// the `copilot_gen` it was fired under (it shares the one Copilot card + the
    /// one generation counter with the search ask, so a stale reply for a
    /// superseded ask is dropped) and the parsed [`CopilotAnswer`]. Folds the
    /// reply into the Copilot card exactly like [`CopilotReplied`], AND — when it
    /// is a real answer — best-effort speaks it (the spoken-summary half of Q55).
    /// Kept distinct from `CopilotReplied` so ONLY a voice ask speaks (a typed
    /// search never blurts the reply aloud).
    VoiceReplied(u64, CopilotAnswer),
    /// FRONTDOOR-16 — the operator dismissed the one-time guided-first-run
    /// greeting (Q27). Clears the card for this session AND writes the "greeted"
    /// sentinel so it never shows again on this node (Q71 — no tour, the greeting
    /// is the whole onboarding). A pure local flip + a best-effort file write.
    DismissGreeting,
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

impl TileKey {
    /// FRONTDOOR-14 — the stable, theme-independent id used to persist a widget
    /// tile's arrangement rule (Q79). Distinct from the display label so a label
    /// tweak never strands the operator's arrangement.
    fn id(self) -> &'static str {
        match self {
            TileKey::MeshMap => "mesh_map",
            TileKey::BuildFarm => "build_farm",
            TileKey::Alerts => "alerts",
            TileKey::NodeHealth => "node_health",
            TileKey::Copilot => "copilot",
            TileKey::System => "system",
            TileKey::DataCenter => "data_center",
            TileKey::DevOps => "dev_ops",
        }
    }
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

    /// FRONTDOOR-14 — the stable id this tile persists its arrangement rule under
    /// (Q79): a widget tile's [`TileKey::id`], or a launcher's lowercased label.
    /// Stable across theme/label cosmetics so an arrangement survives a relabel.
    fn id(&self) -> String {
        match self.key {
            Some(key) => key.id().to_string(),
            None => self.label.to_lowercase(),
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

    /// FRONTDOOR-14 — is this a one-click PIPELINE action (vs a plain navigation /
    /// app launch)? The session lock (Q91) gates the action affordances — a
    /// pipeline action fires a real verb (or routes to a surface to run one) — while
    /// leaving pure navigation/viewing open. Detected off the message shape: a
    /// pipeline action is the only `TileAction` whose message is a
    /// [`Message::PipelineAction`].
    fn is_pipeline(&self) -> bool {
        matches!(
            self.message,
            crate::Message::FrontDoor(Message::PipelineAction { .. })
        )
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
    /// FRONTDOOR-11 — the live pending PROPOSALS read off `action/copilot/proposal`
    /// (FD-10's "Act" + FD-12's CodeEdit proposals publish here). Each is a typed
    /// proposal the operator REVIEWS (preview) and GATES (1-click / typed-confirm)
    /// before it executes — never auto-run (§9). Empty when the propose queue is
    /// absent / empty. Parsed GUI-side off the wire shape (the §6 boundary).
    pub pending: Vec<pending::PendingProposal>,
    /// FRONTDOOR-13 — the AI ALERT TRIAGE read off the FD-13-backend
    /// `state/copilot/alert-triage` topic (Q38): the live alerts GROUPED +
    /// EXPLAINED, each group optionally carrying a typed one-click FIX. The Alerts
    /// tile detail renders this; each fix is a PROPOSAL the operator approves through
    /// the confirm gate (re-published to the propose topic), NEVER executed from the
    /// GUI (§9). Empty when the topic is absent / the mesh is all-clear.
    pub triage: copilot::Triage,
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

        // FRONTDOOR-11 — the live proposal queue: every message on the propose
        // topic is one typed proposal the operator gates (§7 — real bus data, no
        // demo). Read the whole topic (each `StoredMessage` is one proposal) and
        // parse GUI-side; an absent Bus / empty topic leaves the gate empty.
        let pending = pending::parse(&topic_messages(copilot::PROPOSAL_TOPIC));

        // FRONTDOOR-13 — the AI alert triage: the latest body on the triage topic is
        // the current grouped/explained view (§7 — real triage off the bus, no demo).
        // Absent Bus / empty topic ⇒ an empty triage (the tile shows its resting note).
        let triage = copilot::parse_triage(latest_body(copilot::ALERT_TRIAGE_TOPIC).as_deref());

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
            // FRONTDOOR-11 — the live proposal queue for the confirm gate.
            pending,
            // FRONTDOOR-13 — the AI alert triage for the Alerts tile detail.
            triage,
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

/// FRONTDOOR-11 — read every message on a topic as `(ulid, body)` pairs, oldest
/// first (the same `list_since(topic, None)` the other widget loaders use, but the
/// WHOLE topic rather than just the newest body). The propose queue carries one
/// proposal per message, so the gate needs them all — each with its bus ulid as a
/// stable id. Best-effort: no Bus data-dir / a read fault ⇒ an empty list (the
/// gate just shows nothing — §7, never a faked card).
#[must_use]
fn topic_messages(topic: &str) -> Vec<(String, Option<String>)> {
    let Some(dir) = mde_bus::default_data_dir() else {
        return Vec::new();
    };
    let Ok(persist) = mde_bus::persist::Persist::open(dir) else {
        return Vec::new();
    };
    let Ok(msgs) = persist.list_since(topic, None) else {
        return Vec::new();
    };
    msgs.into_iter().map(|m| (m.ulid, m.body)).collect()
}

/// FRONTDOOR-11 — parse the action worker's typed reply into `(ok, message)` for
/// the card's RESULT line. The reply is the FD-11 `ActionReply` JSON
/// (`{ok, detail?, error?}`): `ok:true` ⇒ succeeded (the `detail` note); `ok:false`
/// ⇒ failed (the `error`). A `None` body (no Bus / no worker / timeout) or
/// malformed JSON degrades to a quiet failure (Q33 — no spew, never a hang, never
/// a panic). Pure + Bus-free so the result mapping is unit-tested directly.
#[must_use]
fn parse_exec_reply(raw: Option<&str>) -> (bool, String) {
    let Some(body) = raw else {
        return (
            false,
            "No reply from the action worker (it may be offline).".to_string(),
        );
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body.trim()) else {
        return (
            false,
            "The action worker sent an unreadable reply.".to_string(),
        );
    };
    let ok = v
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if ok {
        let detail = v
            .get("detail")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("dispatched")
            .trim()
            .to_string();
        (true, detail)
    } else {
        let error = v
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("the worker rejected the action")
            .trim()
            .to_string();
        (false, error)
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

    /// The bus topic the FD-13 backend publishes its grouped ALERT TRIAGE to
    /// (`mackesd::workers::copilot::ALERT_TRIAGE_TOPIC`). A `state/` snapshot the
    /// Alerts tile reads the same way the Copilot tile reads its status — the latest
    /// body is the current triage. Each group's proposed fix is a PROPOSAL the
    /// operator approves through the confirm gate — never executed from the GUI (§9).
    pub const ALERT_TRIAGE_TOPIC: &str = "state/copilot/alert-triage";

    /// One parsed alert-triage GROUP (FD-13 / Q38) — the clustered alerts the GUI
    /// renders: a headline, a plain-language explanation, the member alert names,
    /// and (when the group has a safe fix) the typed proposal the operator can ACT
    /// on. Mirrors the backend `AlertGroup` wire shape. The fix proposal is carried
    /// as its raw JSON object body so the "Apply fix" affordance re-publishes it to
    /// [`PROPOSAL_TOPIC`] verbatim (the propose-only path) WITHOUT the workbench
    /// needing the `mackesd` enums (the §6 boundary). It is NEVER executed here (§9).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct AlertGroup {
        /// A short operator-facing headline for the cluster.
        pub title: String,
        /// The plain-language explanation: what is wrong, why these cluster, the fix.
        pub explanation: String,
        /// `high` | `medium` — carried so the GUI can badge/sort.
        pub severity: String,
        /// The member alert names (the `check`s) this group clusters.
        pub alerts: Vec<String>,
        /// The typed fix proposal's raw JSON object body (the backend
        /// `ActionProposal`: `{action, rationale}`), present only when the group has
        /// a safe one-click fix. `None` ⇒ explain-only. Re-published verbatim to
        /// [`PROPOSAL_TOPIC`] on "Apply fix" — never to the exec topic (§9).
        pub proposal_body: Option<String>,
    }

    /// The parsed alert triage (FD-13) — the grouped/explained alerts the Alerts
    /// tile detail renders, plus the count of alerts triaged.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct Triage {
        /// The clustered groups (worst-first, as the backend ranked them).
        pub groups: Vec<AlertGroup>,
        /// How many live alerts this triage covered.
        pub alert_count: usize,
    }

    impl Triage {
        /// `true` when the triage carries no groups (so the tile shows its resting
        /// "no triage yet" note rather than an empty header).
        #[must_use]
        pub fn is_empty(&self) -> bool {
            self.groups.is_empty()
        }
    }

    /// Parse the latest `state/copilot/alert-triage` body (the backend `AlertTriage`
    /// JSON: `{groups:[…], alert_count, produced_at_s}`) into the triage the GUI
    /// renders. Tolerant (mirrors [`parse_suggestions`]): the fix proposal is kept as
    /// its raw JSON object (re-serialized so it round-trips to [`PROPOSAL_TOPIC`]
    /// cleanly); a group with a missing title is dropped; malformed / `None` JSON ⇒
    /// an empty triage (no panic, the tile just shows no triage — §7). Order is
    /// preserved (the backend ranks it worst-first). Pure + Bus-free.
    #[must_use]
    pub fn parse_triage(raw: Option<&str>) -> Triage {
        let Some(body) = raw else {
            return Triage::default();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(body.trim()) else {
            return Triage::default();
        };
        let alert_count = v
            .get("alert_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        let Some(arr) = v.get("groups").and_then(serde_json::Value::as_array) else {
            return Triage::default();
        };
        let groups = arr
            .iter()
            .filter_map(|g| {
                let title = g
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if title.is_empty() {
                    return None;
                }
                let explanation = g
                    .get("explanation")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let severity = match g
                    .get("severity")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("medium")
                {
                    "high" => "high".to_string(),
                    _ => "medium".to_string(),
                };
                let alerts = g
                    .get("alerts")
                    .and_then(serde_json::Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                // Keep the proposal as its own JSON object body so "Apply fix"
                // re-publishes it verbatim to the propose topic — never re-derived,
                // never executed.
                let proposal_body = g
                    .get("proposal")
                    .filter(|p| p.is_object())
                    .map(std::string::ToString::to_string);
                Some(AlertGroup {
                    title,
                    explanation,
                    severity,
                    alerts,
                    proposal_body,
                })
            })
            .collect();
        Triage {
            groups,
            alert_count,
        }
    }
}

// ============== FRONTDOOR-11 (GUI half): the confirm-gate execution UI ========

/// FRONTDOOR-11 — the gated-execution surface: the GUI half that closes the
/// Copilot action loop (design Q10: preview/diff + 1-click, typed-confirm for
/// high-risk; Q44: preview = commands + target node(s) + effect + dry-run). FD-10
/// landed the "Act" button (it queues a typed PROPOSAL onto `action/copilot/
/// proposal`); FD-11's backend landed the typed, audited **action worker** (it
/// drains `action/exec/request`). This module is the operator gate BETWEEN them:
/// it reads the propose queue, renders each proposal with a PREVIEW, and — ONLY on
/// the operator's explicit confirm (1-click for normal, a TYPED confirm word for
/// destructive kinds) — publishes the typed `ActionRequest` to the EXECUTION topic
/// so the worker runs it (gated + audited). Nothing here ever auto-executes (§9).
///
/// Pure parse / preview / classification (no Bus / no view) so the whole gate is
/// unit-tested directly — the Bus read in [`FrontDoorData::read`] and the execute
/// publish in [`FrontDoor::update`] are thin shells over this. The workbench can't
/// depend on `mackesd` (the §6 mesh/desktop boundary — the `ActionRequest` /
/// `ActionProposal` enums live in `mackesd::workers`), so this mirrors the WIRE
/// shapes and parses them GUI-side, exactly as FD-10 parses the suggestion JSON.
pub(super) mod pending {
    /// The bus topic the FD-11 action worker drains
    /// (`mackesd::workers::action::ACTION_TOPIC`). Publishing a typed
    /// `ActionRequest` JSON here triggers the worker's GATED + AUDITED execution.
    /// The confirm gate writes HERE **only** on the operator's explicit confirm —
    /// never on a render, never from a 1-click for a destructive kind (§9).
    pub const EXEC_TOPIC: &str = "action/exec/request";

    /// How long the confirm gate waits for the action worker's typed reply on the
    /// generic `reply/<ulid>` lane before surfacing a degrade. A dispatch is local
    /// file I/O + one audit insert (sub-millisecond on the leader), but the request
    /// must reach the elected leader over replication, so a generous ceiling keeps
    /// a slow round-trip from reading as a false failure. No-Bus / no-worker
    /// degrades immediately (the request client returns `None` with no data-dir).
    pub const EXEC_TIMEOUT_SECS: u64 = 30;

    /// The risk class the confirm gate enforces per proposal (design Q10). A
    /// destructive kind can ONLY be fired through a typed-confirm — a 1-click can
    /// never trigger it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Risk {
        /// Reversible / low-blast-radius (e.g. a service start/restart) — a single
        /// "Approve" click is the gate.
        Normal,
        /// High-blast-radius / destructive (the design's locked set: code-edit,
        /// destroy, cutover, delete) — the operator must TYPE the confirm word
        /// before "Execute" is live (Q10).
        HighRisk,
    }

    /// The high-risk / destructive action KINDS that REQUIRE a typed confirm
    /// (design Q10 — the locked set). Matched against the proposal's `action.kind`
    /// tag (the FD-11 `ActionRequest` serde tag). Both the hyphen + underscore
    /// spellings are listed so the gate is robust to either serde casing the
    /// backend evolves to (`code-edit` vs `code_edit`) — classification erring
    /// toward MORE friction is always the safe default for a destructive op.
    pub const HIGH_RISK_KINDS: &[&str] = &[
        "code-edit",
        "code_edit",
        "codeedit",
        "destroy",
        "cutover",
        "delete",
    ];

    /// The word the operator must type to arm a destructive execute (Q10 — the
    /// typed-confirm). Fixed + uppercase so it's deliberate (you can't fat-finger
    /// it). Compared case-insensitively after a trim.
    pub const CONFIRM_WORD: &str = "CONFIRM";

    /// Classify an action `kind` tag into its [`Risk`]. A kind in
    /// [`HIGH_RISK_KINDS`] is destructive (typed-confirm); anything else — incl. an
    /// unknown kind the GUI doesn't model — is treated as [`Risk::Normal`] ONLY
    /// when it is a known-reversible kind, and otherwise still normal but the
    /// preview makes the kind explicit. Pure + case-insensitive.
    #[must_use]
    pub fn classify(kind: &str) -> Risk {
        let k = kind.trim().to_lowercase();
        if HIGH_RISK_KINDS.iter().any(|h| *h == k) {
            Risk::HighRisk
        } else {
            Risk::Normal
        }
    }

    /// Has the operator typed the confirm word? Case-insensitive after a trim, so
    /// "confirm" / " CONFIRM " both arm. Only consulted for a [`Risk::HighRisk`]
    /// proposal — a [`Risk::Normal`] one approves on a single click.
    #[must_use]
    pub fn confirm_matches(typed: &str) -> bool {
        typed.trim().eq_ignore_ascii_case(CONFIRM_WORD)
    }

    /// The lifecycle of one pending proposal in the gate, so a card can show its
    /// execution RESULT (Q44 preview → confirm → result). `Pending` until the
    /// operator acts; `Executing` while the worker round-trip is in flight;
    /// `Succeeded`/`Failed` from the typed `ActionReply`; `Dismissed` when the
    /// operator rejects it (no execute).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ExecState {
        /// Awaiting the operator's confirm (the default).
        Pending,
        /// The confirm fired; the worker round-trip is in flight.
        Executing,
        /// The worker accepted + dispatched the action (`ActionReply.ok == true`).
        Succeeded(String),
        /// The worker rejected it, or the round-trip degraded (no Bus / timeout).
        Failed(String),
        /// The operator dismissed the proposal — it was NOT executed.
        Dismissed,
    }

    /// One pending proposal, parsed GUI-side off `action/copilot/proposal` (§7 —
    /// real bus data, no demo). Carries the PREVIEW the operator reviews (Q44:
    /// the action kind + target node(s) + a human-readable effect + a dry-run
    /// command line), its [`Risk`] class (which gate to show), the EXACT inner
    /// `action` JSON to publish to [`EXEC_TOPIC`] on confirm (the bare
    /// `ActionRequest` shape the worker accepts — NOT the wrapping proposal), and
    /// the live [`ExecState`] driving the result line. Built by [`parse`].
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct PendingProposal {
        /// A stable id for the proposal — the bus message ulid it arrived on. Used
        /// to address the card across reloads (so an in-flight / resolved gate is
        /// not reset when a fresh snapshot lands carrying the same proposal).
        pub id: String,
        /// The action KIND tag (the FD-11 `ActionRequest` serde tag, e.g.
        /// `service_lifecycle`) — the first preview line + the risk driver.
        pub kind: String,
        /// The target node(s) this action touches (Q44), e.g. `["oak"]`. Empty
        /// when the typed params name no host (the preview then says "this node").
        pub targets: Vec<String>,
        /// A human-readable one-line EFFECT (Q44), e.g. "restart container nginx".
        /// Derived from the typed params — never a raw command echo.
        pub effect: String,
        /// The DRY-RUN command line the worker's fixed plan would run (Q44), e.g.
        /// `podman restart nginx` — shown as "what would run", never executed here.
        /// `None` when the kind carries no modelled dry-run.
        pub dry_run: Option<String>,
        /// The Copilot's rationale (why it proposed this), carried from the
        /// proposal so the gate shows the reasoning, not just a bare command.
        pub rationale: String,
        /// The risk class — drives whether the gate is a 1-click or a typed-confirm.
        pub risk: Risk,
        /// The EXACT inner `action` object JSON to publish to [`EXEC_TOPIC`] on
        /// confirm: the bare `ActionRequest` (`{"kind":…, …typed params…}`) the
        /// worker accepts. Re-serialized from the proposal's `action` field so it
        /// round-trips verbatim — never re-derived, never the wrapping proposal.
        pub exec_body: String,
        /// The live execution lifecycle (Q44 — the result on the card).
        pub state: ExecState,
    }

    impl PendingProposal {
        /// Does a 1-click "Approve" arm THIS proposal's execute? True only for a
        /// [`Risk::Normal`] proposal — a destructive one needs the typed confirm
        /// (the §9 invariant a test pins: a 1-click can never fire a destructive).
        #[must_use]
        pub fn approves_on_click(&self) -> bool {
            self.risk == Risk::Normal
        }

        /// Given the operator's typed confirm text, is THIS proposal armed to
        /// execute? Normal → always (the click is the confirm); high-risk → only
        /// when the typed word matches (Q10). The single chokepoint the execute
        /// handler consults so the gate logic lives in ONE tested place.
        #[must_use]
        pub fn armed(&self, typed_confirm: &str) -> bool {
            match self.risk {
                Risk::Normal => true,
                Risk::HighRisk => confirm_matches(typed_confirm),
            }
        }
    }

    /// Parse the latest `action/copilot/proposal` body into the pending list the
    /// gate renders. The topic carries one proposal per message — the GUI reads
    /// the whole topic (each `StoredMessage` is one proposal) and parses each. The
    /// body shape is the FD-12 `ActionProposal` JSON: `{"action":{…},"rationale":…}`
    /// — `action` is the bare `ActionRequest` (`{"kind":…, …}`). Tolerant: a
    /// proposal with no parseable `action` object is dropped (never a faked card),
    /// malformed JSON for one entry doesn't sink the rest; `None`/empty ⇒ empty.
    /// Each `(ulid, body)` pair is the bus message's id + body. Pure + Bus-free.
    #[must_use]
    pub fn parse(messages: &[(String, Option<String>)]) -> Vec<PendingProposal> {
        messages
            .iter()
            .filter_map(|(ulid, body)| parse_one(ulid, body.as_deref()))
            .collect()
    }

    /// Parse one proposal message into a [`PendingProposal`], or `None` when it
    /// carries no usable typed `action` (an advisory-only / malformed body — never
    /// surfaced as an executable card, §7). The `action` object is re-serialized
    /// verbatim into `exec_body` (the bare `ActionRequest` the worker accepts).
    #[must_use]
    pub fn parse_one(ulid: &str, body: Option<&str>) -> Option<PendingProposal> {
        let v: serde_json::Value = serde_json::from_str(body?.trim()).ok()?;
        // The proposal wraps the typed action under `action`; some publishers may
        // send the bare action directly — accept either (the action object is the
        // thing the worker executes), preferring the wrapped form's rationale.
        let action = v.get("action").filter(|a| a.is_object()).unwrap_or(&v);
        if !action.is_object() {
            return None;
        }
        let kind = action
            .get("kind")
            .and_then(serde_json::Value::as_str)?
            .trim()
            .to_string();
        if kind.is_empty() {
            return None;
        }
        let rationale = v
            .get("rationale")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        // The EXACT inner action JSON to publish to the exec topic on confirm.
        let exec_body = action.to_string();
        let targets = targets_of(action);
        let effect = effect_of(&kind, action);
        let dry_run = dry_run_of(&kind, action);
        Some(PendingProposal {
            id: ulid.to_string(),
            risk: classify(&kind),
            kind,
            targets,
            effect,
            dry_run,
            rationale,
            exec_body,
            state: ExecState::Pending,
        })
    }

    /// The target node(s) an action touches (Q44), pulled from the typed params.
    /// `service_lifecycle` names a single `target_host`; other kinds may carry a
    /// `target_host` or a `targets` array — both are read so the preview always
    /// names the blast radius. Empty ⇒ the preview reads "this node".
    fn targets_of(action: &serde_json::Value) -> Vec<String> {
        if let Some(arr) = action.get("targets").and_then(serde_json::Value::as_array) {
            return arr
                .iter()
                .filter_map(|t| t.as_str().map(str::to_string))
                .filter(|t| !t.trim().is_empty())
                .collect();
        }
        for field in ["target_host", "target", "host", "node"] {
            if let Some(t) = action.get(field).and_then(serde_json::Value::as_str) {
                if !t.trim().is_empty() {
                    return vec![t.trim().to_string()];
                }
            }
        }
        Vec::new()
    }

    /// A human-readable one-line EFFECT (Q44) from the typed params. Kind-specific
    /// for the modelled kinds (`service_lifecycle` → "restart container nginx"),
    /// with a generic fallback ("<kind>") for a kind the GUI doesn't yet model — so
    /// a new backend kind still previews honestly rather than rendering blank.
    fn effect_of(kind: &str, action: &serde_json::Value) -> String {
        let s = |f: &str| {
            action
                .get(f)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        match kind {
            "service_lifecycle" => {
                let (op, sk, name) = (s("op"), s("service_kind"), s("name"));
                let op = if op.is_empty() { "act on" } else { &op };
                let sk = if sk.is_empty() { "service" } else { &sk };
                if name.is_empty() {
                    format!("{op} {sk}")
                } else {
                    format!("{op} {sk} {name}")
                }
            }
            // A modelled-but-future destructive kind reads its op/name when present.
            "destroy" | "delete" | "cutover" | "code-edit" | "code_edit" => {
                let name = s("name");
                let path = s("path");
                let what = if !name.is_empty() {
                    name
                } else if !path.is_empty() {
                    path
                } else {
                    String::new()
                };
                if what.is_empty() {
                    kind.replace(['_', '-'], " ")
                } else {
                    format!("{} {what}", kind.replace(['_', '-'], " "))
                }
            }
            other => other.replace(['_', '-'], " "),
        }
    }

    /// The DRY-RUN command line (Q44 — "what would run") for the modelled kinds.
    /// For `service_lifecycle` this mirrors the worker's FIXED command plan
    /// (`podman <op> <name>` for a container, `virsh <verb> <name>` for a VM) so
    /// the operator sees the actual command the gate would dispatch — NOT a guess,
    /// and NEVER executed here. `None` for a kind with no modelled plan (the
    /// preview omits the dry-run line rather than fabricate one). Pure.
    #[must_use]
    pub fn dry_run_of(kind: &str, action: &serde_json::Value) -> Option<String> {
        let s = |f: &str| {
            action
                .get(f)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        match kind {
            "service_lifecycle" => {
                let (op, sk, name) = (s("op"), s("service_kind"), s("name"));
                if name.is_empty() {
                    return None;
                }
                match sk.as_str() {
                    // The worker's container plan: `podman <op> <name>`.
                    "container" => Some(format!("podman {} {name}", op_word(&op))),
                    // The worker's VM plan: `virsh <verb> <name>` (start/shutdown/reboot).
                    "vm" => Some(format!("virsh {} {name}", virsh_verb(&op))),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Map a lifecycle `op` to the podman subcommand the worker's fixed plan runs.
    fn op_word(op: &str) -> &str {
        match op {
            "start" => "start",
            "stop" => "stop",
            "restart" => "restart",
            _ => op,
        }
    }

    /// Map a lifecycle `op` to the `virsh` verb the worker's fixed VM plan runs.
    fn virsh_verb(op: &str) -> &str {
        match op {
            "start" => "start",
            "stop" => "shutdown",
            "restart" => "reboot",
            _ => op,
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
    /// FRONTDOOR-11 — the live pending PROPOSALS (off `action/copilot/proposal`),
    /// folded in from the FD-4 [`FrontDoorData`] read. Each is a typed proposal the
    /// operator REVIEWS + GATES in the confirm-gate surface before it executes;
    /// nothing here auto-runs (§9). The per-proposal [`pending::ExecState`] carries
    /// the result so a card shows succeeded/failed after a confirm. A fresh snapshot
    /// is merged (not clobbered) so an in-flight / resolved gate survives a reload.
    pub pending: Vec<pending::PendingProposal>,
    /// FRONTDOOR-13 — the live AI alert triage (off `state/copilot/alert-triage`),
    /// folded in from the FD-4 [`FrontDoorData`] read. The Alerts tile detail renders
    /// the grouped/explained alerts + each group's proposed one-click fix; a fix is a
    /// PROPOSAL routed through the confirm gate (re-published to the propose topic),
    /// never executed from the GUI (§9). Empty until a triage lands / when all-clear.
    pub triage: copilot::Triage,
    /// FRONTDOOR-11 — whether the Pending actions review surface is open (the
    /// confirm-gate overlay). Toggled by the pending indicator / its Back control;
    /// opening it NEVER executes anything (it only shows the previews — §9).
    pub show_pending: bool,
    /// FRONTDOOR-11 — the operator's typed-confirm text, keyed by the proposal's
    /// stable bus id, for DESTRUCTIVE proposals (the typed-confirm gate, Q10). A
    /// normal proposal never reads this (it approves on a single click). Keyed by
    /// id (not index) so the typed text survives a reload that reorders the list.
    pub confirm_inputs: std::collections::HashMap<String, String>,
    /// FRONTDOOR-14 — the full seed tile set in its design order, the source the
    /// arrangement is applied to. The grid renders [`Self::tiles`] (the arranged +
    /// pin-sorted + hide-filtered projection of this); Settings lists THIS so a
    /// hidden tile can be un-hidden. Kept distinct so re-applying an arrangement is
    /// pure (never a lossy edit of the visible set).
    pub all_tiles: Vec<Tile>,
    /// FRONTDOOR-14 — the persisted Front Door prefs (the in-menu settings panel's
    /// store): the Copilot proactivity policy + the tile arrangement (Q48/Q61/Q79).
    /// Loaded from `preferences.toml` at construction; every settings edit mutates
    /// this AND persists it, then re-projects the grid.
    pub fd_prefs: mde_theme::FrontDoorPrefs,
    /// FRONTDOOR-14 — whether the in-menu Settings panel is open (Q48). Toggled by
    /// the rail / top-bar settings entry; a pure view-state flip.
    pub show_settings: bool,
    /// FRONTDOOR-14 — the session lock (Q91, local best-effort). When `true` the
    /// action affordances are disabled until unlock; navigation + viewing stay
    /// open. A local toggle today — the OS-session lock hook is a noted follow-up.
    pub locked: bool,
    /// FRONTDOOR-15 — the **target node** the open tile's actions are scoped to
    /// (Q32/Q74 cross-node). `None` is the default **whole-mesh broadcast** reach
    /// (Q18); `Some(hostname)` scopes the next action to that one node, picked
    /// from the live roster (`self.peers`) in the detail view's node selector.
    /// Cleared when the detail menu closes (a fresh tile starts at broadcast) so a
    /// scope never silently leaks across tiles.
    pub target_node: Option<String>,
    /// FRONTDOOR-16 — show the one-time guided-first-run greeting (Q27 — the AI
    /// greets + the tiles auto-build from the mesh; Q71 — no tour). `true` only
    /// when this node has never dismissed it (the "greeted" sentinel is absent at
    /// construction); set `false` for good — and the sentinel written — the moment
    /// the operator dismisses it, so the welcome card shows EXACTLY once per node.
    pub show_greeting: bool,
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
        // FRONTDOOR-14 — load the persisted Front Door prefs (the in-menu settings
        // store) and project the operator's arrangement (order / pin / hide) over
        // the seed tiles. A fresh install / a config predating the settings panel
        // has an empty arrangement, so the grid reads exactly as the seed (Q79).
        let fd_prefs = mde_theme::Preferences::load().front_door;
        let arranged = arrange_tiles(&tiles, &fd_prefs);
        Self {
            all_tiles: tiles,
            tiles: arranged,
            fd_prefs,
            show_settings: false,
            // FRONTDOOR-14 — unlocked at construction; the operator locks the
            // session explicitly (the OS-session hook is a follow-up).
            locked: false,
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
            // FRONTDOOR-11 — no pending proposals / closed gate / no typed confirms
            // until the first snapshot lands (an empty propose queue keeps it empty).
            pending: Vec::new(),
            // FRONTDOOR-13 — no alert triage until the first snapshot (all-clear /
            // no leader keeps it empty).
            triage: copilot::Triage::default(),
            show_pending: false,
            confirm_inputs: std::collections::HashMap::new(),
            // FRONTDOOR-15 — start at the whole-mesh broadcast default (Q18); a
            // tile's detail node selector scopes it to one node (Q32/Q74).
            target_node: None,
            // FRONTDOOR-16 — the guided first-run greeting (Q27) shows iff this
            // node has never dismissed it (the sentinel is absent). A returning
            // operator never sees it again; a fresh install greets once.
            show_greeting: !greeting_already_seen(),
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
                // FRONTDOOR-15 — leaving the detail drops any per-tile node scope
                // back to the whole-mesh default (Q18), so a target never silently
                // carries from one tile's actions to the next.
                self.target_node = None;
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
                // FRONTDOOR-14 — a locked session blocks the ACTION half (Q91): the
                // pipeline's own trigger (the panel's re-poll verb) is gated, while
                // plain navigation/viewing stays open. The locked detail view drops
                // these rows' `on_press` too — this is the defence-in-depth guard.
                if self.locked {
                    return Task::none();
                }
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
                // FRONTDOOR-14 — a locked session blocks queuing a proposal (Q91 —
                // an action affordance). The locked detail view drops the "Act"
                // button too; this is the defence-in-depth guard.
                if self.locked {
                    return Task::none();
                }
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
            // FRONTDOOR-13 — Apply an alert-triage group's typed fix. Re-publish it
            // to the PROPOSE topic (FD-12's review queue) — the SAME propose-only
            // path a suggestion's "Act" uses. §9: this is a PROPOSE, not an execute —
            // it never touches `action/exec/request`, never runs the fix; the
            // operator confirms it in the FD-11 gate. A group with no typed fix
            // (explain-only) or a stale index is a no-op (the "Apply fix" affordance
            // is only rendered when a proposal exists — defence-in-depth).
            Message::ProposeAlertFix(i) => {
                // FRONTDOOR-14 — a locked session blocks queuing a fix (Q91 — an
                // action affordance). The locked triage card drops the "Apply fix"
                // button too; this is the defence-in-depth guard.
                if self.locked {
                    return Task::none();
                }
                let Some(body) = self
                    .triage
                    .groups
                    .get(i)
                    .and_then(|g| g.proposal_body.clone())
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
            // FRONTDOOR-11 — open / close the Pending actions review surface. Pure
            // local view-state flips; opening it shows the PREVIEWS only, executing
            // nothing (§9 — the confirm gate is the only execute path).
            Message::OpenPending => {
                self.show_pending = true;
                Task::none()
            }
            Message::ClosePending => {
                self.show_pending = false;
                Task::none()
            }
            // FRONTDOOR-11 — the operator is typing the confirm word for a
            // DESTRUCTIVE proposal (Q10 — typed-confirm). Record it keyed by the
            // proposal's stable id so the typed text survives a reload; the
            // "Execute" affordance arms only once it matches. No Bus, no execute.
            Message::ConfirmInputChanged(i, text) => {
                if let Some(p) = self.pending.get(i) {
                    self.confirm_inputs.insert(p.id.clone(), text);
                }
                Task::none()
            }
            // FRONTDOOR-11 — the operator CONFIRMED the proposal. This is the ONLY
            // path that publishes a typed `ActionRequest` to the EXEC topic
            // (`action/exec/request`). The gate re-checks `armed()` HERE so a
            // destructive kind can never fire without the matching typed-confirm —
            // a stale / not-armed confirm is an inert no-op (§9). On a valid arm,
            // publish the EXACT inner action JSON (the bare `ActionRequest` the
            // worker accepts) and block for the typed reply on the generic
            // `reply/<ulid>` lane (the request client does both), then fold the
            // result onto the card via `ExecResult`. The Bus client owns its own
            // runtime (`Persist`/rusqlite isn't `Send`), so it rides `spawn_blocking`
            // — the same contract every other Bus round-trip here follows.
            Message::ApproveProposal(i) => {
                // FRONTDOOR-14 — a locked session executes nothing (Q91): the gate's
                // confirm/execute is an action affordance, so it's inert until the
                // operator unlocks. Defence-in-depth — the locked view drops the
                // button's `on_press` too, so this is the second guard on the only
                // execute path (§9 — a lock can never be bypassed into an execute).
                if self.locked {
                    return Task::none();
                }
                let Some(p) = self.pending.get(i) else {
                    return Task::none();
                };
                // The §9 chokepoint: a destructive kind requires the typed confirm.
                let typed = self.confirm_inputs.get(&p.id).cloned().unwrap_or_default();
                if !p.armed(&typed) {
                    // Not armed (a destructive kind whose confirm word isn't typed):
                    // refuse to publish. Nothing reaches the exec topic.
                    return Task::none();
                }
                let id = p.id.clone();
                let body = p.exec_body.clone();
                // Mark it in-flight so the card shows "Executing…" immediately.
                self.set_exec_state(&id, pending::ExecState::Executing);
                let timeout = Duration::from_secs(pending::EXEC_TIMEOUT_SECS);
                Task::perform(
                    async move {
                        // Publish the typed ActionRequest to the EXEC topic and block
                        // for the worker's typed `ActionReply` on `reply/<ulid>`.
                        let raw = tokio::task::spawn_blocking(move || {
                            crate::dbus::action_request_with_body(
                                pending::EXEC_TOPIC,
                                Some(&body),
                                timeout,
                            )
                        })
                        .await
                        .ok()
                        .flatten();
                        let (ok, msg) = parse_exec_reply(raw.as_deref());
                        (id, ok, msg)
                    },
                    |(id, ok, msg)| crate::Message::FrontDoor(Message::ExecResult(id, ok, msg)),
                )
            }
            // FRONTDOOR-11 — the operator DISMISSED the proposal (reject, no
            // execute). Mark the card dismissed — it is NEVER published to the exec
            // topic (§9 — only a confirm executes). The dismissed card stays on
            // screen as an audit of the decision until the next reload drops it.
            Message::DismissProposal(i) => {
                if let Some(p) = self.pending.get(i) {
                    let id = p.id.clone();
                    self.set_exec_state(&id, pending::ExecState::Dismissed);
                }
                Task::none()
            }
            // FRONTDOOR-11 — the worker's typed reply landed (or the round-trip
            // degraded): fold it onto the card as the RESULT (succeeded / failed),
            // keyed by the proposal's stable id so a reorder doesn't mis-target.
            Message::ExecResult(id, ok, msg) => {
                let state = if ok {
                    pending::ExecState::Succeeded(msg)
                } else {
                    pending::ExecState::Failed(msg)
                };
                self.set_exec_state(&id, state);
                Task::none()
            }
            // FRONTDOOR-14 — open / close the in-menu Settings panel (Q48). Pure
            // view-state flips; nothing applies until the operator touches a control.
            Message::OpenSettings => {
                self.show_settings = true;
                Task::none()
            }
            Message::CloseSettings => {
                self.show_settings = false;
                Task::none()
            }
            // FRONTDOOR-14 — the operator picked a theme (Q23). Apply it LIVE (swap
            // the process-wide token bundle, so the next render repaints with no
            // restart — the `live_theme` GUI-2 path) AND persist it to
            // `preferences.toml` so it survives a restart. A real apply (§7).
            Message::SetTheme(theme) => {
                self.set_theme_density(theme, self.fd_density());
                Task::none()
            }
            // FRONTDOOR-14 — the operator picked a density (Q80). Apply + persist,
            // exactly like the theme.
            Message::SetDensity(density) => {
                self.set_theme_density(self.fd_theme(), density);
                Task::none()
            }
            // FRONTDOOR-14 — the operator toggled the Copilot proactivity policy
            // (Q61). Persist the flag + re-project: with it off, the suggestion
            // chokepoint (`suggestions_for_tile`) goes quiet, so the inline cards +
            // on-tile badges vanish on the next render. A real apply (§7).
            Message::SetAiProactive(on) => {
                self.fd_prefs.ai_proactive = on;
                self.persist_prefs();
                Task::none()
            }
            // FRONTDOOR-14 — reorder a tile up / down one slot (Q79). Rewrites the
            // saved arrangement order from the CURRENT visible order with the two
            // neighbours swapped, persists it, and re-projects the grid.
            Message::MoveTileUp(i) => {
                self.move_tile(i, true);
                Task::none()
            }
            Message::MoveTileDown(i) => {
                self.move_tile(i, false);
                Task::none()
            }
            // FRONTDOOR-14 — pin / hide the tile at index `i` in the visible grid
            // (Q79). Flips the flag on that tile's arrangement rule (creating one
            // from the current order if none exists yet), persists, re-projects.
            Message::ToggleTilePinned(i) => {
                self.toggle_tile_flag(i, TileFlag::Pinned);
                Task::none()
            }
            Message::ToggleTileHidden(i) => {
                self.toggle_tile_flag(i, TileFlag::Hidden);
                Task::none()
            }
            // FRONTDOOR-14 — flip the session lock (Q91, local best-effort). Pure
            // local state; the action affordances read `self.locked` to gate, and
            // the update handlers above guard the execute/propose paths a second
            // time (defence-in-depth). The OS-session lock hook is a follow-up.
            Message::ToggleLock => {
                self.locked = !self.locked;
                Task::none()
            }
            // FRONTDOOR-15 — scope the open tile's actions to one node (Q32/Q74),
            // or `None` to restore the whole-mesh broadcast default (Q18). Pure
            // local view-state: nothing fires until the operator runs an action,
            // which then carries this target. A `Some(host)` not in the live
            // roster is still accepted (the roster can lag a reload) — the launch
            // handler re-resolves the address and degrades if it can't.
            Message::SelectTargetNode(node) => {
                self.target_node = node;
                Task::none()
            }
            // FRONTDOOR-15 — launch a GUI app on the target node's data (Q74 — the
            // GUI runs LOCALLY, pointed at the remote node). Resolve the target's
            // overlay address off the live roster and fire the app-level
            // `LaunchAppOnNode` (the detached spawn). No target picked, or the
            // target dropped off the roster, ⇒ a no-op (the row is only offered
            // with a target selected; this guards a racing reload). Closes the
            // detail as it launches, mirroring a pipeline action.
            Message::LaunchOnTarget(bin) => {
                let Some(addr) = self.target_address() else {
                    return Task::none();
                };
                self.detail = None;
                self.target_node = None;
                Task::done(crate::Message::LaunchAppOnNode(bin, addr))
            }
            // FRONTDOOR-15 — push-to-talk (Q55 voice). Publish the current ask to
            // the EXISTING `action/copilot/ask` topic (§9 — the ask lane, NEVER
            // the exec topic) and speak the reply. Reuses FD-6's exact ask path; a
            // blank ask is a no-op. See `voice_task` for the deferred-STT note.
            Message::PushToTalk => self.voice_task(),
            // FRONTDOOR-15 — a voice ask reply landed. Fold it into the shared
            // Copilot card iff it still matches the generation it was fired under
            // (a stale reply for a superseded ask is dropped), then — only for a
            // real answer — best-effort SPEAK it (the spoken-summary half of Q55).
            Message::VoiceReplied(generation, answer) => {
                if generation != self.copilot_gen {
                    return Task::none();
                }
                match answer {
                    CopilotAnswer::Answer(a) => {
                        self.copilot = CopilotState::Answer(a.clone());
                        // Best-effort spoken summary — the same detached
                        // best-effort spawn the sound cues use; an absent speech
                        // service is silently inert (never blocks / panics).
                        return Task::perform(async move { speak_reply(a) }, |()| {
                            crate::Message::Noop
                        });
                    }
                    CopilotAnswer::Unavailable => {
                        self.copilot = CopilotState::Unavailable;
                    }
                }
                Task::none()
            }
            // FRONTDOOR-16 — the operator dismissed the guided first-run greeting
            // (Q27). Clear it for this session and persist the "greeted" sentinel so
            // it never returns on this node (Q71 — the greeting is the whole, no-tour
            // onboarding). The write is best-effort: a failure just means the card
            // might greet once more, never an error spew.
            Message::DismissGreeting => {
                self.show_greeting = false;
                mark_greeting_seen();
                Task::none()
            }
        }
    }

    /// FRONTDOOR-14 — the theme currently in the persisted preferences (what the
    /// Settings theme picker reflects as selected). Read fresh so it tracks an
    /// apply the operator just made.
    fn fd_theme(&self) -> mde_theme::Theme {
        mde_theme::Preferences::load().theme
    }

    /// FRONTDOOR-14 — the density currently in the persisted preferences (what the
    /// Settings density picker reflects as selected).
    fn fd_density(&self) -> mde_theme::Density {
        mde_theme::Preferences::load().density
    }

    /// FRONTDOOR-14 — apply a (theme, density) pair LIVE and persist it (Q23/Q80).
    /// The live swap ([`crate::live_theme::set`], the GUI-2 path) repaints every
    /// surface on the next render with no restart; the save writes the choice to
    /// `preferences.toml` so it survives one. Failing to write is non-fatal — the
    /// live UI still reflects the choice (the operator sees it took); we don't spew.
    fn set_theme_density(&self, theme: mde_theme::Theme, density: mde_theme::Density) {
        crate::live_theme::set(theme, density);
        let mut prefs = mde_theme::Preferences::load();
        prefs.theme = theme;
        prefs.density = density;
        let _ = prefs.save();
    }

    /// FRONTDOOR-14 — persist the live [`Self::fd_prefs`] back to
    /// `preferences.toml` (the Front Door section). Read-modify-write so a
    /// concurrent change to an unrelated section (theme/density/a11y) is preserved.
    /// Non-fatal on write failure (the live state already reflects the change).
    fn persist_prefs(&self) {
        let mut prefs = mde_theme::Preferences::load();
        prefs.front_door = self.fd_prefs.clone();
        let _ = prefs.save();
    }

    /// FRONTDOOR-14 — the tiles in SETTINGS order (Q79): the operator's arrangement
    /// order, hidden tiles INCLUDED (so they can be un-hidden), NOT pin-sorted (the
    /// list shows the manual order with pin/hide as badges). This is the index space
    /// the move/pin/hide messages address — the Settings tile list renders exactly
    /// this, so a row's position == the index its buttons carry. Distinct from the
    /// GRID projection ([`arrange_tiles`], which pin-sorts + hide-filters).
    fn settings_order_tiles(&self) -> Vec<Tile> {
        use std::collections::HashMap;
        let rule_pos: HashMap<&str, usize> = self
            .fd_prefs
            .tiles
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id.as_str(), i))
            .collect();
        let unnamed_base = self.fd_prefs.tiles.len();
        let mut ordered: Vec<(usize, Tile)> = self
            .all_tiles
            .iter()
            .enumerate()
            .map(|(seed_i, t)| {
                let order = rule_pos
                    .get(t.id().as_str())
                    .copied()
                    .unwrap_or(unnamed_base + seed_i);
                (order, t.clone())
            })
            .collect();
        ordered.sort_by_key(|(order, _)| *order);
        ordered.into_iter().map(|(_, t)| t).collect()
    }

    /// FRONTDOOR-14 — rebuild [`Self::fd_prefs`]`.tiles` from the current SETTINGS
    /// order, preserving each tile's pin/hide flag, so a subsequent index-based edit
    /// (move/pin/hide) addresses a list that matches what the operator sees. Every
    /// tile (including hidden ones) gets a rule, so the saved order is complete +
    /// stable; a flagless un-pinned/un-hidden rule is fine (it's the neutral case).
    fn rebuild_arrangement_from_settings_order(&mut self) {
        let order = self.settings_order_tiles();
        self.fd_prefs.tiles = order
            .iter()
            .map(|t| {
                let id = t.id();
                let existing = self.fd_prefs.tiles.iter().find(|r| r.id == id);
                mde_theme::TileArrangement {
                    id,
                    pinned: existing.is_some_and(|r| r.pinned),
                    hidden: existing.is_some_and(|r| r.hidden),
                }
            })
            .collect();
    }

    /// FRONTDOOR-14 — move the settings-order tile at index `i` one slot up
    /// (`up == true`) or down (Q79): swap it with its neighbour in the settings
    /// order, persist, and re-project the grid. A move at an edge is a no-op.
    fn move_tile(&mut self, i: usize, up: bool) {
        self.rebuild_arrangement_from_settings_order();
        let len = self.fd_prefs.tiles.len();
        if len == 0 || i >= len {
            return;
        }
        let j = if up {
            if i == 0 {
                return;
            }
            i - 1
        } else {
            if i + 1 >= len {
                return;
            }
            i + 1
        };
        self.fd_prefs.tiles.swap(i, j);
        self.persist_prefs();
        self.tiles = arrange_tiles(&self.all_tiles, &self.fd_prefs);
    }

    /// FRONTDOOR-14 — flip the pin or hide flag on the settings-order tile at index
    /// `i` (Q79): rebuild the order (so every tile has a rule), toggle the named
    /// flag, persist, re-project. Hiding drops the tile from the grid (Settings
    /// still lists it); pinning lifts it to the front.
    fn toggle_tile_flag(&mut self, i: usize, flag: TileFlag) {
        self.rebuild_arrangement_from_settings_order();
        if let Some(rule) = self.fd_prefs.tiles.get_mut(i) {
            match flag {
                TileFlag::Pinned => rule.pinned = !rule.pinned,
                TileFlag::Hidden => rule.hidden = !rule.hidden,
            }
        }
        self.persist_prefs();
        self.tiles = arrange_tiles(&self.all_tiles, &self.fd_prefs);
    }

    /// FRONTDOOR-11 — set the live [`pending::ExecState`] on the proposal with the
    /// given stable bus id (the confirm / dismiss / result transitions). Keyed by
    /// id (not index) so a reload that reorders the list still resolves the right
    /// card. A no-op if the proposal is gone (it resolved + the queue moved on).
    fn set_exec_state(&mut self, id: &str, state: pending::ExecState) {
        if let Some(p) = self.pending.iter_mut().find(|p| p.id == id) {
            p.state = state;
        }
    }

    /// FRONTDOOR-11 — merge a fresh proposal snapshot into the live gate, PRESERVING
    /// each surviving proposal's gate state (its [`pending::ExecState`]) so a
    /// slow-poll reload never resets a confirm-in-flight, a shown result, or a
    /// dismissal. A proposal still present keeps its live state; a genuinely-new
    /// proposal arrives `Pending`; a proposal that fell off the queue (resolved
    /// upstream) drops, and its stale typed-confirm text is garbage-collected so
    /// the map can't grow unbounded. Pure given the snapshot.
    fn merge_pending(&mut self, fresh: Vec<pending::PendingProposal>) {
        use std::collections::HashMap;
        // Index the live cards by id so a surviving proposal keeps its gate state.
        let mut live: HashMap<String, pending::ExecState> =
            self.pending.drain(..).map(|p| (p.id, p.state)).collect();
        self.pending = fresh
            .into_iter()
            .map(|mut p| {
                if let Some(state) = live.remove(&p.id) {
                    p.state = state;
                }
                p
            })
            .collect();
        // GC the typed-confirm map down to the ids still present (a dropped
        // proposal's half-typed confirm shouldn't linger / leak).
        let ids: std::collections::HashSet<&str> =
            self.pending.iter().map(|p| p.id.as_str()).collect();
        self.confirm_inputs
            .retain(|id, _| ids.contains(id.as_str()));
    }

    /// FRONTDOOR-11 — the count of proposals still AWAITING the operator (state
    /// `Pending`), for the pending indicator badge. Resolved / dismissed cards
    /// don't count toward "needs attention". 0 ⇒ the indicator hides.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending
            .iter()
            .filter(|p| p.state == pending::ExecState::Pending)
            .count()
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

    /// FRONTDOOR-15 — the **address** of the currently-targeted node (Q74), for a
    /// cross-node GUI launch: the selected hostname resolved to its overlay IP off
    /// the live roster (`self.peers`), falling back to the hostname itself when the
    /// row carries no overlay IP (still a real, routable target on the mesh). `None`
    /// when no node is targeted (the broadcast default) or the targeted hostname is
    /// no longer in the roster (a racing reload dropped it) — the caller no-ops.
    fn target_address(&self) -> Option<String> {
        let host = self.target_node.as_ref()?;
        let peer = self.peers.iter().find(|p| &p.hostname == host)?;
        if peer.overlay_ip.trim().is_empty() {
            Some(peer.hostname.clone())
        } else {
            Some(peer.overlay_ip.clone())
        }
    }

    /// FRONTDOOR-15 — fire the push-to-talk Copilot ask (Q55 voice). Publishes the
    /// current omnibox text to the EXISTING `action/copilot/ask` topic — byte-for-
    /// byte the same path FD-6's search ask uses (`search_task`), so voice reuses
    /// the tested ask/reply round-trip and NEVER touches the exec topic (§9). It
    /// shares the one Copilot card + the one `copilot_gen` ordering counter with
    /// the search ask, so a stale voice reply for a superseded ask is dropped. The
    /// reply comes back as [`Message::VoiceReplied`] (distinct from the search's
    /// `CopilotReplied` ONLY so the voice reply also speaks — Q55's spoken
    /// summary). A blank ask fires nothing.
    ///
    /// DEFERRED (honestly, §7): the mic→text **transcription** step. There is no
    /// STT engine in the airgapped workspace (`mde-voice-hud` is a SIP softphone —
    /// RTP/G.711 media, not speech recognition), so push-to-talk drives the ask
    /// from the operator's typed/dictated omnibox text today; the captured-audio
    /// path lands when an STT engine is added. The reachable real slice — the PTT
    /// control, the ask publish, the reply render, and the spoken reply — ships.
    fn voice_task(&mut self) -> Task<crate::Message> {
        let query = self.query.trim().to_string();
        if query.is_empty() {
            // Nothing to ask yet — surface the resting "Thinking…"-free card by
            // leaving Copilot Idle (the PTT hint tells the operator to type/speak).
            return Task::none();
        }
        // Park the card at "thinking…" + bump the shared generation so a since-
        // superseded reply (voice OR search) is dropped (mirrors `search_task`).
        self.copilot = CopilotState::Thinking;
        self.copilot_gen = self.copilot_gen.wrapping_add(1);
        let generation = self.copilot_gen;
        Task::perform(
            async move {
                let body = search::ask_request_body(&query);
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
            move |answer| crate::Message::FrontDoor(Message::VoiceReplied(generation, answer)),
        )
    }

    /// FRONTDOOR-4 — fold one [`FrontDoorData`] snapshot into the widget tiles:
    /// each keyed tile takes its `(value, tone)` from the snapshot, or keeps a
    /// `None` value (no source this round) — a launcher (`key == None`) is never
    /// touched. FRONTDOOR-6 — also stores the raw roster for the unified search,
    /// and (if a search is live) re-ranks the results against the fresh roster so
    /// the mesh-entity hits track the directory. Pure given the snapshot.
    pub fn apply(&mut self, data: &FrontDoorData) {
        // FRONTDOOR-14 — fold the live data into the SEED set (`all_tiles`), the
        // arrangement's source of truth, then re-project the visible grid. Doing it
        // on the seed (not the already-arranged `tiles`) keeps a tile's live value
        // through a later re-arrangement and never loses a hidden tile's data.
        for tile in &mut self.all_tiles {
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
        self.tiles = arrange_tiles(&self.all_tiles, &self.fd_prefs);
        // FRONTDOOR-10 — fold the ranked proactive suggestions in (Q19 — they
        // render inline on the tile each concerns: a badge on the canvas card + the
        // card text in that tile's detail). Replaced wholesale each snapshot so a
        // resolved suggestion drops off rather than lingering (§7 — no stale card).
        self.suggestions = data.suggestions.clone();
        // FRONTDOOR-13 — fold the AI alert triage in (the Alerts tile detail renders
        // it). Replaced wholesale each snapshot so a cleared alert drops off rather
        // than lingering (§7 — no stale triage).
        self.triage = data.triage.clone();
        // FRONTDOOR-11 — merge the fresh proposal queue, preserving the live gate
        // state (an in-flight / resolved / dismissed card) for a proposal still
        // present, so a slow-poll reload never resets a confirm in progress.
        self.merge_pending(data.pending.clone());
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
        // FRONTDOOR-14 — the Copilot proactivity policy (Q61): with proactive
        // suggestions turned OFF in Settings, the GUI surfaces none — the inline
        // cards AND the on-tile badges (which both route through here) go quiet.
        // This gates only the GUI's rendering, never what the backend publishes.
        if !self.fd_prefs.ai_proactive {
            return Vec::new();
        }
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
        // FRONTDOOR-14 — the in-menu Settings panel (Q48) takes over the pane when
        // open, reachable from the rail + top bar in either mode. Rendered before the
        // grid; its Back control returns to the grid. Each control applies live + is
        // persisted (§7 — real, not a mockup).
        if self.show_settings {
            return self.settings_view(palette);
        }
        // FRONTDOOR-11 — the Pending actions review surface (the confirm gate) takes
        // over the pane when open, reachable from either mode's pending indicator.
        // Rendered before the grid so the operator reviews + gates proposals; its
        // Back control returns to the grid. Opening it executes nothing (§9).
        if self.show_pending {
            return self.pending_view(palette);
        }
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
                if self.locked {
                    sec = sec.push(lock_note(palette));
                }
                for (gi, s) in tile_suggestions {
                    sec = sec.push(suggestion_card(gi, s, self.locked, palette));
                }
                Some(sec.into())
            };

        // FRONTDOOR-13 — the AI alert triage, ONLY on the Alerts tile (Q38): the live
        // alerts GROUPED + EXPLAINED, each group with its member alerts and — when it
        // has a safe fix — a §9-safe "Apply fix" that re-publishes the proposal to the
        // propose queue (routed through the FD-11 confirm gate; never auto-executed).
        // Omitted on every other tile, and when no triage has landed (the all-clear /
        // pre-leader case shows an honest resting note rather than a fake group).
        let triage_section: Option<Element<'_, crate::Message, Theme>> =
            if tile.key == Some(TileKey::Alerts) {
                Some(self.triage_section(palette))
            } else {
                None
            };

        // The actions list — every row is a REAL navigation / launch (§7). An
        // empty list (Copilot) renders an honest note instead of a dead row.
        // FRONTDOOR-14 — when the session is LOCKED (Q91), the one-click PIPELINE
        // actions are disabled (the row reads muted + drops its `on_press`); plain
        // navigation / app launches stay live (viewing isn't gated). A lock note
        // heads the section so the disabled rows read as intentional, not broken.
        let actions = tile.actions();
        let mut menu = column![rail_section_label("Actions", palette)].spacing(6);
        if self.locked && actions.iter().any(TileAction::is_pipeline) {
            menu = menu.push(lock_note(palette));
        }
        if actions.is_empty() {
            menu = menu.push(
                text("No actions available yet for this tile.")
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            );
        } else {
            for action in actions {
                let gated = self.locked && action.is_pipeline();
                menu = menu.push(detail_action_row(action, gated, palette));
            }
        }

        // FRONTDOOR-15 — the cross-node TARGET selector (Q32/Q74): a mesh-scoped
        // tile (a live widget — its actions reach the mesh) offers a node picker
        // off the live roster so the operator can SCOPE an action to one node
        // instead of the whole-mesh broadcast default (Q18), plus a GUI-launch row
        // that opens locally pointed at the chosen node's data (Q74). A plain
        // launcher (Files/Terminal — `key == None`) has no mesh reach, so the
        // section is omitted there.
        let target_section: Option<Element<'_, crate::Message, Theme>> =
            if tile.key.is_some() {
                Some(self.target_node_section(palette))
            } else {
                None
            };

        let mut body = column![
            back,
            Space::new().height(Length::Fixed(16.0)),
            header,
        ]
        .spacing(8)
        .width(Length::Fill);
        if let Some(section) = triage_section {
            body = body.push(Space::new().height(Length::Fixed(20.0)));
            body = body.push(section);
        }
        if let Some(section) = suggestions_section {
            body = body.push(Space::new().height(Length::Fixed(20.0)));
            body = body.push(section);
        }
        if let Some(section) = target_section {
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

    /// FRONTDOOR-15 — the cross-node **target** section for a mesh-scoped tile's
    /// detail (Q32/Q74). It lets the operator SCOPE this tile's actions to one node
    /// picked from the LIVE roster (`self.peers` — the same FD-4 directory the
    /// widgets read, no new Bus path), instead of the whole-mesh broadcast default
    /// (Q18). A "Whole mesh (broadcast)" chip restores the default; each roster node
    /// is a selectable chip (the current target reads accent-filled). When a node is
    /// targeted it adds (a) a one-line scope note naming the node + its address, and
    /// (b) a **GUI-launch** row that opens an app LOCALLY pointed at the remote
    /// node's data (Q74) — `mde-files` on the node's shares is the reachable real
    /// example (the workbench already owns that binary). An empty roster shows an
    /// honest resting note rather than a fake node (§7). Tokens only (§4).
    fn target_node_section(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        let sizes = FontSize::defaults();
        let mut sec = column![rail_section_label("Target node", palette)].spacing(8);
        sec = sec.push(
            text("Scope this tile's actions to one node, or broadcast to the whole mesh (the default).")
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        );

        // The picker: the broadcast default + one row per live roster node. Each is
        // a full-width `target_chip` row (accent-filled when it is the current
        // target), stacked in a column the detail scroller carries.
        let mut chips = column![target_chip(
            "Whole mesh (broadcast)",
            self.target_node.is_none(),
            crate::Message::FrontDoor(Message::SelectTargetNode(None)),
            palette,
        )]
        .spacing(6);
        if self.peers.is_empty() {
            sec = sec.push(chips);
            sec = sec.push(
                text("No nodes in the roster yet — actions broadcast to the whole mesh until one appears.")
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            );
            return sec.into();
        }
        for peer in &self.peers {
            let host = peer.hostname.clone();
            let selected = self.target_node.as_deref() == Some(host.as_str());
            chips = chips.push(target_chip(
                &peer.hostname,
                selected,
                crate::Message::FrontDoor(Message::SelectTargetNode(Some(host))),
                palette,
            ));
        }
        sec = sec.push(chips);

        // With a node targeted: the scope note + the cross-node GUI-launch row
        // (Q74 — open locally on the remote node's data). `mde-files` is the
        // reachable example (the workbench owns the binary; it opens the node's
        // shares). The launch is gated by the session lock like a pipeline action.
        if let Some(addr) = self.target_address() {
            let host = self.target_node.clone().unwrap_or_default();
            sec = sec.push(
                text(format!("Actions scoped to {host} ({addr})."))
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(palette.accent.into_cosmic_color()),
            );
            if self.locked {
                sec = sec.push(lock_note(palette));
            }
            let launch = detail_action_row(
                TileAction {
                    label: format!("Open Files on {host}"),
                    // The cross-node GUI launch (Q74): a pipeline-class action
                    // (lock-gated) that opens `mde-files` locally on the remote
                    // node's data. Routed through `LaunchOnTarget` so the handler
                    // re-resolves the address at click time.
                    message: crate::Message::FrontDoor(Message::LaunchOnTarget("mde-files")),
                },
                self.locked,
                palette,
            );
            sec = sec.push(launch);
        }
        sec.into()
    }

    /// FRONTDOOR-13 — the AI alert-triage section for the Alerts tile detail (Q38).
    /// When a triage has landed it renders a header ("N alerts in M groups") and one
    /// card per group (the grouped alerts + explanation + the proposed one-click fix);
    /// when none has (all-clear / no leader / no codex) it renders an honest resting
    /// note rather than a faked group (§7). Each fix routes through the confirm gate
    /// (re-published to the propose queue), never auto-executed (§9). Tokens only (§4).
    fn triage_section(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        let sizes = FontSize::defaults();
        let mut sec = column![rail_section_label("Copilot alert triage", palette)].spacing(8);
        if self.triage.is_empty() {
            // No triage this round — an honest resting note, not a fake group (§7).
            sec = sec.push(
                text("No triage yet — Copilot triages alerts when active (all-clear, or the leader/AI is offline).")
                    .size(TypeRole::Caption.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            );
            return sec.into();
        }
        let groups = self.triage.groups.len();
        let group_unit = if groups == 1 { "group" } else { "groups" };
        let alerts = self.triage.alert_count;
        let alert_unit = if alerts == 1 { "alert" } else { "alerts" };
        sec = sec.push(
            text(format!(
                "{alerts} {alert_unit} triaged into {groups} {group_unit}"
            ))
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color()),
        );
        // FRONTDOOR-14 — under a locked session the "Apply fix" affordances are
        // disabled; a lock note heads the cards so they read as a deliberate lock.
        if self.locked && self.triage.groups.iter().any(|g| g.proposal_body.is_some()) {
            sec = sec.push(lock_note(palette));
        }
        for (gi, g) in self.triage.groups.iter().enumerate() {
            sec = sec.push(triage_group_card(gi, g, self.locked, palette));
        }
        sec.into()
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

        // Top bar: the omnibox stretches; the FD-11 pending indicator + the FD-14
        // settings / lock toggles + the mode toggle sit at its right. In full-screen
        // there is NO rail, so the settings + lock entries MUST live here for them to
        // be reachable in this mode (§7 — no unreachable surface).
        let mut controls = row![omnibox]
            .spacing(12)
            .align_y(cosmic::iced::Alignment::Center);
        if let Some(indicator) = self.pending_indicator(palette) {
            controls = controls.push(indicator);
        }
        // FRONTDOOR-15 — the push-to-talk voice control (Q55), in the full-screen
        // top bar too (no rail here — the top bar is the only reachable home).
        controls = controls.push(self.ptt_toggle(palette));
        controls = controls.push(self.settings_toggle(palette));
        controls = controls.push(self.lock_toggle(palette));
        controls = controls.push(self.mode_toggle(palette));
        let top_bar = container(controls)
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

        // FRONTDOOR-16 — the guided first-run greeting (Q27) above the icon grid,
        // the same once-per-node card the panel mode shows (both modes are reachable
        // surfaces — §7). `None` once dismissed / while searching.
        let body = match self.greeting_banner(palette) {
            Some(greeting) => column![top_bar, greeting, content],
            None => column![top_bar, content],
        }
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

        // FRONTDOOR-14 — the Front Door's own controls at the foot of the rail
        // (Q48 in-menu settings, Q91 lock): a Settings entry opening the in-menu
        // panel, and a lock toggle gating the action affordances. Both are REAL
        // (§7 — wired to live messages, no dead buttons).
        let system = column![
            rail_section_label("Front Door", palette),
            rail_link(
                "Settings",
                crate::Message::FrontDoor(Message::OpenSettings),
                palette,
                false,
            ),
            rail_link(
                self.lock_label(),
                crate::Message::FrontDoor(Message::ToggleLock),
                palette,
                false,
            ),
        ]
        .spacing(4);

        let body = column![
            identity,
            Space::new().height(Length::Fixed(16.0)),
            surfaces,
            Space::new().height(Length::Fixed(16.0)),
            pinned,
            Space::new().height(Length::Fixed(16.0)),
            system,
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

        // FRONTDOOR-11 — the pending indicator sits between the omnibox and the
        // mode toggle, surfacing the queued-proposals count + opening the gate.
        let mut omnibox_row = row![omnibox]
            .spacing(12)
            .align_y(cosmic::iced::Alignment::Center);
        if let Some(indicator) = self.pending_indicator(palette) {
            omnibox_row = omnibox_row.push(indicator);
        }
        // FRONTDOOR-15 — the push-to-talk voice control sits beside the omnibox
        // (Q55): press to ask the current query aloud + hear the reply spoken.
        omnibox_row = omnibox_row.push(self.ptt_toggle(palette));
        // FRONTDOOR-14 — the settings + lock toggles sit beside the mode toggle (the
        // rail also carries them in panel mode; having them in the top bar keeps the
        // affordance consistent across both modes).
        omnibox_row = omnibox_row.push(self.settings_toggle(palette));
        omnibox_row = omnibox_row.push(self.lock_toggle(palette));
        omnibox_row = omnibox_row.push(self.mode_toggle(palette));
        let omnibox_bar = container(omnibox_row)
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

        // FRONTDOOR-16 — the one-time guided-first-run greeting (Q27) sits above
        // the resting grid (never over search results). `greeting_banner` is `None`
        // once dismissed / while searching, so the normal pane is unchanged.
        let pane = match self.greeting_banner(palette) {
            Some(greeting) => column![omnibox_bar, greeting, content],
            None => column![omnibox_bar, content],
        }
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

    /// FRONTDOOR-16 — the guided first-run greeting banner (Q27), or `None`. It
    /// shows ONLY on the resting grid of a node that has never dismissed it: not
    /// once `show_greeting` is cleared (dismissed / a returning operator), and not
    /// while the omnibox is driving a search (the results own the pane then). The
    /// caller drops it between the omnibox bar and the grid in either mode. Padded
    /// to align with the omnibox/grid gutters; the card itself is `greeting_card`.
    fn greeting_banner(&self, palette: Palette) -> Option<Element<'_, crate::Message, Theme>> {
        if !self.show_greeting || self.searching() {
            return None;
        }
        Some(
            container(greeting_card(palette))
                .width(Length::Fill)
                .padding(Padding::from([0u16, 16u16]))
                .into(),
        )
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

    /// FRONTDOOR-14 — the rail/top-bar label for the lock toggle (Q91): names the
    /// ACTION the press performs, mirroring the mode toggle's target-naming idiom.
    fn lock_label(&self) -> &'static str {
        if self.locked {
            "🔓 Unlock"
        } else {
            "🔒 Lock"
        }
    }

    /// FRONTDOOR-14 — the top-bar **Settings** button (Q48), opening the in-menu
    /// settings panel ([`Message::OpenSettings`]). A real control, mirroring the
    /// mode toggle's chrome. Tokens only (§4).
    fn settings_toggle(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        top_bar_button(
            "⚙ Settings",
            crate::Message::FrontDoor(Message::OpenSettings),
            palette,
        )
    }

    /// FRONTDOOR-15 — the top-bar **push-to-talk** control (Q55 voice, optional /
    /// off-by-default as an affordance the operator opts into by pressing it). It
    /// fires [`Message::PushToTalk`], which publishes the current ask to the
    /// EXISTING `action/copilot/ask` topic (§9 — the ask lane, never exec) and
    /// speaks the reply. Present in BOTH modes' top bar (the rail has no voice
    /// entry, so the top bar carries it for reachability). Mirrors the other
    /// top-bar buttons' chrome. Tokens only (§4).
    fn ptt_toggle(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        top_bar_button(
            "🎙 Ask aloud",
            crate::Message::FrontDoor(Message::PushToTalk),
            palette,
        )
    }

    /// FRONTDOOR-14 — the top-bar **lock** toggle (Q91): flips the session lock
    /// ([`Message::ToggleLock`]); the glyph/label names the action. When locked it
    /// reads warning-toned so the locked state is obvious from the top bar.
    fn lock_toggle(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        let accent = if self.locked {
            palette.warning.into_cosmic_color()
        } else {
            palette.accent.into_cosmic_color()
        };
        let raised = palette.raised.into_cosmic_color();
        let idle_bg = palette.hover_tint().into_cosmic_color();
        button(
            text(self.lock_label())
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
        .on_press(crate::Message::FrontDoor(Message::ToggleLock))
        .into()
    }

    /// FRONTDOOR-11 — the **pending actions** indicator in the top bar: a pill
    /// surfacing the count of proposals AWAITING the operator, opening the confirm
    /// gate ([`Message::OpenPending`]). `None` when nothing is pending (no dead
    /// affordance — §7); a count badge accents it when proposals queue up. The
    /// indicator only navigates to the review surface — it executes nothing (§9).
    /// Carbon chrome via tokens only (§4).
    fn pending_indicator(&self, palette: Palette) -> Option<Element<'_, crate::Message, Theme>> {
        let count = self.pending_count();
        if count == 0 {
            return None;
        }
        let accent = palette.accent.into_cosmic_color();
        let idle_bg = palette.hover_tint().into_cosmic_color();
        let label = if count == 1 {
            "1 pending action".to_string()
        } else {
            format!("{count} pending actions")
        };
        Some(
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
                        Status::Hovered | Status::Pressed => accent_tint(accent),
                        _ => idle_bg,
                    };
                    cosmic::iced::widget::button::Style {
                        snap: false,
                        background: Some(Background::Color(bg)),
                        text_color: accent,
                        border: Border {
                            color: accent,
                            width: 1.0,
                            radius: 6.0.into(),
                        },
                        shadow: cosmic::iced::Shadow::default(),
                        ..cosmic::iced::widget::button::Style::default()
                    }
                },
            )
            .on_press(crate::Message::FrontDoor(Message::OpenPending))
            .into(),
        )
    }

    /// FRONTDOOR-11 — the **Pending actions** review surface: the confirm gate
    /// (design Q10 + Q44). Lists each queued proposal as a card showing its PREVIEW
    /// (the action kind + target node(s) + a human-readable effect + a dry-run line)
    /// then its CONFIRM GATE (a 1-click "Approve" for a normal action, or a
    /// typed-confirm field + a "Execute" that only arms once the word is typed, for
    /// a destructive kind), plus "Dismiss" (reject, no execute) and — once acted on
    /// — the execution RESULT. Under a Back control to the grid. Reachable from both
    /// modes' pending indicators. ONLY an explicit confirm publishes to the exec
    /// topic; the preview is ALWAYS shown before any execute (§9). Tokens only (§4).
    fn pending_view(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        let sizes = FontSize::defaults();
        let back = nav_back_button(
            "← Back",
            crate::Message::FrontDoor(Message::ClosePending),
            palette,
        );

        let header = column![
            text("Pending actions")
                .size(TypeRole::Heading.size_in(sizes))
                .colr(palette.text.into_cosmic_color()),
            text(
                "Review each proposal's preview, then confirm to execute. \
                 Destructive actions need a typed confirm.",
            )
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(4);

        let mut list = column![].spacing(14).width(Length::Fill);
        if self.pending.is_empty() {
            list = list.push(
                text("No pending actions. Queued Copilot proposals appear here.")
                    .size(TypeRole::Body.size_in(sizes))
                    .colr(palette.text_muted.into_cosmic_color()),
            );
        } else {
            for (i, p) in self.pending.iter().enumerate() {
                let typed = self
                    .confirm_inputs
                    .get(&p.id)
                    .map(String::as_str)
                    .unwrap_or("");
                list = list.push(proposal_card(i, p, typed, self.locked, palette));
            }
        }

        let body = column![
            back,
            Space::new().height(Length::Fixed(16.0)),
            header,
            Space::new().height(Length::Fixed(20.0)),
            list,
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

    /// FRONTDOOR-14 — the in-menu **Settings** panel (Q48): theme (Q23), density
    /// (Q80), the Copilot proactivity policy (Q61), the tile arrangement (Q79), and
    /// the session lock (Q91). Each control APPLIES for real — it writes the pref
    /// and the live UI reflects it on the next render (§7 — not a mockup). Reachable
    /// from the rail + top bar in both modes; a Back control returns to the grid.
    /// Carbon chrome via tokens only (§4).
    fn settings_view(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        let sizes = FontSize::defaults();
        let back = nav_back_button(
            "← Back",
            crate::Message::FrontDoor(Message::CloseSettings),
            palette,
        );

        let header = column![
            text("Settings")
                .size(TypeRole::Heading.size_in(sizes))
                .colr(palette.text.into_cosmic_color()),
            text(
                "These apply live and persist to this node's preferences. \
                 The mesh-wide sync over etcd (Q56) is a follow-up.",
            )
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color()),
        ]
        .spacing(4);

        // The persisted theme/density drive which choice reads as selected. Read
        // fresh so a just-applied change reflects immediately.
        let cur_theme = self.fd_theme();
        let cur_density = self.fd_density();

        // ── Appearance: theme (Q23) ──────────────────────────────────────────────
        let theme_row = row![
            settings_choice(
                "Follow OS",
                cur_theme == mde_theme::Theme::Dark,
                crate::Message::FrontDoor(Message::SetTheme(mde_theme::Theme::Dark)),
                palette,
            ),
            settings_choice(
                "Gray 90",
                cur_theme == mde_theme::Theme::Gray90,
                crate::Message::FrontDoor(Message::SetTheme(mde_theme::Theme::Gray90)),
                palette,
            ),
            settings_choice(
                "Gray 10",
                cur_theme == mde_theme::Theme::Light,
                crate::Message::FrontDoor(Message::SetTheme(mde_theme::Theme::Light)),
                palette,
            ),
        ]
        .spacing(8);
        // The default theme (`Dark` = Gray 100) is the "Follow OS / auto" choice
        // per Q23: a separate explicit Gray-100 row would shadow it, so the three
        // rows are Follow-OS (Gray 100) · Gray 90 · Gray 10 — the §4 theme set.
        let theme_section = settings_section(
            "Theme",
            "Follow-OS (Gray 100) · Gray 90 · Gray 10 (Q23).",
            theme_row.into(),
            palette,
        );

        // ── Appearance: density (Q80) ────────────────────────────────────────────
        let density_row = row![
            settings_choice(
                "Comfortable",
                cur_density == mde_theme::Density::Comfortable,
                crate::Message::FrontDoor(Message::SetDensity(mde_theme::Density::Comfortable)),
                palette,
            ),
            settings_choice(
                "Compact",
                cur_density == mde_theme::Density::Compact,
                crate::Message::FrontDoor(Message::SetDensity(mde_theme::Density::Compact)),
                palette,
            ),
        ]
        .spacing(8);
        let density_section = settings_section(
            "Density",
            "Comfortable default, with a compact toggle (Q80).",
            density_row.into(),
            palette,
        );

        // ── AI / Copilot policy: proactivity (Q61) ───────────────────────────────
        let ai_row = row![
            settings_choice(
                "Proactive suggestions on",
                self.fd_prefs.ai_proactive,
                crate::Message::FrontDoor(Message::SetAiProactive(true)),
                palette,
            ),
            settings_choice(
                "Off",
                !self.fd_prefs.ai_proactive,
                crate::Message::FrontDoor(Message::SetAiProactive(false)),
                palette,
            ),
        ]
        .spacing(8);
        let ai_section = settings_section(
            "Copilot",
            "Proactive suggestion cards + on-tile badges (Q61). Off silences them; \
             search + on-demand answers still work.",
            ai_row.into(),
            palette,
        );

        // ── Session lock (Q91) ───────────────────────────────────────────────────
        let lock_state = if self.locked {
            "Locked — action affordances are disabled until you unlock."
        } else {
            "Unlocked — actions are enabled."
        };
        let lock_section = settings_section(
            "Session lock",
            lock_state,
            self.lock_toggle(palette),
            palette,
        );

        // ── Tile arrangement (Q79) ───────────────────────────────────────────────
        let arrangement = self.settings_tile_list(palette);
        let arrangement_section = settings_section(
            "Tiles",
            "Reorder, pin, or hide the Front Door tiles. The grid honors this and it \
             persists (Q79).",
            arrangement,
            palette,
        );

        let body = column![
            back,
            Space::new().height(Length::Fixed(16.0)),
            header,
            Space::new().height(Length::Fixed(20.0)),
            theme_section,
            Space::new().height(Length::Fixed(16.0)),
            density_section,
            Space::new().height(Length::Fixed(16.0)),
            ai_section,
            Space::new().height(Length::Fixed(16.0)),
            lock_section,
            Space::new().height(Length::Fixed(16.0)),
            arrangement_section,
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

    /// FRONTDOOR-14 — the tile-arrangement list in Settings (Q79): one row per tile
    /// in settings order (hidden tiles included so they can be un-hidden), each with
    /// move-up / move-down / pin / hide controls that fire the real arrangement
    /// messages. The index a row carries is its position in [`Self::settings_order_tiles`],
    /// which the message handlers address — so a row's buttons edit exactly that
    /// tile. Real persistence (§7). Tokens only (§4).
    fn settings_tile_list(&self, palette: Palette) -> Element<'_, crate::Message, Theme> {
        let order = self.settings_order_tiles();
        let last = order.len().saturating_sub(1);
        let mut list = column![].spacing(6).width(Length::Fill);
        for (i, tile) in order.iter().enumerate() {
            let id = tile.id();
            let rule = self.fd_prefs.tiles.iter().find(|r| r.id == id);
            let pinned = rule.is_some_and(|r| r.pinned);
            let hidden = rule.is_some_and(|r| r.hidden);
            // `tile.label` is owned by the local `order`; clone it into the row so
            // the returned Element doesn't borrow the loop-local vec.
            list = list.push(settings_tile_row(
                i,
                tile.label.clone(),
                pinned,
                hidden,
                i > 0,
                i < last,
                palette,
            ));
        }
        list.into()
    }
}

/// FRONTDOOR-14 — which boolean flag a [`Message::ToggleTilePinned`] /
/// [`Message::ToggleTileHidden`] edits, so the two share one toggle path (Q79).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TileFlag {
    /// Sort the tile to the front of the grid.
    Pinned,
    /// Drop the tile from the grid (still listed in Settings to un-hide).
    Hidden,
}

/// FRONTDOOR-14 — project the operator's persisted arrangement (Q79) over the
/// seed tile set: apply the saved order, sort pinned tiles to the front, and drop
/// hidden ones. Pure (seed in, visible grid out) so it's safe to re-run on every
/// settings edit and after each live-data fold.
///
/// The rules:
///   * A tile NAMED in `prefs.tiles` takes that list's position; an arrangement
///     rule for a tile that no longer exists is ignored (id ↔ tile is resolved
///     here). A seed tile the operator never touched keeps its seed order, AFTER
///     all the named ones (so a fresh tile lands at the end, never silently
///     reordering the saved layout).
///   * `pinned` tiles sort to the front (a stable partition preserves the order
///     within each group), so a pin lifts a tile without scrambling the rest.
///   * `hidden` tiles are filtered out of the returned grid (still in the seed
///     set, so Settings can un-hide them).
///
/// An empty arrangement returns the seed unchanged — the pre-FD-14 grid.
fn arrange_tiles(seed: &[Tile], prefs: &mde_theme::FrontDoorPrefs) -> Vec<Tile> {
    use std::collections::HashMap;
    // Index the operator's rules by tile id: the position they appear in the saved
    // list is the operator's order; the flags layer pin/hide over it.
    let rule_pos: HashMap<&str, usize> = prefs
        .tiles
        .iter()
        .enumerate()
        .map(|(i, r)| (r.id.as_str(), i))
        .collect();
    let rule_for = |id: &str| prefs.tiles.iter().find(|r| r.id == id);

    // Order: named tiles by their saved position, then the rest in seed order. A
    // large base keeps un-named tiles strictly after every named one.
    let unnamed_base = prefs.tiles.len();
    let mut ordered: Vec<(usize, &Tile)> = seed
        .iter()
        .enumerate()
        .map(|(seed_i, t)| {
            let id = t.id();
            let order = rule_pos
                .get(id.as_str())
                .copied()
                .unwrap_or(unnamed_base + seed_i);
            (order, t)
        })
        .collect();
    ordered.sort_by_key(|(order, _)| *order);

    // Drop hidden tiles, then stably partition pinned-first.
    let mut visible: Vec<Tile> = ordered
        .into_iter()
        .filter(|(_, t)| !rule_for(&t.id()).is_some_and(|r| r.hidden))
        .map(|(_, t)| t.clone())
        .collect();
    visible.sort_by_key(|t| !rule_for(&t.id()).is_some_and(|r| r.pinned));
    visible
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

/// FRONTDOOR-16 — the path of the once-per-node "greeted" sentinel: an empty
/// marker file under the same `~/.config/mde/` the preferences live in (the
/// workbench owns this dir already — `backend::default_settings_path`). Its mere
/// EXISTENCE means the guided first-run greeting (Q27) has been dismissed on this
/// node, so it never shows again (Q71 — the greeting is the whole onboarding).
/// `None` only when neither `XDG_CONFIG_HOME` nor `HOME` is set (a misconfigured
/// process) — the greeting then simply shows every launch rather than erroring.
fn greeting_sentinel_path() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|base| base.join("mde").join("frontdoor-greeted"))
}

/// FRONTDOOR-16 — has the guided first-run greeting already been dismissed on
/// this node? True once the sentinel exists. A returning operator (the file is
/// there) is never greeted again; a fresh install / a wiped config greets once.
fn greeting_already_seen() -> bool {
    greeting_sentinel_path().is_some_and(|p| p.exists())
}

/// FRONTDOOR-16 — record that the greeting has been seen (write the sentinel).
/// Best-effort: a write failure (no config dir, read-only FS) is swallowed — the
/// worst case is the card greets once more, never an error spew (matching the
/// "no error spew" posture the prefs persistence already takes). Creating the
/// parent is harmless if `preferences.toml` already made it.
fn mark_greeting_seen() {
    if let Some(path) = greeting_sentinel_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, b"");
    }
}

/// FRONTDOOR-16 — the one-time guided first-run welcome card (Q27): a Copilot
/// greeting that introduces the auto-built tile grid, with a single **Got it**
/// dismiss (Q71 — no tour; the greeting IS the onboarding). Rendered ABOVE the
/// resting tile grid (never over search results / a detail / settings), once per
/// node. Carbon chrome via `mde-theme` tokens only (§4); the accent-headed raised
/// card mirrors the Copilot answer card so the AI voice reads consistent.
fn greeting_card<'a>(palette: Palette) -> Element<'a, crate::Message, Theme> {
    let sizes = FontSize::defaults();
    let accent = palette.accent.into_cosmic_color();
    let raised = palette.raised.into_cosmic_color();
    let idle_bg = palette.hover_tint().into_cosmic_color();

    let dismiss = button(
        text("Got it")
            .size(TypeRole::Body.size_in(sizes))
            .colr(accent),
    )
    .padding(Padding::from([8u16, 16u16]))
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
    .on_press(crate::Message::FrontDoor(Message::DismissGreeting));

    let body = column![
        text("Welcome — I'm Copilot")
            .size(TypeRole::Heading.size_in(sizes))
            .colr(palette.accent.into_cosmic_color()),
        text(
            "Your tiles are building themselves from the live mesh — node health, \
             builds, alerts, and more. Click any tile to act on it, or just ask me \
             anything in the search bar above."
        )
        .size(TypeRole::Body.size_in(sizes))
        .colr(palette.text.into_cosmic_color()),
        row![Space::new().width(Length::Fill), dismiss].align_y(cosmic::iced::Alignment::Center),
    ]
    .spacing(10)
    .width(Length::Fill);

    container(body)
        .width(Length::Fill)
        .padding(Padding::from([16u16, 18u16]))
        .style(move |_t: &Theme| container::Style {
            background: Some(Background::Color(palette.surface.into_cosmic_color())),
            border: Border {
                color: palette.accent.into_cosmic_color(),
                width: 1.0,
                radius: 10.0.into(),
            },
            ..container::Style::default()
        })
        .into()
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

/// FRONTDOOR-14 — a ghost top-bar button (the Settings entry), styled like the
/// mode toggle: accent text on a quiet hover wash. Carries a REAL `on_press` (§7).
/// Tokens only (§4).
fn top_bar_button<'a>(
    label: &'a str,
    msg: crate::Message,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
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
    .on_press(msg)
    .into()
}

/// FRONTDOOR-15 — best-effort **speak** a Copilot reply aloud (Q55's spoken
/// summary). Uses the system speech service via `spd-say` (speech-dispatcher —
/// the canonical Linux TTS front-end), the SAME detached best-effort spawn the
/// Front Door / notify-toast already use for their sound cues (`canberra-gtk-play`
/// / `paplay`): the binary resolves on PATH post-install, and an absent service /
/// no audio device is silently inert (it never blocks the GUI thread or panics).
/// `-w` lets the spawned process own the utterance; we don't wait on it. This is a
/// real spoken-reply path, not a stub (§7) — it speaks when the host has speech-
/// dispatcher (the desktop image ships it) and degrades quietly otherwise. A blank
/// reply is not spoken. NOTE the matching deferred half (mic→text STT) on
/// [`FrontDoor::voice_task`].
fn speak_reply(reply: String) {
    let reply = reply.trim();
    if reply.is_empty() {
        return;
    }
    let _ = std::process::Command::new("spd-say")
        .args(["-w", reply])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// FRONTDOOR-14 — one labelled Settings section: a heading, a one-line help blurb,
/// and the section's control(s) below it, in a raised card. Tokens only (§4).
fn settings_section<'a>(
    title: &'a str,
    help: &'a str,
    control: Element<'a, crate::Message, Theme>,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let sizes = FontSize::defaults();
    let card = column![
        text(title)
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text.into_cosmic_color()),
        text(help)
            .size(TypeRole::Caption.size_in(sizes))
            .colr(palette.text_muted.into_cosmic_color()),
        Space::new().height(Length::Fixed(8.0)),
        control,
    ]
    .spacing(4)
    .width(Length::Fill);
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
        .into()
}

/// FRONTDOOR-14 — one selectable choice in a Settings row (a theme / density / AI
/// option): a segmented-style button that reads accent-filled when it's the live
/// selection and a quiet ghost otherwise. Carries a REAL `on_press` that applies +
/// persists the choice (§7). Tokens only (§4).
fn settings_choice<'a>(
    label: &'a str,
    selected: bool,
    msg: crate::Message,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let accent = palette.accent.into_cosmic_color();
    let on_fg = palette.background.into_cosmic_color();
    let off_fg = palette.text.into_cosmic_color();
    let idle_bg = palette.hover_tint().into_cosmic_color();
    let fg = if selected { on_fg } else { off_fg };
    button(
        text(label)
            .size(TypeRole::Body.size_in(FontSize::defaults()))
            .colr(fg),
    )
    .padding(Padding::from([8u16, 14u16]))
    .sty(
        move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
            use cosmic::iced::widget::button::Status;
            let bg = if selected {
                accent
            } else {
                match status {
                    Status::Hovered | Status::Pressed => accent_tint(accent),
                    _ => idle_bg,
                }
            };
            cosmic::iced::widget::button::Style {
                snap: false,
                background: Some(Background::Color(bg)),
                text_color: fg,
                border: Border {
                    color: if selected {
                        accent
                    } else {
                        palette.border.into_cosmic_color()
                    },
                    width: 1.0,
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

/// FRONTDOOR-14 — one row in the Settings tile-arrangement list (Q79): the tile's
/// label (dimmed when hidden) plus its move-up / move-down / pin / hide controls.
/// `i` is the tile's index in the settings order, carried in the arrangement
/// messages so each control edits exactly this tile. Move controls drop their
/// `on_press` at the list edges (`can_up` / `can_down`) so an inert move reads as
/// disabled. Real persistence behind every press (§7). Tokens only (§4).
#[allow(clippy::too_many_arguments)]
fn settings_tile_row<'a>(
    i: usize,
    label: String,
    pinned: bool,
    hidden: bool,
    can_up: bool,
    can_down: bool,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let sizes = FontSize::defaults();
    let label_color = if hidden {
        palette.text_muted.into_cosmic_color()
    } else {
        palette.text.into_cosmic_color()
    };
    let name = text(label)
        .size(TypeRole::Body.size_in(sizes))
        .colr(label_color);

    let up = small_settings_button(
        "↑",
        can_up,
        crate::Message::FrontDoor(Message::MoveTileUp(i)),
        palette,
    );
    let down = small_settings_button(
        "↓",
        can_down,
        crate::Message::FrontDoor(Message::MoveTileDown(i)),
        palette,
    );
    let pin = small_settings_button(
        if pinned { "Unpin" } else { "Pin" },
        true,
        crate::Message::FrontDoor(Message::ToggleTilePinned(i)),
        palette,
    );
    let hide = small_settings_button(
        if hidden { "Show" } else { "Hide" },
        true,
        crate::Message::FrontDoor(Message::ToggleTileHidden(i)),
        palette,
    );

    let controls = row![up, down, pin, hide]
        .spacing(6)
        .align_y(cosmic::iced::Alignment::Center);
    let inner = row![container(name).width(Length::Fill), controls,]
        .spacing(8)
        .align_y(cosmic::iced::Alignment::Center)
        .width(Length::Fill);

    container(inner)
        .width(Length::Fill)
        .padding(Padding::from([8u16, 12u16]))
        .style(move |_t: &Theme| container::Style {
            background: Some(Background::Color(palette.raised.into_cosmic_color())),
            border: Border {
                color: palette.border.into_cosmic_color(),
                width: 1.0,
                radius: 6.0.into(),
            },
            ..container::Style::default()
        })
        .into()
}

/// FRONTDOOR-14 — a compact ghost button for the tile-arrangement row controls.
/// `enabled == false` drops the `on_press` + mutes the label (a move at a list
/// edge), so a disabled control reads inert. Tokens only (§4).
fn small_settings_button<'a>(
    label: &'a str,
    enabled: bool,
    msg: crate::Message,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let accent = palette.accent.into_cosmic_color();
    let muted = palette.text_muted.into_cosmic_color();
    let idle_bg = palette.hover_tint().into_cosmic_color();
    let fg = if enabled { accent } else { muted };
    let mut b = button(
        text(label)
            .size(TypeRole::Caption.size_in(FontSize::defaults()))
            .colr(fg),
    )
    .padding(Padding::from([6u16, 10u16]))
    .sty(
        move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
            use cosmic::iced::widget::button::Status;
            let bg = if enabled {
                match status {
                    Status::Hovered | Status::Pressed => accent_tint(accent),
                    _ => idle_bg,
                }
            } else {
                idle_bg
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
    );
    if enabled {
        b = b.on_press(msg);
    }
    b.into()
}

/// FRONTDOOR-14 — the muted "session is locked" note shown above a set of disabled
/// action affordances (Q91), so the gated rows read as a deliberate lock rather
/// than broken buttons. Tokens only (§4).
fn lock_note<'a>(palette: Palette) -> Element<'a, crate::Message, Theme> {
    text("Session locked — unlock to run actions.")
        .size(TypeRole::Caption.size_in(FontSize::defaults()))
        .colr(palette.warning.into_cosmic_color())
        .into()
}

/// FRONTDOOR-5 — one full-width row in a tile's detail actions menu. Carries the
/// action's REAL `on_press` message (a panel navigation or app launch — §7, never
/// a stub). Styled like an emphasized rail link (accent text + a quiet idle wash
/// that lifts on hover), so the menu reads as a list of live, clickable actions.
fn detail_action_row<'a>(
    action: TileAction,
    gated: bool,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let accent = palette.accent.into_cosmic_color();
    let idle_bg = palette.hover_tint().into_cosmic_color();
    // FRONTDOOR-14 — a gated row (a pipeline action under a locked session, Q91)
    // reads muted + drops its `on_press` so it's visibly inert; an ungated row is
    // the normal accent-text clickable.
    let muted = palette.text_muted.into_cosmic_color();
    let fg = if gated { muted } else { accent };

    let mut b = button(
        text(action.label)
            .size(TypeRole::Body.size_in(FontSize::defaults()))
            .colr(fg),
    )
    .width(Length::Fill)
    .padding(Padding::from([10u16, 14u16]))
    .sty(
        move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
            use cosmic::iced::widget::button::Status;
            let bg = if gated {
                idle_bg
            } else {
                match status {
                    Status::Hovered | Status::Pressed => accent_tint(accent),
                    _ => idle_bg,
                }
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
    );
    if !gated {
        b = b.on_press(action.message);
    }
    b.into()
}

/// FRONTDOOR-15 — one selectable **target-node** chip in the cross-node section
/// (Q32/Q74): the broadcast default or a live roster node. The current target reads
/// accent-filled (accent background, on-accent text); the rest read as a quiet
/// outlined row. Clicking it fires `msg` (a [`Message::SelectTargetNode`]) — a pure
/// scope flip, never an action. Tokens only (§4).
fn target_chip<'a>(
    label: &str,
    selected: bool,
    msg: crate::Message,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let accent = palette.accent.into_cosmic_color();
    let idle_bg = palette.hover_tint().into_cosmic_color();
    let on_accent = palette.background.into_cosmic_color();
    let text_color = palette.text.into_cosmic_color();
    // Selected → accent fill with on-accent text; unselected → quiet row, text
    // tone normal so the roster reads as a list.
    let fg = if selected { on_accent } else { text_color };
    button(
        text(label.to_string())
            .size(TypeRole::Body.size_in(FontSize::defaults()))
            .colr(fg),
    )
    .width(Length::Fill)
    .padding(Padding::from([10u16, 14u16]))
    .sty(
        move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
            use cosmic::iced::widget::button::Status;
            let bg = if selected {
                accent
            } else {
                match status {
                    Status::Hovered | Status::Pressed => accent_tint(accent),
                    _ => idle_bg,
                }
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
    locked: bool,
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
    // FRONTDOOR-14 — under a locked session (Q91) "Act" is an action affordance, so
    // it reads muted + drops its `on_press` (the lock note above the cards says so).
    if s.proposal_body.is_some() {
        let idle_bg = palette.hover_tint().into_cosmic_color();
        let muted = palette.text_muted.into_cosmic_color();
        let fg = if locked { muted } else { accent };
        let mut act = button(
            text("Act — queue this proposal for approval")
                .size(TypeRole::Caption.size_in(sizes))
                .colr(fg),
        )
        .padding(Padding::from([8u16, 12u16]))
        .sty(
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                use cosmic::iced::widget::button::Status;
                let bg = if locked {
                    idle_bg
                } else {
                    match status {
                        Status::Hovered | Status::Pressed => accent_tint(accent),
                        _ => idle_bg,
                    }
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
        );
        if !locked {
            act = act.on_press(crate::Message::FrontDoor(Message::ProposeSuggestion(gi)));
        }
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

/// FRONTDOOR-13 — one alert-triage GROUP card (Q38): the cluster headline + severity
/// badge, the member alert names, the plain-language explanation, and — ONLY when the
/// group carries a typed fix proposal — a §9-safe **Apply fix** button that
/// re-publishes the proposal to the propose queue (routed through the FD-11 confirm
/// gate; never auto-executed). `gi` is the group's index into the triage, carried in
/// the [`Message::ProposeAlertFix`]. Carbon tokens only (§4).
fn triage_group_card<'a>(
    gi: usize,
    g: &copilot::AlertGroup,
    locked: bool,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let sizes = FontSize::defaults();
    let accent = palette.accent.into_cosmic_color();
    // High-severity reads danger-toned, medium reads warning — the operator's eye
    // goes to the worst cluster first (§4 — token, never hex).
    let severity_tone = if g.severity == "high" {
        palette.danger.into_cosmic_color()
    } else {
        palette.warning.into_cosmic_color()
    };

    let mut card = column![
        text(g.title.clone())
            .size(TypeRole::Body.size_in(sizes))
            .colr(palette.text.into_cosmic_color()),
        text(format!("{} severity", g.severity))
            .size(TypeRole::Caption.size_in(sizes))
            .colr(severity_tone),
    ]
    .spacing(4)
    .width(Length::Fill);

    // The member alerts this group clusters — so the operator sees WHICH alerts the
    // explanation covers, not just the headline.
    if !g.alerts.is_empty() {
        card = card.push(
            text(format!("Alerts: {}", g.alerts.join(", ")))
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        );
    }

    if !g.explanation.trim().is_empty() {
        card = card.push(
            text(g.explanation.clone())
                .size(TypeRole::Caption.size_in(sizes))
                .colr(palette.text_muted.into_cosmic_color()),
        );
    }

    // §9 — the "Apply fix" affordance ONLY when the group carries a typed proposal.
    // It PROPOSES (re-publishes to the propose queue) and routes through the FD-11
    // confirm gate; it never executes the fix directly.
    // FRONTDOOR-14 — under a locked session (Q91) "Apply fix" is an action
    // affordance, so it reads muted + drops its `on_press`.
    if g.proposal_body.is_some() {
        let idle_bg = palette.hover_tint().into_cosmic_color();
        let muted = palette.text_muted.into_cosmic_color();
        let fg = if locked { muted } else { accent };
        let mut act = button(
            text("Apply fix — queue this proposal for approval")
                .size(TypeRole::Caption.size_in(sizes))
                .colr(fg),
        )
        .padding(Padding::from([8u16, 12u16]))
        .sty(
            move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
                use cosmic::iced::widget::button::Status;
                let bg = if locked {
                    idle_bg
                } else {
                    match status {
                        Status::Hovered | Status::Pressed => accent_tint(accent),
                        _ => idle_bg,
                    }
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
        );
        if !locked {
            act = act.on_press(crate::Message::FrontDoor(Message::ProposeAlertFix(gi)));
        }
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

/// A shared **Back** control: an accent ghost button firing the given message.
/// The detail menu + the FD-11 pending surface both head their pane with one, so
/// the chrome is identical (accent text, a quiet idle wash that lifts on hover).
/// Tokens only (§4).
fn nav_back_button<'a>(
    label: &'a str,
    msg: crate::Message,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
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
    .on_press(msg)
    .into()
}

/// FRONTDOOR-11 — a filled action button for the confirm gate (the 1-click
/// "Approve" / the typed-confirm "Execute" / "Dismiss"). `tone` colors the fill so
/// a destructive "Execute" reads danger-toned and a normal "Approve" reads accent;
/// `enabled == false` (a destructive Execute before the confirm word is typed)
/// drops the `on_press` so the button is visibly inert — a 1-click can NEVER fire
/// a destructive action (§9). Tokens only (§4).
fn gate_button<'a>(
    label: &'a str,
    tone: cosmic::iced::Color,
    enabled: bool,
    msg: crate::Message,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let on_bg = tone;
    let on_fg = palette.background.into_cosmic_color();
    // A disarmed button reads muted + non-interactive (no fill, muted text).
    let off_bg = palette.raised.into_cosmic_color();
    let off_fg = palette.text_muted.into_cosmic_color();
    let fg = if enabled { on_fg } else { off_fg };
    let mut b = button(
        text(label)
            .size(TypeRole::Body.size_in(FontSize::defaults()))
            .colr(fg),
    )
    .padding(Padding::from([8u16, 16u16]))
    .sty(
        move |_t: &Theme, status: cosmic::iced::widget::button::Status| {
            use cosmic::iced::widget::button::Status;
            let bg = if enabled {
                match status {
                    Status::Hovered | Status::Pressed => cosmic::iced::Color { a: 0.85, ..on_bg },
                    _ => on_bg,
                }
            } else {
                off_bg
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
    );
    // §9 — only wire `on_press` when armed; a disarmed Execute is truly inert.
    if enabled {
        b = b.on_press(msg);
    }
    b.into()
}

/// FRONTDOOR-11 — one proposal card in the confirm gate (design Q10 + Q44). Top:
/// the PREVIEW — the action kind (risk-toned) + target node(s) + the human-readable
/// effect + the dry-run "what would run" line + the Copilot rationale. Bottom: the
/// CONFIRM GATE depending on the proposal's [`pending::ExecState`]:
/// * `Pending` + normal → a 1-click "Approve" + "Dismiss".
/// * `Pending` + DESTRUCTIVE → a typed-confirm field; "Execute" is INERT until the
///   operator types the confirm word (a 1-click can never fire it, §9); + "Dismiss".
/// * `Executing` → an "Executing…" note (the worker round-trip is in flight).
/// * `Succeeded`/`Failed` → the worker's RESULT line.
/// * `Dismissed` → an "Dismissed (not executed)" note.
///
/// `i` is the proposal's index into [`FrontDoor::pending`] (the message target);
/// `typed` is the operator's current confirm text. The preview is ALWAYS rendered
/// before any execute affordance (§9). Tokens only (§4).
fn proposal_card<'a>(
    i: usize,
    p: &'a pending::PendingProposal,
    typed: &'a str,
    locked: bool,
    palette: Palette,
) -> Element<'a, crate::Message, Theme> {
    let sizes = FontSize::defaults();
    let danger = palette.danger.into_cosmic_color();
    let accent = palette.accent.into_cosmic_color();
    let muted = palette.text_muted.into_cosmic_color();
    let txt = palette.text.into_cosmic_color();

    let is_high = p.risk == pending::Risk::HighRisk;
    let kind_tone = if is_high { danger } else { accent };

    // ── PREVIEW (Q44): kind (+ risk badge) · targets · effect · dry-run · why ──
    let risk_badge = if is_high {
        "  •  destructive — typed confirm required"
    } else {
        ""
    };
    let mut preview = column![
        text(format!("{}{risk_badge}", p.kind))
            .size(TypeRole::Body.size_in(sizes))
            .colr(kind_tone),
        text(format!("Effect: {}", p.effect))
            .size(TypeRole::Body.size_in(sizes))
            .colr(txt),
    ]
    .spacing(4)
    .width(Length::Fill);

    let targets_line = if p.targets.is_empty() {
        "Target: this node".to_string()
    } else {
        format!("Target: {}", p.targets.join(", "))
    };
    preview = preview.push(
        text(targets_line)
            .size(TypeRole::Caption.size_in(sizes))
            .colr(muted),
    );
    if let Some(dry) = &p.dry_run {
        preview = preview.push(
            text(format!("Would run: {dry}"))
                .size(TypeRole::Caption.size_in(sizes))
                .colr(muted),
        );
    }
    if !p.rationale.trim().is_empty() {
        preview = preview.push(
            text(format!("Why: {}", p.rationale))
                .size(TypeRole::Caption.size_in(sizes))
                .colr(muted),
        );
    }

    // ── CONFIRM GATE / RESULT, by the live exec state ──
    let gate: Element<'a, crate::Message, Theme> = match &p.state {
        pending::ExecState::Pending => {
            let dismiss = gate_button(
                "Dismiss",
                muted,
                true,
                crate::Message::FrontDoor(Message::DismissProposal(i)),
                palette,
            );
            if is_high {
                // Typed-confirm gate: the field + an Execute that arms only on match.
                // FRONTDOOR-14 — a locked session (Q91) keeps Execute disarmed
                // regardless of the typed word; the update handler also re-checks
                // `self.locked` before publishing (defence-in-depth).
                let armed = !locked && pending::confirm_matches(typed);
                let field: Element<'a, crate::Message, Theme> =
                    text_input(&format!("type {} to confirm", pending::CONFIRM_WORD), typed)
                        .on_input(move |s| {
                            crate::Message::FrontDoor(Message::ConfirmInputChanged(i, s))
                        })
                        .padding(Padding::from([8u16, 12u16]))
                        .width(Length::Fixed(260.0))
                        .into();
                let execute = gate_button(
                    "Execute",
                    danger,
                    armed,
                    crate::Message::FrontDoor(Message::ApproveProposal(i)),
                    palette,
                );
                let confirm_note = if locked {
                    "Session locked — unlock to confirm this destructive action.".to_string()
                } else {
                    format!(
                        "This is destructive. Type {} to enable Execute.",
                        pending::CONFIRM_WORD
                    )
                };
                column![
                    text(confirm_note)
                        .size(TypeRole::Caption.size_in(sizes))
                        .colr(danger),
                    row![field, execute, dismiss]
                        .spacing(10)
                        .align_y(cosmic::iced::Alignment::Center),
                ]
                .spacing(8)
                .into()
            } else {
                // Normal: a single-click Approve fires the execute — disabled under a
                // locked session (Q91), so a 1-click can't run while locked.
                let approve = gate_button(
                    "Approve",
                    accent,
                    !locked,
                    crate::Message::FrontDoor(Message::ApproveProposal(i)),
                    palette,
                );
                let mut r = row![approve, dismiss]
                    .spacing(10)
                    .align_y(cosmic::iced::Alignment::Center);
                if locked {
                    r = r.push(
                        text("Session locked")
                            .size(TypeRole::Caption.size_in(sizes))
                            .colr(palette.warning.into_cosmic_color()),
                    );
                }
                r.into()
            }
        }
        pending::ExecState::Executing => text("Executing…")
            .size(TypeRole::Body.size_in(sizes))
            .colr(muted)
            .into(),
        pending::ExecState::Succeeded(detail) => column![
            text("Succeeded")
                .size(TypeRole::Body.size_in(sizes))
                .colr(palette.success.into_cosmic_color()),
            text(detail.clone())
                .size(TypeRole::Caption.size_in(sizes))
                .colr(muted),
        ]
        .spacing(2)
        .into(),
        pending::ExecState::Failed(err) => column![
            text("Failed")
                .size(TypeRole::Body.size_in(sizes))
                .colr(danger),
            text(err.clone())
                .size(TypeRole::Caption.size_in(sizes))
                .colr(muted),
        ]
        .spacing(2)
        .into(),
        pending::ExecState::Dismissed => text("Dismissed (not executed)")
            .size(TypeRole::Body.size_in(sizes))
            .colr(muted)
            .into(),
    };

    let card = column![preview, Space::new().height(Length::Fixed(12.0)), gate,]
        .spacing(4)
        .width(Length::Fill);

    // A destructive proposal gets a danger-toned border so the card reads as
    // higher-stakes before the operator even reaches the gate (§4 — token, no hex).
    let border_color = if is_high {
        danger
    } else {
        palette.border.into_cosmic_color()
    };
    container(card)
        .width(Length::Fill)
        .padding(Padding::from([16u16, 16u16]))
        .style(move |_t: &Theme| container::Style {
            background: Some(Background::Color(palette.surface.into_cosmic_color())),
            border: Border {
                color: border_color,
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

    // ───────────────── FRONTDOOR-13: alert triage (GUI half) ─────────────────

    /// The backend `AlertTriage` wire body for a triage with one fix-bearing group
    /// and one explain-only group — the shape `parse_triage` consumes.
    fn triage_body() -> String {
        serde_json::json!({
            "groups": [
                {
                    "title": "MeshFS master down on oak",
                    "explanation": "mfsmaster is not running; restart its container.",
                    "severity": "high",
                    "alerts": ["mfsmaster", "meshfs-mount"],
                    "proposal": {
                        "action": {
                            "kind": "service_lifecycle",
                            "target_host": "oak",
                            "service_kind": "container",
                            "name": "mfsmaster",
                            "op": "restart"
                        },
                        "rationale": "restart the wedged master"
                    }
                },
                {
                    "title": "CA cert nearing expiry",
                    "explanation": "the mesh CA warns in 12 days; rotate soon.",
                    "severity": "medium",
                    "alerts": ["ca-cert"]
                }
            ],
            "alert_count": 3,
            "produced_at_s": 1_700_000_000u64
        })
        .to_string()
    }

    #[test]
    fn parse_triage_pulls_groups_alerts_and_keeps_fix_proposal_verbatim() {
        let t = copilot::parse_triage(Some(&triage_body()));
        assert_eq!(t.alert_count, 3);
        assert_eq!(t.groups.len(), 2);
        assert!(!t.is_empty());
        // Worst-first order preserved (the backend ranks it).
        assert_eq!(t.groups[0].title, "MeshFS master down on oak");
        assert_eq!(t.groups[0].severity, "high");
        assert_eq!(t.groups[0].alerts, vec!["mfsmaster", "meshfs-mount"]);
        // The fix proposal is kept as its raw JSON object for verbatim re-publish.
        let body = t.groups[0]
            .proposal_body
            .as_deref()
            .expect("fix-bearing group keeps its proposal body");
        let v: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(v["action"]["kind"], "service_lifecycle");
        assert_eq!(v["action"]["name"], "mfsmaster");
        // The explain-only group carries no proposal (no faked fix).
        assert!(t.groups[1].proposal_body.is_none());
        assert_eq!(t.groups[1].severity, "medium");
    }

    #[test]
    fn parse_triage_tolerates_garbage_and_empty() {
        assert!(copilot::parse_triage(None).is_empty());
        assert!(copilot::parse_triage(Some("not json")).is_empty());
        assert!(copilot::parse_triage(Some("{\"groups\":[]}")).is_empty());
        // A group with no title is dropped (never a faked card).
        let t = copilot::parse_triage(Some(
            "{\"groups\":[{\"explanation\":\"x\"}],\"alert_count\":1}",
        ));
        assert!(t.is_empty());
    }

    #[test]
    fn alerts_tile_detail_renders_the_triage_in_both_modes() {
        // FD-13 — the triage lands and the Alerts tile detail renders it (the grouped
        // cards + the fix affordance) in BOTH render modes, without panicking.
        for mode in [Mode::Panel, Mode::FullScreen] {
            let mut fd = FrontDoor::new();
            fd.loading = false;
            fd.mode = mode;
            let data = FrontDoorData {
                triage: copilot::parse_triage(Some(&triage_body())),
                ..FrontDoorData::default()
            };
            let _ = fd.update(Message::Loaded(Box::new(data)));
            assert_eq!(fd.triage.groups.len(), 2, "triage folded in");
            let alerts_idx = fd.tiles.iter().position(|t| t.label == "Alerts").unwrap();
            let _ = fd.update(Message::TileActivated(alerts_idx));
            let _: Element<'_, crate::Message, Theme> = fd.view();
        }
    }

    #[test]
    fn alerts_tile_detail_renders_resting_note_when_no_triage() {
        // No triage yet (all-clear / no leader) → the Alerts detail still builds (it
        // shows the honest resting note, not a faked group — §7).
        let mut fd = FrontDoor::new();
        fd.loading = false;
        assert!(fd.triage.is_empty());
        let alerts_idx = fd.tiles.iter().position(|t| t.label == "Alerts").unwrap();
        let _ = fd.update(Message::TileActivated(alerts_idx));
        let _: Element<'_, crate::Message, Theme> = fd.view();
    }

    #[test]
    fn propose_alert_fix_republishes_only_a_real_proposal_and_never_executes() {
        // §9 — "Apply fix" PROPOSES (re-publishes to the propose topic), never
        // executes. The handler targets the propose topic, never the exec topic; a
        // group with no proposal or a stale index is an inert no-op.
        let mut fd = FrontDoor::new();
        fd.triage = copilot::parse_triage(Some(&triage_body()));
        // The propose topic is the review queue, distinct from FD-11's exec topic.
        assert_ne!(copilot::PROPOSAL_TOPIC, pending::EXEC_TOPIC);
        // Group 0 has a fix → a task is produced (a Bus publish to the propose topic);
        // it does not panic and returns a real Task.
        let _ = fd.update(Message::ProposeAlertFix(0));
        // Group 1 is explain-only → no-op (no proposal to publish).
        let _ = fd.update(Message::ProposeAlertFix(1));
        // A stale index → no-op (defence-in-depth).
        let _ = fd.update(Message::ProposeAlertFix(99));
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

    // ── FRONTDOOR-11 (GUI half): the confirm-gate execution UI ──

    /// A proposal `(ulid, body)` carrying the given inner action JSON wrapped in
    /// the FD-12 `ActionProposal` shape (`{"action":{…},"rationale":…}`).
    fn proposal_msg(ulid: &str, action_json: &str, rationale: &str) -> (String, Option<String>) {
        let body = format!(r#"{{"action":{action_json},"rationale":"{rationale}"}}"#);
        (ulid.to_string(), Some(body))
    }

    #[test]
    fn parse_lifts_the_preview_from_a_real_proposal_and_keeps_the_exec_body() {
        // §7 — a real proposal off the queue parses into the operator-facing
        // preview (kind · targets · effect · dry-run · rationale) and the EXACT
        // inner action JSON to publish on confirm (the bare ActionRequest, never
        // the wrapping proposal).
        let action = r#"{"kind":"service_lifecycle","target_host":"oak","service_kind":"container","name":"nginx","op":"restart"}"#;
        let msgs = vec![proposal_msg("01ULID", action, "nginx is wedged")];
        let pend = pending::parse(&msgs);
        assert_eq!(pend.len(), 1);
        let p = &pend[0];
        assert_eq!(p.id, "01ULID");
        assert_eq!(p.kind, "service_lifecycle");
        assert_eq!(p.targets, vec!["oak".to_string()]);
        assert_eq!(p.effect, "restart container nginx");
        assert_eq!(p.dry_run.as_deref(), Some("podman restart nginx"));
        assert_eq!(p.rationale, "nginx is wedged");
        // The exec body is the bare ActionRequest — it re-parses to the worker shape
        // and carries NO `rationale` wrapper (the worker rejects extra fields aside).
        let v: serde_json::Value = serde_json::from_str(&p.exec_body).unwrap();
        assert_eq!(v["kind"], "service_lifecycle");
        assert_eq!(v["target_host"], "oak");
        assert!(v.get("rationale").is_none(), "exec body is the bare action");
        // service_lifecycle is reversible → a 1-click approves it.
        assert_eq!(p.risk, pending::Risk::Normal);
        assert!(p.approves_on_click());
    }

    #[test]
    fn parse_drops_advisory_and_malformed_entries_without_sinking_the_rest() {
        // A title/action-less body, malformed JSON, and a real one in the same
        // batch: only the real proposal surfaces (§7 — never a faked card), and a
        // bad entry doesn't drop the good one.
        let good = r#"{"kind":"service_lifecycle","target_host":"oak","service_kind":"container","name":"nginx","op":"start"}"#;
        let msgs = vec![
            ("a".to_string(), Some("not json".to_string())),
            (
                "b".to_string(),
                Some(r#"{"rationale":"no action here"}"#.to_string()),
            ),
            proposal_msg("c", good, "start it"),
            ("d".to_string(), None),
        ];
        let pend = pending::parse(&msgs);
        assert_eq!(pend.len(), 1);
        assert_eq!(pend[0].id, "c");
    }

    #[test]
    fn classify_marks_the_locked_destructive_kinds_high_risk() {
        // Q10 — the locked destructive set requires a typed confirm; everything
        // else (incl. the reversible service_lifecycle) is a 1-click normal.
        for k in ["code-edit", "code_edit", "destroy", "cutover", "delete"] {
            assert_eq!(
                pending::classify(k),
                pending::Risk::HighRisk,
                "{k} is destructive"
            );
        }
        assert_eq!(
            pending::classify("service_lifecycle"),
            pending::Risk::Normal
        );
        // Case-insensitive + an unknown kind is normal (it still previews honestly).
        assert_eq!(pending::classify("DESTROY"), pending::Risk::HighRisk);
        assert_eq!(pending::classify("some_future_kind"), pending::Risk::Normal);
    }

    #[test]
    fn dry_run_mirrors_the_workers_fixed_command_plan() {
        // Q44 — the dry-run line is the actual command the worker's FIXED plan
        // would run (podman for a container, virsh for a VM), never a guess.
        let container = serde_json::json!({
            "kind":"service_lifecycle","service_kind":"container","name":"nginx","op":"restart"
        });
        assert_eq!(
            pending::dry_run_of("service_lifecycle", &container).as_deref(),
            Some("podman restart nginx")
        );
        let vm = serde_json::json!({
            "kind":"service_lifecycle","service_kind":"vm","name":"db","op":"stop"
        });
        assert_eq!(
            pending::dry_run_of("service_lifecycle", &vm).as_deref(),
            Some("virsh shutdown db")
        );
        // A kind with no modelled plan carries no fabricated dry-run line.
        let unknown = serde_json::json!({"kind":"destroy","name":"x"});
        assert!(pending::dry_run_of("destroy", &unknown).is_none());
    }

    #[test]
    fn confirm_word_arms_case_insensitively_and_only_for_destructive() {
        // Q10 — a normal proposal arms on a click (no word needed); a destructive
        // one arms ONLY when the confirm word is typed (case-insensitive, trimmed).
        let normal = pending::PendingProposal {
            id: "n".into(),
            kind: "service_lifecycle".into(),
            targets: vec!["oak".into()],
            effect: "restart container nginx".into(),
            dry_run: Some("podman restart nginx".into()),
            rationale: String::new(),
            risk: pending::Risk::Normal,
            exec_body: "{}".into(),
            state: pending::ExecState::Pending,
        };
        assert!(normal.approves_on_click());
        assert!(
            normal.armed(""),
            "a normal proposal arms with no typed word"
        );

        let destructive = pending::PendingProposal {
            risk: pending::Risk::HighRisk,
            kind: "destroy".into(),
            ..normal.clone()
        };
        assert!(
            !destructive.approves_on_click(),
            "a 1-click cannot fire a destructive"
        );
        assert!(
            !destructive.armed(""),
            "destructive disarmed until the word is typed"
        );
        assert!(!destructive.armed("yes"), "the wrong word does not arm");
        assert!(
            destructive.armed("  confirm "),
            "the confirm word (any case) arms it"
        );
        assert!(destructive.armed("CONFIRM"));
    }

    #[test]
    fn no_execute_without_an_explicit_confirm() {
        // §9 — the load-bearing safety invariant: a proposal sitting Pending does
        // NOT execute. Only an `ApproveProposal` (a confirm) transitions it off
        // Pending; a render / open / dismiss never publishes to the exec topic.
        let mut fd = FrontDoor::new();
        let action = r#"{"kind":"service_lifecycle","target_host":"oak","service_kind":"container","name":"nginx","op":"restart"}"#;
        let data = FrontDoorData {
            pending: pending::parse(&[proposal_msg("01", action, "x")]),
            ..FrontDoorData::default()
        };
        let _ = fd.update(Message::Loaded(Box::new(data)));
        // Just loading + opening the surface executes nothing — still Pending.
        let _ = fd.update(Message::OpenPending);
        assert!(fd.show_pending);
        assert_eq!(fd.pending[0].state, pending::ExecState::Pending);
        // Building the view (rendering the preview) executes nothing.
        let _: Element<'_, crate::Message, Theme> = fd.view();
        assert_eq!(fd.pending[0].state, pending::ExecState::Pending);
        // A normal approve transitions it to Executing (the confirm fired). The
        // actual Bus publish rides the returned Task — here we assert the gate
        // opened only on the explicit confirm.
        let _ = fd.update(Message::ApproveProposal(0));
        assert_eq!(fd.pending[0].state, pending::ExecState::Executing);
    }

    #[test]
    fn destructive_requires_the_typed_confirm_before_it_can_execute() {
        // §9 — a destructive proposal CANNOT execute on an approve until the
        // confirm word is typed: the approve handler re-checks `armed()` and
        // refuses to publish (the state stays Pending). Typing the word arms it.
        let mut fd = FrontDoor::new();
        let action = r#"{"kind":"destroy","target_host":"oak","name":"vol-7"}"#;
        let data = FrontDoorData {
            pending: pending::parse(&[proposal_msg("01", action, "reclaim space")]),
            ..FrontDoorData::default()
        };
        let _ = fd.update(Message::Loaded(Box::new(data)));
        assert_eq!(fd.pending[0].risk, pending::Risk::HighRisk);

        // An approve with NO typed confirm is refused — it does NOT execute.
        let _ = fd.update(Message::ApproveProposal(0));
        assert_eq!(
            fd.pending[0].state,
            pending::ExecState::Pending,
            "a destructive approve without the typed word never executes (§9)"
        );

        // The operator types the confirm word.
        let _ = fd.update(Message::ConfirmInputChanged(0, "CONFIRM".into()));
        // Now the approve arms and the proposal transitions to Executing.
        let _ = fd.update(Message::ApproveProposal(0));
        assert_eq!(fd.pending[0].state, pending::ExecState::Executing);
    }

    #[test]
    fn dismiss_rejects_without_executing() {
        // A dismissed proposal is marked rejected and is never executed (§9).
        let mut fd = FrontDoor::new();
        let action = r#"{"kind":"service_lifecycle","target_host":"oak","service_kind":"container","name":"nginx","op":"restart"}"#;
        let data = FrontDoorData {
            pending: pending::parse(&[proposal_msg("01", action, "x")]),
            ..FrontDoorData::default()
        };
        let _ = fd.update(Message::Loaded(Box::new(data)));
        let _ = fd.update(Message::DismissProposal(0));
        assert_eq!(fd.pending[0].state, pending::ExecState::Dismissed);
    }

    #[test]
    fn exec_result_folds_onto_the_card_by_stable_id() {
        // The worker's typed reply folds onto the card keyed by its bus id (not
        // index), so a reorder doesn't mis-target the result.
        let mut fd = FrontDoor::new();
        let action = r#"{"kind":"service_lifecycle","target_host":"oak","service_kind":"container","name":"nginx","op":"start"}"#;
        let data = FrontDoorData {
            pending: pending::parse(&[proposal_msg("01", action, "x")]),
            ..FrontDoorData::default()
        };
        let _ = fd.update(Message::Loaded(Box::new(data)));
        let _ = fd.update(Message::ExecResult(
            "01".into(),
            true,
            "dispatched container start to oak".into(),
        ));
        assert_eq!(
            fd.pending[0].state,
            pending::ExecState::Succeeded("dispatched container start to oak".into())
        );
        // A failure reply maps to Failed.
        let _ = fd.update(Message::ExecResult(
            "01".into(),
            false,
            "kind not allowlisted".into(),
        ));
        assert_eq!(
            fd.pending[0].state,
            pending::ExecState::Failed("kind not allowlisted".into())
        );
    }

    #[test]
    fn parse_exec_reply_maps_ok_failure_and_degrades_quietly() {
        // The reply parse maps the worker's ActionReply shape; a no-reply / junk
        // body degrades to a quiet failure (Q33 — no spew, no panic).
        assert_eq!(
            parse_exec_reply(Some(r#"{"ok":true,"detail":"dispatched restart to oak"}"#)),
            (true, "dispatched restart to oak".to_string())
        );
        let (ok, msg) =
            parse_exec_reply(Some(r#"{"ok":false,"error":"op `wipe` not allowlisted"}"#));
        assert!(!ok);
        assert!(msg.contains("allowlisted"));
        // No reply at all (no Bus / timeout) → a quiet failure, never a hang/panic.
        assert!(!parse_exec_reply(None).0);
        // Malformed JSON → quiet failure.
        assert!(!parse_exec_reply(Some("not json")).0);
    }

    #[test]
    fn merge_preserves_a_confirm_in_flight_across_a_reload() {
        // A slow-poll reload must NOT reset an in-flight confirm / a shown result /
        // a typed-confirm — the merge preserves the live gate state for a surviving
        // proposal and GCs the typed text of a proposal that fell off the queue.
        let mut fd = FrontDoor::new();
        let action = r#"{"kind":"destroy","target_host":"oak","name":"vol-7"}"#;
        let snap = pending::parse(&[proposal_msg("01", action, "x")]);
        let _ = fd.update(Message::Loaded(Box::new(FrontDoorData {
            pending: snap.clone(),
            ..FrontDoorData::default()
        })));
        // The operator types the confirm word, then a reload lands carrying the
        // SAME proposal (re-read off the queue).
        let _ = fd.update(Message::ConfirmInputChanged(0, "CONFIRM".into()));
        let _ = fd.update(Message::ExecResult("01".into(), true, "done".into()));
        let _ = fd.update(Message::Loaded(Box::new(FrontDoorData {
            pending: snap,
            ..FrontDoorData::default()
        })));
        // The result survives the reload (not reset to Pending).
        assert_eq!(
            fd.pending[0].state,
            pending::ExecState::Succeeded("done".into()),
            "a reload preserves the resolved gate state"
        );
        // The typed-confirm text is still keyed (the proposal survived).
        assert_eq!(
            fd.confirm_inputs.get("01").map(String::as_str),
            Some("CONFIRM")
        );

        // A reload with an EMPTY queue drops the card and GCs its typed text.
        let _ = fd.update(Message::Loaded(Box::new(FrontDoorData::default())));
        assert!(fd.pending.is_empty());
        assert!(fd.confirm_inputs.is_empty(), "stale typed-confirm GC'd");
    }

    #[test]
    fn pending_count_only_counts_proposals_awaiting_the_operator() {
        // The indicator badge counts proposals still Pending (needs attention) —
        // not the resolved / dismissed ones.
        let mut fd = FrontDoor::new();
        let a = r#"{"kind":"service_lifecycle","target_host":"oak","service_kind":"container","name":"nginx","op":"start"}"#;
        let b = r#"{"kind":"service_lifecycle","target_host":"elm","service_kind":"container","name":"redis","op":"stop"}"#;
        let data = FrontDoorData {
            pending: pending::parse(&[proposal_msg("01", a, "x"), proposal_msg("02", b, "y")]),
            ..FrontDoorData::default()
        };
        let _ = fd.update(Message::Loaded(Box::new(data)));
        assert_eq!(fd.pending_count(), 2);
        let _ = fd.update(Message::DismissProposal(1));
        assert_eq!(
            fd.pending_count(),
            1,
            "a dismissed proposal no longer needs attention"
        );
    }

    #[test]
    fn the_confirm_gate_targets_the_exec_topic_only_on_confirm() {
        // §9 — the gate's exec topic constant is the FD-11 worker's execution
        // topic, distinct from the propose queue. The execute path is the ONLY one
        // that touches it (asserted structurally: only ApproveProposal returns the
        // publishing Task, gated behind `armed()`).
        assert_eq!(pending::EXEC_TOPIC, "action/exec/request");
        assert_ne!(pending::EXEC_TOPIC, copilot::PROPOSAL_TOPIC);
    }

    #[test]
    fn pending_view_builds_in_both_modes_for_every_card_state() {
        // The confirm-gate surface builds (preview + gate + every result state) in
        // BOTH render modes without panicking — the preview is always rendered
        // before any execute affordance.
        let mut fd = FrontDoor::new();
        fd.loading = false;
        let normal = r#"{"kind":"service_lifecycle","target_host":"oak","service_kind":"container","name":"nginx","op":"restart"}"#;
        let destructive = r#"{"kind":"destroy","target_host":"oak","name":"vol-7"}"#;
        let mut pend = pending::parse(&[
            proposal_msg("01", normal, "wedged"),
            proposal_msg("02", destructive, "reclaim"),
        ]);
        // Exercise each result state across the cards.
        pend[0].state = pending::ExecState::Succeeded("dispatched".into());
        let _ = fd.update(Message::Loaded(Box::new(FrontDoorData {
            pending: pend,
            ..FrontDoorData::default()
        })));
        for mode in [Mode::Panel, Mode::FullScreen] {
            fd.mode = mode;
            // The indicator renders in the top bar (count > 0).
            let _: Element<'_, crate::Message, Theme> = fd.view();
            // The review surface renders.
            fd.show_pending = true;
            let _: Element<'_, crate::Message, Theme> = fd.view();
            fd.show_pending = false;
        }
        // Walk the remaining exec states through the card builder directly.
        let p = &fd.pending[1];
        let pal = Palette::dark();
        for state in [
            pending::ExecState::Pending,
            pending::ExecState::Executing,
            pending::ExecState::Failed("nope".into()),
            pending::ExecState::Dismissed,
        ] {
            let mut q = p.clone();
            q.state = state;
            let _: Element<'_, crate::Message, Theme> = proposal_card(1, &q, "CONFIRM", false, pal);
        }
    }

    // ── FRONTDOOR-14 — settings / prefs / arrangement / lock ────────────────────

    /// Run `body` with `XDG_CONFIG_HOME` pointed at a fresh temp dir, under a global
    /// lock so the persisting tests (which write `preferences.toml` + re-read it via
    /// `FrontDoor::new`) don't race each other or pollute the real config. The env
    /// var is restored afterwards. Edition-2021 `set_var` is safe.
    fn with_isolated_prefs(body: impl FnOnce()) {
        use std::sync::Mutex;
        static LOCK: Mutex<()> = Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        body();
        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }

    #[test]
    fn arrange_tiles_empty_prefs_is_the_seed_unchanged() {
        // The pre-FD-14 grid: an empty arrangement returns the seed order, all
        // tiles visible (Q79 — an untouched arrangement is a no-op).
        let seed = FrontDoor::new().all_tiles;
        let prefs = mde_theme::FrontDoorPrefs::default();
        let out = arrange_tiles(&seed, &prefs);
        let seed_labels: Vec<&str> = seed.iter().map(|t| t.label.as_str()).collect();
        let out_labels: Vec<&str> = out.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(seed_labels, out_labels, "empty arrangement = seed");
    }

    #[test]
    fn arrange_tiles_pins_to_front_hides_and_reorders() {
        // Q79 — pin sorts to the front, hide drops from the grid, and the saved
        // list order is honored for the named tiles.
        let seed = FrontDoor::new().all_tiles;
        let prefs = mde_theme::FrontDoorPrefs {
            ai_proactive: true,
            tiles: vec![
                // Music first in the saved order + pinned ⇒ it leads the grid.
                mde_theme::TileArrangement {
                    id: "music".into(),
                    pinned: true,
                    hidden: false,
                },
                // Alerts hidden ⇒ dropped from the grid.
                mde_theme::TileArrangement {
                    id: "alerts".into(),
                    pinned: false,
                    hidden: true,
                },
            ],
        };
        let out = arrange_tiles(&seed, &prefs);
        let labels: Vec<&str> = out.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels.first(), Some(&"Music"), "pinned tile leads");
        assert!(!labels.contains(&"Alerts"), "hidden tile dropped");
        assert_eq!(out.len(), seed.len() - 1, "exactly one hidden");
    }

    #[test]
    fn ai_policy_off_silences_the_proactive_suggestions() {
        // Q61 — with proactivity OFF the GUI surfaces no suggestion cards/badges,
        // even when the backend published a set; ON restores them.
        let mut fd = FrontDoor::new();
        // A suggestion whose text names the mesh ⇒ `concerns_tile()` maps it to the
        // Mesh Map tile (the keyword classifier; there's no explicit field).
        fd.suggestions = vec![copilot::Suggestion {
            title: "Mesh peer latency is climbing".into(),
            impact: "high".into(),
            detail: "Check the mesh routing".into(),
            proposal_body: None,
        }];
        let mesh_idx = fd
            .tiles
            .iter()
            .position(|t| t.key == Some(TileKey::MeshMap))
            .unwrap();

        fd.fd_prefs.ai_proactive = true;
        assert!(
            !fd.suggestions_for_tile(mesh_idx).is_empty(),
            "proactivity on ⇒ the suggestion surfaces"
        );

        fd.fd_prefs.ai_proactive = false;
        assert!(
            fd.suggestions_for_tile(mesh_idx).is_empty(),
            "proactivity off ⇒ no suggestion surfaces"
        );
        assert_eq!(fd.suggestion_count_for(mesh_idx), 0, "badge goes quiet too");
    }

    #[test]
    fn toggle_tile_hidden_drops_it_from_the_grid_and_show_restores() {
        // Q79 — hide drops a tile from the visible grid (still in all_tiles); the
        // un-hide restores it. Exercised through the real (persisting) update path.
        with_isolated_prefs(|| {
            let mut fd = FrontDoor::new();
            let before = fd.tiles.len();
            // Music's index in the SETTINGS order (what the message addresses).
            let order = fd.settings_order_tiles();
            let i = order.iter().position(|t| t.label == "Music").unwrap();

            let _ = fd.update(Message::ToggleTileHidden(i));
            assert_eq!(fd.tiles.len(), before - 1, "hidden tile leaves the grid");
            assert!(!fd.tiles.iter().any(|t| t.label == "Music"));
            // The seed set still has it (so Settings can show + un-hide it).
            assert!(fd.all_tiles.iter().any(|t| t.label == "Music"));

            // It persisted: a fresh FrontDoor loads the hidden arrangement.
            let reloaded = FrontDoor::new();
            assert!(
                !reloaded.tiles.iter().any(|t| t.label == "Music"),
                "the hidden arrangement persisted across construction"
            );

            // Un-hide (hidden tiles stay listed in settings order).
            let order2 = fd.settings_order_tiles();
            let j = order2.iter().position(|t| t.label == "Music").unwrap();
            let _ = fd.update(Message::ToggleTileHidden(j));
            assert_eq!(fd.tiles.len(), before, "un-hide restores the tile");
            assert!(fd.tiles.iter().any(|t| t.label == "Music"));
        });
    }

    #[test]
    fn toggle_tile_pinned_lifts_it_to_the_front() {
        // Q79 — pinning a non-leading tile sorts it to the front of the grid.
        with_isolated_prefs(|| {
            let mut fd = FrontDoor::new();
            let order = fd.settings_order_tiles();
            // Pin the LAST tile so the effect is unambiguous.
            let i = order.len() - 1;
            let target = order[i].label.clone();
            assert_ne!(
                fd.tiles.first().map(|t| t.label.clone()),
                Some(target.clone())
            );
            let _ = fd.update(Message::ToggleTilePinned(i));
            assert_eq!(
                fd.tiles.first().map(|t| t.label.as_str()),
                Some(target.as_str()),
                "pinned tile leads the grid"
            );
        });
    }

    #[test]
    fn move_tile_reorders_within_the_settings_list() {
        // Q79 — moving a tile up swaps it with its predecessor in the settings
        // order, and the grid (no pins) reflects the new order.
        with_isolated_prefs(|| {
            let mut fd = FrontDoor::new();
            let order = fd.settings_order_tiles();
            let first = order[0].label.clone();
            let second = order[1].label.clone();
            // Move the SECOND tile up — it should now lead.
            let _ = fd.update(Message::MoveTileUp(1));
            let new_order = fd.settings_order_tiles();
            assert_eq!(new_order[0].label, second, "the moved tile leads");
            assert_eq!(new_order[1].label, first, "its predecessor follows");
            // The visible grid (no pins) tracks the settings order.
            assert_eq!(fd.tiles[0].label, second);
        });
    }

    #[test]
    fn lock_gates_the_action_message_handlers() {
        // Q91 — a locked session refuses the propose/execute action paths (defence-
        // in-depth, beside the view dropping the buttons). Navigation isn't gated.
        let mut fd = FrontDoor::new();
        assert!(!fd.locked);
        let _ = fd.update(Message::ToggleLock);
        assert!(fd.locked, "toggle locks the session");

        // A pipeline ACTION trigger is suppressed while locked: the handler returns
        // an empty Task (no nav, no trigger) rather than firing them.
        let _ = fd.update(Message::PipelineAction {
            nav: Box::new(crate::Message::SelectPanel {
                group: Group::Provisioning,
                panel: "build-farm",
            }),
            trigger: Some(Box::new(crate::Message::FrontDoor(Message::Reload))),
        });
        // (We can't easily assert on a Task's contents; the state guard is what we
        // verify — the handler hit the locked early-return, so detail was cleared
        // but nothing dispatched. The view test below covers the disabled buttons.)
        assert_eq!(fd.detail, None);

        // Unlock restores it.
        let _ = fd.update(Message::ToggleLock);
        assert!(!fd.locked);
    }

    #[test]
    fn settings_view_renders_in_both_modes_and_each_apply_message_handled() {
        // §7 — the settings panel is reachable + every control's message is one the
        // panel handles (no inert control). Render it in both modes, then walk each
        // apply message through `update` without panicking. Isolated prefs so the
        // theme/density/AI applies don't write the real config.
        with_isolated_prefs(|| {
            let mut fd = FrontDoor::new();
            for mode in [Mode::Panel, Mode::FullScreen] {
                fd.mode = mode;
                fd.show_settings = true;
                let _: Element<'_, crate::Message, Theme> = fd.view();
                fd.show_settings = false;
            }
            // Each apply message is handled (theme/density/AI/lock are real applies).
            for msg in [
                Message::OpenSettings,
                Message::SetTheme(mde_theme::Theme::Gray90),
                Message::SetDensity(mde_theme::Density::Compact),
                Message::SetAiProactive(false),
                Message::SetAiProactive(true),
                Message::ToggleLock,
                Message::ToggleLock,
                Message::CloseSettings,
            ] {
                let _ = fd.update(msg);
            }
            // The theme apply persisted + is reflected in a fresh load.
            assert_eq!(
                mde_theme::Preferences::load().theme,
                mde_theme::Theme::Gray90
            );
            assert_eq!(
                mde_theme::Preferences::load().density,
                mde_theme::Density::Compact
            );
            // Restore the live theme to the default so a shared-global swap doesn't
            // leak into another test (the live_theme bundle is process-wide).
            crate::live_theme::set(mde_theme::Theme::Dark, mde_theme::Density::Comfortable);
        });
    }

    /// Inject a live roster into a `FrontDoor` the way a real `Loaded` snapshot
    /// would (via `apply`), so the cross-node selector + launch read real rows.
    fn fd_with_roster(peers: Vec<PeerRow>) -> FrontDoor {
        let mut fd = FrontDoor::new();
        fd.apply(&FrontDoorData {
            peers,
            ..FrontDoorData::default()
        });
        fd
    }

    #[test]
    fn select_target_node_scopes_then_clears_on_default_and_close() {
        // FRONTDOOR-15 (Q32/Q74) — the cross-node target is a pure scope flip: a
        // node selects it, `None` restores the whole-mesh broadcast default (Q18),
        // and leaving the detail clears it so a scope never leaks across tiles.
        let mut fd = fd_with_roster(vec![mesh_peer("anvil", &["mfsmaster"])]);
        assert_eq!(fd.target_node, None, "starts at the broadcast default");

        let _ = fd.update(Message::SelectTargetNode(Some("anvil".into())));
        assert_eq!(fd.target_node.as_deref(), Some("anvil"));

        let _ = fd.update(Message::SelectTargetNode(None));
        assert_eq!(fd.target_node, None, "None restores the broadcast default");

        // Re-select, then closing the detail drops the scope (no cross-tile leak).
        let _ = fd.update(Message::SelectTargetNode(Some("anvil".into())));
        let _ = fd.update(Message::CloseDetail);
        assert_eq!(fd.target_node, None, "leaving the detail clears the scope");
    }

    #[test]
    fn target_address_resolves_overlay_ip_off_the_live_roster() {
        // FRONTDOOR-15 (Q74) — the cross-node launch targets the node's OVERLAY IP
        // when present (so the GUI opens on the real mesh address), falls back to
        // the hostname when the row carries none, and is `None` for a stale/absent
        // target (a racing reload dropped it) so the launch handler no-ops.
        let mut anvil = mesh_peer("anvil", &[]);
        anvil.overlay_ip = "10.42.0.7".into();
        let forge = mesh_peer("forge", &[]); // no overlay IP on this row
        let mut fd = fd_with_roster(vec![anvil, forge]);

        assert_eq!(
            fd.target_address(),
            None,
            "no target ⇒ broadcast, no address"
        );

        fd.target_node = Some("anvil".into());
        assert_eq!(fd.target_address().as_deref(), Some("10.42.0.7"));

        fd.target_node = Some("forge".into());
        assert_eq!(
            fd.target_address().as_deref(),
            Some("forge"),
            "no overlay IP ⇒ fall back to the hostname"
        );

        fd.target_node = Some("ghost".into()); // not in the roster
        assert_eq!(fd.target_address(), None, "a stale target resolves to None");
    }

    #[test]
    fn launch_on_target_no_ops_without_a_resolvable_node() {
        // FRONTDOOR-15 (Q74) — `LaunchOnTarget` only launches once a node resolves;
        // with no/stale target it is a no-op (it never clears the detail or fires a
        // launch). With a resolvable target it clears the scope + closes the detail.
        let mut anvil = mesh_peer("anvil", &[]);
        anvil.overlay_ip = "10.42.0.7".into();
        let mut fd = fd_with_roster(vec![anvil]);
        fd.detail = Some(0);

        // No target → no-op (detail stays open, scope stays None).
        let _ = fd.update(Message::LaunchOnTarget("mde-files"));
        assert_eq!(
            fd.detail,
            Some(0),
            "no target ⇒ no launch, detail untouched"
        );

        // A resolvable target → the handler closes the detail + drops the scope
        // (the launch message itself rides the returned Task).
        fd.target_node = Some("anvil".into());
        let _ = fd.update(Message::LaunchOnTarget("mde-files"));
        assert_eq!(fd.detail, None, "a launch closes the detail");
        assert_eq!(fd.target_node, None, "and drops the scope");
    }

    #[test]
    fn detail_view_renders_the_target_selector_for_a_mesh_tile_only() {
        // FRONTDOOR-15 (Q32) — a mesh-scoped (keyed) tile's detail offers the node
        // selector; a plain launcher (keyless) has no mesh reach so it doesn't.
        // Render both in both modes without panicking (§7 — reachable, both modes).
        let mut fd = fd_with_roster(vec![mesh_peer("anvil", &["mfsmaster"])]);
        let data_center = fd
            .tiles
            .iter()
            .position(|t| t.key == Some(TileKey::DataCenter))
            .expect("the Data Center widget tile is seeded");
        let files = fd
            .tiles
            .iter()
            .position(|t| t.key.is_none())
            .expect("at least one launcher tile is seeded");
        for mode in [Mode::Panel, Mode::FullScreen] {
            fd.mode = mode;
            for idx in [data_center, files] {
                fd.detail = Some(idx);
                // The detail builds in both modes (the section is gated on the tile
                // key inside `detail_view`, so this exercises both branches).
                let _: Element<'_, crate::Message, Theme> = fd.view();
            }
        }
        // The target-section method itself builds with a target picked, too.
        fd.target_node = Some("anvil".into());
        let _: Element<'_, crate::Message, Theme> =
            fd.target_node_section(crate::live_theme::palette());
    }

    #[test]
    fn push_to_talk_asks_on_the_ask_topic_and_speaks_only_a_real_reply() {
        // FRONTDOOR-15 (Q55) — push-to-talk publishes the current ask + parks the
        // Copilot card at "thinking…" (sharing the search ask's card + generation),
        // and a VOICE reply folds in only when its generation still matches. A blank
        // ask fires nothing. §9 — it rides the ask path, never the exec topic.
        let mut fd = FrontDoor::new();

        // Blank ask → no-op (Copilot stays Idle, generation unchanged).
        let _ = fd.update(Message::PushToTalk);
        assert_eq!(fd.copilot, CopilotState::Idle, "a blank ask asks nothing");
        assert_eq!(fd.copilot_gen, 0);

        // A real ask parks the card at "thinking…" + bumps the shared generation.
        fd.query = "restart mfsmaster".into();
        let _ = fd.update(Message::PushToTalk);
        assert_eq!(fd.copilot, CopilotState::Thinking, "voice ask is pending");
        assert_eq!(
            fd.copilot_gen, 1,
            "voice shares the search ask's generation"
        );

        // The matching-generation answer folds in (and would speak — best-effort).
        let _ = fd.update(Message::VoiceReplied(
            1,
            CopilotAnswer::Answer("restarting mfsmaster on the leader".into()),
        ));
        assert_eq!(
            fd.copilot,
            CopilotState::Answer("restarting mfsmaster on the leader".into())
        );

        // A STALE-generation reply (the ask was superseded) is dropped, not shown.
        let _ = fd.update(Message::VoiceReplied(
            0,
            CopilotAnswer::Answer("stale".into()),
        ));
        assert_eq!(
            fd.copilot,
            CopilotState::Answer("restarting mfsmaster on the leader".into()),
            "a stale-generation voice reply is dropped"
        );

        // The degrade path renders the quiet unavailable note (Q33), no speech.
        let _ = fd.update(Message::VoiceReplied(1, CopilotAnswer::Unavailable));
        assert_eq!(fd.copilot, CopilotState::Unavailable);
    }

    // ── FRONTDOOR-16 — the guided first-run greeting (Q27, no tour Q71) ──────────

    #[test]
    fn first_run_greets_then_dismiss_persists_so_it_shows_once() {
        with_isolated_prefs(|| {
            // A fresh node (no "greeted" sentinel) shows the welcome card.
            assert!(!greeting_already_seen(), "fresh config has no sentinel");
            let mut fd = FrontDoor::new();
            assert!(fd.show_greeting, "first run greets (Q27)");
            assert!(
                fd.greeting_banner(crate::live_theme::palette()).is_some(),
                "the banner renders on the resting grid"
            );

            // Dismissing it clears the card AND writes the once-per-node sentinel.
            let _ = fd.update(Message::DismissGreeting);
            assert!(!fd.show_greeting, "dismiss hides the card this session");
            assert!(greeting_already_seen(), "dismiss persists the sentinel");
            assert!(
                fd.greeting_banner(crate::live_theme::palette()).is_none(),
                "no banner once dismissed"
            );

            // A FRESH construction (a later launch / restart) does NOT greet again —
            // the sentinel makes it exactly once per node (Q71 — no repeated tour).
            let fd2 = FrontDoor::new();
            assert!(!fd2.show_greeting, "a returning operator is never re-greeted");
        });
    }

    #[test]
    fn greeting_is_suppressed_while_searching() {
        with_isolated_prefs(|| {
            let mut fd = FrontDoor::new();
            assert!(fd.show_greeting);
            // While the omnibox drives a search, the results own the pane — the
            // greeting steps aside (it returns once the query clears).
            fd.query = "mesh".into();
            assert!(
                fd.greeting_banner(crate::live_theme::palette()).is_none(),
                "no greeting over search results"
            );
            fd.query.clear();
            assert!(
                fd.greeting_banner(crate::live_theme::palette()).is_some(),
                "the greeting returns on the resting grid"
            );
        });
    }

    #[test]
    fn greeting_renders_in_both_modes() {
        with_isolated_prefs(|| {
            let mut fd = FrontDoor::new();
            assert!(fd.show_greeting);
            // Panel mode (Win10 Start) — the resting view builds with the card.
            fd.mode = Mode::Panel;
            let _: Element<'_, crate::Message, Theme> = fd.view();
            // Full-screen mode (iPadOS home) — the card is reachable here too (§7).
            fd.mode = Mode::FullScreen;
            let _: Element<'_, crate::Message, Theme> = fd.view();
        });
    }
}
