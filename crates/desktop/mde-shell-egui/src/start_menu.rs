//! WIN7-2 — the **Start Menu** shell (design `docs/design/win7-desktop-survey.md`,
//! locks #2/#4/#10/#13/#14; WIN7-DESKTOP-1's second implementation unit).
//!
//! The fixed-size overlay panel that replaces the dock's Start-cell-opens-
//! Console behaviour (WIN7-1 relabelled the cell "Start" but left its click
//! wired to Console directly — this unit is the real two-pane Start Menu lock
//! #4 describes). It reuses the SAME floating-`egui::Area` + [`Motion`]-tweened
//! slide-up pattern `console.rs`'s old standalone panel already used (not a
//! new mechanism): fixed-size (lock #2 — never full-screen, never resizable),
//! anchored at the screen's true bottom-left edge, opening **upward** from the
//! bottom taskbar.
//!
//! **Panes (lock #4):** left = the Start launcher pane — live tiles, pinned
//! shortcuts, type-to-launch search, keyboard navigation, and per-tile context
//! rows; right = [`console::ConsoleState`] embedded via
//! [`console::console_content`] — CONSOLE-1's operational front door (groups,
//! Power section, Custom entries, the CONSOLE-2 spawn-tab seam). The Console
//! content is still owned by `console.rs`; this module owns the surrounding
//! Start Menu shell, left launcher pane, and open/close propagation.
//!
//! **Console's open state is now a mirror, not a source of truth.** Before this
//! unit, `ConsoleState::open` was the Start cell's own toggle latch. Now this
//! module's [`StartMenuState::open`] is the ONE latch (driven by the Start
//! cell click AND the Super key, lock #13 — see `main.rs`'s `mount_start_menu`
//! and its hotkey dispatch); each frame it mirrors into `ConsoleState` via
//! [`console::ConsoleState::set_open`] (the `DockState::set_active` idiom)
//! before rendering, so Console's focus ring / `handle_keys` still read a
//! meaningful "am I showing" bit. The mirror runs the OTHER way too: every
//! action that already closed the whole front door pre-WIN7-2 (a routed link,
//! a spawned tab, a fired power verb, Esc inside the embedded content) still
//! calls `ConsoleState::close` exactly as before (untouched in `console.rs`);
//! this module detects that self-closure (`console.is_open()` having gone
//! false while `state.open` is still true) and dismisses the WHOLE Start Menu
//! with it, so launching anything from the embedded Console still closes the
//! menu the way a Win10 Start Menu always did — never a dangling "Console says
//! closed but the panel is still up" desync.
//!
//! **Super key handoff:** the old vertical-dock reveal latch still exists in
//! retained state, but the rendered left dock is retired. `main.rs` drains the
//! same clean-Super-tap hotkey and opens/closes this Start Menu, which now
//! anchors to the screen's true left edge above the bottom taskbar.
//!
//! **Accesskit (lock #14):** the panel carries a `Role::Menu` label, pane
//! landmarks, named tile/search/context-row controls, live regions for
//! opening/closing, rotating tile facts, and search-result updates.
//!
//! **WIN7-3 update:** the left pane is the real live-tile grid (locks
//! #6/#7/#8/#23): all 19 [`Surface::ALL`] entries, grouped into the shared
//! launcher function groups (Mesh Control · Desktop & Session · Media · Files &
//! Data · Web · Developer Tools · Comms · System — [`TILE_GROUPS`]), each a uniform
//! [`TILE_W`]×[`TILE_H`] tile (lock #6 — one size, no variants). A tile wears
//! the SAME glyph the app picker already draws (`Surface::icon_id`) plus a
//! text label (`Surface::label`). A click reuses the picker's own
//! click-vs-Enter/Space activation predicate (`dock::response_activated`,
//! widened to `pub(crate)` for this reuse, not reimplemented) and records
//! the surface in a new [`StartMenuState::tile_activation`] slot, drained by
//! `main.rs` exactly like an embedded Console `Goto` request — both panes
//! end in the same "go to this surface, close the whole menu" outcome (lock
//! #23), just raised from different data. [`LEFT_PANE_W`] is no longer
//! WIN7-2's early fixed 288pt shell width; it is now sized to the real grid
//! this module renders.
//!
//! **WIN7-4 update:** the live-tile rotation itself (lock #5) — a new
//! [`TileFactInputs`] bundle ([`StartMenuState::set_tile_inputs`], mirroring
//! `dock.rs`'s `DockState::set_status_inputs`/`StatusInputs` idiom exactly)
//! carries the SAME already-published per-surface state an existing dock pip
//! or the surface's own status chip already reads (§7 honest-gating — see
//! each field's own doc comment for its exact source); [`tile_facts`] folds
//! it into 0-4 short per-surface strings the SAME way `dock::badge_for`
//! folds `StatusInputs` into a badge. A tile with 2+ facts rotates through
//! them every [`TILE_FACT_ROTATE_INTERVAL`] (lock #5's own "~4-5s per fact"
//! estimate); 0 facts keeps the static label WIN7-3 already painted; exactly
//! 1 fact replaces the label with that fact rather than showing both at once
//! — the locked 48pt tile height has no pixel room for a genuine third line
//! (verified against the real `Style` spacing tokens: the icon and label
//! already sit ~2px apart), a judgment call flagged in this unit's own
//! report rather than silently picked. [`tile_status_tint`] is no longer the
//! always-`None` seam WIN7-3 left inert: it now paints a severity colour
//! where a surface genuinely has one (System's Device/Power segment
//! rollups, MeshView's mesh health, Chat/Files' accent-on-nonzero-count —
//! the SAME tone language `dock.rs`'s own badges already use), `None`
//! everywhere else (most surfaces are plain counts, not health states — no
//! invented severity). Every rotating tile also carries a live accesskit
//! value (lock #14) and the whole tile grid exports ONE aggregate
//! `Live::Polite` summary node when anything is actually rotating (the
//! `status.rs` NOTIF-11 `status_live_region_id`/`install_status_accessibility`
//! precedent, restated here at the tile-grid level rather than per-tile —
//! mirroring status.rs's OWN shape of one live summary node plus per-item
//! value-bearing-but-not-individually-live nodes, not eight independent
//! announcers all firing on the same clock).
//!
//! **WIN7-5 update:** the right pane's redesigned presentation lives in
//! `console.rs`; this module's embedding contract is unchanged: the same
//! [`console::console_content`] call at the same right-pane rect, the same
//! [`ConsoleState::set_open`] mirror, and the same self-closure propagation.
//!
//! **WIN7-6 update:** a Critical firing now closes the menu too (lock #9) —
//! the same outcome as the other closers listed above (Esc / click-away / an
//! embedded Console self-closure / a tile activation), just triggered from
//! OUTSIDE this module for the first time. `main.rs` calls
//! [`StartMenuState::close`] (widened `pub(crate)`) directly off
//! [`status::CriticalEdgeCue::take_became_visible`]'s one-shot
//! hidden→visible edge, so an open menu closes exactly once per real firing
//! — never every frame the cue stays lit (that would also block reopening
//! the menu while a still-unacknowledged critical is up, which lock #9 never
//! asked for), and never again for the SAME critical once the operator has
//! acknowledged it. This module's own `close` already no-ops while closed,
//! so no new guard was needed here.
//!
//! **SHELL-UX-3 update:** type-to-launch search. A real [`egui::TextEdit`]
//! ([`search_field`]) sits at the bottom of the left pane (Win7's own
//! search-box spot), with a leading search glyph and a query-clear icon button;
//! it auto-focuses when the menu opens so "open, then just type" filters live.
//! An empty query is the unchanged grouped grid (zero behaviour change); the
//! moment anything is typed, [`search_matches`] ranks tileable surfaces and
//! static Console entries (case-insensitive: a label prefix beats a label
//! substring beats a group-name hit) and [`search_results`] paints that flat
//! list in place of the grid — Up/Down move a highlight, Enter launches (the top
//! match by default), a row click launches that row, Esc clears the query
//! (a second, now-empty Esc dismisses the menu). App launches route through the
//! SAME [`StartMenuState::tile_activation`] seam a tile click already uses;
//! Console launches route through `ConsoleState`, so the existing `main.rs`
//! drains still carry searched launches with no duplicate action behavior. The
//! box is a focused text field, so the embedded Console's own keyboard nav
//! politely steps aside (`console::handle_keys` bails while a text field owns
//! the keyboard) — one Enter never both launches a result and fires a Console
//! row. The one wrinkle Esc introduces (egui blurs the focused box on Esc, so
//! Console briefly sees the shared Esc and self-closes) is absorbed by a
//! one-frame guard in [`start_menu_panel`], keeping a query-clear from
//! collapsing the whole menu. Accesskit (lock #14): the box carries a
//! `SearchInput` role + label + live value, each result row a `Button` node
//! (the selected one flagged), and the result set exports ONE `Live::Polite`
//! summary (count + highlight) — the tile-grid live-region shape restated.
//!
//! **SM-QOL-1 update:** three launcher quality-of-life features, all
//! self-contained to this module (no `main.rs` seam change). (1) **Pinning /
//! favourites** — [`StartMenuState::pinned`] renders a "Pinned" section at the
//! TOP of the left pane (above the groups), empty until the operator pins
//! something; a surface is pinned/unpinned from a tile's right-click menu.
//! (2) **Arrow-key tile navigation** — the tile GRID (not just the search list
//! + Console) now takes Up/Down/Left/Right keyboard focus across its 3-column
//! rows AND the pinned section, reusing the dock's OWN focus idiom verbatim
//! (`ui.interact` + `memory.request_focus` + `set_focus_lock_filter` +
//! arrow-event consume, `dock::apply_picker_arrow_focus`) so `response_activated`
//! (the shared click-vs-Enter/Space predicate) and the shared 2px focus ring
//! (`mde_egui::focus::paint_focus_ring`) light up for free; a focused search box
//! only surrenders the keyboard to the grid on the first Down/Up (the box eats
//! Left/Right for its own cursor), and Escape from a focused tile closes the
//! menu. Mouse-only behaviour is byte-for-byte unchanged (a grid tile only ever
//! requests focus once an arrow key is pressed). (3) **Per-tile context menu** —
//! each tile now carries a secondary-click menu (`Sense::click()` already senses
//! it) offering **Open** (same as a click) and **Pin/Unpin** (toggles feature
//! 1), the SAME `context_menu_row` shape `dock::paint_surface_context_menu`
//! already uses; the panel's click-away dismissal is suppressed while that menu
//! is open so choosing "Pin" keeps the menu up (only "Open" closes it, via the
//! existing tile-activation seam).
//!
//! **WIN10-HYBRID B6 update:** the launcher grid now wears the accepted
//! taskbar-era Start styling rather than the old rounded panel default:
//! square Start chrome, visible uniform tile bodies, category accent rails,
//! and explicit hover/focus/pinned state. The topology remains the surviving
//! Win7 decision: every tileable [`Surface::ALL`] entry appears once in the
//! grouped grid, with optional pinned copies rendered above it as shortcuts.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use mde_egui::egui;
use mde_egui::{Motion, Style};
use mde_lighthouse_health::LighthouseHealth;
use serde::{Deserialize, Serialize};

use crate::chrome::MeshSummary;
use crate::console::{self, ConsoleState};
#[cfg(test)]
use crate::dock::launcher_group_accent;
use crate::dock::{
    icon_texture, launcher_group_label, response_activated, LauncherGroup, Surface, LAUNCHER_GROUPS,
};
use crate::status::{self, StatusSegments};
use mde_egui::search_omnibox::{ranked_hits, SearchDomain, SearchItem};
use mde_theme::brand::icons::IconId;

// ── geometry ─────────────────────────────────────────────────────────────────

/// The stable id of the Start Menu's floating [`egui::Area`] layer.
const START_MENU_AREA: &str = "start-menu-area";

/// The Start Menu tile grid's bounded scroll region above the fixed search box.
const LEFT_PANE_SCROLL_ID: &str = "start-menu-left-pane-scroll";

/// Per-seat Start Menu preferences, stored beside the shell's other
/// client-data JSON prefs.
const START_MENU_PREFS_FILE: &str = "start-menu.json";

/// The egui memory key for the panel's slide animation (the `console.rs`
/// `SLIDE_KEY` / `dock.rs` `DOCK_SLIDE_KEY` idiom, restated here since the
/// slide/Area machinery now lives in this module instead).
const SLIDE_KEY: &str = "start-menu-slide";

/// A 1px hairline rule (the dock's/console's `HAIRLINE_W` restated —
/// module-private in each, the established per-module idiom).
const HAIRLINE_W: f32 = 1.0;

/// WIN10-HYBRID B6 — Start/taskbar chrome is square. Surfaces still use the
/// shared 4/6/8 radius tiers; the Start menu and launcher-grid affordances do
/// not.
const START_CHROME_RADIUS: f32 = 0.0;

/// The narrow category/state stripe used on headings and tile bodies.
const START_ACCENT_W: f32 = Style::SP_XS / 2.0;

// ── tile grid geometry (WIN7-3, locks #6/#7/#8) ─────────────────────────────

/// One tile's height — `SP_XL + SP_M` (48pt), the SAME cell-height
/// composition `dock.rs`'s own (module-private) `CELL_W` icon-cell token
/// already uses, restated here per this module's own established
/// per-file-restatement idiom (see [`HAIRLINE_W`] above). Every one of the
/// 19 tiles shares this ONE size (lock #6 — no small/wide/large variants).
const TILE_H: f32 = Style::SP_XL + Style::SP_M;

/// One tile's width — `SP_XL · 2.5` (80pt): wider than tall, so a full
/// surface label (e.g. "Infra as Code") has real room beside the shorter
/// ones. Every tile still shares this ONE width (lock #6 rules out
/// per-tile small/wide/large *variants*, not a non-square aspect ratio).
const TILE_W: f32 = Style::SP_XL * 2.5;

/// The gap between adjacent tiles, in both directions of the grid.
const TILE_GAP: f32 = Style::SP_XS;

/// How many tiles sit in one row before wrapping. The widest launcher groups
/// groups (Mesh Control / Media / Web / Developer Tools) has exactly 2-3
/// members, so every one of today's groups renders as a single tidy row —
/// pinned by a test below rather than just assumed. [`left_pane`]'s render
/// loop still wraps generally (N rows, not hardcoded to 1 — `usize::div_ceil`)
/// so a group that later grows past 3 members degrades to a second row
/// instead of silently overlapping.
const TILE_COLUMNS: usize = 3;

/// A tile-group heading's height — matches `console.rs`'s own
/// (module-private) `HEADING_H` exactly (`SP_L`), so the two panes' section
/// labels read as one visual rhythm (Console's heading sits right next to
/// this pane in the same panel).
const GROUP_HEADING_H: f32 = Style::SP_L;

/// The gap after one group's tile row(s), before the next group's heading.
const GROUP_GAP: f32 = Style::SP_XS;

/// The pane's inner padding on every edge — matches `console.rs`'s own
/// `list_pane` `SP_S` inset idiom.
const PANE_PAD: f32 = Style::SP_S;

/// The tile's icon glyph size — the SAME 24px [`Style::SP_L`] the app
/// picker's own cells already draw their glyphs at (one icon language, not a
/// second size invented here).
const TILE_ICON: f32 = Style::SP_L;

/// The status-tint dot's radius ([`tile_status_tint`]'s seam, WIN7-4).
const TILE_STATUS_DOT_R: f32 = Style::SP_XS / 2.0;

/// The left (tile-grid) pane's width: [`PANE_PAD`] on both sides plus three
/// (see [`TILE_COLUMNS`]) [`TILE_W`]-wide columns, [`TILE_GAP`] apart.
/// WIN7-2 shipped this pane at an early fixed 288pt shell width; this is the
/// later live-grid width, derived from the real grid this module renders rather
/// than picked by eye.
/// A test below pins `TILE_COLUMNS == 3` so the `3.0`/`2.0` literals here
/// can't silently drift from the grid they're meant to fit.
const LEFT_PANE_W: f32 = PANE_PAD * 2.0 + TILE_W * 3.0 + TILE_GAP * 2.0;

/// The whole panel's width — the left tile-grid pane plus Console's existing
/// migrated-content width (right pane, locks #4/#10).
const PANEL_W: f32 = LEFT_PANE_W + console::PANEL_W;

/// The panel's height — reuses Console's existing settled height as-is (576pt,
/// already clamped to the screen at mount and already satisfying lock #2's
/// "roughly half-height"); both panes share one height so the panel reads as
/// one unified frame, not two mismatched panels glued together.
const PANEL_H: f32 = console::PANEL_H;

/// The tile grid's total content height when each launcher group fits one tile
/// row. This is verification-only data: rendering uses the scrollable section
/// model below, so the grouped launcher may exceed the visible pane without
/// colliding with the fixed search band.
#[cfg(test)]
const TILE_GRID_CONTENT_H: f32 = TILE_GROUPS.len() as f32 * (GROUP_HEADING_H + TILE_H)
    + (TILE_GROUPS.len() - 1) as f32 * GROUP_GAP;

// ── type-to-launch search (SHELL-UX-3) ──────────────────────────────────────

/// The search field's row height — a single [`Style::SP_L`] (24pt) line that
/// tucks into the left pane's bottom headroom BELOW the 7-group tile grid
/// without overlapping the last tile row (the grid content bottoms out ~8pt
/// above this band; pinned by a test below rather than trusted by eye). Win7's
/// Start Menu puts its search box at exactly this spot — the bottom of the
/// left pane, under the app list.
const SEARCH_H: f32 = Style::SP_L;

/// One search-result row's height — a compact list row (leading icon · label ·
/// dim group name), `SP_L + SP_XS` (28pt). The result list is scroll-bounded
/// because Start search includes both app tiles and Console commands; broad
/// queries can exceed the visible band above the fixed search field.
const RESULT_ROW_H: f32 = Style::SP_L + Style::SP_XS;

/// The search-result row's leading icon size — [`Style::SP_M`] (16px), smaller
/// than a tile's 24px [`TILE_ICON`] glyph because a result row is a list line,
/// not a tile face.
const RESULT_ICON: f32 = Style::SP_M;

/// The search field's leading/clear glyph edge.
const SEARCH_ICON: f32 = Style::SP_M;
/// Visible placeholder copy for the Start search field. Keep it ASCII so shell
/// chrome copy follows the Browser/Start text-glyph cleanup contract.
const START_SEARCH_HINT: &str = "Search apps and commands...";

fn search_rect(left_rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(
            left_rect.left() + PANE_PAD,
            left_rect.bottom() - PANE_PAD - SEARCH_H,
        ),
        egui::vec2((left_rect.width() - PANE_PAD * 2.0).max(0.0), SEARCH_H),
    )
}

fn left_pane_content_rect(left_rect: egui::Rect, search_rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        left_rect.min,
        egui::pos2(left_rect.right(), search_rect.top() - Style::SP_XS),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartSearchHit {
    Surface(Surface),
    Console(console::ConsoleSearchHit),
}

impl StartSearchHit {
    fn icon_id(self) -> IconId {
        match self {
            Self::Surface(surface) => surface.icon_id(),
            Self::Console(hit) => hit.icon,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Surface(surface) => surface.label(),
            Self::Console(hit) => hit.label,
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Surface(surface) => tile_group_label(surface),
            Self::Console(hit) => hit.group,
        }
    }

    const fn kind(self) -> &'static str {
        match self {
            Self::Surface(_) => "App",
            Self::Console(_) => "Console command",
        }
    }
}

// ── live-tile facts (WIN7-4, lock #5) ───────────────────────────────────────

/// How often a multi-fact tile advances to its next fact (lock #5's Win8-style
/// rotation). The design doc's own note calls this "~4-5s per fact... not
/// surveyed precisely, pick a constant that's trivially tunable later" — a
/// single named constant every rotating tile reads, so retuning the pace
/// later is a one-line change here, never a per-call-site guess.
const TILE_FACT_ROTATE_INTERVAL: Duration = Duration::from_secs(5);

/// How often [`start_menu_panel`] asks for a repaint while the panel is
/// settled-open and something is rotating — finer-grained than
/// [`TILE_FACT_ROTATE_INTERVAL`] itself so a rotation step is never more than
/// ~1s stale, without repainting every single frame the way a continuously
/// animated value (e.g. `explorer.rs`'s spinning status ring) would need to.
const TILE_FACT_REPAINT_TICK: Duration = Duration::from_secs(1);

/// The live inputs [`tile_facts`]/[`tile_status_tint`] fold — bundled into ONE
/// [`StartMenuState`] field, refreshed each frame by the shell
/// ([`StartMenuState::set_tile_inputs`]) with the SAME already-published
/// sources an existing dock pip or the surface's own status chip already
/// reads (§7 honest-gating — never a second/fake source of truth). Mirrors
/// `dock.rs`'s `DockState::set_status_inputs`/`StatusInputs` idiom exactly:
/// owned clones/copies, not references, so this struct outlives the frame's
/// borrows of each surface. `pub(crate)` fields (not a constructor) — the
/// `StatusSegments` idiom (`status.rs`) for a plain per-frame data bundle
/// with no invariants to enforce, since `main.rs` builds one fresh every
/// frame from ~14 independent surfaces (a positional constructor would be a
/// worse-than-`StatusInputs`-length unreadable argument list).
#[derive(Debug, Default, Clone)]
pub(crate) struct TileFactInputs {
    /// Chat's unread total — the SAME `self.chat.total_unread()` dock.rs's
    /// OWN Chat badge already reads (`dock::badge_for`).
    pub(crate) chat_unread: usize,
    /// The host whose conversation carries the most recent message
    /// ([`crate::chat::ChatState::most_recent_sender`]) — a different fold of
    /// the SAME live conversation store `chat_unread` sums, not a second read.
    pub(crate) chat_recent_sender: Option<String>,
    /// The mesh summary — the SAME `self.chrome.summary()` dock.rs's OWN
    /// MeshView badge already reads (`dock::badge_for`).
    pub(crate) mesh: MeshSummary,
    /// The daemon segment rollups — the SAME `self.notify_status.segments()`
    /// dock.rs's own bottom rail already reads; Device/Power already route to
    /// `Surface::System` when their pip is clicked (`StatusSegment::route`).
    pub(crate) segments: StatusSegments,
    /// Media's now-playing title, when a track is loaded — the SAME
    /// `mde_media_egui::model::now_playing_title` the lock-screen curtain
    /// already reads for its own now-playing readout.
    pub(crate) media_title: Option<String>,
    /// Whether Media is actively playing (vs. paused) — `self.media.is_playing()`,
    /// the SAME accessor the curtain reads alongside `media_title`.
    pub(crate) media_playing: bool,
    /// Music's now-playing `(title, artist)`, when a track is loaded —
    /// `MusicApp::now_playing()` (added this unit, mirroring
    /// `MediaController::player()`'s established shape).
    pub(crate) music_now_playing: Option<(String, String)>,
    /// Voice's current call-state label, when a call is active — folded via
    /// `mde_voice_hud::sip::CallState::label()`, the SAME formatting the
    /// dialer's own status row already uses (`VoiceApp::call_state()`, added
    /// this unit), gated on that method's own `Idle` → empty-string
    /// convention rather than a second parallel "is there a call" check.
    pub(crate) voice_call_label: Option<String>,
    /// Files' active-transfer count — the SAME `self.files.transfers_counts().active`
    /// dock.rs's OWN Files badge already reads.
    pub(crate) files_active_transfers: usize,
    /// This seat's local Storage node `(disk count, total free MiB)`, once its
    /// mirror has landed — `StorageState::local_summary()` (added this unit),
    /// folded from the SAME `state/storage/<node>` projection the Storage
    /// surface's own panel already renders.
    pub(crate) storage_local: Option<(usize, u64)>,
    /// Bookmarks' live item total — `Manager::total()` (already `pub`,
    /// already Bus-fed via `state/bookmarks/collection`; simply never called
    /// from the shell chrome before this unit).
    pub(crate) bookmarks_total: usize,
    /// Phones' `(paired, online)` device counts —
    /// `PhonesHubState::device_counts()` (added this unit), folded from the
    /// SAME roster `action/connect/devices` reply the Phones panel itself
    /// already renders per-device.
    pub(crate) phones: (usize, usize),
    /// Whether a mesh-status snapshot has been seen at all — gates
    /// `workbench_peer_count`/`workbench_leader` the same way every other
    /// pre-poll-honest field in this bundle gates on its own "seen" bit.
    pub(crate) workbench_seen: bool,
    /// The mesh peer count Workbench's OWN status-chip cluster already shows
    /// — `ControllerState::peer_count()`.
    pub(crate) workbench_peer_count: usize,
    /// The mesh leader, when one is elected — `ControllerState::leader()`,
    /// the SAME source `workbench_peer_count` reads.
    pub(crate) workbench_leader: Option<String>,
    /// The Desktop Chooser's discovered-source count — the SAME
    /// `ChooserState::source_count()` Desktop's own status chip already
    /// shows ("No desktop" / "N sources").
    pub(crate) desktop_sources: usize,
    /// The pending/active Desktop session's `(name, protocol)`, when one
    /// exists — `VdiState::requested_summary()`, the SAME source Desktop's
    /// own status chip already shows for the "connecting…" state.
    pub(crate) desktop_session: Option<(String, &'static str)>,
    /// Infra as Code's `(total, healthy)` cataloged-service counts, once a
    /// catalog has landed — `InfraCodeState::service_summary()` (added this
    /// unit), folded from the SAME outcome the Overview's own status chips
    /// already read.
    pub(crate) infra_services: Option<(usize, usize)>,
    /// The Browser's open-tab count — `WebState::tab_count()` (added this
    /// unit), the SAME `tabs.len()` the accessibility summary already folds.
    pub(crate) browser_tabs: usize,
    /// The Terminal's open-tab count, when the surface has a live terminal —
    /// `TerminalSurface::tab_count()` (added this unit), the SAME already-`pub`
    /// `TabbedTerminal::tab_count()` the standalone binary's own tab strip
    /// already calls.
    pub(crate) terminal_tabs: Option<usize>,
}

// ── tile groups (lock #8: function-based grouping) ──────────────────────────

/// The 8 function-based groups in their locked order (lock #8), each listing
/// its surfaces in [`Surface::ALL`] relative order. The data comes from the
/// shared shell launcher taxonomy so Start and Front Door cannot drift apart.
const TILE_GROUPS: [LauncherGroup; 8] = LAUNCHER_GROUPS;

// Compile-time guard: every `Surface::ALL` entry appears in `TILE_GROUPS`
// exactly once, so a future `Surface` addition that forgets to place a tile
// fails the BUILD, not a silent missing/duplicate tile.
const _: () = {
    let mut i = 0;
    while i < Surface::ALL.len() {
        let target = Surface::ALL[i] as usize;
        let mut count = 0;
        let mut g = 0;
        while g < TILE_GROUPS.len() {
            let surfaces = TILE_GROUPS[g].surfaces;
            let mut s = 0;
            while s < surfaces.len() {
                if surfaces[s] as usize == target {
                    count += 1;
                }
                s += 1;
            }
            g += 1;
        }
        assert!(
            count == 1,
            "every Surface::ALL entry must appear in TILE_GROUPS exactly once (lock #8)",
        );
        i += 1;
    }
};

/// Stable JSON id for a Start-menu-pinnable surface. Kept local to the Start
/// menu so the persisted preferences do not depend on display labels.
fn surface_wire_id(surface: Surface) -> &'static str {
    match surface {
        Surface::Workbench => "workbench",
        Surface::MeshView => "mesh_view",
        Surface::Explorer => "explorer",
        Surface::Desktop => "desktop",
        Surface::InfraCode => "infra_code",
        Surface::Music => "music",
        Surface::Media => "media",
        Surface::Files => "files",
        Surface::Voice => "voice",
        Surface::Browser => "browser",
        Surface::Bookmarks => "bookmarks",
        Surface::MapsLocation => "maps_location",
        Surface::Terminal => "terminal",
        Surface::Editor => "editor",
        Surface::Chat => "chat",
        Surface::Phones => "phones",
        Surface::System => "system",
        Surface::Storage => "storage",
        Surface::About => "about",
        Surface::Timers => "timers",
    }
}

/// Parse the stable Start-menu pin id. Unknown ids are treated as drifted /
/// hand-edited data and ignored by the loader.
fn surface_from_wire_id(id: &str) -> Option<Surface> {
    match id.trim() {
        "workbench" => Some(Surface::Workbench),
        "mesh_view" => Some(Surface::MeshView),
        "explorer" => Some(Surface::Explorer),
        "desktop" => Some(Surface::Desktop),
        "infra_code" => Some(Surface::InfraCode),
        "music" => Some(Surface::Music),
        "media" => Some(Surface::Media),
        "files" => Some(Surface::Files),
        "voice" => Some(Surface::Voice),
        "browser" => Some(Surface::Browser),
        "bookmarks" => Some(Surface::Bookmarks),
        "maps_location" => Some(Surface::MapsLocation),
        "terminal" => Some(Surface::Terminal),
        "editor" => Some(Surface::Editor),
        "chat" => Some(Surface::Chat),
        "phones" => Some(Surface::Phones),
        "system" => Some(Surface::System),
        "storage" => Some(Surface::Storage),
        "about" => Some(Surface::About),
        "timers" => Some(Surface::Timers),
        _ => None,
    }
}

fn tileable_surface(surface: Surface) -> bool {
    Surface::ALL.contains(&surface)
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
struct StartMenuPrefs {
    #[serde(default)]
    pinned: Vec<String>,
}

impl StartMenuPrefs {
    fn from_pins(pinned: &[Surface]) -> Self {
        let mut out = Vec::new();
        for &surface in pinned {
            if tileable_surface(surface) && !out.iter().any(|id| id == surface_wire_id(surface)) {
                out.push(surface_wire_id(surface).to_string());
            }
        }
        Self { pinned: out }
    }

    fn into_pins(self) -> Vec<Surface> {
        let mut out = Vec::new();
        for id in self.pinned {
            let Some(surface) = surface_from_wire_id(&id) else {
                continue;
            };
            if tileable_surface(surface) && !out.contains(&surface) {
                out.push(surface);
            }
        }
        out
    }

    fn default_path() -> Option<PathBuf> {
        mde_bus::client_data_dir().map(|d| d.join(START_MENU_PREFS_FILE))
    }

    fn load_from(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .unwrap_or_default()
    }

    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

// ── state ────────────────────────────────────────────────────────────────────

/// The Start Menu's cross-frame state: the open latch (driven by the Start
/// cell click and the Super key, lock #13), the same-frame click-away guard
/// (the Console/VDOCK-4 `just_toggled` idiom, restated here since this panel
/// now owns its own outer `Area`/dismiss machinery), and (WIN7-3) a pending
/// tile-click surface activation. Pure (no egui handles), so open/close and
/// tile activation are unit-tested without a GPU. WIN7-4 (tile rotation) and
/// WIN7-8 (multi-seat sync) are what grow this further.
#[derive(Debug)]
pub struct StartMenuState {
    /// Where persisted Start Menu preferences are stored. `None` keeps the
    /// state purely in-memory, which is what the existing render tests want.
    prefs_path: Option<PathBuf>,
    /// Whether the panel is up — toggled by a Start-cell click or a clean
    /// Super tap (lock #13); the single source of truth `main.rs` mirrors into
    /// [`ConsoleState`] each frame ([`console::ConsoleState::set_open`]).
    open: bool,
    /// Set on any edge (open or close) and cleared at the end of the panel's
    /// own frame — the same-frame click-away guard: the very click/key that
    /// opened the panel must not immediately read as a click-away dismissal
    /// (the Console/VDOCK-4 `just_toggled` idiom).
    just_toggled: bool,
    /// WIN7-3 — a live-tile click's pending surface activation (lock #23):
    /// set by [`left_pane`]/[`tile`] when a tile fires, drained once by
    /// `main.rs` ([`Self::take_tile_activation`]) exactly like
    /// [`console::ConsoleState::take_request`]'s `Goto` variant — the SAME
    /// "go to this surface, close the menu" outcome as an embedded Console
    /// row, just raised from the OTHER pane's data.
    tile_activation: Option<Surface>,
    /// WIN7-4 — the live-tile fact sources, mirrored in each frame by the
    /// shell ([`Self::set_tile_inputs`], the `DockState::set_status_inputs`
    /// idiom) so [`start_menu_panel`]'s `(ctx, state, console)` signature
    /// stays put. Defaults to the honest pre-poll-empty bundle — every field
    /// gates on its own "seen"/`Option` bit, so a not-yet-refreshed bundle
    /// renders every tile at its plain static label, never a fake rotation.
    tile_inputs: TileFactInputs,
    /// SHELL-UX-3 — the live type-to-launch query. Empty = the normal grouped
    /// tile grid (no behaviour change); non-empty = the left pane shows a
    /// ranked flat list of the tiles whose label (or group name) matches
    /// ([`search_matches`]). The bound buffer of the bottom-of-left-pane
    /// [`egui::TextEdit`] ([`search_field`]). Cleared on close so reopening
    /// the menu always starts fresh (the `console.rs` "close resets" posture).
    search_query: String,
    /// SHELL-UX-3 — which entry of the current [`search_matches`] list carries
    /// the keyboard highlight (Up/Down move it, Enter launches it). An index
    /// into the *filtered* list, always clamped to its length at use so a
    /// shrinking result set can never point past the end; reset to the top
    /// match (`0`) whenever the query text changes.
    search_highlight: usize,
    /// SHELL-UX-3 — a one-shot request to focus the search box on the next
    /// render (the `explorer.rs`/`chooser.rs` `focus_pending` idiom): set when
    /// the menu opens (so "open, then type" filters live) and when Esc clears
    /// a live query (so the emptied box keeps the keyboard), consumed once by
    /// [`search_field`]'s `request_focus`.
    search_focus_pending: bool,
    /// SM-QOL-1 — the operator's pinned/favourite surfaces, rendered as a
    /// "Pinned" section at the TOP of the left pane (above the function
    /// groups). A [`Vec`]-as-ordered-set (pin order is the render order); the
    /// no-duplicate invariant is held by the ONE mutator [`toggle_pin`], driven
    /// from a tile's right-click context menu ([`tile_context_menu`]). Empty by
    /// default, so an operator who never pins sees exactly the WIN7-3 grid (zero
    /// behaviour change) — the section only appears once something is pinned.
    /// Persisted when the real shell constructs this state with [`Self::load`];
    /// the plain [`Default`] constructor remains in-memory for unit fixtures.
    pinned: Vec<Surface>,
}

impl Default for StartMenuState {
    fn default() -> Self {
        Self {
            prefs_path: None,
            open: false,
            just_toggled: false,
            tile_activation: None,
            tile_inputs: TileFactInputs::default(),
            search_query: String::new(),
            search_highlight: 0,
            search_focus_pending: false,
            pinned: Vec::new(),
        }
    }
}

impl StartMenuState {
    /// Load the real shell's persisted Start Menu preferences from client data.
    /// Missing or malformed files fold to an empty in-memory-compatible state.
    pub(crate) fn load() -> Self {
        let prefs_path = StartMenuPrefs::default_path();
        let pinned = prefs_path
            .as_deref()
            .map(StartMenuPrefs::load_from)
            .unwrap_or_default()
            .into_pins();
        Self {
            prefs_path,
            pinned,
            ..Self::default()
        }
    }

    #[cfg(test)]
    fn load_from(path: PathBuf) -> Self {
        let pinned = StartMenuPrefs::load_from(&path).into_pins();
        Self {
            prefs_path: Some(path),
            pinned,
            ..Self::default()
        }
    }

    fn persist_pins(&self) {
        if let Some(path) = &self.prefs_path {
            let _ = StartMenuPrefs::from_pins(&self.pinned).save_to(path);
        }
    }

    /// Whether the panel is up.
    pub(crate) const fn is_open(&self) -> bool {
        self.open
    }

    /// Ordered operator favourites for launcher surfaces. The Front Door reads
    /// this as a display priority only; Start remains the owner of pin mutation
    /// and persistence.
    pub(crate) fn pinned_surfaces(&self) -> &[Surface] {
        &self.pinned
    }

    /// Toggle a launcher surface in the same persisted favorites set used by
    /// the Start tile context menu. Front Door uses this seam so the platform
    /// has one local pin store, not two diverging preference files.
    pub(crate) fn toggle_surface_pin(&mut self, surface: Surface) -> bool {
        let before = self.pinned.clone();
        toggle_pin(&mut self.pinned, surface);
        let changed = self.pinned != before;
        if changed {
            self.persist_pins();
        }
        changed
    }

    pub(crate) fn move_surface_pin_up(&mut self, surface: Surface) -> bool {
        let Some(idx) = self
            .pinned
            .iter()
            .position(|&candidate| candidate == surface)
        else {
            return false;
        };
        if idx == 0 {
            return false;
        }
        self.pinned.swap(idx - 1, idx);
        self.persist_pins();
        true
    }

    pub(crate) fn move_surface_pin_down(&mut self, surface: Surface) -> bool {
        let Some(idx) = self
            .pinned
            .iter()
            .position(|&candidate| candidate == surface)
        else {
            return false;
        };
        if idx + 1 >= self.pinned.len() {
            return false;
        }
        self.pinned.swap(idx, idx + 1);
        self.persist_pins();
        true
    }

    /// WIN7-4 — refresh the live-tile fact inputs for this frame (the
    /// `DockState::set_status_inputs` idiom): `main.rs` calls this each frame
    /// before [`start_menu_panel`] with the SAME already-published sources an
    /// existing dock pip or the surface's own status chip already reads (§7 —
    /// see [`TileFactInputs`]'s own field docs for each source).
    pub(crate) fn set_tile_inputs(&mut self, inputs: TileFactInputs) {
        self.tile_inputs = inputs;
    }

    /// Toggle the panel open/closed — the Start-cell click and the Super-tap
    /// hotkey path both drain into this (lock #13, "both, not either/or").
    /// SHELL-UX-3: opening arms the search box's auto-focus (so "open, then
    /// type" filters live); closing wipes any live query so the next open
    /// starts on the full grid, not a stale filter.
    pub(crate) fn toggle(&mut self) {
        self.open = !self.open;
        self.just_toggled = true;
        if self.open {
            self.search_focus_pending = true;
        } else {
            self.clear_search();
        }
    }

    /// SHELL-UX-3 — drop any live query + highlight (called on every close and
    /// when Esc dismisses a live search); leaves `search_focus_pending`
    /// untouched so an Esc-clear can re-arm the emptied box's focus separately.
    fn clear_search(&mut self) {
        self.search_query.clear();
        self.search_highlight = 0;
    }

    /// Close the panel (Esc / click-away / an embedded Console action closing
    /// itself / WIN7-6's Critical edge-cue firing, lock #9). A no-op while
    /// already closed, so a redundant close (e.g. Esc racing the embedded
    /// content's own Esc-close, or the cue firing while the menu is already
    /// shut, see the module doc) never re-arms the click-away guard for no
    /// reason. Widened `pub(crate)` (the `dock::response_activated`/
    /// `status::severity_color` cross-module-widening idiom already used in
    /// this module) so `main.rs` can call it directly off
    /// `CriticalEdgeCue::take_became_visible`'s one-shot edge — a firing
    /// closes an open menu exactly once and never re-fights an operator who
    /// reopens it afterward.
    pub(crate) fn close(&mut self) {
        if self.open {
            self.open = false;
            self.just_toggled = true;
            self.clear_search();
        }
    }

    /// Drain a pending tile-click surface activation (WIN7-3, lock #23) —
    /// `main.rs` calls this each frame after [`start_menu_panel`] and routes
    /// `nav.surface` exactly as it already does for an embedded Console
    /// `Goto` request (the SAME deferred-wire idiom, §6 — this panel can't
    /// reach the shell nav itself). `None` (drained once) otherwise. `const`
    /// matching [`console::ConsoleState::take_request`]'s identical
    /// `self.pending.take()` shape.
    pub(crate) const fn take_tile_activation(&mut self) -> Option<Surface> {
        self.tile_activation.take()
    }
}

// ── render ───────────────────────────────────────────────────────────────────

/// Mount the Start Menu for this frame: a fixed-size two-pane panel (lock #2)
/// sliding up from the bottom edge, anchored to the screen's true left edge
/// (lock #4's bottom-left footprint, updated for the retired left dock).
/// Fully hidden + settled it mounts **no layer at all** (the dock/console
/// passthrough guarantee), so a closed Start Menu steals no input from the
/// surface beneath — and even open, it only claims its own footprint (it
/// overlays, never hides/replaces the active surface behind it). Esc, a click
/// away, and a second trigger all dismiss; so does an embedded Console action
/// that fires for real (a routed link, a spawned tab, a power verb — see the
/// module doc's self-closure note), and so (WIN7-3) does a live-tile click
/// (lock #23) — both panes close the WHOLE menu on activation, just via
/// different data ([`ConsoleState::is_open`]'s self-closure vs.
/// [`StartMenuState::tile_activation`]).
///
/// `rail_h` is the live bottom-taskbar height ([`crate::dock::DockState::
/// rail_height`]) the panel reserves above itself (the WIN7-DESKTOP-1
/// regression fix, `docs/WORKLIST.md`) so its Power-anchored bottom (lock
/// #11) sits flush ABOVE the taskbar rather than underneath/behind it — a
/// true Win7 Start Menu never overlaps the taskbar (lock #1). Callers with no
/// taskbar in the fixture (most of this module's own tests) pass `0.0`.
#[allow(clippy::suboptimal_flops)] // the slide offset reads clearer than mul_add
pub fn start_menu_panel(
    ctx: &egui::Context,
    state: &mut StartMenuState,
    console: &mut ConsoleState,
    rail_h: f32,
) {
    let t = Motion::animate(ctx, SLIDE_KEY, state.open, Motion::BASE);
    if t <= 0.001 {
        state.just_toggled = false;
        return;
    }

    // Mirror the ONE source of truth into Console before it renders (the
    // `DockState::set_active` idiom) so its focus ring / `handle_keys` read a
    // meaningful "am I showing" bit even though it no longer self-toggles.
    console.set_open(state.open);

    let screen = ctx.screen_rect();
    // `rail_h` (`DockState::rail_height()`, live) reserves the bottom
    // taskbar's own band: a true Win7 Start Menu sits flush ABOVE the
    // taskbar, never overlapping it (design lock #1's "true Win7 bottom
    // taskbar"). Before this fix `panel_h`/`top` ignored the rail entirely —
    // a latent WIN7-DESKTOP-1 regression (see `docs/WORKLIST.md`) invisible
    // only because the taskbar itself was mispositioned at the time; once it
    // renders where it belongs this panel's Power-anchored bottom (lock #11)
    // would sit right underneath it without this reservation.
    let panel_h = PANEL_H.min(screen.height() - rail_h - Style::SP_XL);
    // The slide-up: the panel's top rides from the taskbar's top edge (t=0)
    // to its settled height above it (t=1) — the console.rs precedent,
    // restated here since the Area now lives in this module.
    let top = screen.bottom() - rail_h - t * panel_h;

    let area = egui::Area::new(egui::Id::new(START_MENU_AREA))
        .order(egui::Order::Foreground)
        .fade_in(false)
        .constrain(false)
        .fixed_pos(egui::pos2(screen.left(), top))
        .show(ctx, |ui| {
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(PANEL_W, panel_h), egui::Sense::hover());
            paint_frame(ui, rect);
            let left_rect =
                egui::Rect::from_min_size(rect.min, egui::vec2(LEFT_PANE_W, rect.height()));
            let right_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left() + LEFT_PANE_W, rect.top()),
                rect.max,
            );
            install_accessibility(ui.ctx(), rect, left_rect, right_rect);
            // WIN7-7, lock #14 — the OPEN/CLOSE transition itself needs an
            // announcement, not just the `Role::Menu` landmark above: a
            // screen reader user needs to know the menu just opened, not
            // discover it only by happening to explore the tree afterward.
            // Runs every frame this closure does (i.e. whenever `t > 0.001`
            // — opening, settled open, OR still mid-close-tween), keyed off
            // `state.open` rather than `t` itself: the value stays constant
            // for as long as the menu is steadily open (or steadily
            // closing), so — matching how `install_status_accessibility`/
            // `install_tiles_live_summary` already behave every frame their
            // own condition holds — an AT only re-announces on the actual
            // edge (a genuine value change), never once per frame. No node
            // at all once fully closed and settled (`t` reaches ~0 and this
            // whole closure stops running for the frame) — nothing left to
            // announce, the same honest-silence posture `install_tiles_live_summary`
            // already uses.
            install_start_menu_state_announcement(ui.ctx(), rect, state.open);
            // SHELL-UX-3 — type-to-launch search. The box lives at the bottom
            // of the left pane (Win7's own spot); the tile grid renders above
            // it untouched when the query is empty, and a ranked flat result
            // list replaces the grid the moment anything is typed. Returns
            // whether the search consumed this frame's Esc (a live query's
            // Esc clears the query, it must NOT be read as a menu dismissal —
            // see the self-closure guard below).
            let (search_ate_escape, menu_open) = start_menu_search(ui, left_rect, state, console);
            console::console_content(ui, right_rect, console);
            (search_ate_escape, menu_open)
        });
    let (search_ate_escape, menu_open) = area.inner;

    // An embedded Console action fired for real this frame (a routed link, a
    // spawned tab, a power verb) and already called `ConsoleState::close`
    // (unchanged console.rs behaviour) — propagate that self-closure to the
    // WHOLE panel so launching anything still closes the menu, matching the
    // pre-WIN7-2 behaviour (the module doc's self-closure note).
    //
    // SHELL-UX-3 guard: when a live search swallowed this frame's Esc to clear
    // its query, egui had already surrendered the focused search box (its
    // default filter lets Esc blur it), so the embedded Console — inert only
    // *while* a text field owns the keyboard — briefly saw that same Esc and
    // self-closed. That is a one-frame artifact of the shared Esc, not a real
    // Console dismissal: skip the propagation this frame and next frame's
    // `console.set_open(state.open)` mirror restores it, so clearing a query
    // keeps the whole menu up (a SECOND, now-empty Esc is what closes it).
    if state.open && !console.is_open() && !search_ate_escape {
        state.close();
    }

    // Click-away dismissal — but never on the very frame the trigger opened it
    // (that click/key lands outside the panel and must not self-dismiss; the
    // Console/VDOCK-4 `just_toggled` guard). SM-QOL-1: also never while a tile's
    // own right-click context menu is open — that menu is a separate popup
    // Area, so clicking one of its rows (e.g. "Pin to top") lands OUTSIDE this
    // panel's rect and would otherwise read as a click-away and dismiss the
    // whole menu. Choosing "Open" still closes it, but via the tile-activation
    // seam (a real launch), not this click-away path.
    if state.open && !state.just_toggled && !menu_open && area.response.clicked_elsewhere() {
        state.close();
    }
    state.just_toggled = false;

    // Keep frames flowing while the slide is in flight (the dock/console tween
    // idiom).
    if t > 0.001 && t < 0.999 {
        ctx.request_repaint();
    }
    // WIN7-4 — once settled open, keep a coarser heartbeat alive so a
    // rotating tile's next fact actually paints without waiting on incidental
    // input (a mouse move, another animation) to trigger the next frame. Only
    // while open: a closed-but-not-yet-unmounted panel (t between the slide's
    // endpoints on the way down) is already covered by the tween repaint
    // above, and there is nothing to rotate once fully closed.
    if state.open && t >= 0.999 && ctx.style().animation_time > f32::EPSILON {
        ctx.request_repaint_after(TILE_FACT_REPAINT_TICK);
    }
}

/// The panel's outer chrome: the solid SURFACE sheet, the outer hairline, and
/// the left|right pane divider (§4 tokens) — the frame `console.rs`'s old
/// standalone panel used to paint for itself; this module owns it now since
/// it's the outer panel, and the embedded [`console::console_content`] paints
/// only its OWN inner rail|list divider (no doubled-up border).
fn paint_frame(ui: &egui::Ui, rect: egui::Rect) {
    let painter = ui.painter().clone();
    painter.rect_filled(rect, START_CHROME_RADIUS, Style::SURFACE);
    painter.rect_stroke(
        rect,
        START_CHROME_RADIUS,
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
        egui::StrokeKind::Inside,
    );
    painter.vline(
        rect.left() + LEFT_PANE_W,
        (rect.top() + Style::SP_XS)..=(rect.bottom() - Style::SP_XS),
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
    );
}

/// One addressable cell in the keyboard-nav grid (SM-QOL-1): a surface plus
/// whether it renders in the top **Pinned** section vs. one of the function
/// groups. The flag matters because a pinned surface ALSO still appears in its
/// group below, so the two copies must carry DISTINCT interact/accesskit ids
/// ([`nav_cell_id`]/[`tile_accesskit_id`]) — an egui id collision otherwise
/// confuses interaction + focus between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NavCell {
    surface: Surface,
    pinned: bool,
}

/// One rendered section of the left pane (SM-QOL-1): a heading plus its tiles
/// chunked into [`TILE_COLUMNS`]-wide rows. The optional Pinned section leads,
/// then the [`TILE_GROUPS`]. Built fresh each frame from the live pin set so
/// the layout and the arrow-nav row math ([`nav_rows`]) can never diverge.
struct NavSection {
    heading: &'static str,
    accent: egui::Color32,
    pinned: bool,
    rows: Vec<Vec<Surface>>,
}

/// The pane's ordered sections for this frame: the Pinned section first (only
/// when something is pinned — an empty pin set renders EXACTLY the WIN7-3 grid,
/// zero behaviour change), then the locked [`TILE_GROUPS`]. Each section's
/// surfaces are chunked into [`TILE_COLUMNS`]-wide rows (today every group is a
/// single row, but chunking keeps a future >3-member group — or a >3 pin set —
/// wrapping to a second row instead of overlapping).
fn nav_sections(pinned: &[Surface]) -> Vec<NavSection> {
    let mut sections = Vec::new();
    if !pinned.is_empty() {
        sections.push(NavSection {
            heading: "Pinned",
            accent: Style::ACCENT,
            pinned: true,
            rows: pinned
                .chunks(TILE_COLUMNS)
                .map(<[Surface]>::to_vec)
                .collect(),
        });
    }
    for group in &TILE_GROUPS {
        sections.push(NavSection {
            heading: group.label,
            accent: group.accent,
            pinned: false,
            rows: group
                .surfaces
                .chunks(TILE_COLUMNS)
                .map(<[Surface]>::to_vec)
                .collect(),
        });
    }
    sections
}

fn nav_sections_content_h(sections: &[NavSection]) -> f32 {
    let mut h = PANE_PAD;
    for section in sections {
        let rows_n = section.rows.len().max(1);
        h += GROUP_HEADING_H;
        h += rows_n as f32 * (TILE_H + TILE_GAP) - TILE_GAP;
        h += GROUP_GAP;
    }
    h
}

/// Flatten [`nav_sections`] into the top-to-bottom rows of [`NavCell`]s the
/// arrow-key math walks — sections concatenated in render order, each carrying
/// its own Pinned-vs-group id namespace. `(row, col)` into this is the ONE
/// address space both [`drive_grid_focus`] and the render loop agree on.
fn nav_rows(sections: &[NavSection]) -> Vec<Vec<NavCell>> {
    let mut rows = Vec::new();
    for section in sections {
        for row in &section.rows {
            rows.push(
                row.iter()
                    .map(|&surface| NavCell {
                        surface,
                        pinned: section.pinned,
                    })
                    .collect(),
            );
        }
    }
    rows
}

/// The stable interact id of a nav cell — namespaced by section so a pinned
/// copy and its group copy never collide ([`pinned_tile_id`] vs. [`tile_id`]).
fn nav_cell_id(cell: NavCell) -> egui::Id {
    if cell.pinned {
        pinned_tile_id(cell.surface)
    } else {
        tile_id(cell.surface)
    }
}

/// A grid movement direction (SM-QOL-1 arrow-key nav).
#[derive(Debug, Clone, Copy)]
enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// The neighbouring `(row, col)` of `(r, c)` in `rows` for `dir`, or `None` at
/// an edge (the move clamps rather than wraps). Left/Right flow across row
/// boundaries (end of a row → start of the next), respecting each row's real
/// width (≤ [`TILE_COLUMNS`], and the Pinned/group rows can differ); Up/Down
/// move one row and clamp the column to the target row's width so a jump from a
/// full 3-wide row into a 1-wide group lands on that group's only tile.
fn grid_neighbor(rows: &[Vec<NavCell>], r: usize, c: usize, dir: Dir) -> Option<(usize, usize)> {
    match dir {
        Dir::Right => {
            if c + 1 < rows[r].len() {
                Some((r, c + 1))
            } else if r + 1 < rows.len() {
                Some((r + 1, 0))
            } else {
                None
            }
        }
        Dir::Left => {
            if c > 0 {
                Some((r, c - 1))
            } else if r > 0 {
                Some((r - 1, rows[r - 1].len() - 1))
            } else {
                None
            }
        }
        Dir::Down => (r + 1 < rows.len()).then(|| (r + 1, c.min(rows[r + 1].len() - 1))),
        Dir::Up => (r > 0).then(|| (r - 1, c.min(rows[r - 1].len() - 1))),
    }
}

/// Which grid cell currently holds egui keyboard focus, if any — reads
/// `memory.focused()` (last frame's resolved focus) and locates it in `rows` by
/// [`nav_cell_id`]. `None` while the search box (or nothing) is focused.
fn focused_grid_cell(ui: &egui::Ui, rows: &[Vec<NavCell>]) -> Option<(usize, usize)> {
    let focused = ui.ctx().memory(egui::Memory::focused)?;
    for (r, row) in rows.iter().enumerate() {
        for (c, &cell) in row.iter().enumerate() {
            if nav_cell_id(cell) == focused {
                return Some((r, c));
            }
        }
    }
    None
}

/// Consume every arrow-key press left in this frame's event queue — the
/// `dock::apply_picker_arrow_focus` idiom: `key_pressed` does NOT consume, so
/// without this a widget rendered later the same frame could see the SAME still
/// pressed arrow and move focus again (a one-key cascade through the grid).
fn consume_grid_arrows(ui: &egui::Ui) {
    ui.input_mut(|i| {
        i.events.retain(|ev| {
            !matches!(
                ev,
                egui::Event::Key { key, pressed: true, .. }
                    if matches!(
                        key,
                        egui::Key::ArrowDown
                            | egui::Key::ArrowRight
                            | egui::Key::ArrowUp
                            | egui::Key::ArrowLeft
                    )
            )
        });
    });
}

/// Drive the tile grid's keyboard focus for this frame (SM-QOL-1, feature 2) —
/// the `dock::apply_picker_arrow_focus` idiom raised to a 2-D grid + the pinned
/// section, called ONCE before the tiles render (so the newly-focused tile
/// shows its ring + answers `has_focus()` this same frame). Two cases:
///
/// * **No grid cell focused** (the search box owns the keyboard, or nothing
///   does): the FIRST Down/Up hands the keyboard to the grid at its first cell.
///   Only Down/Up can enter — a focused single-line search box eats Left/Right
///   for its own cursor, so those never reach us until the box has yielded.
/// * **A grid cell is focused**: lock the arrows to it (so egui's own
///   directional focus doesn't ALSO move it), then move to the [`grid_neighbor`]
///   in the pressed direction and consume the arrow so no later widget re-reads
///   it. Once the grid owns focus the box is blurred, so all four arrows flow.
fn drive_grid_focus(ui: &egui::Ui, rows: &[Vec<NavCell>]) {
    if rows.is_empty() {
        return;
    }
    let (up, down, left, right) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::ArrowLeft),
            i.key_pressed(egui::Key::ArrowRight),
        )
    });
    match focused_grid_cell(ui, rows) {
        None => {
            if up || down {
                ui.ctx()
                    .memory_mut(|m| m.request_focus(nav_cell_id(rows[0][0])));
                consume_grid_arrows(ui);
            }
        }
        Some((r, c)) => {
            ui.ctx().memory_mut(|m| {
                m.set_focus_lock_filter(
                    nav_cell_id(rows[r][c]),
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..egui::EventFilter::default()
                    },
                );
            });
            let dir = if right {
                Some(Dir::Right)
            } else if left {
                Some(Dir::Left)
            } else if down {
                Some(Dir::Down)
            } else if up {
                Some(Dir::Up)
            } else {
                None
            };
            if let Some((nr, nc)) = dir.and_then(|d| grid_neighbor(rows, r, c, d)) {
                ui.ctx()
                    .memory_mut(|m| m.request_focus(nav_cell_id(rows[nr][nc])));
                consume_grid_arrows(ui);
            }
        }
    }
}

/// The outcome of one [`left_pane`] frame (SM-QOL-1): which tile the operator
/// launched (a click, an Enter/Space on the focused tile, or a context-menu
/// "Open"), whether Escape from a focused tile asked to close the whole menu,
/// and whether a tile's right-click context menu is open (so the caller can
/// suppress its click-away dismissal — see [`start_menu_panel`]).
#[derive(Debug, Default, Clone, Copy)]
struct LeftPaneOutcome {
    activated: Option<Surface>,
    closed: bool,
    menu_open: bool,
}

/// The left pane (WIN7-3, locks #6/#7/#8; WIN7-4, lock #5; SM-QOL-1): the
/// optional **Pinned** section then [`TILE_GROUPS`]' headed sections, each a
/// row of uniform [`TILE_W`]×[`TILE_H`] tiles. Drives the grid's arrow-key
/// keyboard focus ([`drive_grid_focus`]) before rendering, and applies a
/// right-click Pin/Unpin AFTER the render loop (so the frame's read-only pin
/// snapshot the layout was built from is already released). Reads the ONE frame
/// clock (`ui.input(|i| i.time)`) once and threads it to every tile so every
/// rotating tile advances in lockstep. `pinned` is `&mut` only so the deferred
/// toggle can land; the layout itself reads a cheap [`Copy`]-element snapshot.
#[allow(
    clippy::cast_precision_loss, // row/col indices are tiny (< TILE_COLUMNS)
    clippy::suboptimal_flops     // layout arithmetic reads clearer than mul_add
)]
fn left_pane(
    ui: &egui::Ui,
    rect: egui::Rect,
    inputs: &TileFactInputs,
    pinned: &mut Vec<Surface>,
) -> LeftPaneOutcome {
    let time_secs = ui.input(|i| i.time);
    let rotation_enabled = live_tile_rotation_enabled(ui);
    install_tiles_live_summary(ui.ctx(), rect, inputs, time_secs, rotation_enabled);

    let snapshot = pinned.clone();
    let sections = nav_sections(&snapshot);
    let rows = nav_rows(&sections);
    drive_grid_focus(ui, &rows);

    // Escape while a grid tile holds focus closes the whole menu (feature 2) —
    // the `esc_pressed` path can't, because a focused tile makes
    // `memory.focused()` non-empty. Read after `drive_grid_focus` so an entry
    // frame (which just took focus for the grid) is already "a tile is focused".
    let closed =
        focused_grid_cell(ui, &rows).is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape));

    let mut activated = None;
    let mut toggle = None;
    let mut menu_open = false;
    let x0 = rect.left() + PANE_PAD;
    let mut y = rect.top() + PANE_PAD;
    for section in &sections {
        let heading_rect = egui::Rect::from_min_size(
            egui::pos2(x0, y),
            egui::vec2((rect.width() - PANE_PAD * 2.0).max(0.0), GROUP_HEADING_H),
        );
        tile_group_heading(ui, heading_rect, section.heading, section.accent);
        y += GROUP_HEADING_H;

        for (row_idx, row) in section.rows.iter().enumerate() {
            for (col, &surface) in row.iter().enumerate() {
                let tile_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        x0 + col as f32 * (TILE_W + TILE_GAP),
                        y + row_idx as f32 * (TILE_H + TILE_GAP),
                    ),
                    egui::vec2(TILE_W, TILE_H),
                );
                let cell = NavCell {
                    surface,
                    pinned: section.pinned,
                };
                let facts = tile_facts(surface, inputs);
                let tint = tile_status_tint(surface, inputs);
                let is_pinned = section.pinned || snapshot.contains(&surface);
                let out = tile(
                    ui,
                    cell,
                    tile_rect,
                    &facts,
                    tint,
                    time_secs,
                    rotation_enabled,
                    is_pinned,
                    section.accent,
                );
                match out.action {
                    TileAction::Activate => activated = Some(surface),
                    TileAction::TogglePin => toggle = Some(surface),
                    TileAction::None => {}
                }
                menu_open |= out.menu_open;
            }
        }
        let rows_n = section.rows.len().max(1);
        y += rows_n as f32 * (TILE_H + TILE_GAP) - TILE_GAP + GROUP_GAP;
    }

    if let Some(surface) = toggle {
        toggle_pin(pinned, surface);
    }
    LeftPaneOutcome {
        activated,
        closed,
        menu_open,
    }
}

fn scrollable_left_pane(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    inputs: &TileFactInputs,
    pinned: &mut Vec<Surface>,
) -> LeftPaneOutcome {
    let sections = nav_sections(pinned);
    let content_h = nav_sections_content_h(&sections).max(rect.height());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(rect);
    egui::ScrollArea::vertical()
        .id_salt(LEFT_PANE_SCROLL_ID)
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            let pane_rect = egui::Rect::from_min_size(
                ui.next_widget_position(),
                egui::vec2(rect.width(), content_h),
            );
            let out = left_pane(ui, pane_rect, inputs, pinned);
            ui.allocate_rect(pane_rect, egui::Sense::hover());
            out
        })
        .inner
}

/// Add or remove `surface` from the pin set (SM-QOL-1) — the ONE mutator, so
/// the no-duplicate invariant lives in a single place. Pinning appends (pin
/// order = render order); unpinning removes in place, preserving the rest.
fn toggle_pin(pinned: &mut Vec<Surface>, surface: Surface) {
    if !tileable_surface(surface) {
        return;
    }
    if let Some(idx) = pinned.iter().position(|&s| s == surface) {
        pinned.remove(idx);
    } else {
        pinned.push(surface);
    }
}

/// One tile-group heading — B6 keeps the compact uppercase rhythm but adds a
/// category rail so the grouped launcher reads as organized sections, not a
/// flat wall of icons.
fn tile_group_heading(ui: &egui::Ui, rect: egui::Rect, label: &str, accent: egui::Color32) {
    let painter = ui.painter();
    painter.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - Style::SP_XS / 2.0),
            egui::vec2(START_ACCENT_W, Style::SP_XS),
        ),
        START_CHROME_RADIUS,
        accent,
    );
    painter.text(
        egui::pos2(rect.left() + Style::SP_S, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label.to_uppercase(),
        egui::FontId::proportional(Style::SMALL),
        accent,
    );
}

/// What a tile asked for this frame (SM-QOL-1): nothing, launch the surface (a
/// click, an Enter/Space on the focused tile, or the context-menu "Open"), or
/// toggle the surface's pin membership (the context-menu "Pin"/"Unpin").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TileAction {
    None,
    Activate,
    TogglePin,
}

/// One tile's per-frame result: its [`TileAction`] plus whether its right-click
/// context menu is open (so [`left_pane`] can aggregate it up to
/// [`start_menu_panel`]'s click-away guard).
#[derive(Debug, Clone, Copy)]
struct TileOutcome {
    action: TileAction,
    menu_open: bool,
}

/// One live tile (WIN7-3, locks #6/#8/#23; WIN7-4, lock #5; SM-QOL-1): a uniform
/// [`TILE_W`]×[`TILE_H`] cell wearing the surface's existing picker glyph
/// (`Surface::icon_id`, the SAME [`icon_texture`] loader + the SAME 24px
/// [`TILE_ICON`] size the app picker's own cells use) over its label slot,
/// which now shows [`tile_display_text`]'s fold of `facts` instead of always
/// `Surface::label()` (WIN7-3's own static behaviour, still exactly what
/// renders when `facts` is empty). A hover OR keyboard focus brightens both the
/// fill and the tint — the same two-tone contract the app picker's own cells
/// already use (§4). A focused tile also wears the shared 2px focus ring
/// (`mde_egui::focus::paint_focus_ring`, the dock/Console focus idiom) so a
/// keyboard user sees where they are. A click, an Enter/Space while focused
/// ([`response_activated`], reused verbatim), or the context menu's "Open"
/// yields [`TileAction::Activate`]; a secondary click opens a Pin/Unpin + Open
/// context menu ([`tile_context_menu`]). Exports its own accesskit `Button`
/// node (lock #14) carrying the CURRENT display text as its value.
fn tile(
    ui: &egui::Ui,
    cell: NavCell,
    rect: egui::Rect,
    facts: &[String],
    status_tint: Option<egui::Color32>,
    time_secs: f64,
    rotation_enabled: bool,
    is_pinned: bool,
    group_accent: egui::Color32,
) -> TileOutcome {
    let surface = cell.surface;
    let resp = ui.interact(rect, nav_cell_id(cell), egui::Sense::click());
    let hovered = resp.hovered();
    let focused = resp.has_focus();
    let lit = hovered || focused;
    let painter = ui.painter().clone();

    let fill = if lit { Style::SURFACE_HI } else { Style::BG };
    painter.rect_filled(rect, START_CHROME_RADIUS, fill);
    painter.rect_stroke(
        rect,
        START_CHROME_RADIUS,
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
        egui::StrokeKind::Inside,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(rect.min, egui::vec2(START_ACCENT_W, rect.height())),
        START_CHROME_RADIUS,
        group_accent,
    );
    if is_pinned {
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.top()),
                egui::vec2(rect.width(), START_ACCENT_W),
            ),
            START_CHROME_RADIUS,
            Style::ACCENT,
        );
    }
    let tint = if lit { Style::TEXT } else { Style::TEXT_DIM };

    if let Some(tex) = icon_texture(ui.ctx(), surface.icon_id(), TILE_ICON, tint) {
        let icon = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.top() + Style::SP_XS + TILE_ICON / 2.0),
            egui::vec2(TILE_ICON, TILE_ICON),
        );
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(tex.id(), icon, uv, egui::Color32::WHITE);
    }

    // WIN7-4 — a severity-colour dot where this surface genuinely has one
    // ([`tile_status_tint`]'s own doc names which surfaces + why); `None`
    // paints nothing, never an invented colour (§7).
    if let Some(color) = status_tint {
        painter.circle_filled(
            egui::pos2(
                rect.right() - Style::SP_XS - TILE_STATUS_DOT_R,
                rect.top() + Style::SP_XS + TILE_STATUS_DOT_R,
            ),
            TILE_STATUS_DOT_R,
            color,
        );
    }

    // The label slot (WIN7-4): the static label with no live fact yet, the
    // one fact itself when there is exactly one (replacing, not stacking —
    // the module doc's pixel-budget note), or the current rotation step
    // among 2+ facts (lock #5). Bottom-centred, clipped to the tile so a long
    // string trims cleanly at the tile edge instead of spilling into its
    // neighbour (unchanged WIN7-3 behaviour).
    let display_text = tile_display_text(surface, facts, time_secs, rotation_enabled);
    painter.with_clip_rect(rect).text(
        egui::pos2(rect.center().x, rect.bottom() - Style::SP_XS),
        egui::Align2::CENTER_BOTTOM,
        display_text,
        egui::FontId::proportional(Style::SMALL),
        tint,
    );

    // SM-QOL-1 — the shared 2px keyboard-focus ring (the dock/Console idiom),
    // drawn only when this tile holds keyboard focus.
    mde_egui::focus::paint_focus_ring(&painter, rect, focused);

    install_tile_accessibility(ui.ctx(), cell, rect, display_text);

    let mut action = if response_activated(ui, &resp) {
        TileAction::Activate
    } else {
        TileAction::None
    };
    let menu_open = tile_context_menu(&resp, surface, is_pinned, &mut action);
    TileOutcome { action, menu_open }
}

/// The stable id of one grouped tile's interactive rect (the `dock.rs`
/// `pick_cell_id` idiom restated — tests read a tile's settled `Rect` back to
/// click its exact centre, the addressable-cell idiom).
fn tile_id(surface: Surface) -> egui::Id {
    egui::Id::new(("start-menu-tile", surface))
}

/// The stable id of one PINNED-section tile's rect (SM-QOL-1) — a namespace
/// distinct from [`tile_id`] so a surface pinned at the top and still shown in
/// its group below carry two non-colliding interact ids.
fn pinned_tile_id(surface: Surface) -> egui::Id {
    egui::Id::new(("start-menu-pinned-tile", surface))
}

/// One row of a tile's right-click context menu (SM-QOL-1, feature 3) — the
/// stable ids let a test read a menu row's `Rect` back and click it, exactly
/// as `dock`'s own `SurfaceContextItem` rows are tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TileContextItem {
    Open,
    Pin,
}

/// The stable id of one context-menu row (the `dock::surface_context_item_id`
/// idiom).
fn tile_context_item_id(surface: Surface, item: TileContextItem) -> egui::Id {
    egui::Id::new(("start-menu-tile-context", surface, item))
}

/// A tile's secondary-click context menu (SM-QOL-1, feature 3): **Open** (same
/// outcome as a left-click) and **Pin to top**/**Unpin from top** (toggles the
/// pinned section). Mirrors `dock::paint_surface_context_menu` exactly —
/// `resp.context_menu` opens it on a secondary click (`Sense::click()` already
/// senses that), each row is a [`context_menu_row`], and the chosen action is
/// written back through `action` (Open overrides the passed-through
/// click/keyboard action; both end at [`TileAction::Activate`]). Returns
/// whether the menu is open this frame so the caller can suppress the panel's
/// click-away dismissal while it is (a menu-row click lands in a SEPARATE popup
/// Area, outside the panel rect, and would otherwise read as a click-away).
fn tile_context_menu(
    resp: &egui::Response,
    surface: Surface,
    is_pinned: bool,
    action: &mut TileAction,
) -> bool {
    let inner = resp.context_menu(|ui| {
        if context_menu_row(
            ui,
            tile_context_item_id(surface, TileContextItem::Open),
            "Open",
        ) {
            *action = TileAction::Activate;
            ui.close_menu();
        }
        let pin_label = if is_pinned {
            "Unpin from top"
        } else {
            "Pin to top"
        };
        if context_menu_row(
            ui,
            tile_context_item_id(surface, TileContextItem::Pin),
            pin_label,
        ) {
            *action = TileAction::TogglePin;
            ui.close_menu();
        }
    });
    inner.is_some()
}

/// One context-menu row — restates `dock::context_menu_row` exactly (a
/// hand-painted `Sense::click()` row with the shared hover wash + focus ring),
/// since that helper is private to `dock`. All colours are `Style` tokens (§4);
/// [`response_activated`] gives it the SAME click-vs-keyboard activation the
/// tiles themselves use.
fn context_menu_row(ui: &mut egui::Ui, id: egui::Id, label: &str) -> bool {
    let width = ui.available_width().max(Style::SP_XL * 4.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, Style::SP_L), egui::Sense::hover());
    let resp = ui.interact(rect, id, egui::Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, START_CHROME_RADIUS, Style::SURFACE_HI);
    }
    ui.painter().text(
        egui::pos2(rect.left() + Style::SP_S, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(Style::SMALL),
        Style::TEXT,
    );
    mde_egui::focus::paint_focus_ring(ui.painter(), rect, resp.has_focus());
    install_context_menu_row_accessibility(ui.ctx(), id, rect, label);
    response_activated(ui, &resp)
}

/// Fold [`TileFactInputs`] into `surface`'s rotating live facts (WIN7-4,
/// lock #5) — the SAME "read already-published state, fold to a short
/// display string" idiom [`crate::dock::badge_for`] already uses per-surface
/// for the picker's own badges, mirrored here rather than reinvented. Every
/// arm reads a [`TileFactInputs`] field folded from the SAME source an
/// existing dock pip / status indicator / the surface's own already-shown
/// status chip already reads (§7 honest-gating — see each field's own doc
/// comment on [`TileFactInputs`] for its exact source). Surfaces with no
/// genuinely live fact anywhere in the codebase today (Editor, About —
/// verified, not assumed: Editor has no aggregate open/dirty count anywhere,
/// About is 100% compile-time build info) fall to the empty `Vec` —
/// [`tile_display_text`] then shows the plain static label, never a
/// fabricated rotation. Zero counts/absent data are honest silence (the
/// `dock.rs` "zero paints no badge" convention, restated per-fact here) —
/// e.g. Files shows nothing while idle, not "0 transferring".
#[allow(clippy::too_many_lines)] // one match arm per Surface::ALL variant (19), same shape as badge_for
fn tile_facts(surface: Surface, inputs: &TileFactInputs) -> Vec<String> {
    match surface {
        Surface::Chat => {
            let mut facts = Vec::new();
            if inputs.chat_unread > 0 {
                facts.push(format!("{} unread", inputs.chat_unread));
            }
            if let Some(host) = &inputs.chat_recent_sender {
                facts.push(format!("Latest: {host}"));
            }
            facts
        }
        Surface::MeshView => {
            let mut facts = Vec::new();
            if inputs.mesh.seen && inputs.mesh.peers_total > 0 {
                facts.push(format!(
                    "{}/{} peers online",
                    inputs.mesh.peers_online, inputs.mesh.peers_total
                ));
            }
            if inputs.mesh.seen {
                facts.push(mesh_health_label(inputs.mesh.health).to_string());
            }
            facts
        }
        Surface::System => {
            let mut facts = Vec::new();
            if let Some(r) = &inputs.segments.device {
                facts.push(format!("Device: {}", r.summary));
            }
            if let Some(r) = &inputs.segments.power {
                facts.push(format!("Power: {}", r.summary));
            }
            facts
        }
        Surface::Media => inputs.media_title.as_ref().map_or_else(Vec::new, |title| {
            vec![
                title.clone(),
                if inputs.media_playing {
                    "Playing".to_string()
                } else {
                    "Paused".to_string()
                },
            ]
        }),
        Surface::Music => match &inputs.music_now_playing {
            Some((title, artist)) if !artist.is_empty() => {
                vec![title.clone(), format!("by {artist}")]
            }
            Some((title, _)) => vec![title.clone()],
            None => Vec::new(),
        },
        Surface::Voice => inputs.voice_call_label.clone().into_iter().collect(),
        Surface::Files => {
            if inputs.files_active_transfers > 0 {
                vec![format!("{} transferring", inputs.files_active_transfers)]
            } else {
                Vec::new()
            }
        }
        Surface::Storage => inputs
            .storage_local
            .map_or_else(Vec::new, |(disks, free_mib)| {
                vec![
                    format!("{disks} disk{}", plural_suffix(disks)),
                    format!("{} GiB free", free_mib / 1024),
                ]
            }),
        Surface::Bookmarks => {
            if inputs.bookmarks_total > 0 {
                vec![format!("{} bookmarks", inputs.bookmarks_total)]
            } else {
                Vec::new()
            }
        }
        Surface::Phones => {
            let (paired, online) = inputs.phones;
            if paired > 0 {
                vec![format!("{paired} paired"), format!("{online} online")]
            } else {
                Vec::new()
            }
        }
        Surface::Workbench => {
            if !inputs.workbench_seen {
                return Vec::new();
            }
            vec![
                format!(
                    "{} peer{}",
                    inputs.workbench_peer_count,
                    plural_suffix(inputs.workbench_peer_count)
                ),
                inputs
                    .workbench_leader
                    .as_ref()
                    .map_or_else(|| "no leader".to_string(), |l| format!("leader {l}")),
            ]
        }
        Surface::Desktop => {
            let mut facts = Vec::new();
            if let Some((name, protocol)) = &inputs.desktop_session {
                facts.push(format!("{name} · {protocol}"));
            }
            if inputs.desktop_sources > 0 {
                facts.push(format!(
                    "{} source{}",
                    inputs.desktop_sources,
                    plural_suffix(inputs.desktop_sources)
                ));
            }
            facts
        }
        Surface::InfraCode => inputs
            .infra_services
            .map_or_else(Vec::new, |(total, healthy)| {
                vec![
                    format!("{total} service{}", plural_suffix(total)),
                    format!("{healthy} healthy"),
                ]
            }),
        Surface::Browser => {
            if inputs.browser_tabs > 0 {
                vec![format!("{} tabs", inputs.browser_tabs)]
            } else {
                Vec::new()
            }
        }
        Surface::Terminal => inputs
            .terminal_tabs
            .filter(|&n| n > 0)
            .map_or_else(Vec::new, |n| vec![format!("{n} tabs")]),
        // Editor / About / Explorer: no genuinely live fact exists anywhere in
        // the codebase today (verified, not assumed) — the honest static tile,
        // matching §7 over forcing a rotation with nothing real to show. (The
        // Explorer's discovered-unit count is not plumbed to this pane's
        // `TileFactInputs`; wiring one is a follow-on, not this promotion.)
        Surface::Editor | Surface::About | Surface::Explorer | Surface::MapsLocation => Vec::new(),
        // Timers deliberately sits OUTSIDE Surface::ALL/TILE_GROUPS (lock
        // #20 — the clock strip is its ONE home, never a picker/tile
        // entry), so this arm is never actually reached by `left_pane`'s
        // render loop; it exists only because `Surface`'s match must be
        // exhaustive over every variant, not just the tileable ones.
        Surface::Timers => Vec::new(),
    }
}

/// `""` for 1, `"s"` otherwise — the tiny pluralization helper every
/// count-shaped [`tile_facts`] arm above shares (mirrors the identical
/// inline ternary `dock.rs`'s own count badges already repeat per call site;
/// named once here instead of restating the ternary N times).
fn plural_suffix(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Short label for a mesh health reading (WIN7-4's System/MeshView tile
/// facts) — the SAME [`LighthouseHealth`] `dock::badge_for`'s own System
/// arm already folds into a badge, worded for a tile's label slot instead of
/// a bare dot.
const fn mesh_health_label(health: LighthouseHealth) -> &'static str {
    match health {
        LighthouseHealth::AllHealthy => "Mesh healthy",
        LighthouseHealth::Degraded => "Mesh degraded",
        LighthouseHealth::None => "Mesh offline",
    }
}

/// Tone colour for a mesh health reading — the SAME `Style::SUPPORT_*` tones
/// `dock.rs`'s own `badge_tone_color` uses for its `BadgeTone::Healthy /
/// Degraded / Offline` (that helper + its `BadgeTone` enum are private to
/// dock.rs; restating this trivial 3-arm enum→constant-colour mapping here
/// is not a second *source of truth* — no data is re-derived, only a fixed
/// presentation colour is chosen per already-folded [`LighthouseHealth`]
/// variant).
const fn mesh_health_color(health: LighthouseHealth) -> egui::Color32 {
    match health {
        LighthouseHealth::AllHealthy => Style::SUPPORT_SUCCESS,
        LighthouseHealth::Degraded => Style::SUPPORT_WARNING,
        LighthouseHealth::None => Style::SUPPORT_ERROR,
    }
}

/// Rank a segment rollup's severity for a worst-of comparison (System's tile
/// tint below, [`tile_status_tint`]) — folds [`status::severity_label`]'s
/// own 5-bucket vocabulary (the SAME fold `status.rs`'s segment pips already
/// use) into an ordinal so "critical" outranks "warning" outranks
/// "ok"/"info" outranks "no reading yet".
fn severity_rank(rollup: Option<&status::SegmentRollup>) -> u8 {
    match status::severity_label(rollup) {
        "critical" => 4,
        "warning" => 3,
        "ok" => 2,
        "info" => 1,
        _ => 0,
    }
}

/// The seam WIN7-4 (design lock #5's rotating live-tile content) lights up —
/// a severity-colour dot where a surface genuinely HAS a severity/health
/// concept (System's Device/Power segment rollups, MeshView's mesh health,
/// Chat/Files' accent-on-nonzero-count, the SAME tone language `dock.rs`'s
/// own badges already use for the identical surfaces/data), `None`
/// everywhere else — most of the 19 surfaces are plain counts with no
/// health concept at all, and painting a tint for those would be an
/// invented severity, not a reused one (§7). `None` is also the honest
/// "nothing has landed yet" answer (pre-poll / never-visited-this-session),
/// matching [`tile_facts`]'s own zero-is-silence posture.
fn tile_status_tint(surface: Surface, inputs: &TileFactInputs) -> Option<egui::Color32> {
    match surface {
        Surface::System => {
            let device = inputs.segments.device.as_ref();
            let power = inputs.segments.power.as_ref();
            if device.is_none() && power.is_none() {
                return None;
            }
            Some(if severity_rank(device) >= severity_rank(power) {
                status::severity_color(device)
            } else {
                status::severity_color(power)
            })
        }
        Surface::MeshView if inputs.mesh.seen => Some(mesh_health_color(inputs.mesh.health)),
        Surface::Chat if inputs.chat_unread > 0 => Some(Style::ACCENT),
        Surface::Files if inputs.files_active_transfers > 0 => Some(Style::ACCENT),
        _ => None,
    }
}

/// Whether live tiles may advance their rotating facts this frame. The system
/// Appearance motion setting already drives egui's per-context `animation_time`;
/// reading that context-local signal avoids a second Start-menu preference path
/// and keeps reduced/disabled motion deterministic in tests.
fn live_tile_rotation_enabled(ui: &egui::Ui) -> bool {
    ui.style().animation_time > f32::EPSILON
}

/// Which of `len` facts should show right now, given the current frame clock
/// (`ui.input(|i| i.time)`, seconds since the egui `Context` was created —
/// the SAME time source `explorer.rs`'s `hero_card`/`tick_ambient` already
/// read for their own time-driven visuals, not a new mechanism). Pure: no
/// per-tile stored state, so [`StartMenuState`] stays the "no egui handles"
/// pure struct its own doc comment promises — every tile derives its
/// current step fresh from the ONE shared clock each frame, so they all
/// advance in lockstep and the displayed value never drifts/accumulates
/// error the way a stored "last rotated at" counter could. `len <= 1` and
/// reduced/disabled motion always answer `0` (nothing should advance).
#[allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "time_secs is always >= 0.0 (a monotonic clock since context creation); \
              truncating to whole rotation ticks is the intended behaviour"
)]
fn rotating_fact_index(len: usize, time_secs: f64, rotation_enabled: bool) -> usize {
    if len <= 1 || !rotation_enabled {
        return 0;
    }
    let tick = (time_secs / TILE_FACT_ROTATE_INTERVAL.as_secs_f64()) as u64;
    (tick % len as u64) as usize
}

/// The text [`tile`] paints in its label slot this frame (WIN7-4): the
/// surface's static [`Surface::label`] when `facts` is empty (unchanged
/// WIN7-3 behaviour), the one fact when exactly one exists (replacing the
/// label on the SAME slot — the tile's locked 48pt height has no pixel room
/// for a genuine third line, verified against the real `Style` spacing
/// tokens: the icon and label already sit only ~2px apart at the current
/// geometry), or the current [`rotating_fact_index`] step among 2+ facts
/// (lock #5). The surface's real name is never actually lost when a fact is
/// showing instead: its accesskit `Button` node still carries
/// `surface.label()` as `.label()` unconditionally
/// ([`install_tile_accessibility`]) — only the VISUAL slot swaps.
fn tile_display_text<'a>(
    surface: Surface,
    facts: &'a [String],
    time_secs: f64,
    rotation_enabled: bool,
) -> &'a str {
    match facts.len() {
        0 => surface.label(),
        1 => facts[0].as_str(),
        len => facts[rotating_fact_index(len, time_secs, rotation_enabled)].as_str(),
    }
}

/// Whether Esc should dismiss the WHOLE Start Menu this frame — inert while a
/// text field owns the keyboard (the embedded Custom-entry form's draft
/// fields can hold Esc-to-cancel focus one day; the same gate
/// `console::handle_keys` already applies to its own Esc, restated here so
/// typing in the embedded form can't also collapse the outer panel).
fn esc_pressed(ui: &egui::Ui) -> bool {
    if ui.ctx().memory(|m| m.focused().is_some()) {
        return false;
    }
    ui.input(|i| i.key_pressed(egui::Key::Escape))
}

// ── type-to-launch search (SHELL-UX-3) ──────────────────────────────────────

/// Render the left pane's search affordance for this frame and drive its
/// keyboard: the always-visible search box at the pane's bottom edge, plus —
/// while a query is live — the ranked result list that REPLACES the grouped
/// tile grid above it (the right/Console pane is never touched, keeping this
/// whole feature self-contained to the left pane). Launches route through the
/// SAME [`StartMenuState::tile_activation`] seam a tile click already uses
/// (set the surface, close the menu), so `main.rs`'s existing drain carries a
/// searched launch with zero new plumbing — a keyboard Enter and a mouse
/// click on a tile end in the identical outcome.
///
/// Returns `(search_ate_escape, menu_open)`: whether the search consumed this
/// frame's Esc (query non-empty → Esc cleared it rather than dismissing the
/// menu — the caller uses that to suppress the one-frame Console self-closure
/// the shared Esc would otherwise propagate, see [`start_menu_panel`]'s guard),
/// and whether a tile's right-click context menu is open this frame (SM-QOL-1 —
/// the caller suppresses its click-away dismissal so a menu-row click doesn't
/// read as a click-away). The search branch never opens a tile menu, so its
/// `menu_open` is always `false`.
#[allow(clippy::suboptimal_flops)] // layout arithmetic reads clearer than mul_add (the left_pane idiom)
fn start_menu_search(
    ui: &mut egui::Ui,
    left_rect: egui::Rect,
    state: &mut StartMenuState,
    console: &mut ConsoleState,
) -> (bool, bool) {
    // The search field sits in the left pane's bottom headroom; the grid /
    // result list get everything above it.
    let search_rect = search_rect(left_rect);
    let content_rect = left_pane_content_rect(left_rect, search_rect);

    let query_changed = search_field(
        ui,
        search_rect,
        &mut state.search_query,
        &mut state.search_focus_pending,
    );
    if query_changed {
        // A fresh keystroke re-ranks the list — snap the highlight back to the
        // new top match rather than leaving it pointing at a now-shifted row.
        state.search_highlight = 0;
    }
    install_search_accessibility(ui.ctx(), search_rect, &state.search_query);

    let query = state.search_query.trim().to_owned();
    if query.is_empty() {
        // Empty query — the unchanged full grouped grid (WIN7-3 behaviour, plus
        // SM-QOL-1's pinned section + arrow-key tile nav + per-tile context
        // menu), and Esc dismisses the whole menu exactly as before. The two
        // `&state` borrows below are disjoint fields (`tile_inputs` read-only,
        // `pinned` mutated by a right-click Pin/Unpin), so they coexist.
        let pinned_before = state.pinned.clone();
        let out = scrollable_left_pane(ui, content_rect, &state.tile_inputs, &mut state.pinned);
        if state.pinned != pinned_before {
            state.persist_pins();
        }
        if let Some(surface) = out.activated {
            state.tile_activation = Some(surface);
            state.close();
        } else if out.closed {
            // SM-QOL-1 — Escape while a grid tile holds keyboard focus closes
            // the whole menu (the `esc_pressed` gate below can't, since a
            // focused tile makes `memory.focused()` non-empty).
            state.close();
        }
        if state.open && esc_pressed(ui) {
            state.close();
        }
        return (false, out.menu_open);
    }

    // A live query — the ranked result list replaces the grid in this pane.
    let matches = search_matches(&query);
    let (up, down, enter, esc) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowUp),
            i.key_pressed(egui::Key::ArrowDown),
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });
    // Move the highlight BEFORE rendering so the painted row and any Enter this
    // frame agree on which match is selected (clamped — a shrinking list can
    // never leave the highlight past the end).
    if !matches.is_empty() {
        let mut sel = state.search_highlight.min(matches.len() - 1);
        if down {
            sel = (sel + 1).min(matches.len() - 1);
        }
        if up {
            sel = sel.saturating_sub(1);
        }
        state.search_highlight = sel;
    }
    install_search_results_announcement(
        ui.ctx(),
        content_rect,
        &query,
        &matches,
        state.search_highlight,
    );
    let clicked = scrollable_search_results(ui, content_rect, &matches, state.search_highlight);

    // Enter launches the highlighted match (the top match by default); a click
    // on any row launches that one. App hits use the tile_activation seam, and
    // Console hits delegate to ConsoleState's existing activation path.
    let launch = clicked.or_else(|| {
        (enter && !matches.is_empty())
            .then(|| matches[state.search_highlight.min(matches.len() - 1)])
    });
    if let Some(hit) = launch {
        match hit {
            StartSearchHit::Surface(surface) => {
                state.tile_activation = Some(surface);
                state.close();
            }
            StartSearchHit::Console(hit) => {
                console.activate_index(hit.flat);
            }
        }
        return (false, false);
    }
    if esc {
        // Esc over a live query clears it (and re-arms the emptied box's focus
        // so typing continues), never closing the menu — a second, now-empty
        // Esc is what dismisses it. Report it so the caller suppresses the
        // Console self-closure the shared Esc briefly triggered.
        state.clear_search();
        state.search_focus_pending = true;
        return (true, false);
    }
    (false, false)
}

/// The type-to-launch search field (SHELL-UX-3), placed at the bottom of the
/// left pane via `ui.put` (the `chooser.rs`/`explorer.rs` `TextEdit` +
/// `focus_pending` idiom). `return_key(None)` deliberately stops Enter from
/// surrendering the box's focus: this module handles Enter itself to launch
/// the highlight, and keeping the box focused keeps the embedded Console's own
/// Enter/arrow nav inert (its `handle_keys` bails while a text field owns the
/// keyboard), so one Enter never both launches a result AND fires a Console
/// row. Auto-focuses on the frame `focus_pending` is armed (menu open / query
/// cleared) so "open, then just type" works. Returns whether the query text
/// changed this frame (the caller resets the highlight to the top match).
fn search_field(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    query: &mut String,
    focus_pending: &mut bool,
) -> bool {
    let painter = ui.painter().clone();
    painter.rect_filled(rect, START_CHROME_RADIUS, Style::BG);
    painter.rect_stroke(
        rect,
        START_CHROME_RADIUS,
        egui::Stroke::new(HAIRLINE_W, Style::BORDER),
        egui::StrokeKind::Inside,
    );

    let search_icon = egui::Rect::from_center_size(
        egui::pos2(
            rect.left() + Style::SP_XS + SEARCH_ICON / 2.0,
            rect.center().y,
        ),
        egui::vec2(SEARCH_ICON, SEARCH_ICON),
    );
    if let Some(tex) = icon_texture(ui.ctx(), IconId::Search, SEARCH_ICON, Style::TEXT_DIM) {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(tex.id(), search_icon, uv, egui::Color32::WHITE);
    }

    let clear_icon = egui::Rect::from_center_size(
        egui::pos2(
            rect.right() - Style::SP_XS - SEARCH_ICON / 2.0,
            rect.center().y,
        ),
        egui::vec2(SEARCH_ICON, SEARCH_ICON),
    );
    let clear_button = clear_icon.expand(Style::SP_XS);
    let text_right = if query.is_empty() {
        rect.right() - Style::SP_XS
    } else {
        clear_button.left() - Style::SP_XS
    };
    let text_rect = egui::Rect::from_min_max(
        egui::pos2(search_icon.right() + Style::SP_XS, rect.top()),
        egui::pos2(
            text_right.max(search_icon.right() + Style::SP_L),
            rect.bottom(),
        ),
    );
    let resp = ui.put(
        text_rect,
        egui::TextEdit::singleline(query)
            .hint_text(START_SEARCH_HINT)
            .font(egui::FontId::proportional(Style::BODY))
            .desired_width(text_rect.width())
            .return_key(None)
            .frame(false),
    );
    if *focus_pending {
        resp.request_focus();
        *focus_pending = false;
    }
    let mut changed = resp.changed();

    if !query.is_empty() {
        let clear_resp = ui.interact(clear_button, search_clear_button_id(), egui::Sense::click());
        if clear_resp.hovered() || clear_resp.has_focus() {
            painter.rect_filled(clear_button, START_CHROME_RADIUS, Style::SURFACE_HI);
        }
        let tint = if clear_resp.hovered() || clear_resp.has_focus() {
            Style::TEXT
        } else {
            Style::TEXT_DIM
        };
        if let Some(tex) = icon_texture(ui.ctx(), IconId::Close, SEARCH_ICON, tint) {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(tex.id(), clear_icon, uv, egui::Color32::WHITE);
        }
        mde_egui::focus::paint_focus_ring(&painter, clear_button, clear_resp.has_focus());
        install_search_clear_accessibility(ui.ctx(), clear_button);
        if response_activated(ui, &clear_resp) {
            query.clear();
            resp.request_focus();
            changed = true;
        }
    }

    changed
}

/// The stable id for the Start Menu search clear icon button.
fn search_clear_button_id() -> egui::Id {
    egui::Id::new("start-menu-search-clear")
}

/// Rank the 19 tileable [`Surface::ALL`] entries against `query`
/// (case-insensitive), best match first: a label *prefix* hit (rank 0)
/// outranks a label *substring* hit (rank 1), which outranks a hit on the
/// surface's *group name* only (rank 2 — so typing a category like "media"
/// still surfaces its members), which outranks a *fuzzy subsequence* hit on the
/// label (rank 3 — the query's chars appear in order but not contiguous, so
/// "edtr" still finds Editor and "meshmp" still finds Mesh Map). The fuzzy tier
/// is purely additive below the three exact tiers, so no prefix/substring
/// result is displaced; among fuzzy hits the tighter match wins ([`fuzzy_cost`]
/// orders by fewer gaps then earlier start). Ties within any tier keep
/// [`Surface::ALL`] order, so the ranking is stable and predictable, never RNG.
/// A surface that matches nowhere is dropped; an empty/whitespace query yields
/// no matches (the caller then shows the full grouped grid instead). Pure over
/// the static surface tables — unit-tested without a GPU, and the ONE authority
/// the render + keyboard both read so the painted list and Enter's target can't
/// diverge.
fn search_matches(query: &str) -> Vec<StartSearchHit> {
    let console_hits = console::static_search_candidates();
    let cap = Surface::ALL.len() + console_hits.len();
    let surface_items = Surface::ALL
        .iter()
        .copied()
        .enumerate()
        .map(|(idx, surface)| {
            SearchItem::new(
                SearchDomain::App,
                surface.label(),
                format!("surface:{surface:?}"),
                StartSearchHit::Surface(surface),
            )
            .with_terms([tile_group_label(surface), "App"])
            .with_source_rank(idx)
        });
    let console_items = console_hits.into_iter().map(|hit| {
        SearchItem::new(
            SearchDomain::App,
            hit.label,
            format!("console:{}", hit.flat),
            StartSearchHit::Console(hit),
        )
        .with_terms([hit.desc, hit.group, hit.tool, "Console"])
        .with_source_rank(Surface::ALL.len() + hit.flat)
    });
    ranked_hits(query, surface_items.chain(console_items), cap)
        .into_iter()
        .map(|hit| hit.item.payload)
        .collect()
}

/// Which [`TILE_GROUPS`] group a surface sits in (its label). Every
/// [`Surface::ALL`] entry sits in exactly one group (the compile-time guard
/// above), so the lookup always resolves for a searchable surface; the
/// never-searched `Timers` (outside `ALL`) would fall to `""`.
fn tile_group_label(surface: Surface) -> &'static str {
    launcher_group_label(surface)
}

/// The category accent for a grouped-grid surface (B6). Returns `None` only
/// for non-grid surfaces such as [`Surface::Timers`].
#[cfg(test)]
fn tile_group_accent(surface: Surface) -> Option<egui::Color32> {
    launcher_group_accent(surface)
}

/// The one authoritative grouped-grid surface order: every tileable surface
/// once, with no pinned/search duplicates. The render loop walks the same
/// [`TILE_GROUPS`] table; tests use this pure helper to pin the B6 launcher
/// reachability contract.
#[cfg(test)]
fn grouped_grid_surfaces() -> Vec<Surface> {
    TILE_GROUPS
        .iter()
        .flat_map(|group| group.surfaces.iter().copied())
        .collect()
}

/// The ranked result list that replaces the grouped grid while a query is live
/// (SHELL-UX-3): one compact row per match — leading glyph, result label,
/// and dim source/group name right-aligned — with the keyboard-highlighted row
/// (and any hover) wearing the SAME `SURFACE_HI` fill the tiles use, plus an
/// accent left stripe on the highlight so the Enter target reads at a glance.
/// A click on a row returns its hit for the caller to launch through the owning
/// activation seam, matching the tile grid's own click contract. An
/// empty result set paints an honest "no match" note (§7 — never a silent
/// blank). Direct-painter + `ui.interact` per row, the SAME addressable-cell
/// style [`tile`] uses (so a test can read a row's rect back by id and click
/// its centre).
#[allow(clippy::suboptimal_flops)] // layout arithmetic reads clearer than mul_add (the tile() idiom)
fn search_results(
    ui: &egui::Ui,
    rect: egui::Rect,
    matches: &[StartSearchHit],
    selected: usize,
) -> Option<StartSearchHit> {
    let painter = ui.painter().clone();
    if matches.is_empty() {
        painter.text(
            egui::pos2(rect.left() + PANE_PAD, rect.top() + PANE_PAD),
            egui::Align2::LEFT_TOP,
            "No apps or commands match your search",
            egui::FontId::proportional(Style::BODY),
            Style::TEXT_DIM,
        );
        return None;
    }
    let mut activated = None;
    let x0 = rect.left() + PANE_PAD;
    let w = (rect.width() - PANE_PAD * 2.0).max(0.0);
    let mut y = rect.top() + PANE_PAD;
    for (i, &hit) in matches.iter().enumerate() {
        let row = egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(w, RESULT_ROW_H));
        let resp = ui.interact(row, search_result_hit_id(hit), egui::Sense::click());
        let is_sel = i == selected;
        let hovered = resp.hovered();
        if is_sel || hovered {
            painter.rect_filled(row, START_CHROME_RADIUS, Style::SURFACE_HI);
        }
        if is_sel {
            painter.rect_filled(
                egui::Rect::from_min_size(row.min, egui::vec2(Style::SP_XS / 2.0, row.height())),
                0.0,
                Style::ACCENT,
            );
        }
        let tint = if is_sel || hovered {
            Style::TEXT
        } else {
            Style::TEXT_DIM
        };
        if let Some(tex) = icon_texture(ui.ctx(), hit.icon_id(), RESULT_ICON, tint) {
            let icon = egui::Rect::from_center_size(
                egui::pos2(row.left() + Style::SP_S + RESULT_ICON / 2.0, row.center().y),
                egui::vec2(RESULT_ICON, RESULT_ICON),
            );
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(tex.id(), icon, uv, egui::Color32::WHITE);
        }
        painter.with_clip_rect(row).text(
            egui::pos2(
                row.left() + Style::SP_S + RESULT_ICON + Style::SP_S,
                row.center().y,
            ),
            egui::Align2::LEFT_CENTER,
            hit.label(),
            egui::FontId::proportional(Style::BODY),
            tint,
        );
        painter.text(
            egui::pos2(row.right() - Style::SP_S, row.center().y),
            egui::Align2::RIGHT_CENTER,
            hit.detail(),
            egui::FontId::proportional(Style::SMALL),
            Style::TEXT_DIM,
        );
        install_result_accessibility(ui.ctx(), hit, row, is_sel, i, matches.len());
        if response_activated(ui, &resp) {
            activated = Some(hit);
        }
        y += RESULT_ROW_H;
    }
    activated
}

fn search_results_content_h(matches: &[StartSearchHit], rect_h: f32) -> f32 {
    if matches.is_empty() {
        rect_h
    } else {
        (PANE_PAD * 2.0 + matches.len() as f32 * RESULT_ROW_H).max(rect_h)
    }
}

fn scrollable_search_results(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    matches: &[StartSearchHit],
    selected: usize,
) -> Option<StartSearchHit> {
    let content_h = search_results_content_h(matches, rect.height());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(rect);
    egui::ScrollArea::vertical()
        .id_salt("start-menu-search-results-scroll")
        .auto_shrink([false, false])
        .show(&mut child, |ui| {
            let results_rect = egui::Rect::from_min_size(
                ui.next_widget_position(),
                egui::vec2(rect.width(), content_h),
            );
            let out = search_results(ui, results_rect, matches, selected);
            ui.allocate_rect(results_rect, egui::Sense::hover());
            out
        })
        .inner
}

/// The stable id of one search-result row's interactive rect (the `tile_id`
/// addressable-cell idiom, restated for the result list so tests can read a
/// row's settled `Rect` back to click its centre).
fn search_result_id(surface: Surface) -> egui::Id {
    egui::Id::new(("start-menu-search-result", surface))
}

fn console_search_result_id(flat: usize) -> egui::Id {
    egui::Id::new(("start-menu-console-search-result", flat))
}

fn search_result_hit_id(hit: StartSearchHit) -> egui::Id {
    match hit {
        StartSearchHit::Surface(surface) => search_result_id(surface),
        StartSearchHit::Console(hit) => console_search_result_id(hit.flat),
    }
}

// ── accesskit (lock #14) ─────────────────────────────────────────────────────

/// Convert an egui rect to an accesskit one (the `status.rs` helper, restated
/// module-locally — each panel's accesskit section owns its own copy).
fn accesskit_rect(rect: egui::Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// The stable accesskit node id for the whole panel (`Role::Menu`).
fn start_menu_accesskit_id() -> egui::Id {
    egui::Id::new("start-menu-accesskit")
}

/// The stable accesskit node id for the left (tile-grid) pane landmark.
fn tiles_pane_accesskit_id() -> egui::Id {
    egui::Id::new("start-menu-tiles-pane-accesskit")
}

/// The stable accesskit node id for the right (embedded Console) pane
/// landmark.
fn console_pane_accesskit_id() -> egui::Id {
    egui::Id::new("start-menu-console-pane-accesskit")
}

/// Install the panel-level accesskit tree: the whole panel as a `Menu`, and
/// each pane as a landmark `Group`, so a screen reader can navigate the Start
/// Menu shell. Per-tile, search, context-row, and live-region nodes are emitted
/// by the focused helpers that own those controls.
fn install_accessibility(
    ctx: &egui::Context,
    rect: egui::Rect,
    left: egui::Rect,
    right: egui::Rect,
) {
    let _ = ctx.accesskit_node_builder(start_menu_accesskit_id(), |node| {
        node.set_role(egui::accesskit::Role::Menu);
        node.set_label("Start Menu");
        node.set_bounds(accesskit_rect(rect));
    });
    let _ = ctx.accesskit_node_builder(tiles_pane_accesskit_id(), |node| {
        node.set_role(egui::accesskit::Role::Group);
        node.set_label("Start Menu tiles");
        node.set_bounds(accesskit_rect(left));
    });
    let _ = ctx.accesskit_node_builder(console_pane_accesskit_id(), |node| {
        node.set_role(egui::accesskit::Role::Group);
        node.set_label("Console");
        node.set_bounds(accesskit_rect(right));
    });
}

/// The stable accesskit node id for the Start Menu's own open/close
/// announcement (WIN7-7, lock #14) — deliberately distinct from
/// [`start_menu_accesskit_id`]'s `Role::Menu` landmark node (a different
/// role AND a different label, "Start Menu status" vs. "Start Menu", so a
/// label-keyed lookup — the established test idiom in this file — can never
/// find the wrong one of the two).
fn start_menu_state_accesskit_id() -> egui::Id {
    egui::Id::new("start-menu-state-accesskit")
}

/// Announce the Start Menu's own open/close transition (lock #14 — "a
/// screen reader user needs to know the menu opened, not just see new
/// content appear"). [`install_accessibility`] gives the panel a `Role::Menu`
/// landmark, but a landmark node silently appearing in the tree is not
/// itself an announcement — this crate's own convention for "something
/// changed, tell the user" is a `Live::Polite` region (`status.rs`'s
/// `install_status_accessibility`, this module's own
/// [`install_tiles_live_summary`]), restated here at the whole-panel level.
/// Called every frame [`start_menu_panel`]'s content closure runs (i.e.
/// whenever `t > 0.001`: opening, settled open, or still mid-close-tween),
/// keyed off `open` rather than the slide progress — the value is constant
/// for as long as the menu is steadily in ONE state, so it only actually
/// re-announces on a genuine open→closed or closed→open edge, never once
/// per frame (the same value-stability-avoids-spam reasoning
/// `install_status_accessibility` already relies on). No call at all once
/// the panel is fully closed and settled — [`start_menu_panel`]'s own early
/// return means this whole closure stops running for that frame, so the
/// node simply stops being emitted, the honest-silence posture
/// [`install_tiles_live_summary`] already uses for "nothing to say."
fn install_start_menu_state_announcement(ctx: &egui::Context, rect: egui::Rect, open: bool) {
    let value = if open {
        "Start Menu opened"
    } else {
        "Start Menu closed"
    };
    let _ = ctx.accesskit_node_builder(start_menu_state_accesskit_id(), |node| {
        node.set_role(egui::accesskit::Role::Status);
        node.set_live(egui::accesskit::Live::Polite);
        node.set_label("Start Menu status");
        node.set_value(value);
        node.set_bounds(accesskit_rect(rect));
    });
}

/// The stable accesskit node id for one tile (WIN7-3, lock #14) — namespaced by
/// section (SM-QOL-1) so a surface's Pinned copy and its group copy export two
/// distinct nodes rather than colliding on one id.
fn tile_accesskit_id(cell: NavCell) -> egui::Id {
    if cell.pinned {
        egui::Id::new(("start-menu-pinned-tile-accesskit", cell.surface))
    } else {
        egui::Id::new(("start-menu-tile-accesskit", cell.surface))
    }
}

/// Install one tile's own accesskit node (lock #14 — "every tile", not just
/// the panel level): a `Button` role with the surface's display label and
/// bounds, plus the `Click` action — the SAME shape `status.rs`'s
/// `install_segment_accessibility` already uses for its own per-item pips
/// (role + label + bounds + `add_action(Click)`), restated here since that
/// helper is module-private there. WIN7-4 adds `.set_value(display_text)` —
/// the SAME "label stays the identity, value carries the live reading" split
/// `install_segment_accessibility` already uses (its own `.set_value` carries
/// the segment's current severity summary while `.set_label` stays the fixed
/// "{Segment} status"). Pinned shortcut copies keep the visible surface label,
/// but prefix the accesskit value so assistive consumers can distinguish them
/// from the grouped copy. Deliberately NOT individually `Live::Polite`: a screen
/// reader hearing every one of up to 19 tiles announce itself on the same
/// rotation clock would be a spam regression, not an accessibility win —
/// [`install_tiles_live_summary`] is the ONE live-announcing node for the whole
/// grid, mirroring NOTIF-11's own shape exactly (one live
/// `status_live_region_id` summary + per-item value-bearing-but-not-live
/// `segment_pip` nodes in `status.rs`).
fn install_tile_accessibility(
    ctx: &egui::Context,
    cell: NavCell,
    rect: egui::Rect,
    display_text: &str,
) {
    let _ = ctx.accesskit_node_builder(tile_accesskit_id(cell), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(cell.surface.label());
        if cell.pinned {
            node.set_value(format!("Pinned shortcut, {display_text}"));
        } else {
            node.set_value(display_text);
        }
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
}

/// Stable id for WIN7-4's tile-grid live region — the NOTIF-11
/// `status_live_region_id` precedent, restated at the tile-grid level.
fn tiles_live_region_id() -> egui::Id {
    egui::Id::new("start-menu-tiles-live-region")
}

/// Join every ACTUALLY-rotating tile's (2+ facts) currently-shown fact into
/// one summary string, `"{Surface label}: {current fact}"` per tile,
/// `". "`-joined — the SAME shape `status.rs`'s `status_live_summary`
/// already folds `StatusSegment::ALL` into. A tile with 0-1 facts never
/// contributes (nothing to announce — it never changes on its own), so a
/// menu with no live data anywhere yields an empty string.
fn tiles_live_summary(inputs: &TileFactInputs, time_secs: f64, rotation_enabled: bool) -> String {
    if !rotation_enabled {
        return String::new();
    }
    let mut parts = Vec::new();
    for surface in Surface::ALL {
        let facts = tile_facts(surface, inputs);
        if facts.len() >= 2 {
            let text = &facts[rotating_fact_index(facts.len(), time_secs, true)];
            parts.push(format!("{}: {text}", surface.label()));
        }
    }
    parts.join(". ")
}

/// Install the tile grid's ONE aggregate live region (lock #14's rotating-
/// content requirement, the NOTIF-11 `install_status_accessibility`
/// precedent restated at the tile-grid level rather than per-tile — see
/// [`install_tile_accessibility`]'s doc for why per-tile `Live::Polite` was
/// rejected). Installs NO node at all when nothing is currently rotating
/// (an empty summary) — a menu with no live data anywhere exports no live
/// region, rather than one that politely announces silence.
fn install_tiles_live_summary(
    ctx: &egui::Context,
    rect: egui::Rect,
    inputs: &TileFactInputs,
    time_secs: f64,
    rotation_enabled: bool,
) {
    let summary = tiles_live_summary(inputs, time_secs, rotation_enabled);
    if summary.is_empty() {
        return;
    }
    let _ = ctx.accesskit_node_builder(tiles_live_region_id(), |node| {
        node.set_role(egui::accesskit::Role::Status);
        node.set_live(egui::accesskit::Live::Polite);
        node.set_label("Start Menu live tiles");
        node.set_value(summary);
        node.set_bounds(accesskit_rect(rect));
    });
}

// ── accesskit: search (SHELL-UX-3, lock #14) ────────────────────────────────

/// The stable accesskit node id for the search box (distinct from egui's own
/// auto-generated node for the `TextEdit` widget — this one carries the
/// explicit `SearchInput` role + label + current value the shell contract
/// wants, mirroring how `install_start_menu_state_announcement` layers an
/// explicit node beside the widget tree).
fn search_field_accesskit_id() -> egui::Id {
    egui::Id::new("start-menu-search-accesskit")
}

/// The stable accesskit node id for the search clear icon button.
fn search_clear_accesskit_id() -> egui::Id {
    egui::Id::new("start-menu-search-clear-accesskit")
}

/// Install the search box's own accesskit node (lock #14): a `SearchInput`
/// role with a fixed label and the live query as its value — the SAME
/// "label = identity, value = current reading" split the tile / segment nodes
/// use, so a screen reader announces what the field is AND what has been typed
/// into it. Emitted every frame the panel is up, so the value tracks live
/// keystrokes.
fn install_search_accessibility(ctx: &egui::Context, rect: egui::Rect, query: &str) {
    let _ = ctx.accesskit_node_builder(search_field_accesskit_id(), |node| {
        node.set_role(egui::accesskit::Role::SearchInput);
        node.set_label("Start Menu search");
        node.set_value(query);
        node.set_bounds(accesskit_rect(rect));
    });
}

/// Install the search field's query-clear button as a named clickable control.
fn install_search_clear_accessibility(ctx: &egui::Context, rect: egui::Rect) {
    let _ = ctx.accesskit_node_builder(search_clear_accesskit_id(), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label("Clear Start Menu search");
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
}

/// The stable accesskit node id for one search-result row.
fn search_result_accesskit_id(hit: StartSearchHit) -> egui::Id {
    match hit {
        StartSearchHit::Surface(surface) => {
            egui::Id::new(("start-menu-search-result-accesskit", surface))
        }
        StartSearchHit::Console(hit) => {
            egui::Id::new(("start-menu-console-search-result-accesskit", hit.flat))
        }
    }
}

/// The stable accesskit node id for one tile context-menu row.
fn context_menu_row_accesskit_id(id: egui::Id) -> egui::Id {
    id.with("accesskit")
}

/// Install one Start Menu context row as a named clickable button.
fn install_context_menu_row_accessibility(
    ctx: &egui::Context,
    id: egui::Id,
    rect: egui::Rect,
    label: &str,
) {
    let _ = ctx.accesskit_node_builder(context_menu_row_accesskit_id(id), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(label);
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
    });
}

/// Install one result row's accesskit node (lock #14): a `Button` role +
/// label + bounds + `Click` action (the per-tile shape), plus `set_selected`
/// on the keyboard-highlighted row so an AT can report which match Enter would
/// launch. The list's aggregate live announcement is
/// [`install_search_results_announcement`] (the NOTIF-11 one-live-summary
/// shape), not per-row `Live::Polite` — the same anti-spam posture the tile
/// grid already uses.
fn install_result_accessibility(
    ctx: &egui::Context,
    hit: StartSearchHit,
    rect: egui::Rect,
    selected: bool,
    index: usize,
    total: usize,
) {
    let _ = ctx.accesskit_node_builder(search_result_accesskit_id(hit), |node| {
        node.set_role(egui::accesskit::Role::Button);
        node.set_label(hit.label());
        node.set_value(format!(
            "Result {} of {total}: {}, {}",
            index + 1,
            hit.kind(),
            hit.detail()
        ));
        node.set_bounds(accesskit_rect(rect));
        node.add_action(egui::accesskit::Action::Click);
        if selected {
            node.set_selected(true);
        }
    });
}

/// The stable id of the search results' one aggregate live region.
fn search_results_region_id() -> egui::Id {
    egui::Id::new("start-menu-search-results-region")
}

/// Announce the live-filtered result state (lock #14 — "the filtered-result
/// state should be announceable"): a single `Live::Polite` summary carrying
/// the match count and the currently-highlighted item, or an honest "no
/// match" when nothing matches — the NOTIF-11 / [`install_tiles_live_summary`]
/// one-live-summary-node shape, so the whole result set announces as one
/// polite update on each keystroke rather than a storm of per-row nodes.
/// Emitted only while a query is live (the caller only reaches this on the
/// non-empty branch).
fn install_search_results_announcement(
    ctx: &egui::Context,
    rect: egui::Rect,
    query: &str,
    matches: &[StartSearchHit],
    selected: usize,
) {
    let value = if matches.is_empty() {
        format!("No apps or commands match {query}")
    } else {
        let sel = matches[selected.min(matches.len() - 1)];
        format!(
            "{} result{}, {} highlighted",
            matches.len(),
            plural_suffix(matches.len()),
            sel.label()
        )
    };
    let _ = ctx.accesskit_node_builder(search_results_region_id(), |node| {
        node.set_role(egui::accesskit::Role::Status);
        node.set_live(egui::accesskit::Live::Polite);
        node.set_label("Start Menu search results");
        node.set_value(value);
        node.set_bounds(accesskit_rect(rect));
    });
}

#[cfg(test)]
mod tests {
    use super::{start_menu_panel, StartMenuPrefs, StartMenuState, PANEL_H, PANEL_W};
    use crate::console::{self, ConsoleRequest, ConsoleState};
    use crate::dock::Surface;
    use crate::screenshot::Capture;
    use crate::status::{SegmentRollup, StatusSegments};
    use crate::workbench::Plane;
    use mde_egui::egui;
    use mde_egui::Style;
    use std::path::Path;

    const SZ: egui::Vec2 = egui::Vec2::new(1280.0, 800.0);

    /// Drive ONE headless frame of the Start Menu over a stand-in surface (the
    /// dock/console tests' `drive_vdock`/`drive` idiom — the same
    /// `Context::run` path the DRM runner drives, minus the GPU).
    fn drive(
        ctx: &egui::Context,
        state: &mut StartMenuState,
        console: &mut ConsoleState,
        events: Vec<egui::Event>,
        size: egui::Vec2,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), size)),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = ui.button("surface");
            });
            // No taskbar modeled in this isolated fixture — `rail_h = 0.0`
            // preserves every existing test's "panel reaches the screen's
            // true bottom edge" fixture semantics exactly (see the dedicated
            // `win7_desktop_1_regression_a_nonzero_rail_height_reserves_room_
            // above_the_taskbar` test below for the reservation itself).
            start_menu_panel(ctx, state, console, 0.0);
        })
    }

    /// Drive `frames` quiet headless frames on the dock tests' 1280x800 size.
    fn run(
        ctx: &egui::Context,
        state: &mut StartMenuState,
        console: &mut ConsoleState,
        frames: usize,
    ) {
        for _ in 0..frames {
            drive(ctx, state, console, Vec::new(), SZ);
        }
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

    fn press_at(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn release_at(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn secondary_press_at(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }
    }

    fn secondary_release_at(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// Click `center` — press one frame, release the next (the dock/console
    /// tests' click model). The caller primes the layout first.
    fn click(
        ctx: &egui::Context,
        state: &mut StartMenuState,
        console: &mut ConsoleState,
        center: egui::Pos2,
        size: egui::Vec2,
    ) {
        drive(
            ctx,
            state,
            console,
            vec![egui::Event::PointerMoved(center), press_at(center)],
            size,
        );
        drive(ctx, state, console, vec![release_at(center)], size);
    }

    fn secondary_click(
        ctx: &egui::Context,
        state: &mut StartMenuState,
        console: &mut ConsoleState,
        center: egui::Pos2,
        size: egui::Vec2,
    ) -> egui::FullOutput {
        drive(
            ctx,
            state,
            console,
            vec![
                egui::Event::PointerMoved(center),
                secondary_press_at(center),
            ],
            size,
        );
        drive(
            ctx,
            state,
            console,
            vec![secondary_release_at(center)],
            size,
        )
    }

    /// The Start Menu's floating-Area `LayerId`.
    fn start_menu_layer() -> egui::LayerId {
        egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new(super::START_MENU_AREA),
        )
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

    fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
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

    /// A minimal real [`crate::status::SegmentRollup`] at a given severity —
    /// every field but `severity` is a placeholder, mirroring `status.rs`'s
    /// own private `rollup()` test helper (not reusable across modules) for
    /// the fields WIN7-4's tint fold ([`super::severity_rank`]/
    /// `status::severity_color`) actually reads.
    fn segment_rollup(severity: &str) -> SegmentRollup {
        SegmentRollup {
            segment: "device".to_string(),
            severity: severity.to_string(),
            source: "test".to_string(),
            summary: "test summary".to_string(),
            host: "test-host".to_string(),
            critical_policy: String::new(),
            ts_unix_ms: 0,
        }
    }

    // ── open/close (lock #13: Super tap + Start-cell click, both) ───────────

    #[test]
    fn the_start_menu_toggle_opens_and_a_second_toggle_closes_it() {
        // The pure state contract both trigger paths (the Start-cell click,
        // drained in `main.rs`'s `mount_start_menu`; a clean Super tap, drained
        // in the hotkey dispatch block alongside VDOCK-1's own dock toggle —
        // see the module doc) fold into: a toggle opens, a second toggle
        // (either trigger) closes (lock #13's "both, not either/or").
        let mut s = StartMenuState::default();
        assert!(!s.is_open(), "closed by default");
        s.toggle();
        assert!(s.is_open(), "a toggle opens the Start Menu");
        s.toggle();
        assert!(!s.is_open(), "a second toggle closes it");
    }

    // ── geometry (lock #2: fixed-size, bottom-left-anchored, not full-screen) ─

    #[test]
    fn a_closed_start_menu_mounts_no_layer_and_an_open_one_claims_its_fixed_bottom_left_footprint()
    {
        // Lock #2 — fixed-size (never full-screen, never resizable): the
        // constants themselves ARE the whole footprint (the
        // `the_vertical_dock_is_a_48px_full_height_column` precedent — assert
        // directly on the compile-time geometry, no runtime resize path
        // exists to test against).
        assert!(
            (PANEL_H - 576.0).abs() < f32::EPSILON,
            "the panel reuses Console's existing settled height"
        );
        assert!(
            PANEL_W < SZ.x,
            "narrower than a real screen — never full width"
        );
        assert!(
            PANEL_H < SZ.y,
            "shorter than a real screen — never full height"
        );

        // Closed + settled -> no layer at all (the dock/console passthrough
        // guarantee): input over the panel's would-be footprint reaches the
        // surface beneath.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        run(&ctx, &mut s, &mut console, 2);
        let inside = egui::pos2(10.0, SZ.y - 10.0);
        assert_ne!(
            ctx.layer_id_at(inside),
            Some(start_menu_layer()),
            "a CLOSED Start Menu must not float an intercepting layer"
        );

        // Open on a fresh context (the slide latch settles at the open
        // endpoint on first sight, the console.rs precedent) -> claims exactly
        // its bottom-left footprint, anchored to the true screen-left edge.
        let ctx2 = egui::Context::default();
        Style::install(&ctx2);
        let mut s2 = StartMenuState::default();
        let mut console2 = ConsoleState::with_store(None);
        s2.toggle();
        run(&ctx2, &mut s2, &mut console2, 1);
        assert_eq!(
            ctx2.layer_id_at(inside),
            Some(start_menu_layer()),
            "an OPEN Start Menu claims its bottom-left footprint"
        );
    }

    #[test]
    fn the_open_start_menu_does_not_cover_the_rest_of_the_screen() {
        // Lock #2 (not full-screen) + the design's "it overlays, never hides
        // the active surface behind it": the top-right corner (far from the
        // bottom-left footprint) and a point immediately to the right of the
        // panel both stay unclaimed while the Start Menu is open.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 1);

        let top_right = egui::pos2(SZ.x - 10.0, 10.0);
        assert_ne!(
            ctx.layer_id_at(top_right),
            Some(start_menu_layer()),
            "the Start Menu must not blanket the whole screen"
        );
        let right_of_panel = egui::pos2(PANEL_W + 10.0, SZ.y - 10.0);
        assert_ne!(
            ctx.layer_id_at(right_of_panel),
            Some(start_menu_layer()),
            "the panel must not extend past its fixed width"
        );
    }

    #[test]
    fn win7_desktop_1_regression_a_nonzero_rail_height_reserves_room_above_the_taskbar() {
        // Regression companion to dock.rs's own WIN7-DESKTOP-1 regression-fix
        // tests: fixing the taskbar rail's absolute position (it now really
        // renders flush with the screen's bottom edge, instead of floating
        // uselessly near the top) exposed a second, latent bug this test
        // covers — `start_menu_panel`'s own slide-up anchor never reserved any
        // room for the rail, so its footprint always extended to the literal
        // screen bottom regardless of `rail_h`. That was invisible before the
        // taskbar fix (the rail was nowhere near the Start Menu's footprint)
        // and would become a REAL visible overlap the moment the taskbar
        // started rendering where it belongs — the taskbar sitting on top of
        // (or under) the last slice of the Start Menu's right pane, exactly
        // where its Power section anchors (lock #11's "the pane's TRUE bottom
        // edge"). Proves the fix: a point just inside the reserved band (near
        // the screen's true bottom) is no longer part of the panel's
        // footprint, while a point just above that band still is — the panel
        // now stops flush at the taskbar's own top edge, matching a true Win7
        // Start Menu that never overlaps the taskbar (lock #1).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        let rail_h = 40.0;
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SZ)),
            ..Default::default()
        };
        for _ in 0..2 {
            let _ = ctx.run(input(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = ui.button("surface");
                });
                start_menu_panel(ctx, &mut s, &mut console, rail_h);
            });
        }

        let inside_the_reserved_band = egui::pos2(10.0, SZ.y - rail_h / 2.0);
        assert_ne!(
            ctx.layer_id_at(inside_the_reserved_band),
            Some(start_menu_layer()),
            "the Start Menu must not extend into the reserved taskbar band \
             (the last {rail_h}pt above the screen's true bottom edge)"
        );
        let just_above_the_band = egui::pos2(10.0, SZ.y - rail_h - 10.0);
        assert_eq!(
            ctx.layer_id_at(just_above_the_band),
            Some(start_menu_layer()),
            "the Start Menu's footprint must still reach flush up to the top \
             of the reserved taskbar band, not shrink further than necessary"
        );
    }

    // ── dismiss (Esc / click-away / an embedded action closing for real) ────

    #[test]
    fn esc_and_click_away_close_the_start_menu_but_never_on_the_opening_frame() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 1);
        assert!(s.is_open());
        drive(&ctx, &mut s, &mut console, vec![key(egui::Key::Escape)], SZ);
        assert!(!s.is_open(), "Esc dismisses the Start Menu");

        let ctx2 = egui::Context::default();
        Style::install(&ctx2);
        let mut s2 = StartMenuState::default();
        let mut console2 = ConsoleState::with_store(None);
        s2.toggle();
        // The very frame the trigger opened it: its click lands outside the
        // panel — the guard must swallow it (the power-menu / Console
        // `just_toggled` idiom).
        let far = egui::pos2(SZ.x - 40.0, 40.0);
        drive(
            &ctx2,
            &mut s2,
            &mut console2,
            vec![egui::Event::PointerMoved(far), release_at(far)],
            SZ,
        );
        assert!(s2.is_open(), "the opening click must not self-dismiss");
        run(&ctx2, &mut s2, &mut console2, 1); // settle
        click(&ctx2, &mut s2, &mut console2, far, SZ);
        assert!(!s2.is_open(), "a click away dismisses the Start Menu");
    }

    #[test]
    fn activating_an_embedded_console_row_closes_the_whole_start_menu() {
        // Proves the WIN7-2 embedding actually works end-to-end, not just
        // architecturally: a click on the embedded Console's pinned Terminal
        // row (the right pane) fires the SAME `ConsoleRequest` it always has,
        // AND closes the WHOLE Start Menu — the self-closure propagation the
        // module doc describes — not just Console's own mirrored `open` bit.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);
        let row = ctx
            .read_response(console::console_entry_id(0))
            .expect("the embedded pinned Terminal row is registered")
            .rect;
        click(&ctx, &mut s, &mut console, row.center(), SZ);
        assert_eq!(
            console.take_request(),
            Some(console::ConsoleRequest::Goto(
                crate::dock::Surface::Terminal
            )),
            "the embedded row still fires the real Console request"
        );
        assert!(!console.is_open(), "Console's own close() fired, unchanged");
        assert!(
            !s.is_open(),
            "...and the Start Menu propagated that self-closure to the WHOLE panel"
        );
    }

    // ── WIN7-3: live tiles (locks #6/#7/#8/#23) ──────────────────────────────

    #[test]
    fn the_19_surfaces_are_grouped_into_shared_function_based_groups() {
        // The shared launcher taxonomy + order, consumed from the shared launcher
        // table and restated here so this pane's layout contract stays visible.
        use Surface::{
            About, Bookmarks, Browser, Chat, Desktop, Editor, Explorer, Files, InfraCode,
            MapsLocation, Media, MeshView, Music, Phones, Storage, System, Terminal, Voice,
            Workbench,
        };
        let expect: [(&str, &[Surface]); 8] = [
            ("Mesh Control", &[Workbench, MeshView, InfraCode]),
            ("Desktop & Session", &[Desktop, MapsLocation]),
            ("Media", &[Music, Media]),
            ("Files & Data", &[Files, Storage]),
            ("Web", &[Browser, Bookmarks]),
            ("Developer Tools", &[Terminal, Editor]),
            ("Comms", &[Voice, Chat, Phones]),
            ("System", &[System, About, Explorer]),
        ];
        assert_eq!(
            super::TILE_GROUPS.len(),
            expect.len(),
            "shared launcher group count"
        );
        for (g, (label, surfaces)) in super::TILE_GROUPS.iter().zip(expect) {
            assert_eq!(g.label, label, "group order");
            assert_eq!(
                g.surfaces, surfaces,
                "{label} membership + within-group order"
            );
        }
        // The shared taxonomy places ALL 19 Surface::ALL entries inside a
        // group, none outside. The compile-time guard above already enforces
        // "exactly once"; re-prove it here at runtime too.
        let mut placed: Vec<Surface> = Vec::new();
        for g in &super::TILE_GROUPS {
            placed.extend_from_slice(g.surfaces);
        }
        assert_eq!(
            placed.len(),
            Surface::ALL.len(),
            "every surface placed once"
        );
        for surface in Surface::ALL {
            assert_eq!(
                placed.iter().filter(|&&s| s == surface).count(),
                1,
                "{surface:?} must be placed in exactly one tile group"
            );
        }
    }

    #[test]
    fn start_tiles_use_the_shared_launcher_taxonomy_source() {
        assert_eq!(
            super::TILE_GROUPS,
            crate::dock::LAUNCHER_GROUPS,
            "Start must consume the shared launcher taxonomy, not a local copy"
        );
        for surface in Surface::ALL {
            assert_eq!(
                super::tile_group_label(surface),
                crate::dock::launcher_group_label(surface),
                "{surface:?} group label drifted between Start and dock"
            );
            assert_eq!(
                super::tile_group_accent(surface),
                crate::dock::launcher_group_accent(surface),
                "{surface:?} group accent drifted between Start and dock"
            );
        }
        assert_eq!(super::tile_group_label(Surface::Browser), "Web");
        assert_eq!(super::tile_group_label(Surface::Bookmarks), "Web");
        assert_eq!(
            super::tile_group_label(Surface::Terminal),
            "Developer Tools"
        );
        assert_eq!(super::tile_group_label(Surface::Files), "Files & Data");
        assert_ne!(super::tile_group_label(Surface::Files), "System");
    }

    #[test]
    fn b6_grouped_grid_reaches_every_tileable_surface_once() {
        // WIN10-HYBRID B6 completion: the Start launcher grid is THE surface
        // launcher. Optional Pinned/search copies are convenience affordances,
        // but the grouped grid itself must contain every tileable surface once
        // and only once.
        let grouped = super::grouped_grid_surfaces();
        assert_eq!(
            grouped.len(),
            Surface::ALL.len(),
            "the grouped grid must expose exactly the tileable surface roster"
        );
        for surface in Surface::ALL {
            assert_eq!(
                grouped.iter().filter(|&&s| s == surface).count(),
                1,
                "{surface:?} must be reachable exactly once from the grouped grid"
            );
            assert!(
                !super::tile_group_label(surface).is_empty(),
                "{surface:?} must have a visible group label"
            );
            assert!(
                super::tile_group_accent(surface).is_some(),
                "{surface:?} must inherit a category accent"
            );
        }
        assert!(
            !grouped.contains(&Surface::Timers),
            "Timers remains clock/taskbar-owned, not a duplicate Start-grid tile"
        );
    }

    #[test]
    fn start_menu_pin_preferences_keep_valid_tile_surfaces_once() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("start-menu.json");
        std::fs::write(
            &path,
            r#"{"pinned":["browser","timers","files","browser","unknown","mesh_view"]}"#,
        )
        .expect("write hand-edited prefs");

        let loaded = StartMenuState::load_from(path.clone());
        assert_eq!(
            loaded.pinned,
            vec![Surface::Browser, Surface::Files, Surface::MeshView],
            "load drops non-grid surfaces, unknown ids, and duplicate pins"
        );

        StartMenuPrefs::from_pins(&[
            Surface::Browser,
            Surface::Timers,
            Surface::Files,
            Surface::Browser,
        ])
        .save_to(&path)
        .expect("save normalized prefs");
        assert_eq!(
            StartMenuPrefs::load_from(&path).into_pins(),
            vec![Surface::Browser, Surface::Files],
            "save writes only persisted tile-surface pins once"
        );
    }

    #[test]
    fn b6_start_grid_uses_square_chrome_and_distinct_group_accents() {
        // WIN10-HYBRID explicitly makes taskbar + Start chrome square while
        // surfaces keep the shared radius tiers. Pin that locally so this file
        // does not drift back to `Style::RADIUS` for Start-owned chrome.
        assert_eq!(super::START_CHROME_RADIUS, 0.0);
        assert!(
            super::START_ACCENT_W > 0.0 && super::START_ACCENT_W < Style::SP_XS,
            "category/state stripes should be visible but compact"
        );

        let mut accents = Vec::new();
        for group in &super::TILE_GROUPS {
            assert_ne!(
                group.accent,
                Style::SURFACE,
                "{} needs a visible category accent, not the panel fill",
                group.label
            );
            assert_ne!(
                group.accent,
                Style::BORDER,
                "{} needs a category accent, not a separator tone",
                group.label
            );
            assert!(
                !accents.contains(&group.accent),
                "{} reuses another group's accent; B6 wants scannable sections",
                group.label
            );
            accents.push(group.accent);
        }
        assert_eq!(accents.len(), super::TILE_GROUPS.len());
    }

    #[test]
    fn the_tile_grid_content_fits_the_shared_panel_height_without_overflow() {
        // The panel is fixed-size (lock #2) and shares ONE height across
        // both panes (`PANEL_H`) — the tile grid's own derived content
        // height, plus its top/bottom `PANE_PAD` inset, must fit inside it.
        // Asserted directly on the compile-time geometry (the WIN7-2
        // `PANEL_H`/`PANEL_W` constant-assertion precedent), no
        // GPU/context needed.
        assert!(
            super::TILE_GRID_CONTENT_H + super::PANE_PAD * 2.0 <= PANEL_H,
            "the tile grid overflows the shared panel height"
        );
        // The widest locked group (3 members) is what `LEFT_PANE_W`'s
        // literal `3.0`/`2.0` and `TILE_GRID_CONTENT_H`'s "one row per
        // group" literal `7.0`/`6.0` both depend on — pin the assumption so
        // a future `TILE_GROUPS` edit that breaks it fails a test, not a
        // silently wrong layout.
        assert_eq!(super::TILE_COLUMNS, 3);
        assert!(
            super::TILE_GROUPS
                .iter()
                .all(|g| g.surfaces.len() <= super::TILE_COLUMNS),
            "a group wider than TILE_COLUMNS wraps to a second row, which \
             TILE_GRID_CONTENT_H's literal derivation does not account for"
        );
    }

    #[test]
    fn the_consoles_jump_row_height_matches_this_panes_own_tile_height() {
        // WIN7-5's `console::JUMP_ROW_H` doc comment claims it is
        // "deliberately the SAME value as `start_menu::TILE_H`" so the
        // rail's nav rows line up with this pane's own tiles (one visual
        // rhythm across the whole Start Menu). `console.rs` cannot import
        // `TILE_H` itself (it sits lower in the module graph than
        // `start_menu.rs`, which embeds it — a cycle), so the claim was
        // only ever prose on two independently-edited constants; pin it
        // here, the module that CAN see both, rather than trusting it
        // stays true by eye across a future edit to either one.
        assert!(
            (console::JUMP_ROW_H - super::TILE_H).abs() < f32::EPSILON,
            "console::JUMP_ROW_H ({}) must match start_menu::TILE_H ({}) so \
             the rail's jump rows visually line up with the tile grid beside them",
            console::JUMP_ROW_H,
            super::TILE_H,
        );
    }

    #[test]
    fn all_19_tiles_render_at_one_uniform_size_and_stay_within_the_left_pane() {
        // Lock #6 — one uniform tile size for all 19, no variants — proven
        // on REAL rendered rects (the addressable-cell idiom via
        // `tile_id`), not just on the shared constants two tiles happen to
        // both reference.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);

        let left_pane_right_edge = super::LEFT_PANE_W;
        for surface in Surface::ALL {
            let rect = ctx
                .read_response(super::tile_id(surface))
                .unwrap_or_else(|| panic!("{surface:?} tile is not registered"))
                .rect;
            assert!(
                (rect.width() - super::TILE_W).abs() < 0.01,
                "{surface:?} tile width drifted from the uniform TILE_W"
            );
            assert!(
                (rect.height() - super::TILE_H).abs() < 0.01,
                "{surface:?} tile height drifted from the uniform TILE_H"
            );
            assert!(
                rect.right() <= left_pane_right_edge + 0.01,
                "{surface:?} tile overflows the left pane's right edge"
            );
        }
    }

    #[test]
    fn clicking_a_tile_activates_its_surface_and_closes_the_whole_start_menu() {
        // Lock #23 — a single click activates, mirroring the embedded
        // Console pane's own click-routes-and-closes contract (proven above
        // in `activating_an_embedded_console_row_closes_the_whole_start_menu`)
        // — proven here for the OTHER pane's tiles. Picks a tile that is
        // neither the default surface nor the first tile in its group, so
        // the assertion actually distinguishes "the right one" from "any
        // one."
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);
        let rect = ctx
            .read_response(super::tile_id(Surface::Phones))
            .expect("the Phones tile is registered")
            .rect;
        click(&ctx, &mut s, &mut console, rect.center(), SZ);
        assert_eq!(
            s.take_tile_activation(),
            Some(Surface::Phones),
            "the clicked tile's surface is recorded for main.rs to route"
        );
        assert!(
            !s.is_open(),
            "a tile click closes the whole Start Menu, matching the embedded \
             Console pane's own activation contract"
        );
    }

    // ── accesskit (lock #14) ─────────────────────────────────────────────────

    #[test]
    fn the_panel_exports_a_menu_role_and_labelled_panes_before_any_content_lands() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);

        let menu = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Start Menu"))
            .expect("Start Menu node");
        assert_eq!(menu.role(), egui::accesskit::Role::Menu);

        let tiles = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Start Menu tiles"))
            .expect("tiles pane node");
        assert_eq!(tiles.role(), egui::accesskit::Role::Group);

        let console_pane = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Console"))
            .expect("console pane node");
        assert_eq!(console_pane.role(), egui::accesskit::Role::Group);
    }

    #[test]
    fn every_tile_exports_a_labelled_button_role_for_accesskit() {
        // Lock #14 — "every tile", not just the panel level proven above
        // (`console.rs`'s own rows export none yet — WIN7-7's later full
        // sweep — so a tile's label is unambiguous among this frame's
        // exported nodes).
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);

        for surface in Surface::ALL {
            let node = nodes
                .iter()
                .map(|(_, n)| n)
                .find(|n| n.label() == Some(surface.label()))
                .unwrap_or_else(|| panic!("{surface:?} tile exports no accesskit node"));
            assert_eq!(
                node.role(),
                egui::accesskit::Role::Button,
                "{surface:?} tile's accesskit role"
            );
        }
    }

    #[test]
    fn tile_context_menu_rows_export_labelled_button_roles_for_accesskit() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);
        let rect = ctx
            .read_response(super::tile_id(Surface::Browser))
            .expect("the Browser tile is registered")
            .rect;
        let _ = secondary_click(&ctx, &mut s, &mut console, rect.center(), SZ);
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);

        for label in ["Open", "Pin to top"] {
            let node = nodes
                .iter()
                .map(|(_, n)| n)
                .find(|n| n.label() == Some(label))
                .unwrap_or_else(|| panic!("context menu row {label:?} exports no accesskit node"));
            assert_eq!(
                node.role(),
                egui::accesskit::Role::Button,
                "{label:?} context menu row role"
            );
        }
    }

    // ── WIN7-4: live-tile rotation (lock #5) ─────────────────────────────────

    #[test]
    fn a_surface_with_two_real_facts_reports_both_for_rotation() {
        // Chat: total_unread() + most_recent_sender() are BOTH real,
        // already-wired sources (dock.rs's own Chat badge; chat.rs's own
        // conversation store) — the len()>=2 rotating case lock #5 asks for,
        // and lock #5's own literal example ("Chat cycles recent senders").
        let inputs = super::TileFactInputs {
            chat_unread: 3,
            chat_recent_sender: Some("eagle".to_string()),
            ..super::TileFactInputs::default()
        };
        assert_eq!(
            super::tile_facts(Surface::Chat, &inputs),
            vec!["3 unread".to_string(), "Latest: eagle".to_string()]
        );
    }

    #[test]
    fn a_surface_with_exactly_one_real_fact_shows_it_without_rotating() {
        // Bookmarks: Manager::total() is real + already-Bus-fed, but there is
        // only ONE genuine fact to show — it must not rotate against itself.
        let inputs = super::TileFactInputs {
            bookmarks_total: 42,
            ..super::TileFactInputs::default()
        };
        let facts = super::tile_facts(Surface::Bookmarks, &inputs);
        assert_eq!(facts, vec!["42 bookmarks".to_string()]);
        for t in [0.0, 5.0, 50.0, 500.0] {
            assert_eq!(
                super::tile_display_text(Surface::Bookmarks, &facts, t, true),
                "42 bookmarks",
                "a single fact never advances, at any point on the clock"
            );
        }
    }

    #[test]
    fn a_currently_zero_count_shows_no_fact_not_a_fake_zero() {
        // Files idle (no active transfers): "zero is meaningful silence" —
        // the SAME convention dock.rs's own Files badge already follows
        // (`badge_for`'s `Surface::Files if transfer_active_count > 0` gate) —
        // restated per-fact here, never a painted "0 transferring".
        let idle = super::TileFactInputs {
            files_active_transfers: 0,
            ..super::TileFactInputs::default()
        };
        assert!(super::tile_facts(Surface::Files, &idle).is_empty());
        let busy = super::TileFactInputs {
            files_active_transfers: 2,
            ..super::TileFactInputs::default()
        };
        assert_eq!(
            super::tile_facts(Surface::Files, &busy),
            vec!["2 transferring".to_string()]
        );
    }

    #[test]
    fn surfaces_verified_to_have_no_live_fact_anywhere_never_rotate() {
        // About (100% compile-time build info) and Editor (no aggregate
        // open/dirty count exists anywhere in mde-editor-egui) — verified by
        // this unit's own investigation, not assumed. Even a fully-defaulted
        // bundle must never invent a fact for either: `tile_facts` never
        // reads a single `inputs` field for these two arms, so any input
        // (default or otherwise) yields the same honest empty answer.
        let inputs = super::TileFactInputs::default();
        assert!(super::tile_facts(Surface::About, &inputs).is_empty());
        assert!(super::tile_facts(Surface::Editor, &inputs).is_empty());
        assert_eq!(
            super::tile_display_text(Surface::About, &[], 0.0, true),
            Surface::About.label(),
            "0 facts paints the plain static label — unchanged WIN7-3 behaviour"
        );
    }

    #[test]
    fn tile_facts_covers_a_spread_of_the_other_wired_surfaces() {
        // A spot-check across the remaining source shapes (tuple/Option
        // fields, the cross-crate Music/Voice accessors) so a wiring mistake
        // in any one surface's match arm doesn't hide behind only testing
        // Chat/Bookmarks/Files above.
        let storage = super::TileFactInputs {
            storage_local: Some((2, 10240)),
            ..super::TileFactInputs::default()
        };
        assert_eq!(
            super::tile_facts(Surface::Storage, &storage),
            vec!["2 disks".to_string(), "10 GiB free".to_string()]
        );
        assert!(super::tile_facts(Surface::Storage, &super::TileFactInputs::default()).is_empty());

        let media = super::TileFactInputs {
            media_title: Some("Some Track".to_string()),
            media_playing: true,
            ..super::TileFactInputs::default()
        };
        assert_eq!(
            super::tile_facts(Surface::Media, &media),
            vec!["Some Track".to_string(), "Playing".to_string()]
        );
        assert!(super::tile_facts(Surface::Media, &super::TileFactInputs::default()).is_empty());

        let music = super::TileFactInputs {
            music_now_playing: Some(("A Song".to_string(), "An Artist".to_string())),
            ..super::TileFactInputs::default()
        };
        assert_eq!(
            super::tile_facts(Surface::Music, &music),
            vec!["A Song".to_string(), "by An Artist".to_string()]
        );

        let voice = super::TileFactInputs {
            voice_call_label: Some("In call · bob".to_string()),
            ..super::TileFactInputs::default()
        };
        assert_eq!(
            super::tile_facts(Surface::Voice, &voice),
            vec!["In call · bob".to_string()]
        );
        assert!(super::tile_facts(Surface::Voice, &super::TileFactInputs::default()).is_empty());

        let workbench = super::TileFactInputs {
            workbench_seen: true,
            workbench_peer_count: 4,
            workbench_leader: Some("sfo3".to_string()),
            ..super::TileFactInputs::default()
        };
        assert_eq!(
            super::tile_facts(Surface::Workbench, &workbench),
            vec!["4 peers".to_string(), "leader sfo3".to_string()]
        );

        let browser = super::TileFactInputs {
            browser_tabs: 5,
            ..super::TileFactInputs::default()
        };
        assert_eq!(
            super::tile_facts(Surface::Browser, &browser),
            vec!["5 tabs".to_string()]
        );

        let terminal_absent = super::TileFactInputs {
            terminal_tabs: None,
            ..super::TileFactInputs::default()
        };
        assert!(
            super::tile_facts(Surface::Terminal, &terminal_absent).is_empty(),
            "no live terminal (the first PTY was refused) shows no fact"
        );
    }

    #[test]
    fn rotating_fact_index_advances_every_locked_interval_and_wraps() {
        let interval = super::TILE_FACT_ROTATE_INTERVAL.as_secs_f64();
        assert_eq!(super::rotating_fact_index(3, 0.0, true), 0);
        assert_eq!(super::rotating_fact_index(3, interval - 0.01, true), 0);
        assert_eq!(super::rotating_fact_index(3, interval, true), 1);
        assert_eq!(super::rotating_fact_index(3, interval * 2.0, true), 2);
        assert_eq!(
            super::rotating_fact_index(3, interval * 3.0, true),
            0,
            "wraps back to the first fact after a full cycle"
        );
    }

    #[test]
    fn rotating_fact_index_never_advances_a_0_or_1_fact_list() {
        for t in [0.0, 1.0, 100.0, 1000.0] {
            assert_eq!(super::rotating_fact_index(0, t, true), 0);
            assert_eq!(super::rotating_fact_index(1, t, true), 0);
        }
    }

    #[test]
    fn reduced_motion_freezes_multi_fact_tiles_on_the_primary_fact() {
        let facts = vec!["3 unread".to_string(), "Latest: eagle".to_string()];
        let interval = super::TILE_FACT_ROTATE_INTERVAL.as_secs_f64();
        assert_eq!(
            super::rotating_fact_index(2, interval * 7.0, false),
            0,
            "reduced/disabled motion must not advance the rotation clock"
        );
        assert_eq!(
            super::tile_display_text(Surface::Chat, &facts, interval * 7.0, false),
            "3 unread",
            "a frozen tile holds its primary live fact instead of cycling"
        );
    }

    #[test]
    fn tile_display_text_rotates_through_2plus_facts_on_the_locked_interval() {
        let facts = vec!["3 unread".to_string(), "Latest: eagle".to_string()];
        let interval = super::TILE_FACT_ROTATE_INTERVAL.as_secs_f64();
        assert_eq!(
            super::tile_display_text(Surface::Chat, &facts, 0.0, true),
            "3 unread"
        );
        assert_eq!(
            super::tile_display_text(Surface::Chat, &facts, interval, true),
            "Latest: eagle"
        );
        assert_eq!(
            super::tile_display_text(Surface::Chat, &facts, interval * 2.0, true),
            "3 unread",
            "wraps back to the first fact"
        );
    }

    #[test]
    fn tile_status_tint_paints_severity_where_meaningful_and_nothing_elsewhere() {
        // System: the worse of Device/Power segment severity wins the tile's
        // tint — the SAME `Style::SUPPORT_*` tone `status.rs`'s own segment
        // pips already use for the identical rollups.
        let system = super::TileFactInputs {
            segments: StatusSegments {
                device: Some(segment_rollup("warning")),
                power: Some(segment_rollup("critical")),
                seen: true,
                ..StatusSegments::default()
            },
            ..super::TileFactInputs::default()
        };
        assert_eq!(
            super::tile_status_tint(Surface::System, &system),
            Some(Style::SUPPORT_ERROR),
            "critical Power outranks warning Device"
        );
        assert_eq!(
            super::tile_status_tint(Surface::System, &super::TileFactInputs::default()),
            None,
            "no segment has landed yet — the honest pre-poll answer"
        );

        // Chat/Files: an accent dot exactly when their count badge would
        // paint one (`dock.rs`'s own `paint_count_badge` tone), `None` when
        // quiet — never an invented severity for a plain count.
        let chat_unread = super::TileFactInputs {
            chat_unread: 1,
            ..super::TileFactInputs::default()
        };
        assert_eq!(
            super::tile_status_tint(Surface::Chat, &chat_unread),
            Some(Style::ACCENT)
        );
        assert_eq!(
            super::tile_status_tint(Surface::Chat, &super::TileFactInputs::default()),
            None
        );

        // A plain count-only surface (no health/severity concept at all)
        // never gets a tint, no matter how large the count.
        let bookmarks = super::TileFactInputs {
            bookmarks_total: 999,
            ..super::TileFactInputs::default()
        };
        assert_eq!(
            super::tile_status_tint(Surface::Bookmarks, &bookmarks),
            None
        );
    }

    // ── WIN7-4: accesskit live-region (lock #14) ─────────────────────────────

    #[test]
    fn the_tile_grid_exports_one_live_polite_summary_while_a_tile_is_rotating() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        s.set_tile_inputs(super::TileFactInputs {
            chat_unread: 3,
            chat_recent_sender: Some("eagle".to_string()),
            ..super::TileFactInputs::default()
        });
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);

        let live = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Start Menu live tiles"))
            .expect("a live region exports once a tile is rotating");
        assert_eq!(live.role(), egui::accesskit::Role::Status);
        assert_eq!(live.live(), Some(egui::accesskit::Live::Polite));
        let value = live.value().expect("live summary value");
        assert!(
            value.contains("Chat: "),
            "names the rotating surface: {value}"
        );
    }

    #[test]
    fn the_tile_grid_exports_no_live_region_when_nothing_is_rotating() {
        // The default bundle: every surface has 0-1 facts, so nothing is
        // ACTUALLY rotating — an honest silence, not a live region that
        // politely announces nothing.
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);
        assert!(
            !nodes
                .iter()
                .any(|(_, n)| n.label() == Some("Start Menu live tiles")),
            "no live-data bundle was ever set — no live region should export"
        );
    }

    #[test]
    fn reduced_motion_suppresses_the_rotating_tile_live_region() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.0);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        s.set_tile_inputs(super::TileFactInputs {
            chat_unread: 3,
            chat_recent_sender: Some("eagle".to_string()),
            ..super::TileFactInputs::default()
        });
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);
        assert!(
            !nodes
                .iter()
                .any(|(_, n)| n.label() == Some("Start Menu live tiles")),
            "a frozen tile should not export a live region because nothing changes"
        );
    }

    #[test]
    fn a_tiles_accesskit_node_carries_its_current_display_text_as_its_value() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        s.set_tile_inputs(super::TileFactInputs {
            bookmarks_total: 42,
            ..super::TileFactInputs::default()
        });
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);

        let bookmarks = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some(Surface::Bookmarks.label()))
            .expect("Bookmarks tile node");
        assert_eq!(bookmarks.value(), Some("42 bookmarks"));

        // A tile with genuinely no live fact still carries its static label
        // as BOTH its accesskit label and value (never a blank/omitted
        // value) — unchanged content, just also mirrored into `.value()`.
        let about = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some(Surface::About.label()))
            .expect("About tile node");
        assert_eq!(about.value(), Some(Surface::About.label()));
    }

    #[test]
    fn pinned_tile_accesskit_value_names_the_shortcut_copy() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        s.pinned = vec![Surface::Browser];
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);
        let browser_values: Vec<_> = nodes
            .iter()
            .map(|(_, n)| n)
            .filter(|n| n.label() == Some(Surface::Browser.label()))
            .filter_map(|n| n.value())
            .collect();

        assert!(
            browser_values.contains(&"Pinned shortcut, Browser"),
            "the pinned copy must be distinguishable from the grouped Browser tile: {browser_values:?}"
        );
        assert!(
            browser_values.contains(&"Browser"),
            "the grouped Browser tile keeps the normal value: {browser_values:?}"
        );
    }

    // ── WIN7-7: the open/close transition itself is announced (lock #14) ────
    // Before this unit the panel exported a `Role::Menu` landmark (proven
    // above) but NOTHING announced the transition into/out of existing — a
    // screen reader user would only discover the menu by independently
    // exploring the tree, never by an actual announcement the way this
    // crate's other state changes (NOTIF-6's critical alert, WIN7-4's tile
    // rotation, WIN7-5's honest-gate notice) already work.

    #[test]
    fn opening_the_start_menu_announces_it_via_a_live_region() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);

        let status = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Start Menu status"))
            .expect("a live status region announces the Start Menu opening");
        assert_eq!(status.role(), egui::accesskit::Role::Status);
        assert_eq!(status.live(), Some(egui::accesskit::Live::Polite));
        assert_eq!(status.value(), Some("Start Menu opened"));
    }

    #[test]
    fn closing_the_start_menu_announces_it_before_the_panel_fully_unmounts() {
        // Motion::animate uses egui's own `predicted_dt` (1/60s, deterministic
        // — NOT real wall-clock time, since `drive`'s RawInput leaves `time`
        // as `None`) against `Motion::BASE` (0.18s), so one frame after
        // closing is nowhere near settled — the panel is still mid-tween,
        // exactly when this announcement needs to fire.
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2); // settle open
        s.close();
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ); // one closing-tween frame
        let nodes = accesskit_nodes(&out);

        let status = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Start Menu status"))
            .expect("a live status region announces the Start Menu closing, mid-tween");
        assert_eq!(status.value(), Some("Start Menu closed"));
    }

    #[test]
    fn a_fully_settled_closed_start_menu_exports_no_state_announcement() {
        // Never opened — settled closed from the very first frame (the
        // `a_closed_start_menu_mounts_no_layer...` precedent's own starting
        // state): nothing to announce, the honest-silence posture
        // `install_tiles_live_summary` already uses elsewhere in this file.
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);
        assert!(
            !nodes
                .iter()
                .any(|(_, n)| n.label() == Some("Start Menu status")),
            "nothing to announce once fully closed and settled"
        );
    }

    // ── SHELL-UX-3: type-to-launch search ────────────────────────────────────

    /// Feed typed text to the focused search box (the `Event::Text` a real key
    /// press turns into once a text field owns the keyboard).
    fn text(s: &str) -> egui::Event {
        egui::Event::Text(s.to_string())
    }

    fn search_surfaces(query: &str) -> Vec<Surface> {
        super::search_matches(query)
            .into_iter()
            .filter_map(|hit| match hit {
                super::StartSearchHit::Surface(surface) => Some(surface),
                super::StartSearchHit::Console(_) => None,
            })
            .collect()
    }

    fn search_console_labels(query: &str) -> Vec<&'static str> {
        super::search_matches(query)
            .into_iter()
            .filter_map(|hit| match hit {
                super::StartSearchHit::Surface(_) => None,
                super::StartSearchHit::Console(hit) => Some(hit.label),
            })
            .collect()
    }

    #[test]
    fn a_query_filters_to_the_tiles_whose_label_matches() {
        // The pure ranking authority both the render and Enter read (so the
        // painted list and Enter's target can never diverge). Case-insensitive
        // substring over Surface::label().
        use Surface::{Phones, Terminal, Workbench};
        assert_eq!(search_surfaces("phone"), vec![Phones]);
        assert_eq!(search_surfaces("term"), vec![Terminal]);
        assert_eq!(search_surfaces("workbench"), vec![Workbench]);
        assert_eq!(
            search_surfaces("PHONES"),
            vec![Phones],
            "matching is case-insensitive"
        );
        assert!(
            super::search_matches("zzzz").is_empty(),
            "a query that matches nothing yields no matches (an honest no-match, \
             not a silent full grid)"
        );
    }

    #[test]
    fn an_empty_or_whitespace_query_matches_nothing_so_the_full_grid_shows() {
        // Empty query == no search == the caller renders the unchanged grouped
        // grid (no behaviour change when not searching).
        assert!(super::search_matches("").is_empty());
        assert!(super::search_matches("   ").is_empty());
    }

    #[test]
    fn a_query_also_matches_console_static_entries() {
        assert_eq!(
            search_console_labels("cloud plane").first(),
            Some(&"Cloud Plane (GUI)"),
            "Console static entries join the Start Menu search result set"
        );
        assert!(
            search_console_labels("journal").contains(&"Live Logs"),
            "Console descriptions and labels are searchable"
        );
    }

    #[test]
    fn search_ranks_prefix_over_substring_over_group_name() {
        use Surface::{InfraCode, MeshView, Music, Workbench};
        // "mesh": a label PREFIX ("Mesh Map") outranks a GROUP-name-only hit
        // ("Mesh Control" → Workbench/InfraCode); ties keep Surface::ALL order.
        assert_eq!(
            search_surfaces("mesh"),
            vec![MeshView, Workbench, InfraCode],
            "the label-prefix hit leads, then the group-only hits in ALL order"
        );
        // "i": the sole label-PREFIX ("Infra as Code") ranks above every
        // label-SUBSTRING hit (Music/Media/Files/…), never buried among them.
        let by_i = search_surfaces("i");
        assert_eq!(by_i.first(), Some(&InfraCode), "the prefix match leads");
        assert!(
            by_i.len() > 1 && by_i.contains(&Music),
            "substring matches are still included, just ranked below the prefix"
        );
    }

    #[test]
    fn fuzzy_subsequence_finds_labels_no_substring_reaches() {
        // SEARCH-fuzzy: the query's chars appear in ORDER but not contiguous, so
        // the old substring tiers found nothing — the 4th tier does.
        use Surface::{Editor, MeshView};
        assert!(
            search_surfaces("edtr").contains(&Editor),
            "'edtr' is a subsequence of 'Editor' (e-d-i-t-o-r), so it fuzzy-matches"
        );
        assert!(
            search_surfaces("meshmp").contains(&MeshView),
            "'meshmp' is a subsequence of 'Mesh Map' across the space, so it fuzzy-matches"
        );
        assert!(
            search_surfaces("mm").contains(&MeshView),
            "'mm' is a subsequence of 'Mesh Map' (the two m's), so it fuzzy-matches"
        );
    }

    #[test]
    fn exact_prefix_still_outranks_a_fuzzy_hit() {
        use Surface::{Bookmarks, Browser, Editor};
        // The task's literal example: an exact PREFIX leads its result list, so
        // the fuzzy tier never displaces it.
        assert_eq!(
            search_surfaces("edi").first(),
            Some(&Editor),
            "'edi' is a prefix of 'Editor' — the prefix hit still leads"
        );
        // A query carrying BOTH an exact hit and a fuzzy-only hit: the prefix
        // ('Bookmarks') leads and the fuzzy subsequence ('Browser' = b..o, not a
        // substring of "browser") is included but pinned to the bottom.
        let bo = search_surfaces("bo");
        assert_eq!(bo.first(), Some(&Bookmarks), "the label-prefix hit leads");
        assert_eq!(
            bo.last(),
            Some(&Browser),
            "the fuzzy-only hit is kept but ranks below every exact tier"
        );
    }

    #[test]
    fn a_query_matching_nothing_even_fuzzily_returns_empty() {
        // No label contains a 'q' at all, so not even the fuzzy tier bites.
        assert!(super::search_matches("qq").is_empty());
        // Order matters: 'rotide' holds Editor's letters but out of order, so it
        // is NOT a subsequence — an honest no-match, not a fuzzy false positive.
        assert!(
            search_surfaces("rotide").is_empty(),
            "fuzzy matching is order-sensitive, not a bag-of-chars test"
        );
    }

    #[test]
    fn fuzzy_tie_break_prefers_the_tighter_match() {
        use Surface::{Browser, Storage};
        // 'sr' fuzzy-matches both 'Storage' (s..r, span 3) and 'Browser'
        // (s..r, span 2). Neither is a substring, so both land in the fuzzy
        // tier; the tighter (fewer-gaps) 'Browser' must sort first even though
        // 'Storage' starts earlier.
        assert_eq!(
            search_surfaces("sr"),
            vec![Browser, Storage],
            "among fuzzy hits the tighter match (fewer gaps) leads"
        );
    }

    #[test]
    fn the_search_field_tucks_below_the_tile_grid_without_overlap() {
        // Constant-geometry assertion (the WIN7-2 `PANEL_H`/`PANEL_W`
        // precedent, no GPU): the 7-group grid must bottom out strictly above
        // the search band, and all surface result rows must fit the results area at
        // once (worst case — a one-letter query matching every surface).
        let grid_bottom = super::PANE_PAD + super::TILE_GRID_CONTENT_H;
        let search_top = PANEL_H - super::PANE_PAD - super::SEARCH_H;
        assert!(
            grid_bottom <= search_top,
            "the tile grid ({grid_bottom}) must end above the search band ({search_top})"
        );
        let results_rows_h = search_top - Style::SP_XS - super::PANE_PAD;
        assert!(
            Surface::ALL.len() as f32 * super::RESULT_ROW_H <= results_rows_h,
            "all surface result rows must fit the results band"
        );
    }

    #[test]
    fn pinned_tiles_scroll_above_the_fixed_search_band() {
        let left_rect = egui::Rect::from_min_size(
            egui::pos2(0.0, SZ.y - PANEL_H),
            egui::vec2(super::LEFT_PANE_W, PANEL_H),
        );
        let search_rect = super::search_rect(left_rect);
        let content_rect = super::left_pane_content_rect(left_rect, search_rect);
        assert!(
            super::nav_sections_content_h(&super::nav_sections(&[]))
                <= content_rect.height() + 0.01,
            "the unpinned 7-group grid should fit above search without scrolling"
        );
        assert!(
            super::nav_sections_content_h(&super::nav_sections(&[Surface::Browser]))
                > content_rect.height() + 0.01,
            "one pinned row makes the left pane overflow, so the grid must scroll"
        );

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        s.pinned = vec![Surface::Browser];
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);

        let pinned = ctx
            .read_response(super::pinned_tile_id(Surface::Browser))
            .expect("the pinned Browser tile is registered")
            .rect;
        assert!(
            pinned.bottom() <= search_rect.top(),
            "the pinned section must stay in the scroll viewport above the fixed search field"
        );
    }

    #[test]
    fn typing_then_enter_launches_the_top_match_via_tile_activation() {
        // The end-to-end "open, then just type, then Enter" path: the box
        // auto-focuses on open, typed text filters live, and Enter launches the
        // top match through the SAME tile_activation seam a click uses — with
        // NO main.rs change (main.rs already drains that seam).
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2); // open + the search box auto-focuses
        drive(&ctx, &mut s, &mut console, vec![text("phones")], SZ);
        assert_eq!(
            s.search_query, "phones",
            "typing lands in the auto-focused box (no click into it)"
        );
        drive(&ctx, &mut s, &mut console, vec![key(egui::Key::Enter)], SZ);
        assert_eq!(
            s.take_tile_activation(),
            Some(Surface::Phones),
            "Enter launches the top match via the tile_activation seam"
        );
        assert!(!s.is_open(), "launching from search closes the whole menu");
        assert!(
            console.take_request().is_none(),
            "the focused search box kept the embedded Console's own Enter nav inert \
            (one Enter never both launches a result AND fires a Console row)"
        );
    }

    #[test]
    fn typing_then_enter_launches_a_console_search_hit_through_console() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);
        drive(&ctx, &mut s, &mut console, vec![text("cloud plane")], SZ);
        assert_eq!(
            search_console_labels("cloud plane").first(),
            Some(&"Cloud Plane (GUI)")
        );

        drive(&ctx, &mut s, &mut console, vec![key(egui::Key::Enter)], SZ);

        assert_eq!(
            console.take_request(),
            Some(ConsoleRequest::Plane(Plane::Cloud)),
            "Console search hits dispatch through ConsoleState, not a duplicated Start action"
        );
        assert!(
            s.take_tile_activation().is_none(),
            "Console search hits must not masquerade as app tile activations"
        );
        assert!(
            !s.is_open(),
            "a successful Console search launch closes the whole Start Menu"
        );
    }

    #[test]
    fn up_and_down_move_the_search_highlight_clamped_to_the_result_list() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2); // open + auto-focus the box
        s.search_query = "m".to_string(); // a query with many matches
        run(&ctx, &mut s, &mut console, 1); // settle the focused box's arrow filter
        assert!(
            super::search_matches("m").len() >= 3,
            "the fixture query needs >=3 matches to exercise the highlight"
        );
        assert_eq!(
            s.search_highlight, 0,
            "the highlight starts on the top match"
        );
        drive(
            &ctx,
            &mut s,
            &mut console,
            vec![key(egui::Key::ArrowDown)],
            SZ,
        );
        assert_eq!(s.search_highlight, 1, "Down advances the highlight");
        drive(
            &ctx,
            &mut s,
            &mut console,
            vec![key(egui::Key::ArrowDown)],
            SZ,
        );
        assert_eq!(s.search_highlight, 2);
        drive(
            &ctx,
            &mut s,
            &mut console,
            vec![key(egui::Key::ArrowUp)],
            SZ,
        );
        assert_eq!(s.search_highlight, 1, "Up moves back toward the top");
        drive(
            &ctx,
            &mut s,
            &mut console,
            vec![key(egui::Key::ArrowUp)],
            SZ,
        );
        drive(
            &ctx,
            &mut s,
            &mut console,
            vec![key(egui::Key::ArrowUp)],
            SZ,
        );
        assert_eq!(
            s.search_highlight, 0,
            "Up clamps at the top, never negative"
        );
    }

    #[test]
    fn clicking_a_search_result_launches_it_via_tile_activation() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);
        s.search_query = "phone".to_string();
        run(&ctx, &mut s, &mut console, 1); // register the result rows
        let rect = ctx
            .read_response(super::search_result_id(Surface::Phones))
            .expect("the Phones result row is registered while searching")
            .rect;
        click(&ctx, &mut s, &mut console, rect.center(), SZ);
        assert_eq!(
            s.take_tile_activation(),
            Some(Surface::Phones),
            "clicking a result launches it through the tile_activation seam"
        );
        assert!(
            !s.is_open(),
            "launching from a result closes the whole menu"
        );
    }

    #[test]
    fn esc_clears_a_live_query_first_then_a_second_esc_closes_the_menu() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);
        s.search_query = "phone".to_string();
        run(&ctx, &mut s, &mut console, 1);
        assert!(s.is_open() && !s.search_query.is_empty());
        // First Esc clears the query; the menu (and its embedded Console) stay
        // up — the shared-Esc Console self-closure is absorbed by the guard.
        drive(&ctx, &mut s, &mut console, vec![key(egui::Key::Escape)], SZ);
        assert!(s.search_query.is_empty(), "Esc clears a live query");
        assert!(
            s.is_open(),
            "clearing a live query does NOT dismiss the menu"
        );
        // Re-focus settles, then a second (now-empty) Esc dismisses the menu.
        run(&ctx, &mut s, &mut console, 1);
        drive(&ctx, &mut s, &mut console, vec![key(egui::Key::Escape)], SZ);
        assert!(
            !s.is_open(),
            "a second, empty-query Esc closes the whole menu"
        );
    }

    #[test]
    fn opening_the_menu_shows_the_full_grid_and_no_result_list() {
        // Empty-query invariant on the REAL render: with nothing typed, XXX (the grouped grid), and NO search
        // result row exists — the grid is untouched when not searching.
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);
        for surface in Surface::ALL {
            assert!(
                ctx.read_response(super::tile_id(surface)).is_some(),
                "{surface:?} tile renders on the empty-query grid"
            );
            assert!(
                ctx.read_response(super::search_result_id(surface))
                    .is_none(),
                "{surface:?} exports no result row while the query is empty"
            );
        }
    }

    #[test]
    fn the_search_box_exports_a_searchinput_role_and_label_for_accesskit() {
        // Lock #14 — the search box carries a proper role + label (the file's
        // label-keyed-lookup test idiom).
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);
        let search = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Start Menu search"))
            .expect("the search box exports an accesskit node");
        assert_eq!(search.role(), egui::accesskit::Role::SearchInput);
    }

    #[test]
    fn start_menu_search_hint_uses_ascii_chrome_copy() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let texts = painted_text(&out.shapes);
        assert!(
            texts.iter().any(|text| text == super::START_SEARCH_HINT),
            "the Start search hint should be visible using ASCII chrome copy: {texts:?}"
        );
        assert!(
            !texts.iter().any(|text| text.contains('…')),
            "the Start search field should not paint a Unicode ellipsis: {texts:?}"
        );
    }

    #[test]
    fn live_query_exposes_a_clear_search_icon_button() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);
        s.search_query = "phone".to_string();
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);
        let clear = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Clear Start Menu search"))
            .expect("a live query exports the clear icon button");
        assert_eq!(clear.role(), egui::accesskit::Role::Button);

        let rect = ctx
            .read_response(super::search_clear_button_id())
            .expect("the clear icon button is registered")
            .rect;
        click(&ctx, &mut s, &mut console, rect.center(), SZ);
        assert!(s.search_query.is_empty(), "clicking the icon clears search");
        assert!(s.is_open(), "clearing search keeps the Start Menu open");
    }

    #[test]
    fn app_search_result_rows_export_positioned_accesskit_buttons() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        s.search_query = "phone".to_string();
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);
        let row = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Phones"))
            .expect("Phones search result row");

        assert_eq!(row.role(), egui::accesskit::Role::Button);
        assert_eq!(row.value(), Some("Result 1 of 1: App, Comms"));
        assert_eq!(row.is_selected(), Some(true));
        assert!(row.supports_action(egui::accesskit::Action::Click));
    }

    #[test]
    fn console_search_result_rows_export_positioned_accesskit_buttons() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        s.search_query = "cloud plane".to_string();
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);
        let row = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Cloud Plane (GUI)"))
            .expect("Cloud Plane Console search result row");

        assert_eq!(row.role(), egui::accesskit::Role::Button);
        assert_eq!(
            row.value(),
            Some("Result 1 of 1: Console command, Containers & VMs")
        );
        assert_eq!(row.is_selected(), Some(true));
        assert!(row.supports_action(egui::accesskit::Action::Click));
    }

    #[test]
    fn broad_search_results_scroll_above_the_fixed_search_field() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        run(&ctx, &mut s, &mut console, 2);
        s.search_query = "e".to_string();

        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SZ);
        let panel_h = PANEL_H.min(screen.height() - Style::SP_XL);
        let panel_top = screen.bottom() - panel_h;
        let left_rect = egui::Rect::from_min_size(
            egui::pos2(screen.left(), panel_top),
            egui::vec2(super::LEFT_PANE_W, panel_h),
        );
        let search_rect = super::search_rect(left_rect);
        let content_rect = super::left_pane_content_rect(left_rect, search_rect);
        let matches = super::search_matches(&s.search_query);
        let spill_idx = ((search_rect.top() - content_rect.top() - super::PANE_PAD)
            / super::RESULT_ROW_H)
            .ceil()
            .max(0.0) as usize;
        assert!(
            matches.len() > spill_idx,
            "fixture query must produce an offscreen search result row: {} <= {spill_idx}",
            matches.len()
        );
        assert!(
            super::search_results_content_h(&matches, content_rect.height())
                > content_rect.height(),
            "broad Start search should require a scrollable result list"
        );
        s.search_highlight = spill_idx;

        let input = || egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut cap = Capture::new();
        let _settle = cap.frame(&ctx, input(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = ui.button("surface");
            });
            start_menu_panel(ctx, &mut s, &mut console, 0.0);
        });
        let canvas = cap.frame(&ctx, input(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = ui.button("surface");
            });
            start_menu_panel(ctx, &mut s, &mut console, 0.0);
        });
        assert!(
            !canvas.is_blank(),
            "the broad Start search screenshot must not be blank"
        );
        assert_eq!(canvas.count_near_color_in_rect(search_rect, Style::ACCENT, 4), 0,
            "an offscreen selected result row must not paint its accent stripe into the fixed search field"
        );

        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("screenshots")
            .join("start-menu-search-results-scroll.png");
        canvas
            .write_png(&path)
            .expect("write the bounded Start search screenshot");
        println!(
            "Start Menu search-results screenshot written to {}",
            path.display()
        );
    }

    #[test]
    fn the_result_list_exports_one_live_polite_summary_of_the_filtered_state() {
        // Lock #14 — the filtered-result state is announceable: ONE live-polite
        // summary (count + highlight), the NOTIF-11 / tile-grid live-region
        // shape restated for the result set.
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        Style::install(&ctx);
        let mut s = StartMenuState::default();
        let mut console = ConsoleState::with_store(None);
        s.toggle();
        s.search_query = "phone".to_string();
        let out = drive(&ctx, &mut s, &mut console, Vec::new(), SZ);
        let nodes = accesskit_nodes(&out);
        let region = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some("Start Menu search results"))
            .expect("a live region summarizes the filtered results");
        assert_eq!(region.role(), egui::accesskit::Role::Status);
        assert_eq!(region.live(), Some(egui::accesskit::Live::Polite));
        let value = region.value().expect("summary value");
        assert!(
            value.contains("Phones"),
            "the summary names the highlighted match: {value}"
        );
    }
}
